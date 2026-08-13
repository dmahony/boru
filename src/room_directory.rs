//! Bounded local cache of discovered public rooms (PDF Phase 4, Task 4.1).
//!
//! The **room directory** is the client-side cache of room-discovery
//! metadata maintained by the discovery/control-plane layer. It is
//! deliberately **not** conversation state: discovering a room never
//! creates a [`ConversationEntry`](crate::conversations::ConversationEntry),
//! subscribes to the room's gossip topic, downloads history, or grants any
//! permission (PDF Core rule). This module has no reference to
//! `crate::conversations` at all — the guarantee is structural.
//!
//! # What is stored
//!
//! Entries are keyed by the **stable `room_id`** (the room's gossip
//! [`TopicId`], PDF Task 4.1 step 2). Each entry holds:
//!
//! * the **latest valid advertisement** ([`PublicRoomAdvertisement`]),
//! * the **advertiser/authority identity** — the publishing node
//!   ([`DirectoryEntry::publisher`]) and the publisher-authentication
//!   verdict at receipt ([`DirectoryEntry::auth`], BORU-DIR-03),
//! * `first_seen` / `last_seen` / `expires_at` (DIR-08 TTL policy),
//! * **compatibility state** ([`DirectoryEntry::compatibility`],
//!   room protocol vs this client, refined by Phase 6 Task 6.2),
//! * **local join state** ([`DirectoryEntry::local_join_state`];
//!   defaults to [`LocalJoinState::NotJoined`] — BORU-DIR-12 derives
//!   Joined/Blocked from the real room database, never from the
//!   directory itself). The app layer feeds the facts
//!   ([`LocalRoomFacts`]) via [`RoomDirectory::sync_local_states`]:
//!   joined room ids come from the real local room database (the
//!   source of truth for Joined, never the advertisement), hidden
//!   room ids from the persisted hide preference. Hidden rooms are
//!   derived [`LocalJoinState::Blocked`] and excluded from
//!   [`RoomDirectory::snapshot`], so they stay hidden across
//!   advertisement refreshes until the preference is explicitly reset
//!   (PDF Task 4.3; BORU-DIR-20 adds the user-facing Hide/Block
//!   controls, this module only stores the state).
//!
//! # Bounds (PDF Task 4.1 step 5)
//!
//! The cache enforces a maximum entry count
//! ([`MAX_DIRECTORY_ENTRIES`]) and an aggregate metadata-size budget
//! ([`MAX_DIRECTORY_TOTAL_BYTES`]). When full, expired entries are
//! evicted first, then the least-recently-seen entry — thousands of
//! advertisements cannot grow memory without bound.
//!
//! # Deterministic replacement (PDF Task 4.1 step 6)
//!
//! Multiple advertisements for the same `room_id` collapse into **one**
//! entry (duplicates merge, never duplicate cards):
//!
//! 1. Same publisher → the later envelope (higher control-plane
//!    `sequence`) replaces the earlier one; `first_seen` is preserved and
//!    `last_seen`/`expires_at` advance (a DIR-08 refresh).
//! 2. Different publisher → a **verified authoritative** publisher (the
//!    advertisement verifies and the publisher equals `owner_peer_id`)
//!    beats a non-authority publisher, so a random peer cannot silently
//!    overwrite another room's canonical metadata (PDF Task 1.3).
//! 3. Both authoritative or both non-authoritative → the advertisement
//!    with the later envelope creation timestamp wins; ties break on the
//!    lexicographically larger publisher key. Deterministic — no
//!    HashMap-order dependence, no oscillation on stale replays.
//!
//! # Deduplication and conflicts (PDF Task 4.2, BORU-DIR-11)
//!
//! Directory results stay stable when the P2P network delivers repeated or
//! conflicting information:
//!
//! 1. **Identical duplicates** — an advertisement with the same room_id
//!    (map key), advert version, envelope sequence and content digest is a
//!    pure liveness refresh: [`AdvertiseOutcome::Duplicate`], no content
//!    change, no subscriber event. Repeated gossip can never churn the UI
//!    or create a second card.
//! 2. **Authentication gates replacement** — a verified (signed)
//!    advertisement always beats an unverified one, so an unauthenticated
//!    peer cannot trivially rename a room that a verified peer has already
//!    identified. A verified **authority** (publisher == `owner_peer_id`)
//!    is canonical and clears any conflict.
//! 3. **Conflict state** — when two different non-authority sources claim
//!    different metadata and Boru cannot prove a canonical authority, the
//!    deterministic winner is retained but the entry is flagged
//!    [`DirectoryEntry::conflict`]; the UI must show it as unverified
//!    rather than silently trusting arbitrary metadata.
//! 4. **Anti-oscillation** — once an entry is conflicted, only the winning
//!    publisher's own refresh or a verified authority may change it; a
//!    different non-authority source can no longer flip the metadata back
//!    and forth.
//!
//! # Withdrawal (BORU-DIR-09 / PDF Task 3.3)
//!
//! [`apply_withdrawal`](Self::apply_withdrawal) removes the matching entry
//! immediately. The control-plane receive gate has already verified the
//! withdrawal is signed by its claimed authority; the directory adds one
//! more guard: the withdrawal's `owner_peer_id` must match the **stored
//! entry's** `owner_peer_id`, so a withdrawal cannot remove a listing
//! owned by a different room authority. TTL expiry
//! ([`evict_expired`](Self::evict_expired)) remains the safety net when a
//! withdrawal is missed.

use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
    time::{Duration, Instant},
};

use iroh_base::PublicKey;

use crate::control_plane::advertisement::{AdvertisementAuth, PublicRoomAdvertisement};
use crate::control_plane::capabilities::{default_local_capabilities, CapabilityId, CapabilitySet};
use crate::proto::TopicId;

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// Maximum number of cached room-directory entries (PDF Task 4.1 step 5).
///
/// The directory is a **bounded cache** of discovered metadata, not a
/// database: 1024 live rooms is far beyond what any single user's
/// discovery cohort produces, while still bounding memory for a hostile
/// network that floods advertisements. When full, entries are evicted by
/// expiry first, then by least-recently-seen.
pub const MAX_DIRECTORY_ENTRIES: usize = 1024;

/// Aggregate metadata-size budget for the cache, in encoded advertisement
/// bytes (PDF Task 4.1 step 5).
///
/// Each advertisement is already bounded by the privacy layer (name,
/// description, tags, flags, TTL limits), but the aggregate cap makes the
/// total footprint explicit and testable. A single advertisement larger
/// than this budget is still cached (one room is worth one entry; the cap
/// is on aggregate growth).
pub const MAX_DIRECTORY_TOTAL_BYTES: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// Compatibility and local relationship (stored per entry)
// ---------------------------------------------------------------------------

/// Compatibility of a discovered room's chat protocol with this client
/// (PDF Task 6.2; stored per entry by BORU-DIR-10, refined by the Phase 6
/// join-flow task).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoomCompatibility {
    /// No advertisement seen yet for the room (only used for empty
    /// placeholders; every cached entry has a concrete verdict).
    Unknown,
    /// The local client can join the room (same or older room protocol).
    Compatible,
    /// The room speaks a newer protocol than this client — joining would
    /// require a client upgrade.
    UpgradeRequired,
    /// The room's protocol is not supported by this client at all.
    Unsupported,
}

impl RoomCompatibility {
    /// Deterministic compatibility verdict from the room's advertised
    /// chat protocol version (PDF Task 6.2 step 1).
    ///
    /// Formalization of the comparison:
    ///
    /// * a room speaking the **same** protocol version as this client, or
    ///   an **older** one, is joinable → [`Compatible`](Self::Compatible);
    /// * a room speaking exactly **one version newer** requires a client
    ///   upgrade → [`UpgradeRequired`](Self::UpgradeRequired) (upgrading
    ///   this client to the next protocol version makes the room usable);
    /// * a room speaking a protocol **more than one version newer** is
    ///   [`Unsupported`](Self::Unsupported) — the room's protocol has
    ///   diverged beyond the adjacent version this client can reason
    ///   about, so no simple upgrade path exists and joining is not
    ///   attempted (PDF Task 6.2 step 5).
    ///
    /// Version `0` is treated as [`Compatible`](Self::Compatible): it is
    /// the legacy "no protocol version declared" marker used by the
    /// pre-control-plane directory store, and blocking those rooms would
    /// unnecessarily break basic room access (PDF Task 6.2 acceptance:
    /// optional/unknown fields must not block a compatible base protocol).
    pub fn for_room_protocol(room_protocol_version: u8) -> Self {
        match room_protocol_version.cmp(&crate::public_room::PROTOCOL_VERSION) {
            Ordering::Equal | Ordering::Less => RoomCompatibility::Compatible,
            Ordering::Greater => {
                if room_protocol_version == crate::public_room::PROTOCOL_VERSION.saturating_add(1) {
                    RoomCompatibility::UpgradeRequired
                } else {
                    RoomCompatibility::Unsupported
                }
            }
        }
    }
}

/// Optional-feature compatibility of a discovered room with this client
/// (PDF Task 6.2 step 2: capability negotiation for optional room
/// features).
///
/// The base-room-protocol verdict ([`RoomCompatibility`]) decides whether
/// the local client can join at all. Optional features are negotiated
/// separately against the local capability set (reusing the BORU-CP
/// capability machinery): a room may advertise optional feature flags
/// (e.g. `files-v2`, `voice-v1`) that this client does not support.
///
/// **Optional feature differences never block basic room access** (PDF
/// Task 6.2 acceptance: "Optional feature differences do not
/// unnecessarily block basic room access"). The verdict here is
/// informational — the UI can surface it as a hint, but a room whose
/// base protocol is [`RoomCompatibility::Compatible`] remains joinable
/// even when some advertised optional features are missing locally.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RoomFeatureCompatibility {
    /// The room advertises no optional feature flags.
    None,
    /// Every advertised optional feature flag is supported by this
    /// client (the local capability set contains the negotiated
    /// feature-version id).
    AllSupported,
    /// Some advertised optional features are **not** supported by this
    /// client. Informational only — basic room access is unaffected;
    /// the listed flags are the feature-version ids this client lacks.
    SomeMissing(Vec<String>),
}

impl RoomFeatureCompatibility {
    /// Negotiate a room's advertised optional feature flags against the
    /// local capability set (PDF Task 6.2 step 2).
    ///
    /// Each advertised flag is a `feature-version` capability id (the
    /// BORU-CP capability format). The flag is supported when the local
    /// capability set contains the exact id. Unknown future flags are
    /// **preserved and reported as missing**, never fatal — a future
    /// client can ignore unknown optional fields without rejecting the
    /// room when the base protocol remains compatible (PDF Task 6.2 step
    /// 4).
    pub fn negotiate(local: &CapabilitySet, advertised: &[String]) -> Self {
        if advertised.is_empty() {
            return Self::None;
        }
        let missing: Vec<String> = advertised
            .iter()
            .filter(|flag| {
                CapabilityId::parse(flag)
                    .map(|id| !local.contains(&id))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        if missing.is_empty() {
            Self::AllSupported
        } else {
            Self::SomeMissing(missing)
        }
    }
}

/// Local relationship to a discovered room (PDF Task 4.3 field; the
/// derivation is BORU-DIR-12, out of scope for DIR-10).
///
/// The directory only **stores** this state — it never decides it. The
/// source of truth for `Joined`/`Blocked` is the real local room database;
/// the directory defaults every entry to [`LocalJoinState::NotJoined`] and
/// never duplicates local membership records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalJoinState {
    /// The user has not joined this room (default for every cached entry).
    NotJoined,
    /// The user has joined the room (derived from the room database).
    Joined,
    /// A join attempt is in flight.
    JoinPending,
    /// The user has hidden/blocked the room locally.
    Blocked,
    /// The room is incompatible with this client.
    Incompatible,
}

/// Facts about the local relationship to rooms, fed from the real local
/// room database (BORU-DIR-12, PDF Task 4.3).
///
/// The directory itself never consults the room database — the app layer
/// owns the source of truth (the persisted conversation store / room
/// database plus the persisted hide preference) and pushes the facts in
/// via [`RoomDirectory::sync_local_states`]. This keeps the directory a
/// pure cache: it can derive `local_join_state` per entry, but it can
/// never create, duplicate, or mutate local membership records (PDF Core
/// rule).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalRoomFacts {
    /// Room ids the user has joined, from the real local room database.
    pub joined: BTreeSet<TopicId>,
    /// Room ids with a join attempt in flight (Phase 6 join flow).
    pub pending: BTreeSet<TopicId>,
    /// Room ids the user has hidden/blocked locally (persisted
    /// preference, BORU-DIR-20 adds the user-facing controls).
    pub hidden: BTreeSet<TopicId>,
}

/// Deterministic derivation of a room's local relationship state
/// (PDF Task 4.3).
///
/// Precedence: hidden/blocked wins (a hidden room is never re-shown),
/// then joined (Open rather than Join), then a pending join, then
/// incompatibility, and finally NotJoined.
pub fn derive_local_state(
    compatibility: RoomCompatibility,
    is_joined: bool,
    is_pending: bool,
    is_hidden: bool,
) -> LocalJoinState {
    if is_hidden {
        LocalJoinState::Blocked
    } else if is_joined {
        LocalJoinState::Joined
    } else if is_pending {
        LocalJoinState::JoinPending
    } else if compatibility != RoomCompatibility::Compatible {
        LocalJoinState::Incompatible
    } else {
        LocalJoinState::NotJoined
    }
}

// ---------------------------------------------------------------------------
// Entry + outcome
// ---------------------------------------------------------------------------

/// One cached discovered room (PDF Task 4.1 step 2/3).
///
/// Fields are public read-only metadata for subscribers (the Phase 5
/// Discover Rooms UI); the private `bytes`/`digest` fields are internal
/// accounting for the aggregate size bound and deduplication, which is why
/// external code can read but not construct an entry.
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    /// The latest valid advertisement for the room.
    pub advert: PublicRoomAdvertisement,
    /// The node that published the stored advertisement (advertiser /
    /// authority identity).
    pub publisher: PublicKey,
    /// Publisher-authentication verdict at receipt (BORU-DIR-03). Only
    /// [`AdvertisementAuth::Verified`] entries are canonical metadata;
    /// [`AdvertisementAuth::MissingSignature`] entries are listed as
    /// clearly untrusted and never as canonical.
    pub auth: AdvertisementAuth,
    /// Control-plane sequence of the stored advertisement (per-publisher
    /// monotonic; the same-publisher replacement tiebreak).
    pub sequence: u64,
    /// Envelope creation timestamp of the stored advertisement (unix
    /// seconds; the cross-publisher freshness tiebreak).
    pub advertised_at_secs: u64,
    /// First time this `room_id` was seen — never reset by refreshes.
    pub first_seen: Instant,
    /// Last time a valid advertisement for this `room_id` was received.
    pub last_seen: Instant,
    /// Expiry instant: `last_seen` + the advertisement's TTL (DIR-08
    /// policy — a refresh restarts the lifetime).
    pub expires_at: Instant,
    /// Room chat-protocol compatibility with this client.
    pub compatibility: RoomCompatibility,
    /// Optional-feature compatibility with this client (PDF Task 6.2 step
    /// 2). Derived from the advertised `feature_flags` against the local
    /// capability set at apply time. Informational only — it never blocks
    /// basic room access; the base `compatibility` verdict is the join
    /// gate.
    pub feature_compat: RoomFeatureCompatibility,
    /// Local relationship state (NotJoined by default; BORU-DIR-12).
    pub local_join_state: LocalJoinState,
    /// Conflict state (BORU-DIR-11, PDF Task 4.2): `true` when different
    /// non-authority sources advertised **conflicting metadata** for this
    /// room and Boru could not prove a canonical authority. The stored
    /// `advert` is the deterministic winner, but it is contested — the UI
    /// must show the listing as unverified rather than silently trusting
    /// it. Cleared only when a verified authority advertisement replaces
    /// the entry.
    pub conflict: bool,
    /// Encoded size of `advert` (bytes), used for the aggregate size bound.
    bytes: usize,
    /// Content digest of `advert` (blake3 over the signature-stripped
    /// postcard payload) — the dedup identity (BORU-DIR-11).
    digest: [u8; 32],
}

/// Outcome of applying an advertisement to the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertiseOutcome {
    /// A genuinely new room was added to the directory.
    Added,
    /// An existing entry was refreshed or replaced — the room was already
    /// cached, so this is a merge, never a second card. The stored content
    /// (or its trust tier) changed; the entry may carry a conflict flag.
    Refreshed,
    /// The advertisement was byte-identical to the cached one (same room_id
    /// + advert version + envelope sequence + content digest) — a pure
    /// liveness refresh with no content change and no subscriber event
    /// (PDF Task 4.2: repeated gossip must not churn the UI).
    Duplicate,
    /// Conflicting metadata from a different non-authority source was
    /// received for a room whose canonical authority is unproven. The
    /// deterministic winner is retained (existing content kept) and the
    /// entry is flagged [`DirectoryEntry::conflict`]; the incoming
    /// advertisement is **not** stored.
    Conflict,
    /// The advertisement was a deterministic no-op (an older, less
    /// authoritative, or anti-oscillation-rejected advertisement for a
    /// cached room) — nothing changed.
    Unchanged,
}

/// The action the Discover Rooms UI should offer for a cached room
/// (PDF Task 5.2). Derived from the entry's [`LocalJoinState`] plus its
/// [`RoomCompatibility`]; BORU-DIR-12 exposes this so the future UI
/// layer shows **Open** for already-joined rooms instead of **Join**
/// (PDF Task 4.3 step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoomAction {
    /// Offer a Join button — the room is discoverable and the user has
    /// not joined it.
    Join,
    /// Offer an Open button — the user has already joined this room.
    Open,
    /// Never render — the room is locally hidden/blocked (do not
    /// re-show unless the preference is reset).
    Hidden,
    /// Render as incompatible — joining is blocked/explained (Phase 6
    /// Task 6.2).
    Incompatible,
}

impl DirectoryEntry {
    /// The browse action for this room (PDF Task 4.3 step 4: a joined
    /// room shows Open, never Join).
    pub fn offered_action(&self) -> RoomAction {
        match self.local_join_state {
            LocalJoinState::Joined => RoomAction::Open,
            LocalJoinState::Blocked => RoomAction::Hidden,
            LocalJoinState::Incompatible => RoomAction::Incompatible,
            LocalJoinState::NotJoined | LocalJoinState::JoinPending => RoomAction::Join,
        }
    }
}

// ---------------------------------------------------------------------------
// RoomDirectory
// ---------------------------------------------------------------------------

/// Bounded cache of discovered public rooms, keyed by stable `room_id`.
///
/// Owned by the discovery/control-plane layer: the receive path
/// ([`crate::discovery_service::DiscoveryService`]) maintains it as
/// PUBLIC_ROOM_ADVERTISEMENT / PUBLIC_ROOM_WITHDRAWAL envelopes arrive, and
/// subscribers read snapshots. It never creates conversation records,
/// subscribes to room topics, or grants permissions (PDF Core rule).
#[derive(Debug)]
pub struct RoomDirectory {
    /// Entries keyed by stable room_id (the room's gossip topic bytes).
    entries: HashMap<TopicId, DirectoryEntry>,
    /// Sum of `bytes` over all entries (aggregate metadata-size bound).
    total_bytes: usize,
    /// Maximum entry count (injectable for tests).
    max_entries: usize,
    /// Maximum aggregate metadata bytes (injectable for tests).
    max_bytes: usize,
    /// Local relationship facts (BORU-DIR-12, PDF Task 4.3): joined /
    /// pending / hidden room ids fed from the real local room database.
    /// Used to derive each entry's `local_join_state`; the directory
    /// never creates or duplicates membership records itself.
    local_facts: LocalRoomFacts,
    /// The local capability set this client advertises (BORU-CP-11).
    /// Used to negotiate optional room features (PDF Task 6.2 step 2):
    /// each entry's `feature_compat` is derived by comparing the room's
    /// advertised `feature_flags` against this set. Defaults to
    /// [`default_local_capabilities`]; the discovery service replaces it
    /// when the app updates the local capability set.
    local_capabilities: CapabilitySet,
}

impl RoomDirectory {
    /// Create an empty directory with the default bounds.
    pub fn new() -> Self {
        Self::with_limits(MAX_DIRECTORY_ENTRIES, MAX_DIRECTORY_TOTAL_BYTES)
    }

    /// Create an empty directory with explicit bounds (tests use small
    /// limits to exercise the eviction paths cheaply).
    pub fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            max_entries,
            max_bytes,
            local_facts: LocalRoomFacts::default(),
            local_capabilities: default_local_capabilities(),
        }
    }

    /// Replace the local capability set used to negotiate optional room
    /// features (PDF Task 6.2 step 2) and re-derive every cached entry's
    /// `feature_compat`.
    ///
    /// The set should match what this client advertises on the control
    /// plane (BORU-CP-11) — the app updates it via the discovery service
    /// when locally enabled capabilities materially change. Entries added
    /// after this call are derived immediately.
    pub fn set_local_capabilities(&mut self, capabilities: CapabilitySet) {
        self.local_capabilities = capabilities;
        for entry in self.entries.values_mut() {
            entry.feature_compat = RoomFeatureCompatibility::negotiate(
                &self.local_capabilities,
                &entry.advert.feature_flags,
            );
        }
    }

    /// The local capability set currently used for optional-feature
    /// negotiation.
    pub fn local_capabilities(&self) -> &CapabilitySet {
        &self.local_capabilities
    }

    /// Replace the local relationship facts and re-derive every cached
    /// entry's `local_join_state` (BORU-DIR-12, PDF Task 4.3).
    ///
    /// `facts.joined` must come from the real local room database (the
    /// source of truth for `Joined` — never from the advertisement);
    /// `facts.hidden` carries the persisted hide preference so hidden
    /// rooms stay hidden across advertisement refreshes and restarts.
    /// The directory stores the facts so entries added *after* this call
    /// (e.g. a hidden room re-advertised later) are derived immediately.
    pub fn sync_local_states(&mut self, facts: LocalRoomFacts) {
        self.local_facts = facts;
        for (room_id, entry) in self.entries.iter_mut() {
            entry.local_join_state = derive_local_state(
                entry.compatibility,
                self.local_facts.joined.contains(room_id),
                self.local_facts.pending.contains(room_id),
                self.local_facts.hidden.contains(room_id),
            );
        }
    }

    /// The currently-known local relationship facts.
    pub fn local_facts(&self) -> &LocalRoomFacts {
        &self.local_facts
    }

    fn derive_for(&self, room_id: &TopicId, compatibility: RoomCompatibility) -> LocalJoinState {
        derive_local_state(
            compatibility,
            self.local_facts.joined.contains(room_id),
            self.local_facts.pending.contains(room_id),
            self.local_facts.hidden.contains(room_id),
        )
    }

    /// Apply a decoded, bounded room advertisement (BORU-DIR-01/02) to the
    /// cache (PDF Task 4.1).
    ///
    /// `publisher` is the control-plane envelope's sender, `auth` the
    /// publisher-authentication verdict (BORU-DIR-03), `sequence` the
    /// envelope's per-publisher monotonic counter, and `timestamp_secs`
    /// the envelope's creation time. The advertisement must already have
    /// passed the receive gate (well-formed, bounded, attributed);
    /// [`AdvertisementAuth::InvalidSignature`] advertisements never reach
    /// this method.
    ///
    /// Returns [`AdvertiseOutcome`] describing whether the cache gained a
    /// new room, refreshed an existing one, or left the entry unchanged.
    pub fn apply_advertisement(
        &mut self,
        advert: PublicRoomAdvertisement,
        publisher: PublicKey,
        auth: AdvertisementAuth,
        sequence: u64,
        timestamp_secs: u64,
    ) -> AdvertiseOutcome {
        self.apply_advertisement_at(
            advert,
            publisher,
            auth,
            sequence,
            timestamp_secs,
            Instant::now(),
        )
    }

    /// [`apply_advertisement`](Self::apply_advertisement) with an explicit
    /// `now` — deterministic-time core used by tests.
    pub fn apply_advertisement_at(
        &mut self,
        advert: PublicRoomAdvertisement,
        publisher: PublicKey,
        auth: AdvertisementAuth,
        sequence: u64,
        timestamp_secs: u64,
        now: Instant,
    ) -> AdvertiseOutcome {
        let room_id = advert.room_id;
        let bytes = encoded_size(&advert);
        let ttl = Duration::from_secs(u64::from(advert.expires_after_secs.max(1)));
        let compatibility = RoomCompatibility::for_room_protocol(advert.room_protocol_version);
        // PDF Task 6.2 step 2: negotiate optional room features against
        // the local capability set. Informational only — never a join
        // gate; the base `compatibility` verdict above is the gate.
        let feature_compat =
            RoomFeatureCompatibility::negotiate(&self.local_capabilities, &advert.feature_flags);
        // BORU-DIR-12: derive the local relationship state once, before
        // the entries map is mutably borrowed (Joined/hidden come from
        // the local room DB facts, never from the advertisement).
        let derived_local_state = self.derive_for(&room_id, compatibility);

        match self.entries.get_mut(&room_id) {
            Some(existing) => {
                let incoming_digest = content_digest(&advert);
                match decide_update(
                    existing,
                    &advert,
                    &publisher,
                    &auth,
                    sequence,
                    timestamp_secs,
                    &incoming_digest,
                ) {
                    UpdateDecision::Duplicate => {
                        // Identical advertisement (same room_id + advert
                        // version + envelope sequence + content digest) —
                        // a pure liveness refresh (the advertiser is alive)
                        // with no content change and no subscriber event.
                        existing.last_seen = now;
                        existing.expires_at = now + ttl;
                        AdvertiseOutcome::Duplicate
                    }
                    UpdateDecision::Keep { conflict } => {
                        if conflict && !existing.conflict {
                            // A different non-authority source claimed
                            // conflicting metadata and the deterministic
                            // winner (the existing content) was retained:
                            // flag the entry as conflicted. The incoming
                            // advertisement is NOT stored (PDF Task 4.2
                            // step 3 — do not silently trust it).
                            existing.conflict = true;
                            existing.last_seen = now;
                            existing.expires_at = now + ttl;
                            AdvertiseOutcome::Conflict
                        } else {
                            AdvertiseOutcome::Unchanged
                        }
                    }
                    UpdateDecision::Replace { conflict } => {
                        let old_bytes = existing.bytes;
                        existing.advert = advert;
                        existing.publisher = publisher;
                        existing.auth = auth;
                        existing.sequence = sequence;
                        existing.advertised_at_secs = timestamp_secs;
                        existing.last_seen = now;
                        existing.expires_at = now + ttl;
                        existing.compatibility = compatibility;
                        existing.feature_compat = feature_compat;
                        existing.conflict = conflict;
                        existing.digest = incoming_digest;
                        existing.bytes = bytes;
                        // BORU-DIR-12: re-derive the local relationship
                        // state — a refresh may change compatibility
                        // (e.g. a newer protocol version) but never the
                        // facts themselves (Joined/hidden come from the
                        // local room DB, never the advertisement).
                        existing.local_join_state = derived_local_state;
                        self.total_bytes = self
                            .total_bytes
                            .saturating_sub(old_bytes)
                            .saturating_add(bytes);
                        AdvertiseOutcome::Refreshed
                    }
                }
            }
            None => {
                // New room: enforce the bounds before inserting.
                let digest = content_digest(&advert);
                self.evict_expired_at(now);
                while self.entries.len() >= self.max_entries
                    || self.total_bytes.saturating_add(bytes) > self.max_bytes
                {
                    // Deterministic victim: earliest expiry, then oldest
                    // last_seen (least-recently-seen eviction).
                    let victim = self
                        .entries
                        .iter()
                        .min_by_key(|(_, entry)| (entry.expires_at, entry.last_seen))
                        .map(|(room_id, _)| *room_id);
                    let Some(victim) = victim else { break };
                    let removed = self.entries.remove(&victim);
                    if let Some(entry) = removed {
                        self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
                    }
                }
                let entry = DirectoryEntry {
                    advert,
                    publisher,
                    auth,
                    sequence,
                    advertised_at_secs: timestamp_secs,
                    first_seen: now,
                    last_seen: now,
                    expires_at: now + ttl,
                    compatibility,
                    feature_compat,
                    // BORU-DIR-12: derive the local relationship state at
                    // insert from the stored facts — a hidden room that is
                    // re-advertised after eviction stays hidden, a joined
                    // room is never offered Join.
                    local_join_state: derived_local_state,
                    conflict: false,
                    bytes,
                    digest,
                };
                self.entries.insert(room_id, entry);
                self.total_bytes = self.total_bytes.saturating_add(bytes);
                AdvertiseOutcome::Added
            }
        }
    }

    /// Apply a verified, authoritative room withdrawal (BORU-DIR-09, PDF
    /// Task 3.3) — remove the matching entry immediately.
    ///
    /// The control-plane receive gate has already verified the withdrawal
    /// is signed by its claimed authority (`sender_node_id ==
    /// owner_peer_id`). The directory adds one more guard: the withdrawal's
    /// `authority` must match the **stored entry's** `owner_peer_id`, so a
    /// withdrawal claimed by a different room authority can never remove an
    /// unrelated listing (PDF test matrix: "Spoofed withdrawals cannot
    /// remove unrelated rooms"). TTL expiry
    /// ([`evict_expired`](Self::evict_expired)) remains the safety net if a
    /// withdrawal is missed.
    ///
    /// Returns `true` when an entry was actually removed.
    pub fn apply_withdrawal(&mut self, room_id: TopicId, authority: [u8; 32]) -> bool {
        let matches = self
            .entries
            .get(&room_id)
            .is_some_and(|entry| entry.advert.owner_peer_id == authority);
        if matches {
            if let Some(entry) = self.entries.remove(&room_id) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
        }
        matches
    }

    /// Remove entries whose TTL has elapsed since the last valid refresh
    /// (DIR-08 policy; PDF Task 3.2 step 4). Returns the evicted room ids.
    pub fn evict_expired(&mut self) -> Vec<TopicId> {
        self.evict_expired_at(Instant::now())
    }

    /// [`evict_expired`](Self::evict_expired) with an explicit `now` —
    /// deterministic-time core used by tests.
    pub fn evict_expired_at(&mut self, now: Instant) -> Vec<TopicId> {
        let expired: Vec<TopicId> = self
            .entries
            .iter()
            .filter(|(_, entry)| now >= entry.expires_at)
            .map(|(room_id, _)| *room_id)
            .collect();
        for room_id in &expired {
            if let Some(entry) = self.entries.remove(room_id) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
        }
        expired
    }

    /// Read one cached room.
    pub fn get(&self, room_id: &TopicId) -> Option<&DirectoryEntry> {
        self.entries.get(room_id)
    }

    /// Whether a room is currently cached.
    pub fn contains(&self, room_id: &TopicId) -> bool {
        self.entries.contains_key(room_id)
    }

    /// Deterministic snapshot of the **browse surface** — all cached
    /// entries that should be offered to the user, sorted by room_id.
    ///
    /// Locally hidden/blocked rooms are excluded (BORU-DIR-12, PDF Task
    /// 4.3: do not re-show hidden rooms unless the user explicitly resets
    /// that preference). Diagnostics that need the full cache including
    /// hidden rooms use [`snapshot_all`](Self::snapshot_all).
    ///
    /// Ordering is deterministic so the UI never churns on map iteration
    /// order.
    pub fn snapshot(&self) -> Vec<DirectoryEntry> {
        let mut rooms: Vec<DirectoryEntry> = self
            .entries
            .values()
            .filter(|e| e.local_join_state != LocalJoinState::Blocked)
            .cloned()
            .collect();
        rooms.sort_by(|a, b| a.advert.room_id.as_bytes().cmp(b.advert.room_id.as_bytes()));
        rooms
    }

    /// Deterministic snapshot of **all** cached entries including
    /// locally hidden/blocked rooms (for diagnostics / the Phase 8
    /// directory view). Same ordering as [`snapshot`](Self::snapshot).
    pub fn snapshot_all(&self) -> Vec<DirectoryEntry> {
        let mut rooms: Vec<DirectoryEntry> = self.entries.values().cloned().collect();
        rooms.sort_by(|a, b| a.advert.room_id.as_bytes().cmp(b.advert.room_id.as_bytes()));
        rooms
    }

    /// Number of cached rooms.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Aggregate encoded metadata size in bytes (for the size bound).
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// The maximum entry count this cache enforces.
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }
}

impl Default for RoomDirectory {
    fn default() -> Self {
        Self::new()
    }
}

/// Encoded size of an advertisement (infallible for in-memory values).
fn encoded_size(advert: &PublicRoomAdvertisement) -> usize {
    postcard::to_stdvec(advert)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

/// Content digest of an advertisement — the dedup identity for BORU-DIR-11
/// (PDF Task 4.2 step 1).
///
/// blake3 over the postcard encoding of the advertisement with the
/// publisher signature stripped. The signature is publisher-specific
/// (provenance, not content), so two members endorsing the *same* room
/// metadata produce the same digest and are deduplicated rather than
/// treated as conflicting claims.
fn content_digest(advert: &PublicRoomAdvertisement) -> [u8; 32] {
    let mut stripped = advert.clone();
    stripped.signature = None;
    let bytes = postcard::to_stdvec(&stripped).unwrap_or_default();
    *blake3::hash(&bytes).as_bytes()
}

/// What to do with an incoming advertisement for an already-cached room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateDecision {
    /// The advertisement is an exact duplicate (same room_id + advert
    /// version + envelope sequence + content digest) — nothing changes
    /// except liveness timestamps; no subscriber event.
    Duplicate,
    /// Keep the existing entry. `conflict` is the *desired* conflict flag:
    /// when `true` and the entry is not already flagged, the caller marks
    /// it conflicted (a conflicting non-authority claim was seen but the
    /// deterministic winner was retained).
    Keep { conflict: bool },
    /// Replace the existing entry with the incoming advertisement; the
    /// stored entry carries `conflict` as the new conflict flag.
    Replace { conflict: bool },
}

/// Deterministic update decision for multiple advertisements of the same
/// room (PDF Task 4.1 step 6 + Task 4.2). See the module docs for the full
/// ordering: deduplicate identical ads → prefer a verified authority →
/// authentication-gate replacements (verified beats unverified) → retain a
/// conflict state when non-authority sources disagree → anti-oscillation
/// once conflicted.
fn decide_update(
    existing: &DirectoryEntry,
    incoming: &PublicRoomAdvertisement,
    incoming_publisher: &PublicKey,
    incoming_auth: &AdvertisementAuth,
    incoming_sequence: u64,
    incoming_timestamp_secs: u64,
    incoming_digest: &[u8; 32],
) -> UpdateDecision {
    // ── 1. Identical advertisement (PDF Task 4.2 step 1) ────────────────
    // Same publisher, same envelope sequence, same advert version, same
    // content digest → a pure replay of the same gossip. Never a change,
    // never a conflict, never a UI event.
    if existing.publisher == *incoming_publisher
        && existing.sequence == incoming_sequence
        && existing.advert.advert_version == incoming.advert_version
        && existing.digest == *incoming_digest
    {
        return UpdateDecision::Duplicate;
    }

    // ── 2. Authority classification (PDF Task 4.2 step 2) ───────────────
    // A verified authority (publisher == owner_peer_id) beats a
    // non-authority listing: it establishes canonical metadata and clears
    // any conflict state. The reverse is never allowed — a random peer
    // cannot silently overwrite another room's canonical metadata.
    let incoming_is_authority =
        incoming_auth.is_verified() && incoming.is_authoritative_publisher(incoming_publisher);
    let existing_is_authority = existing.auth.is_verified()
        && existing
            .advert
            .is_authoritative_publisher(&existing.publisher);
    match (incoming_is_authority, existing_is_authority) {
        (true, false) => return UpdateDecision::Replace { conflict: false },
        (false, true) => {
            return UpdateDecision::Keep {
                conflict: existing.conflict,
            };
        }
        _ => {}
    }

    // ── 3. Same advertiser refresh (BORU-DIR-08) ────────────────────────
    // The same publisher re-broadcasts with a higher sequence: a refresh.
    // A same-source update is never a *new* conflict — it keeps whatever
    // conflict state the entry already has.
    if existing.publisher == *incoming_publisher {
        if incoming_sequence < existing.sequence {
            // Stale replay: cannot downgrade the cached metadata.
            return UpdateDecision::Keep {
                conflict: existing.conflict,
            };
        }
        return UpdateDecision::Replace {
            conflict: existing.conflict,
        };
    }

    // ── 4. Authentication gate (PDF Task 4.2 step 4) ────────────────────
    // An untrusted (missing-signature) advertisement can never replace a
    // verified one: an unauthenticated peer cannot rename a room that a
    // verified peer has identified.
    if existing.auth.is_verified() && !incoming_auth.is_verified() {
        return UpdateDecision::Keep {
            conflict: existing.conflict,
        };
    }
    // A verified advertisement may replace an unverified one (trust
    // upgrade). If the metadata differs and no authority is proven, this is
    // still a conflict — two sources disagree about the room.
    if !existing.auth.is_verified() && incoming_auth.is_verified() {
        let conflict = existing.digest != *incoming_digest;
        return UpdateDecision::Replace { conflict };
    }

    // ── 5. Same authority class, different advertisers ──────────────────
    // Identical content from a different publisher is an endorsement of the
    // same metadata — deduplicate, not a conflict.
    if existing.digest == *incoming_digest {
        return UpdateDecision::Duplicate;
    }

    // Conflicting metadata. When BOTH are verified authorities (two owners
    // claiming the same room_id), each claim is canonical for its own
    // publisher: the newer envelope wins deterministically (DIR-10 rule)
    // without a conflict flag — matches the pre-existing replacement
    // semantics.
    if incoming_is_authority && existing_is_authority {
        if incoming_timestamp_secs != existing.advertised_at_secs {
            if incoming_timestamp_secs > existing.advertised_at_secs {
                return UpdateDecision::Replace { conflict: false };
            }
            return UpdateDecision::Keep {
                conflict: existing.conflict,
            };
        }
        if incoming_publisher.as_bytes() > existing.publisher.as_bytes() {
            return UpdateDecision::Replace { conflict: false };
        }
        return UpdateDecision::Keep {
            conflict: existing.conflict,
        };
    }

    // Both non-authority: conflicting metadata with no canonical authority
    // (PDF Task 4.2 step 3). If the entry is already conflicted, a different
    // non-authority source cannot flip-flop it (anti-oscillation, step 4):
    // only the winning publisher's own refresh (handled above) or a verified
    // authority (handled above) may change it.
    if existing.conflict {
        return UpdateDecision::Keep {
            conflict: existing.conflict,
        };
    }

    // First disagreement: deterministic winner — newer envelope timestamp
    // wins, then the lexicographically larger publisher key. Whichever
    // wins, the entry is now conflicted because Boru cannot prove which
    // non-authority claim is canonical.
    if incoming_timestamp_secs != existing.advertised_at_secs {
        if incoming_timestamp_secs > existing.advertised_at_secs {
            return UpdateDecision::Replace { conflict: true };
        }
        return UpdateDecision::Keep { conflict: true };
    }
    if incoming_publisher.as_bytes() > existing.publisher.as_bytes() {
        return UpdateDecision::Replace { conflict: true };
    }
    UpdateDecision::Keep { conflict: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    fn secret_key(byte: u8) -> iroh_base::SecretKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed)
    }

    fn ad(room_id: TopicId, owner_byte: u8, name: &str) -> PublicRoomAdvertisement {
        PublicRoomAdvertisement::minimal(
            room_id,
            name.to_string(),
            key(owner_byte).as_bytes().to_owned(),
        )
    }

    fn ad_named(room_id: TopicId, owner_byte: u8, name: &str, ttl: u32) -> PublicRoomAdvertisement {
        let mut a = ad(room_id, owner_byte, name);
        a.expires_after_secs = ttl;
        a
    }

    fn verified_auth(publisher: PublicKey) -> AdvertisementAuth {
        AdvertisementAuth::Verified { publisher }
    }

    fn t0() -> Instant {
        Instant::now()
    }

    // ── Basics + duplicate merge (PDF Task 4.1 acceptance) ────────────

    #[test]
    fn new_directory_is_empty() {
        let dir = RoomDirectory::new();
        assert!(dir.is_empty());
        assert_eq!(dir.len(), 0);
        assert_eq!(dir.total_bytes(), 0);
    }

    /// Duplicate advertisements merge rather than create duplicate cards:
    /// the same room advertised twice (refresh) yields one entry.
    #[test]
    fn duplicate_advertisements_merge_into_one_entry() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);

        let first =
            dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 1, 1000);
        assert_eq!(first, AdvertiseOutcome::Added);
        assert_eq!(dir.len(), 1);

        // Same publisher re-broadcasts with a higher sequence (refresh).
        let second =
            dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 2, 1060);
        assert_eq!(second, AdvertiseOutcome::Refreshed);
        assert_eq!(dir.len(), 1, "refresh must not create a second card");

        // A stale replay (lower sequence) is a deterministic no-op.
        let stale =
            dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 1, 900);
        assert_eq!(stale, AdvertiseOutcome::Unchanged);
        assert_eq!(dir.len(), 1);
        assert_eq!(
            dir.get(&room).unwrap().sequence,
            2,
            "stale replay cannot downgrade"
        );
    }

    /// A refresh updates last_seen/expiry but preserves first_seen.
    #[test]
    fn refresh_preserves_first_seen_updates_last_seen() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let now = t0();

        dir.apply_advertisement_at(
            ad_named(room, 0x42, "room", 300),
            owner,
            verified_auth(owner),
            1,
            1000,
            now,
        );
        let entry = dir.get(&room).unwrap();
        let first_seen = entry.first_seen;
        let expires_at = entry.expires_at;

        dir.apply_advertisement_at(
            ad_named(room, 0x42, "room", 300),
            owner,
            verified_auth(owner),
            2,
            1060,
            now + Duration::from_secs(10),
        );
        let entry = dir.get(&room).unwrap();
        assert_eq!(entry.first_seen, first_seen, "first_seen is sticky");
        assert_eq!(entry.last_seen, now + Duration::from_secs(10));
        assert!(entry.expires_at > expires_at, "expiry restarts on refresh");
        assert_eq!(dir.len(), 1);
    }

    // ── Deterministic replacement (PDF Task 4.1 step 6) ───────────────

    /// A verified authoritative publisher replaces a non-authority
    /// listing; the non-authority cannot bounce the authority back.
    #[test]
    fn authority_publisher_wins_over_endorsement() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let endorser = key(0x43);

        // A member endorses the room (ad claims owner 0x42, signed by the
        // member → non-authority, but verified for the publisher).
        let mut endorsement = ad(room, 0x42, "room");
        endorsement.sign(&secret_key(0x43));
        let first =
            dir.apply_advertisement(endorsement, endorser, verified_auth(endorser), 1, 1000);
        assert_eq!(first, AdvertiseOutcome::Added);
        assert_eq!(dir.get(&room).unwrap().publisher, endorser);

        // The authority publishes the canonical advertisement — replaces
        // the endorsement.
        let mut canonical = ad(room, 0x42, "room");
        canonical.sign(&secret_key(0x42));
        let second = dir.apply_advertisement(canonical, owner, verified_auth(owner), 5, 1100);
        assert_eq!(second, AdvertiseOutcome::Refreshed);
        assert_eq!(dir.get(&room).unwrap().publisher, owner);
        assert_eq!(dir.len(), 1);

        // A later non-authority endorsement cannot overwrite canonical.
        let mut late_endorsement = ad(room, 0x42, "room");
        late_endorsement.sign(&secret_key(0x44));
        let third = dir.apply_advertisement(
            late_endorsement,
            key(0x44),
            verified_auth(key(0x44)),
            9,
            2000,
        );
        assert_eq!(
            third,
            AdvertiseOutcome::Unchanged,
            "non-authority cannot overwrite canonical"
        );
        assert_eq!(dir.get(&room).unwrap().publisher, owner);
    }

    /// Between two non-authority advertisements the newer envelope wins,
    /// and an older replay cannot bounce the entry back.
    #[test]
    fn newer_non_authority_replaces_older_deterministically() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let a = key(0x41);
        let b = key(0x42);

        let first = dir.apply_advertisement(ad(room, 0x41, "room-a"), a, verified_auth(a), 1, 1000);
        assert_eq!(first, AdvertiseOutcome::Added);
        assert_eq!(dir.get(&room).unwrap().publisher, a);

        let second =
            dir.apply_advertisement(ad(room, 0x42, "room-b"), b, verified_auth(b), 1, 2000);
        assert_eq!(second, AdvertiseOutcome::Refreshed, "newer envelope wins");
        assert_eq!(dir.get(&room).unwrap().publisher, b);
        assert_eq!(dir.len(), 1);

        let stale =
            dir.apply_advertisement(ad(room, 0x41, "room-a-old"), a, verified_auth(a), 2, 1500);
        assert_eq!(
            stale,
            AdvertiseOutcome::Unchanged,
            "older replay cannot bounce back"
        );
        assert_eq!(dir.get(&room).unwrap().publisher, b);
    }

    /// An untrusted (missing-signature) advertisement is cached but never
    /// treated as canonical: a verified authority still wins over it.
    #[test]
    fn missing_signature_is_untrusted_not_canonical() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let stranger = key(0x55);

        let first = dir.apply_advertisement(
            ad(room, 0x55, "spoof"),
            stranger,
            AdvertisementAuth::MissingSignature,
            1,
            1000,
        );
        assert_eq!(first, AdvertiseOutcome::Added);
        assert_eq!(
            dir.get(&room).unwrap().auth,
            AdvertisementAuth::MissingSignature,
            "untrusted ad stored as untrusted"
        );

        // The room's real owner (verified) replaces it.
        let owner = key(0x42);
        let mut canonical = ad(room, 0x42, "real-room");
        canonical.sign(&secret_key(0x42));
        let second = dir.apply_advertisement(canonical, owner, verified_auth(owner), 1, 1500);
        assert_eq!(second, AdvertiseOutcome::Refreshed);
        assert_eq!(dir.get(&room).unwrap().publisher, owner);
    }

    // ── Bounds (PDF Task 4.1 step 5) ──────────────────────────────────

    /// Thousands of advertisements cannot grow memory without bound: the
    /// entry count is capped at `max_entries`, and re-advertising the same
    /// rooms never grows the cache.
    #[test]
    fn thousands_of_advertisements_are_bounded() {
        let mut dir = RoomDirectory::with_limits(64, usize::MAX);

        // 10,000 distinct rooms → capped at 64 entries.
        for i in 0..10_000u16 {
            let room = TopicId::from_bytes(i.to_le_bytes().repeat(16).try_into().unwrap());
            let owner = key((i % 250) as u8);
            dir.apply_advertisement(
                ad(room, (i % 250) as u8, "room"),
                owner,
                verified_auth(owner),
                1,
                1000,
            );
        }
        assert_eq!(dir.len(), 64, "entry count is bounded");
        assert!(
            dir.total_bytes() < usize::MAX,
            "aggregate metadata stays bounded"
        );

        // Re-advertising the same 10,000 rooms cannot grow the cache either.
        for i in 0..10_000u16 {
            let room = TopicId::from_bytes(i.to_le_bytes().repeat(16).try_into().unwrap());
            let owner = key((i % 250) as u8);
            dir.apply_advertisement(
                ad(room, (i % 250) as u8, "room"),
                owner,
                verified_auth(owner),
                2,
                2000,
            );
        }
        assert_eq!(dir.len(), 64);
    }

    /// The aggregate metadata-size budget is enforced: when the cache would
    /// exceed `max_bytes`, entries are evicted (expired first, then
    /// least-recently-seen) until the budget fits.
    #[test]
    fn aggregate_metadata_size_is_bounded() {
        // Each minimal ad encodes to ~50-90 bytes; a 300-byte budget fits
        // only a handful of entries.
        let mut dir = RoomDirectory::with_limits(usize::MAX, 300);
        let owner = key(0x42);

        for i in 0..50u8 {
            let room = TopicId::from_bytes([i; 32]);
            dir.apply_advertisement(
                ad(room, 0x42, "room"),
                owner,
                verified_auth(owner),
                u64::from(i),
                1000,
            );
        }
        assert!(
            dir.len() < 50,
            "byte budget caps the cache before entry count does"
        );
        assert!(
            dir.total_bytes() <= 300 + 200,
            "aggregate stays near the budget (one-entry overshoot allowed)"
        );
    }

    /// Eviction prefers expired entries first, then the least-recently-seen
    /// entry.
    #[test]
    fn eviction_prefers_expired_then_least_recently_seen() {
        let mut dir = RoomDirectory::with_limits(2, usize::MAX);
        let now = t0();
        let owner = key(0x42);

        // Room A: 1-second TTL, seen at t0.
        dir.apply_advertisement_at(
            ad_named(topic(1), 0x42, "a", 1),
            owner,
            verified_auth(owner),
            1,
            1000,
            now,
        );
        // Room B: long TTL, seen at t0 + 1s.
        dir.apply_advertisement_at(
            ad_named(topic(2), 0x42, "b", 300),
            owner,
            verified_auth(owner),
            1,
            1000,
            now + Duration::from_secs(1),
        );

        // A third room arrives at t0 + 2s: A has expired → evicted first,
        // making room without touching B (which is older last_seen).
        dir.apply_advertisement_at(
            ad_named(topic(3), 0x42, "c", 300),
            owner,
            verified_auth(owner),
            1,
            1000,
            now + Duration::from_secs(2),
        );
        assert_eq!(dir.len(), 2);
        assert!(dir.contains(&topic(2)), "B survives (A was expired)");
        assert!(dir.contains(&topic(3)));
        assert!(!dir.contains(&topic(1)), "expired A evicted");

        // Fill to capacity again: the next insert evicts the least-recently-
        // seen live entry (B, seen earlier than C).
        dir.apply_advertisement_at(
            ad_named(topic(4), 0x42, "d", 300),
            owner,
            verified_auth(owner),
            1,
            1000,
            now + Duration::from_secs(3),
        );
        assert_eq!(dir.len(), 2);
        assert!(dir.contains(&topic(3)), "C survives (more recently seen)");
        assert!(dir.contains(&topic(4)));
        assert!(!dir.contains(&topic(2)), "least-recently-seen B evicted");
    }

    // ── Withdrawal (BORU-DIR-09 / PDF Task 3.3) ───────────────────────

    /// A verified withdrawal removes the matching entry immediately.
    #[test]
    fn withdrawal_removes_matching_entry() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);

        dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 1, 1000);
        assert!(dir.contains(&room));

        assert!(dir.apply_withdrawal(room, owner.as_bytes().to_owned()));
        assert!(!dir.contains(&room));
        assert!(dir.is_empty());
        // Idempotent: a second withdrawal is a no-op.
        assert!(!dir.apply_withdrawal(room, owner.as_bytes().to_owned()));
    }

    /// A withdrawal claimed by a different authority cannot remove an
    /// unrelated listing (PDF test matrix: spoofed withdrawals).
    #[test]
    fn withdrawal_from_different_authority_does_not_remove() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let other_owner = key(0x43);

        dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 1, 1000);
        assert!(!dir.apply_withdrawal(room, other_owner.as_bytes().to_owned()));
        assert!(
            dir.contains(&room),
            "unrelated authority cannot remove the listing"
        );
    }

    /// Withdrawing an unknown room is a no-op.
    #[test]
    fn withdrawal_of_unknown_room_is_noop() {
        let mut dir = RoomDirectory::new();
        let owner = key(0x42);
        assert!(!dir.apply_withdrawal(topic(9), owner.as_bytes().to_owned()));
    }

    // ── TTL expiry (DIR-08 safety net) ────────────────────────────────

    /// An advertisement whose TTL elapses without a refresh is evicted —
    /// a room whose advertiser disappears eventually leaves the directory
    /// even if no withdrawal ever arrives.
    #[test]
    fn ttl_expiry_evicts_stale_room() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let now = t0();

        dir.apply_advertisement_at(
            ad_named(room, 0x42, "stale", 1),
            owner,
            verified_auth(owner),
            1,
            1000,
            now,
        );
        assert!(dir.contains(&room));

        let evicted = dir.evict_expired_at(now + Duration::from_secs(2));
        assert_eq!(evicted, vec![room], "TTL expiry removes the stale room");
        assert!(dir.is_empty());
    }

    /// A refresh within the TTL keeps the room alive (temporary packet
    /// loss shorter than the TTL never flickers the room out).
    #[test]
    fn refresh_within_ttl_keeps_room_active() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let now = t0();

        dir.apply_advertisement_at(
            ad_named(room, 0x42, "steady", 5),
            owner,
            verified_auth(owner),
            1,
            1000,
            now,
        );
        // Refresh 3 s in (within the 5 s TTL).
        dir.apply_advertisement_at(
            ad_named(room, 0x42, "steady", 5),
            owner,
            verified_auth(owner),
            2,
            1300,
            now + Duration::from_secs(3),
        );
        // 6 s after first seen, 3 s after refresh: still active.
        assert!(
            dir.evict_expired_at(now + Duration::from_secs(6))
                .is_empty(),
            "refresh restarts the lifetime"
        );
        assert!(dir.contains(&room));
        // Only after the refresh TTL also elapses does it expire.
        assert_eq!(dir.evict_expired_at(now + Duration::from_secs(9)).len(), 1);
    }

    // ── Deduplication + conflicts (PDF Task 4.2, BORU-DIR-11) ─────────

    /// An identical advertisement (same publisher + envelope sequence +
    /// advert version + content digest) is a pure liveness refresh —
    /// `Duplicate`, single entry, no content change, no conflict flag.
    #[test]
    fn identical_advertisement_is_deduped_no_ui_churn() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let advert = ad(room, 0x42, "room");

        assert_eq!(
            dir.apply_advertisement(advert.clone(), owner, verified_auth(owner), 7, 1000),
            AdvertiseOutcome::Added
        );
        assert_eq!(dir.len(), 1);

        // Exact re-broadcast: same publisher, same sequence, same digest.
        let dup = dir.apply_advertisement(advert, owner, verified_auth(owner), 7, 1000);
        assert_eq!(dup, AdvertiseOutcome::Duplicate, "identical replay deduped");
        assert_eq!(dir.len(), 1, "no second card from repeated gossip");
        let entry = dir.get(&room).unwrap();
        assert_eq!(entry.sequence, 7, "content unchanged");
        assert!(!entry.conflict, "a duplicate is not a conflict");
    }

    /// Two different non-authority sources advertising conflicting metadata
    /// for the same room produce a deterministic winner that is flagged as
    /// conflicted (Boru cannot prove a canonical authority).
    #[test]
    fn conflicting_non_authority_metadata_marks_conflict() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let member_a = key(0x41);
        let member_b = key(0x42);
        let owner = key(0x55); // neither member is the room authority

        let first = dir.apply_advertisement(
            ad(room, 0x55, "Room A"),
            member_a,
            verified_auth(member_a),
            1,
            1000,
        );
        assert_eq!(first, AdvertiseOutcome::Added);
        assert!(!dir.get(&room).unwrap().conflict);

        // A different member claims different metadata, newer envelope.
        let second = dir.apply_advertisement(
            ad(room, 0x55, "Room B"),
            member_b,
            verified_auth(member_b),
            1,
            2000,
        );
        assert_eq!(second, AdvertiseOutcome::Refreshed, "newer winner stored");
        let entry = dir.get(&room).unwrap();
        assert_eq!(entry.advert.room_name, "Room B");
        assert!(entry.conflict, "conflicting metadata is flagged");

        let _ = owner;
    }

    /// Once an entry is conflicted, a different non-authority source cannot
    /// flip the metadata back and forth (anti-oscillation) — only the
    /// winning publisher's own refresh or a verified authority may change it.
    #[test]
    fn conflict_state_rejects_rapid_oscillation() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let member_a = key(0x41);
        let member_b = key(0x42);
        let member_c = key(0x43);

        dir.apply_advertisement(
            ad(room, 0x55, "Room A"),
            member_a,
            verified_auth(member_a),
            1,
            1000,
        );
        dir.apply_advertisement(
            ad(room, 0x55, "Room B"),
            member_b,
            verified_auth(member_b),
            1,
            2000,
        );
        assert!(dir.get(&room).unwrap().conflict);

        // A third member (newer envelope) cannot oscillate the metadata.
        let third = dir.apply_advertisement(
            ad(room, 0x55, "Room C"),
            member_c,
            verified_auth(member_c),
            1,
            3000,
        );
        assert_eq!(
            third,
            AdvertiseOutcome::Unchanged,
            "conflicted entry rejects further non-authority flips"
        );
        assert_eq!(dir.get(&room).unwrap().advert.room_name, "Room B");

        // The winning publisher refreshing its own claim is still allowed.
        let refresh = dir.apply_advertisement(
            ad(room, 0x55, "Room B"),
            member_b,
            verified_auth(member_b),
            2,
            4000,
        );
        assert_eq!(refresh, AdvertiseOutcome::Refreshed);
        assert!(dir.get(&room).unwrap().conflict, "conflict persists");
    }

    /// A verified authority advertisement resolves the conflict: it
    /// replaces the contested metadata and clears the conflict flag.
    #[test]
    fn verified_authority_resolves_conflict() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let member_a = key(0x41);
        let owner = key(0x55);

        dir.apply_advertisement(
            ad(room, 0x55, "Room A"),
            member_a,
            verified_auth(member_a),
            1,
            1000,
        );
        dir.apply_advertisement(
            ad(room, 0x55, "Room B"),
            key(0x42),
            verified_auth(key(0x42)),
            1,
            2000,
        );
        assert!(dir.get(&room).unwrap().conflict);

        // The room's real owner advertises canonical metadata.
        let mut canonical = ad(room, 0x55, "Canonical Room");
        canonical.sign(&secret_key(0x55));
        let resolved = dir.apply_advertisement(canonical, owner, verified_auth(owner), 9, 3000);
        assert_eq!(resolved, AdvertiseOutcome::Refreshed);
        let entry = dir.get(&room).unwrap();
        assert_eq!(entry.advert.room_name, "Canonical Room");
        assert!(!entry.conflict, "canonical authority clears the conflict");
        assert_eq!(entry.publisher, owner);
    }

    /// An untrusted (missing-signature) advertisement can never rename a
    /// room that a verified peer has identified — authentication gates the
    /// replacement regardless of envelope freshness.
    #[test]
    fn untrusted_update_cannot_rename_verified_room() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let member = key(0x41);

        dir.apply_advertisement(
            ad(room, 0x55, "Verified Room"),
            member,
            verified_auth(member),
            1,
            1000,
        );

        // Newer, but unsigned: the stranger's claim cannot overwrite the
        // verified listing (PDF Task 4.2 acceptance).
        let spoof = dir.apply_advertisement(
            ad(room, 0x55, "Hacked Room"),
            key(0x66),
            AdvertisementAuth::MissingSignature,
            99,
            9999,
        );
        assert_eq!(spoof, AdvertiseOutcome::Unchanged);
        assert_eq!(dir.get(&room).unwrap().advert.room_name, "Verified Room");
        assert!(!dir.get(&room).unwrap().conflict);
    }

    /// When a conflicting advertisement loses the deterministic tie, the
    /// existing winner is kept and the entry is marked conflicted — Boru
    /// retains a conflict state rather than silently trusting either claim.
    #[test]
    fn older_conflicting_advertisement_keeps_winner_marks_conflict() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let member_a = key(0x41);
        let member_b = key(0x42);

        // Winner: newer envelope from member A.
        dir.apply_advertisement(
            ad(room, 0x55, "Room A"),
            member_a,
            verified_auth(member_a),
            1,
            2000,
        );
        assert!(!dir.get(&room).unwrap().conflict);

        // Older conflicting claim from member B loses the tie but reveals
        // the disagreement: entry stays Room A, now flagged conflicted.
        let conflict = dir.apply_advertisement(
            ad(room, 0x55, "Room B"),
            member_b,
            verified_auth(member_b),
            1,
            1000,
        );
        assert_eq!(conflict, AdvertiseOutcome::Conflict);
        let entry = dir.get(&room).unwrap();
        assert_eq!(entry.advert.room_name, "Room A", "winner retained");
        assert!(entry.conflict, "disagreement retained as conflict state");
    }

    // ── Snapshot ──────────────────────────────────────────────────────

    /// `snapshot` is deterministic: entries sorted by room_id regardless
    /// of insertion order.
    #[test]
    fn snapshot_is_deterministic() {
        let mut dir = RoomDirectory::new();
        let owner = key(0x42);
        // Insert out of order.
        dir.apply_advertisement(
            ad(topic(3), 0x42, "c"),
            owner,
            verified_auth(owner),
            1,
            1000,
        );
        dir.apply_advertisement(
            ad(topic(1), 0x42, "a"),
            owner,
            verified_auth(owner),
            1,
            1000,
        );
        dir.apply_advertisement(
            ad(topic(2), 0x42, "b"),
            owner,
            verified_auth(owner),
            1,
            1000,
        );

        let snap = dir.snapshot();
        assert_eq!(snap.len(), 3);
        let names: Vec<&str> = snap.iter().map(|e| e.advert.room_name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"], "sorted by room_id");
    }

    /// Every cached entry carries the DIR-10 required fields: latest valid
    /// advertisement, advertiser identity, first/last seen, expiry,
    /// compatibility state, and local join state.
    #[test]
    fn entry_carries_required_directory_metadata() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let mut advert = ad(room, 0x42, "room");
        advert.room_protocol_version = crate::public_room::PROTOCOL_VERSION;

        dir.apply_advertisement(advert.clone(), owner, verified_auth(owner), 1, 1000);
        let entry = dir.get(&room).unwrap();

        assert_eq!(entry.advert, advert, "latest valid advertisement");
        assert_eq!(entry.publisher, owner, "advertiser identity");
        assert_eq!(
            entry.auth,
            verified_auth(owner),
            "authority/authentication state"
        );
        assert!(entry.first_seen <= entry.last_seen);
        assert!(entry.expires_at > entry.last_seen, "expiry from TTL");
        assert_eq!(
            entry.compatibility,
            RoomCompatibility::Compatible,
            "compatible room protocol"
        );
        assert_eq!(
            entry.local_join_state,
            LocalJoinState::NotJoined,
            "local join state defaults to NotJoined"
        );
    }

    /// A room advertising a newer protocol version than this client is
    /// stored as UpgradeRequired (Phase 6 Task 6.2 consumes this).
    #[test]
    fn newer_room_protocol_is_upgrade_required() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let mut advert = ad(room, 0x42, "future-room");
        advert.room_protocol_version = crate::public_room::PROTOCOL_VERSION + 1;

        dir.apply_advertisement(advert, owner, verified_auth(owner), 1, 1000);
        assert_eq!(
            dir.get(&room).unwrap().compatibility,
            RoomCompatibility::UpgradeRequired
        );
    }

    /// PDF Task 6.2 step 1 formalization: a room speaking a protocol more
    /// than one version newer is Unsupported — the protocol has diverged
    /// beyond the adjacent version this client can reason about.
    #[test]
    fn far_newer_room_protocol_is_unsupported() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let mut advert = ad(room, 0x42, "far-future-room");
        advert.room_protocol_version = crate::public_room::PROTOCOL_VERSION + 2;

        dir.apply_advertisement(advert, owner, verified_auth(owner), 1, 1000);
        assert_eq!(
            dir.get(&room).unwrap().compatibility,
            RoomCompatibility::Unsupported,
            "protocol more than one version ahead is Unsupported"
        );
    }

    /// PDF Task 6.2 step 1: the same, older, and legacy (version 0)
    /// protocols are all Compatible — a client can join them.
    #[test]
    fn same_older_and_legacy_protocols_are_compatible() {
        assert_eq!(
            RoomCompatibility::for_room_protocol(crate::public_room::PROTOCOL_VERSION),
            RoomCompatibility::Compatible
        );
        assert_eq!(
            RoomCompatibility::for_room_protocol(crate::public_room::PROTOCOL_VERSION - 1),
            RoomCompatibility::Compatible
        );
        // Version 0 = legacy "no protocol version declared" marker from
        // the pre-control-plane directory store — joinable, never blocked.
        assert_eq!(RoomCompatibility::for_room_protocol(0), RoomCompatibility::Compatible);
    }

    /// PDF Task 6.2 step 1: exactly one version newer is UpgradeRequired;
    /// more than one version newer is Unsupported.
    #[test]
    fn adjacent_newer_is_upgrade_required_not_unsupported() {
        assert_eq!(
            RoomCompatibility::for_room_protocol(crate::public_room::PROTOCOL_VERSION + 1),
            RoomCompatibility::UpgradeRequired
        );
        assert_eq!(
            RoomCompatibility::for_room_protocol(crate::public_room::PROTOCOL_VERSION + 2),
            RoomCompatibility::Unsupported
        );
        assert_eq!(
            RoomCompatibility::for_room_protocol(crate::public_room::PROTOCOL_VERSION + 3),
            RoomCompatibility::Unsupported
        );
    }

    /// PDF Task 6.2 step 2: a room advertising no optional feature flags
    /// has no feature-compatibility verdict (None).
    #[test]
    fn room_without_feature_flags_has_no_feature_verdict() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let advert = ad(room, 0x42, "plain-room");
        assert!(advert.feature_flags.is_empty());

        dir.apply_advertisement(advert, owner, verified_auth(owner), 1, 1000);
        assert_eq!(
            dir.get(&room).unwrap().feature_compat,
            RoomFeatureCompatibility::None
        );
    }

    /// PDF Task 6.2 step 2: a room whose advertised optional feature
    /// flags are all in the local capability set is AllSupported.
    #[test]
    fn all_advertised_features_supported() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let mut advert = ad(room, 0x42, "files-room");
        advert.feature_flags = vec![
            crate::control_plane::capabilities::ids::FILES_V2.to_string(),
            crate::control_plane::capabilities::ids::RICH_TEXT_V1.to_string(),
        ];

        dir.apply_advertisement(advert, owner, verified_auth(owner), 1, 1000);
        assert_eq!(
            dir.get(&room).unwrap().feature_compat,
            RoomFeatureCompatibility::AllSupported
        );
        // The base verdict stays Compatible — the join gate is untouched.
        assert_eq!(
            dir.get(&room).unwrap().compatibility,
            RoomCompatibility::Compatible
        );
    }

    /// PDF Task 6.2 step 2/4: a room advertising optional features this
    /// client does NOT support is still Compatible (joinable) — the
    /// missing flags are informational only. Unknown future flags are
    /// preserved and reported, never fatal.
    #[test]
    fn missing_optional_features_do_not_block_room() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let mut advert = ad(room, 0x42, "future-features-room");
        advert.feature_flags = vec![
            crate::control_plane::capabilities::ids::FILES_V2.to_string(),
            "voice-v1".to_string(),    // well-known, in the default local set
            "hologram-v9".to_string(), // unknown future flag
        ];
        // NOTE: `voice-v1` IS in the default local capability set (the
        // default set contains every well-known id). To exercise a
        // genuinely missing feature we use `hologram-v9`, an id that is
        // not well-known at all.

        dir.apply_advertisement(advert, owner, verified_auth(owner), 1, 1000);
        let entry = dir.get(&room).unwrap();
        assert_eq!(
            entry.compatibility,
            RoomCompatibility::Compatible,
            "base protocol compatible — room stays joinable"
        );
        // voice-v1 is well-known and in the default local set → supported.
        // hologram-v9 is unknown → reported missing, never fatal.
        assert_eq!(
            entry.feature_compat,
            RoomFeatureCompatibility::SomeMissing(vec!["hologram-v9".to_string()]),
            "unknown future flag reported as missing, never fatal"
        );
        assert_eq!(
            dir.snapshot()[0].offered_action(),
            RoomAction::Join,
            "optional-feature differences never block basic room access"
        );
    }

    /// PDF Task 6.2 step 2: replacing the local capability set re-derives
    /// every cached entry's feature compatibility — a previously missing
    /// future feature becomes AllSupported once the client gains it.
    #[test]
    fn set_local_capabilities_renegotiates_entries() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let mut advert = ad(room, 0x42, "future-room");
        advert.feature_flags = vec!["hologram-v9".to_string()];

        dir.apply_advertisement(advert, owner, verified_auth(owner), 1, 1000);
        // Unknown future flag → missing under the default local set.
        assert_eq!(
            dir.get(&room).unwrap().feature_compat,
            RoomFeatureCompatibility::SomeMissing(vec!["hologram-v9".to_string()])
        );

        // The client later gains the feature → renegotiation flips to
        // supported.
        let mut caps = dir.local_capabilities().clone();
        caps.insert_id("hologram-v9");
        dir.set_local_capabilities(caps);
        assert_eq!(
            dir.get(&room).unwrap().feature_compat,
            RoomFeatureCompatibility::AllSupported
        );
    }

    // ── Local relationship state (PDF Task 4.3, BORU-DIR-12) ─────────

    fn facts(joined: &[TopicId], pending: &[TopicId], hidden: &[TopicId]) -> LocalRoomFacts {
        LocalRoomFacts {
            joined: joined.iter().copied().collect(),
            pending: pending.iter().copied().collect(),
            hidden: hidden.iter().copied().collect(),
        }
    }

    /// The directory does not offer Join for an already joined room:
    /// `sync_local_states` with the real room DB's joined set marks the
    /// entry Joined and `offered_action` returns Open.
    #[test]
    fn joined_room_offers_open_not_join() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 1, 1000);

        dir.sync_local_states(facts(&[room], &[], &[]));

        assert_eq!(
            dir.get(&room).unwrap().local_join_state,
            LocalJoinState::Joined,
            "Joined derived from the real room database"
        );
        assert_eq!(
            dir.get(&room).unwrap().offered_action(),
            RoomAction::Open,
            "already-joined room shows Open, never Join"
        );
    }

    /// A joined room is still part of the browse surface (Open, not
    /// hidden), and Join is only offered for genuinely unjoined rooms.
    #[test]
    fn join_offered_only_for_not_joined_room() {
        let mut dir = RoomDirectory::new();
        let joined_room = topic(1);
        let discoverable_room = topic(2);
        let owner = key(0x42);
        dir.apply_advertisement(
            ad(joined_room, 0x42, "joined"),
            owner,
            verified_auth(owner),
            1,
            1000,
        );
        dir.apply_advertisement(
            ad(discoverable_room, 0x42, "discoverable"),
            owner,
            verified_auth(owner),
            1,
            1000,
        );

        dir.sync_local_states(facts(&[joined_room], &[], &[]));

        assert_eq!(
            dir.get(&joined_room).unwrap().offered_action(),
            RoomAction::Open
        );
        assert_eq!(
            dir.get(&discoverable_room).unwrap().offered_action(),
            RoomAction::Join
        );
    }

    /// Directory state cannot duplicate local membership records: syncing
    /// facts never creates or inserts entries, and a joined room that is
    /// not (or no longer) advertised does not appear in the directory.
    #[test]
    fn sync_local_states_never_duplicates_membership() {
        let mut dir = RoomDirectory::new();
        let advertised = topic(1);
        let owner = key(0x42);
        dir.apply_advertisement(
            ad(advertised, 0x42, "room"),
            owner,
            verified_auth(owner),
            1,
            1000,
        );
        let before = dir.len();

        // Facts include rooms that were never advertised (e.g. the local
        // conversation store has a room this directory has never seen).
        let local_only_room = topic(9);
        dir.sync_local_states(facts(&[advertised, local_only_room], &[], &[]));

        assert_eq!(dir.len(), before, "sync never inserts new entries");
        assert!(
            !dir.contains(&local_only_room),
            "a local-only room is not materialised in the directory"
        );
        assert_eq!(
            dir.get(&advertised).unwrap().local_join_state,
            LocalJoinState::Joined
        );
    }

    /// Hidden rooms are derived Blocked, excluded from the browse
    /// snapshot, and stay hidden across advertisement refreshes when the
    /// preference is persisted (fed back on every sync).
    #[test]
    fn hidden_room_stays_hidden_across_refreshes() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);

        // First advertisement, then the user hides the room.
        dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 1, 1000);
        dir.sync_local_states(facts(&[], &[], &[room]));

        assert_eq!(
            dir.get(&room).unwrap().local_join_state,
            LocalJoinState::Blocked
        );
        assert!(
            dir.snapshot().is_empty(),
            "hidden room is not re-shown in the browse surface"
        );
        assert_eq!(
            dir.snapshot_all().len(),
            1,
            "diagnostics still see the hidden entry"
        );

        // A refresh advertisement arrives: the persisted hide preference
        // keeps the room hidden (no re-show).
        let outcome =
            dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 2, 1060);
        assert_eq!(outcome, AdvertiseOutcome::Refreshed);
        assert_eq!(
            dir.get(&room).unwrap().local_join_state,
            LocalJoinState::Blocked,
            "refresh must not un-hide the room"
        );
        assert!(dir.snapshot().is_empty());

        // A second refresh (new publisher, verified) also keeps it hidden.
        let endorser = key(0x43);
        let mut endorsement = ad(room, 0x42, "room");
        endorsement.sign(&secret_key(0x43));
        dir.apply_advertisement(endorsement, endorser, verified_auth(endorser), 1, 2000);
        assert_eq!(
            dir.get(&room).unwrap().local_join_state,
            LocalJoinState::Blocked
        );
        assert!(dir.snapshot().is_empty());
    }

    /// A hidden room that is later re-advertised from scratch (evicted /
    /// expired meanwhile) stays hidden because the facts are stored.
    #[test]
    fn hidden_room_readded_after_eviction_stays_hidden() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let now = t0();

        dir.apply_advertisement_at(
            ad_named(room, 0x42, "room", 1),
            owner,
            verified_auth(owner),
            1,
            1000,
            now,
        );
        dir.sync_local_states(facts(&[], &[], &[room]));

        // TTL elapses → entry evicted.
        assert_eq!(dir.evict_expired_at(now + Duration::from_secs(2)).len(), 1);
        assert!(dir.is_empty());

        // The advertiser re-publishes: hidden preference still applied.
        dir.apply_advertisement_at(
            ad_named(room, 0x42, "room", 300),
            owner,
            verified_auth(owner),
            2,
            3000,
            now + Duration::from_secs(3),
        );
        assert_eq!(
            dir.get(&room).unwrap().local_join_state,
            LocalJoinState::Blocked,
            "re-added hidden room stays hidden"
        );
        assert!(dir.snapshot().is_empty());
    }

    /// Explicitly resetting the preference (facts.hidden no longer
    /// contains the room) restores the room to the browse surface.
    #[test]
    fn unhide_restores_room_to_browse_surface() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 1, 1000);

        dir.sync_local_states(facts(&[], &[], &[room]));
        assert!(dir.snapshot().is_empty());

        // The user explicitly resets the hide preference (BORU-DIR-20).
        dir.sync_local_states(facts(&[], &[], &[]));
        assert_eq!(
            dir.get(&room).unwrap().local_join_state,
            LocalJoinState::NotJoined
        );
        assert_eq!(dir.snapshot().len(), 1, "room is browseable again");
    }

    /// Incompatible rooms derive Incompatible local state: Join is
    /// blocked (Phase 6 Task 6.2), and the directory never offers Join.
    #[test]
    fn incompatible_room_never_offers_join() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        let mut advert = ad(room, 0x42, "future-room");
        advert.room_protocol_version = crate::public_room::PROTOCOL_VERSION + 1;
        dir.apply_advertisement(advert, owner, verified_auth(owner), 1, 1000);

        dir.sync_local_states(facts(&[], &[], &[]));
        assert_eq!(
            dir.get(&room).unwrap().local_join_state,
            LocalJoinState::Incompatible
        );
        assert_eq!(
            dir.get(&room).unwrap().offered_action(),
            RoomAction::Incompatible
        );
    }

    /// BORU-DIR-18 (PDF Task 6.3): a locally blocked room (banned /
    /// hidden via `LocalRoomFacts.hidden`) is never offered as Join — the
    /// directory derives `Blocked` and the offered action is `Hidden`,
    /// even though the room is protocol-compatible and the advertisement
    /// itself remains valid (visibility and join authorization stay
    /// independent).
    #[test]
    fn blocked_room_never_offers_join_and_advertisement_survives() {
        let mut dir = RoomDirectory::new();
        let room = topic(1);
        let owner = key(0x42);
        dir.apply_advertisement(ad(room, 0x42, "room"), owner, verified_auth(owner), 1, 1000);

        dir.sync_local_states(facts(&[], &[], &[room]));
        assert_eq!(
            dir.get(&room).unwrap().local_join_state,
            LocalJoinState::Blocked,
            "hidden preference derives Blocked"
        );
        assert_eq!(
            dir.get(&room).unwrap().offered_action(),
            RoomAction::Hidden,
            "a blocked room is never offered as Join"
        );
        assert!(
            dir.get(&room).is_some(),
            "the advertisement itself is not deleted by a local block"
        );
        // The browse surface hides it, but diagnostics still see the entry
        // (the ad is intact, just not joinable).
        assert!(dir.snapshot().is_empty());
        assert_eq!(dir.snapshot_all().len(), 1);
    }

    /// Precedence: hidden beats joined; joined beats pending;
    /// pending beats incompatible; incompatible beats NotJoined.
    #[test]
    fn derive_local_state_precedence() {
        let compatible = RoomCompatibility::Compatible;
        let incompatible = RoomCompatibility::UpgradeRequired;

        assert_eq!(
            derive_local_state(compatible, true, false, true),
            LocalJoinState::Blocked,
            "hidden wins over joined"
        );
        assert_eq!(
            derive_local_state(compatible, true, true, false),
            LocalJoinState::Joined,
            "joined wins over pending"
        );
        assert_eq!(
            derive_local_state(compatible, false, true, false),
            LocalJoinState::JoinPending
        );
        assert_eq!(
            derive_local_state(incompatible, false, false, false),
            LocalJoinState::Incompatible
        );
        assert_eq!(
            derive_local_state(compatible, false, false, false),
            LocalJoinState::NotJoined
        );
    }
}
