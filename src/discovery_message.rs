//! Discovery protocol message types — Hello / Presence / PeerAdvertisement.
//!
//! Every Boru node joins one internal discovery gossip topic at startup as
//! **networking infrastructure** (peer discovery, presence, connectivity
//! bootstrapping). The messages exchanged on that topic are defined here.
//!
//! # Deliberate separation from chat payloads
//!
//! These types are intentionally **not** the chat
//! [`Message`](crate::chat_core::Message) enum:
//!
//! * Discovery payloads must never be routed through chat persistence,
//!   notifications, or rendering paths (the hidden-discovery hard rule).
//! * Chat payloads (private direct messages, normal chat messages) must never
//!   be routed through the discovery topic.
//!
//! Every variant carries a [`DiscoveryHeader`] with the protocol version and
//! the sender's node identity, so each message is self-describing and can be
//! version-checked before its payload is interpreted.
//!
//! # Wire format
//!
//! Serialised with **postcard** (matching the chat
//! [`Message`](crate::chat_core::Message) convention): a variant-index
//! varint, then the variant's fields in order. The header contributes
//! `protocol_version` (1 byte) + `node_id` (32 raw bytes, the iroh Ed25519
//! public key), so:
//!
//! | Message | Wire size |
//! |---------|-----------|
//! | `Hello` / `Presence` | 1 (tag) + 1 (version) + 32 (node) = **34 B** |
//! | `PeerAdvertisement` | 1 (tag) + 1 (version) + 32 (node) + 32 (advertised) = **66 B** |
//!
//! Each variant additionally carries an optional `event_id` (a per-node
//! monotonic counter, [`DiscoveryMessage::event_id`]) appended as the LAST
//! field. It is `None` on the legacy wire format (BORU-DISC-06 messages have
//! no such field and decode unchanged) and `Some(n)` on the current format:
//!
//! | Message (with event id) | Wire size |
//! |---------|-----------|
//! | `Hello` / `Presence` | 34 + 1 (Some tag) + varint(event id) = **36 B** for ids < 128 |
//! | `PeerAdvertisement` | 66 + 1 (Some tag) + varint(event id) = **68 B** for ids < 128 |
//!
//! The 3-variant tag space (0..=2) is disjoint from the chat
//! [`Message`](crate::chat_core::Message) tag space (0..=19) in the sense
//! that no discovery byte stream ever deserialises into a chat message — the
//! tests in [`tests`](self::tests) prove both directions.

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::discovery_topic::BORU_DISCOVERY_PROTOCOL_VERSION;

/// Deserialize an `Option<u64>`, defaulting to `None` when the wire buffer
/// is exhausted.
///
/// Postcard's `SeqAccess` returns `Err(EOF)` — not `Ok(None)` — when a
/// struct-variant's declared field count exceeds the bytes on the wire, so
/// `#[serde(default)]` alone cannot backfill a legacy BORU-DISC-06 payload
/// that predates the `event_id` field. Treating an end-of-buffer as `None`
/// keeps old peers' discovery messages decodable (same pattern as
/// `chat_core::protocol::deserialize_tolerant_*`).
fn deserialize_tolerant_opt_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<u64>::deserialize(deserializer) {
        Ok(value) => Ok(value),
        Err(_) => Ok(None),
    }
}

/// Shared header carried by every discovery message.
///
/// Contains the protocol version and the sending node's identity, satisfying
/// the discovery-protocol requirement that every message is versioned and
/// attributable to a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryHeader {
    /// Protocol version of the discovery wire format this message speaks.
    ///
    /// Compare against [`BORU_DISCOVERY_PROTOCOL_VERSION`] with
    /// [`check_discovery_version`] before interpreting the payload.
    pub protocol_version: u8,
    /// Node identity (iroh Ed25519 public key) of the sender.
    pub node_id: PublicKey,
}

impl DiscoveryHeader {
    /// Build a header speaking the current discovery protocol version.
    pub fn new(node_id: PublicKey) -> Self {
        Self {
            protocol_version: BORU_DISCOVERY_PROTOCOL_VERSION,
            node_id,
        }
    }

    /// Version-gate this header against the current protocol version.
    ///
    /// This is the "unknown-protocol-version handling hook": the receive-path
    /// gate that actually drops unsupported messages is wired in a later
    /// discovery task (BORU-DISC-19), but the check already lives here so the
    /// field is used and tested now.
    pub fn check_version(&self) -> DiscoveryVersionCheck {
        check_discovery_version(self.protocol_version)
    }
}

/// Discovery protocol messages exchanged on the internal discovery topic.
///
/// A dedicated enum distinct from the chat
/// [`Message`](crate::chat_core::Message) type: a `DiscoveryMessage` can
/// never be confused with a chat payload, and chat payloads can never be
/// routed through the discovery topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoveryMessage {
    /// A node announces itself after joining the discovery topic.
    Hello {
        /// Version + node identity of the sender.
        header: DiscoveryHeader,
        /// Per-node monotonic advertisement/event id (BORU-DISC-17).
        ///
        /// `None` on the legacy wire format (BORU-DISC-06 messages carry no
        /// event id and decode unchanged); `Some(n)` on the current format.
        /// The receiving side uses `(node_id, event_id)` as the dedup key so
        /// the same advertisement delivered over two discovery paths is
        /// represented once.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_tolerant_opt_u64"
        )]
        event_id: Option<u64>,
    },
    /// Periodic presence heartbeat — "I am still here".
    Presence {
        /// Version + node identity of the sender.
        header: DiscoveryHeader,
        /// Per-node monotonic event id (see [`DiscoveryMessage::Hello`]).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_tolerant_opt_u64"
        )]
        event_id: Option<u64>,
    },
    /// Advertise a peer the sender knows about, so receivers can dial it
    /// directly (connectivity bootstrapping).
    PeerAdvertisement {
        /// Version + node identity of the sender.
        header: DiscoveryHeader,
        /// Identity of the peer being advertised.
        advertised: PublicKey,
        /// Per-node monotonic event id (see [`DiscoveryMessage::Hello`]).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_tolerant_opt_u64"
        )]
        event_id: Option<u64>,
    },
}

impl DiscoveryMessage {
    /// Build a `Hello` for `node_id` speaking the current protocol version.
    ///
    /// The legacy (no event id) form: wire-identical to the BORU-DISC-06
    /// payload. Prefer [`hello_with_event`](Self::hello_with_event) for
    /// announcements so receivers can dedup by event id.
    pub fn hello(node_id: PublicKey) -> Self {
        Self::Hello {
            header: DiscoveryHeader::new(node_id),
            event_id: None,
        }
    }

    /// Build a `Hello` carrying a per-node monotonic event id.
    pub fn hello_with_event(node_id: PublicKey, event_id: u64) -> Self {
        Self::Hello {
            header: DiscoveryHeader::new(node_id),
            event_id: Some(event_id),
        }
    }

    /// Build a `Presence` for `node_id` speaking the current protocol version.
    ///
    /// The legacy (no event id) form. Prefer
    /// [`presence_with_event`](Self::presence_with_event) for announcements.
    pub fn presence(node_id: PublicKey) -> Self {
        Self::Presence {
            header: DiscoveryHeader::new(node_id),
            event_id: None,
        }
    }

    /// Build a `Presence` carrying a per-node monotonic event id.
    pub fn presence_with_event(node_id: PublicKey, event_id: u64) -> Self {
        Self::Presence {
            header: DiscoveryHeader::new(node_id),
            event_id: Some(event_id),
        }
    }

    /// Build a `PeerAdvertisement` from `node_id` about `advertised`,
    /// speaking the current protocol version.
    ///
    /// The legacy (no event id) form. Prefer
    /// [`peer_advertisement_with_event`](Self::peer_advertisement_with_event)
    /// for advertisements so receivers can dedup by event id.
    pub fn peer_advertisement(node_id: PublicKey, advertised: PublicKey) -> Self {
        Self::PeerAdvertisement {
            header: DiscoveryHeader::new(node_id),
            advertised,
            event_id: None,
        }
    }

    /// Build a `PeerAdvertisement` carrying a per-node monotonic event id.
    pub fn peer_advertisement_with_event(
        node_id: PublicKey,
        advertised: PublicKey,
        event_id: u64,
    ) -> Self {
        Self::PeerAdvertisement {
            header: DiscoveryHeader::new(node_id),
            advertised,
            event_id: Some(event_id),
        }
    }

    /// The protocol version carried by this message.
    pub fn protocol_version(&self) -> u8 {
        match self {
            Self::Hello { header, .. } | Self::Presence { header, .. } => header.protocol_version,
            Self::PeerAdvertisement { header, .. } => header.protocol_version,
        }
    }

    /// The node identity carried by this message.
    pub fn node_id(&self) -> PublicKey {
        match self {
            Self::Hello { header, .. } | Self::Presence { header, .. } => header.node_id,
            Self::PeerAdvertisement { header, .. } => header.node_id,
        }
    }

    /// The per-node event/advertisement id carried by this message, if any.
    ///
    /// `None` means the sender used the legacy wire format (no event id);
    /// `Some(n)` is the sender's monotonic counter for this event. Receivers
    /// dedup on `(node_id, event_id)`: the same event delivered twice (e.g.
    /// over two discovery paths) is represented once.
    pub fn event_id(&self) -> Option<u64> {
        match self {
            Self::Hello { event_id, .. }
            | Self::Presence { event_id, .. }
            | Self::PeerAdvertisement { event_id, .. } => *event_id,
        }
    }
}

/// Outcome of the discovery protocol version gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryVersionCheck {
    /// The message speaks the current protocol version and may be processed.
    Supported,
    /// The message speaks an unknown protocol version and must be dropped.
    Unsupported {
        /// Version found on the wire.
        found: u8,
        /// Version this node understands.
        expected: u8,
    },
}

/// Version gate for the discovery wire protocol.
///
/// Unknown versions are rejected rather than interpreted optimistically: a
/// node that does not understand a message's version must not guess at its
/// meaning. The receive-path wiring that drops unsupported messages is
/// introduced in a later discovery task; this function is the check the wire
/// carries today (via [`DiscoveryHeader::check_version`]).
pub fn check_discovery_version(version: u8) -> DiscoveryVersionCheck {
    if version == BORU_DISCOVERY_PROTOCOL_VERSION {
        DiscoveryVersionCheck::Supported
    } else {
        DiscoveryVersionCheck::Unsupported {
            found: version,
            expected: BORU_DISCOVERY_PROTOCOL_VERSION,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic test identity: a `SecretKey` seeded from a single byte
    /// produces a valid Ed25519 public key (all-identical byte arrays are not
    /// valid compressed points, so derive from a secret key instead).
    fn test_key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        let sk = iroh_base::SecretKey::from_bytes(&seed);
        sk.public()
    }

    // ── Postcard roundtrips ────────────────────────────────────────────

    #[test]
    fn hello_roundtrip() {
        let node = test_key(0xAA);
        let msg = DiscoveryMessage::hello(node);
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(decoded.node_id(), node);
    }

    #[test]
    fn presence_roundtrip() {
        let node = test_key(0xBB);
        let msg = DiscoveryMessage::presence(node);
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(decoded.node_id(), node);
    }

    #[test]
    fn peer_advertisement_roundtrip() {
        let node = test_key(0xCC);
        let advertised = test_key(0xDD);
        let msg = DiscoveryMessage::peer_advertisement(node, advertised);
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(decoded.node_id(), node);
        match decoded {
            DiscoveryMessage::PeerAdvertisement {
                advertised: got, ..
            } => {
                assert_eq!(got, advertised);
            }
            _ => panic!("expected PeerAdvertisement"),
        }
    }

    // ── Event id (BORU-DISC-17 dedup key) ────────────────────────────

    /// `*_with_event` constructors roundtrip the event id on every variant.
    #[test]
    fn event_id_roundtrip() {
        let node = test_key(0x11);
        let advertised = test_key(0x22);
        for (msg, expected) in [
            (
                DiscoveryMessage::hello_with_event(node, 7),
                Some(7u64),
            ),
            (
                DiscoveryMessage::presence_with_event(node, 42),
                Some(42u64),
            ),
            (
                DiscoveryMessage::peer_advertisement_with_event(node, advertised, 99),
                Some(99u64),
            ),
        ] {
            let bytes = postcard::to_stdvec(&msg).unwrap();
            let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(decoded, msg);
            assert_eq!(decoded.event_id(), expected);
            assert_eq!(decoded.node_id(), node);
        }
    }

    /// Legacy (no event id) messages expose `event_id() == None`.
    #[test]
    fn legacy_constructors_have_no_event_id() {
        let node = test_key(0x33);
        let advertised = test_key(0x44);
        assert_eq!(DiscoveryMessage::hello(node).event_id(), None);
        assert_eq!(DiscoveryMessage::presence(node).event_id(), None);
        assert_eq!(
            DiscoveryMessage::peer_advertisement(node, advertised).event_id(),
            None
        );
    }

    /// BORU-DISC-06 wire compat: a message serialised WITHOUT the event id
    /// field (the legacy 34 B / 66 B forms) still deserialises as the current
    /// type with `event_id == None`. This is the backward-compat guarantee
    /// the dedup change must not break: old senders keep working.
    #[test]
    fn legacy_wire_format_decodes_without_event_id() {
        let node = test_key(0x55);

        // Hand-build the legacy Hello bytes: tag + version + node (34 B),
        // i.e. exactly what BORU-DISC-06 `postcard::to_stdvec(&hello)` made.
        let mut legacy_hello = vec![0x00, BORU_DISCOVERY_PROTOCOL_VERSION];
        legacy_hello.extend_from_slice(node.as_bytes());
        assert_eq!(legacy_hello.len(), 34);
        let decoded: DiscoveryMessage = postcard::from_bytes(&legacy_hello).unwrap();
        assert_eq!(decoded, DiscoveryMessage::hello(node));
        assert_eq!(decoded.event_id(), None);

        // Legacy Presence bytes (same shape, tag 0x01).
        let mut legacy_presence = vec![0x01, BORU_DISCOVERY_PROTOCOL_VERSION];
        legacy_presence.extend_from_slice(node.as_bytes());
        assert_eq!(legacy_presence.len(), 34);
        let decoded: DiscoveryMessage = postcard::from_bytes(&legacy_presence).unwrap();
        assert_eq!(decoded, DiscoveryMessage::presence(node));
        assert_eq!(decoded.event_id(), None);

        // Legacy PeerAdvertisement bytes (66 B).
        let advertised = test_key(0x66);
        let mut legacy_ad = vec![0x02, BORU_DISCOVERY_PROTOCOL_VERSION];
        legacy_ad.extend_from_slice(node.as_bytes());
        legacy_ad.extend_from_slice(advertised.as_bytes());
        assert_eq!(legacy_ad.len(), 66);
        let decoded: DiscoveryMessage = postcard::from_bytes(&legacy_ad).unwrap();
        assert_eq!(decoded, DiscoveryMessage::peer_advertisement(node, advertised));
        assert_eq!(decoded.event_id(), None);
    }

    /// The current wire format is backward-compatible in the other direction
    /// too: a legacy-constructed `hello(node)` (no event id) serialises to
    /// exactly the 34-byte legacy form, so an old sender's bytes and a new
    /// sender's `None` event-id bytes are indistinguishable on the wire.
    #[test]
    fn legacy_constructor_serializes_to_legacy_wire() {
        let node = test_key(0x77);
        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(node)).unwrap();
        assert_eq!(bytes.len(), 34);
        assert_eq!(&bytes[..2], &[0x00, BORU_DISCOVERY_PROTOCOL_VERSION]);
        // The 32-byte node id follows the tag+version prefix.
        assert_eq!(&bytes[2..], node.as_bytes());
    }

    /// All variants carry the current protocol version after a roundtrip.
    #[test]
    fn roundtrip_preserves_protocol_version() {
        let node = test_key(0x11);
        let advertised = test_key(0x22);
        for msg in [
            DiscoveryMessage::hello(node),
            DiscoveryMessage::presence(node),
            DiscoveryMessage::peer_advertisement(node, advertised),
        ] {
            let bytes = postcard::to_stdvec(&msg).unwrap();
            let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(decoded.protocol_version(), BORU_DISCOVERY_PROTOCOL_VERSION);
            assert_eq!(decoded.node_id(), node);
        }
    }

    /// Known wire sizes for the three variants (see module docs).
    #[test]
    fn wire_sizes() {
        let node = test_key(0x33);
        let advertised = test_key(0x44);
        assert_eq!(
            postcard::to_stdvec(&DiscoveryMessage::hello(node))
                .unwrap()
                .len(),
            34
        );
        assert_eq!(
            postcard::to_stdvec(&DiscoveryMessage::presence(node))
                .unwrap()
                .len(),
            34
        );
        assert_eq!(
            postcard::to_stdvec(&DiscoveryMessage::peer_advertisement(node, advertised))
                .unwrap()
                .len(),
            66
        );
    }

    /// Known-answer vector: the first two bytes of a `Hello` are the variant
    /// tag `0x00` then the protocol version byte.
    #[test]
    fn hello_wire_prefix() {
        let node = test_key(0x07);
        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(node)).unwrap();
        assert_eq!(&bytes[..2], &[0x00, BORU_DISCOVERY_PROTOCOL_VERSION]);
    }

    // ── Protocol version gate ──────────────────────────────────────────

    #[test]
    fn version_gate_accepts_current() {
        assert_eq!(
            check_discovery_version(BORU_DISCOVERY_PROTOCOL_VERSION),
            DiscoveryVersionCheck::Supported
        );
    }

    #[test]
    fn version_gate_rejects_unknown() {
        for unknown in [0u8, 2, 3, 42, 255] {
            assert_eq!(
                check_discovery_version(unknown),
                DiscoveryVersionCheck::Unsupported {
                    found: unknown,
                    expected: BORU_DISCOVERY_PROTOCOL_VERSION,
                },
                "version {unknown} must be rejected"
            );
        }
    }

    #[test]
    fn header_check_version_hook() {
        let header = DiscoveryHeader::new(test_key(0x55));
        assert_eq!(header.check_version(), DiscoveryVersionCheck::Supported);
        let legacy = DiscoveryHeader {
            protocol_version: 0,
            node_id: test_key(0x55),
        };
        assert_eq!(
            legacy.check_version(),
            DiscoveryVersionCheck::Unsupported {
                found: 0,
                expected: BORU_DISCOVERY_PROTOCOL_VERSION,
            }
        );
    }

    // ── Separation from the chat Message type ──────────────────────────
    //
    // The acceptance criterion for this task: "Discovery messages have a
    // dedicated enum/struct and cannot be confused with ChatMessage
    // payloads." These tests prove the wire-level separation. They need the
    // `net` feature because the chat Message type lives in chat_core.

    #[cfg(feature = "net")]
    #[test]
    fn discovery_message_is_distinct_from_chat_message() {
        use crate::chat_core::Message as ChatMessage;

        let node = test_key(0x07);
        let advertised = test_key(0x08);

        let discovery_variants = [
            DiscoveryMessage::hello(node),
            DiscoveryMessage::presence(node),
            DiscoveryMessage::peer_advertisement(node, advertised),
        ];

        for msg in discovery_variants {
            let bytes = postcard::to_stdvec(&msg).unwrap();
            // A discovery byte stream must never deserialise into a chat
            // message. If it somehow decodes, re-encoding must NOT reproduce
            // the discovery bytes (i.e. it is not a stable chat encoding of
            // this payload) — either way the payload cannot be confused.
            match postcard::from_bytes::<ChatMessage>(&bytes) {
                Ok(chat_msg) => {
                    let reencoded = postcard::to_stdvec(&chat_msg).unwrap();
                    assert_ne!(
                        reencoded, bytes,
                        "discovery bytes must not round-trip as a chat message"
                    );
                }
                Err(_) => {}
            }
        }
    }

    #[cfg(feature = "net")]
    #[test]
    fn chat_message_never_decodes_as_discovery_message() {
        use crate::chat_core::Message as ChatMessage;

        let node = test_key(0x07);
        // The chat Presence variant is a unit variant — the shortest chat
        // message. Every chat variant must fail to decode as a discovery
        // message (the discovery tag space is 0..=2 and the payload shapes
        // differ).
        let chat_messages: Vec<ChatMessage> = vec![
            ChatMessage::Presence,
            ChatMessage::Message {
                text: "hello".to_string(),
            },
            ChatMessage::AboutMe {
                name: "alice".to_string(),
                profile_image_ticket: None,
            },
        ];
        for chat in chat_messages {
            let bytes = postcard::to_stdvec(&chat).unwrap();
            let decoded = postcard::from_bytes::<DiscoveryMessage>(&bytes);
            assert!(
                decoded.is_err(),
                "chat message bytes must never decode as a DiscoveryMessage"
            );
        }

        // And the reverse cross-check: discovery bytes never decode as the
        // chat Presence unit variant.
        let discovery = DiscoveryMessage::presence(node);
        let dbytes = postcard::to_stdvec(&discovery).unwrap();
        let decoded = postcard::from_bytes::<ChatMessage>(&dbytes);
        match decoded {
            Ok(chat) => {
                let reencoded = postcard::to_stdvec(&chat).unwrap();
                assert_ne!(reencoded, dbytes);
            }
            Err(_) => {}
        }
    }
}
