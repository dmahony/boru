#![cfg(feature = "net")]

//! # Required end-to-end test matrix (BORU-DISC-28)
//!
//! PDF Phase 7 — "Required end-to-end test matrix". Every connectivity
//! scenario from the PDF's required matrix is exercised end-to-end over a
//! real in-process iroh gossip mesh, and for each scenario the discovery
//! refactor invariants are asserted:
//!
//! 1. **Discovery works** — the peers rendezvous on the internal discovery
//!    topic (`discovery_topic(network)`, [`TopicKind::Discovery`]) and each
//!    node's [`DiscoveryService`] peer registry learns the other node
//!    (presence → connectivity wiring input).
//! 2. **No lobby chat appears** — no node ever has a conversation entry for
//!    the discovery topic; the topic classifies as Discovery, never
//!    Conversation, and differs from the public lobby topic.
//! 3. **Conversation traffic stays on its topic** — every payload that
//!    crossed the discovery topic decodes as a [`DiscoveryMessage`] and NONE
//!    verifies as a chat [`SignedMessage`]; conversely, conversation topics
//!    (direct / group) carry only chat payloads, never discovery.
//!
//! ## Required matrix
//!
//! | # | Scenario | Test in this file |
//! |---|----------|-------------------|
//! | 1 | A starts first, then B starts | [`scenario_1_a_starts_first_then_b`] |
//! | 2 | B starts first, then A starts | [`scenario_2_b_starts_first_then_a`] |
//! | 3 | Both start while the other is offline, then one reconnects | [`scenario_3_both_offline_then_one_reconnects`] |
//! | 4 | LAN direct path available | [`scenario_4_lan_direct_path_available`] |
//! | 5 | Relay path required / LAN direct path unavailable | [`scenario_5_relay_path_required`] |
//! | 6 | Direct conversation open: neither / one side / both sides | [`scenario_6a_direct_open_neither_side`], [`scenario_6b_direct_open_one_side`], [`scenario_6c_direct_open_both_sides`] |
//! | 7 | Multiple simultaneous conversations plus discovery traffic | [`scenario_7_multiple_conversations_plus_discovery`] |
//!
//! Scenarios 4 and 5 are network-mode options: scenario 4 runs with
//! `RelayMode::Disabled` and a shared in-memory address book (deterministic
//! LAN/direct simulation, the same pattern as
//! `tests/test_two_peers_exchange.rs`); scenario 5 runs against a local
//! relay server (`iroh::test_utils::run_relay_server`) with only relay
//! addresses published, so the direct path is structurally unavailable.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use boru_core::{
    api::{Event as GossipEvent, GossipTopic},
    chat_core::{Message, SignedMessage},
    contact::direct_topic,
    conversations::ConversationStore,
    discovery_message::DiscoveryMessage,
    discovery_service::{AnnounceOutcome, DiscoveryService, PeerSource},
    discovery_topic::{discovery_topic, is_discovery_topic, topic_kind, TopicKind},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room::PublicNetwork,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, tls::CaTlsConfig,
    Endpoint, PublicKey, RelayMode, SecretKey,
};
use n0_error::{bail_any, Result};
use n0_future::{boxed::BoxFuture, StreamExt};
use rand::{RngExt, SeedableRng};
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// How long a two-node mesh may take to form (dial + topic joins + gossip
/// handshakes). Generous for CI, but every poll loop exits as soon as its
/// condition is satisfied.
const MESH_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting for the mesh / spies.
const POLL_TICK: Duration = Duration::from_millis(100);
/// How long the "offline" window lasts before asserting a node stays alone.
const OFFLINE_WINDOW: Duration = Duration::from_millis(800);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Deterministic 32-byte identity seed from a single byte.
fn seed(byte: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[0] = byte;
    s
}

/// Spawn a fresh in-process node with an **explicit identity**: real iroh
/// endpoint (no relay, loopback) with the given address book, plus a gossip
/// actor and protocol router. Mirrors the deterministic harness node setup.
async fn spawn_node(
    memory: MemoryLookup,
    sk: SecretKey,
) -> Result<(Router, Endpoint, Gossip, PublicKey)> {
    let pk = sk.public();
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(sk)
        .address_lookup(memory)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), gossip, pk))
}

/// Spawn a node against a **local relay server** with `RelayMode::Custom`.
/// Used by scenario 5 (relay path required / LAN direct unavailable). The
/// endpoint must reach the relay (`online`) so the relay can route to it.
async fn spawn_node_relay(
    memory: MemoryLookup,
    sk: SecretKey,
    relay_map: iroh::RelayMap,
) -> Result<(Router, Endpoint, Gossip, PublicKey)> {
    let pk = sk.public();
    let ep = Endpoint::builder(presets::Minimal)
        .secret_key(sk)
        .address_lookup(memory)
        .relay_mode(RelayMode::Custom(relay_map))
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    // Register with the local relay so the relay can route to this endpoint.
    tokio::time::timeout(Duration::from_secs(10), ep.online())
        .await
        .map_err(|_| n0_error::anyerr!("endpoint did not reach the local relay in time"))?;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), gossip, pk))
}

/// A live node under test: its network half is kept alive while the
/// discovery service runs; the conversation store is the user-facing surface
/// that must stay untouched by discovery.
struct MatrixNode {
    _router: Router,
    _endpoint: Endpoint,
    _gossip: Gossip,
    sk: SecretKey,
    service: DiscoveryService,
    store: ConversationStore,
    _dir: TempDir,
}

/// Start a node with `identity` and join it to the internal discovery topic
/// — the startup sequence `src/bin/boru/main.rs` performs on every
/// launch. `spawn` is the endpoint constructor (direct or relay). When
/// `relay_only` is true, the address book learns ONLY the endpoint's relay
/// addresses (scenario 5 — LAN direct path structurally unavailable);
/// otherwise it learns the full address (direct + relay, if any).
async fn start_node(
    spawn: impl FnOnce(
        MemoryLookup,
        SecretKey,
    ) -> BoxFuture<Result<(Router, Endpoint, Gossip, PublicKey)>>,
    memory: MemoryLookup,
    identity: [u8; 32],
    bootstrap: Vec<PublicKey>,
    network: PublicNetwork,
    relay_only: bool,
) -> Result<(MatrixNode, PublicKey)> {
    let sk = SecretKey::from_bytes(&identity);
    let (router, ep, gossip, pk) = spawn(memory.clone(), sk.clone()).await?;
    // A fresh endpoint gets a fresh transient address; the address book must
    // learn it (mirrors the restart / known-peer path).
    if relay_only {
        let mut addr = ep.addr();
        addr.addrs.retain(|a| a.is_relay());
        memory.set_endpoint_info(addr);
    } else {
        memory.set_endpoint_info(ep.addr());
    }
    let dir = TempDir::new().expect("temp dir for conversation store");
    let store = ConversationStore::empty_at(dir.path());

    let service =
        DiscoveryService::join(&gossip, discovery_topic(network), bootstrap, pk, sk.clone())
            .await
            .expect("node joins the internal discovery topic")
            .with_announce_min_interval(Duration::ZERO);

    Ok((
        MatrixNode {
            _router: router,
            _endpoint: ep,
            _gossip: gossip,
            sk,
            service,
            store,
            _dir: dir,
        },
        pk,
    ))
}

/// A payload captured by a wire spy: the gossip topic it was received on and
/// the raw payload bytes. Recording the topic **per sample** is what lets the
/// matrix prove which topic carried which payload.
#[derive(Debug, Clone)]
struct WireSample {
    topic: TopicId,
    content: Vec<u8>,
}

/// Spawn a raw spy subscription on `topic`: it captures every payload that
/// crossed the mesh on that topic (in addition to any service's own
/// subscription) so the test can prove which topic carried which payload.
async fn spawn_spy(
    gossip: &Gossip,
    topic: TopicId,
    collected: Arc<Mutex<Vec<WireSample>>>,
) -> Result<JoinHandle<()>> {
    let mut spy = gossip.subscribe(topic, Vec::new()).await?;
    Ok(tokio::spawn(async move {
        while let Some(Ok(event)) = spy.next().await {
            if let GossipEvent::Received(message) = event {
                collected
                    .lock()
                    .expect("spy lock poisoned")
                    .push(WireSample {
                        topic,
                        content: message.content.to_vec(),
                    });
            }
        }
    }))
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

/// Wait until a gossip topic subscription is joined — i.e. its stream has
/// processed at least one `NeighborUp` and the swarm edge exists — so a
/// broadcast is not lost to the empty-mesh trap.
async fn wait_for_joined(sub: &mut GossipTopic, what: &str) -> Result<()> {
    match tokio::time::timeout(MESH_TIMEOUT, sub.joined()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => bail_any!("timed out waiting for {what} to join"),
    }
}

/// Wait until the given spy has captured a chat [`SignedMessage`] whose text
/// equals `expected_text` (proves that direction's conversation message
/// actually arrived on that topic).
async fn wait_for_msg(
    spy: &Arc<Mutex<Vec<WireSample>>>,
    expected_text: &str,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        let samples = spy.lock().expect("spy lock poisoned").clone();
        for sample in &samples {
            if let Ok((_, Message::Message { text }, _)) =
                SignedMessage::verify_and_decode(&sample.content)
            {
                if text == expected_text {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    let count = spy.lock().expect("spy lock poisoned").len();
    bail_any!("timed out waiting for {what}: spy captured {count} samples")
}

// ---------------------------------------------------------------------------
// Payload classification helpers
// ---------------------------------------------------------------------------

/// Assert the no-conversation half of the discovery invariant on one node:
/// no visible lobby chat anywhere (store, topic classification).
fn assert_no_visible_lobby_chat(store: &ConversationStore, topic: &TopicId, who: &str) {
    assert_eq!(
        store.len(),
        0,
        "{who}: fresh node must have zero conversations"
    );
    assert!(
        store.find(topic).is_none(),
        "{who}: discovery topic must never be a conversation entry"
    );
    assert!(
        store.iter().all(|entry| entry.topic != *topic),
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

/// Assert the discovery-topic invariant when the node may legitimately hold
/// other (conversation) entries: the discovery topic is never a conversation
/// entry, no entry references it, and it classifies as Discovery.
fn assert_no_discovery_conversation(store: &ConversationStore, topic: &TopicId, who: &str) {
    assert!(
        store.find(topic).is_none(),
        "{who}: discovery topic must never be a conversation entry"
    );
    assert!(
        store.iter().all(|entry| entry.topic != *topic),
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

/// Assert every captured sample on a discovery-topic spy decodes as a
/// [`DiscoveryMessage`] and NONE verifies as a chat [`SignedMessage`] — no
/// conversation payload was ever routed through discovery (the hard rule).
fn assert_discovery_only(samples: &[WireSample], expected_topic: &TopicId, who: &str) {
    assert!(
        !samples.is_empty(),
        "{who}: spy must have observed the discovery exchange on the topic"
    );
    for sample in samples {
        assert_eq!(
            &sample.topic, expected_topic,
            "{who}: discovery payload arrived on the wrong topic: {sample:?}"
        );
        let is_discovery = postcard::from_bytes::<DiscoveryMessage>(&sample.content).is_ok();
        // BORU-CP-04: control-plane presence envelopes (magic "BC") are the
        // second legitimate wire format on the discovery topic.
        let is_control = sample
            .content
            .starts_with(&boru_core::control_plane::message::CONTROL_PLANE_MAGIC)
            && matches!(
                boru_core::control_plane::message::ControlEnvelope::decode(&sample.content),
                Ok(boru_core::control_plane::message::ControlPlaneDecode::Message(_))
            );
        assert!(
            is_discovery || is_control,
            "{who}: discovery topic carried a non-discovery payload"
        );
        assert!(
            SignedMessage::verify_and_decode(&sample.content).is_err(),
            "{who}: discovery topic carried a chat payload (SignedMessage)"
        );
    }
}

/// Assert every captured sample on a conversation-topic spy (direct or
/// group) is a chat [`SignedMessage`] (never a discovery payload), and that
/// the expected message text for this direction is present — i.e. this
/// direction used ONLY its own topic.
fn assert_conversation_only(
    samples: &[WireSample],
    expected_topic: &TopicId,
    who: &str,
    expected_text: &str,
) {
    assert!(
        !samples.is_empty(),
        "{who}: conversation spy must have captured the message"
    );
    let mut saw_expected = false;
    for sample in samples {
        assert_eq!(
            &sample.topic, expected_topic,
            "{who}: conversation payload arrived on the wrong topic: {sample:?}"
        );
        match SignedMessage::verify_and_decode(&sample.content) {
            Ok((from, Message::Message { text: got }, _)) => {
                assert!(
                    !from.as_bytes().iter().all(|&b| b == 0),
                    "{who}: message must be signed by a real key"
                );
                if got == expected_text {
                    saw_expected = true;
                }
            }
            Ok((_, other, _)) => {
                panic!("{who}: conversation topic carried a non-message chat payload: {other:?}")
            }
            Err(error) => {
                panic!("{who}: conversation topic carried a non-chat payload: {error}")
            }
        }
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&sample.content).is_err(),
            "{who}: a discovery message leaked onto the conversation topic: {sample:?}"
        );
    }
    assert!(
        saw_expected,
        "{who}: expected message text {expected_text:?} never arrived on the conversation topic"
    );
}

// =========================================================================
// 1. A starts first, then B starts
// =========================================================================

/// A joins the internal discovery topic first (with no bootstrap — it is the
/// first node). B starts afterwards and bootstraps to A. Both discover each
/// other, presence flows both ways, and neither node ever shows a lobby chat
/// or routes a chat payload through discovery.
#[tokio::test]
async fn scenario_1_a_starts_first_then_b() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    // ── A starts first ───────────────────────────────────────────────────
    let (node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x11),
        Vec::new(),
        network,
        false,
    )
    .await?;
    let spy_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_a = spawn_spy(&node_a._gossip, topic, spy_a.clone()).await?;
    tokio::time::sleep(OFFLINE_WINDOW).await;

    // ── B starts second, bootstraps to A ─────────────────────────────────
    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x12),
        vec![node_a._endpoint.id()],
        network,
        false,
    )
    .await?;
    let spy_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_b = spawn_spy(&node_b._gossip, topic, spy_b.clone()).await?;

    // ── Discovery works: both learn each other ───────────────────────────
    wait_for_peer(&node_a.service, pk_b, "A to learn B").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A").await?;

    // ── Presence both ways (live discovery traffic) ──────────────────────
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;
    assert_eq!(
        node_b.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // ── No lobby chat appears; discovery-only payloads ───────────────────
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");
    assert_ne!(
        topic,
        boru_core::topic_derivation::public_room_topic(network.network_byte(), "public-lobby", 1),
        "discovery topic must differ from the public lobby"
    );
    let spy_a = spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = spy_b.lock().expect("spy lock poisoned").clone();
    assert_discovery_only(&spy_a, &topic, "A spy");
    assert_discovery_only(&spy_b, &topic, "B spy");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_a.abort();
    spy_task_b.abort();
    Ok(())
}

// =========================================================================
// 2. B starts first, then A starts
// =========================================================================

/// Mirror of scenario 1: B joins first (no bootstrap), A starts afterwards
/// and bootstraps to B. The discovery exchange is direction-symmetric.
#[tokio::test]
async fn scenario_2_b_starts_first_then_a() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    // ── B starts first ───────────────────────────────────────────────────
    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x21),
        Vec::new(),
        network,
        false,
    )
    .await?;
    let spy_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_b = spawn_spy(&node_b._gossip, topic, spy_b.clone()).await?;
    tokio::time::sleep(OFFLINE_WINDOW).await;

    // ── A starts second, bootstraps to B ─────────────────────────────────
    let (node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x22),
        vec![node_b._endpoint.id()],
        network,
        false,
    )
    .await?;
    let spy_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_a = spawn_spy(&node_a._gossip, topic, spy_a.clone()).await?;

    // ── Discovery works: both learn each other ───────────────────────────
    wait_for_peer(&node_a.service, pk_b, "A to learn B").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A").await?;

    // ── Presence both ways ───────────────────────────────────────────────
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;
    assert_eq!(
        node_b.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // ── No lobby chat appears; discovery-only payloads ───────────────────
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");
    let spy_a = spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = spy_b.lock().expect("spy lock poisoned").clone();
    assert_discovery_only(&spy_a, &topic, "A spy");
    assert_discovery_only(&spy_b, &topic, "B spy");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_a.abort();
    spy_task_b.abort();
    Ok(())
}

// =========================================================================
// 3. Both start while the other is offline, then one reconnects
// =========================================================================

/// A and B start while the other is offline — separate address books, so
/// neither has address knowledge of the other and no mesh can form. Then one
/// reconnects: A gains B's address (the persisted known-peer path) and dials
/// B into the discovery mesh; both rediscover each other and exchange live
/// presence.
#[tokio::test]
async fn scenario_3_both_offline_then_one_reconnects() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory_a = MemoryLookup::new();
    let memory_b = MemoryLookup::new();

    // ── A starts alone on the discovery topic ────────────────────────────
    let (node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory_a.clone(),
        seed(0x31),
        Vec::new(),
        network,
        false,
    )
    .await?;
    let spy_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_a = spawn_spy(&node_a._gossip, topic, spy_a.clone()).await?;

    // ── B starts while A is OFFLINE — no address knowledge of A ──────────
    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory_b.clone(),
        seed(0x32),
        Vec::new(),
        network,
        false,
    )
    .await?;
    let spy_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_b = spawn_spy(&node_b._gossip, topic, spy_b.clone()).await?;

    tokio::time::sleep(OFFLINE_WINDOW).await;
    assert_eq!(
        node_b.service.peer_count(),
        0,
        "B must stay alone while A is offline (no address knowledge of A)"
    );
    assert_eq!(
        node_a.service.peer_count(),
        0,
        "A must stay alone while B is unreachable"
    );

    // ── One reconnects: A gains B's address and dials B into the mesh ────
    memory_a.set_endpoint_info(node_b._endpoint.addr());
    node_a
        .service
        .joiner()
        .join_peers(vec![pk_b])
        .await
        .expect("A dials known peer B into the discovery mesh");

    wait_for_peer(&node_a.service, pk_b, "A to rediscover B").await?;
    wait_for_peer(&node_b.service, pk_a, "B to rediscover A").await?;

    // ── Live presence in both directions after the reconnect ─────────────
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;
    assert_eq!(
        node_b.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // ── No lobby chat appears; discovery-only payloads ───────────────────
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");
    let spy_a = spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = spy_b.lock().expect("spy lock poisoned").clone();
    assert_discovery_only(&spy_a, &topic, "A spy");
    assert_discovery_only(&spy_b, &topic, "B spy");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_a.abort();
    spy_task_b.abort();
    Ok(())
}

// =========================================================================
// 4. LAN direct path available
// =========================================================================

/// Both nodes run with `RelayMode::Disabled` and a shared in-memory address
/// book that contains each node's loopback direct addresses — the
/// deterministic LAN/direct simulation from
/// `tests/test_two_peers_exchange.rs`. The discovery exchange succeeds over
/// the direct path: both learn each other, presence flows, and no relay is
/// involved.
#[tokio::test]
async fn scenario_4_lan_direct_path_available() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    let (node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x41),
        Vec::new(),
        network,
        false,
    )
    .await?;
    // Direct path proof: the address book knows A's loopback direct address
    // (RelayMode::Disabled — no relay URL anywhere).
    let a_info = memory
        .get_endpoint_info(node_a._endpoint.id())
        .expect("A's direct addressing must be in the shared address book");
    assert!(
        a_info.addrs().any(|a| a.is_ip()),
        "A must publish a direct IP address for the LAN path"
    );
    assert!(
        a_info.addrs().all(|a| !a.is_relay()),
        "no relay address may be present when RelayMode::Disabled"
    );

    let spy_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_a = spawn_spy(&node_a._gossip, topic, spy_a.clone()).await?;

    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x42),
        vec![node_a._endpoint.id()],
        network,
        false,
    )
    .await?;
    let spy_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_b = spawn_spy(&node_b._gossip, topic, spy_b.clone()).await?;

    // ── Discovery works over the direct path ─────────────────────────────
    wait_for_peer(&node_a.service, pk_b, "A to learn B over the direct path").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A over the direct path").await?;
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;
    assert_eq!(
        node_b.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // ── No lobby chat; discovery-only payloads ───────────────────────────
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");
    let spy_a = spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = spy_b.lock().expect("spy lock poisoned").clone();
    assert_discovery_only(&spy_a, &topic, "A spy");
    assert_discovery_only(&spy_b, &topic, "B spy");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_a.abort();
    spy_task_b.abort();
    Ok(())
}

// =========================================================================
// 5. Relay path required / LAN direct path unavailable
// =========================================================================

/// Both nodes run against a **local relay server** (`run_relay_server`) with
/// `RelayMode::Custom`, and the shared address book publishes ONLY relay
/// addresses (the loopback direct addresses are stripped). This makes the
/// LAN direct path structurally unavailable — the discovery exchange must
/// succeed via the relay.
#[tokio::test]
async fn scenario_5_relay_path_required() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let (relay_map, _relay_url, _relay_guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("local relay server");
    let memory = MemoryLookup::new();

    let (node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node_relay(m, sk, relay_map.clone())),
        memory.clone(),
        seed(0x51),
        Vec::new(),
        network,
        true,
    )
    .await?;
    let spy_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_a = spawn_spy(&node_a._gossip, topic, spy_a.clone()).await?;

    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node_relay(m, sk, relay_map.clone())),
        memory.clone(),
        seed(0x52),
        vec![node_a._endpoint.id()],
        network,
        true,
    )
    .await?;
    let spy_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_b = spawn_spy(&node_b._gossip, topic, spy_b.clone()).await?;

    // Relay-only proof: the shared address book knows only relay addresses
    // for both nodes — no direct IP path exists.
    let a_info = memory
        .get_endpoint_info(node_a._endpoint.id())
        .expect("A's relay addressing must be in the shared address book");
    let a_addrs: Vec<String> = a_info.addrs().map(|a| a.to_string()).collect();
    assert!(
        a_info.addrs().all(|a| a.is_relay()),
        "A's address book entry must be relay-only (LAN direct unavailable), got {a_addrs:?}"
    );
    let b_info = memory
        .get_endpoint_info(node_b._endpoint.id())
        .expect("B's relay addressing must be in the shared address book");
    let b_addrs: Vec<String> = b_info.addrs().map(|a| a.to_string()).collect();
    assert!(
        b_info.addrs().all(|a| a.is_relay()),
        "B's address book entry must be relay-only (LAN direct unavailable), got {b_addrs:?}"
    );

    // ── Discovery works via the relay path ───────────────────────────────
    wait_for_peer(&node_a.service, pk_b, "A to learn B via the relay").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A via the relay").await?;
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;
    assert_eq!(
        node_b.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // ── No lobby chat; discovery-only payloads ───────────────────────────
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");
    let spy_a = spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = spy_b.lock().expect("spy lock poisoned").clone();
    assert_discovery_only(&spy_a, &topic, "A spy");
    assert_discovery_only(&spy_b, &topic, "B spy");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_a.abort();
    spy_task_b.abort();
    Ok(())
}

// =========================================================================
// 6a. Direct conversation open on neither side
// =========================================================================

/// Neither node has a direct conversation open (no direct-topic
/// subscription, no conversation entry) — pure discovery infrastructure.
/// The discovery exchange succeeds and neither side ever creates a
/// conversation.
#[tokio::test]
async fn scenario_6a_direct_open_neither_side() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    let (node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x61),
        Vec::new(),
        network,
        false,
    )
    .await?;
    let spy_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_a = spawn_spy(&node_a._gossip, topic, spy_a.clone()).await?;

    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x62),
        vec![node_a._endpoint.id()],
        network,
        false,
    )
    .await?;
    let spy_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_b = spawn_spy(&node_b._gossip, topic, spy_b.clone()).await?;

    wait_for_peer(&node_a.service, pk_b, "A to learn B").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A").await?;
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;

    // No conversation was ever created by discovery on either side.
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");
    let spy_a = spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = spy_b.lock().expect("spy lock poisoned").clone();
    assert_discovery_only(&spy_a, &topic, "A spy");
    assert_discovery_only(&spy_b, &topic, "B spy");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_a.abort();
    spy_task_b.abort();
    Ok(())
}

// =========================================================================
// 6b. Direct conversation open on one side only
// =========================================================================

/// A has the direct conversation open (subscribed to the deterministic pair
/// topic `direct_topic(pk_a, pk_b)`, with a conversation entry); B does not
/// — B only runs the internal discovery topic. Discovery still works for
/// both nodes, and A's direct message never leaks into discovery (B sees no
/// chat payload anywhere, because the DM stays on its own topic and B has
/// not opened that conversation).
#[tokio::test]
async fn scenario_6b_direct_open_one_side() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    let (mut node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x6B),
        Vec::new(),
        network,
        false,
    )
    .await?;
    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x6C),
        vec![node_a._endpoint.id()],
        network,
        false,
    )
    .await?;

    let direct = direct_topic(&pk_a, &pk_b);
    let spy_disc_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_disc_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_disc_a = spawn_spy(&node_a._gossip, topic, spy_disc_a.clone()).await?;
    let spy_task_disc_b = spawn_spy(&node_b._gossip, topic, spy_disc_b.clone()).await?;

    // A opens the direct conversation: wire-level subscription (the
    // OpenFriendChat → BackgroundSubscribe pattern) AND a conversation entry
    // in A's store (the user-facing open conversation).
    let mut sub_direct_a = node_a
        ._gossip
        .subscribe(direct, vec![node_b._endpoint.id()])
        .await?;
    node_a
        .store
        .upsert(boru_core::conversations::ConversationEntry::new(
            direct,
            pk_b.fmt_short().to_string(),
            "B",
        ));

    // Discovery works: both learn each other via the internal topic.
    wait_for_peer(&node_a.service, pk_b, "A to learn B").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A").await?;
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;

    // A sends a DM on the direct topic. B is not subscribed, so it must not
    // appear anywhere on B's side — and crucially it must never cross the
    // discovery topic on either node.
    let dm_text = "one-sided direct message";
    let dm = SignedMessage::sign_and_encode(
        &node_a.sk,
        &Message::Message {
            text: dm_text.into(),
        },
    )
    .expect("A signs the direct message");
    sub_direct_a.broadcast(dm).await?;
    // Let the broadcast drain through the mesh and the spies observe.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The discovery topic on BOTH nodes carries only discovery payloads —
    // the DM never leaked into discovery, even though A has the conversation
    // open and B does not.
    let disc_a = spy_disc_a.lock().expect("spy lock poisoned").clone();
    let disc_b = spy_disc_b.lock().expect("spy lock poisoned").clone();
    assert_discovery_only(&disc_a, &topic, "A discovery spy");
    assert_discovery_only(&disc_b, &topic, "B discovery spy");

    // A has the conversation open (a real conversation entry for the direct
    // topic) — but the discovery topic is still never a conversation entry.
    assert!(
        node_a.store.find(&direct).is_some(),
        "A must have the direct conversation open"
    );
    assert_no_discovery_conversation(&node_a.store, &topic, "A");
    // B never opened the direct conversation and has no entry for it either.
    assert!(
        node_b.store.find(&direct).is_none(),
        "B must have no conversation entry for the unopened direct topic"
    );
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_disc_a.abort();
    spy_task_disc_b.abort();
    Ok(())
}

// =========================================================================
// 6c. Direct conversation open on both sides
// =========================================================================

/// Both A and B have the direct conversation open (both subscribed to the
/// deterministic pair topic). DMs flow in both directions using ONLY the
/// direct topic while discovery traffic continues concurrently on the
/// internal topic — the hard rule holds: private DMs are never routed
/// through discovery.
#[tokio::test]
async fn scenario_6c_direct_open_both_sides() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    let (node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x6D),
        Vec::new(),
        network,
        false,
    )
    .await?;
    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x6E),
        vec![node_a._endpoint.id()],
        network,
        false,
    )
    .await?;

    let direct = direct_topic(&pk_a, &pk_b);
    let spy_disc_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_disc_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_direct_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_direct_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_disc_a = spawn_spy(&node_a._gossip, topic, spy_disc_a.clone()).await?;
    let spy_task_disc_b = spawn_spy(&node_b._gossip, topic, spy_disc_b.clone()).await?;
    let spy_task_direct_a = spawn_spy(&node_a._gossip, direct, spy_direct_a.clone()).await?;
    let spy_task_direct_b = spawn_spy(&node_b._gossip, direct, spy_direct_b.clone()).await?;

    // Both sides open the direct conversation (OpenFriendChat →
    // BackgroundSubscribe pattern).
    let mut sub_direct_a = node_a
        ._gossip
        .subscribe(direct, vec![node_b._endpoint.id()])
        .await?;
    let mut sub_direct_b = node_b
        ._gossip
        .subscribe(direct, vec![node_a._endpoint.id()])
        .await?;

    // Discovery mesh + direct-topic swarms form.
    wait_for_peer(&node_a.service, pk_b, "A to learn B").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A").await?;
    wait_for_joined(&mut sub_direct_a, "A direct-topic subscription").await?;
    wait_for_joined(&mut sub_direct_b, "B direct-topic subscription").await?;

    // Discovery presence traffic continues concurrently.
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;
    assert_eq!(
        node_b.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // ── A → B DM on the direct topic ────────────────────────────────────
    let text_ab = "both-sided DM A→B";
    let dm_ab = SignedMessage::sign_and_encode(
        &node_a.sk,
        &Message::Message {
            text: text_ab.into(),
        },
    )
    .expect("A signs the direct message");
    sub_direct_a.broadcast(dm_ab).await?;
    wait_for_msg(&spy_direct_b, text_ab, "B to receive A's DM").await?;

    // ── B → A DM on the direct topic ────────────────────────────────────
    let text_ba = "both-sided DM B→A";
    let dm_ba = SignedMessage::sign_and_encode(
        &node_b.sk,
        &Message::Message {
            text: text_ba.into(),
        },
    )
    .expect("B signs the direct message");
    sub_direct_b.broadcast(dm_ba).await?;
    wait_for_msg(&spy_direct_a, text_ba, "A to receive B's DM").await?;

    // ── Capture and classify ────────────────────────────────────────────
    let disc_a = spy_disc_a.lock().expect("spy lock poisoned").clone();
    let disc_b = spy_disc_b.lock().expect("spy lock poisoned").clone();
    let direct_a = spy_direct_a.lock().expect("spy lock poisoned").clone();
    let direct_b = spy_direct_b.lock().expect("spy lock poisoned").clone();

    assert_ne!(
        direct, topic,
        "direct topic and discovery topic must differ"
    );
    assert_eq!(topic_kind(direct), TopicKind::Conversation);
    assert_eq!(topic_kind(topic), TopicKind::Discovery);

    // (a) DMs use ONLY the direct topic.
    assert_conversation_only(&direct_b, &direct, "A→B direct spy", text_ab);
    assert_conversation_only(&direct_a, &direct, "B→A direct spy", text_ba);
    // (b) NO DM ever crossed the discovery topic.
    assert_discovery_only(&disc_a, &topic, "A discovery spy");
    assert_discovery_only(&disc_b, &topic, "B discovery spy");

    // (c) No lobby chat / discovery-topic conversation on either side.
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_disc_a.abort();
    spy_task_disc_b.abort();
    spy_task_direct_a.abort();
    spy_task_direct_b.abort();
    Ok(())
}

// =========================================================================
// 7. Multiple simultaneous conversations plus discovery traffic
// =========================================================================

/// A and B have TWO conversations open simultaneously — the deterministic
/// direct pair topic AND a group topic — while the internal discovery topic
/// runs concurrently as infrastructure. Direct messages stay on the direct
/// topic, group messages stay on the group topic, discovery stays on the
/// discovery topic; each topic's spy sees only its own payload class and the
/// group membership stays exactly {A, B} (discovery never grants
/// membership).
#[tokio::test]
async fn scenario_7_multiple_conversations_plus_discovery() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C28A1);
    let memory = MemoryLookup::new();

    let (node_a, pk_a) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x71),
        Vec::new(),
        network,
        false,
    )
    .await?;
    let (node_b, pk_b) = start_node(
        |m, sk| Box::pin(spawn_node(m, sk)),
        memory.clone(),
        seed(0x72),
        vec![node_a._endpoint.id()],
        network,
        false,
    )
    .await?;

    // Two conversations: the deterministic direct pair topic + a fresh
    // random group topic (exactly how the app creates groups).
    let direct = direct_topic(&pk_a, &pk_b);
    let group = TopicId::from_bytes(rng.random());
    assert_ne!(direct, group, "the two conversations must differ");

    // Spies: discovery + direct + group, on both nodes.
    let spy_disc_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_disc_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_direct_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_direct_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_group_a: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_group_b: Arc<Mutex<Vec<WireSample>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_disc_a = spawn_spy(&node_a._gossip, topic, spy_disc_a.clone()).await?;
    let spy_task_disc_b = spawn_spy(&node_b._gossip, topic, spy_disc_b.clone()).await?;
    let spy_task_direct_a = spawn_spy(&node_a._gossip, direct, spy_direct_a.clone()).await?;
    let spy_task_direct_b = spawn_spy(&node_b._gossip, direct, spy_direct_b.clone()).await?;
    let spy_task_group_a = spawn_spy(&node_a._gossip, group, spy_group_a.clone()).await?;
    let spy_task_group_b = spawn_spy(&node_b._gossip, group, spy_group_b.clone()).await?;

    // Both sides open BOTH conversations.
    let mut sub_direct_a = node_a
        ._gossip
        .subscribe(direct, vec![node_b._endpoint.id()])
        .await?;
    let mut sub_direct_b = node_b
        ._gossip
        .subscribe(direct, vec![node_a._endpoint.id()])
        .await?;
    let mut sub_group_a = node_a
        ._gossip
        .subscribe(group, vec![node_b._endpoint.id()])
        .await?;
    let mut sub_group_b = node_b
        ._gossip
        .subscribe(group, vec![node_a._endpoint.id()])
        .await?;

    // Everything joins: discovery mesh, direct swarm, group swarm.
    wait_for_peer(&node_a.service, pk_b, "A to learn B").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A").await?;
    wait_for_joined(&mut sub_direct_a, "A direct-topic subscription").await?;
    wait_for_joined(&mut sub_direct_b, "B direct-topic subscription").await?;
    wait_for_joined(&mut sub_group_a, "A group subscription").await?;
    wait_for_joined(&mut sub_group_b, "B group subscription").await?;

    // Group membership stays exactly {A, B} before discovery traffic.
    assert_eq!(
        sub_group_a.neighbors().collect::<Vec<_>>(),
        vec![pk_b],
        "A's group membership must be exactly B"
    );
    assert_eq!(
        sub_group_b.neighbors().collect::<Vec<_>>(),
        vec![pk_a],
        "B's group membership must be exactly A"
    );

    // Discovery presence traffic continues concurrently.
    assert_eq!(
        node_a.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "A's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Presence,
        "B's registry for A",
    )
    .await?;
    assert_eq!(
        node_b.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Presence,
        "A's registry for B",
    )
    .await?;

    // Discovery must NOT change group membership (discovery does not grant
    // membership).
    assert_eq!(
        sub_group_a.neighbors().collect::<Vec<_>>(),
        vec![pk_b],
        "A's group membership must be unchanged by discovery traffic"
    );
    assert_eq!(
        sub_group_b.neighbors().collect::<Vec<_>>(),
        vec![pk_a],
        "B's group membership must be unchanged by discovery traffic"
    );

    // ── DM on the direct topic ──────────────────────────────────────────
    let text_dm = "matrix DM on direct";
    let dm = SignedMessage::sign_and_encode(
        &node_a.sk,
        &Message::Message {
            text: text_dm.into(),
        },
    )
    .expect("A signs the direct message");
    sub_direct_a.broadcast(dm).await?;
    wait_for_msg(&spy_direct_b, text_dm, "B to receive the direct DM").await?;

    // ── Group message on the group topic ────────────────────────────────
    let text_group = "matrix group message";
    let group_msg = SignedMessage::sign_and_encode(
        &node_a.sk,
        &Message::Message {
            text: text_group.into(),
        },
    )
    .expect("A signs the group message");
    sub_group_a.broadcast(group_msg).await?;
    wait_for_msg(&spy_group_b, text_group, "B to receive the group message").await?;

    // ── Capture and classify every topic's wire samples ─────────────────
    let disc_a = spy_disc_a.lock().expect("spy lock poisoned").clone();
    let disc_b = spy_disc_b.lock().expect("spy lock poisoned").clone();
    let direct_a = spy_direct_a.lock().expect("spy lock poisoned").clone();
    let direct_b = spy_direct_b.lock().expect("spy lock poisoned").clone();
    let group_a = spy_group_a.lock().expect("spy lock poisoned").clone();
    let group_b = spy_group_b.lock().expect("spy lock poisoned").clone();

    println!("captured direct topic id:  {direct}");
    println!("captured group topic id:   {group}");
    println!("captured discovery topic:  {topic}");
    println!(
        "wire samples: disc_A={} disc_B={} direct_A={} direct_B={} group_A={} group_B={}",
        disc_a.len(),
        disc_b.len(),
        direct_a.len(),
        direct_b.len(),
        group_a.len(),
        group_b.len()
    );

    // Domain separation: three distinct topics with the right kinds.
    assert_ne!(direct, group);
    assert_ne!(direct, topic);
    assert_ne!(group, topic);
    assert_eq!(topic_kind(direct), TopicKind::Conversation);
    assert_eq!(topic_kind(group), TopicKind::Conversation);
    assert_eq!(topic_kind(topic), TopicKind::Discovery);
    assert!(is_discovery_topic(topic));

    // (a) DMs use ONLY the direct topic.
    assert_conversation_only(&direct_b, &direct, "A→B direct spy", text_dm);
    // (b) Group messages use ONLY the group topic.
    assert_conversation_only(&group_b, &group, "A→B group spy", text_group);
    // (c) Discovery topic carries NO chat payload.
    assert_discovery_only(&disc_a, &topic, "A discovery spy");
    assert_discovery_only(&disc_b, &topic, "B discovery spy");

    // (d) No lobby chat / discovery-topic conversation on either side.
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_disc_a.abort();
    spy_task_disc_b.abort();
    spy_task_direct_a.abort();
    spy_task_direct_b.abort();
    spy_task_group_a.abort();
    spy_task_group_b.abort();
    Ok(())
}
