//! Low-level BLAKE3-based gossip topic derivation for public rooms.
//!
//! The canonical topic for a public room is derived from a domain-separated
//! hash of the room name, network, and protocol version.  This ensures that
//! different networks (mainnet, development, test) live on disjoint gossip
//! topics even when they use the same room name.

use crate::proto::state::TopicId;

/// Domain separator for public-room gossip topics.
///
/// Chosen to be distinct from [`DISCOVERY_KEY_DOMAIN_SEPARATOR`] so that the
/// same inputs (network, room name, version) always produce different topic
/// and discovery-key outputs — providing **domain separation** between the
/// gossip mesh and the DHT discovery namespace.
pub const PUBLIC_ROOM_DOMAIN_SEPARATOR: &[u8] = b"boru-chat public-room v1";

/// Derive a deterministic gossip [`TopicId`] for a public room.
///
/// # Inputs
///
/// * `network_byte` — single byte identifying the network (see
///   [`PublicNetwork::network_byte`]).
/// * `room_name` — UTF-8 room name (e.g. `"public-lobby"`).
/// * `version` — protocol version byte (currently `1`).
///
/// # Derivation
///
/// ```text
/// TopicId = BLAKE3(
///     PUBLIC_ROOM_DOMAIN_SEPARATOR ||
///     network_byte ||
///     LE_u16(len(room_name)) ||
///     room_name_bytes ||
///     version_byte
/// )
/// ```
///
/// The room-name length is encoded as a little-endian 16-bit unsigned integer
/// so that the encoding is unambiguous when room names can contain variable
/// amounts of arbitrary bytes.
pub fn public_room_topic(network_byte: u8, room_name: &str, version: u8) -> TopicId {
    let room_name_bytes = room_name.as_bytes();
    let room_name_len = (room_name_bytes.len() as u16).to_le_bytes();

    let mut hasher = blake3::Hasher::new();
    hasher.update(PUBLIC_ROOM_DOMAIN_SEPARATOR);
    hasher.update(&[network_byte]);
    hasher.update(&room_name_len);
    hasher.update(room_name_bytes);
    hasher.update(&[version]);

    let hash = hasher.finalize();
    TopicId::from(*hash.as_bytes())
}

// ---------------------------------------------------------------------------
// Tracker namespace derivation (distributed-topic-tracker namespace)
// ---------------------------------------------------------------------------

/// Domain separator for tracker namespace derivation from raw [`TopicId`] bytes.
///
/// Deliberately distinct from [`PUBLIC_ROOM_DOMAIN_SEPARATOR`] and
/// [`crate::public_room::DISCOVERY_KEY_DOMAIN_SEPARATOR`] so that the same
/// room topic always produces a different tracker namespace than either the
/// gossip topic or the public-room discovery key — providing **domain
/// separation** between the gossip mesh, public-room discovery, and the
/// room-discovery tracker namespace.
///
/// This is a fixed ASCII constant (not derived from any runtime value) so
/// that the derivation is platform-independent and cross-language stable.
pub const TRACKER_NAMESPACE_DOMAIN_SEPARATOR: &[u8] = b"boru-chat room discovery v1";

/// Derive a deterministic tracker namespace from a room's raw [`TopicId`] bytes.
///
/// Returns a [`distributed_topic_tracker::TopicId`] — the namespace type used
/// by the distributed-topic-tracker crate for publishing and looking up
/// discovery records on the DHT.
///
/// # Derivation
///
/// ```text
/// namespace = SHA-256(
///     TRACKER_NAMESPACE_DOMAIN_SEPARATOR ||
///     topic_bytes
/// )
/// ```
///
/// # Properties
///
/// * **Deterministic.** Same topic bytes always produce the same namespace.
/// * **Domain-separated.** Uses a fixed ASCII prefix that differs from all
///   other boru-chat domain separators, preventing cross-protocol confusion
///   between gossip topics, discovery keys, and tracker namespaces.
/// * **No display-string dependency.** Operates solely on raw 32-byte topic
///   bytes — no room name, network byte, or protocol version is involved.
///   This is essential for private rooms where only a TopicId exists.
/// * **Cross-platform stable.** SHA-256 is a FIPS-standardised algorithm
///   with identical output on all platforms and in all languages.
///
/// # Example
///
/// ```ignore
/// use crate::proto::state::TopicId;
/// let topic = TopicId::from([0xABu8; 32]);
/// let namespace = tracker_namespace_from_topic(topic.as_bytes());
/// // `namespace` is a distributed_topic_tracker::TopicId suitable for
/// // publication and lookup on the room's discovery DHT.
/// ```
pub fn tracker_namespace_from_topic(topic_bytes: &[u8; 32]) -> distributed_topic_tracker::TopicId {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(TRACKER_NAMESPACE_DOMAIN_SEPARATOR);
    hasher.update(topic_bytes.as_slice());
    let hash: [u8; 32] = hasher.finalize().into();
    distributed_topic_tracker::TopicId::from_hash(&hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Tracker namespace tests ──────────────────────────────────────

    /// Determinism: same topic bytes always produce the same namespace.
    #[test]
    fn tracker_namespace_is_deterministic() {
        let topic = [0xABu8; 32];
        let a = tracker_namespace_from_topic(&topic);
        let b = tracker_namespace_from_topic(&topic);
        assert_eq!(a, b);
    }

    /// Different topics produce different namespaces.
    #[test]
    fn different_topics_produce_different_namespaces() {
        let topic_a = [0xABu8; 32];
        let topic_b = [0xCDu8; 32];
        let ns_a = tracker_namespace_from_topic(&topic_a);
        let ns_b = tracker_namespace_from_topic(&topic_b);
        assert_ne!(ns_a, ns_b);
    }

    /// Domain separation: the tracker namespace differs from the gossip
    /// topic for the same room (mainnet public-lobby).
    #[test]
    fn tracker_namespace_differs_from_gossip_topic() {
        let topic = crate::topic_derivation::public_room_topic(0x00, "public-lobby", 1);
        let namespace = tracker_namespace_from_topic(topic.as_bytes());
        assert_ne!(namespace.hash(), *topic.as_bytes());
    }

    /// Domain separation: the tracker namespace differs from the public-room
    /// discovery key for the same room (mainnet public-lobby).
    #[test]
    fn tracker_namespace_differs_from_discovery_key() {
        let dk = crate::public_room::public_discovery_key(
            crate::public_room::PublicNetwork::Mainnet,
            "public-lobby",
            1,
        );
        let topic = crate::topic_derivation::public_room_topic(0x00, "public-lobby", 1);
        let namespace = tracker_namespace_from_topic(topic.as_bytes());
        assert_ne!(&namespace.hash()[..], &dk[..]);
    }

    /// Domain separation: two different rooms have different tracker namespaces.
    #[test]
    fn different_rooms_have_different_tracker_namespaces() {
        let topic_a = crate::topic_derivation::public_room_topic(0x00, "lobby-alpha", 1);
        let topic_b = crate::topic_derivation::public_room_topic(0x00, "lobby-beta", 1);
        let ns_a = tracker_namespace_from_topic(topic_a.as_bytes());
        let ns_b = tracker_namespace_from_topic(topic_b.as_bytes());
        assert_ne!(ns_a, ns_b);
    }

    /// Domain separation: same room on different networks produces different
    /// tracker namespaces (because the TopicIds already differ).
    #[test]
    fn different_networks_produce_different_tracker_namespaces() {
        let topic_main = crate::topic_derivation::public_room_topic(0x00, "public-lobby", 1);
        let topic_dev = crate::topic_derivation::public_room_topic(0x01, "public-lobby", 1);
        let topic_test = crate::topic_derivation::public_room_topic(0x02, "public-lobby", 1);
        let ns_main = tracker_namespace_from_topic(topic_main.as_bytes());
        let ns_dev = tracker_namespace_from_topic(topic_dev.as_bytes());
        let ns_test = tracker_namespace_from_topic(topic_test.as_bytes());
        assert_ne!(ns_main, ns_dev);
        assert_ne!(ns_main, ns_test);
        assert_ne!(ns_dev, ns_test);
    }

    /// Known-answer test: mainnet public-lobby tracker namespace.
    /// Verified with:
    /// ```text
    /// printf 'boru-chat room discovery v1' > /tmp/prefix.bin
    /// printf '\xeb\xab\x66\xf6\x0f\xf7\x34\x45\x2d\x4f\xd8\x32\x83\xb4\xd5\xee\x22\x1d\xfa\x73\xa8\x1c\xc2\xef\x52\x0b\x91\x93\x78\xfe\x40\x16' > /tmp/topic.bin
    /// cat /tmp/prefix.bin /tmp/topic.bin | sha256sum
    /// ```
    #[test]
    fn known_answer_tracker_namespace_mainnet() {
        let topic = crate::topic_derivation::public_room_topic(0x00, "public-lobby", 1);
        let namespace = tracker_namespace_from_topic(topic.as_bytes());
        // Pre-computed SHA-256("boru-chat room discovery v1" || mainnet_public_lobby_topic_bytes)
        let expected =
            hex::decode("01722494c6723b592eadc2ad65aead7f6be6513d9837764f9ae416c44c8ef860")
                .unwrap();
        assert_eq!(&namespace.hash()[..], &expected[..]);
    }

    /// Non-zero output (avalanche sanity check).
    #[test]
    fn tracker_namespace_is_nonzero() {
        let topic = [0xABu8; 32];
        let namespace = tracker_namespace_from_topic(&topic);
        assert!(namespace.hash().iter().any(|&b| b != 0));
    }

    /// Smoke: Send + Sync for the namespace type.
    #[test]
    fn tracker_namespace_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<distributed_topic_tracker::TopicId>();
    }

    /// Smoke: a zeroed topic still produces a valid non-zero namespace.
    #[test]
    fn zero_topic_produces_nonzero_namespace() {
        let topic = [0u8; 32];
        let namespace = tracker_namespace_from_topic(&topic);
        assert!(namespace.hash().iter().any(|&b| b != 0));
    }

    // ── Original topic derivation tests ──────────────────────────────

    /// Determinism: same inputs always produce the same topic.
    #[test]
    fn topic_derivation_is_deterministic() {
        let a = public_room_topic(0x00, "public-lobby", 1);
        let b = public_room_topic(0x00, "public-lobby", 1);
        assert_eq!(a, b);
    }

    /// Different room names produce different topics.
    #[test]
    fn different_room_names_differ() {
        let a = public_room_topic(0x00, "lobby-alpha", 1);
        let b = public_room_topic(0x00, "lobby-beta", 1);
        assert_ne!(a, b);
    }

    /// Different networks produce different topics.
    #[test]
    fn different_networks_differ() {
        let mainnet = public_room_topic(0x00, "public-lobby", 1);
        let dev = public_room_topic(0x01, "public-lobby", 1);
        let test = public_room_topic(0x02, "public-lobby", 1);
        assert_ne!(mainnet, dev);
        assert_ne!(mainnet, test);
        assert_ne!(dev, test);
    }

    /// Different versions produce different topics.
    #[test]
    fn different_versions_differ() {
        let v1 = public_room_topic(0x00, "public-lobby", 1);
        let v2 = public_room_topic(0x00, "public-lobby", 2);
        assert_ne!(v1, v2);
    }

    /// Known-answer test vector for the mainnet public-lobby topic.
    ///
    /// Verified with:
    /// ```text
    /// printf 'boru-chat public-room v1\x00\x0c\x00public-lobby\x01' | b3sum --length 32
    /// ```
    #[test]
    fn known_answer_mainnet() {
        let topic = public_room_topic(0x00, "public-lobby", 1);
        let expected =
            hex::decode("ebab66f60ff734452d4fd83283b4d5ee221dfa73a81cc2ef520b919378fe4016")
                .unwrap();
        assert_eq!(topic.as_bytes(), &expected[..]);
    }

    /// Known-answer test vector for the dev public-lobby topic.
    #[test]
    fn known_answer_development() {
        let topic = public_room_topic(0x01, "public-lobby", 1);
        let expected =
            hex::decode("b8a6372ebf048d3756082eb4adb6d181a46b6c249395532acbd6043e5718bb1a")
                .unwrap();
        assert_eq!(topic.as_bytes(), &expected[..]);
    }

    /// Known-answer test vector for the test public-lobby topic.
    #[test]
    fn known_answer_test() {
        let topic = public_room_topic(0x02, "public-lobby", 1);
        let expected =
            hex::decode("188dc1a76d5766010e85a4e8deb3424526bc1c8e0e02d784373e115afa0f308c")
                .unwrap();
        assert_eq!(topic.as_bytes(), &expected[..]);
    }

    /// Non-zero output (avalanche sanity check).
    #[test]
    fn topic_is_nonzero() {
        let topic = public_room_topic(0x00, "public-lobby", 1);
        assert!(topic.as_bytes().iter().any(|&b| b != 0));
    }
}
