#![cfg(feature = "net")]

//! # Two-node discovery test — no visible lobby chat
//!
//! BORU-DISC-22 (PDF task 19): two fresh Boru nodes, A and B, start with NO
//! direct chat open and discover each other on the internal discovery gossip
//! topic — as **networking infrastructure only** — with no visible lobby
//! chat and no chat message ever created.
//!
//! ## What the test proves
//!
//! 1. **Discovery exchange** — both nodes join `discovery_topic(network)`
//!    (the internal discovery topic, [`TopicKind::Discovery`], distinct from
//!    the public lobby) and exchange discovery `Hello` / `Presence` messages
//!    over a real in-process gossip mesh. Relay is disabled; both endpoints
//!    share one in-memory address book ([`MemoryLookup`]) so they can dial
//!    each other by endpoint id — the deterministic two-node pattern from
//!    `tests/test_two_peers_exchange.rs`.
//! 2. **Peer info learned** — each node's [`DiscoveryService`] peer registry
//!    contains the other node (the BORU-DISC-11 presence → connectivity
//!    wiring input): A learns B, B learns A. A `PeerAdvertisement` about a
//!    third node emits a `PeerUpdate::Advertised` dial candidate.
//! 3. **No visible lobby chat** — neither node has a conversation entry,
//!    nothing references the discovery topic, and the topic classifies as
//!    Discovery (never Conversation) and differs from the public lobby topic.
//! 4. **Isolation guarantee** — raw spy subscriptions on both nodes observe
//!    every payload that crossed the discovery mesh: each decodes as a
//!    [`DiscoveryMessage`] and NONE verify as a chat [`SignedMessage`]
//!    payload, so no chat message was created or routed through the
//!    discovery topic (the hard rule from the discovery refactor).

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use boru_core::{
    api::Event as GossipEvent,
    chat_core::SignedMessage,
    conversations::ConversationStore,
    discovery_message::DiscoveryMessage,
    discovery_service::{AnnounceOutcome, DiscoveryService, PeerSource, PeerUpdate},
    discovery_topic::{discovery_topic, is_discovery_topic, topic_kind, TopicKind},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room::PublicNetwork,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMode, SecretKey,
};
use n0_error::{bail_any, Result};
use n0_future::StreamExt;
use rand::{RngExt, SeedableRng};
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// How long a two-node mesh may take to form (dial + topic join + history
/// sync). Generous for CI, but the poll loop exits as soon as the assertion
/// is satisfied.
const MESH_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting for the mesh / registry updates.
const POLL_TICK: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn a fresh in-process node: real iroh endpoint (no relay, loopback)
/// with the shared in-memory address book, plus a gossip actor and protocol
/// router. Mirrors the deterministic harness node setup.
async fn spawn_node(
    rng: &mut impl rand::Rng,
    memory: MemoryLookup,
) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(memory)
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

/// A node under test: its network half is kept alive while the discovery
/// service runs; the conversation store is the user-facing surface that must
/// stay untouched.
struct DiscoveryNode {
    _router: Router,
    _endpoint: Endpoint,
    _gossip: Gossip,
    service: DiscoveryService,
    store: ConversationStore,
    _dir: TempDir,
}

/// Spawn a raw spy subscription on the discovery topic: it captures every
/// payload that crossed the mesh (in addition to the service's own
/// subscription) so the test can prove the isolation guarantee — discovery
/// payloads only, never chat.
async fn spawn_spy(
    gossip: &Gossip,
    topic: TopicId,
    collected: Arc<Mutex<Vec<Vec<u8>>>>,
) -> Result<JoinHandle<()>> {
    let mut spy = gossip.subscribe(topic, Vec::new()).await?;
    Ok(tokio::spawn(async move {
        while let Some(Ok(event)) = spy.next().await {
            if let GossipEvent::Received(message) = event {
                collected
                    .lock()
                    .expect("spy lock poisoned")
                    .push(message.content.to_vec());
            }
        }
    }))
}

/// A two-node discovery harness: A and B join the internal discovery topic
/// with NO direct chat open (their conversation stores are fresh and empty)
/// and exchange discovery messages over a real loopback gossip mesh.
struct TwoNodeHarness {
    a: DiscoveryNode,
    b: DiscoveryNode,
    topic: TopicId,
    pk_a: PublicKey,
    pk_b: PublicKey,
    spy_a: Arc<Mutex<Vec<Vec<u8>>>>,
    spy_b: Arc<Mutex<Vec<Vec<u8>>>>,
    _spy_task_a: JoinHandle<()>,
    _spy_task_b: JoinHandle<()>,
}

impl TwoNodeHarness {
    /// Start A and B with no direct chat open and join both to the internal
    /// discovery topic. A subscribes with no bootstrap (B dials in); B
    /// bootstraps to A. Each side publishes its join `Hello` on the topic.
    async fn spawn(rng: &mut impl rand::Rng, network: PublicNetwork) -> Result<Self> {
        let topic = discovery_topic(network);

        // Shared in-memory address book: both endpoints can dial each other
        // by endpoint id (the deterministic two-node pattern).
        let memory = MemoryLookup::new();
        let (router_a, ep_a, sk_a, gossip_a) = spawn_node(rng, memory.clone()).await?;
        let (router_b, ep_b, sk_b, gossip_b) = spawn_node(rng, memory.clone()).await?;
        memory.add_endpoint_info(ep_a.addr());
        memory.add_endpoint_info(ep_b.addr());

        let pk_a = sk_a.public();
        let pk_b = sk_b.public();

        let dir_a = TempDir::new().expect("temp dir for A's conversation store");
        let store_a = ConversationStore::empty_at(dir_a.path());
        let dir_b = TempDir::new().expect("temp dir for B's conversation store");
        let store_b = ConversationStore::empty_at(dir_b.path());

        // Raw spies subscribe before the services so nothing is missed.
        let spy_a: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_b: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_task_a = spawn_spy(&gossip_a, topic, spy_a.clone()).await?;
        let spy_task_b = spawn_spy(&gossip_b, topic, spy_b.clone()).await?;

        // The startup path from `examples/iced_chat/main.rs`: join the
        // internal discovery topic via DiscoveryService::join. A short
        // announce interval lets the test drive presence exchange without
        // sleeping through the production 30s throttle.
        let service_a = DiscoveryService::join(&gossip_a, topic, Vec::new(), pk_a)
            .await
            .expect("A joins the internal discovery topic")
            .with_announce_min_interval(Duration::ZERO);
        let service_b = DiscoveryService::join(&gossip_b, topic, vec![ep_a.id()], pk_b)
            .await
            .expect("B joins the internal discovery topic")
            .with_announce_min_interval(Duration::ZERO);

        Ok(Self {
            a: DiscoveryNode {
                _router: router_a,
                _endpoint: ep_a,
                _gossip: gossip_a,
                service: service_a,
                store: store_a,
                _dir: dir_a,
            },
            b: DiscoveryNode {
                _router: router_b,
                _endpoint: ep_b,
                _gossip: gossip_b,
                service: service_b,
                store: store_b,
                _dir: dir_b,
            },
            topic,
            pk_a,
            pk_b,
            spy_a,
            spy_b,
            _spy_task_a: spy_task_a,
            _spy_task_b: spy_task_b,
        })
    }

    /// Stop the spies and shut both discovery services down cleanly.
    async fn shutdown(self) {
        self._spy_task_a.abort();
        self._spy_task_b.abort();
        self.a.service.shutdown().await;
        self.b.service.shutdown().await;
    }
}

/// Wait until `service`'s peer registry contains `peer`.
async fn wait_for_peer(service: &DiscoveryService, peer: PublicKey, what: &str) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if service.known_peers().iter().any(|(id, _)| *id == peer) {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    let known: Vec<String> = service
        .known_peers()
        .iter()
        .map(|(id, _)| id.fmt_short().to_string())
        .collect();
    bail_any!("timed out waiting for {what}: registry has {known:?}")
}

/// Wait until `service`'s registry entry for `peer` reports `source`.
async fn wait_for_source(
    service: &DiscoveryService,
    peer: PublicKey,
    source: PeerSource,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if let Some((_, entry)) = service.known_peers().iter().find(|(id, _)| *id == peer) {
            if entry.source == source {
                return Ok(());
            }
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    bail_any!("timed out waiting for {what} to report source {source:?}")
}

/// Assert the no-conversation half of the discovery invariant on one node:
/// no lobby chat is visible anywhere (store, topic classification).
fn assert_no_visible_lobby_chat(node: &DiscoveryNode, topic: &TopicId, who: &str) {
    assert!(
        node.store.is_empty(),
        "{who}: fresh node must have zero conversations, got {}",
        node.store.len()
    );
    assert_eq!(node.store.len(), 0, "{who}: conversation store must be empty");
    assert!(
        node.store.find(topic).is_none(),
        "{who}: discovery topic must never be a conversation entry"
    );
    assert!(
        node.store.iter().all(|entry| entry.topic != *topic),
        "{who}: no conversation entry may reference the discovery topic"
    );
    assert_eq!(
        topic_kind(*topic),
        TopicKind::Discovery,
        "{who}: the discovery topic must classify as Discovery, not Conversation"
    );
    assert!(
        is_discovery_topic(*topic),
        "{who}: topic must be the internal discovery topic"
    );
}

/// Prove the isolation guarantee on the wire: every payload that crossed the
/// discovery topic decodes as a [`DiscoveryMessage`] and NONE verify as a
/// chat [`SignedMessage`] payload — i.e. the discovery exchange happened
/// without any chat message being created or routed through discovery.
fn assert_no_chat_payloads(collected: &[Vec<u8>], who: &str) {
    assert!(
        !collected.is_empty(),
        "{who}: spy must have observed the discovery exchange on the topic"
    );
    for content in collected {
        let decoded = postcard::from_bytes::<DiscoveryMessage>(content).unwrap_or_else(|error| {
            panic!("{who}: discovery topic carried a non-discovery payload: {error}")
        });
        assert!(
            SignedMessage::verify_and_decode(content).is_err(),
            "{who}: discovery topic carried a chat payload (SignedMessage): {decoded:?}"
        );
    }
}

// =========================================================================
// 1. Two nodes discover each other on the internal discovery topic — no chat
// =========================================================================

/// A and B start with NO direct chat open; they join the internal discovery
/// topic and exchange discovery `Hello` / `Presence` messages over the
/// gossip mesh. Each node learns the other's peer info (presence → address
/// book / connectivity wiring), and neither side ever has a visible lobby
/// chat or a chat message.
#[tokio::test]
async fn two_nodes_discover_each_other_without_lobby_chat() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C22A1);
    let harness = TwoNodeHarness::spawn(&mut rng, PublicNetwork::Test).await?;

    // ── A and B learn each other via the join Hello exchange ────────────
    wait_for_peer(&harness.a.service, harness.pk_b, "A to learn B").await?;
    wait_for_peer(&harness.b.service, harness.pk_a, "B to learn A").await?;

    // ── Presence heartbeat exchange (live, after the mesh edge exists) ──
    assert_eq!(
        harness.a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &harness.b.service,
        harness.pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;

    assert_eq!(
        harness.b.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &harness.a.service,
        harness.pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // ── No visible lobby chat on either side ────────────────────────────
    assert_no_visible_lobby_chat(&harness.a, &harness.topic, "A");
    assert_no_visible_lobby_chat(&harness.b, &harness.topic, "B");

    // The discovery topic is NOT the public lobby topic, so no lobby chat
    // is even possible on this mesh.
    let lobby = boru_core::topic_derivation::public_room_topic(
        PublicNetwork::Test.network_byte(),
        "public-lobby",
        1,
    );
    assert_ne!(
        harness.topic, lobby,
        "discovery topic must differ from the public lobby"
    );

    // ── Isolation guarantee: only discovery payloads crossed the mesh ───
    let spy_a = harness.spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = harness.spy_b.lock().expect("spy lock poisoned").clone();
    assert_no_chat_payloads(&spy_a, "A spy");
    assert_no_chat_payloads(&spy_b, "B spy");

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 2. PeerAdvertisement creates a dial candidate — still no chat
// =========================================================================

/// After the two nodes know each other, A advertises a third node C. B's
/// discovery service emits a `PeerUpdate::Advertised` dial candidate (the
/// BORU-DISC-11 connectivity wiring input) WITHOUT creating any conversation
/// or chat message.
#[tokio::test]
async fn two_nodes_advertisement_creates_dial_candidate_without_chat() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C22A2);
    let harness = TwoNodeHarness::spawn(&mut rng, PublicNetwork::Test).await?;

    wait_for_peer(&harness.a.service, harness.pk_b, "A to learn B").await?;
    wait_for_peer(&harness.b.service, harness.pk_a, "B to learn A").await?;

    // A advertises a third node C that B has never seen.
    let pk_c = test_key(0xC0);
    harness
        .a
        .service
        .publish(DiscoveryMessage::peer_advertisement_with_event(
            harness.pk_a,
            pk_c,
            0xD15C,
        ))
        .await?;

    // B's discovery service emits the Advertised dial candidate.
    let mut updates = harness.b.service.peer_updates();
    let deadline = Instant::now() + MESH_TIMEOUT;
    let mut saw_advertised = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(POLL_TICK, updates.recv()).await {
            Ok(Ok(PeerUpdate::Advertised { node_id, advertised }))
                if node_id == harness.pk_a && advertised == pk_c =>
            {
                saw_advertised = true;
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {} // poll timeout — keep waiting
        }
    }
    assert!(
        saw_advertised,
        "B must emit a PeerUpdate::Advertised dial candidate for C"
    );

    // Still no visible lobby chat and no chat message.
    assert_no_visible_lobby_chat(&harness.a, &harness.topic, "A");
    assert_no_visible_lobby_chat(&harness.b, &harness.topic, "B");
    let spy_a = harness.spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = harness.spy_b.lock().expect("spy lock poisoned").clone();
    assert_no_chat_payloads(&spy_a, "A spy");
    assert_no_chat_payloads(&spy_b, "B spy");

    harness.shutdown().await;
    Ok(())
}
