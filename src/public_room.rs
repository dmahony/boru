//! Public room identity — deterministic network-aware room identifiers.
//!
//! A public room is identified by a pair of 32-byte values:
//!
//! * **Topic** — the gossip mesh topic used for peer-to-peer communication.
//! * **Discovery key** — the DHT/publication key used to publish and locate
//!   peers in the distributed topic tracker.
//!
//! Both values are derived deterministically from the same inputs (network,
//! room name, protocol version) using **different domain separators**, ensuring
//! domain separation: knowing one does not help derive the other.
//!
//! # Security properties
//!
//! * **Public, not secret.** The discovery key is a public value. It provides
//!   **compatibility** (all clients derive the same key for the same room)
//!   and **locatability** (peers find each other), not confidentiality.
//!   Anyone who knows the room name can compute the key.
//! * **Domain separated.** The topic and discovery key use distinct domain
//!   separators (`BLAKE3(domain_sep || ...)` where `domain_sep` differs).
//!   This prevents cross-protocol confusion between the gossip mesh and the
//!   DHT discovery namespace.
//! * **Versioned.** The protocol version byte is hashed into both values,
//!   allowing future protocol upgrades without changing the derivation logic.
//! * **Network-isolated.** The network byte ensures that mainnet, development,
//!   and test rooms are unconditionally disjoint even when they share a name.

use crate::proto::state::TopicId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Domain separator for the public-room discovery key.
///
/// Deliberately different from [`PUBLIC_ROOM_DOMAIN_SEPARATOR`] so that the
/// same (network, room name, version) triple always produces different topic
/// and discovery-key outputs.
pub const DISCOVERY_KEY_DOMAIN_SEPARATOR: &[u8] = b"boru-chat discovery-key v1";

/// Application-level namespace for boru-chat public rooms.
///
/// This can be used as a prefix or label in multi-application DHT setups.
pub const APPLICATION_NAMESPACE: &str = "boru-chat";

/// Canonical room name for the default public lobby.
pub const PUBLIC_ROOM_NAME: &str = "public-lobby";

/// Current protocol version for public-room identity derivation.
pub const PROTOCOL_VERSION: u8 = 1;

// ---------------------------------------------------------------------------
// Network enum
// ---------------------------------------------------------------------------

/// The network environment in which a public room exists.
///
/// Each variant maps to a unique byte used in the hash derivation,
/// guaranteeing that different networks produce different topics and
/// discovery keys even when the room name and version are identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublicNetwork {
    /// Production / main network.
    Mainnet,
    /// Development / staging network.
    Development,
    /// Test / integration-test network.
    Test,
}

impl PublicNetwork {
    /// Returns the single-byte discriminator for this network.
    ///
    /// | Variant      | Byte |
    /// |--------------|------|
    /// | `Mainnet`    | 0x00 |
    /// | `Development`| 0x01 |
    /// | `Test`       | 0x02 |
    pub const fn network_byte(&self) -> u8 {
        match self {
            Self::Mainnet => 0x00,
            Self::Development => 0x01,
            Self::Test => 0x02,
        }
    }
}

// ---------------------------------------------------------------------------
// PublicRoomIdentity
// ---------------------------------------------------------------------------

/// A deterministic public-room identity, composed of a gossip topic and a
/// discovery key.
///
/// Both fields are 32-byte BLAKE3 hashes derived from the same inputs using
/// different domain-separator prefixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRoomIdentity {
    /// Gossip mesh topic — used for subscribing to the room's message stream.
    pub topic: TopicId,
    /// Discovery key — used for publishing/lookup in the distributed topic
    /// tracker DHT.
    pub discovery_key: [u8; 32],
}

impl PublicRoomIdentity {
    /// Create a new identity from pre-computed components.
    pub fn new(topic: TopicId, discovery_key: [u8; 32]) -> Self {
        Self {
            topic,
            discovery_key,
        }
    }

    /// Short hex identifier for logging (first 8 hex chars of the topic).
    pub fn short_id(&self) -> String {
        hex::encode(&self.topic.as_bytes()[..4])
    }
}

// ---------------------------------------------------------------------------
// Derivation functions
// ---------------------------------------------------------------------------

/// Derive the 32-byte discovery key for a public room.
///
/// # Derivation
///
/// ```text
/// discovery_key = BLAKE3(
///     DISCOVERY_KEY_DOMAIN_SEPARATOR ||
///     network_byte ||
///     LE_u16(len(room_name)) ||
///     room_name_bytes ||
///     version_byte
/// )
/// ```
///
/// The only difference from the topic derivation is the domain separator
/// prefix, ensuring domain separation.
pub fn public_discovery_key(network: PublicNetwork, room_name: &str, version: u8) -> [u8; 32] {
    let room_name_bytes = room_name.as_bytes();
    let room_name_len = (room_name_bytes.len() as u16).to_le_bytes();

    let mut hasher = blake3::Hasher::new();
    hasher.update(DISCOVERY_KEY_DOMAIN_SEPARATOR);
    hasher.update(&[network.network_byte()]);
    hasher.update(&room_name_len);
    hasher.update(room_name_bytes);
    hasher.update(&[version]);

    let hash = hasher.finalize();
    <[u8; 32]>::from(*hash.as_bytes())
}

/// Derive the full [`PublicRoomIdentity`] for a public room.
///
/// Convenience wrapper that computes both the topic and the discovery key for
/// the canonical room name and protocol version.
pub fn public_room_identity(network: PublicNetwork) -> PublicRoomIdentity {
    let topic = crate::topic_derivation::public_room_topic(
        network.network_byte(),
        PUBLIC_ROOM_NAME,
        PROTOCOL_VERSION,
    );
    let discovery_key = public_discovery_key(network, PUBLIC_ROOM_NAME, PROTOCOL_VERSION);
    PublicRoomIdentity::new(topic, discovery_key)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Discovery key tests ──────────────────────────────────────────

    /// Determinism: same inputs always produce the same discovery key.
    #[test]
    fn discovery_key_is_deterministic() {
        let a = public_discovery_key(PublicNetwork::Mainnet, "public-lobby", 1);
        let b = public_discovery_key(PublicNetwork::Mainnet, "public-lobby", 1);
        assert_eq!(a, b);
    }

    /// Different room names produce different discovery keys.
    #[test]
    fn discovery_key_differs_by_room_name() {
        let a = public_discovery_key(PublicNetwork::Mainnet, "lobby-alpha", 1);
        let b = public_discovery_key(PublicNetwork::Mainnet, "lobby-beta", 1);
        assert_ne!(a, b);
    }

    /// Different networks produce different discovery keys.
    #[test]
    fn discovery_key_differs_by_network() {
        let mainnet = public_discovery_key(PublicNetwork::Mainnet, "public-lobby", 1);
        let dev = public_discovery_key(PublicNetwork::Development, "public-lobby", 1);
        let test = public_discovery_key(PublicNetwork::Test, "public-lobby", 1);
        assert_ne!(mainnet, dev);
        assert_ne!(mainnet, test);
        assert_ne!(dev, test);
    }

    /// Different protocol versions produce different discovery keys.
    #[test]
    fn discovery_key_differs_by_version() {
        let v1 = public_discovery_key(PublicNetwork::Mainnet, "public-lobby", 1);
        let v2 = public_discovery_key(PublicNetwork::Mainnet, "public-lobby", 2);
        assert_ne!(v1, v2);
    }

    /// Domain separation: discovery key differs from topic for the same inputs.
    #[test]
    fn discovery_key_differs_from_topic() {
        let net = PublicNetwork::Mainnet;
        let topic = crate::topic_derivation::public_room_topic(net.network_byte(), "public-lobby", 1);
        let dk = public_discovery_key(net, "public-lobby", 1);
        assert_ne!(topic.as_bytes(), &dk);

        let dev = PublicNetwork::Development;
        let t_dev = crate::topic_derivation::public_room_topic(dev.network_byte(), "public-lobby", 1);
        let dk_dev = public_discovery_key(dev, "public-lobby", 1);
        assert_ne!(t_dev.as_bytes(), &dk_dev);

        let test = PublicNetwork::Test;
        let t_test = crate::topic_derivation::public_room_topic(test.network_byte(), "public-lobby", 1);
        let dk_test = public_discovery_key(test, "public-lobby", 1);
        assert_ne!(t_test.as_bytes(), &dk_test);
    }

    /// Version changes the discovery key for all three networks.
    #[test]
    fn version_changes_discovery_key() {
        for net in [PublicNetwork::Mainnet, PublicNetwork::Development, PublicNetwork::Test] {
            let v1 = public_discovery_key(net, "public-lobby", 1);
            let v2 = public_discovery_key(net, "public-lobby", 2);
            assert_ne!(v1, v2, "version must change key for {:?}", net);
        }
    }

    // ── Known-answer test vectors (discovery key) ────────────────────

    /// Known-answer test vector for the mainnet public-lobby discovery key.
    ///
    /// Verified with:
    /// ```text
    /// printf 'boru-chat discovery-key v1\x00\x0c\x00public-lobby\x01' | b3sum --length 32
    /// ```
    #[test]
    fn known_answer_discovery_key_mainnet() {
        let dk = public_discovery_key(PublicNetwork::Mainnet, "public-lobby", 1);
        let expected = hex::decode("b64678c2350fc74df608598fefc97f26557624cc9c68504526c2c3f9756d57f1")
            .unwrap();
        assert_eq!(&dk[..], &expected[..]);
    }

    /// Known-answer test vector for the dev public-lobby discovery key.
    #[test]
    fn known_answer_discovery_key_development() {
        let dk = public_discovery_key(PublicNetwork::Development, "public-lobby", 1);
        let expected = hex::decode("57f065d2ed324eeeb9e3145d21c25278cf9315cf551d9148a9ed5339389ceadc")
            .unwrap();
        assert_eq!(&dk[..], &expected[..]);
    }

    /// Known-answer test vector for the test public-lobby discovery key.
    #[test]
    fn known_answer_discovery_key_test() {
        let dk = public_discovery_key(PublicNetwork::Test, "public-lobby", 1);
        let expected = hex::decode("4433f17c87e278eeb521d0c013aae4edfecdc92de10f892ab67ea06d19f99829")
            .unwrap();
        assert_eq!(&dk[..], &expected[..]);
    }

    /// Version 2 discovery key differs (known-answer for version change).
    #[test]
    fn known_answer_discovery_key_v2() {
        let dk = public_discovery_key(PublicNetwork::Mainnet, "public-lobby", 2);
        let expected = hex::decode("dcca4e664fa7e02d4e16caff1fe0c03c7ab743cf45b1cb72fd830a5198e76666")
            .unwrap();
        assert_eq!(&dk[..], &expected[..]);
    }

    // ── PublicRoomIdentity tests ─────────────────────────────────────

    /// Identity uses the canonical constants.
    #[test]
    fn identity_uses_canonical_constants() {
        let ident = public_room_identity(PublicNetwork::Mainnet);
        let expected_topic = crate::topic_derivation::public_room_topic(
            PublicNetwork::Mainnet.network_byte(),
            PUBLIC_ROOM_NAME,
            PROTOCOL_VERSION,
        );
        let expected_dk = public_discovery_key(
            PublicNetwork::Mainnet,
            PUBLIC_ROOM_NAME,
            PROTOCOL_VERSION,
        );
        assert_eq!(ident.topic, expected_topic);
        assert_eq!(ident.discovery_key, expected_dk);
    }

    /// Identities for different networks are distinct.
    #[test]
    fn identities_differ_by_network() {
        let m = public_room_identity(PublicNetwork::Mainnet);
        let d = public_room_identity(PublicNetwork::Development);
        let t = public_room_identity(PublicNetwork::Test);
        assert_ne!(m.topic, d.topic);
        assert_ne!(m.discovery_key, d.discovery_key);
        assert_ne!(m.topic, t.topic);
        assert_ne!(m.discovery_key, t.discovery_key);
        assert_ne!(d.topic, t.topic);
        assert_ne!(d.discovery_key, t.discovery_key);
    }

    /// Non-zero output (avalanche sanity check).
    #[test]
    fn discovery_key_is_nonzero() {
        let dk = public_discovery_key(PublicNetwork::Mainnet, "public-lobby", 1);
        assert!(dk.iter().any(|&b| b != 0));
    }
}
