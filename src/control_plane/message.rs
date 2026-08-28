#![allow(clippy::large_enum_variant)]

//! Versioned, typed control-plane message envelope (PDF Phase 1, Task 1.1).
//!
//! This module defines the wire format for the hidden-discovery **control
//! plane**: a compact envelope that carries **metadata only** (reachability,
//! state, capabilities, protocol compatibility) between Boru nodes on the
//! internal discovery topic. It must never carry user chat text, attachment
//! data, file bytes, group history, tunnel payloads, or call media — those
//! stay on the authenticated data plane.
//!
//! # Guarantees
//!
//! * **Cannot be confused with chat.** Every envelope starts with a 2-byte
//!   magic (`BC`), and the payloads are postcard-encoded
//!   [`ControlPayload`] enum values. The chat wire type
//!   ([`crate::chat_core::Message`](crate::chat_core::Message)) is a
//!   postcard enum whose variant tags live in `0..=19`; the magic byte
//!   `0x42` can never be a valid chat variant tag, so no control-plane byte
//!   stream ever deserialises as a chat message and no chat byte stream
//!   ever passes [`decode`](ControlEnvelope::decode) (tests prove both
//!   directions).
//! * **Forward compatible.** Unknown *message types* produce
//!   [`ControlPlaneDecode::UnknownType`] (the header is parsed, the payload
//!   is skipped by its length prefix, nothing crashes). Unknown *fields* on
//!   a known payload are ignored: the payload section is decoded with
//!   `postcard::take_from_bytes` and trailing bytes (fields appended by a
//!   newer sender) are discarded.
//! * **Fail closed per feature.** An unsupported
//!   [`protocol_version`](ControlEnvelope::protocol_version) yields
//!   [`ControlPlaneDecode::UnsupportedVersion`] — the feature is dropped but
//!   the client keeps running.
//! * **Strict.** [`decode`](ControlEnvelope::decode) never panics. Malformed
//!   input (bad magic, truncated frames, oversized payloads, invalid node
//!   ids, payload/type mismatch) returns a structured
//!   [`ControlPlaneError`]; the caller logs and rate-limits it, then
//!   discards the frame. The decoder has no access to the gossip actor or
//!   chat processing, so a malformed packet can never affect them.
//! * **Bounded.** Payloads are capped at [`MAX_CONTROL_PAYLOAD_LEN`] bytes.
//!
//! # Wire format
//!
//! ```text
//! magic           2 bytes  0x42 0x43 ("BC")
//! protocol_version 1 byte  u8
//! message_type     1 byte  u8 (ControlMessageType tag; unknown tags ignored)
//! sender_node_id  32 bytes raw iroh PublicKey (ed25519)
//! sequence        varint   u64  (postcard; per-sender monotonic counter)
//! timestamp_secs  varint   u64  (postcard; unix epoch seconds, 0 = unknown)
//! payload_len     varint   u32  (postcard; length of the payload section)
//! payload         payload_len bytes (postcard ControlPayload)
//! ```
//!
//! Typical `Hello` envelope: `2 + 1 + 1 + 32 + 1 + 5 + 1 + 2 = 45 B`.
//!
//! # Versioning contract
//!
//! Bumping [`CONTROL_PLANE_PROTOCOL_VERSION`] signals an **incompatible
//! header layout** — older clients return
//! [`ControlPlaneDecode::UnsupportedVersion`] and never parse further.
//! Within a version, the format stays compatible: new `message_type` tags
//! and new payload fields are tolerated by the rules above.
//!
//! # Caller contract (BORU-CP-02+)
//!
//! [`decode`](ControlEnvelope::decode) is a pure function. The receive path
//! should log the returned error/outcome (state transitions, never message
//! contents), apply abuse-control rate limits to peers that repeatedly send
//! malformed frames, and discard the frame. Deduplicate announcements by
//! `(sender_node_id, sequence)` via [`ControlEnvelope::dedup_key`].

use iroh_base::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

/// Magic prefix distinguishing control-plane envelopes from everything else
/// on the wire. `0x42` can never be a valid chat [`crate::chat_core::Message`]
/// variant tag (those are `0..=19`), so a control packet can never
/// deserialise as a chat message and vice versa.
pub const CONTROL_PLANE_MAGIC: [u8; 2] = *b"BC";

/// Current control-plane envelope format version.
///
/// Independent from the discovery-topic derivation version
/// ([`BORU_DISCOVERY_PROTOCOL_VERSION`](crate::discovery_topic::BORU_DISCOVERY_PROTOCOL_VERSION)):
/// bumping this signals an incompatible header layout, not a topic change.
pub const CONTROL_PLANE_PROTOCOL_VERSION: u8 = 1;

/// Upper bound for a control-plane payload section (bounded-resources
/// guardrail). Control payloads carry tiny metadata; 4 KiB is far more than
/// any current or planned payload needs while still capping abuse.
pub const MAX_CONTROL_PAYLOAD_LEN: u32 = 4096;

/// Application-protocol version this client advertises in its HELLO
/// (BORU-CP-04 / PDF Task 2.1).
///
/// Distinct from [`CONTROL_PLANE_PROTOCOL_VERSION`] (the envelope/header
/// format version) and from the discovery-topic derivation version
/// ([`BORU_DISCOVERY_PROTOCOL_VERSION`](crate::discovery_topic::BORU_DISCOVERY_PROTOCOL_VERSION)).
/// Bump it when the *application-level* control-plane semantics change
/// (new presence semantics, capability negotiation rules); receivers store
/// it per-peer in [`crate::control_plane::privacy::PeerControlState`] but
/// never fail the whole client on an unknown value (fail closed per
/// feature).
pub const BORU_APP_PROTOCOL_VERSION: u8 = 1;

/// `message_type` enum for the control plane (PDF Task 1.1 step 3).
///
/// Tag values are stable wire constants: `0 = HELLO`, `1 = PRESENCE`,
/// `2 = CAPABILITIES`, `3 = DIAGNOSTIC_HINT`, `4 = EXTENSIONS` (BORU-CP-16,
/// PDF Phase 6), `5 = PUBLIC_ROOM_ADVERTISEMENT` (BORU-DIR-01, PDF Phase 1
/// Task 1.1). Unknown tags are tolerated by
/// [`ControlEnvelope::decode`] as [`ControlPlaneDecode::UnknownType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ControlMessageType {
    /// A node announces itself after joining the discovery topic.
    Hello = 0,
    /// Periodic presence heartbeat — "I am still here".
    Presence = 1,
    /// Advertised feature support (metadata only, namespaced versioned ids).
    Capabilities = 2,
    /// Structured connectivity hint for diagnostics (never chat contents).
    DiagnosticHint = 3,
    /// Optional Phase 6 extensions (metadata only — see
    /// [`ExtensionsPayload`](crate::control_plane::extensions::ExtensionsPayload)).
    Extensions = 4,
    /// A room-discovery advertisement — "this public room exists and is
    /// discoverable" (BORU-DIR-01, PDF Phase 1 Task 1.1). Metadata only:
    /// the advertisement describes a room for the local directory cache; it
    /// never joins the room, subscribes to its chat topic, downloads its
    /// history, or grants permission. Fully separate from peer presence and
    /// from normal chat messages.
    PublicRoomAdvertisement = 5,
    /// A room-withdrawal / tombstone — "this public room is no longer
    /// advertised" (BORU-DIR-09, PDF Phase 3 Task 3.3). Broadcast when a
    /// room is deleted, made unlisted, or intentionally removed from
    /// discovery so directory clients can remove the matching advertisement
    /// immediately instead of waiting for the advertisement TTL. Carries a
    /// signed payload authenticated with the same authoritative identity
    /// rules as advertisements; TTL expiry remains the safety net if the
    /// withdrawal is missed.
    PublicRoomWithdrawal = 6,
}

impl ControlMessageType {
    /// Map a wire tag byte to a [`ControlMessageType`], or `None` for an
    /// unknown (future) tag.
    pub fn from_u8(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Hello),
            1 => Some(Self::Presence),
            2 => Some(Self::Capabilities),
            3 => Some(Self::DiagnosticHint),
            4 => Some(Self::Extensions),
            5 => Some(Self::PublicRoomAdvertisement),
            6 => Some(Self::PublicRoomWithdrawal),
            _ => None,
        }
    }

    /// The stable wire tag for this message type.
    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// HELLO payload — announces a node after it joins the discovery topic.
///
/// Metadata only: the stable peer identity lives in the envelope
/// ([`ControlEnvelope::sender_node_id`]); this payload carries the minimum
/// protocol metadata a peer needs to know before interpreting capability or
/// presence state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloPayload {
    /// Application-protocol version the sender speaks, separate from the
    /// envelope's `protocol_version` (PDF Task 4.1: separate application
    /// protocol version from individual feature capability versions).
    pub app_protocol_version: u8,
}

/// Maximum length of a coarse network country code.
pub const MAX_PRESENCE_COUNTRY_CODE_LEN: usize = 2;

/// Coarse, privacy-safe location/network metadata attached to presence.
/// No raw address is represented by this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoarsePresence {
    /// ISO-3166-1 alpha-2 country code, when known.
    #[serde(default)]
    pub country_code: Option<String>,
    /// Coarse latitude in degrees, in the inclusive range -90..=90.
    #[serde(default)]
    pub latitude: Option<f64>,
    /// Coarse longitude in degrees, in the inclusive range -180..=180.
    #[serde(default)]
    pub longitude: Option<f64>,
    /// Autonomous system number, when known.
    #[serde(default)]
    pub asn: Option<u32>,
}

impl Eq for CoarsePresence {}

impl CoarsePresence {
    /// Return whether every supplied field has an acceptable shape/range.
    pub fn is_valid(&self) -> bool {
        let country_valid = self.country_code.as_deref().is_none_or(|code| {
            code.len() == MAX_PRESENCE_COUNTRY_CODE_LEN
                && code.bytes().all(|byte| byte.is_ascii_uppercase())
        });
        let coordinates_valid = match (self.latitude, self.longitude) {
            (None, None) => true,
            (Some(latitude), Some(longitude)) => {
                latitude.is_finite()
                    && longitude.is_finite()
                    && (-90.0..=90.0).contains(&latitude)
                    && (-180.0..=180.0).contains(&longitude)
            }
            _ => false,
        };
        country_valid && coordinates_valid
    }

    /// Drop malformed fields without rejecting the surrounding presence.
    pub fn sanitized(&self) -> Option<Self> {
        let mut value = self.clone();
        if value.country_code.as_deref().is_some_and(|code| {
            code.len() != MAX_PRESENCE_COUNTRY_CODE_LEN
                || !code.bytes().all(|byte| byte.is_ascii_uppercase())
        }) {
            value.country_code = None;
        }
        if !matches!((value.latitude, value.longitude), (None, None))
            && !matches!((value.latitude, value.longitude), (Some(_), Some(_)))
        {
            value.latitude = None;
            value.longitude = None;
        } else if let (Some(latitude), Some(longitude)) = (value.latitude, value.longitude) {
            if !(latitude.is_finite()
                && longitude.is_finite()
                && (-90.0..=90.0).contains(&latitude)
                && (-180.0..=180.0).contains(&longitude))
            {
                value.latitude = None;
                value.longitude = None;
            }
        }
        if value.country_code.is_none()
            && value.latitude.is_none()
            && value.longitude.is_none()
            && value.asn.is_none()
        {
            None
        } else {
            Some(value)
        }
    }
}

fn deserialize_tolerant_opt_coarse<'de, D>(
    deserializer: D,
) -> Result<Option<CoarsePresence>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<CoarsePresence>::deserialize(deserializer) {
        Ok(value) => Ok(value),
        Err(_) => Ok(None),
    }
}

/// PRESENCE payload — "I am still here".
///
/// Metadata only: no usernames, profile text, device details, or raw IPs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresencePayload {
    /// Suggested TTL (seconds) before this presence should be considered
    /// stale. `None` = use the receiver's default TTL.
    #[serde(default)]
    pub ttl_secs: Option<u32>,
    /// Optional privacy-safe coarse metadata. Trailing for legacy decoding.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_tolerant_opt_coarse"
    )]
    pub coarse: Option<CoarsePresence>,
}

impl Eq for PresencePayload {}

/// CAPABILITIES payload — advertised feature support.
///
/// Namespaced, versioned capability identifiers (e.g. `files-v2`,
/// `tunnels-v1`, `voice-v1`). Older clients ignore identifiers they do not
/// understand (PDF Task 4.1: represent capabilities as a set/map that can
/// tolerate unknown future values). This is metadata, never authorisation:
/// a capability advertisement never grants chat/group/file/tunnel access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitiesPayload {
    /// Namespaced, versioned capability ids advertised by the sender.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl CapabilitiesPayload {
    /// Build a payload from a typed [`CapabilitySet`]
    /// (crate::control_plane::capabilities::CapabilitySet).
    ///
    /// The wire form is the set's ordered, deduplicated id list; unknown
    /// future ids are preserved verbatim.
    pub fn from_set(set: &crate::control_plane::capabilities::CapabilitySet) -> Self {
        Self {
            capabilities: set.to_wire(),
        }
    }

    /// Decode this payload into a typed [`CapabilitySet`]
    /// (crate::control_plane::capabilities::CapabilitySet).
    ///
    /// Lossless: every id this client does not understand is preserved
    /// rather than dropped.
    pub fn to_set(&self) -> crate::control_plane::capabilities::CapabilitySet {
        crate::control_plane::capabilities::CapabilitySet::from_wire(
            self.capabilities.iter().cloned(),
        )
    }
}

/// DIAGNOSTIC_HINT payload — a structured connectivity hint.
///
/// Carries a stable numeric hint code plus an optional short note. Never
/// carries chat contents, secrets, or message data (PDF: log state
/// transitions, not message contents).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticHintPayload {
    /// Stable numeric hint code (e.g. `1 = relay-only path`,
    /// `2 = direct path re-established`, `3 = topic join failed`).
    pub hint_code: u32,
    /// Optional short human-readable note (no chat contents, no secrets).
    #[serde(default)]
    pub note: Option<String>,
}

/// PUBLIC_ROOM_ADVERTISEMENT payload (BORU-DIR-01, PDF Phase 1 Task 1.1;
/// metadata model BORU-DIR-02, PDF Task 1.2).
///
/// A room-discovery advertisement: "this public room exists and is
/// discoverable". Metadata only — it describes a room for the local
/// directory cache; it never joins the room, subscribes to its chat topic,
/// downloads its history, or grants permission (PDF Core rule).
///
/// The typed payload is the bounded, versioned metadata model
/// ([`crate::control_plane::advertisement::PublicRoomAdvertisement`])
/// defined by BORU-DIR-02: room identity, name, short description, room
/// protocol version, owner peer id, visibility, TTL, and optional tags /
/// coarse activity / approximate member count / avatar hash / feature
/// flags — every field with a documented semantics and size limit, and no
/// private room information (no member lists, history, previews, secrets,
/// or attachment content).
///
/// Fully separate from peer presence and normal chat messages: the
/// envelope's `message_type` is `PublicRoomAdvertisement` (tag 5), its
/// payload is this typed struct, and the decoder's cross-type guard rejects
/// any frame that mixes this payload with a different `message_type`.
pub type PublicRoomAdvertisementPayload =
    crate::control_plane::advertisement::PublicRoomAdvertisement;

/// PUBLIC_ROOM_WITHDRAWAL payload (BORU-DIR-09, PDF Phase 3 Task 3.3).
///
/// A room-withdrawal / tombstone: \"this public room is no longer
/// advertised\". Metadata only — it names the room being withdrawn and the
/// room's designated authority; directory clients remove the matching
/// advertisement immediately **when the withdrawal verifies** (the same
/// authoritative identity rules as advertisements, BORU-DIR-03), and TTL
/// expiry remains the safety net if the withdrawal is missed.
pub type PublicRoomWithdrawalPayload = crate::control_plane::advertisement::PublicRoomWithdrawal;

/// Typed payload carried by a [`ControlEnvelope`].
///
/// The payload enum is self-describing on the wire (postcard variant tag),
/// which lets [`ControlEnvelope::decode`] cross-check that the payload's own
/// type matches the envelope's `message_type` — a mismatch is malformed.
///
/// # Adding a payload type
///
/// Append the new variant at the END of the enum so existing postcard
/// variant indices (and therefore existing wire tags) stay stable. Update
/// [`ControlMessageType`] (new tag + `from_u8`), [`ControlPayload::message_type`],
/// [`ControlEnvelope::decode`]'s cross-check (it uses `message_type`), and
/// the strict-decoder tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlPayload {
    /// [`HelloPayload`] — initial announcement.
    Hello(HelloPayload),
    /// [`PresencePayload`] — periodic heartbeat.
    Presence(PresencePayload),
    /// [`CapabilitiesPayload`] — advertised feature support.
    Capabilities(CapabilitiesPayload),
    /// [`DiagnosticHintPayload`] — structured connectivity hint.
    DiagnosticHint(DiagnosticHintPayload),
    /// [`ExtensionsPayload`] — optional Phase 6 metadata extensions
    /// (BORU-CP-16; see
    /// [`crate::control_plane::extensions`](crate::control_plane::extensions)).
    Extensions(crate::control_plane::extensions::ExtensionsPayload),
    /// [`PublicRoomAdvertisementPayload`] — a room-discovery advertisement
    /// (BORU-DIR-01, PDF Phase 1 Task 1.1). Metadata only; the advertised
    /// metadata model is defined by BORU-DIR-02 (PDF Task 1.2).
    PublicRoomAdvertisement(PublicRoomAdvertisementPayload),
    /// [`PublicRoomWithdrawalPayload`] — a room-withdrawal / tombstone
    /// (BORU-DIR-09, PDF Phase 3 Task 3.3). Metadata only; carries the room
    /// being withdrawn plus the room's designated authority, authenticated
    /// with the same authoritative identity rules as advertisements.
    PublicRoomWithdrawal(PublicRoomWithdrawalPayload),
}

impl ControlPayload {
    /// The [`ControlMessageType`] this payload carries.
    pub fn message_type(&self) -> ControlMessageType {
        match self {
            Self::Hello(_) => ControlMessageType::Hello,
            Self::Presence(_) => ControlMessageType::Presence,
            Self::Capabilities(_) => ControlMessageType::Capabilities,
            Self::DiagnosticHint(_) => ControlMessageType::DiagnosticHint,
            Self::Extensions(_) => ControlMessageType::Extensions,
            Self::PublicRoomAdvertisement(_) => ControlMessageType::PublicRoomAdvertisement,
            Self::PublicRoomWithdrawal(_) => ControlMessageType::PublicRoomWithdrawal,
        }
    }
}

/// A versioned, typed control-plane envelope (PDF Task 1.1 step 2).
///
/// See the [module docs](self) for the wire format and compatibility
/// contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlEnvelope {
    /// Control-plane envelope format version
    /// ([`CONTROL_PLANE_PROTOCOL_VERSION`]).
    pub protocol_version: u8,
    /// Typed message kind (HELLO / PRESENCE / CAPABILITIES /
    /// DIAGNOSTIC_HINT / EXTENSIONS / PUBLIC_ROOM_ADVERTISEMENT).
    pub message_type: ControlMessageType,
    /// Identity (iroh Ed25519 public key) of the sending node.
    pub sender_node_id: PublicKey,
    /// Per-sender monotonic sequence counter / nonce. Receivers dedup
    /// announcements by `(sender_node_id, sequence)`.
    pub sequence: u64,
    /// Unix epoch seconds when the message was created; `0` = unknown.
    pub timestamp_secs: u64,
    /// The typed payload for [`Self::message_type`].
    pub payload: ControlPayload,
    /// Ed25519 signature over the canonical envelope bytes (BORU-CP-17 /
    /// PDF Task 4.3 fix): proves the envelope was authored by
    /// `sender_node_id` regardless of which gossip peer relayed it. This is
    /// what lets a receiver attribute a relayed control envelope — the
    /// gossip transport only authenticates `delivered_from` (the immediate
    /// forwarder), which on a multi-node mesh is NOT the original author.
    ///
    /// `None` = legacy unsigned envelope (accepted only from the
    /// authenticated direct delivery source).
    pub signature: Option<[u8; 64]>,
    /// True iff `signature` was verified against `sender_node_id` over the
    /// canonical bytes at decode time. Only meaningful when `signature` is
    /// `Some`.
    pub signature_valid: bool,
}

impl ControlEnvelope {
    /// Build an envelope speaking the current protocol version. The
    /// `message_type` is derived from the payload so the two can never
    /// disagree.
    pub fn new(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        payload: ControlPayload,
    ) -> Self {
        let message_type = payload.message_type();
        Self {
            protocol_version: CONTROL_PLANE_PROTOCOL_VERSION,
            message_type,
            sender_node_id,
            sequence,
            timestamp_secs,
            payload,
            signature: None,
            signature_valid: false,
        }
    }

    /// Convenience constructor for a HELLO envelope.
    pub fn hello(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        app_protocol_version: u8,
    ) -> Self {
        Self::new(
            sender_node_id,
            sequence,
            timestamp_secs,
            ControlPayload::Hello(HelloPayload {
                app_protocol_version,
            }),
        )
    }

    /// Convenience constructor for a PRESENCE envelope.
    pub fn presence(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        ttl_secs: Option<u32>,
    ) -> Self {
        Self::presence_with_coarse(sender_node_id, sequence, timestamp_secs, ttl_secs, None)
    }

    /// Convenience constructor for a PRESENCE envelope with optional coarse
    /// network metadata. The metadata is bounded and contains no raw address.
    pub fn presence_with_coarse(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        ttl_secs: Option<u32>,
        coarse: Option<CoarsePresence>,
    ) -> Self {
        Self::new(
            sender_node_id,
            sequence,
            timestamp_secs,
            ControlPayload::Presence(PresencePayload { ttl_secs, coarse }),
        )
    }

    /// Convenience constructor for a CAPABILITIES envelope.
    pub fn capabilities(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        capabilities: Vec<String>,
    ) -> Self {
        Self::new(
            sender_node_id,
            sequence,
            timestamp_secs,
            ControlPayload::Capabilities(CapabilitiesPayload { capabilities }),
        )
    }

    /// Convenience constructor for a DIAGNOSTIC_HINT envelope.
    pub fn diagnostic_hint(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        hint_code: u32,
        note: Option<String>,
    ) -> Self {
        Self::new(
            sender_node_id,
            sequence,
            timestamp_secs,
            ControlPayload::DiagnosticHint(DiagnosticHintPayload { hint_code, note }),
        )
    }

    /// Convenience constructor for an EXTENSIONS envelope (BORU-CP-16, PDF
    /// Phase 6). Carries optional metadata-only Phase 6 extensions; the
    /// payload is bounded by the privacy layer's [`ExtensionsBounds`](crate::control_plane::extensions::ExtensionsBounds).
    pub fn extensions(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        extensions: crate::control_plane::extensions::ExtensionsPayload,
    ) -> Self {
        Self::new(
            sender_node_id,
            sequence,
            timestamp_secs,
            ControlPayload::Extensions(extensions),
        )
    }

    /// Convenience constructor for a PUBLIC_ROOM_ADVERTISEMENT envelope
    /// (BORU-DIR-01, PDF Phase 1 Task 1.1). Carries a typed, bounded
    /// room-discovery advertisement (BORU-DIR-02 metadata model), fully
    /// separate from peer presence and chat messages.
    pub fn public_room_advertisement(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        advert: crate::control_plane::advertisement::PublicRoomAdvertisement,
    ) -> Self {
        Self::new(
            sender_node_id,
            sequence,
            timestamp_secs,
            ControlPayload::PublicRoomAdvertisement(advert),
        )
    }

    /// Convenience constructor for a PUBLIC_ROOM_WITHDRAWAL envelope
    /// (BORU-DIR-09, PDF Phase 3 Task 3.3). Carries a typed, bounded
    /// room-withdrawal / tombstone authenticated with the same
    /// authoritative identity rules as advertisements; directory clients
    /// remove the matching advertisement when it verifies, and TTL expiry
    /// remains the safety net if it is missed.
    pub fn public_room_withdrawal(
        sender_node_id: PublicKey,
        sequence: u64,
        timestamp_secs: u64,
        withdrawal: crate::control_plane::advertisement::PublicRoomWithdrawal,
    ) -> Self {
        Self::new(
            sender_node_id,
            sequence,
            timestamp_secs,
            ControlPayload::PublicRoomWithdrawal(withdrawal),
        )
    }

    /// Dedup key for announcements: `(sender_node_id, sequence)`.
    ///
    /// Duplicate delivery of the same announcement (e.g. over two discovery
    /// paths) yields the same key; the caller represents the event once.
    pub fn dedup_key(&self) -> (PublicKey, u64) {
        (self.sender_node_id, self.sequence)
    }

    /// Sign this envelope with the node's Ed25519 key (BORU-CP-17).
    ///
    /// The signature covers the canonical bytes
    /// (`boru/control-plane`, protocol version) of every security-relevant
    /// field: message type, sender identity, sequence, timestamp, and the
    /// exact payload section bytes. After signing, [`encode`](Self::encode)
    /// appends the 64-byte signature to the wire frame; a receiver verifies
    /// it against `sender_node_id` so a relayed envelope can be attributed
    /// to its true author even though the gossip transport only
    /// authenticates the immediate forwarder.
    pub fn sign(&mut self, sk: &SecretKey) {
        let payload = self.payload_bytes();
        let canonical = crate::protocol_signing::canonical_signed_bytes(
            crate::control_plane::CONTROL_PLANE_SIGNING_DOMAIN,
            self.protocol_version as u16,
            &(
                self.message_type.to_u8(),
                *self.sender_node_id.as_bytes(),
                self.sequence,
                self.timestamp_secs,
                &payload[..],
            ),
        )
        .expect("control-plane canonical bytes are infallible");
        let sig = sk.sign(&canonical);
        self.signature = Some(sig.to_bytes());
        self.signature_valid = true;
    }

    /// The exact payload section bytes as they appear on the wire (and as
    /// they are covered by [`sign`](Self::sign)).
    fn payload_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&self.payload).expect("control payload encoding is infallible")
    }

    /// Serialise this envelope to its compact wire form.
    ///
    /// Cannot fail for in-memory envelopes: payload encoding is infallible
    /// for the supported types and the payload length is bounded by
    /// [`MAX_CONTROL_PAYLOAD_LEN`].
    ///
    /// Layout: `magic(2) | protocol_version(1) | WireHeader | payload | [signature(64)]`
    /// where the trailing signature is present iff [`sign`](Self::sign) ran.
    /// Old receivers ignore trailing bytes beyond the payload section
    /// (documented forward-compatibility), so signed frames still decode on
    /// peers that predate BORU-CP-17 — they simply cannot verify the
    /// author and fall back to the direct-delivery attribution rule.
    pub fn encode(&self) -> Vec<u8> {
        let payload = self.payload_bytes();
        debug_assert!(
            payload.len() <= MAX_CONTROL_PAYLOAD_LEN as usize,
            "payload exceeds MAX_CONTROL_PAYLOAD_LEN"
        );
        let header = WireHeader {
            message_type: self.message_type.to_u8(),
            sender_node_id: *self.sender_node_id.as_bytes(),
            sequence: self.sequence,
            timestamp_secs: self.timestamp_secs,
            payload_len: payload.len() as u32,
        };
        let mut out = Vec::with_capacity(3 + 40 + payload.len() + 64);
        out.extend_from_slice(&CONTROL_PLANE_MAGIC);
        out.push(self.protocol_version); // fixed offset bytes[2]
        out.extend_from_slice(
            &postcard::to_stdvec(&header).expect("header encoding is infallible"),
        );
        out.extend_from_slice(&payload);
        if let Some(sig) = &self.signature {
            out.extend_from_slice(sig);
        }
        out
    }

    /// Strictly decode a control-plane envelope from its wire form.
    ///
    /// Never panics. Returns:
    /// * [`ControlPlaneDecode::Message`] for a fully decoded, type-consistent
    ///   envelope;
    /// * [`ControlPlaneDecode::UnknownType`] for a structurally valid
    ///   envelope whose `message_type` this client does not know (payload
    ///   skipped by its length prefix — safe forward compatibility);
    /// * [`ControlPlaneDecode::UnsupportedVersion`] for an envelope speaking
    ///   an unknown protocol version (fail closed for that feature);
    /// * [`Err`] with a structured [`ControlPlaneError`] for malformed
    ///   input. The caller logs/rate-limits and discards; this decoder never
    ///   touches the gossip actor or chat processing.
    pub fn decode(bytes: &[u8]) -> Result<ControlPlaneDecode, ControlPlaneError> {
        // Magic prefix (2 bytes) — distinguishes control plane from chat.
        if bytes.len() < 2 {
            return Err(ControlPlaneError::TooShort);
        }
        if bytes[0..2] != CONTROL_PLANE_MAGIC {
            return Err(ControlPlaneError::BadMagic);
        }
        // Protocol version gate: parse exactly one byte after the magic so
        // an unsupported version is rejected before any further trust.
        if bytes.len() < 3 {
            return Err(ControlPlaneError::TooShort);
        }
        let protocol_version = bytes[2];
        if protocol_version != CONTROL_PLANE_PROTOCOL_VERSION {
            return Ok(ControlPlaneDecode::UnsupportedVersion {
                found: protocol_version,
                expected: CONTROL_PLANE_PROTOCOL_VERSION,
            });
        }
        // Fixed header (postcard varints for sequence/timestamp/payload_len).
        let (header, rest) = postcard::take_from_bytes::<WireHeader>(&bytes[3..])
            .map_err(|_| ControlPlaneError::Truncated)?;
        let sender_node_id = PublicKey::from_bytes(&header.sender_node_id)
            .map_err(|_| ControlPlaneError::InvalidNodeId)?;
        // Bounded payload.
        if header.payload_len > MAX_CONTROL_PAYLOAD_LEN {
            return Err(ControlPlaneError::PayloadTooLarge {
                len: header.payload_len,
                max: MAX_CONTROL_PAYLOAD_LEN,
            });
        }
        let payload_bytes = rest
            .get(..header.payload_len as usize)
            .ok_or(ControlPlaneError::Truncated)?;
        // Trailing bytes beyond payload_len: exactly 64 bytes = the
        // BORU-CP-17 envelope signature (appended by a new sender);
        // anything else is a future envelope extension and is intentionally
        // ignored (forward compatibility).
        let trailing = &rest[header.payload_len as usize..];
        let signature = match trailing.len() {
            64 => Some(<[u8; 64]>::try_from(trailing).expect("64-byte slice is a [u8; 64]")),
            _ => None,
        };

        let Some(message_type) = ControlMessageType::from_u8(header.message_type) else {
            // Unknown (future) message type: header parsed, payload skipped.
            return Ok(ControlPlaneDecode::UnknownType {
                protocol_version,
                message_type: header.message_type,
                sender_node_id,
                sequence: header.sequence,
                timestamp_secs: header.timestamp_secs,
            });
        };

        // Typed payload; trailing bytes inside the payload section (fields
        // appended by a newer sender) are ignored (forward compatibility).
        let (payload, _trailing) = postcard::take_from_bytes::<ControlPayload>(payload_bytes)
            .map_err(|e| ControlPlaneError::MalformedPayload {
                message_type: header.message_type,
                reason: e.to_string(),
            })?;

        // Cross-type guard: the payload's own tag must match the envelope's
        // message_type, otherwise the frame is malformed.
        if payload.message_type() != message_type {
            return Err(ControlPlaneError::TypeMismatch {
                header_type: message_type.to_u8(),
                payload_type: payload.message_type().to_u8(),
            });
        }

        // BORU-CP-17: verify the trailing signature against the claimed
        // sender over the canonical bytes. An envelope that carries a
        // signature that does NOT verify is a spoofing attempt — it claims
        // to be `sender_node_id` without possessing that key — so it is
        // flagged as invalid (the guard rejects it), never silently
        // accepted.
        let signature_valid = match &signature {
            Some(sig) => {
                let canonical = crate::protocol_signing::canonical_signed_bytes(
                    crate::control_plane::CONTROL_PLANE_SIGNING_DOMAIN,
                    protocol_version as u16,
                    &(
                        message_type.to_u8(),
                        *sender_node_id.as_bytes(),
                        header.sequence,
                        header.timestamp_secs,
                        payload_bytes,
                    ),
                )
                .expect("control-plane canonical bytes are infallible");
                crate::protocol_signing::verify(&sender_node_id, sig, &canonical)
            }
            None => false,
        };

        Ok(ControlPlaneDecode::Message(ControlEnvelope {
            protocol_version,
            message_type,
            sender_node_id,
            sequence: header.sequence,
            timestamp_secs: header.timestamp_secs,
            payload,
            signature,
            signature_valid,
        }))
    }
}

/// Private fixed wire header (postcard-encoded after the magic prefix and
/// the standalone protocol_version byte).
///
/// Layout on the wire: `magic(2) | protocol_version(1) | WireHeader | payload`.
/// The sender node id travels as 32 raw bytes; validation to an iroh
/// [`PublicKey`] happens in [`ControlEnvelope::decode`] so an invalid key is
/// reported as [`ControlPlaneError::InvalidNodeId`] rather than a generic
/// decode error.
///
/// `pub(crate)` so integration tests (e.g. the discovery service's receive
/// path) can craft precise malformed/future-field frames; it is not part of
/// the public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WireHeader {
    pub(crate) message_type: u8,
    pub(crate) sender_node_id: [u8; 32],
    pub(crate) sequence: u64,
    pub(crate) timestamp_secs: u64,
    pub(crate) payload_len: u32,
}

/// Outcome of a successful (non-malformed) decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlPlaneDecode {
    /// A fully decoded, type-consistent control-plane envelope.
    Message(ControlEnvelope),
    /// A structurally valid envelope whose `message_type` tag is not known
    /// to this client (a future type). The payload was skipped by its length
    /// prefix — safe to ignore.
    UnknownType {
        /// Format version found on the wire.
        protocol_version: u8,
        /// The unknown message_type tag byte.
        message_type: u8,
        /// Parsed sender identity.
        sender_node_id: PublicKey,
        /// Parsed sequence counter.
        sequence: u64,
        /// Parsed timestamp (unix seconds).
        timestamp_secs: u64,
    },
    /// An envelope speaking an unsupported protocol version. Fail closed for
    /// that feature; the rest of the client is unaffected.
    UnsupportedVersion {
        /// Version found on the wire.
        found: u8,
        /// Version this client understands.
        expected: u8,
    },
}

/// Why a control-plane frame was rejected as malformed.
///
/// The caller (BORU-CP-02 receive path) logs the error, applies abuse-control
/// rate limits to repeat offenders, and discards the frame. Decoding never
/// panics and never touches the gossip actor or chat processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlPlaneError {
    /// Fewer than 2 bytes — cannot even verify the magic prefix.
    TooShort,
    /// The frame does not start with [`CONTROL_PLANE_MAGIC`] — it is not a
    /// control-plane envelope (could be a chat message or other traffic).
    BadMagic,
    /// The fixed header or declared payload section is shorter than the
    /// frame claims.
    Truncated,
    /// `sender_node_id` is not a valid iroh Ed25519 public key.
    InvalidNodeId,
    /// The payload section exceeds [`MAX_CONTROL_PAYLOAD_LEN`].
    PayloadTooLarge {
        /// Length declared in the header.
        len: u32,
        /// The bound that was exceeded.
        max: u32,
    },
    /// The payload section did not deserialise as a [`ControlPayload`] for
    /// the declared `message_type`.
    MalformedPayload {
        /// The message_type tag from the header.
        message_type: u8,
        /// Underlying postcard/serde error.
        reason: String,
    },
    /// The payload decoded, but its own type tag disagrees with the
    /// envelope's `message_type` — cross-type confusion within the control
    /// plane is malformed.
    TypeMismatch {
        /// The message_type tag from the header.
        header_type: u8,
        /// The type tag the payload carried.
        payload_type: u8,
    },
}

impl std::fmt::Display for ControlPlaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "control-plane frame too short"),
            Self::BadMagic => write!(f, "missing control-plane magic prefix"),
            Self::Truncated => write!(f, "truncated control-plane frame"),
            Self::InvalidNodeId => write!(f, "invalid sender_node_id in control-plane frame"),
            Self::PayloadTooLarge { len, max } => {
                write!(f, "control-plane payload too large: {len} > {max}")
            }
            Self::MalformedPayload { message_type, reason } => {
                write!(f, "malformed control payload for type {message_type}: {reason}")
            }
            Self::TypeMismatch {
                header_type,
                payload_type,
            } => write!(
                f,
                "control-plane payload type mismatch: header {header_type} vs payload {payload_type}"
            ),
        }
    }
}

impl std::error::Error for ControlPlaneError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic test identity (same pattern as discovery_message.rs):
    /// a `SecretKey` seeded from a single byte produces a valid Ed25519
    /// public key.
    fn test_key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    fn sample_payloads() -> Vec<(ControlMessageType, ControlPayload)> {
        vec![
            (
                ControlMessageType::Hello,
                ControlPayload::Hello(HelloPayload {
                    app_protocol_version: 1,
                }),
            ),
            (
                ControlMessageType::Presence,
                ControlPayload::Presence(PresencePayload {
                    ttl_secs: Some(120),
                    coarse: None,
                }),
            ),
            (
                ControlMessageType::Presence,
                ControlPayload::Presence(PresencePayload {
                    ttl_secs: None,
                    coarse: None,
                }),
            ),
            (
                ControlMessageType::Capabilities,
                ControlPayload::Capabilities(CapabilitiesPayload {
                    capabilities: vec!["files-v2".into(), "tunnels-v1".into()],
                }),
            ),
            (
                ControlMessageType::DiagnosticHint,
                ControlPayload::DiagnosticHint(DiagnosticHintPayload {
                    hint_code: 1,
                    note: Some("relay-only path".into()),
                }),
            ),
            (
                ControlMessageType::DiagnosticHint,
                ControlPayload::DiagnosticHint(DiagnosticHintPayload {
                    hint_code: 2,
                    note: None,
                }),
            ),
            (
                ControlMessageType::Extensions,
                ControlPayload::Extensions(crate::control_plane::extensions::ExtensionsPayload {
                    file: Some(crate::control_plane::extensions::FileReadiness {
                        protocol_versions: vec!["v2".into()],
                        can_receive: true,
                    }),
                    ..Default::default()
                }),
            ),
            (
                ControlMessageType::PublicRoomAdvertisement,
                ControlPayload::PublicRoomAdvertisement(test_advert()),
            ),
        ]
    }

    #[test]
    fn presence_coarse_metadata_roundtrips_without_addresses() {
        let payload = PresencePayload {
            ttl_secs: Some(300),
            coarse: Some(CoarsePresence {
                country_code: Some("AU".into()),
                latitude: Some(-33.86),
                longitude: Some(151.21),
                asn: Some(1221),
            }),
        };
        let encoded = postcard::to_stdvec(&payload).unwrap();
        let decoded: PresencePayload = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(decoded, payload);
        assert!(payload.coarse.as_ref().unwrap().is_valid());
    }

    #[test]
    fn presence_legacy_payload_without_coarse_metadata_decodes() {
        let legacy = postcard::to_stdvec(&Some(300u32)).unwrap();
        let decoded: PresencePayload = postcard::from_bytes(&legacy).unwrap();
        assert_eq!(decoded.ttl_secs, Some(300));
        assert_eq!(decoded.coarse, None);
    }

    #[test]
    fn malformed_coarse_metadata_is_sanitized_not_presence_failure() {
        let malformed = CoarsePresence {
            country_code: Some("Australia".into()),
            latitude: Some(f64::NAN),
            longitude: Some(200.0),
            asn: Some(64500),
        };
        assert!(!malformed.is_valid());
        let sanitized = malformed.sanitized().expect("valid ASN remains");
        assert_eq!(sanitized.country_code, None);
        assert_eq!(sanitized.latitude, None);
        assert_eq!(sanitized.longitude, None);
        assert_eq!(sanitized.asn, Some(64500));
    }

    #[test]
    fn coarse_presence_rejects_invalid_country_and_coordinate_ranges() {
        for coarse in [
            CoarsePresence {
                country_code: Some("aU".into()),
                latitude: None,
                longitude: None,
                asn: None,
            },
            CoarsePresence {
                country_code: Some("AUS".into()),
                latitude: None,
                longitude: None,
                asn: None,
            },
            CoarsePresence {
                country_code: None,
                latitude: Some(91.0),
                longitude: Some(0.0),
                asn: None,
            },
            CoarsePresence {
                country_code: None,
                latitude: Some(0.0),
                longitude: Some(-181.0),
                asn: None,
            },
        ] {
            assert!(!coarse.is_valid());
        }
    }

    /// A valid, bounded, discoverable room advertisement for tests.
    fn test_advert() -> crate::control_plane::advertisement::PublicRoomAdvertisement {
        crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x51; 32]),
            "Test Room".into(),
            {
                let mut seed = [0u8; 32];
                seed[0] = 0x52;
                iroh_base::SecretKey::from_bytes(&seed)
                    .public()
                    .as_bytes()
                    .to_owned()
            },
        )
    }

    // ── Round-trips for every control message type ────────────────────

    #[test]
    fn roundtrip_every_control_message_type() {
        let node = test_key(0xAA);
        for (message_type, payload) in sample_payloads() {
            let envelope = ControlEnvelope::new(node, 7, 1_700_000_000, payload);
            assert_eq!(envelope.message_type, message_type);
            let bytes = envelope.encode();
            match ControlEnvelope::decode(&bytes).expect("valid envelope decodes") {
                ControlPlaneDecode::Message(decoded) => {
                    assert_eq!(decoded, envelope);
                    assert_eq!(decoded.dedup_key(), (node, 7));
                }
                other => panic!("expected Message, got {other:?}"),
            }
        }
    }

    // ── BORU-CP-17 envelope signatures ────────────────────────────────

    /// A signed envelope round-trips and the receiver can cryptographically
    /// attribute it to `sender_node_id` (relayed-delivery fix).
    #[test]
    fn signed_envelope_roundtrips_with_valid_signature() {
        let sk = iroh_base::SecretKey::generate();
        let node = sk.public();
        let mut envelope =
            ControlEnvelope::capabilities(node, 9, 1_700_000_000, vec!["files-v2".into()]);
        assert!(envelope.signature.is_none());
        envelope.sign(&sk);
        assert!(envelope.signature.is_some());

        let bytes = envelope.encode();
        match ControlEnvelope::decode(&bytes).expect("signed envelope decodes") {
            ControlPlaneDecode::Message(decoded) => {
                assert_eq!(decoded, envelope);
                assert!(decoded.signature.is_some(), "signature survives transport");
                assert!(decoded.signature_valid, "signature must verify");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// Tampering with any signed field must invalidate the signature — a
    /// relayed/spoofed envelope claiming a sender it cannot prove.
    #[test]
    fn tampered_signed_envelope_fails_verification() {
        let sk = iroh_base::SecretKey::generate();
        let node = sk.public();
        let mut envelope =
            ControlEnvelope::capabilities(node, 9, 1_700_000_000, vec!["files-v2".into()]);
        envelope.sign(&sk);

        let mut bytes = envelope.encode();
        // Flip a byte INSIDE the payload's string content (keeps postcard
        // parsing valid — the string length prefix is untouched) so the
        // frame decodes but the signature no longer verifies.
        let payload_start = 3 + postcard::to_stdvec(&WireHeader {
            message_type: envelope.message_type.to_u8(),
            sender_node_id: *node.as_bytes(),
            sequence: envelope.sequence,
            timestamp_secs: envelope.timestamp_secs,
            payload_len: bytes.len() as u32,
        })
        .unwrap()
        .len();
        // payload layout: variant tag(1) | len(1) | "files-v2"(8); flip a
        // content byte (index 3 = 'l') to 'm' — stays valid UTF-8 so the
        // frame decodes, but the signature no longer verifies.
        bytes[payload_start + 3] ^= 0x01;

        match ControlEnvelope::decode(&bytes).expect("frame still parses") {
            ControlPlaneDecode::Message(decoded) => {
                assert!(
                    decoded.signature.is_some(),
                    "signature bytes survive transport"
                );
                assert!(
                    !decoded.signature_valid,
                    "tampered payload must fail signature verification"
                );
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// A signature made by a DIFFERENT key must not verify against the
    /// claimed sender (spoofing with an unrelated key).
    #[test]
    fn signature_from_wrong_key_fails_verification() {
        let real_sk = iroh_base::SecretKey::generate();
        let real_node = real_sk.public();
        let attacker_sk = iroh_base::SecretKey::generate();

        // Attacker builds an envelope claiming to be `real_node` but signs
        // with their own key.
        let mut envelope = ControlEnvelope::presence(real_node, 1, 1_700_000_000, None);
        envelope.sign(&attacker_sk);
        let bytes = envelope.encode();

        match ControlEnvelope::decode(&bytes).expect("frame parses") {
            ControlPlaneDecode::Message(decoded) => {
                assert_eq!(decoded.sender_node_id, real_node);
                assert!(
                    !decoded.signature_valid,
                    "signature from wrong key must not verify"
                );
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn convenience_constructors_roundtrip() {
        let node = test_key(0xBB);
        let envelopes = [
            ControlEnvelope::hello(node, 1, 1_700_000_000, 3),
            ControlEnvelope::presence(node, 2, 1_700_000_000, Some(60)),
            ControlEnvelope::capabilities(
                node,
                3,
                1_700_000_000,
                vec!["voice-v1".into(), "video-v1".into()],
            ),
            ControlEnvelope::diagnostic_hint(node, 4, 1_700_000_000, 7, None),
            ControlEnvelope::extensions(
                node,
                5,
                1_700_000_000,
                crate::control_plane::extensions::ExtensionsPayload {
                    tunnel: Some(crate::control_plane::extensions::TunnelCapability {
                        protocol_versions: vec!["v1".into()],
                    }),
                    ..Default::default()
                },
            ),
            ControlEnvelope::public_room_advertisement(node, 6, 1_700_000_000, test_advert()),
        ];
        for envelope in envelopes {
            let bytes = envelope.encode();
            let decoded = ControlEnvelope::decode(&bytes).expect("valid envelope decodes");
            match decoded {
                ControlPlaneDecode::Message(decoded) => assert_eq!(decoded, envelope),
                other => panic!("expected Message, got {other:?}"),
            }
        }
    }

    /// Wire is compact: a minimal Hello is well under 64 bytes.
    #[test]
    fn wire_is_compact() {
        let node = test_key(0xCC);
        let envelope = ControlEnvelope::hello(node, 1, 1_700_000_000, 1);
        let bytes = envelope.encode();
        assert!(
            bytes.len() < 64,
            "Hello envelope should be compact, got {} bytes",
            bytes.len()
        );
        assert_eq!(&bytes[..2], &CONTROL_PLANE_MAGIC);
        assert_eq!(bytes[2], CONTROL_PLANE_PROTOCOL_VERSION);
        assert_eq!(bytes[3], ControlMessageType::Hello.to_u8());
    }

    #[test]
    fn message_type_tags_are_stable() {
        assert_eq!(ControlMessageType::Hello.to_u8(), 0);
        assert_eq!(ControlMessageType::Presence.to_u8(), 1);
        assert_eq!(ControlMessageType::Capabilities.to_u8(), 2);
        assert_eq!(ControlMessageType::DiagnosticHint.to_u8(), 3);
        assert_eq!(ControlMessageType::Extensions.to_u8(), 4);
        assert_eq!(ControlMessageType::PublicRoomAdvertisement.to_u8(), 5);
        // BORU-DIR-09 (PDF Task 3.3): PUBLIC_ROOM_WITHDRAWAL took the next
        // stable tag — never renumber existing tags.
        assert_eq!(ControlMessageType::PublicRoomWithdrawal.to_u8(), 6);
        assert_eq!(
            ControlMessageType::from_u8(0),
            Some(ControlMessageType::Hello)
        );
        assert_eq!(
            ControlMessageType::from_u8(3),
            Some(ControlMessageType::DiagnosticHint)
        );
        assert_eq!(
            ControlMessageType::from_u8(4),
            Some(ControlMessageType::Extensions)
        );
        assert_eq!(
            ControlMessageType::from_u8(5),
            Some(ControlMessageType::PublicRoomAdvertisement)
        );
        assert_eq!(
            ControlMessageType::from_u8(6),
            Some(ControlMessageType::PublicRoomWithdrawal)
        );
        assert_eq!(ControlMessageType::from_u8(7), None);
        assert_eq!(ControlMessageType::from_u8(255), None);
    }

    // ── Forward compatibility: unknown types and fields ───────────────

    /// Unknown (future) message_type: header parses, payload skipped, no
    /// crash.
    #[test]
    fn unknown_message_type_is_ignored_safely() {
        let node = test_key(0xDD);
        let envelope = ControlEnvelope::hello(node, 9, 1_700_000_000, 1);
        let mut bytes = envelope.encode();
        // Rewrite the message_type byte (offset 3) to an unknown tag and
        // replace the payload with garbage — it must still be skipped safely.
        bytes[3] = 0x7F;
        match ControlEnvelope::decode(&bytes).expect("unknown type is not malformed") {
            ControlPlaneDecode::UnknownType {
                protocol_version,
                message_type,
                sender_node_id,
                sequence,
                timestamp_secs,
            } => {
                assert_eq!(protocol_version, CONTROL_PLANE_PROTOCOL_VERSION);
                assert_eq!(message_type, 0x7F);
                assert_eq!(sender_node_id, node);
                assert_eq!(sequence, 9);
                assert_eq!(timestamp_secs, 1_700_000_000);
            }
            other => panic!("expected UnknownType, got {other:?}"),
        }
    }

    /// Unsupported protocol version: fail closed for that feature, no crash.
    #[test]
    fn unsupported_protocol_version_fails_closed() {
        let node = test_key(0xEE);
        let envelope = ControlEnvelope::hello(node, 1, 1_700_000_000, 1);
        let mut bytes = envelope.encode();
        bytes[2] = CONTROL_PLANE_PROTOCOL_VERSION + 1;
        match ControlEnvelope::decode(&bytes).expect("unsupported version is not malformed") {
            ControlPlaneDecode::UnsupportedVersion { found, expected } => {
                assert_eq!(found, CONTROL_PLANE_PROTOCOL_VERSION + 1);
                assert_eq!(expected, CONTROL_PLANE_PROTOCOL_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    /// Unknown fields appended inside the payload section (simulating a
    /// newer sender) are ignored: build a valid Hello frame with two extra
    /// bytes after the known payload fields.
    #[test]
    fn unknown_payload_fields_are_ignored() {
        let node = test_key(0x11);
        // Encode the payload by itself so we can compute its exact length.
        let payload = postcard::to_stdvec(&ControlPayload::Hello(HelloPayload {
            app_protocol_version: 2,
        }))
        .unwrap();
        let header = postcard::to_stdvec(&WireHeader {
            message_type: ControlMessageType::Hello.to_u8(),
            sender_node_id: *node.as_bytes(),
            sequence: 5,
            timestamp_secs: 1_700_000_000,
            payload_len: payload.len() as u32 + 2, // +2 fake field bytes
        })
        .unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&CONTROL_PLANE_MAGIC);
        frame.push(CONTROL_PLANE_PROTOCOL_VERSION);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(&[0x01, 0x2A]); // future appended field
        match ControlEnvelope::decode(&frame).expect("extra fields are ignored") {
            ControlPlaneDecode::Message(decoded) => {
                assert_eq!(
                    decoded,
                    ControlEnvelope::hello(node, 5, 1_700_000_000, 2),
                    "future fields must not change decoding"
                );
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// Trailing bytes after the declared payload (future envelope
    /// extensions) are ignored.
    #[test]
    fn trailing_envelope_bytes_are_ignored() {
        let node = test_key(0x22);
        let envelope = ControlEnvelope::presence(node, 1, 1_700_000_000, None);
        let mut bytes = envelope.encode();
        bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        match ControlEnvelope::decode(&bytes).expect("trailing bytes are ignored") {
            ControlPlaneDecode::Message(decoded) => assert_eq!(decoded, envelope),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    // ── Strict decoder: malformed input rejection ─────────────────────

    #[test]
    fn rejects_bad_magic() {
        assert_eq!(
            ControlEnvelope::decode(b"not-a-control-frame").unwrap_err(),
            ControlPlaneError::BadMagic
        );
        // Chat-like postcard traffic (variant tag 0x00) is not control plane.
        assert_eq!(
            ControlEnvelope::decode(&[0x00, 0x01, 0x02, 0x03]).unwrap_err(),
            ControlPlaneError::BadMagic
        );
    }

    #[test]
    fn rejects_too_short() {
        assert_eq!(
            ControlEnvelope::decode(b"").unwrap_err(),
            ControlPlaneError::TooShort
        );
        assert_eq!(
            ControlEnvelope::decode(b"B").unwrap_err(),
            ControlPlaneError::TooShort
        );
        // Magic + version but truncated header.
        assert_eq!(
            ControlEnvelope::decode(&[0x42, 0x43, CONTROL_PLANE_PROTOCOL_VERSION]).unwrap_err(),
            ControlPlaneError::Truncated
        );
    }

    #[test]
    fn rejects_truncated_payload() {
        let node = test_key(0x33);
        // Claim a payload longer than the frame actually carries.
        let header = postcard::to_stdvec(&WireHeader {
            message_type: ControlMessageType::Hello.to_u8(),
            sender_node_id: *node.as_bytes(),
            sequence: 1,
            timestamp_secs: 1_700_000_000,
            payload_len: 4096,
        })
        .unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&CONTROL_PLANE_MAGIC);
        frame.push(CONTROL_PLANE_PROTOCOL_VERSION);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&[0x00, 0x01]); // not 4096 bytes
        assert_eq!(
            ControlEnvelope::decode(&frame).unwrap_err(),
            ControlPlaneError::Truncated
        );
    }

    #[test]
    fn rejects_oversized_payload() {
        let node = test_key(0x44);
        let header = postcard::to_stdvec(&WireHeader {
            message_type: ControlMessageType::Hello.to_u8(),
            sender_node_id: *node.as_bytes(),
            sequence: 1,
            timestamp_secs: 1_700_000_000,
            payload_len: MAX_CONTROL_PAYLOAD_LEN + 1,
        })
        .unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&CONTROL_PLANE_MAGIC);
        frame.push(CONTROL_PLANE_PROTOCOL_VERSION);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&vec![0u8; MAX_CONTROL_PAYLOAD_LEN as usize + 1]);
        assert_eq!(
            ControlEnvelope::decode(&frame).unwrap_err(),
            ControlPlaneError::PayloadTooLarge {
                len: MAX_CONTROL_PAYLOAD_LEN + 1,
                max: MAX_CONTROL_PAYLOAD_LEN,
            }
        );
    }

    #[test]
    fn rejects_malformed_payload() {
        let node = test_key(0x55);
        let header = postcard::to_stdvec(&WireHeader {
            message_type: ControlMessageType::Hello.to_u8(),
            sender_node_id: *node.as_bytes(),
            sequence: 1,
            timestamp_secs: 1_700_000_000,
            payload_len: 3,
        })
        .unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&CONTROL_PLANE_MAGIC);
        frame.push(CONTROL_PLANE_PROTOCOL_VERSION);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // garbage payload
        let err = ControlEnvelope::decode(&frame).unwrap_err();
        assert!(
            matches!(err, ControlPlaneError::MalformedPayload { .. }),
            "expected MalformedPayload, got {err:?}"
        );
    }

    #[test]
    fn rejects_invalid_node_id() {
        // 32 bytes of 0x02 are NOT a valid ed25519 compressed point
        // (verified against the resolved curve25519-dalek decompressor).
        assert!(PublicKey::from_bytes(&[0x02; 32]).is_err());
        let header = postcard::to_stdvec(&WireHeader {
            message_type: ControlMessageType::Hello.to_u8(),
            sender_node_id: [0x02; 32],
            sequence: 1,
            timestamp_secs: 1_700_000_000,
            payload_len: 2,
        })
        .unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&CONTROL_PLANE_MAGIC);
        frame.push(CONTROL_PLANE_PROTOCOL_VERSION);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&[0x00, 0x01]); // Hello payload tag + version
        assert_eq!(
            ControlEnvelope::decode(&frame).unwrap_err(),
            ControlPlaneError::InvalidNodeId
        );
    }

    /// Cross-type confusion within the control plane: a HELLO header with a
    /// PRESENCE payload is malformed.
    #[test]
    fn rejects_header_payload_type_mismatch() {
        let node = test_key(0x66);
        let presence = ControlEnvelope::presence(node, 1, 1_700_000_000, None);
        let pbytes = presence.encode();
        // Rewrite the message_type byte to Hello while keeping the Presence
        // payload intact.
        let mut bytes = pbytes;
        bytes[3] = ControlMessageType::Hello.to_u8();
        let err = ControlEnvelope::decode(&bytes).unwrap_err();
        assert!(
            matches!(err, ControlPlaneError::TypeMismatch { .. }),
            "expected TypeMismatch, got {err:?}"
        );
    }

    // ── Separation from chat / existing discovery wire types ──────────

    /// A control-plane byte stream must never decode as the BORU-DISC
    /// discovery message type (variant tags 0..=2; magic 0x42 is not a valid
    /// tag).
    #[test]
    fn control_envelope_never_decodes_as_legacy_discovery_message() {
        use crate::discovery_message::DiscoveryMessage;
        let node = test_key(0x77);
        let envelope = ControlEnvelope::hello(node, 1, 1_700_000_000, 1);
        let bytes = envelope.encode();
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&bytes).is_err(),
            "control envelope bytes must never decode as DiscoveryMessage"
        );
    }

    /// A BORU-DISC discovery byte stream must never pass the control-plane
    /// decoder (no magic prefix).
    #[test]
    fn legacy_discovery_message_never_decodes_as_control_envelope() {
        use crate::discovery_message::DiscoveryMessage;
        let node = test_key(0x88);
        let dbytes = postcard::to_stdvec(&DiscoveryMessage::hello(node)).unwrap();
        assert_eq!(
            ControlEnvelope::decode(&dbytes).unwrap_err(),
            ControlPlaneError::BadMagic
        );
    }

    /// A control-plane byte stream must never deserialise as a normal Boru
    /// chat message (the chat Message enum's variant tags are 0..=19; the
    /// first byte of a control envelope is always 0x42, an invalid tag).
    #[cfg(feature = "net")]
    #[test]
    fn control_envelope_never_decodes_as_chat_message() {
        use crate::chat_core::Message as ChatMessage;
        let node = test_key(0x99);
        let envelopes = [
            ControlEnvelope::hello(node, 1, 1_700_000_000, 1),
            ControlEnvelope::presence(node, 2, 1_700_000_000, None),
            ControlEnvelope::capabilities(node, 3, 1_700_000_000, vec!["files-v2".into()]),
            ControlEnvelope::diagnostic_hint(node, 4, 1_700_000_000, 1, None),
            ControlEnvelope::extensions(
                node,
                5,
                1_700_000_000,
                crate::control_plane::extensions::ExtensionsPayload {
                    file: Some(crate::control_plane::extensions::FileReadiness {
                        protocol_versions: vec!["v2".into()],
                        can_receive: true,
                    }),
                    ..Default::default()
                },
            ),
            ControlEnvelope::public_room_advertisement(node, 6, 1_700_000_000, test_advert()),
        ];
        for envelope in envelopes {
            let bytes = envelope.encode();
            assert!(
                postcard::from_bytes::<ChatMessage>(&bytes).is_err(),
                "control envelope bytes must never decode as a chat message"
            );
        }
    }

    /// A normal Boru chat message byte stream must never pass the
    /// control-plane decoder.
    #[cfg(feature = "net")]
    #[test]
    fn chat_message_never_decodes_as_control_envelope() {
        use crate::chat_core::Message as ChatMessage;
        let chat_messages = [
            ChatMessage::Presence,
            ChatMessage::Message {
                text: "hello".into(),
            },
            ChatMessage::AboutMe {
                name: "alice".into(),
                profile_image_ticket: None,
            },
        ];
        for chat in chat_messages {
            let bytes = postcard::to_stdvec(&chat).unwrap();
            // Every chat byte stream must be rejected. Multi-byte chat
            // messages fail the magic check; the unit `Presence` variant
            // encodes to a single byte and fails the 2-byte minimum. Either
            // way the frame never decodes as a control envelope.
            match ControlEnvelope::decode(&bytes) {
                Err(ControlPlaneError::BadMagic) => {}
                Err(ControlPlaneError::TooShort) => {}
                other => {
                    panic!("chat bytes must never decode as a control envelope, got {other:?}")
                }
            }
        }
    }

    // ── BORU-DIR-01: PUBLIC_ROOM_ADVERTISEMENT separation ─────────────

    /// A PUBLIC_ROOM_ADVERTISEMENT envelope decodes as a room advertisement —
    /// never as a PRESENCE payload and never as a chat message. The typed
    /// payload is versioned and carries only room-discovery metadata.
    #[cfg(feature = "net")]
    #[test]
    fn advertisement_never_decodes_as_presence_or_chat() {
        use crate::chat_core::Message as ChatMessage;
        let node = test_key(0x12);
        let envelope =
            ControlEnvelope::public_room_advertisement(node, 1, 1_700_000_000, test_advert());
        let bytes = envelope.encode();

        // Decodes as a room advertisement with the version anchor intact.
        match ControlEnvelope::decode(&bytes).expect("advertisement decodes") {
            ControlPlaneDecode::Message(decoded) => {
                assert_eq!(
                    decoded.message_type,
                    ControlMessageType::PublicRoomAdvertisement
                );
                match decoded.payload {
                    ControlPayload::PublicRoomAdvertisement(payload) => {
                        assert_eq!(payload.advert_version, 1);
                    }
                    other => panic!(
                        "advertisement must decode as PublicRoomAdvertisement, got {other:?}"
                    ),
                }
            }
            other => panic!("expected Message, got {other:?}"),
        }

        // The wire bytes never deserialise as a chat message.
        assert!(
            postcard::from_bytes::<ChatMessage>(&bytes).is_err(),
            "advertisement bytes must never decode as a chat message"
        );

        // A chat PRESENCE payload never decodes as a room advertisement.
        let presence_chat = postcard::to_stdvec(&ChatMessage::Presence).unwrap();
        assert!(
            postcard::from_bytes::<ControlPayload>(&presence_chat).is_err(),
            "chat presence bytes must never decode as a control payload"
        );

        // A control-plane PRESENCE envelope's payload is a Presence payload,
        // never a PublicRoomAdvertisement (the cross-type guard enforces it).
        let presence = ControlEnvelope::presence(node, 2, 1_700_000_000, None);
        match ControlEnvelope::decode(&presence.encode()).expect("presence decodes") {
            ControlPlaneDecode::Message(decoded) => match decoded.payload {
                ControlPayload::Presence(_) => {}
                other => panic!("presence must decode as Presence, got {other:?}"),
            },
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// Malformed room advertisements are rejected safely — never a panic,
    /// never a misinterpretation. Garbage payload, truncated frame, and
    /// header/payload type mismatch all return a structured error.
    #[test]
    fn malformed_advertisement_rejected_safely() {
        let node = test_key(0x13);

        // Header says PUBLIC_ROOM_ADVERTISEMENT but the payload section is
        // garbage — MalformedPayload.
        let header = postcard::to_stdvec(&WireHeader {
            message_type: ControlMessageType::PublicRoomAdvertisement.to_u8(),
            sender_node_id: *node.as_bytes(),
            sequence: 1,
            timestamp_secs: 1_700_000_000,
            payload_len: 3,
        })
        .unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&CONTROL_PLANE_MAGIC);
        frame.push(CONTROL_PLANE_PROTOCOL_VERSION);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // garbage payload
        let err = ControlEnvelope::decode(&frame).unwrap_err();
        assert!(
            matches!(err, ControlPlaneError::MalformedPayload { .. }),
            "expected MalformedPayload, got {err:?}"
        );

        // Header says PUBLIC_ROOM_ADVERTISEMENT but the payload is a PRESENCE
        // payload — cross-type confusion is malformed (TypeMismatch).
        let presence = ControlEnvelope::presence(node, 1, 1_700_000_000, None);
        let mut mismatched = presence.encode();
        mismatched[3] = ControlMessageType::PublicRoomAdvertisement.to_u8();
        let err = ControlEnvelope::decode(&mismatched).unwrap_err();
        assert!(
            matches!(err, ControlPlaneError::TypeMismatch { .. }),
            "expected TypeMismatch, got {err:?}"
        );

        // Truncated advertisement frame — TooShort/Truncated, not a panic.
        let full =
            ControlEnvelope::public_room_advertisement(node, 1, 1_700_000_000, test_advert())
                .encode();
        for cut in 0..full.len() {
            let err = ControlEnvelope::decode(&full[..cut]).unwrap_err();
            assert!(
                matches!(
                    err,
                    ControlPlaneError::TooShort | ControlPlaneError::Truncated
                ),
                "truncated advertisement (cut={cut}) must be TooShort/Truncated, got {err:?}"
            );
        }
    }

    /// Unknown future advertisement fields are ignored safely: a newer
    /// sender appends metadata fields after the current payload; an older
    /// client decodes the known prefix and discards the trailing bytes.
    #[test]
    fn unknown_future_advertisement_fields_tolerated() {
        let node = test_key(0x14);
        let payload =
            postcard::to_stdvec(&ControlPayload::PublicRoomAdvertisement(test_advert())).unwrap();
        let header = postcard::to_stdvec(&WireHeader {
            message_type: ControlMessageType::PublicRoomAdvertisement.to_u8(),
            sender_node_id: *node.as_bytes(),
            sequence: 5,
            timestamp_secs: 1_700_000_000,
            payload_len: payload.len() as u32 + 4, // +4 fake future fields
        })
        .unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&CONTROL_PLANE_MAGIC);
        frame.push(CONTROL_PLANE_PROTOCOL_VERSION);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(&[0x01, 0x00, 0x2A, 0x7F]); // future fields

        match ControlEnvelope::decode(&frame).expect("future fields are ignored") {
            ControlPlaneDecode::Message(decoded) => {
                assert_eq!(
                    decoded,
                    ControlEnvelope::public_room_advertisement(
                        node,
                        5,
                        1_700_000_000,
                        test_advert()
                    ),
                    "future fields must not change decoding"
                );
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }
}
