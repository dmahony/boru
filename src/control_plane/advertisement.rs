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
//! # Wire compatibility
//!
//! The payload is postcard-encoded as part of [`ControlPayload`](crate::control_plane::message::ControlPayload).
//! New fields are appended at the END of the struct so older clients decode
//! the known prefix and ignore the trailing bytes (the envelope decoder uses
//! `postcard::take_from_bytes` and discards trailing payload bytes).

use crate::proto::state::TopicId;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Current advertisement payload version (PDF Task 1.2 step 1). Bump when
/// the metadata model changes incompatibly; receivers treat unknown versions
/// as metadata to cache, never as an authorisation signal.
pub const ADVERTISEMENT_PAYLOAD_VERSION: u8 = 1;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoomVisibility {
    /// Closed groups / private communities — invite/authorisation only.
    /// Never advertised.
    Private,
    /// Shareable but not browsable — requires room ID/invite/link.
    /// Never advertised.
    PublicUnlisted,
    /// Open public communities — advertised in the directory and joinable
    /// via an explicit Join action.
    PublicDiscoverable,
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
///
/// Explicitly **not** present (privacy guardrails): member lists, member
/// identities, chat history, chat previews, filenames, invite secrets,
/// moderation state, private keys, attachment content, and
/// `signature_or_auth_proof` (that is BORU-DIR-03, out of scope here).
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
    /// Descriptive metadata only until BORU-DIR-03 signs advertisements; the
    /// directory must not grant moderation or join privileges based solely
    /// on this field (PDF Task 1.3 step 3).
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
        }
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
        }
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
}
