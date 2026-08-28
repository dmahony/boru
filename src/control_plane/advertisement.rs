//! Versioned, bounded room-discovery advertisement metadata (BORU-DIR-02,
//! PDF Phase 1 Task 1.2).
//!
//! This module defines the **advertised room metadata model**: the typed
//! [`PublicRoomAdvertisement`] payload carried by the
//! PUBLIC_ROOM_ADVERTISEMENT control-plane message (tag 5, added by
//! BORU-DIR-01). It advertises enough information to browse a room and
//! decide whether to join, without exposing unnecessary metadata.
//!
//! # Privacy guardrails (from the PDF)
//!
//! The advertisement is **metadata only, by construction**. The struct has
//! exactly the fields documented below and nothing else: there is no field
//! that can carry a member list, member identities, chat history, chat
//! previews, filenames, invite secrets, moderation state, private keys, or
//! attachment content. Adding any such field is a protocol change that must
//! be rejected in review and would break the "no private data fields"
//! regression test in this module.
//!
//! The discovery network only advertises that a room **exists**. It never
//! joins the room, subscribes to its chat topic, downloads its history, or
//! grants permission (PDF Core rule).
//!
//! # Bounded by design
//!
//! Every variable-size field is capped by [`AdvertisementBounds`]: the room
//! name, short description, tag count + tag length, and feature-flag count +
//! flag length all have strict maxima, and the total postcard-encoded
//! payload is capped by [`AdvertisementBounds::max_encoded_len`] so a fully
//! maxed-out advertisement still fits comfortably inside the control-plane
//! envelope cap ([`MAX_CONTROL_PAYLOAD_LEN`](crate::control_plane::message::MAX_CONTROL_PAYLOAD_LEN)).
//!
//! # Versioning
//!
//! `advert_version` is the advertisement **payload** version (currently
//! [`ADVERTISEMENT_PAYLOAD_VERSION`]). It is independent from the
//! control-plane envelope version and the room protocol version. Receivers
//! treat an unknown future `advert_version` as metadata to cache — never as
//! an authorisation signal (PDF Task 1.3 step 4).
//!
//! # Authentication (BORU-DIR-03)
//!
//! Advertisements are signed by the **publisher** — the node that sends the
//! PUBLIC_ROOM_ADVERTISEMENT control message, whose identity is the
//! envelope's `sender_node_id`. [`PublicRoomAdvertisement::sign`] stamps the
//! payload with an Ed25519 signature over the canonical framing of every
//! security-relevant field (see [`signing_bytes`](PublicRoomAdvertisement::signing_bytes)),
//! using the same [`crate::protocol_signing`] primitives as the rest of
//! Boru's authenticated protocol objects.
//!
//! Receivers call [`verify_signed`](PublicRoomAdvertisement::verify_signed)
//! against the **claimed publisher** (the envelope `sender_node_id`) before
//! the advertisement may enter the trusted directory view:
//!
//! * [`AdvertisementAuth::Verified`] — signature valid; the advertisement
//!   is attributed to that publisher.
//! * [`AdvertisementAuth::MissingSignature`] — no signature; the
//!   advertisement is **clearly untrusted** (it may be listed as
//!   unverified, never as canonical).
//! * [`AdvertisementAuth::InvalidSignature`] — signature present but
//!   invalid; the advertisement is forged/tampered and must be discarded.
//!
//! [`owner_peer_id`](PublicRoomAdvertisement::owner_peer_id) remains
//! **descriptive metadata**: a valid signature proves who *published* the
//! advertisement, not who *owns* the room. Room ownership is
//! cryptographically proven only when the verified publisher is the room
//! authority — see [`is_authoritative_publisher`](PublicRoomAdvertisement::is_authoritative_publisher)
//! and the design note `docs/public-room-directory/advertisement-authentication.md`.
//!
//! # Wire compatibility
//!
//! The payload is postcard-encoded as part of [`ControlPayload`](crate::control_plane::message::ControlPayload).
//! New fields are appended at the END of the struct so older clients decode
//! the known prefix and ignore the trailing bytes (the envelope decoder uses
//! `postcard::take_from_bytes` and discards trailing payload bytes). The
//! `signature` field is the last field and is `#[serde(default)]`, so older
//! clients ignore it and newer clients treat its absence as
//! [`AdvertisementAuth::MissingSignature`].

use crate::proto::state::TopicId;
use iroh_base::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Current advertisement payload version (PDF Task 1.2 step 1). Bump when
/// the metadata model changes incompatibly; receivers treat unknown versions
/// as metadata to cache, never as an authorisation signal.
pub const ADVERTISEMENT_PAYLOAD_VERSION: u8 = 1;

/// Domain-separation tag for the advertisement publisher signature
/// (BORU-DIR-03). A signature over this tag can never verify as a signature
/// over any other Boru protocol object family (mailbox acks, tunnel
/// capabilities, download descriptors, ...), even if the signed fields
/// happened to line up byte-for-byte.
///
/// The signed-bytes layout follows [`crate::protocol_signing::canonical_signed_bytes`]:
/// `postcard((protocol, version, (publisher, advert_version, room_id,
/// room_name, short_description, room_protocol_version, owner_peer_id,
/// visibility, expires_after_secs, tags, last_active_hint_secs,
/// approximate_member_count, room_avatar_hash, feature_flags)))`.
pub const ADVERTISEMENT_SIGNING_PROTOCOL: &str = "boru/public-room-advertisement/v1";

// ---------------------------------------------------------------------------
// Publisher authentication (BORU-DIR-03)
// ---------------------------------------------------------------------------

/// Outcome of verifying a room advertisement's publisher signature.
///
/// Produced by [`PublicRoomAdvertisement::verify_signed`] against the
/// **claimed publisher** — the control-plane envelope's `sender_node_id`,
/// which the transport attribution gate (BORU-CP-03) has already bound to
/// the authenticated gossip delivery source.
///
/// # Trust model
///
/// * [`Verified`](Self::Verified) — the payload can be attributed to the
///   claimed publisher; it may enter the trusted directory view.
/// * [`MissingSignature`](Self::MissingSignature) — the publisher did not
///   sign the payload; the advertisement is **clearly untrusted** and must
///   never be treated as canonical metadata (PDF Task 1.3: "Failed
///   verification results in discard or clearly untrusted state").
/// * [`InvalidSignature`](Self::InvalidSignature) — a signature is present
///   but does not verify for the claimed publisher; the payload was
///   tampered with or forged, and must be **discarded**.
///
/// Authentication only attributes the advertisement to a publisher. It
/// never grants moderation or join privileges (PDF Task 1.3 step 4): even a
/// [`Verified`](Self::Verified) advertisement is metadata, and room-level
/// authorization (join/permission/moderation) stays with the normal
/// room logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertisementAuth {
    /// The signature is present and valid for `publisher` — the
    /// advertisement is attributed to that node.
    Verified {
        /// The verified publisher (matches the envelope `sender_node_id`).
        publisher: PublicKey,
    },
    /// No signature present. The advertisement cannot be attributed to a
    /// publisher at the payload level: clearly untrusted, never canonical.
    MissingSignature,
    /// A signature is present but does not verify for the claimed
    /// publisher — forged or tampered advertisement. Discard.
    InvalidSignature,
}

impl AdvertisementAuth {
    /// Whether this auth state is trusted enough to enter the trusted
    /// directory view (only a valid publisher signature is trusted).
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

/// Default maximum length (Unicode characters) of a room name.
pub const DEFAULT_MAX_ROOM_NAME_LEN: usize = 64;
/// Default maximum length (Unicode characters) of the short description.
pub const DEFAULT_MAX_DESCRIPTION_LEN: usize = 256;
/// Default maximum number of tags.
pub const DEFAULT_MAX_TAGS: usize = 8;
/// Default maximum length (Unicode characters) of a single tag.
pub const DEFAULT_MAX_TAG_LEN: usize = 24;
/// Default maximum number of compatible feature flags.
pub const DEFAULT_MAX_FEATURE_FLAGS: usize = 8;
/// Default maximum length (Unicode characters) of a single feature flag id.
pub const DEFAULT_MAX_FEATURE_FLAG_LEN: usize = 48;
/// Default minimum advertisement TTL (seconds). Shorter TTLs are rejected:
/// a room that refreshes faster than this would be pure network churn.
pub const DEFAULT_MIN_ADVERT_TTL_SECS: u32 = 60;
/// Default maximum advertisement TTL (seconds) — 7 days. Longer TTLs are
/// rejected so a stale room cannot linger in directories forever.
pub const DEFAULT_MAX_ADVERT_TTL_SECS: u32 = 7 * 24 * 60 * 60;
/// Default TTL used by [`PublicRoomAdvertisement::minimal`] (1 hour).
pub const DEFAULT_ADVERT_TTL_SECS: u32 = 60 * 60;

/// Field tags used by [`AdvertisementViolation::ControlChar`].
pub mod fields {
    /// The room name field.
    pub const ROOM_NAME: u8 = 0;
    /// The short description field.
    pub const DESCRIPTION: u8 = 1;
    /// A tag field.
    pub const TAG: u8 = 2;
}

// ---------------------------------------------------------------------------
// Visibility model
// ---------------------------------------------------------------------------

/// Room visibility (PDF recommended visibility model).
///
/// Only [`RoomVisibility::PublicDiscoverable`] rooms are ever advertised:
/// Private and PublicUnlisted rooms emit **no** PUBLIC_ROOM_ADVERTISEMENT
/// (the visibility model in the PDF, and an advertisement carrying a
/// non-discoverable visibility is rejected by
/// [`PublicRoomAdvertisement::validate`] as [`AdvertisementViolation::NotDiscoverable`]).
///
/// # Wire stability
///
/// The variant order is the stable postcard wire tag:
/// `Private = 0`, `PublicUnlisted = 1`, `PublicDiscoverable = 2`.
/// Do not reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RoomVisibility {
    /// Closed groups / private communities — invite/authorisation only.
    /// Never advertised.
    #[default]
    Private,
    /// Shareable but not browsable — requires room ID/invite/link.
    /// Never advertised.
    PublicUnlisted,
    /// Open public communities — advertised in the directory and joinable
    /// via an explicit Join action.
    PublicDiscoverable,
}

impl RoomVisibility {
    /// Whether this room may emit directory advertisements
    /// (BORU-DIR-04, PDF visibility model): only
    /// [`RoomVisibility::PublicDiscoverable`] rooms are advertised.
    pub fn is_discoverable(self) -> bool {
        self == RoomVisibility::PublicDiscoverable
    }
}

// ---------------------------------------------------------------------------
// Directory visibility switching (BORU-DIR-06, PDF Phase 2 Task 2.3)
// ---------------------------------------------------------------------------

/// Outcome of an owner/admin request to change a room's directory visibility
/// (BORU-DIR-06, PDF Task 2.3).
///
/// Produced by [`plan_visibility_switch`] from the room's current visibility,
/// the requested visibility, and the existing room permission model
/// (`is_owner`). The caller performs the side effects the outcome names:
///
/// * [`Published`](Self::Published) — the room is now `PublicDiscoverable`;
///   publish a fresh advertisement immediately and keep refreshing.
/// * [`Unlisted`](Self::Unlisted) — the room is now `PublicUnlisted`; stop
///   refreshing. If the protocol has a withdrawal/tombstone message it should
///   be sent; otherwise remote directories drop the advertisement on TTL
///   expiry (BORU-DIR-09 adds the explicit tombstone later).
/// * [`NoChange`](Self::NoChange) — the room is already in the requested
///   state; no side effects.
/// * [`Forbidden`](Self::Forbidden) — the requester is not authorized to
///   change directory visibility; nothing changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilitySwitchOutcome {
    /// The room is now `PublicDiscoverable` — publish a fresh advertisement.
    Published,
    /// The room is now `PublicUnlisted` — stop refreshing (withdrawal /
    /// TTL expiry applies; no tombstone message exists yet).
    Unlisted,
    /// The room is already in the requested state.
    NoChange,
    /// The requester is not the room owner/admin — nothing changes.
    Forbidden,
}

/// Plan a directory-visibility switch for a room (BORU-DIR-06, PDF Task 2.3
/// step 1).
///
/// The switch is a pure decision: it never touches the store or the network.
/// The caller applies the returned [`VisibilitySwitchOutcome`].
///
/// # Permission model
///
/// Only the room **owner/admin** may change directory visibility (PDF Task
/// 2.3: "Do not: let non-authorized users change directory visibility"). The
/// caller decides `is_owner` using the existing room permission model — e.g.
/// the local user created the room as a public room, or already advertises
/// it (see the app's `is_room_directory_owner`).
///
/// # Scope
///
/// Only the `PublicDiscoverable <-> PublicUnlisted` transition is a
/// directory switch. `Private` rooms are never advertised (PDF visibility
/// model); switching a room into or out of `Private` is out of scope for the
/// directory controls and is rejected as [`Forbidden`](Self::Forbidden).
pub fn plan_visibility_switch(
    current: RoomVisibility,
    requested: RoomVisibility,
    is_owner: bool,
) -> VisibilitySwitchOutcome {
    if !is_owner {
        return VisibilitySwitchOutcome::Forbidden;
    }
    if current == requested {
        return VisibilitySwitchOutcome::NoChange;
    }
    match (current, requested) {
        (RoomVisibility::PublicUnlisted, RoomVisibility::PublicDiscoverable) => {
            VisibilitySwitchOutcome::Published
        }
        (RoomVisibility::PublicDiscoverable, RoomVisibility::PublicUnlisted) => {
            VisibilitySwitchOutcome::Unlisted
        }
        // Anything involving Private is not a directory switch.
        _ => VisibilitySwitchOutcome::Forbidden,
    }
}

// ---------------------------------------------------------------------------
// Advertisement bounds
// ---------------------------------------------------------------------------

/// Bounds applied to a [`PublicRoomAdvertisement`] by the privacy layer.
///
/// Mirrors the bounded-resources guardrail used for capabilities and
/// extensions: a peer cannot grow our memory or smuggle content through the
/// metadata channels beyond these caps, and the total encoded size is
/// capped so the advertisement stays compact (PDF acceptance criterion:
/// "The advertisement is compact and bounded").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvertisementBounds {
    /// Maximum length (Unicode characters) of a room name.
    pub max_room_name_len: usize,
    /// Maximum length (Unicode characters) of the short description.
    pub max_description_len: usize,
    /// Maximum number of tags.
    pub max_tags: usize,
    /// Maximum length (Unicode characters) of a single tag.
    pub max_tag_len: usize,
    /// Maximum number of compatible feature flags.
    pub max_feature_flags: usize,
    /// Maximum length (Unicode characters) of a single feature flag id.
    pub max_feature_flag_len: usize,
    /// Minimum TTL (seconds) for `expires_after_secs`.
    pub min_ttl_secs: u32,
    /// Maximum TTL (seconds) for `expires_after_secs`.
    pub max_ttl_secs: u32,
    /// Maximum postcard-encoded payload length (bytes).
    pub max_encoded_len: usize,
}

impl Default for AdvertisementBounds {
    fn default() -> Self {
        Self {
            max_room_name_len: DEFAULT_MAX_ROOM_NAME_LEN,
            max_description_len: DEFAULT_MAX_DESCRIPTION_LEN,
            max_tags: DEFAULT_MAX_TAGS,
            max_tag_len: DEFAULT_MAX_TAG_LEN,
            max_feature_flags: DEFAULT_MAX_FEATURE_FLAGS,
            max_feature_flag_len: DEFAULT_MAX_FEATURE_FLAG_LEN,
            min_ttl_secs: DEFAULT_MIN_ADVERT_TTL_SECS,
            max_ttl_secs: DEFAULT_MAX_ADVERT_TTL_SECS,
            // Well under the control-plane envelope cap (4096), leaving
            // room for the envelope header + future appended fields.
            max_encoded_len: 2048,
        }
    }
}

// ---------------------------------------------------------------------------
// Violations
// ---------------------------------------------------------------------------

/// Why a [`PublicRoomAdvertisement`] violates the metadata bounds or the
/// privacy guardrails.
///
/// All fields are numeric so the enum stays `Copy` (it is surfaced through
/// [`AdvertViolation`](crate::control_plane::privacy::AdvertViolation)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertisementViolation {
    /// The room name is empty.
    RoomNameEmpty,
    /// The room name is longer than `max`.
    RoomNameTooLong {
        /// Length of the offending name (Unicode chars).
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// The short description is longer than `max`.
    DescriptionTooLong {
        /// Length of the offending description (Unicode chars).
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// An advertisement carries more tags than `max`.
    TooManyTags {
        /// Number of tags in the advertisement.
        count: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A tag is longer than `max`.
    TagTooLong {
        /// Index of the offending tag.
        index: usize,
        /// Length of the offending tag (Unicode chars).
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A tag is empty.
    TagEmpty {
        /// Index of the offending tag.
        index: usize,
    },
    /// An advertisement carries more feature flags than `max`.
    TooManyFeatureFlags {
        /// Number of feature flags in the advertisement.
        count: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A feature flag is longer than `max`.
    FeatureFlagTooLong {
        /// Index of the offending flag.
        index: usize,
        /// Length of the offending flag (Unicode chars).
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
    /// A feature flag is empty or contains characters outside the metadata
    /// charset (`[A-Za-z0-9._-]`).
    FeatureFlagInvalid {
        /// Index of the offending flag.
        index: usize,
    },
    /// `expires_after_secs` is below the protocol minimum.
    TtlTooSmall {
        /// TTL suggested by the peer.
        ttl: u32,
        /// Minimum allowed.
        min: u32,
    },
    /// `expires_after_secs` is above the protocol maximum.
    TtlTooLarge {
        /// TTL suggested by the peer.
        ttl: u32,
        /// Maximum allowed.
        max: u32,
    },
    /// The advertisement's visibility is not PublicDiscoverable — private
    /// and unlisted rooms must never be advertised (PDF visibility model).
    NotDiscoverable,
    /// `owner_peer_id` is not a valid iroh Ed25519 public key.
    InvalidOwnerPeerId,
    /// A free-form field contains an ASCII control character (log-injection
    /// / display-injection defence).
    ControlChar {
        /// [`fields`](mod@self::fields) tag of the offending field.
        field: u8,
    },
    /// The postcard-encoded payload exceeds `max` bytes.
    EncodedTooLarge {
        /// Encoded length (bytes).
        len: usize,
        /// Maximum allowed.
        max: usize,
    },
}

// ---------------------------------------------------------------------------
// The advertisement payload
// ---------------------------------------------------------------------------

/// The versioned, bounded room-discovery advertisement payload
/// (BORU-DIR-02, PDF Task 1.2).
///
/// Required fields (all always present):
/// * `advert_version` — advertisement payload version.
/// * `room_id` — stable room identity (the gossip [`TopicId`]).
/// * `room_name` — display name (bounded).
/// * `short_description` — short description (bounded).
/// * `room_protocol_version` — room chat protocol version.
/// * `owner_peer_id` — creator/owner peer id (raw iroh Ed25519 key bytes).
/// * `visibility` — must be [`RoomVisibility::PublicDiscoverable`].
/// * `expires_after_secs` — TTL: the expiry/refresh mechanism.
///
/// Optional fields (all `#[serde(default)]`, forward-compatible):
/// * `tags` — searchable category tags (bounded).
/// * `last_active_hint_secs` — coarse activity timestamp (unix seconds).
/// * `approximate_member_count` — approximate member count (untrusted hint).
/// * `room_avatar_hash` — content-addressed avatar/blob reference (BLAKE3
///   hash, never bytes; fetchable via Boru's existing blob-transfer path).
/// * `feature_flags` — compatible feature flag ids (bounded, metadata
///   charset).
/// * `signature` — publisher Ed25519 signature (BORU-DIR-03): proves the
///   payload was produced by the publisher whose key is the control-plane
///   envelope's `sender_node_id`. Optional only for forward/backward wire
///   compatibility; a missing signature means the advertisement is
///   **untrusted**, never canonical.
///
/// Explicitly **not** present (privacy guardrails): member lists, member
/// identities, chat history, chat previews, filenames, invite secrets,
/// moderation state, private keys, attachment content, and any free-form
/// authentication blob (the signature is a fixed 64-byte Ed25519 value,
/// nothing else).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicRoomAdvertisement {
    /// Advertisement payload version ([`ADVERTISEMENT_PAYLOAD_VERSION`]).
    /// Receivers treat an unknown future version as metadata to cache —
    /// never as an authorisation signal.
    pub advert_version: u8,
    /// Stable room identity — the room's gossip [`TopicId`] raw bytes.
    ///
    /// This is the deterministic identity derived from the room's
    /// network/name/protocol-version inputs (see
    /// [`crate::topic_derivation::public_room_topic`]); it is what a joiner
    /// subscribes to, and the directory keys entries by it. It is NOT the
    /// room name and does not leak the room's invite secret or membership.
    pub room_id: TopicId,
    /// Human-readable room name (bounded by
    /// [`AdvertisementBounds::max_room_name_len`]).
    pub room_name: String,
    /// Short description shown in the directory card (bounded by
    /// [`AdvertisementBounds::max_description_len`]).
    pub short_description: String,
    /// Room chat protocol version — used for compatibility checks before
    /// join (PDF Phase 6 Task 6.2). Matches the room's identity-derivation
    /// protocol version.
    pub room_protocol_version: u8,
    /// Creator/owner peer id — raw iroh Ed25519 public key bytes.
    ///
    /// **Descriptive metadata** (BORU-DIR-03): it names the room's
    /// designated room authority, but the directory must not grant
    /// moderation or join privileges based solely on this field (PDF Task
    /// 1.3 step 3). Room ownership is cryptographically proven only when an
    /// advertisement verifies as signed by this key — see
    /// [`is_authoritative_publisher`](Self::is_authoritative_publisher).
    pub owner_peer_id: [u8; 32],
    /// Room visibility. Must be [`RoomVisibility::PublicDiscoverable`] for a
    /// valid advertisement (private/unlisted rooms are never advertised).
    pub visibility: RoomVisibility,
    /// Advertisement TTL in seconds — the expiry/refresh mechanism. The
    /// receiver considers the advertisement stale at
    /// `envelope.timestamp_secs + expires_after_secs` and the publisher must
    /// refresh before expiry (PDF Phase 3 Task 3.2). Clamped to
    /// [`AdvertisementBounds`] min/max.
    pub expires_after_secs: u32,
    /// Optional searchable category tags (bounded count + length). Empty =
    /// no tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional coarse activity timestamp (unix seconds since epoch).
    /// Coarse and untrusted: it is a hint, not verified activity.
    #[serde(default)]
    pub last_active_hint_secs: Option<u32>,
    /// Optional approximate member count. An **untrusted self-reported
    /// hint** — never an authorisation or ranking signal (PDF Phase 7
    /// Task 7.3).
    #[serde(default)]
    pub approximate_member_count: Option<u32>,
    /// Optional content-addressed room avatar/blob reference: a BLAKE3 hash
    /// of the avatar blob, fetched through Boru's existing blob-transfer
    /// path. Never carries avatar bytes, paths, URLs, or tickets.
    #[serde(default)]
    pub room_avatar_hash: Option<[u8; 32]>,
    /// Optional compatible feature flag ids (e.g. `files-v2`, `voice-v1`).
    /// Namespaced, versioned identifiers using the metadata charset
    /// `[A-Za-z0-9._-]`. Empty = no extra feature flags.
    #[serde(default)]
    pub feature_flags: Vec<String>,
    /// Publisher Ed25519 signature (BORU-DIR-03, PDF Task 1.3 step 1).
    ///
    /// Set by [`sign`](Self::sign) with the publisher's node secret key and
    /// verified by [`verify_signed`](Self::verify_signed) against the
    /// claimed publisher (the control-plane envelope's `sender_node_id`).
    /// Covers every other field of this advertisement through the canonical
    /// [`signing_bytes`](Self::signing_bytes) framing, so any tampering
    /// invalidates verification.
    ///
    /// `None` (the wire default for older clients) means the advertisement
    /// is **untrusted** — never canonical metadata.
    ///
    /// Stored as `Vec<u8>` (always exactly [`SIGNATURE_LEN`](crate::protocol_signing::SIGNATURE_LEN)
    /// = 64 bytes when present) because postcard's serde support in this
    /// codebase does not deserialize `[u8; 64]`; verification validates the
    /// length and fails closed on anything else.
    #[serde(default)]
    pub signature: Option<Vec<u8>>,
}

impl PublicRoomAdvertisement {
    /// Build the smallest valid advertisement for a discoverable room:
    /// version 1, empty description, no tags/flags/optionals, and the
    /// default TTL.
    ///
    /// Convenience for tests and for the Phase 3 publisher that starts from
    /// the room's core identity and fills optional metadata in later.
    pub fn minimal(room_id: TopicId, room_name: String, owner_peer_id: [u8; 32]) -> Self {
        Self {
            advert_version: ADVERTISEMENT_PAYLOAD_VERSION,
            room_id,
            room_name,
            short_description: String::new(),
            room_protocol_version: crate::public_room::PROTOCOL_VERSION,
            owner_peer_id,
            visibility: RoomVisibility::PublicDiscoverable,
            expires_after_secs: DEFAULT_ADVERT_TTL_SECS,
            tags: Vec::new(),
            last_active_hint_secs: None,
            approximate_member_count: None,
            room_avatar_hash: None,
            feature_flags: Vec::new(),
            signature: None,
        }
    }

    /// The canonical bytes a publisher signs and a receiver recomputes for
    /// verification (BORU-DIR-03).
    ///
    /// Follows the crate-wide [`crate::protocol_signing`] framing:
    /// `postcard((protocol, version, fields))`. The signed fields are every
    /// security-relevant advertisement field **except** `signature`, plus
    /// the publisher's own public key — embedding the publisher makes the
    /// signature self-describing: it can only ever verify for the key that
    /// produced it, so a cached advertisement cannot be silently
    /// re-attributed to a different node.
    ///
    /// Serialisation is deterministic and infallible for in-memory values.
    pub fn signing_bytes(&self, publisher: &PublicKey) -> Vec<u8> {
        crate::protocol_signing::canonical_signed_bytes(
            ADVERTISEMENT_SIGNING_PROTOCOL,
            self.advert_version as u16,
            &(
                *publisher.as_bytes(),
                self.room_id,
                &self.room_name,
                &self.short_description,
                self.room_protocol_version,
                self.owner_peer_id,
                self.visibility,
                self.expires_after_secs,
                &self.tags,
                self.last_active_hint_secs,
                self.approximate_member_count,
                self.room_avatar_hash,
                &self.feature_flags,
            ),
        )
        .expect("advertisement canonical signing bytes cannot fail")
    }

    /// Sign this advertisement with the **publisher's** node key
    /// (BORU-DIR-03, PDF Task 1.3 step 1).
    ///
    /// The publisher is the node that will send the advertisement — the
    /// control-plane envelope's `sender_node_id`. For a room's canonical
    /// metadata the publisher should be the room owner (`owner_peer_id`);
    /// see [`is_authoritative_publisher`](Self::is_authoritative_publisher).
    ///
    /// After signing, the receiver can attribute the payload to
    /// `publisher.public()` via [`verify_signed`](Self::verify_signed).
    /// This is the publisher's own node key — never an advertisement field,
    /// never a room secret.
    pub fn sign(&mut self, publisher: &SecretKey) {
        let public = publisher.public();
        let bytes = self.signing_bytes(&public);
        self.signature = Some(publisher.sign(&bytes).to_bytes().to_vec());
    }

    /// Verify this advertisement against the **claimed publisher** — the
    /// control-plane envelope's `sender_node_id` (BORU-DIR-03, PDF Task 1.3
    /// step 2).
    ///
    /// Returns [`AdvertisementAuth::Verified`] only when a signature is
    /// present and valid for `claimed_publisher`. A missing signature is
    /// [`AdvertisementAuth::MissingSignature`] (clearly untrusted, never
    /// canonical); a present-but-invalid signature is
    /// [`AdvertisementAuth::InvalidSignature`] (forged/tampered — discard).
    /// Never panics: a malformed signature (wrong length) simply fails
    /// verification.
    pub fn verify_signed(&self, claimed_publisher: &PublicKey) -> AdvertisementAuth {
        let Some(signature) = &self.signature else {
            return AdvertisementAuth::MissingSignature;
        };
        let bytes = self.signing_bytes(claimed_publisher);
        if crate::protocol_signing::verify(claimed_publisher, signature, &bytes) {
            AdvertisementAuth::Verified {
                publisher: *claimed_publisher,
            }
        } else {
            AdvertisementAuth::InvalidSignature
        }
    }

    /// Whether `publisher` is the **designated room authority** for this
    /// room — i.e. the publisher's key equals `owner_peer_id`.
    ///
    /// This is the authority rule that prevents a random peer from silently
    /// overwriting another room's canonical metadata (PDF Task 1.3 step 5,
    /// acceptance criterion):
    ///
    /// * Only an advertisement that verifies as signed by the room authority
    ///   ([`AdvertisementAuth::Verified`] AND
    ///   [`is_authoritative_publisher`](Self::is_authoritative_publisher))
    ///   may establish or update a room's **canonical** directory metadata.
    /// * An advertisement verified as signed by any other member is an
    ///   **independent endorsement** of the room's existence. It may appear
    ///   in the directory as a member-endorsed listing but can never replace
    ///   the authority's canonical metadata.
    ///
    /// Note that this alone proves nothing unless the signature already
    /// verified: `owner_peer_id` is descriptive metadata, and the publisher
    /// must hold the matching private key (proven by the signature) for the
    /// authority claim to hold cryptographically.
    pub fn is_authoritative_publisher(&self, publisher: &PublicKey) -> bool {
        publisher.as_bytes() == &self.owner_peer_id
    }

    /// Validate this advertisement against `bounds`.
    ///
    /// Returns `Ok(())` for a bounded, metadata-only, discoverable
    /// advertisement; `Err(violation)` with the specific bound or guardrail
    /// that was exceeded. Never panics.
    ///
    /// Checks, in order:
    /// 1. Visibility must be [`RoomVisibility::PublicDiscoverable`] (private
    ///    and unlisted rooms are never advertised).
    /// 2. `owner_peer_id` must be a valid iroh Ed25519 public key.
    /// 3. Room name non-empty + bounded, no ASCII control chars.
    /// 4. Short description bounded, no ASCII control chars.
    /// 5. Tags: count + per-tag length + non-empty + no control chars.
    /// 6. Feature flags: count + per-flag length + metadata charset.
    /// 7. `expires_after_secs` within `[min, max]`.
    /// 8. Postcard-encoded payload within `max_encoded_len` bytes.
    pub fn validate(&self, bounds: &AdvertisementBounds) -> Result<(), AdvertisementViolation> {
        // 1. Visibility guardrail — only discoverable rooms are advertised.
        if self.visibility != RoomVisibility::PublicDiscoverable {
            return Err(AdvertisementViolation::NotDiscoverable);
        }

        // 2. Owner identity must be a real iroh key (garbage-proof).
        if iroh_base::PublicKey::from_bytes(&self.owner_peer_id).is_err() {
            return Err(AdvertisementViolation::InvalidOwnerPeerId);
        }

        // 3. Room name: non-empty, bounded, no control chars.
        if self.room_name.is_empty() {
            return Err(AdvertisementViolation::RoomNameEmpty);
        }
        let name_len = self.room_name.chars().count();
        if name_len > bounds.max_room_name_len {
            return Err(AdvertisementViolation::RoomNameTooLong {
                len: name_len,
                max: bounds.max_room_name_len,
            });
        }
        if contains_ascii_control(&self.room_name) {
            return Err(AdvertisementViolation::ControlChar {
                field: fields::ROOM_NAME,
            });
        }

        // 4. Short description: bounded, no control chars (may be empty).
        let desc_len = self.short_description.chars().count();
        if desc_len > bounds.max_description_len {
            return Err(AdvertisementViolation::DescriptionTooLong {
                len: desc_len,
                max: bounds.max_description_len,
            });
        }
        if contains_ascii_control(&self.short_description) {
            return Err(AdvertisementViolation::ControlChar {
                field: fields::DESCRIPTION,
            });
        }

        // 5. Tags.
        if self.tags.len() > bounds.max_tags {
            return Err(AdvertisementViolation::TooManyTags {
                count: self.tags.len(),
                max: bounds.max_tags,
            });
        }
        for (index, tag) in self.tags.iter().enumerate() {
            if tag.is_empty() {
                return Err(AdvertisementViolation::TagEmpty { index });
            }
            let tag_len = tag.chars().count();
            if tag_len > bounds.max_tag_len {
                return Err(AdvertisementViolation::TagTooLong {
                    index,
                    len: tag_len,
                    max: bounds.max_tag_len,
                });
            }
            if contains_ascii_control(tag) {
                return Err(AdvertisementViolation::ControlChar { field: fields::TAG });
            }
        }

        // 6. Feature flags: count + per-flag length + metadata charset.
        if self.feature_flags.len() > bounds.max_feature_flags {
            return Err(AdvertisementViolation::TooManyFeatureFlags {
                count: self.feature_flags.len(),
                max: bounds.max_feature_flags,
            });
        }
        for (index, flag) in self.feature_flags.iter().enumerate() {
            if flag.is_empty() || flag.chars().count() > bounds.max_feature_flag_len {
                return Err(AdvertisementViolation::FeatureFlagTooLong {
                    index,
                    len: flag.chars().count(),
                    max: bounds.max_feature_flag_len,
                });
            }
            if !flag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return Err(AdvertisementViolation::FeatureFlagInvalid { index });
            }
        }

        // 7. TTL bounds (reject absurd values, PDF Phase 7 Task 7.1).
        if self.expires_after_secs < bounds.min_ttl_secs {
            return Err(AdvertisementViolation::TtlTooSmall {
                ttl: self.expires_after_secs,
                min: bounds.min_ttl_secs,
            });
        }
        if self.expires_after_secs > bounds.max_ttl_secs {
            return Err(AdvertisementViolation::TtlTooLarge {
                ttl: self.expires_after_secs,
                max: bounds.max_ttl_secs,
            });
        }

        // 8. Total encoded size bound (compact + bounded).
        let encoded_len = postcard::to_stdvec(self)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        if encoded_len > bounds.max_encoded_len {
            return Err(AdvertisementViolation::EncodedTooLarge {
                len: encoded_len,
                max: bounds.max_encoded_len,
            });
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The withdrawal / tombstone payload (BORU-DIR-09, PDF Task 3.3)
// ---------------------------------------------------------------------------

/// Current withdrawal payload version.
///
/// Independent from [`ADVERTISEMENT_PAYLOAD_VERSION`] (the advertisement
/// metadata model version) and from the control-plane envelope version.
pub const WITHDRAWAL_PAYLOAD_VERSION: u8 = 1;

/// Domain-separation tag for the room-withdrawal publisher signature
/// (BORU-DIR-09). A signature over this tag can never verify as a signature
/// over any other Boru protocol object family (room advertisements, chat
/// messages, mailbox acks, ...).
///
/// The signed-bytes layout follows
/// [`crate::protocol_signing::canonical_signed_bytes`]:
/// `postcard((protocol, version, (publisher, withdrawal_version, room_id,
/// owner_peer_id, timestamp_secs)))`.
pub const WITHDRAWAL_SIGNING_PROTOCOL: &str = "boru/public-room-withdrawal/v1";

/// A signed room withdrawal / tombstone (BORU-DIR-09, PDF Task 3.3).
///
/// Broadcast when a room is deleted, made unlisted, or intentionally
/// removed from discovery. Directory clients remove the matching
/// advertisement immediately **when the withdrawal verifies**; TTL expiry
/// remains the safety net if the withdrawal is missed.
///
/// # Identity rules (same as advertisements, BORU-DIR-03)
///
/// * `signature` is an Ed25519 signature by the **publisher** — the node
///   that sends the PUBLIC_ROOM_WITHDRAWAL control message, whose identity
///   is the envelope's `sender_node_id` (see [`sign`](Self::sign) /
///   [`verify_signed`](Self::verify_signed)).
/// * A withdrawal is applied only when it verifies as signed by the room's
///   **designated authority** — the key named in `owner_peer_id` (see
///   [`is_authoritative_publisher`](Self::is_authoritative_publisher)),
///   mirroring the canonical-metadata rule for advertisements. A
///   withdrawal that fails verification, or verifies for a non-authority
///   publisher, is discarded and removes nothing.
/// * A withdrawal carries only `room_id` (the advertisement being
///   withdrawn) plus the authority identity — no member lists, history,
///   previews, invite secrets, or attachment content.
///
/// # Wire compatibility
///
/// `signature` is the last field with `#[serde(default)]`; older clients
/// decode the known prefix and ignore it, and a missing signature means the
/// withdrawal is untrusted (never applied).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicRoomWithdrawal {
    /// Withdrawal payload version ([`WITHDRAWAL_PAYLOAD_VERSION`]).
    pub withdrawal_version: u8,
    /// The room being withdrawn — its stable gossip [`TopicId`] raw bytes,
    /// matching the `room_id` of the advertisement(s) to remove.
    pub room_id: TopicId,
    /// The room's designated authority — raw iroh Ed25519 public key bytes.
    /// Only a withdrawal signed by this key may be applied (same
    /// authoritative identity rule as canonical advertisements).
    pub owner_peer_id: [u8; 32],
    /// Unix epoch seconds when the withdrawal was created (the envelope
    /// also carries a timestamp; this one is signed so the payload is
    /// self-contained).
    pub timestamp_secs: u64,
    /// Publisher Ed25519 signature (see [`sign`](Self::sign) /
    /// [`verify_signed`](Self::verify_signed)). `None` (or any value that
    /// does not verify for the claimed publisher) means the withdrawal is
    /// **untrusted** and must never remove anything.
    ///
    /// Stored as `Vec<u8>` (always exactly
    /// [`SIGNATURE_LEN`](crate::protocol_signing::SIGNATURE_LEN) = 64 bytes
    /// when present) because postcard's serde support in this codebase does
    /// not deserialize `[u8; 64]`; verification validates the length and
    /// fails closed on anything else.
    #[serde(default)]
    pub signature: Option<Vec<u8>>,
}

impl PublicRoomWithdrawal {
    /// Build a minimal unsigned withdrawal for a room by its authority.
    ///
    /// Convenience for publishers that fill the timestamp and call
    /// [`sign`](Self::sign) before broadcast.
    pub fn minimal(room_id: TopicId, owner_peer_id: [u8; 32]) -> Self {
        Self {
            withdrawal_version: WITHDRAWAL_PAYLOAD_VERSION,
            room_id,
            owner_peer_id,
            timestamp_secs: 0,
            signature: None,
        }
    }

    /// The canonical bytes a publisher signs and a receiver recomputes for
    /// verification (BORU-DIR-09).
    ///
    /// Follows the crate-wide [`crate::protocol_signing`] framing:
    /// `postcard((protocol, version, fields))`. Embedding the publisher's
    /// public key makes the signature self-describing: it can only ever
    /// verify for the key that produced it, so a cached withdrawal cannot
    /// be silently re-attributed to a different node.
    ///
    /// Serialisation is deterministic and infallible for in-memory values.
    pub fn signing_bytes(&self, publisher: &PublicKey) -> Vec<u8> {
        crate::protocol_signing::canonical_signed_bytes(
            WITHDRAWAL_SIGNING_PROTOCOL,
            self.withdrawal_version as u16,
            &(
                *publisher.as_bytes(),
                self.withdrawal_version,
                self.room_id,
                self.owner_peer_id,
                self.timestamp_secs,
            ),
        )
        .expect("withdrawal canonical signing bytes cannot fail")
    }

    /// Sign this withdrawal with the **publisher's** node key (BORU-DIR-09).
    ///
    /// The publisher is the node that will send the withdrawal — the
    /// control-plane envelope's `sender_node_id`. For a withdrawal to be
    /// applied it must also be the room's designated authority
    /// (`owner_peer_id`); see [`is_authoritative_publisher`](Self::is_authoritative_publisher).
    pub fn sign(&mut self, publisher: &SecretKey) {
        let public = publisher.public();
        let bytes = self.signing_bytes(&public);
        self.signature = Some(publisher.sign(&bytes).to_bytes().to_vec());
    }

    /// Verify this withdrawal against the **claimed publisher** — the
    /// control-plane envelope's `sender_node_id` (BORU-DIR-09, PDF Task
    /// 3.3 step 2).
    ///
    /// Returns [`AdvertisementAuth::Verified`] only when a signature is
    /// present and valid for `claimed_publisher`. A missing signature is
    /// [`AdvertisementAuth::MissingSignature`] (untrusted — never applied);
    /// a present-but-invalid signature is
    /// [`AdvertisementAuth::InvalidSignature`] (forged/tampered — discard).
    /// Never panics: a malformed signature (wrong length) simply fails
    /// verification.
    pub fn verify_signed(&self, claimed_publisher: &PublicKey) -> AdvertisementAuth {
        let Some(signature) = &self.signature else {
            return AdvertisementAuth::MissingSignature;
        };
        let bytes = self.signing_bytes(claimed_publisher);
        if crate::protocol_signing::verify(claimed_publisher, signature, &bytes) {
            AdvertisementAuth::Verified {
                publisher: *claimed_publisher,
            }
        } else {
            AdvertisementAuth::InvalidSignature
        }
    }

    /// Whether `publisher` is the **designated room authority** for the
    /// withdrawn room — i.e. the publisher's key equals `owner_peer_id`.
    ///
    /// This is the same authority rule used for advertisements (BORU-DIR-03):
    /// a withdrawal is applied only when it verifies as signed by the room
    /// authority ([`AdvertisementAuth::Verified`] AND
    /// [`is_authoritative_publisher`](Self::is_authoritative_publisher)).
    /// A verified-but-non-authority publisher's withdrawal is discarded —
    /// it cannot remove the room's canonical advertisement.
    pub fn is_authoritative_publisher(&self, publisher: &PublicKey) -> bool {
        publisher.as_bytes() == &self.owner_peer_id
    }

    /// Validate this withdrawal against `bounds`.
    ///
    /// Returns `Ok(())` for a bounded, metadata-only withdrawal;
    /// `Err(violation)` with the specific bound that was exceeded. Never
    /// panics. The withdrawal is deliberately tiny (fixed-size identity
    /// fields + optional 64-byte signature), so the only checks are that
    /// the authority is a real iroh key and the encoded payload stays
    /// within the same bound as advertisements.
    pub fn validate(&self, bounds: &AdvertisementBounds) -> Result<(), AdvertisementViolation> {
        // Authority identity must be a real iroh key (garbage-proof).
        if iroh_base::PublicKey::from_bytes(&self.owner_peer_id).is_err() {
            return Err(AdvertisementViolation::InvalidOwnerPeerId);
        }
        // Total encoded size bound (compact + bounded).
        let encoded_len = postcard::to_stdvec(self)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        if encoded_len > bounds.max_encoded_len {
            return Err(AdvertisementViolation::EncodedTooLarge {
                len: encoded_len,
                max: bounds.max_encoded_len,
            });
        }
        Ok(())
    }
}

/// Normalized, validated room metadata entered by a creator in the UI
/// (BORU-DIR-05, PDF Task 2.2).
///
/// Produced by [`normalize_room_metadata`] from raw form input. Every value
/// is bounded by the same [`AdvertisementBounds`] that [`PublicRoomAdvertisement::validate`]
/// enforces on the wire, so a normalized value can always be placed into an
/// advertisement without a second rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRoomMetadata {
    /// Sanitized, trimmed room name (non-empty, ≤ `max_room_name_len`).
    pub room_name: String,
    /// Sanitized short description (may be empty, ≤ `max_description_len`).
    pub short_description: String,
    /// Sanitized, de-duplicated-in-input tags (≤ `max_tags`, each ≤ `max_tag_len`).
    pub tags: Vec<String>,
}

/// Normalize and validate creator-supplied room metadata before it is stored
/// or advertised (PDF Task 2.2 step 5: "Validate and normalize room names /
/// tags before creating the advertisement; reject invalid/oversized metadata
/// before broadcast").
///
/// # Normalization
///
/// * Name and tags are sanitized single-line (NFC-normalized, invisible
///   format characters stripped, control characters replaced with spaces,
///   whitespace collapsed) and trimmed.
/// * Description is sanitized the same way but may contain spaces; it is
///   trimmed.
/// * Tags are split on `,`; empty segments are dropped (a trailing or
///   doubled comma is treated as a formatting artifact, not a tag).
///
/// # Validation
///
/// The normalized values must satisfy the same bounds as the wire
/// [`PublicRoomAdvertisement::validate`]: non-empty bounded room name,
/// bounded description, bounded tag count + per-tag length. Returns the
/// specific [`AdvertisementViolation`] (with accurate input lengths) when a
/// bound is exceeded.
pub fn normalize_room_metadata(
    raw_name: &str,
    raw_description: &str,
    raw_tags: &str,
    bounds: &AdvertisementBounds,
) -> Result<NormalizedRoomMetadata, AdvertisementViolation> {
    // 1. Room name: trimmed length is checked on the raw input so oversized
    //    input is rejected with its true length; then sanitize for control /
    //    format characters and collapse whitespace.
    let name_trimmed = raw_name.trim();
    let name_len = name_trimmed.chars().count();
    if name_len > bounds.max_room_name_len {
        return Err(AdvertisementViolation::RoomNameTooLong {
            len: name_len,
            max: bounds.max_room_name_len,
        });
    }
    let name = crate::abuse_controls::sanitize_single_line_with_max(
        name_trimmed,
        bounds.max_room_name_len,
    );
    let name = name.trim();
    if name.is_empty() {
        return Err(AdvertisementViolation::RoomNameEmpty);
    }

    // 2. Short description: bounded on the raw trimmed input, then sanitized.
    let desc_trimmed = raw_description.trim();
    let desc_len = desc_trimmed.chars().count();
    if desc_len > bounds.max_description_len {
        return Err(AdvertisementViolation::DescriptionTooLong {
            len: desc_len,
            max: bounds.max_description_len,
        });
    }
    let short_description =
        crate::abuse_controls::sanitize_display_text(desc_trimmed, bounds.max_description_len);
    let short_description = short_description.trim().to_string();

    // 3. Tags: split, trim, sanitize, drop empties, then enforce bounds.
    let mut tags = Vec::new();
    for raw_tag in raw_tags.split(',') {
        let tag_trimmed = raw_tag.trim();
        if tag_trimmed.is_empty() {
            continue;
        }
        let tag_len = tag_trimmed.chars().count();
        if tag_len > bounds.max_tag_len {
            return Err(AdvertisementViolation::TagTooLong {
                index: tags.len(),
                len: tag_len,
                max: bounds.max_tag_len,
            });
        }
        let tag =
            crate::abuse_controls::sanitize_single_line_with_max(tag_trimmed, bounds.max_tag_len);
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        tags.push(tag.to_string());
    }
    if tags.len() > bounds.max_tags {
        return Err(AdvertisementViolation::TooManyTags {
            count: tags.len(),
            max: bounds.max_tags,
        });
    }

    Ok(NormalizedRoomMetadata {
        room_name: name.to_string(),
        short_description,
        tags,
    })
}

impl std::fmt::Display for AdvertisementViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdvertisementViolation::RoomNameEmpty => write!(f, "Room name is required"),
            AdvertisementViolation::RoomNameTooLong { len, max } => {
                write!(f, "Room name is too long ({len} characters, max {max})")
            }
            AdvertisementViolation::DescriptionTooLong { len, max } => {
                write!(f, "Description is too long ({len} characters, max {max})")
            }
            AdvertisementViolation::TooManyTags { count, max } => {
                write!(f, "Too many tags ({count}, max {max})")
            }
            AdvertisementViolation::TagTooLong { index, len, max } => write!(
                f,
                "Tag {} is too long ({len} characters, max {max})",
                index + 1
            ),
            AdvertisementViolation::TagEmpty { index } => {
                write!(f, "Tag {} is empty", index + 1)
            }
            AdvertisementViolation::TooManyFeatureFlags { count, max } => {
                write!(f, "Too many feature flags ({count}, max {max})")
            }
            AdvertisementViolation::FeatureFlagTooLong { index, len, max } => write!(
                f,
                "Feature flag {} is too long ({len} characters, max {max})",
                index + 1
            ),
            AdvertisementViolation::FeatureFlagInvalid { index } => {
                write!(f, "Feature flag {} contains invalid characters", index + 1)
            }
            AdvertisementViolation::TtlTooSmall { ttl, min } => write!(
                f,
                "Advertisement lifetime {ttl}s is below the minimum {min}s"
            ),
            AdvertisementViolation::TtlTooLarge { ttl, max } => write!(
                f,
                "Advertisement lifetime {ttl}s exceeds the maximum {max}s"
            ),
            AdvertisementViolation::NotDiscoverable => {
                write!(f, "Only discoverable rooms can be advertised")
            }
            AdvertisementViolation::InvalidOwnerPeerId => {
                write!(f, "Invalid room owner identity")
            }
            AdvertisementViolation::ControlChar { .. } => {
                write!(f, "Metadata contains invalid control characters")
            }
            AdvertisementViolation::EncodedTooLarge { len, max } => {
                write!(f, "Advertisement is too large ({len} bytes, max {max})")
            }
        }
    }
}

/// Whether `text` contains an ASCII control character (0x00–0x1F or 0x7F).
///
/// Used to reject free-form metadata that could inject log lines or corrupt
/// terminal/UI rendering.
fn contains_ascii_control(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_control())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> [u8; 32] {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed)
            .public()
            .as_bytes()
            .to_owned()
    }

    fn topic(byte: u8) -> TopicId {
        TopicId::from_bytes([byte; 32])
    }

    fn full_advert() -> PublicRoomAdvertisement {
        PublicRoomAdvertisement {
            advert_version: ADVERTISEMENT_PAYLOAD_VERSION,
            room_id: topic(0x11),
            room_name: "Rust Community".into(),
            short_description: "A friendly place to discuss Rust.".into(),
            room_protocol_version: crate::public_room::PROTOCOL_VERSION,
            owner_peer_id: key(0x22),
            visibility: RoomVisibility::PublicDiscoverable,
            expires_after_secs: 3600,
            tags: vec!["rust".into(), "programming".into()],
            last_active_hint_secs: Some(1_700_000_000),
            approximate_member_count: Some(42),
            room_avatar_hash: Some([0xAB; 32]),
            feature_flags: vec!["files-v2".into(), "voice-v1".into()],
            signature: None,
        }
    }

    /// A secret key deterministically derived from `byte` (matches `key`).
    fn secret_key(byte: u8) -> SecretKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        SecretKey::from_bytes(&seed)
    }

    // ── Round-trip ─────────────────────────────────────────────────────

    #[test]
    fn roundtrip_full_advertisement() {
        let advert = full_advert();
        let bytes = postcard::to_stdvec(&advert).unwrap();
        let decoded: PublicRoomAdvertisement = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, advert);
    }

    #[test]
    fn roundtrip_minimal_advertisement() {
        let advert = PublicRoomAdvertisement::minimal(topic(0x33), "Lobby".into(), key(0x44));
        let bounds = AdvertisementBounds::default();
        assert!(advert.validate(&bounds).is_ok());
        let bytes = postcard::to_stdvec(&advert).unwrap();
        let decoded: PublicRoomAdvertisement = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, advert);
        assert!(decoded.tags.is_empty());
        assert!(decoded.feature_flags.is_empty());
        assert_eq!(decoded.last_active_hint_secs, None);
        assert_eq!(decoded.approximate_member_count, None);
        assert_eq!(decoded.room_avatar_hash, None);
    }

    // ── Optional-field presence / absence ──────────────────────────────

    #[test]
    fn optional_fields_presence_and_absence() {
        let bounds = AdvertisementBounds::default();

        // Absent: all optionals None/empty.
        let absent = PublicRoomAdvertisement::minimal(topic(0x55), "No Optional".into(), key(0x66));
        assert!(absent.validate(&bounds).is_ok());

        // Present: every optional field set.
        let present = full_advert();
        assert!(present.validate(&bounds).is_ok());

        // Presence survives a round trip.
        let bytes = postcard::to_stdvec(&present).unwrap();
        let decoded: PublicRoomAdvertisement = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(
            decoded.tags,
            vec!["rust".to_string(), "programming".to_string()]
        );
        assert_eq!(decoded.last_active_hint_secs, Some(1_700_000_000));
        assert_eq!(decoded.approximate_member_count, Some(42));
        assert_eq!(decoded.room_avatar_hash, Some([0xAB; 32]));
        assert_eq!(
            decoded.feature_flags,
            vec!["files-v2".to_string(), "voice-v1".to_string()]
        );
    }

    // ── Size-limit enforcement ─────────────────────────────────────────

    #[test]
    fn rejects_empty_room_name() {
        let mut advert = full_advert();
        advert.room_name.clear();
        let err = advert
            .validate(&AdvertisementBounds::default())
            .unwrap_err();
        assert!(matches!(err, AdvertisementViolation::RoomNameEmpty));
    }

    #[test]
    fn rejects_room_name_too_long() {
        let mut advert = full_advert();
        advert.room_name = "x".repeat(DEFAULT_MAX_ROOM_NAME_LEN + 1);
        let err = advert
            .validate(&AdvertisementBounds::default())
            .unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::RoomNameTooLong { len, max } if len == DEFAULT_MAX_ROOM_NAME_LEN + 1 && max == DEFAULT_MAX_ROOM_NAME_LEN
        ));
    }

    #[test]
    fn rejects_description_too_long() {
        let mut advert = full_advert();
        advert.short_description = "y".repeat(DEFAULT_MAX_DESCRIPTION_LEN + 1);
        let err = advert
            .validate(&AdvertisementBounds::default())
            .unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::DescriptionTooLong { len, max } if len == DEFAULT_MAX_DESCRIPTION_LEN + 1 && max == DEFAULT_MAX_DESCRIPTION_LEN
        ));
    }

    #[test]
    fn rejects_too_many_tags() {
        let mut advert = full_advert();
        advert.tags = (0..=DEFAULT_MAX_TAGS).map(|i| format!("tag{i}")).collect();
        let err = advert
            .validate(&AdvertisementBounds::default())
            .unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::TooManyTags { count, max } if count == DEFAULT_MAX_TAGS + 1 && max == DEFAULT_MAX_TAGS
        ));
    }

    #[test]
    fn rejects_tag_too_long() {
        let mut advert = full_advert();
        advert.tags = vec!["ok".into(), "z".repeat(DEFAULT_MAX_TAG_LEN + 1)];
        let err = advert
            .validate(&AdvertisementBounds::default())
            .unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::TagTooLong { index: 1, len, max } if len == DEFAULT_MAX_TAG_LEN + 1 && max == DEFAULT_MAX_TAG_LEN
        ));
    }

    #[test]
    fn rejects_empty_tag() {
        let mut advert = full_advert();
        advert.tags = vec!["ok".into(), String::new()];
        let err = advert
            .validate(&AdvertisementBounds::default())
            .unwrap_err();
        assert!(matches!(err, AdvertisementViolation::TagEmpty { index: 1 }));
    }

    #[test]
    fn rejects_control_chars_in_free_form_fields() {
        let mut advert = full_advert();
        advert.room_name = "bad\u{0000}name".into();
        assert!(matches!(
            advert
                .validate(&AdvertisementBounds::default())
                .unwrap_err(),
            AdvertisementViolation::ControlChar {
                field: fields::ROOM_NAME
            }
        ));

        let mut advert = full_advert();
        advert.short_description = "line\nbreak".into();
        assert!(matches!(
            advert
                .validate(&AdvertisementBounds::default())
                .unwrap_err(),
            AdvertisementViolation::ControlChar {
                field: fields::DESCRIPTION
            }
        ));

        let mut advert = full_advert();
        advert.tags = vec!["ok".into(), "tab\u{0009}tag".into()];
        assert!(matches!(
            advert
                .validate(&AdvertisementBounds::default())
                .unwrap_err(),
            AdvertisementViolation::ControlChar { field: fields::TAG }
        ));
    }

    #[test]
    fn rejects_too_many_feature_flags() {
        let mut advert = full_advert();
        advert.feature_flags = (0..=DEFAULT_MAX_FEATURE_FLAGS)
            .map(|i| format!("flag{i}"))
            .collect();
        let err = advert
            .validate(&AdvertisementBounds::default())
            .unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::TooManyFeatureFlags { count, max } if count == DEFAULT_MAX_FEATURE_FLAGS + 1 && max == DEFAULT_MAX_FEATURE_FLAGS
        ));
    }

    #[test]
    fn rejects_feature_flag_too_long_or_invalid() {
        let mut advert = full_advert();
        advert.feature_flags = vec!["x".repeat(DEFAULT_MAX_FEATURE_FLAG_LEN + 1)];
        assert!(matches!(
            advert
                .validate(&AdvertisementBounds::default())
                .unwrap_err(),
            AdvertisementViolation::FeatureFlagTooLong { index: 0, .. }
        ));

        let mut advert = full_advert();
        advert.feature_flags = vec!["bad flag!".into()]; // space + bang invalid
        assert!(matches!(
            advert
                .validate(&AdvertisementBounds::default())
                .unwrap_err(),
            AdvertisementViolation::FeatureFlagInvalid { index: 0 }
        ));
    }

    #[test]
    fn rejects_ttl_out_of_bounds() {
        let bounds = AdvertisementBounds::default();

        let mut advert = full_advert();
        advert.expires_after_secs = DEFAULT_MIN_ADVERT_TTL_SECS - 1;
        assert!(matches!(
            advert.validate(&bounds).unwrap_err(),
            AdvertisementViolation::TtlTooSmall { ttl, min } if ttl == DEFAULT_MIN_ADVERT_TTL_SECS - 1 && min == DEFAULT_MIN_ADVERT_TTL_SECS
        ));

        let mut advert = full_advert();
        advert.expires_after_secs = DEFAULT_MAX_ADVERT_TTL_SECS + 1;
        assert!(matches!(
            advert.validate(&bounds).unwrap_err(),
            AdvertisementViolation::TtlTooLarge { ttl, max } if ttl == DEFAULT_MAX_ADVERT_TTL_SECS + 1 && max == DEFAULT_MAX_ADVERT_TTL_SECS
        ));
    }

    #[test]
    fn rejects_non_discoverable_visibility() {
        for visibility in [RoomVisibility::Private, RoomVisibility::PublicUnlisted] {
            let mut advert = full_advert();
            advert.visibility = visibility;
            assert!(matches!(
                advert
                    .validate(&AdvertisementBounds::default())
                    .unwrap_err(),
                AdvertisementViolation::NotDiscoverable
            ));
        }
    }

    #[test]
    fn rejects_invalid_owner_peer_id() {
        let mut advert = full_advert();
        advert.owner_peer_id = [0x02; 32]; // not a valid ed25519 point
        assert!(matches!(
            advert
                .validate(&AdvertisementBounds::default())
                .unwrap_err(),
            AdvertisementViolation::InvalidOwnerPeerId
        ));
    }

    #[test]
    fn rejects_encoded_payload_too_large() {
        let bounds = AdvertisementBounds {
            max_encoded_len: 64,
            ..Default::default()
        };
        let advert = full_advert();
        let err = advert.validate(&bounds).unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::EncodedTooLarge { .. }
        ));
    }

    // ── No private data fields present ─────────────────────────────────

    /// The advertisement carries exactly the documented metadata fields and
    /// no field that could hold private room information. The struct has no
    /// member list, history, preview, invite-secret, moderation, private-key,
    /// or attachment field — this test pins the Debug field names so a
    /// future addition of a private-data field is caught in review.
    #[test]
    fn no_private_data_fields_present() {
        let debug = format!("{:?}", full_advert());
        // Every documented field name must appear (we still have them).
        for field in [
            "advert_version",
            "room_id",
            "room_name",
            "short_description",
            "room_protocol_version",
            "owner_peer_id",
            "visibility",
            "expires_after_secs",
            "tags",
            "last_active_hint_secs",
            "approximate_member_count",
            "room_avatar_hash",
            "feature_flags",
            "signature",
        ] {
            assert!(debug.contains(field), "missing field {field} in {debug}");
        }
        // No private-room data field names may appear.
        for forbidden in [
            "members",
            "member_list",
            "member_ids",
            "history",
            "preview",
            "invite_secret",
            "invite",
            "moderation",
            "moderator",
            "private_key",
            "secret_key",
            "attachment",
            "filename",
            "password",
            "token",
        ] {
            assert!(
                !debug.contains(forbidden),
                "private-data field leaked into advertisement: {forbidden} in {debug}"
            );
        }
    }

    /// A minimal advertisement is compact: well under the encoded bound.
    #[test]
    fn advertisement_is_compact() {
        let advert = PublicRoomAdvertisement::minimal(topic(0x77), "Lobby".into(), key(0x88));
        let encoded = postcard::to_stdvec(&advert).unwrap();
        assert!(
            encoded.len() < 200,
            "minimal advertisement should be tiny, got {} bytes",
            encoded.len()
        );
    }

    // ── BORU-DIR-03: publisher authentication ─────────────────────────

    /// A publisher-signed advertisement verifies as
    /// [`AdvertisementAuth::Verified`] for that publisher.
    #[test]
    fn signed_advertisement_verifies_for_publisher() {
        let publisher = secret_key(0x22); // matches full_advert()'s owner
        let mut advert = full_advert();
        assert_eq!(
            advert.verify_signed(&publisher.public()),
            AdvertisementAuth::MissingSignature
        );
        advert.sign(&publisher);
        assert!(advert.signature.is_some());
        assert_eq!(
            advert.verify_signed(&publisher.public()),
            AdvertisementAuth::Verified {
                publisher: publisher.public()
            }
        );
        assert!(advert.verify_signed(&publisher.public()).is_verified());
    }

    /// Tampering with any signed field invalidates the signature:
    /// [`AdvertisementAuth::InvalidSignature`], never a panic.
    #[test]
    fn tampered_advertisement_rejected() {
        let publisher = secret_key(0x22);
        let mut advert = full_advert();
        advert.sign(&publisher);

        // Tamper with each kind of field: scalar, string, option, vec.
        let mut tampered = advert.clone();
        tampered.room_name = "Evil Name".into();
        assert_eq!(
            tampered.verify_signed(&publisher.public()),
            AdvertisementAuth::InvalidSignature
        );

        let mut tampered = advert.clone();
        tampered.expires_after_secs += 1;
        assert_eq!(
            tampered.verify_signed(&publisher.public()),
            AdvertisementAuth::InvalidSignature
        );

        let mut tampered = advert.clone();
        tampered.approximate_member_count = Some(9_999_999);
        assert_eq!(
            tampered.verify_signed(&publisher.public()),
            AdvertisementAuth::InvalidSignature
        );

        let mut tampered = advert.clone();
        tampered.tags.push("spam".into());
        assert_eq!(
            tampered.verify_signed(&publisher.public()),
            AdvertisementAuth::InvalidSignature
        );

        // The original still verifies (signature covered only the original).
        assert_eq!(
            advert.verify_signed(&publisher.public()),
            AdvertisementAuth::Verified {
                publisher: publisher.public()
            }
        );
    }

    /// A signature by one key never verifies for a different claimed
    /// publisher — the wrong-publisher forgery is rejected.
    #[test]
    fn wrong_publisher_signature_rejected() {
        let publisher = secret_key(0x22);
        let attacker = secret_key(0x99);
        let mut advert = full_advert();
        advert.sign(&publisher);
        assert_eq!(
            advert.verify_signed(&attacker.public()),
            AdvertisementAuth::InvalidSignature,
            "signature by the real publisher must not verify for the attacker"
        );
        // The attacker cannot produce a valid signature for their own claim
        // either, because the embedded publisher in the signed bytes would
        // differ and the canonical bytes would not match.
        let mut forged = full_advert();
        forged.sign(&attacker);
        assert_eq!(
            forged.verify_signed(&publisher.public()),
            AdvertisementAuth::InvalidSignature
        );
    }

    /// An unsigned advertisement is clearly untrusted
    /// ([`AdvertisementAuth::MissingSignature`]) — never canonical.
    #[test]
    fn unsigned_advertisement_is_untrusted() {
        let publisher = secret_key(0x22);
        let advert = full_advert(); // signature: None
        assert_eq!(
            advert.verify_signed(&publisher.public()),
            AdvertisementAuth::MissingSignature
        );
        assert!(!advert.verify_signed(&publisher.public()).is_verified());
    }

    /// A corrupted signature byte (present but wrong) fails verification
    /// without panicking — malformed signatures are not a crash path.
    #[test]
    fn corrupted_signature_fails_closed() {
        let publisher = secret_key(0x22);
        let mut advert = full_advert();
        advert.sign(&publisher);
        let mut bytes = advert.signature.unwrap();
        bytes[0] ^= 0xFF;
        advert.signature = Some(bytes);
        assert_eq!(
            advert.verify_signed(&publisher.public()),
            AdvertisementAuth::InvalidSignature
        );
    }

    /// The signature survives a postcard round trip (wire transport).
    #[test]
    fn signature_survives_postcard_roundtrip() {
        let publisher = secret_key(0x22);
        let mut advert = full_advert();
        advert.sign(&publisher);
        let bytes = postcard::to_stdvec(&advert).unwrap();
        let decoded: PublicRoomAdvertisement = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.signature, advert.signature);
        assert_eq!(
            decoded.verify_signed(&publisher.public()),
            AdvertisementAuth::Verified {
                publisher: publisher.public()
            }
        );
    }

    /// The room authority is the publisher whose key equals `owner_peer_id`.
    /// Only an owner-signed advertisement is canonical-eligible; a verified
    /// non-owner advertisement is an independent endorsement that can never
    /// replace canonical metadata (PDF Task 1.3 step 5).
    #[test]
    fn owner_is_designated_room_authority() {
        let owner = secret_key(0x22); // full_advert() owner_peer_id = key(0x22)
        let member = secret_key(0x77);

        // Owner-signed: authoritative + verified.
        let mut owner_advert = full_advert();
        owner_advert.sign(&owner);
        assert!(owner_advert.is_authoritative_publisher(&owner.public()));
        assert!(owner_advert.verify_signed(&owner.public()).is_verified());

        // Member-signed: verified endorsement, but NOT authoritative.
        let mut member_advert = full_advert();
        member_advert.sign(&member);
        assert!(member_advert.verify_signed(&member.public()).is_verified());
        assert!(
            !member_advert.is_authoritative_publisher(&member.public()),
            "a verified non-owner publisher is not the room authority"
        );

        // An unsigned advertisement claiming the owner id is NOT proof of
        // ownership: ownership requires the owner's signature.
        let unsigned = full_advert();
        assert!(unsigned.is_authoritative_publisher(&owner.public()));
        assert_eq!(
            unsigned.verify_signed(&owner.public()),
            AdvertisementAuth::MissingSignature,
            "owner_peer_id alone (no signature) must not prove ownership"
        );
    }

    /// A spoofed advertisement cannot be treated as canonical metadata:
    /// either the signature does not verify (invalid) or the verified
    /// publisher is not the room authority (endorsement, not canonical).
    /// This is the deterministic conflict rule the directory store applies
    /// per room (PDF Phase 4 Task 4.2): canonical entries are only ever
    /// replaced by verified owner-signed advertisements.
    #[test]
    fn spoofed_advertisement_cannot_overwrite_canonical() {
        let owner = secret_key(0x22);
        let attacker = secret_key(0x99);

        // Canonical advertisement: owner-signed, verified, authoritative.
        let mut canonical = full_advert();
        canonical.sign(&owner);
        assert!(canonical.verify_signed(&owner.public()).is_verified());
        assert!(canonical.is_authoritative_publisher(&owner.public()));

        // Attack 1: attacker reuses the owner's identity in owner_peer_id
        // but signs with their own key. Verification against the attacker
        // passes, but the attacker is NOT the room authority, so their
        // advertisement is at most an endorsement — it cannot replace the
        // canonical metadata.
        let mut spoofed = full_advert();
        spoofed.room_name = "Spoofed Name".into();
        spoofed.sign(&attacker);
        assert!(spoofed.verify_signed(&attacker.public()).is_verified());
        assert!(
            !spoofed.is_authoritative_publisher(&attacker.public()),
            "spoofed publisher must not be the room authority"
        );

        // Attack 2: attacker forges a signature claimed to be the owner's.
        // The signature does not verify against the owner key.
        let mut forged = full_advert();
        forged.room_name = "Forged Name".into();
        forged.sign(&attacker);
        assert_eq!(
            forged.verify_signed(&owner.public()),
            AdvertisementAuth::InvalidSignature,
            "a signature by the attacker must not verify as the owner's"
        );

        // Attack 3: attacker copies the owner's valid signature onto a
        // tampered payload. The canonical bytes differ, so verification
        // fails.
        let mut replayed = full_advert();
        replayed.sign(&owner);
        let stolen_sig = replayed.signature;
        let mut tampered = full_advert();
        tampered.room_name = "Stolen Signature".into();
        tampered.signature = stolen_sig;
        assert_eq!(
            tampered.verify_signed(&owner.public()),
            AdvertisementAuth::InvalidSignature,
            "a signature over a different payload must not verify"
        );

        // The deterministic rule: only a Verified + authoritative
        // advertisement may replace the canonical one. All three attacks
        // fail at least one side of that rule.
        fn may_replace_canonical(
            incoming: &PublicRoomAdvertisement,
            claimed_publisher: &PublicKey,
        ) -> bool {
            incoming.verify_signed(claimed_publisher).is_verified()
                && incoming.is_authoritative_publisher(claimed_publisher)
        }
        assert!(may_replace_canonical(&canonical, &owner.public()));
        assert!(!may_replace_canonical(&spoofed, &attacker.public()));
        assert!(!may_replace_canonical(&forged, &owner.public()));
        assert!(!may_replace_canonical(&tampered, &owner.public()));
    }

    // ── BORU-DIR-05: creator metadata normalization ──────────────────

    /// Basic valid input: trims whitespace, parses comma-separated tags.
    #[test]
    fn normalize_basic_valid_input() {
        let bounds = AdvertisementBounds::default();
        let out = normalize_room_metadata(
            "  Rust Community  ",
            "  A friendly place to discuss Rust.  ",
            "rust, programming, community",
            &bounds,
        )
        .unwrap();
        assert_eq!(out.room_name, "Rust Community");
        assert_eq!(out.short_description, "A friendly place to discuss Rust.");
        assert_eq!(
            out.tags,
            vec![
                "rust".to_string(),
                "programming".to_string(),
                "community".to_string()
            ]
        );
    }

    /// Optional fields can be empty: no description, no tags.
    #[test]
    fn normalize_optional_fields_empty() {
        let bounds = AdvertisementBounds::default();
        let out = normalize_room_metadata("Lobby", "", "", &bounds).unwrap();
        assert_eq!(out.room_name, "Lobby");
        assert_eq!(out.short_description, "");
        assert!(out.tags.is_empty());
    }

    /// An empty room name is rejected before broadcast.
    #[test]
    fn normalize_rejects_empty_name() {
        let bounds = AdvertisementBounds::default();
        let err = normalize_room_metadata("   ", "desc", "", &bounds).unwrap_err();
        assert!(matches!(err, AdvertisementViolation::RoomNameEmpty));
    }

    /// An oversized room name is rejected with its true length.
    #[test]
    fn normalize_rejects_oversized_name() {
        let bounds = AdvertisementBounds::default();
        let long = "x".repeat(DEFAULT_MAX_ROOM_NAME_LEN + 1);
        let err = normalize_room_metadata(&long, "", "", &bounds).unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::RoomNameTooLong { len, max }
                if len == DEFAULT_MAX_ROOM_NAME_LEN + 1 && max == DEFAULT_MAX_ROOM_NAME_LEN
        ));
    }

    /// An oversized description is rejected before broadcast.
    #[test]
    fn normalize_rejects_oversized_description() {
        let bounds = AdvertisementBounds::default();
        let long = "y".repeat(DEFAULT_MAX_DESCRIPTION_LEN + 1);
        let err = normalize_room_metadata("Lobby", &long, "", &bounds).unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::DescriptionTooLong { len, max }
                if len == DEFAULT_MAX_DESCRIPTION_LEN + 1 && max == DEFAULT_MAX_DESCRIPTION_LEN
        ));
    }

    /// More tags than the protocol allows are rejected.
    #[test]
    fn normalize_rejects_too_many_tags() {
        let bounds = AdvertisementBounds::default();
        let tags = (0..=DEFAULT_MAX_TAGS)
            .map(|i| format!("tag{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let err = normalize_room_metadata("Lobby", "", &tags, &bounds).unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::TooManyTags { count, max }
                if count == DEFAULT_MAX_TAGS + 1 && max == DEFAULT_MAX_TAGS
        ));
    }

    /// A single oversized tag is rejected with its index.
    #[test]
    fn normalize_rejects_oversized_tag() {
        let bounds = AdvertisementBounds::default();
        let long = "z".repeat(DEFAULT_MAX_TAG_LEN + 1);
        let err = normalize_room_metadata("Lobby", "", &format!("ok,{long}"), &bounds).unwrap_err();
        assert!(matches!(
            err,
            AdvertisementViolation::TagTooLong { index, len, max }
                if index == 1 && len == DEFAULT_MAX_TAG_LEN + 1 && max == DEFAULT_MAX_TAG_LEN
        ));
    }

    /// Empty tag segments (trailing/doubled commas) are dropped, not errors.
    #[test]
    fn normalize_drops_empty_tag_segments() {
        let bounds = AdvertisementBounds::default();
        let out = normalize_room_metadata("Lobby", "", "rust,,dev,", &bounds).unwrap();
        assert_eq!(out.tags, vec!["rust".to_string(), "dev".to_string()]);
    }

    /// Control characters are sanitized away (replaced), never broadcast.
    #[test]
    fn normalize_sanitizes_control_characters() {
        let bounds = AdvertisementBounds::default();
        let out = normalize_room_metadata(
            "bad\u{0000}name",
            "line1\u{0008}line2",
            "ta\u{0000}g",
            &bounds,
        )
        .unwrap();
        assert!(!out.room_name.contains('\u{0000}'));
        assert!(!out.short_description.contains('\u{0008}'));
        assert!(!out.tags[0].contains('\u{0000}'));
        // The sanitized output must also pass the wire validate().
        let mut advert =
            PublicRoomAdvertisement::minimal(topic(0xAB), out.room_name.clone(), key(0xCD));
        advert.short_description = out.short_description.clone();
        advert.tags = out.tags.clone();
        assert!(advert.validate(&bounds).is_ok());
    }

    /// Normalized output always satisfies the wire validation bounds.
    #[test]
    fn normalize_output_passes_wire_validation() {
        let bounds = AdvertisementBounds::default();
        let name = "Community".to_string();
        let desc = "A place for everyone.".to_string();
        let out = normalize_room_metadata(&name, &desc, "rust, open-source", &bounds).unwrap();
        let mut advert =
            PublicRoomAdvertisement::minimal(topic(0xEF), out.room_name.clone(), key(0x01));
        advert.short_description = out.short_description.clone();
        advert.tags = out.tags.clone();
        assert!(advert.validate(&bounds).is_ok());
    }

    // ── BORU-DIR-06: directory-visibility switch planning ────────────────

    /// An authorized owner switching an unlisted room to discoverable must
    /// get `Published` — the caller publishes a fresh advertisement.
    #[test]
    fn switch_unlisted_to_discoverable_publishes() {
        assert_eq!(
            plan_visibility_switch(
                RoomVisibility::PublicUnlisted,
                RoomVisibility::PublicDiscoverable,
                true,
            ),
            VisibilitySwitchOutcome::Published
        );
    }

    /// An authorized owner switching a discoverable room to unlisted must
    /// get `Unlisted` — the caller stops refreshing (TTL expiry applies).
    #[test]
    fn switch_discoverable_to_unlisted_unlists() {
        assert_eq!(
            plan_visibility_switch(
                RoomVisibility::PublicDiscoverable,
                RoomVisibility::PublicUnlisted,
                true,
            ),
            VisibilitySwitchOutcome::Unlisted
        );
    }

    /// Requesting the same visibility is a no-op.
    #[test]
    fn switch_to_same_visibility_is_no_change() {
        assert_eq!(
            plan_visibility_switch(
                RoomVisibility::PublicDiscoverable,
                RoomVisibility::PublicDiscoverable,
                true,
            ),
            VisibilitySwitchOutcome::NoChange
        );
        assert_eq!(
            plan_visibility_switch(
                RoomVisibility::PublicUnlisted,
                RoomVisibility::PublicUnlisted,
                true,
            ),
            VisibilitySwitchOutcome::NoChange
        );
    }

    /// A non-authorized user cannot change directory visibility (PDF Task
    /// 2.3: "Do not: let non-authorized users change directory visibility").
    #[test]
    fn switch_forbidden_for_non_owner() {
        for current in [
            RoomVisibility::Private,
            RoomVisibility::PublicUnlisted,
            RoomVisibility::PublicDiscoverable,
        ] {
            for requested in [
                RoomVisibility::Private,
                RoomVisibility::PublicUnlisted,
                RoomVisibility::PublicDiscoverable,
            ] {
                assert_eq!(
                    plan_visibility_switch(current, requested, false),
                    VisibilitySwitchOutcome::Forbidden,
                    "non-owner must never change visibility {current:?} -> {requested:?}",
                );
            }
        }
    }

    /// Private rooms are never part of the directory; switching into or out
    /// of Private is out of scope and rejected even for the owner.
    #[test]
    fn switch_involving_private_is_forbidden() {
        assert_eq!(
            plan_visibility_switch(
                RoomVisibility::Private,
                RoomVisibility::PublicDiscoverable,
                true,
            ),
            VisibilitySwitchOutcome::Forbidden
        );
        assert_eq!(
            plan_visibility_switch(
                RoomVisibility::PublicDiscoverable,
                RoomVisibility::Private,
                true,
            ),
            VisibilitySwitchOutcome::Forbidden
        );
        assert_eq!(
            plan_visibility_switch(
                RoomVisibility::PublicUnlisted,
                RoomVisibility::Private,
                true,
            ),
            VisibilitySwitchOutcome::Forbidden
        );
    }

    // ── BORU-DIR-09 (PDF Task 3.3): withdrawal / tombstone ────────────

    fn full_withdrawal() -> PublicRoomWithdrawal {
        PublicRoomWithdrawal {
            withdrawal_version: WITHDRAWAL_PAYLOAD_VERSION,
            room_id: topic(0x11),
            owner_peer_id: key(0x22),
            timestamp_secs: 1_700_000_000,
            signature: None,
        }
    }

    /// A publisher-signed withdrawal verifies as
    /// [`AdvertisementAuth::Verified`] for that publisher, and only for
    /// that publisher.
    #[test]
    fn signed_withdrawal_verifies_for_publisher() {
        let publisher = secret_key(0x22); // matches full_withdrawal()'s owner
        let mut withdrawal = full_withdrawal();
        assert_eq!(
            withdrawal.verify_signed(&publisher.public()),
            AdvertisementAuth::MissingSignature
        );
        withdrawal.sign(&publisher);
        assert_eq!(
            withdrawal.verify_signed(&publisher.public()),
            AdvertisementAuth::Verified {
                publisher: publisher.public()
            }
        );

        // A different key cannot claim the withdrawal (spoofed publisher).
        let stranger = secret_key(0x33);
        assert_eq!(
            withdrawal.verify_signed(&stranger.public()),
            AdvertisementAuth::InvalidSignature
        );
    }

    /// A withdrawal verifies only for the room authority named in
    /// `owner_peer_id` — the same authoritative identity rule used for
    /// canonical advertisements (BORU-DIR-03). A verified-but-non-authority
    /// publisher must not be able to withdraw a room.
    #[test]
    fn withdrawal_requires_authoritative_publisher() {
        let owner = secret_key(0x22);
        let member = secret_key(0x33);

        let mut withdrawal = full_withdrawal();
        withdrawal.owner_peer_id = owner.public().as_bytes().to_owned();
        withdrawal.sign(&owner);
        assert_eq!(
            withdrawal.verify_signed(&owner.public()),
            AdvertisementAuth::Verified {
                publisher: owner.public()
            }
        );
        assert!(
            withdrawal.is_authoritative_publisher(&owner.public()),
            "owner is the designated room authority"
        );

        // A member endorsement-style signature verifies but is NOT the
        // authority — the withdrawal must be discarded, never applied.
        let mut member_withdrawal = full_withdrawal();
        member_withdrawal.owner_peer_id = owner.public().as_bytes().to_owned();
        member_withdrawal.sign(&member);
        assert_eq!(
            member_withdrawal.verify_signed(&member.public()),
            AdvertisementAuth::Verified {
                publisher: member.public()
            }
        );
        assert!(
            !member_withdrawal.is_authoritative_publisher(&member.public()),
            "non-authority publisher must not be treated as the room authority"
        );
    }

    /// A withdrawal for one room cannot be replayed against another room:
    /// the room id is inside the signed framing.
    #[test]
    fn withdrawal_is_bound_to_room_id() {
        let owner = secret_key(0x22);
        let mut withdrawal = full_withdrawal();
        withdrawal.sign(&owner);
        let signature = withdrawal.signature.clone().unwrap();

        let mut other = full_withdrawal();
        other.room_id = topic(0x99);
        other.signature = Some(signature);
        assert_eq!(
            other.verify_signed(&owner.public()),
            AdvertisementAuth::InvalidSignature,
            "replaying a withdrawal for a different room must fail"
        );
    }

    /// A valid withdrawal is compact and passes the same bounds as
    /// advertisements.
    #[test]
    fn withdrawal_validates_and_is_compact() {
        let mut withdrawal = full_withdrawal();
        withdrawal.sign(&secret_key(0x22));
        let bounds = AdvertisementBounds::default();
        assert!(withdrawal.validate(&bounds).is_ok());
        let encoded = postcard::to_stdvec(&withdrawal).unwrap();
        assert!(
            encoded.len() < 200,
            "withdrawal should be tiny, got {} bytes",
            encoded.len()
        );
    }

    /// An invalid authority key fails validation.
    #[test]
    fn withdrawal_rejects_invalid_owner_key() {
        let mut withdrawal = full_withdrawal();
        withdrawal.owner_peer_id = [0x02; 32]; // not a valid ed25519 point
        assert_eq!(
            withdrawal.validate(&AdvertisementBounds::default()),
            Err(AdvertisementViolation::InvalidOwnerPeerId)
        );
    }
}
