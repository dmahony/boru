//! Control-plane privacy + abuse guards (PDF Phase 1, Task 1.3).
//!
//! Discovery must be useful without becoming a metadata broadcast or spam
//! surface. This module is the BORU-CP-03 privacy/abuse layer for the
//! hidden-discovery control plane. It composes five guards:
//!
//! 1. **Minimum advertisement content** — [`ControlAdvertPolicy`] is an
//!    explicit whitelist of the ONLY fields a control-plane message may
//!    carry (stable peer identity + protocol metadata), plus hard bounds on
//!    every variable-size field. Usernames, profile text, friend lists,
//!    group memberships, filenames, message previews, tunnel destinations,
//!    and detailed device information are structurally impossible in the
//!    typed payloads ([`ControlEnvelope`]) and are rejected by the policy
//!    where free-form content could smuggle them in (capability ids,
//!    diagnostic notes).
//! 2. **Per-sender rate limiting** — [`ControlPlaneRateLimiter`] bounds how
//!    many control frames one real peer can deliver within a sliding window,
//!    keyed on the *authenticated* gossip delivery source so a spoofing
//!    flood cannot bypass it. A malicious peer cannot cause unbounded log
//!    spam or presence churn.
//! 3. **Deduplication** — control frames are deduplicated by
//!    `(sender_node_id, sequence)`; duplicate presence/hello deliveries have
//!    no side effects. The dedup set is bounded and cleared when full.
//! 4. **TTL-based presence expiry** — [`PeerControlStateStore`] keeps
//!    per-peer control-plane state (last seen, protocol version,
//!    capabilities) with a configured TTL; stale peers disappear from
//!    active presence ([`expire_stale`](PeerControlStateStore::expire_stale)).
//!    The store is bounded: at most [`MAX_CONTROL_PEERS`] peers, evicting
//!    stale-then-oldest entries when full.
//! 5. **Attribution** — where Boru has authenticated identity material
//!    (the gossip transport authenticates `delivered_from` as the real
//!    sender of a frame), [`ControlPlaneGuard::admit`] verifies the
//!    envelope's claimed `sender_node_id` equals the authenticated delivery
//!    source before trusting capability or presence state. A frame that
//!    claims a different identity is dropped as a spoof.
//!
//! # No authorisation by presence
//!
//! Nothing in this module can grant authorisation. [`PeerControlStateStore`]
//! is a metadata cache with no friendship, group, file, or tunnel methods —
//! being discoverable never makes a peer a friend, group member, tunnel
//! client, or file recipient, and discovery state cannot bypass the existing
//! friendship/trust checks (tests prove this structurally).
//!
//! # Hints, not promises
//!
//! All discovery data (presence, capabilities, protocol versions) is a
//! **hint** until the actual private/direct connection succeeds. The store
//! records what a peer *advertised*; it never drives connection attempts,
//! conversations, or authorisation decisions.
//!
//! # Observability
//!
//! Callers log the *state transition* (accepted / spoofed / rate-limited /
//! duplicate / violation / expired), never message contents.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use iroh_base::PublicKey;

use super::capabilities::CapabilitySet;
use super::message::{ControlEnvelope, ControlPayload};

// ---------------------------------------------------------------------------
// Constants (bounded-resources guardrail)
// ---------------------------------------------------------------------------

/// Default TTL for control-plane presence: a peer not heard from within
/// this window disappears from active presence.
pub const DEFAULT_PRESENCE_TTL: Duration = Duration::from_secs(300);

/// Upper bound a peer may advertise as its presence TTL. A larger suggested
/// TTL is clamped to this value (a peer cannot make us remember it forever).
pub const MAX_PRESENCE_TTL_SECS: u32 = 3600;

/// Maximum number of peers tracked in the control-plane presence store.
/// Beyond this the store evicts stale-then-oldest entries (bounded memory).
pub const MAX_CONTROL_PEERS: usize = 1024;

/// Maximum number of capability ids one CAPABILITIES advertisement may
/// carry.
pub const MAX_CAPABILITIES: usize = 64;

/// Maximum length of a single capability id.
pub const MAX_CAPABILITY_ID_LEN: usize = 64;

/// Maximum length of a DIAGNOSTIC_HINT note (free-form text is the main
/// place an attacker could smuggle metadata; keep it tiny and bounded).
pub const MAX_DIAGNOSTIC_NOTE_LEN: usize = 256;

/// Default per-sender control-frame rate limit: frames per window.
pub const CONTROL_RATE_LIMIT_MAX_FRAMES: u32 = 60;

/// Default per-sender control-frame rate-limit window.
pub const CONTROL_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(10);

/// Maximum number of distinct senders tracked by the rate limiter. When
/// exceeded, the oldest window is evicted so memory stays bounded even
/// against a flood of unique fake identities.
pub const MAX_RATE_LIMITED_SENDERS: usize = 4096;

/// Default interval for the presence-expiry sweep.
pub const EXPIRY_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Default dedup-set capacity for control frames. Cleared when full
/// (announcements are low-frequency, so a full clear is safe).
pub const CONTROL_DEDUP_CAP: usize = 4096;

// ---------------------------------------------------------------------------
// Minimum advertisement content (whitelist + bounds)
// ---------------------------------------------------------------------------

/// Why a control-plane advertisement violates the minimal-content whitelist
/// or a bound. All fields are numeric so the enum stays `Copy` (it is
/// returned through [`GuardVerdict`] and surfaced in [`IncomingOutcome`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertViolation {
    /// CAPABILITIES carries more ids than [`MAX_CAPABILITIES`].
    CapabilitiesTooMany {
        /// Number of capability ids in the advertisement.
        count: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A capability id is longer than [`MAX_CAPABILITY_ID_LEN`].
    CapabilityTooLong {
        /// Index of the offending capability id.
        index: usize,
        /// Length of the offending id.
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A capability id is empty or contains characters outside the
    /// namespaced-id charset (`[A-Za-z0-9._-]`). Rejected both because
    /// capability ids are protocol metadata (not free text) and to prevent
    /// log-injection via crafted ids.
    CapabilityInvalid {
        /// Index of the offending capability id.
        index: usize,
    },
    /// A DIAGNOSTIC_HINT note is longer than [`MAX_DIAGNOSTIC_NOTE_LEN`].
    DiagnosticNoteTooLong {
        /// Length of the note.
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A PRESENCE payload suggests a TTL larger than [`MAX_PRESENCE_TTL_SECS`].
    PresenceTtlTooLarge {
        /// TTL suggested by the peer.
        ttl: u32,
        /// Maximum allowed.
        max: u32,
    },
    /// An EXTENSIONS payload violates the metadata bounds (BORU-CP-16).
    Extensions(crate::control_plane::extensions::ExtensionsViolation),
    /// A PUBLIC_ROOM_ADVERTISEMENT payload violates the room-advertisement
    /// metadata bounds or privacy guardrails (BORU-DIR-02).
    Advertisement(crate::control_plane::advertisement::AdvertisementViolation),
}

/// The minimal-advertisement whitelist policy (PDF Task 1.3 steps 1–2).
///
/// # Field whitelist
///
/// The ONLY fields a control-plane message may carry, by message type:
///
/// | Field | Allowed on | Bound |
/// |-------|-----------|-------|
/// | `protocol_version` (envelope) | all | fixed at decode |
/// | `sender_node_id` (envelope) | all | fixed 32-byte key |
/// | `sequence` (envelope) | all | u64 |
/// | `timestamp_secs` (envelope) | all | u64 |
/// | `app_protocol_version` | HELLO | u8 |
/// | `ttl_secs` | PRESENCE | ≤ [`MAX_PRESENCE_TTL_SECS`] |
/// | `capabilities[]` | CAPABILITIES | ≤ [`MAX_CAPABILITIES`], each id ≤ [`MAX_CAPABILITY_ID_LEN`], charset `[A-Za-z0-9._-]` |
/// | `hint_code` | DIAGNOSTIC_HINT | u32 |
/// | `note` | DIAGNOSTIC_HINT | ≤ [`MAX_DIAGNOSTIC_NOTE_LEN`] |
///
/// Anything else — usernames, profile text, friend lists, group
/// memberships, filenames, message previews, tunnel destinations, device
/// details — is structurally impossible in the typed [`ControlEnvelope`]
/// payloads and is *rejected by construction*. This policy enforces the
/// bounds on the variable-size fields so a malicious peer cannot use the
/// metadata channels to smuggle content or grow state without limit.
#[derive(Debug, Clone)]
pub struct ControlAdvertPolicy {
    /// Maximum capability ids per advertisement.
    pub max_capabilities: usize,
    /// Maximum length of a single capability id.
    pub max_capability_id_len: usize,
    /// Maximum length of a DIAGNOSTIC_HINT note.
    pub max_diagnostic_note_len: usize,
    /// Maximum advertisable presence TTL in seconds.
    pub max_presence_ttl_secs: u32,
    /// Bounds applied to an EXTENSIONS payload (BORU-CP-16, PDF Phase 6).
    pub extensions_bounds: crate::control_plane::extensions::ExtensionsBounds,
    /// Bounds applied to a PUBLIC_ROOM_ADVERTISEMENT payload (BORU-DIR-02,
    /// PDF Task 1.2): room-name/description/tag/flag limits, TTL clamp, and
    /// total encoded size cap so the advertisement stays compact and cannot
    /// smuggle content through the room-discovery metadata channel.
    pub advertisement_bounds: crate::control_plane::advertisement::AdvertisementBounds,
}

impl Default for ControlAdvertPolicy {
    fn default() -> Self {
        Self {
            max_capabilities: MAX_CAPABILITIES,
            max_capability_id_len: MAX_CAPABILITY_ID_LEN,
            max_diagnostic_note_len: MAX_DIAGNOSTIC_NOTE_LEN,
            max_presence_ttl_secs: MAX_PRESENCE_TTL_SECS,
            extensions_bounds: crate::control_plane::extensions::ExtensionsBounds::default(),
            advertisement_bounds: crate::control_plane::advertisement::AdvertisementBounds::default(
            ),
        }
    }
}

impl ControlAdvertPolicy {
    /// Check `envelope` against the minimal-content whitelist and bounds.
    ///
    /// Returns `Ok(())` for a minimal advertisement; `Err(violation)` with
    /// the specific bound that was exceeded. The check never panics.
    pub fn check(&self, envelope: &ControlEnvelope) -> Result<(), AdvertViolation> {
        match &envelope.payload {
            ControlPayload::Hello(_) => {
                // app_protocol_version is a u8 — inherently bounded.
                Ok(())
            }
            ControlPayload::Presence(payload) => {
                if let Some(ttl) = payload.ttl_secs {
                    if ttl > self.max_presence_ttl_secs {
                        return Err(AdvertViolation::PresenceTtlTooLarge {
                            ttl,
                            max: self.max_presence_ttl_secs,
                        });
                    }
                }
                Ok(())
            }
            ControlPayload::Capabilities(payload) => {
                if payload.capabilities.len() > self.max_capabilities {
                    return Err(AdvertViolation::CapabilitiesTooMany {
                        count: payload.capabilities.len(),
                        max: self.max_capabilities,
                    });
                }
                for (index, id) in payload.capabilities.iter().enumerate() {
                    if id.is_empty() || id.len() > self.max_capability_id_len {
                        return Err(AdvertViolation::CapabilityTooLong {
                            index,
                            len: id.len(),
                            max: self.max_capability_id_len,
                        });
                    }
                    if !id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                    {
                        return Err(AdvertViolation::CapabilityInvalid { index });
                    }
                }
                Ok(())
            }
            ControlPayload::DiagnosticHint(payload) => {
                if let Some(note) = &payload.note {
                    if note.len() > self.max_diagnostic_note_len {
                        return Err(AdvertViolation::DiagnosticNoteTooLong {
                            len: note.len(),
                            max: self.max_diagnostic_note_len,
                        });
                    }
                }
                Ok(())
            }
            ControlPayload::Extensions(payload) => payload
                .validate(&self.extensions_bounds)
                .map_err(AdvertViolation::Extensions),
            ControlPayload::PublicRoomAdvertisement(payload) => payload
                .validate(&self.advertisement_bounds)
                .map_err(AdvertViolation::Advertisement),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-sender sliding-window rate limiter
// ---------------------------------------------------------------------------

/// Per-sender sliding-window rate limiter for inbound control frames.
///
/// Keyed on the **authenticated gossip delivery source** (the real peer that
/// pushed the frame), so a spoofing flood cannot rotate through fake
/// envelope identities to bypass the limit. Mirrors the proven
/// [`crate::catalogue_rate_limits::PeerCatalogueRateLimiter`] pattern
/// (VecDeque of timestamps per key, purging expired entries on each check)
/// with a bounded sender map.
#[derive(Debug, Clone)]
pub struct ControlPlaneRateLimiter {
    windows: HashMap<PublicKey, VecDeque<Instant>>,
    max_frames: u32,
    window: Duration,
    max_senders: usize,
}

impl ControlPlaneRateLimiter {
    /// A rate limiter with the default limits.
    pub fn new() -> Self {
        Self::with_limits(
            CONTROL_RATE_LIMIT_MAX_FRAMES,
            CONTROL_RATE_LIMIT_WINDOW,
            MAX_RATE_LIMITED_SENDERS,
        )
    }

    /// A rate limiter with explicit limits (tests use small windows).
    pub fn with_limits(max_frames: u32, window: Duration, max_senders: usize) -> Self {
        Self {
            windows: HashMap::new(),
            max_frames: max_frames.max(1),
            window,
            max_senders: max_senders.max(1),
        }
    }

    /// Whether `sender` may send another frame now; records it when allowed.
    ///
    /// When the sender map is at capacity, the oldest-recorded sender window
    /// is evicted first so memory stays bounded.
    pub fn admit(&mut self, sender: &PublicKey) -> bool {
        let now = Instant::now();
        let window_start = now - self.window;

        if !self.windows.contains_key(sender) && self.windows.len() >= self.max_senders {
            // Evict the oldest-recorded window (HashMap iteration order is
            // arbitrary, so track insertion order via a scan for the
            // oldest front timestamp; bounded work since max_senders is
            // capped).
            let oldest = self
                .windows
                .iter()
                .filter_map(|(k, q)| q.front().map(|t| (*k, *t)))
                .min_by_key(|(_, t)| *t)
                .map(|(k, _)| k);
            if let Some(oldest) = oldest {
                self.windows.remove(&oldest);
            }
        }

        let entries = self.windows.entry(*sender).or_default();

        // Purge entries older than the window.
        while let Some(&t) = entries.front() {
            if t < window_start {
                entries.pop_front();
            } else {
                break;
            }
        }

        if entries.len() as u32 >= self.max_frames {
            return false; // Rate limited — do NOT record this frame.
        }
        entries.push_back(now);
        true
    }

    /// Remove all state for `sender` (tests / peer expiry).
    pub fn reset(&mut self, sender: &PublicKey) {
        self.windows.remove(sender);
    }

    /// Number of senders currently tracked (bounded by `max_senders`).
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// Whether no senders are tracked.
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

impl Default for ControlPlaneRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// TTL-based peer control state (presence hints)
// ---------------------------------------------------------------------------

/// Per-peer control-plane state held in [`PeerControlStateStore`].
///
/// Metadata only: what the peer advertised and when we last heard from it.
/// This is a **hint** until the actual private/direct connection succeeds,
/// and it grants no authorisation.
///
/// "Online" is never stored as permanent truth: presence is **derived**
/// from recent activity + TTL. [`PeerControlState::presence_state`]
/// computes [`PresenceState::Active`] vs [`PresenceState::Stale`] from
/// [`last_seen`](Self::last_seen) and [`ttl`](Self::ttl) at read time, and
/// [`expire_stale`](PeerControlStateStore::expire_stale) removes stale
/// entries entirely (PDF Task 2.1 step 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerControlState {
    /// Stable peer identity (envelope `sender_node_id`).
    pub peer_id: PublicKey,
    /// When this peer was last heard from (drives TTL expiry).
    pub last_seen: Instant,
    /// When this peer was FIRST seen on the discovery topic (BORU-CP-04 /
    /// PDF Task 2.1 step 5: `discovery_seen_at`). Set on the first
    /// advertisement and preserved across refreshes — it is the peer's
    /// discovery age, not its activity recency.
    pub discovery_seen_at: Instant,
    /// Highest sequence accepted from this peer (monotonic per sender).
    pub last_sequence: u64,
    /// Control-plane envelope protocol version the peer speaks.
    pub protocol_version: u8,
    /// HELLO `app_protocol_version`, if a HELLO was seen.
    pub app_protocol_version: Option<u8>,
    /// Latest CAPABILITIES advertisement (bounded by the advert policy).
    pub capabilities: Vec<String>,
    /// Latest EXTENSIONS advertisement (BORU-CP-16, PDF Phase 6), if any.
    /// Metadata only, bounded by the advert policy's [`ExtensionsBounds`](crate::control_plane::extensions::ExtensionsBounds).
    pub extensions: Option<crate::control_plane::extensions::ExtensionsPayload>,
    /// Effective presence TTL for this peer: the peer-suggested TTL clamped
    /// to the store default (a peer cannot make us remember it longer than
    /// our own default), or the store default when the peer suggests none.
    pub ttl: Duration,
}

/// Derived presence state (PDF Task 2.1 step 5: "current reachability
/// state" stored in memory). Deliberately NOT a persisted field — it is
/// recomputed from recent activity + TTL so "online" is never permanent
/// truth. A peer is [`PresenceState::Active`] only while it has been heard
/// from within its TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceState {
    /// The peer has been heard from within its TTL — recently reachable.
    Active,
    /// The peer has not been heard from within its TTL — stale/offline.
    Stale,
}

impl PeerControlState {
    /// Whether this peer's presence is stale at `now` (beyond its TTL).
    pub fn is_stale(&self, now: Instant) -> bool {
        now.duration_since(self.last_seen) >= self.ttl
    }

    /// The derived presence state at `now` — active while within TTL, stale
    /// after. Never stored, always derived from recent activity (PDF Task
    /// 2.1 step 6).
    pub fn presence_state(&self, now: Instant) -> PresenceState {
        if self.is_stale(now) {
            PresenceState::Stale
        } else {
            PresenceState::Active
        }
    }

    /// The latest valid capability set advertised by this peer, as a typed
    /// [`CapabilitySet`] (BORU-CP-11 / PDF Task 4.2 step 3).
    ///
    /// Lossless: ids this client does not understand are preserved in the
    /// set's raw bucket, so an advertisement is cached exactly as received.
    /// The raw wire list ([`PeerControlState::capabilities`]) is the cache
    /// of record; this is the typed view over it.
    pub fn capability_set(&self) -> CapabilitySet {
        CapabilitySet::from_wire(self.capabilities.iter().cloned())
    }
}

/// Outcome of [`PeerControlStateStore::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// A fresh entry was created for an unknown peer.
    New,
    /// A known peer's state was refreshed (newer sequence).
    Refreshed,
    /// The envelope's sequence is not newer than the last accepted one —
    /// an out-of-order older delivery. No state change (idempotence).
    Duplicate,
}

/// Bounded, TTL-expiring store of control-plane presence state.
///
/// Active presence = peers whose `last_seen` is within their TTL. Stale
/// peers disappear from active presence via [`expire_stale`](Self::expire_stale).
/// The store is capped at [`MAX_CONTROL_PEERS`]; when full it evicts stale
/// entries first, then the oldest entry, so a flood of unique identities
/// cannot grow memory without bound.
///
/// The store has **no authorisation surface**: it cannot create friends,
/// groups, files, or tunnels, and nothing here is consulted by the
/// friendship/trust checks (tests prove the absence).
#[derive(Debug, Clone)]
pub struct PeerControlStateStore {
    peers: HashMap<PublicKey, PeerControlState>,
    max_peers: usize,
    default_ttl: Duration,
}

impl Default for PeerControlStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerControlStateStore {
    /// An empty store with the default limits.
    pub fn new() -> Self {
        Self::with_limits(MAX_CONTROL_PEERS, DEFAULT_PRESENCE_TTL)
    }

    /// An empty store with explicit limits (tests use small caps / TTLs).
    pub fn with_limits(max_peers: usize, default_ttl: Duration) -> Self {
        Self {
            peers: HashMap::new(),
            max_peers: max_peers.max(1),
            default_ttl: default_ttl.max(Duration::from_millis(1)),
        }
    }

    /// Change the default presence TTL applied to new entries.
    pub fn set_default_ttl(&mut self, ttl: Duration) {
        self.default_ttl = ttl.max(Duration::from_millis(1));
    }

    /// The store's configured default presence TTL.
    pub fn default_ttl(&self) -> Duration {
        self.default_ttl
    }

    /// Record a control-plane envelope as presence state for its sender.
    ///
    /// * Unknown peer → insert a fresh entry ([`StoreOutcome::New`]),
    ///   evicting stale-then-oldest if at capacity.
    /// * Known peer, newer sequence → refresh last-seen / protocol /
    ///   capabilities / TTL ([`StoreOutcome::Refreshed`]).
    /// * Known peer, not-newer sequence → no state change
    ///   ([`StoreOutcome::Duplicate`]) — duplicate/out-of-order deliveries
    ///   never refresh presence or create side effects.
    ///
    /// `now` is explicit so tests can drive TTL expiry deterministically.
    pub fn record(&mut self, envelope: &ControlEnvelope, now: Instant) -> StoreOutcome {
        let peer_id = envelope.sender_node_id;
        let ttl = match &envelope.payload {
            ControlPayload::Presence(payload) => payload
                .ttl_secs
                .map(|s| Duration::from_secs(s as u64))
                .unwrap_or(self.default_ttl)
                .min(self.default_ttl),
            _ => self.default_ttl,
        };

        if let Some(entry) = self.peers.get_mut(&peer_id) {
            if envelope.sequence <= entry.last_sequence {
                return StoreOutcome::Duplicate;
            }
            entry.last_seen = now;
            entry.last_sequence = envelope.sequence;
            entry.protocol_version = envelope.protocol_version;
            entry.ttl = ttl;
            match &envelope.payload {
                ControlPayload::Hello(payload) => {
                    entry.app_protocol_version = Some(payload.app_protocol_version);
                }
                ControlPayload::Capabilities(payload) => {
                    entry.capabilities = payload.capabilities.clone();
                }
                ControlPayload::Extensions(payload) => {
                    entry.extensions = Some(payload.clone());
                }
                _ => {}
            }
            return StoreOutcome::Refreshed;
        }

        // Unknown peer: enforce the capacity bound.
        if self.peers.len() >= self.max_peers {
            self.evict_one(now);
        }

        let app_protocol_version = match &envelope.payload {
            ControlPayload::Hello(payload) => Some(payload.app_protocol_version),
            _ => None,
        };
        let capabilities = match &envelope.payload {
            ControlPayload::Capabilities(payload) => payload.capabilities.clone(),
            _ => Vec::new(),
        };
        let extensions = match &envelope.payload {
            ControlPayload::Extensions(payload) => Some(payload.clone()),
            _ => None,
        };
        self.peers.insert(
            peer_id,
            PeerControlState {
                peer_id,
                last_seen: now,
                discovery_seen_at: now,
                last_sequence: envelope.sequence,
                protocol_version: envelope.protocol_version,
                app_protocol_version,
                capabilities,
                extensions,
                ttl,
            },
        );
        StoreOutcome::New
    }

    /// Remove peers whose presence is stale at `now`, returning their ids.
    ///
    /// This is what makes stale peers *disappear from active presence*:
    /// after the sweep, [`peers`](Self::peers) and [`len`](Self::len) only
    /// reflect peers heard from within their TTL.
    pub fn expire_stale(&mut self, now: Instant) -> Vec<PublicKey> {
        let mut expired = Vec::new();
        self.peers.retain(|_, entry| {
            let keep = !entry.is_stale(now);
            if !keep {
                expired.push(entry.peer_id);
            }
            keep
        });
        expired
    }

    /// The latest valid capability set cached for `node_id`, if the peer is
    /// known and actually advertised capabilities (`None` for a peer that
    /// never sent a CAPABILITIES envelope — unknown, not "empty").
    pub fn capability_set_of(&self, node_id: &PublicKey) -> Option<CapabilitySet> {
        let state = self.peers.get(node_id)?;
        if state.capabilities.is_empty() {
            return None;
        }
        Some(state.capability_set())
    }

    /// The latest EXTENSIONS advertisement cached for `node_id` (BORU-CP-16,
    /// PDF Phase 6), if the peer is known and actually advertised one.
    ///
    /// Metadata only, bounded by the advert policy's extensions bounds.
    /// Returns `None` for a peer that never sent an EXTENSIONS envelope —
    /// unknown, not "empty". Like capabilities, this is a hint that grants
    /// no authorisation; the peer's presence staleness must be checked by
    /// the caller via [`get_active`](Self::get_active) /
    /// [`get_stale`](Self::get_stale).
    pub fn extensions_of(
        &self,
        node_id: &PublicKey,
    ) -> Option<crate::control_plane::extensions::ExtensionsPayload> {
        self.peers.get(node_id)?.extensions.clone()
    }

    /// Look up the control state for `node_id`, if present.
    pub fn get(&self, node_id: &PublicKey) -> Option<&PeerControlState> {
        self.peers.get(node_id)
    }

    /// Look up the control state for `node_id` only while its presence is
    /// ACTIVE at `now` (BORU-CP-11 / PDF Task 4.2 step 4).
    ///
    /// Returns `None` for unknown peers AND for peers whose presence has
    /// gone stale (beyond their TTL) — so a caller that asks "what does
    /// this peer currently support?" can never treat stale capability data
    /// as current. Capability data dies with presence: when the entry is
    /// removed by [`expire_stale`](Self::expire_stale), the cached
    /// capabilities go with it.
    pub fn get_active(&self, node_id: &PublicKey, now: Instant) -> Option<&PeerControlState> {
        let entry = self.peers.get(node_id)?;
        if entry.is_stale(now) {
            return None;
        }
        Some(entry)
    }

    /// Look up the control state for `node_id` even when its presence has
    /// gone stale at `now` (the complement of [`get_active`](Self::get_active)).
    ///
    /// Stale entries still exist in the store until the expiry sweep removes
    /// them; this accessor lets diagnostics/observability read the *last
    /// known* state while making the staleness explicit to the caller.
    pub fn get_stale(&self, node_id: &PublicKey, now: Instant) -> Option<&PeerControlState> {
        let entry = self.peers.get(node_id)?;
        if entry.is_stale(now) {
            return Some(entry);
        }
        None
    }

    /// Whether `node_id` currently has an entry.
    pub fn contains(&self, node_id: &PublicKey) -> bool {
        self.peers.contains_key(node_id)
    }

    /// Iterate over all tracked peers.
    pub fn peers(&self) -> impl Iterator<Item = (&PublicKey, &PeerControlState)> {
        self.peers.iter()
    }

    /// Number of tracked peers (bounded by `max_peers`).
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Remove every entry, returning the removed ids.
    pub fn clear(&mut self) -> Vec<PublicKey> {
        let removed: Vec<PublicKey> = self.peers.keys().copied().collect();
        self.peers.clear();
        removed
    }

    /// Evict one entry to make room: stale entries first, then oldest.
    fn evict_one(&mut self, now: Instant) {
        // Prefer evicting a stale entry.
        let stale = self
            .peers
            .iter()
            .find(|(_, entry)| entry.is_stale(now))
            .map(|(k, _)| *k);
        if let Some(key) = stale {
            self.peers.remove(&key);
            return;
        }
        // Otherwise evict the oldest-seen entry.
        let oldest = self
            .peers
            .iter()
            .min_by_key(|(_, entry)| entry.last_seen)
            .map(|(k, _)| *k);
        if let Some(key) = oldest {
            self.peers.remove(&key);
        }
    }
}

// ---------------------------------------------------------------------------
// Guard: the composed privacy/abuse gate
// ---------------------------------------------------------------------------

/// Why the control-plane guard rejected a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRejectReason {
    /// The envelope's claimed `sender_node_id` differs from the
    /// authenticated gossip delivery source — a spoofing attempt.
    SpoofedSender,
    /// The authenticated sender exceeded the per-sender frame rate limit.
    RateLimited,
    /// The `(sender, sequence)` pair was already accepted, or the sequence
    /// is not newer than the last accepted one (duplicate / out-of-order).
    Duplicate,
    /// The advertisement violates the minimal-content whitelist / bounds.
    AdvertViolation(AdvertViolation),
}

/// Verdict of [`ControlPlaneGuard::admit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardVerdict {
    /// The frame passed every gate; the caller emits the control event and
    /// the presence state was updated.
    Accept,
    /// The frame was dropped for the given reason (bounded logging).
    Reject(GuardRejectReason),
}

/// The composed control-plane privacy/abuse gate (PDF Task 1.3).
///
/// Gate order (cheap, abuse-first):
///
/// 1. **Rate limit** by the authenticated delivery source — bounds log spam
///    and presence churn before any other work.
/// 2. **Attribution** — claimed `sender_node_id` must equal the
///    authenticated delivery source.
/// 3. **Minimal-content policy** — whitelist + bounds.
/// 4. **Dedup** by `(sender_node_id, sequence)`.
/// 5. **Presence state** — record/refresh; out-of-order older sequences are
///    dropped.
///
/// One instance is shared by the service receive path (wrapped in an
/// `Arc<Mutex<_>>`); it is owned solely by the discovery service and never
/// touches chat, friendship, group, file, or tunnel code.
#[derive(Debug, Clone)]
pub struct ControlPlaneGuard {
    rate_limiter: ControlPlaneRateLimiter,
    dedup: HashSet<(PublicKey, u64)>,
    dedup_cap: usize,
    presence: PeerControlStateStore,
    policy: ControlAdvertPolicy,
}

impl Default for ControlPlaneGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlPlaneGuard {
    /// A guard with the default limits.
    pub fn new() -> Self {
        Self {
            rate_limiter: ControlPlaneRateLimiter::new(),
            dedup: HashSet::new(),
            dedup_cap: CONTROL_DEDUP_CAP,
            presence: PeerControlStateStore::new(),
            policy: ControlAdvertPolicy::default(),
        }
    }

    /// A guard with explicit limits (tests use small windows/caps).
    pub fn with_limits(
        rate_limiter: ControlPlaneRateLimiter,
        dedup_cap: usize,
        presence: PeerControlStateStore,
        policy: ControlAdvertPolicy,
    ) -> Self {
        Self {
            rate_limiter,
            dedup: HashSet::new(),
            dedup_cap,
            presence,
            policy,
        }
    }

    /// Admit one decoded control-plane envelope.
    ///
    /// `delivered_from` is the **authenticated** gossip delivery source
    /// (the real peer that pushed the frame). `now` is explicit so tests
    /// can drive presence TTL deterministically.
    ///
    /// On [`GuardVerdict::Accept`] the presence store has already been
    /// updated; the caller emits [`ControlEvent::Received`](super::ControlEvent)
    /// (or its own equivalent) exactly once.
    pub fn admit(
        &mut self,
        envelope: &ControlEnvelope,
        delivered_from: PublicKey,
        now: Instant,
    ) -> GuardVerdict {
        // 1. Rate limit by the authenticated delivery source.
        if !self.rate_limiter.admit(&delivered_from) {
            return GuardVerdict::Reject(GuardRejectReason::RateLimited);
        }

        // 2. Attribution: the claimed sender must be the authenticated
        //    delivery source.
        if envelope.sender_node_id != delivered_from {
            return GuardVerdict::Reject(GuardRejectReason::SpoofedSender);
        }

        // 3. Minimal-content whitelist + bounds.
        if let Err(violation) = self.policy.check(envelope) {
            return GuardVerdict::Reject(GuardRejectReason::AdvertViolation(violation));
        }

        // 4. Dedup by (sender, sequence); bounded set.
        let key = envelope.dedup_key();
        if self.dedup.len() >= self.dedup_cap {
            self.dedup.clear();
        }
        if !self.dedup.insert(key) {
            return GuardVerdict::Reject(GuardRejectReason::Duplicate);
        }

        // 5. Presence state (out-of-order older sequence = duplicate).
        match self.presence.record(envelope, now) {
            StoreOutcome::Duplicate => GuardVerdict::Reject(GuardRejectReason::Duplicate),
            StoreOutcome::New | StoreOutcome::Refreshed => GuardVerdict::Accept,
        }
    }

    /// The bounded control-plane presence store (active presence hints).
    pub fn presence(&self) -> &PeerControlStateStore {
        &self.presence
    }

    /// Remove peers whose presence is stale at `now`; returns their ids.
    pub fn expire_stale(&mut self, now: Instant) -> Vec<PublicKey> {
        self.presence.expire_stale(now)
    }

    /// Change the default presence TTL for new entries.
    pub fn set_default_presence_ttl(&mut self, ttl: Duration) {
        self.presence.set_default_ttl(ttl);
    }

    /// Number of peers currently in the control-plane presence store.
    pub fn presence_count(&self) -> usize {
        self.presence.len()
    }

    /// Clear the dedup set (bounded-resources safety valve).
    pub fn clear_dedup(&mut self) {
        self.dedup.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::message::{ControlEnvelope, CONTROL_PLANE_PROTOCOL_VERSION};
    use std::collections::BTreeSet;

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    fn hello(sender: PublicKey, sequence: u64) -> ControlEnvelope {
        ControlEnvelope::hello(sender, sequence, 1_700_000_000, 1)
    }

    fn presence(sender: PublicKey, sequence: u64, ttl: Option<u32>) -> ControlEnvelope {
        ControlEnvelope::presence(sender, sequence, 1_700_000_000, ttl)
    }

    fn capabilities(sender: PublicKey, sequence: u64, caps: Vec<String>) -> ControlEnvelope {
        ControlEnvelope::capabilities(sender, sequence, 1_700_000_000, caps)
    }

    fn hint(sender: PublicKey, sequence: u64, note: Option<String>) -> ControlEnvelope {
        ControlEnvelope::diagnostic_hint(sender, sequence, 1_700_000_000, 1, note)
    }

    // ── Policy: minimal advertisement content ──────────────────────────

    #[test]
    fn policy_accepts_minimal_hello_presence_capabilities_hint() {
        let policy = ControlAdvertPolicy::default();
        let peer = key(0x01);
        assert!(policy.check(&hello(peer, 1)).is_ok());
        assert!(policy.check(&presence(peer, 2, None)).is_ok());
        assert!(policy.check(&presence(peer, 3, Some(120))).is_ok());
        assert!(policy
            .check(&capabilities(peer, 4, vec!["files-v2".into()]))
            .is_ok());
        assert!(policy
            .check(&hint(peer, 5, Some("relay-only".into())))
            .is_ok());
        assert!(policy.check(&hint(peer, 6, None)).is_ok());
    }

    /// BORU-DIR-01/02: a PUBLIC_ROOM_ADVERTISEMENT envelope passes the
    /// minimal-content whitelist when it carries a bounded, discoverable,
    /// metadata-only room advertisement (BORU-DIR-02 metadata model).
    #[test]
    fn policy_accepts_public_room_advertisement() {
        let policy = ControlAdvertPolicy::default();
        let peer = key(0x01);
        let advert = ControlEnvelope::public_room_advertisement(
            peer,
            1,
            1_700_000_000,
            crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
                crate::proto::state::TopicId::from_bytes([0x61; 32]),
                "Lobby".into(),
                {
                    let mut seed = [0u8; 32];
                    seed[0] = 0x02;
                    iroh_base::SecretKey::from_bytes(&seed)
                        .public()
                        .as_bytes()
                        .to_owned()
                },
            ),
        );
        assert!(
            policy.check(&advert).is_ok(),
            "a bounded metadata-only room advertisement must be accepted"
        );
    }

    /// BORU-DIR-02: a PUBLIC_ROOM_ADVERTISEMENT envelope carrying metadata
    /// that exceeds the advertisement bounds (oversized room name) is
    /// rejected by the minimal-content policy — it never reaches the
    /// directory.
    #[test]
    fn policy_rejects_oversized_room_advertisement() {
        let policy = ControlAdvertPolicy::default();
        let peer = key(0x03);
        let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x61; 32]),
            "Lobby".into(),
            {
                let mut seed = [0u8; 32];
                seed[0] = 0x04;
                iroh_base::SecretKey::from_bytes(&seed)
                    .public()
                    .as_bytes()
                    .to_owned()
            },
        );
        advert.room_name =
            "x".repeat(crate::control_plane::advertisement::DEFAULT_MAX_ROOM_NAME_LEN + 1);
        let envelope = ControlEnvelope::public_room_advertisement(peer, 2, 1_700_000_000, advert);
        let err = policy.check(&envelope).unwrap_err();
        assert!(
            matches!(
                err,
                AdvertViolation::Advertisement(
                    crate::control_plane::advertisement::AdvertisementViolation::RoomNameTooLong { .. }
                )
            ),
            "oversized room name must be rejected, got {err:?}"
        );
    }

    /// BORU-DIR-02 visibility guardrail: an advertisement claiming a
    /// non-discoverable visibility (Private/PublicUnlisted) is rejected —
    /// private and unlisted rooms are never advertised.
    #[test]
    fn policy_rejects_non_discoverable_room_advertisement() {
        let policy = ControlAdvertPolicy::default();
        let peer = key(0x05);
        for visibility in [
            crate::control_plane::advertisement::RoomVisibility::Private,
            crate::control_plane::advertisement::RoomVisibility::PublicUnlisted,
        ] {
            let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
                crate::proto::state::TopicId::from_bytes([0x61; 32]),
                "Lobby".into(),
                {
                    let mut seed = [0u8; 32];
                    seed[0] = 0x06;
                    iroh_base::SecretKey::from_bytes(&seed)
                        .public()
                        .as_bytes()
                        .to_owned()
                },
            );
            advert.visibility = visibility;
            let envelope =
                ControlEnvelope::public_room_advertisement(peer, 3, 1_700_000_000, advert);
            let err = policy.check(&envelope).unwrap_err();
            assert!(
                matches!(
                    err,
                    AdvertViolation::Advertisement(
                        crate::control_plane::advertisement::AdvertisementViolation::NotDiscoverable
                    )
                ),
                "non-discoverable advertisement must be rejected, got {err:?}"
            );
        }
    }

    #[test]
    fn policy_rejects_too_many_capabilities() {
        let policy = ControlAdvertPolicy::default();
        let caps: Vec<String> = (0..policy.max_capabilities + 1)
            .map(|i| format!("feat-{i}"))
            .collect();
        let err = policy.check(&capabilities(key(0x01), 1, caps)).unwrap_err();
        assert!(matches!(err, AdvertViolation::CapabilitiesTooMany { .. }));
    }

    #[test]
    fn policy_rejects_oversized_capability_id() {
        let policy = ControlAdvertPolicy::default();
        let long = "x".repeat(policy.max_capability_id_len + 1);
        let err = policy
            .check(&capabilities(key(0x01), 1, vec![long]))
            .unwrap_err();
        assert!(matches!(err, AdvertViolation::CapabilityTooLong { .. }));
    }

    #[test]
    fn policy_rejects_capability_ids_with_invalid_charset() {
        let policy = ControlAdvertPolicy::default();
        for bad in ["files v2", "files\nv2", "file\tname", "emoji-😀", "a,b"] {
            let err = policy
                .check(&capabilities(key(0x01), 1, vec![bad.to_string()]))
                .unwrap_err();
            assert!(
                matches!(err, AdvertViolation::CapabilityInvalid { .. }),
                "capability id {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn policy_rejects_empty_capability_id() {
        let policy = ControlAdvertPolicy::default();
        let err = policy
            .check(&capabilities(key(0x01), 1, vec![String::new()]))
            .unwrap_err();
        assert!(
            matches!(err, AdvertViolation::CapabilityTooLong { len: 0, .. }),
            "an empty capability id must be rejected"
        );
    }

    #[test]
    fn policy_rejects_oversized_diagnostic_note() {
        let policy = ControlAdvertPolicy::default();
        let note = "y".repeat(policy.max_diagnostic_note_len + 1);
        let err = policy.check(&hint(key(0x01), 1, Some(note))).unwrap_err();
        assert!(matches!(err, AdvertViolation::DiagnosticNoteTooLong { .. }));
    }

    #[test]
    fn policy_clamps_oversized_presence_ttl() {
        let policy = ControlAdvertPolicy::default();
        let err = policy
            .check(&presence(
                key(0x01),
                1,
                Some(policy.max_presence_ttl_secs + 1),
            ))
            .unwrap_err();
        assert!(matches!(err, AdvertViolation::PresenceTtlTooLarge { .. }));
    }

    // ── Extensions policy (BORU-CP-16, PDF Phase 6) ───────────────────

    fn extensions(
        sender: PublicKey,
        sequence: u64,
        payload: crate::control_plane::extensions::ExtensionsPayload,
    ) -> ControlEnvelope {
        ControlEnvelope::extensions(sender, sequence, 1_700_000_000, payload)
    }

    #[test]
    fn policy_accepts_minimal_and_full_extensions() {
        let policy = ControlAdvertPolicy::default();
        assert!(policy
            .check(&extensions(key(0x01), 1, Default::default()))
            .is_ok());
        let full = crate::control_plane::extensions::ExtensionsPayload {
            group: Some(crate::control_plane::extensions::GroupHints { available: true }),
            file: Some(crate::control_plane::extensions::FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: true,
            }),
            tunnel: Some(crate::control_plane::extensions::TunnelCapability {
                protocol_versions: vec!["v1".into()],
            }),
            call: Some(crate::control_plane::extensions::CallCapability {
                protocol_versions: vec!["v1".into()],
                availability: Some(crate::control_plane::extensions::CallAvailability::Available),
            }),
            screen_share: Some(crate::control_plane::extensions::ScreenShareCapability {
                protocol_versions: vec!["v1".into()],
            }),
            identity: Some(crate::control_plane::extensions::MultiDeviceIdentity {
                identity_id: "user-alice".into(),
                device_id: "dev-phone".into(),
                active_device: true,
            }),
            path_preference: Some(
                crate::control_plane::extensions::PathPreference::DirectPreferred,
            ),
            relay_health: Some(crate::control_plane::extensions::RelayHealthHint::Healthy),
        };
        assert!(policy.check(&extensions(key(0x01), 2, full)).is_ok());
    }

    #[test]
    fn policy_rejects_extensions_bound_violations() {
        let policy = ControlAdvertPolicy {
            extensions_bounds: crate::control_plane::extensions::ExtensionsBounds {
                max_protocol_versions: 1,
                max_version_len: 4,
                max_identity_id_len: 8,
                max_device_id_len: 8,
            },
            ..Default::default()
        };

        let too_many = crate::control_plane::extensions::ExtensionsPayload {
            file: Some(crate::control_plane::extensions::FileReadiness {
                protocol_versions: vec!["v1".into(), "v2".into()],
                can_receive: true,
            }),
            ..Default::default()
        };
        let err = policy
            .check(&extensions(key(0x01), 1, too_many))
            .unwrap_err();
        assert!(matches!(err, AdvertViolation::Extensions(_)));

        let bad_id = crate::control_plane::extensions::ExtensionsPayload {
            identity: Some(crate::control_plane::extensions::MultiDeviceIdentity {
                identity_id: "x".repeat(9),
                device_id: "dev".into(),
                active_device: true,
            }),
            ..Default::default()
        };
        let err = policy.check(&extensions(key(0x01), 2, bad_id)).unwrap_err();
        assert!(matches!(err, AdvertViolation::Extensions(_)));
    }

    // ── Rate limiter ──────────────────────────────────────────────────

    #[test]
    fn rate_limiter_allows_within_limit_rejects_excess() {
        let mut limiter = ControlPlaneRateLimiter::with_limits(3, Duration::from_secs(60), 16);
        let a = key(0x0A);
        assert!(limiter.admit(&a));
        assert!(limiter.admit(&a));
        assert!(limiter.admit(&a));
        assert!(
            !limiter.admit(&a),
            "4th frame within window must be limited"
        );
    }

    #[test]
    fn rate_limiter_peers_are_independent() {
        let mut limiter = ControlPlaneRateLimiter::with_limits(1, Duration::from_secs(60), 16);
        let a = key(0x0A);
        let b = key(0x0B);
        assert!(limiter.admit(&a));
        assert!(!limiter.admit(&a));
        assert!(limiter.admit(&b), "different peer must be independent");
    }

    #[test]
    fn rate_limiter_window_expires() {
        let mut limiter = ControlPlaneRateLimiter::with_limits(1, Duration::from_millis(30), 16);
        let a = key(0x0A);
        assert!(limiter.admit(&a));
        assert!(!limiter.admit(&a));
        std::thread::sleep(Duration::from_millis(60));
        assert!(limiter.admit(&a), "window expiry must allow a new frame");
    }

    #[test]
    fn rate_limiter_bounded_sender_count() {
        let mut limiter = ControlPlaneRateLimiter::with_limits(1, Duration::from_secs(60), 2);
        let a = key(0x0A);
        let b = key(0x0B);
        let c = key(0x0C);
        assert!(limiter.admit(&a));
        assert!(limiter.admit(&b));
        assert!(
            limiter.admit(&c),
            "evicting the oldest sender must admit a new one"
        );
        assert!(limiter.len() <= 2, "sender map must stay bounded");
    }

    // ── Presence store: TTL expiry ────────────────────────────────────

    #[test]
    fn presence_store_records_and_refreshes() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(300));
        let peer = key(0x0A);
        let t0 = Instant::now();

        assert_eq!(store.record(&hello(peer, 1), t0), StoreOutcome::New);
        assert!(store.contains(&peer));
        assert_eq!(
            store.record(&hello(peer, 2), t0 + Duration::from_secs(1)),
            StoreOutcome::Refreshed
        );
        assert_eq!(
            store.record(&hello(peer, 1), t0 + Duration::from_secs(2)),
            StoreOutcome::Duplicate
        );
        // A lower sequence never regresses state.
        assert_eq!(store.get(&peer).unwrap().last_sequence, 2);
    }

    #[test]
    fn presence_store_tracks_discovery_seen_at_across_refresh() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(300));
        let peer = key(0x0A);
        let t0 = Instant::now();

        // First sighting stamps discovery_seen_at.
        assert_eq!(store.record(&hello(peer, 1), t0), StoreOutcome::New);
        let state = store.get(&peer).unwrap();
        assert_eq!(state.discovery_seen_at, t0);
        assert_eq!(state.last_seen, t0);

        // A refresh moves last_seen but preserves the original discovery
        // time (discovery age is not activity recency).
        store.record(&presence(peer, 2, None), t0 + Duration::from_secs(30));
        let state = store.get(&peer).unwrap();
        assert_eq!(
            state.discovery_seen_at, t0,
            "discovery_seen_at must be preserved"
        );
        assert_eq!(state.last_seen, t0 + Duration::from_secs(30));

        // A duplicate/older delivery changes nothing.
        store.record(&hello(peer, 1), t0 + Duration::from_secs(60));
        let state = store.get(&peer).unwrap();
        assert_eq!(state.discovery_seen_at, t0);
        assert_eq!(state.last_seen, t0 + Duration::from_secs(30));
    }

    #[test]
    fn presence_state_is_derived_from_activity_and_ttl_never_stored() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(10));
        let peer = key(0x0A);
        let t0 = Instant::now();
        store.record(&hello(peer, 1), t0);

        // Within TTL → Active; after TTL → Stale — always computed, never
        // persisted as a permanent 'online' flag.
        let state = store.get(&peer).unwrap();
        assert_eq!(
            state.presence_state(t0 + Duration::from_secs(5)),
            PresenceState::Active
        );
        assert_eq!(
            state.presence_state(t0 + Duration::from_secs(11)),
            PresenceState::Stale
        );
        assert_eq!(
            state.presence_state(t0 + Duration::from_secs(11)),
            state.presence_state(t0 + Duration::from_secs(50))
        );
    }

    #[test]
    fn presence_store_capabilities_update_on_refresh() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(300));
        let peer = key(0x0A);
        let t0 = Instant::now();
        store.record(&capabilities(peer, 1, vec!["files-v2".into()]), t0);
        store.record(
            &capabilities(peer, 2, vec!["files-v2".into(), "voice-v1".into()]),
            t0 + Duration::from_secs(1),
        );
        let state = store.get(&peer).unwrap();
        assert_eq!(
            state.capabilities,
            vec!["files-v2".to_string(), "voice-v1".to_string()]
        );
        // A HELLO refreshes app version without wiping capabilities.
        store.record(&hello(peer, 3), t0 + Duration::from_secs(2));
        let state = store.get(&peer).unwrap();
        assert_eq!(state.app_protocol_version, Some(1));
        assert_eq!(state.capabilities.len(), 2);
    }

    /// The store caches the peer's EXTENSIONS advertisement (BORU-CP-16),
    /// refreshes it on a newer sequence, and returns it via
    /// [`PeerControlStateStore::extensions_of`]. A peer that never
    /// advertised extensions has none cached.
    #[test]
    fn presence_store_extensions_update_on_refresh() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(300));
        let peer = key(0x0A);
        let t0 = Instant::now();
        let payload = crate::control_plane::extensions::ExtensionsPayload {
            file: Some(crate::control_plane::extensions::FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: true,
            }),
            ..Default::default()
        };
        store.record(&extensions(peer, 1, payload.clone()), t0);
        assert_eq!(store.extensions_of(&peer), Some(payload.clone()));

        // A newer extensions advertisement replaces it.
        let payload2 = crate::control_plane::extensions::ExtensionsPayload {
            file: Some(crate::control_plane::extensions::FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: false,
            }),
            ..Default::default()
        };
        store.record(
            &extensions(peer, 2, payload2.clone()),
            t0 + Duration::from_secs(1),
        );
        assert_eq!(store.extensions_of(&peer), Some(payload2.clone()));

        // A HELLO refreshes presence WITHOUT wiping the extensions cache.
        store.record(&hello(peer, 3), t0 + Duration::from_secs(2));
        assert_eq!(store.extensions_of(&peer), Some(payload2));

        // A peer that never advertised extensions has none cached.
        let silent = key(0x0B);
        store.record(&hello(silent, 1), t0);
        assert_eq!(store.extensions_of(&silent), None);
    }

    /// The typed capability view is lossless: unknown future ids are
    /// preserved, and the wire form round-trips.
    #[test]
    fn presence_store_capability_set_typed_query_preserves_unknowns() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(300));
        let peer = key(0x0A);
        let t0 = Instant::now();
        store.record(
            &capabilities(
                peer,
                1,
                vec![
                    "files-v2".into(),
                    "hologram-v3".into(),
                    "files-v2.1-beta".into(),
                ],
            ),
            t0,
        );

        let set = store.capability_set_of(&peer).expect("caps must be cached");
        assert!(set.has_feature("files"));
        assert_eq!(set.versions_of("files"), Some(&BTreeSet::from([2u16])));
        // Unknown ids survive the typed view.
        let wire = set.to_wire();
        assert!(wire.iter().any(|id| id == "hologram-v3"));
        assert!(wire.iter().any(|id| id == "files-v2.1-beta"));
        assert_eq!(CapabilitySet::from_wire(wire), set);

        // A peer that never advertised capabilities has none cached.
        let silent = key(0x0B);
        store.record(&hello(silent, 1), t0);
        assert_eq!(store.capability_set_of(&silent), None);
    }

    /// Stale capability data is not treated as current: get_active returns
    /// None once presence is beyond its TTL, get_stale exposes the last
    /// known state explicitly, and expire_stale removes the capabilities
    /// together with the presence entry.
    #[test]
    fn presence_store_stale_capabilities_not_current() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(5));
        let peer = key(0x0A);
        let t0 = Instant::now();
        store.record(&capabilities(peer, 1, vec!["files-v2".into()]), t0);

        // Within TTL: active and current.
        assert!(store
            .get_active(&peer, t0 + Duration::from_secs(4))
            .is_some());
        assert!(store
            .get_stale(&peer, t0 + Duration::from_secs(4))
            .is_none());
        assert!(store.capability_set_of(&peer).is_some());

        // Beyond TTL: NOT current (active lookup fails closed).
        let stale_now = t0 + Duration::from_secs(6);
        assert!(
            store.get_active(&peer, stale_now).is_none(),
            "stale capability data must not be treated as current"
        );
        // The last-known state is still readable as stale.
        assert!(store.get_stale(&peer, stale_now).is_some());
        // But the typed cache accessor still returns the raw data — the
        // caller decides staleness via get_active/get_stale.
        assert!(store.capability_set_of(&peer).is_some());

        // Expiry removes the capabilities WITH the presence entry.
        let expired = store.expire_stale(stale_now);
        assert_eq!(expired, vec![peer]);
        assert_eq!(store.capability_set_of(&peer), None);
    }

    #[test]
    fn presence_store_expires_stale_peers_after_ttl() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(5));
        let a = key(0x0A);
        let b = key(0x0B);
        let t0 = Instant::now();
        store.record(&hello(a, 1), t0);
        store.record(&hello(b, 1), t0);

        // Nothing stale yet.
        assert!(store.expire_stale(t0 + Duration::from_secs(4)).is_empty());
        assert_eq!(store.len(), 2);

        // After the TTL (5s), both are stale and disappear.
        let expired = store.expire_stale(t0 + Duration::from_secs(6));
        assert_eq!(expired.len(), 2);
        assert!(
            store.is_empty(),
            "stale peers must disappear from active presence"
        );
    }

    #[test]
    fn presence_store_expires_only_stale_peers() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(10));
        let a = key(0x0A);
        let b = key(0x0B);
        let t0 = Instant::now();
        store.record(&hello(a, 1), t0);
        store.record(&hello(b, 1), t0);
        // B refreshes at t+6 — its TTL now ends at t+16.
        store.record(&hello(b, 2), t0 + Duration::from_secs(6));

        let expired = store.expire_stale(t0 + Duration::from_secs(11));
        assert_eq!(expired, vec![a], "only A is stale at t+11");
        assert!(store.contains(&b));
    }

    #[test]
    fn presence_store_bounded_eviction_oldest_first() {
        let mut store = PeerControlStateStore::with_limits(2, Duration::from_secs(300));
        let a = key(0x0A);
        let b = key(0x0B);
        let c = key(0x0C);
        let t0 = Instant::now();
        store.record(&hello(a, 1), t0);
        store.record(&hello(b, 1), t0 + Duration::from_secs(1));
        store.record(&hello(c, 1), t0 + Duration::from_secs(2));

        assert_eq!(store.len(), 2, "store must stay bounded");
        assert!(!store.contains(&a), "oldest entry must be evicted");
        assert!(store.contains(&b));
        assert!(store.contains(&c));
    }

    #[test]
    fn presence_store_evicts_stale_before_oldest() {
        let mut store = PeerControlStateStore::with_limits(2, Duration::from_secs(5));
        let a = key(0x0A);
        let b = key(0x0B);
        let c = key(0x0C);
        let t0 = Instant::now();
        store.record(&hello(a, 1), t0); // stale by t+6
        store.record(&hello(b, 1), t0 + Duration::from_secs(4));
        // a is stale now; b is not.
        store.record(&hello(c, 1), t0 + Duration::from_secs(6));

        assert!(!store.contains(&a), "stale entry must be evicted first");
        assert!(store.contains(&b));
        assert!(store.contains(&c));
    }

    // ── Guard: composed gates ─────────────────────────────────────────

    #[test]
    fn guard_accepts_minimal_frame_and_records_presence() {
        let mut guard = ControlPlaneGuard::new();
        let peer = key(0x0B);
        let now = Instant::now();
        let env = hello(peer, 1);
        assert_eq!(guard.admit(&env, peer, now), GuardVerdict::Accept);
        assert_eq!(guard.presence_count(), 1);
        assert!(guard.presence().contains(&peer));
    }

    #[test]
    fn guard_rejects_spoofed_sender() {
        let mut guard = ControlPlaneGuard::new();
        let claimed = key(0x0B);
        let actual = key(0x0C); // the authenticated delivery source
        let now = Instant::now();
        let env = hello(claimed, 1);
        assert_eq!(
            guard.admit(&env, actual, now),
            GuardVerdict::Reject(GuardRejectReason::SpoofedSender),
            "an envelope claiming a different identity must be dropped"
        );
        assert_eq!(guard.presence_count(), 0);
    }

    #[test]
    fn guard_rejects_rate_limited_sender() {
        let mut guard = ControlPlaneGuard::with_limits(
            ControlPlaneRateLimiter::with_limits(2, Duration::from_secs(60), 16),
            CONTROL_DEDUP_CAP,
            PeerControlStateStore::with_limits(16, Duration::from_secs(300)),
            ControlAdvertPolicy::default(),
        );
        let peer = key(0x0B);
        let now = Instant::now();
        assert_eq!(
            guard.admit(&hello(peer, 1), peer, now),
            GuardVerdict::Accept
        );
        assert_eq!(
            guard.admit(&hello(peer, 2), peer, now),
            GuardVerdict::Accept
        );
        assert_eq!(
            guard.admit(&hello(peer, 3), peer, now),
            GuardVerdict::Reject(GuardRejectReason::RateLimited),
            "third frame within the window must be rate limited"
        );
    }

    #[test]
    fn guard_rejects_duplicate_sequence() {
        let mut guard = ControlPlaneGuard::new();
        let peer = key(0x0B);
        let now = Instant::now();
        let env = hello(peer, 7);
        assert_eq!(guard.admit(&env, peer, now), GuardVerdict::Accept);
        assert_eq!(
            guard.admit(&env, peer, now + Duration::from_secs(1)),
            GuardVerdict::Reject(GuardRejectReason::Duplicate),
            "same (sender, sequence) must be deduplicated with no side effects"
        );
    }

    #[test]
    fn guard_rejects_out_of_order_sequence() {
        let mut guard = ControlPlaneGuard::new();
        let peer = key(0x0B);
        let now = Instant::now();
        assert_eq!(
            guard.admit(&hello(peer, 5), peer, now),
            GuardVerdict::Accept
        );
        assert_eq!(
            guard.admit(&hello(peer, 3), peer, now + Duration::from_secs(1)),
            GuardVerdict::Reject(GuardRejectReason::Duplicate),
            "an older sequence must not regress presence state"
        );
    }

    #[test]
    fn guard_rejects_advert_violation() {
        let mut guard = ControlPlaneGuard::new();
        let peer = key(0x0B);
        let now = Instant::now();
        let env = presence(peer, 1, Some(MAX_PRESENCE_TTL_SECS + 1));
        assert_eq!(
            guard.admit(&env, peer, now),
            GuardVerdict::Reject(GuardRejectReason::AdvertViolation(
                AdvertViolation::PresenceTtlTooLarge {
                    ttl: MAX_PRESENCE_TTL_SECS + 1,
                    max: MAX_PRESENCE_TTL_SECS,
                }
            ))
        );
        assert_eq!(guard.presence_count(), 0);
    }

    #[test]
    fn guard_expires_stale_presence() {
        let mut guard = ControlPlaneGuard::new();
        let peer = key(0x0B);
        let now = Instant::now();
        // Set a short default TTL BEFORE recording so the entry inherits it.
        guard.set_default_presence_ttl(Duration::from_secs(1));
        assert_eq!(
            guard.admit(&hello(peer, 1), peer, now),
            GuardVerdict::Accept
        );
        assert_eq!(guard.presence_count(), 1);
        let expired = guard.expire_stale(now + Duration::from_secs(2));
        assert_eq!(expired, vec![peer]);
        assert_eq!(
            guard.presence_count(),
            0,
            "stale peer must disappear from active presence"
        );
    }

    #[test]
    fn guard_accepts_different_sequences_from_same_sender() {
        let mut guard = ControlPlaneGuard::new();
        let peer = key(0x0B);
        let now = Instant::now();
        assert_eq!(
            guard.admit(&hello(peer, 1), peer, now),
            GuardVerdict::Accept
        );
        assert_eq!(
            guard.admit(&presence(peer, 2, None), peer, now),
            GuardVerdict::Accept
        );
        assert_eq!(
            guard.admit(&capabilities(peer, 3, vec!["files-v2".into()]), peer, now),
            GuardVerdict::Accept
        );
        assert_eq!(guard.presence_count(), 1, "one sender = one presence entry");
        assert_eq!(
            guard.presence().get(&peer).unwrap().capabilities,
            vec!["files-v2".to_string()]
        );
    }

    // ── Presence never grants authorisation ───────────────────────────

    /// The control-plane presence store is a metadata cache ONLY: it has no
    /// friendship/group/file/tunnel surface, and the guard never consults
    /// any trust store. A peer that is fully present in the control plane is
    /// still not a friend, group member, tunnel client, or file recipient.
    #[test]
    fn presence_never_grants_authorisation() {
        let mut guard = ControlPlaneGuard::new();
        let peer = key(0x0B);
        let now = Instant::now();
        // Advertise everything a peer can advertise.
        assert_eq!(
            guard.admit(&hello(peer, 1), peer, now),
            GuardVerdict::Accept
        );
        assert_eq!(
            guard.admit(
                &capabilities(peer, 2, vec!["files-v2".into(), "tunnels-v1".into()]),
                peer,
                now
            ),
            GuardVerdict::Accept
        );
        assert_eq!(
            guard.admit(&presence(peer, 3, None), peer, now),
            GuardVerdict::Accept
        );

        let state = guard.presence().get(&peer).unwrap();
        // The state is metadata-only: identity + advertised protocol info.
        assert_eq!(state.peer_id, peer);
        assert_eq!(state.protocol_version, CONTROL_PLANE_PROTOCOL_VERSION);
        assert_eq!(state.app_protocol_version, Some(1));
        assert_eq!(
            state.capabilities,
            vec!["files-v2".to_string(), "tunnels-v1".to_string()]
        );

        // Authorisation surfaces do not exist here: there is no method on
        // the store or the state that grants friendship, group membership,
        // tunnel access, or file-recipient status. The guard's public API
        // only admits/rejects frames and manages presence hints — it cannot
        // create a conversation, friend, group, or transfer.
        // (Compile-time guarantee; the assertion below documents the shape.)
        assert_eq!(
            guard.presence().len(),
            1,
            "presence state is a hint cache — nothing more"
        );
        // And it does not leak into friendship/trust: there is no way to
        // ask this store whether `peer` may chat with us, because that
        // decision lives in the friends/trust stores this module never
        // imports or touches.
    }

    /// Envelope protocol version flows into presence state.
    #[test]
    fn presence_state_carries_protocol_version() {
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(300));
        let peer = key(0x0A);
        let t0 = Instant::now();
        store.record(&hello(peer, 1), t0);
        assert_eq!(
            store.get(&peer).unwrap().protocol_version,
            CONTROL_PLANE_PROTOCOL_VERSION
        );
    }
}
