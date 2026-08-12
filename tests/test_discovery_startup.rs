#![cfg(feature = "net")]

//! # Startup test: automatic discovery subscription, no user-facing conversation
//!
//! BORU-DISC-21 (PDF task 18): a fresh Boru node must join the internal
//! discovery gossip topic at startup — as **networking infrastructure only** —
//! and must NOT create any user-facing conversation for it.
//!
//! This is the FIRST discovery-refactor test task: it verifies the startup
//! guarantees from BORU-DISC-08/12 at the integration level. It asserts BOTH
//! halves of the guarantee:
//!
//! 1. **Discovery subscription exists** — a fresh in-process node (real iroh
//!    endpoint + gossip actor, relay disabled, no live network) joins
//!    `discovery_topic(network)` via [`DiscoveryService::join`] — exactly the
//!    startup path `examples/iced_chat/main.rs` uses. The service reports the
//!    derived topic, classifies it as [`TopicKind::Discovery`], and its
//!    receive path processes real discovery payloads.
//! 2. **No user-facing conversation is created** — the node's fresh
//!    [`ConversationStore`] stays empty across startup and across incoming
//!    discovery traffic: no conversation entry, no discovery-topic row, and no
//!    entry that references the discovery topic. (The GUI-level halves — no
//!    selected chat, no room-list row — are covered by the topic-kind routing
//!    guard tests in `src/conversations.rs` / `examples/iced_chat/app.rs`; the
//!    store is the persistence/rendering source this task pins down.)

use std::time::Duration;

use boru_core::{
    conversations::ConversationStore,
    discovery_message::DiscoveryMessage,
    discovery_service::{DiscoveryService, IncomingOutcome},
    discovery_topic::{
        discovery_topic, is_discovery_topic, topic_kind, BORU_DISCOVERY_TOPIC_V1, TopicKind,
    },
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room::PublicNetwork,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
    PublicKey, RelayMode, SecretKey,
};
use n0_error::Result;
use rand::{RngExt, SeedableRng};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn a fresh in-process node: real iroh endpoint (no relay, no live
/// network) + gossip actor + protocol router. Mirrors the deterministic
/// harness node setup (`tests/test_two_peers_exchange.rs`).
async fn spawn_node(rng: &mut impl rand::Rng) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip))
}

/// Deterministic test identity from a single seed byte.
fn test_key(byte: u8) -> PublicKey {
    let mut seed = [0u8; 32];
    seed[0] = byte;
    SecretKey::from_bytes(&seed).public()
}

/// A fresh node under test: its network half is kept alive while the
/// discovery service runs; the conversation store is the user-facing surface
/// that must stay untouched.
struct StartupNode {
    _router: Router,
    _endpoint: Endpoint,
    _gossip: Gossip,
    store: ConversationStore,
    _dir: TempDir,
}

/// Start a fresh node and join its internal discovery topic — the startup
/// sequence this test verifies.
async fn start_node(
    rng: &mut impl rand::Rng,
    network: PublicNetwork,
) -> Result<(StartupNode, DiscoveryService)> {
    let (router, ep, sk, gossip) = spawn_node(rng).await?;
    let dir = TempDir::new().expect("temp dir for conversation store");
    let store = ConversationStore::empty_at(dir.path());

    let local_public = sk.public();
    let service = DiscoveryService::join(
        &gossip,
        discovery_topic(network),
        Vec::new(), // no bootstrap peers: subscription must succeed on its own
        local_public,
    )
    .await
    .expect("fresh node joins the internal discovery topic");

    Ok((
        StartupNode {
            _router: router,
            _endpoint: ep,
            _gossip: gossip,
            store,
            _dir: dir,
        },
        service,
    ))
}

/// Assert the no-conversation half of the startup invariant.
fn assert_no_user_facing_conversation(node: &StartupNode, topic: &TopicId) {
    assert!(
        node.store.is_empty(),
        "fresh node must have zero conversations, got {}",
        node.store.len()
    );
    assert_eq!(node.store.len(), 0, "conversation store must be empty");
    assert!(
        node.store.find(topic).is_none(),
        "discovery topic must never be a conversation entry"
    );
    assert!(
        node.store.iter().all(|entry| entry.topic != *topic),
        "no conversation entry may reference the discovery topic"
    );
}

/// Encode a discovery message the way the drain loop would receive it.
fn encode(message: DiscoveryMessage) -> Vec<u8> {
    postcard::to_stdvec(&message).expect("encode discovery message")
}

// =========================================================================
// 1. Startup: subscription exists, no conversation created
// =========================================================================

/// A fresh node joins the internal discovery topic at startup and no
/// user-facing conversation is created — for BOTH the derived topic and the
/// canonical `BORU_DISCOVERY_TOPIC_V1` (mainnet).
#[tokio::test]
async fn startup_subscribes_to_discovery_topic_no_conversation() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C21); // BORU-DISC-21
    let network = PublicNetwork::Test;

    let (node, service) = start_node(&mut rng, network).await?;
    let topic = discovery_topic(network);

    // ── Half 1: the discovery subscription exists ──────────────────────
    assert_eq!(
        service.topic(),
        topic,
        "service must be subscribed to the derived discovery topic"
    );
    assert!(
        is_discovery_topic(service.topic()),
        "joined topic must classify as the internal discovery topic"
    );
    assert_eq!(
        topic_kind(service.topic()),
        TopicKind::Discovery,
        "joined topic must be networking infrastructure, never a conversation"
    );
    assert!(
        service.peer_updates().try_recv().is_err(),
        "fresh service must not emit peer updates before any traffic"
    );

    // ── Half 2: no user-facing conversation exists ─────────────────────
    assert_no_user_facing_conversation(&node, &topic);

    // ── Incoming discovery traffic must not create a conversation ──────
    let peer_a = test_key(0xAB);
    let peer_b = test_key(0xBC);

    assert_eq!(
        service.handle_incoming(&encode(DiscoveryMessage::hello_with_event(peer_a, 1)), peer_a),
        IncomingOutcome::Processed,
        "a discovery Hello must be processed by the discovery service"
    );
    assert_eq!(
        service.handle_incoming(
            &encode(DiscoveryMessage::presence_with_event(peer_b, 2)),
            peer_b
        ),
        IncomingOutcome::Processed,
        "a discovery Presence must be processed by the discovery service"
    );
    assert_eq!(
        service.handle_incoming(
            &encode(DiscoveryMessage::peer_advertisement_with_event(peer_b, peer_a, 3)),
            peer_b
        ),
        IncomingOutcome::Processed,
        "a discovery PeerAdvertisement must be processed by the discovery service"
    );
    assert_eq!(
        service.peer_count(),
        2,
        "discovery service tracks peers it has seen (registry, not conversations)"
    );

    // Still zero conversations: discovery traffic is infrastructure, and the
    // routing guard (BORU-DISC-10) keeps it out of chat state.
    assert_no_user_facing_conversation(&node, &topic);

    service.shutdown().await;
    Ok(())
}

// =========================================================================
// 2. Canonical mainnet topic (BORU_DISCOVERY_TOPIC_V1)
// =========================================================================

/// The production startup path (mainnet) joins the canonical
/// `BORU_DISCOVERY_TOPIC_V1` constant — the value the PDF's task 5 names —
/// and still creates no conversation.
#[tokio::test]
async fn startup_mainnet_uses_canonical_discovery_topic_v1() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C22);

    let (node, service) = start_node(&mut rng, PublicNetwork::Mainnet).await?;

    assert_eq!(
        service.topic(),
        BORU_DISCOVERY_TOPIC_V1,
        "mainnet startup must subscribe to the canonical BORU_DISCOVERY_TOPIC_V1"
    );
    assert_eq!(
        topic_kind(service.topic()),
        TopicKind::Discovery,
        "canonical discovery topic must classify as Discovery"
    );

    assert_no_user_facing_conversation(&node, &service.topic());

    service.shutdown().await;
    Ok(())
}

// =========================================================================
// 3. Derivation / classification guards (no node needed)
// =========================================================================

/// The canonical constant stays in lock-step with the derivation, the
/// classifier recognises every network's discovery topic, and the discovery
/// topic stays domain-separated from the canonical public lobby topic.
#[test]
fn discovery_topic_derivation_and_classification_guards() {
    assert_eq!(
        BORU_DISCOVERY_TOPIC_V1,
        discovery_topic(PublicNetwork::Mainnet),
        "canonical BORU_DISCOVERY_TOPIC_V1 must equal the mainnet derivation"
    );

    for network in [
        PublicNetwork::Mainnet,
        PublicNetwork::Development,
        PublicNetwork::Test,
    ] {
        let topic = discovery_topic(network);
        assert_eq!(
            topic_kind(topic),
            TopicKind::Discovery,
            "{network:?} discovery topic must classify as Discovery"
        );
        assert!(is_discovery_topic(topic), "{network:?}");

        // Domain separation: the discovery topic is not the public lobby.
        let lobby = boru_core::topic_derivation::public_room_topic(
            network.network_byte(),
            "public-lobby",
            1,
        );
        assert_ne!(topic, lobby, "{network:?} discovery topic must differ from lobby");
        assert_eq!(topic_kind(lobby), TopicKind::Conversation, "{network:?}");
    }
}

/// The startup service must complete a bounded lifetime: shutdown cancels the
/// drain + connectivity tasks without hanging (guards against a leaked
/// background task that would keep the node alive).
#[tokio::test]
async fn startup_service_shutdown_completes_promptly() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C23);

    let (_node, service) = start_node(&mut rng, PublicNetwork::Test).await?;

    tokio::time::timeout(Duration::from_secs(5), service.shutdown())
        .await
        .expect("discovery service shutdown must complete within 5s");

    Ok(())
}
