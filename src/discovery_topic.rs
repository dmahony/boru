//! Versioned internal discovery topic identifier.
//!
//! Every Boru node derives and joins one internal discovery gossip topic at
//! startup purely as **networking infrastructure** (peer discovery, presence,
//! connectivity bootstrapping). The topic is not a conversation: it must never
//! be added to the sidebar, persisted as a chat, or rendered in the chat UI,
//! and private direct messages / normal chat payloads must never be routed
//! through it.
//!
//! The identifier is derived deterministically from a fixed domain separator,
//! the network byte, and an explicit protocol version byte, so that:
//!
//! * all nodes on the same network derive the **same** topic,
//! * mainnet, development, and test are unconditionally disjoint,
//! * the protocol can evolve later by bumping the version byte (which changes
//!   the derived topic) without altering the derivation logic.

use crate::proto::state::TopicId;
use crate::public_room::PublicNetwork;

/// Domain separator for the internal discovery gossip topic.
///
/// Deliberately distinct from all other boru-chat domain separators
/// ([`PUBLIC_ROOM_DOMAIN_SEPARATOR`](crate::topic_derivation::PUBLIC_ROOM_DOMAIN_SEPARATOR),
/// [`DISCOVERY_KEY_DOMAIN_SEPARATOR`](crate::public_room::DISCOVERY_KEY_DOMAIN_SEPARATOR),
/// the directory separator, the tracker-namespace separator, etc.) so that the
/// same inputs never produce a public-room topic, discovery key, directory
/// topic, or discovery topic — providing **domain separation** between
/// conversation/room namespaces and the internal discovery mesh.
pub const DISCOVERY_TOPIC_DOMAIN_SEPARATOR: &[u8] = b"boru-chat internal-discovery v1";

/// Current protocol version for the internal discovery topic.
///
/// Bump this (and add a new known-answer vector) when the discovery protocol
/// evolves; the derived topic changes, so mixed-version nodes no longer
/// converge on the same mesh.
pub const BORU_DISCOVERY_PROTOCOL_VERSION: u8 = 1;

/// The canonical internal discovery topic for **Mainnet, protocol v1**.
///
/// This is the value the PDF's task 5 names `BORU_DISCOVERY_TOPIC_V1`. It
/// equals [`discovery_topic`]`(PublicNetwork::Mainnet)` — the sync test
/// [`boru_discovery_topic_v1_matches_derivation`](crate::discovery_topic::tests::boru_discovery_topic_v1_matches_derivation)
/// keeps the hard-coded bytes in lock-step with the derivation. Use
/// [`discovery_topic`] for Development/Test networks.
pub const BORU_DISCOVERY_TOPIC_V1: TopicId = TopicId::from_bytes([
    0x7f, 0x6e, 0x69, 0x18, 0x55, 0xff, 0x22, 0xb7, 0xbd, 0xab, 0x0a, 0x29, 0x8d, 0xa1, 0x8a, 0x9d,
    0xde, 0x2d, 0xf1, 0x96, 0x54, 0x64, 0x25, 0x85, 0x35, 0x80, 0x4d, 0x60, 0xa8, 0x63, 0x8d, 0x1c,
]);

/// Derive the internal discovery gossip [`TopicId`] for a network at the
/// current protocol version ([`BORU_DISCOVERY_PROTOCOL_VERSION`]).
///
/// # Derivation
///
/// ```text
/// TopicId = BLAKE3(
///     DISCOVERY_TOPIC_DOMAIN_SEPARATOR ||
///     network_byte ||
///     version_byte
/// )
/// ```
///
/// Unlike [`public_room_topic`](crate::topic_derivation::public_room_topic)
/// there is no room-name component: the internal discovery topic is a single
/// global rendezvous point per network, so the separator + network + version
/// fully determine it.
pub fn discovery_topic(network: PublicNetwork) -> TopicId {
    discovery_topic_with_version(network, BORU_DISCOVERY_PROTOCOL_VERSION)
}

/// Derive the internal discovery gossip [`TopicId`] for a network at an
/// explicit protocol version.
///
/// This is the versioned core used by [`discovery_topic`]; exposing the
/// version parameter keeps future protocol evolution (and its tests) honest
/// without changing the call site used by production code.
pub fn discovery_topic_with_version(network: PublicNetwork, version: u8) -> TopicId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DISCOVERY_TOPIC_DOMAIN_SEPARATOR);
    hasher.update(&[network.network_byte()]);
    hasher.update(&[version]);
    TopicId::from_bytes(*hasher.finalize().as_bytes())
}

// ---------------------------------------------------------------------------
// Topic-kind classification (BORU-DISC-10 routing guard)
// ---------------------------------------------------------------------------

/// Classification of a gossip topic in the receive path.
///
/// This is the FIRST classification applied to an inbound gossip event: the
/// app must know whether an event came from the internal discovery topic or
/// from a conversation topic BEFORE deserializing it into high-level chat
/// state (the Phase-4 routing guard from the discovery refactor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopicKind {
    /// The internal discovery gossip topic — networking infrastructure only.
    ///
    /// Payloads on this topic are handled exclusively by
    /// [`DiscoveryService`](crate::discovery_service::DiscoveryService);
    /// they must never reach chat persistence, rendering, or unread state.
    Discovery,
    /// Any conversation topic: direct, group, or public room (including the
    /// canonical lobby during the transition).
    Conversation,
}

/// Classify a gossip [`TopicId`] as Discovery or Conversation.
///
/// The classifier recognises the versioned internal discovery topic
/// ([`discovery_topic`]) on every network. Everything else — direct topics,
/// group topics, public-room topics, and the canonical lobby — is a
/// Conversation. The classifier deliberately does **not** treat the stale
/// MCP lobby literal (`mcp_server.rs` `b"iroh-gossip-chat/default-lobby/v1"`)
/// as canonical: that hash is not the discovery topic and must never be
/// routed as discovery infrastructure.
pub fn topic_kind(topic: TopicId) -> TopicKind {
    if is_discovery_topic(topic) {
        TopicKind::Discovery
    } else {
        TopicKind::Conversation
    }
}

/// Whether `topic` is the internal discovery gossip topic for any network.
///
/// Equivalent to `topic_kind(topic) == TopicKind::Discovery`; provided as a
/// convenience for guard sites that only need the boolean.
pub fn is_discovery_topic(topic: TopicId) -> bool {
    topic == BORU_DISCOVERY_TOPIC_V1
        || [
            PublicNetwork::Mainnet,
            PublicNetwork::Development,
            PublicNetwork::Test,
        ]
        .iter()
        .any(|network| discovery_topic(*network) == topic)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Determinism / separation ────────────────────────────────────

    /// Determinism: same network always produces the same topic.
    #[test]
    fn discovery_topic_is_deterministic() {
        let a = discovery_topic(PublicNetwork::Mainnet);
        let b = discovery_topic(PublicNetwork::Mainnet);
        assert_eq!(a, b);
    }

    /// Different networks produce different discovery topics.
    #[test]
    fn discovery_topic_differs_by_network() {
        let mainnet = discovery_topic(PublicNetwork::Mainnet);
        let dev = discovery_topic(PublicNetwork::Development);
        let test = discovery_topic(PublicNetwork::Test);
        assert_ne!(mainnet, dev);
        assert_ne!(mainnet, test);
        assert_ne!(dev, test);
    }

    /// Different protocol versions produce different discovery topics.
    #[test]
    fn discovery_topic_differs_by_version() {
        let v1 = discovery_topic_with_version(PublicNetwork::Mainnet, 1);
        let v2 = discovery_topic_with_version(PublicNetwork::Mainnet, 2);
        assert_ne!(v1, v2);
    }

    /// The default helper pins to the current protocol version.
    #[test]
    fn default_helper_uses_current_version() {
        assert_eq!(
            discovery_topic(PublicNetwork::Mainnet),
            discovery_topic_with_version(PublicNetwork::Mainnet, BORU_DISCOVERY_PROTOCOL_VERSION)
        );
    }

    /// Non-zero output (avalanche sanity check).
    #[test]
    fn discovery_topic_is_nonzero() {
        let topic = discovery_topic(PublicNetwork::Mainnet);
        assert!(topic.as_bytes().iter().any(|&b| b != 0));
    }

    // ── Known-answer test vectors ────────────────────────────────────

    /// Known-answer test vector for the mainnet discovery topic (v1).
    ///
    /// Verified with:
    /// ```text
    /// printf 'boru-chat internal-discovery v1\x00\x01' | b3sum --length 32
    /// ```
    #[test]
    fn known_answer_discovery_topic_mainnet_v1() {
        let topic = discovery_topic(PublicNetwork::Mainnet);
        let expected =
            hex::decode("7f6e691855ff22b7bdab0a298da18a9dde2df1965464258535804d60a8638d1c")
                .unwrap();
        assert_eq!(topic.as_bytes(), &expected[..]);
    }

    /// Known-answer test vector for the development discovery topic (v1).
    ///
    /// Verified with:
    /// ```text
    /// printf 'boru-chat internal-discovery v1\x01\x01' | b3sum --length 32
    /// ```
    #[test]
    fn known_answer_discovery_topic_development_v1() {
        let topic = discovery_topic(PublicNetwork::Development);
        let expected =
            hex::decode("ec234183fddc5c828e797c880067c75103af5234a863dacc79a8812a5c8ba6ca")
                .unwrap();
        assert_eq!(topic.as_bytes(), &expected[..]);
    }

    /// Known-answer test vector for the test discovery topic (v1).
    ///
    /// Verified with:
    /// ```text
    /// printf 'boru-chat internal-discovery v1\x02\x01' | b3sum --length 32
    /// ```
    #[test]
    fn known_answer_discovery_topic_test_v1() {
        let topic = discovery_topic(PublicNetwork::Test);
        let expected =
            hex::decode("586316a5e0beb8c0da7f18e71e25d0bd0497a20df9fbe5529a98c38ef3bc4dc4")
                .unwrap();
        assert_eq!(topic.as_bytes(), &expected[..]);
    }

    /// Known-answer test vector for the mainnet discovery topic (v2) —
    /// proves the version byte changes the derived topic.
    ///
    /// Verified with:
    /// ```text
    /// printf 'boru-chat internal-discovery v1\x00\x02' | b3sum --length 32
    /// ```
    #[test]
    fn known_answer_discovery_topic_mainnet_v2() {
        let topic = discovery_topic_with_version(PublicNetwork::Mainnet, 2);
        let expected =
            hex::decode("1523fe9d8ccfdcf279988488c792ee6118f342585fd007fa85acaec67c81ecc8")
                .unwrap();
        assert_eq!(topic.as_bytes(), &expected[..]);
    }

    /// The hard-coded `BORU_DISCOVERY_TOPIC_V1` const stays in lock-step with
    /// the derivation function (mainnet, current version).
    #[test]
    fn boru_discovery_topic_v1_matches_derivation() {
        assert_eq!(
            BORU_DISCOVERY_TOPIC_V1,
            discovery_topic(PublicNetwork::Mainnet)
        );
    }

    // ── Domain separation ───────────────────────────────────────────

    /// Domain separation: the discovery topic differs from the public-room
    /// gossip topic for the canonical lobby on every network.
    #[test]
    fn discovery_topic_differs_from_public_room_topic() {
        for network in [
            PublicNetwork::Mainnet,
            PublicNetwork::Development,
            PublicNetwork::Test,
        ] {
            let lobby = crate::topic_derivation::public_room_topic(
                network.network_byte(),
                "public-lobby",
                1,
            );
            assert_ne!(
                discovery_topic(network),
                lobby,
                "discovery topic must differ from public-room topic on {:?}",
                network
            );
        }
    }

    /// Domain separation: the discovery topic differs from the public-room
    /// discovery key on every network.
    #[test]
    fn discovery_topic_differs_from_discovery_key() {
        for network in [
            PublicNetwork::Mainnet,
            PublicNetwork::Development,
            PublicNetwork::Test,
        ] {
            let dk = crate::public_room::public_discovery_key(network, "public-lobby", 1);
            assert_ne!(
                discovery_topic(network).as_bytes(),
                &dk,
                "discovery topic must differ from public-room discovery key on {:?}",
                network
            );
        }
    }

    /// Domain separation: the discovery topic differs from the relay-scoped
    /// directory gossip topic. (`directory` is net-gated.)
    #[cfg(feature = "net")]
    #[test]
    fn discovery_topic_differs_from_directory_topic() {
        let dir_topic = crate::directory::directory_topic("https://boru.chat:8443");
        assert_ne!(
            discovery_topic(PublicNetwork::Mainnet),
            dir_topic,
            "discovery topic must differ from the directory topic"
        );
    }

    // ── Topic-kind classification (BORU-DISC-10 routing guard) ────────

    /// The discovery topic for every network classifies as Discovery.
    #[test]
    fn topic_kind_classifies_discovery_topics() {
        for network in [
            PublicNetwork::Mainnet,
            PublicNetwork::Development,
            PublicNetwork::Test,
        ] {
            let topic = discovery_topic(network);
            assert_eq!(topic_kind(topic), TopicKind::Discovery, "{network:?}");
            assert!(is_discovery_topic(topic), "{network:?}");
        }
    }

    /// The hard-coded `BORU_DISCOVERY_TOPIC_V1` const classifies as
    /// Discovery (it equals the mainnet derivation).
    #[test]
    fn topic_kind_classifies_boru_discovery_topic_v1() {
        assert_eq!(topic_kind(BORU_DISCOVERY_TOPIC_V1), TopicKind::Discovery);
        assert!(is_discovery_topic(BORU_DISCOVERY_TOPIC_V1));
    }

    /// Arbitrary conversation topics (direct/group/public rooms) classify
    /// as Conversation, never Discovery.
    #[test]
    fn topic_kind_classifies_conversation_topics() {
        for seed in 0u8..=8 {
            let topic = TopicId::from_bytes([seed; 32]);
            assert_eq!(topic_kind(topic), TopicKind::Conversation, "{seed}");
            assert!(!is_discovery_topic(topic), "{seed}");
        }
    }

    /// The canonical public-lobby topic is a Conversation during the
    /// transition — it is not the discovery topic.
    #[test]
    fn topic_kind_lobby_is_conversation() {
        for network in [
            PublicNetwork::Mainnet,
            PublicNetwork::Development,
            PublicNetwork::Test,
        ] {
            let lobby = crate::topic_derivation::public_room_topic(
                network.network_byte(),
                "public-lobby",
                1,
            );
            assert_eq!(topic_kind(lobby), TopicKind::Conversation, "{network:?}");
            assert!(!is_discovery_topic(lobby), "{network:?}");
        }
    }

    /// Direct (deterministic pairwise) topics classify as Conversation.
    #[test]
    fn topic_kind_direct_topic_is_conversation() {
        let a = iroh_base::SecretKey::generate().public();
        let b = iroh_base::SecretKey::generate().public();
        let topic = crate::contact::direct_topic(&a, &b);
        assert_eq!(topic_kind(topic), TopicKind::Conversation);
        assert!(!is_discovery_topic(topic));
    }
}
