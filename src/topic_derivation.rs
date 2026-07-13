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

#[cfg(test)]
mod tests {
    use super::*;

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
