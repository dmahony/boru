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

use iroh_base::PublicKey;
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

/// `message_type` enum for the control plane (PDF Task 1.1 step 3).
///
/// Tag values are stable wire constants: `0 = HELLO`, `1 = PRESENCE`,
/// `2 = CAPABILITIES`, `3 = DIAGNOSTIC_HINT`. Unknown tags are tolerated by
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

/// PRESENCE payload — "I am still here".
///
/// Metadata only: no usernames, profile text, or device details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresencePayload {
    /// Suggested TTL (seconds) before this presence should be considered
    /// stale. `None` = use the receiver's default TTL.
    #[serde(default)]
    pub ttl_secs: Option<u32>,
}

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

/// Typed payload carried by a [`ControlEnvelope`].
///
/// The payload enum is self-describing on the wire (postcard variant tag),
/// which lets [`ControlEnvelope::decode`] cross-check that the payload's own
/// type matches the envelope's `message_type` — a mismatch is malformed.
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
}

impl ControlPayload {
    /// The [`ControlMessageType`] this payload carries.
    pub fn message_type(&self) -> ControlMessageType {
        match self {
            Self::Hello(_) => ControlMessageType::Hello,
            Self::Presence(_) => ControlMessageType::Presence,
            Self::Capabilities(_) => ControlMessageType::Capabilities,
            Self::DiagnosticHint(_) => ControlMessageType::DiagnosticHint,
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
    /// Typed message kind (HELLO / PRESENCE / CAPABILITIES / DIAGNOSTIC_HINT).
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
        Self::new(
            sender_node_id,
            sequence,
            timestamp_secs,
            ControlPayload::Presence(PresencePayload { ttl_secs }),
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

    /// Dedup key for announcements: `(sender_node_id, sequence)`.
    ///
    /// Duplicate delivery of the same announcement (e.g. over two discovery
    /// paths) yields the same key; the caller represents the event once.
    pub fn dedup_key(&self) -> (PublicKey, u64) {
        (self.sender_node_id, self.sequence)
    }

    /// Serialise this envelope to its compact wire form.
    ///
    /// Cannot fail for in-memory envelopes: payload encoding is infallible
    /// for the supported types and the payload length is bounded by
    /// [`MAX_CONTROL_PAYLOAD_LEN`].
    pub fn encode(&self) -> Vec<u8> {
        let payload =
            postcard::to_stdvec(&self.payload).expect("control payload encoding is infallible");
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
        let mut out = Vec::with_capacity(3 + 40 + payload.len());
        out.extend_from_slice(&CONTROL_PLANE_MAGIC);
        out.push(self.protocol_version); // fixed offset bytes[2]
        out.extend_from_slice(
            &postcard::to_stdvec(&header).expect("header encoding is infallible"),
        );
        out.extend_from_slice(&payload);
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
        // Trailing bytes beyond payload_len (future envelope extensions)
        // are intentionally ignored (forward compatibility).

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

        Ok(ControlPlaneDecode::Message(ControlEnvelope {
            protocol_version,
            message_type,
            sender_node_id,
            sequence: header.sequence,
            timestamp_secs: header.timestamp_secs,
            payload,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct WireHeader {
    message_type: u8,
    sender_node_id: [u8; 32],
    sequence: u64,
    timestamp_secs: u64,
    payload_len: u32,
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
                }),
            ),
            (
                ControlMessageType::Presence,
                ControlPayload::Presence(PresencePayload { ttl_secs: None }),
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
        ]
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
        assert_eq!(
            ControlMessageType::from_u8(0),
            Some(ControlMessageType::Hello)
        );
        assert_eq!(
            ControlMessageType::from_u8(3),
            Some(ControlMessageType::DiagnosticHint)
        );
        assert_eq!(ControlMessageType::from_u8(4), None);
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
}
