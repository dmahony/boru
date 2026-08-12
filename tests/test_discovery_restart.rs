#![cfg(feature = "net")]

//! # Restart test — peer rejoins discovery and reconnects
//!
//! BORU-DISC-26 (PDF task 23): restart one peer while the other stays
//! online. The restarted peer rejoins the internal discovery gossip topic
//! and reconnects to the peer it already knew, with no chat message and no
//! user-facing conversation ever created.
//!
//! ## What the tests prove
//!
//! 1. **Rejoin discovery on restart** — node B is restarted (its gossip
//!    instance and discovery service shut down cleanly — the app's normal
//!    stop/start path — while node A stays online). A fresh B is spawned
//!    with the **same identity** (same `SecretKey` — a real restart keeps
//!    the node's key) and calls [`DiscoveryService::join`] on the internal
//!    discovery topic — exactly the startup path
//!    `examples/iced_chat/main.rs` uses. The rejoin succeeds and the new
//!    service is joined to `discovery_topic(network)`, classified as
//!    [`TopicKind::Discovery`], never a conversation.
//! 2. **Reconnect to a known peer** — the restarted node's peer registry
//!    re-learns A (the node it knew before the restart) purely through the
//!    discovery mesh: B dials A via its known address (the
//!    bootstrap/known-peer path), A's drain loop re-announces a `Hello` on
//!    neighbour-up, and B's registry fills back in. A, in turn, sees B's
//!    post-restart `Presence` heartbeat and refreshes B's registry entry —
//!    proving the mesh edge survived the restart and carries live discovery
//!    traffic again.
//! 3. **Both start while the other is offline, then one reconnects** (the
//!    E2E matrix scenario) — A starts alone; B starts while A is offline
//!    (B has no address knowledge of A, so the mesh cannot form). B stays
//!    alone. Then A comes back online with B's address known (the persisted
//!    known-peer path) and dials B into the discovery mesh; both nodes
//!    rediscover each other.
//! 4. **Isolation guarantee** — raw spy subscriptions observe every payload
//!    that crossed the discovery topic across the restart: each decodes as
//!    a [`DiscoveryMessage`] and NONE verify as a chat [`SignedMessage`],
//!    and no node ever has a conversation entry. The hard rule from the
//!    discovery refactor holds before, during, and after the restart.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use boru_core::{
    api::Event as GossipEvent,
    chat_core::SignedMessage,
    conversations::ConversationStore,
    discovery_message::DiscoveryMessage,
    discovery_service::{AnnounceOutcome, DiscoveryService, PeerSource},
    discovery_topic::{discovery_topic, is_discovery_topic, topic_kind, TopicKind},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room::PublicNetwork,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
    PublicKey, RelayMode, SecretKey,
};
use n0_error::{bail_any, Result};
use n0_future::StreamExt;
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// How long a two-node mesh may take to (re)form (dial + topic join +
/// discovery exchange). Generous for CI, but the poll loop exits as soon as
/// the assertion is satisfied.
const MESH_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting for the mesh / registry updates.
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

/// Spawn a fresh in-process node with an **explicit identity** (the same
/// `SecretKey` is reused across a restart): real iroh endpoint (no relay,
/// loopback) with the given address book, plus a gossip actor and protocol
/// router. Mirrors the deterministic harness node setup.
async fn spawn_node_with_key(
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

/// A live node under test: its network half is kept alive while the
/// discovery service runs; the conversation store is the user-facing surface
/// that must stay untouched.
struct LiveNode {
    _router: Router,
    _endpoint: Endpoint,
    _gossip: Gossip,
    service: DiscoveryService,
    store: ConversationStore,
    _dir: TempDir,
}

/// Start a node with `identity` and join it to the internal discovery topic
/// — the startup sequence `examples/iced_chat/main.rs` performs on every
/// launch, including after a restart.
async fn start_node(
    memory: MemoryLookup,
    identity: [u8; 32],
    bootstrap: Vec<PublicKey>,
    network: PublicNetwork,
) -> Result<(LiveNode, PublicKey)> {
    let (router, ep, gossip, pk) =
        spawn_node_with_key(memory.clone(), SecretKey::from_bytes(&identity)).await?;
    // A restart gives the endpoint a fresh transient address; the shared
    // address book must learn it (replacing the stale pre-restart entry).
    memory.set_endpoint_info(ep.addr());
    let dir = TempDir::new().expect("temp dir for conversation store");
    let store = ConversationStore::empty_at(dir.path());

    let service = DiscoveryService::join(&gossip, discovery_topic(network), bootstrap, pk)
        .await
        .expect("node joins the internal discovery topic")
        .with_announce_min_interval(Duration::ZERO);

    Ok((
        LiveNode {
            _router: router,
            _endpoint: ep,
            _gossip: gossip,
            service,
            store,
            _dir: dir,
        },
        pk,
    ))
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
/// no visible lobby chat anywhere (store, topic classification).
fn assert_no_visible_lobby_chat(store: &ConversationStore, topic: &TopicId, who: &str) {
    assert!(
        store.is_empty(),
        "{who}: fresh node must have zero conversations, got {}",
        store.len()
    );
    assert_eq!(store.len(), 0, "{who}: conversation store must be empty");
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

/// Prove the isolation guarantee on the wire: every payload that crossed the
/// discovery topic decodes as a [`DiscoveryMessage`] and NONE verify as a
/// chat [`SignedMessage`] payload.
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
// 1. Restart B while A stays online — B rejoins discovery and reconnects
// =========================================================================

/// A and B are connected on the internal discovery topic. B then restarts
/// (its gossip instance and discovery service shut down cleanly, and a
/// fresh B with the **same identity** comes up) while A stays online. The
/// restarted B rejoins the discovery topic at startup, re-learns A through
/// the discovery mesh, and A's registry is refreshed by B's post-restart
/// presence — with no conversation and no chat payload anywhere.
#[tokio::test]
async fn restarted_peer_rejoins_discovery_and_reconnects() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let id_a = seed(0xAA);
    let id_b = seed(0xBB);
    let pk_a = SecretKey::from_bytes(&id_a).public();
    let pk_b = SecretKey::from_bytes(&id_b).public();

    // Shared in-memory address book: both endpoints can dial each other by
    // endpoint id (the deterministic two-node pattern).
    let memory = MemoryLookup::new();

    // ── Phase 0: A and B up, connected on the discovery topic ────────────
    let (node_a, _) = start_node(memory.clone(), id_a, Vec::new(), network).await?;
    let (node_b, _) = start_node(memory.clone(), id_b, vec![node_a._endpoint.id()], network).await?;

    // Raw spies subscribe before the services so nothing is missed.
    let spy_a: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_b: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_a = spawn_spy(&node_a._gossip, topic, spy_a.clone()).await?;
    let spy_task_b = spawn_spy(&node_b._gossip, topic, spy_b.clone()).await?;

    // A and B discover each other via the join Hello exchange.
    wait_for_peer(&node_a.service, pk_b, "A to learn B").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A").await?;

    // ── Phase 1: B restarts — graceful shutdown (like the app's normal
    //    stop/start) while A stays online. Gossip::shutdown sends
    //    Disconnect to peers and closes the QUIC connection cleanly, so A
    //    processes B's departure and its discovery topic state drops B as a
    //    neighbour (a hard crash would rely on QUIC idle-timeout death
    //    detection, which takes tens of seconds — out of scope here). ─────
    spy_task_b.abort();
    let LiveNode {
        _router,
        _endpoint,
        _gossip,
        service,
        store,
        _dir,
    } = node_b;
    service.shutdown().await;
    _gossip.shutdown().await?;
    drop((_router, _endpoint, store, _dir));
    // Give A's gossip actor a moment to process B's disconnect.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── Phase 2: B comes back with the SAME identity on a fresh endpoint ──
    let (node_b2, pk_b2) = start_node(memory.clone(), id_b, vec![node_a._endpoint.id()], network)
        .await?;
    assert_eq!(pk_b2, pk_b, "restarted node must keep its identity");

    let spy_b2: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_b2 = spawn_spy(&node_b2._gossip, topic, spy_b2.clone()).await?;

    // ── B2 rejoins the discovery topic on startup ────────────────────────
    assert_eq!(
        node_b2.service.topic(),
        topic,
        "restarted node must be joined to the internal discovery topic"
    );
    assert!(
        is_discovery_topic(node_b2.service.topic()),
        "restarted node's topic must classify as the internal discovery topic"
    );
    assert_eq!(
        topic_kind(node_b2.service.topic()),
        TopicKind::Discovery,
        "restarted node's topic must be networking infrastructure, never a conversation"
    );

    // ── B2 reconnects to A: B dials A (known peer), A's drain loop
    //    re-announces a Hello on neighbour-up, B2's registry fills back in ─
    wait_for_peer(&node_b2.service, pk_a, "B2 to re-learn A after restart").await?;
    wait_for_source(
        &node_b2.service,
        pk_a,
        PeerSource::Hello,
        "B2's registry for A (post-restart Hello)",
    )
    .await?;

    // ── A reconnects to B2: B2's post-restart Presence refreshes A's
    //    registry entry for B (proving the mesh carries live discovery
    //    traffic across the restart) ──────────────────────────────────────
    assert_eq!(
        node_b2.service.announce_presence().await?,
        AnnounceOutcome::Announced,
        "B2's presence must be broadcast on the discovery topic"
    );
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Presence,
        "A's registry for B (post-restart presence)",
    )
    .await?;

    // ── No visible lobby chat, no chat payload anywhere ──────────────────
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b2.store, &topic, "B2");
    let spy_a = spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b2 = spy_b2.lock().expect("spy lock poisoned").clone();
    assert_no_chat_payloads(&spy_a, "A spy");
    assert_no_chat_payloads(&spy_b2, "B2 spy");

    node_a.service.shutdown().await;
    node_b2.service.shutdown().await;
    spy_task_a.abort();
    spy_task_b2.abort();
    Ok(())
}

// =========================================================================
// 2. Both start while the other is offline, then one reconnects
// =========================================================================

/// A starts first and is alone. B starts while A is offline — B has no
/// address knowledge of A, so the mesh cannot form and B's discovery
/// registry stays empty. Then A comes back online with B's address known
/// (the persisted known-peer path) and dials B into the discovery mesh; both
/// nodes rediscover each other and exchange live presence. The E2E matrix
/// scenario "both start while the other is offline, then one reconnects".
#[tokio::test]
async fn both_start_offline_then_one_reconnects() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let id_a = seed(0xA1);
    let id_b = seed(0xB1);
    let pk_a = SecretKey::from_bytes(&id_a).public();
    let pk_b = SecretKey::from_bytes(&id_b).public();

    // Separate address books: while A is "offline", B has no address
    // knowledge of A (and vice versa), so no mesh can form.
    let memory_a = MemoryLookup::new();
    let memory_b = MemoryLookup::new();

    // ── Phase 0: A starts alone on the discovery topic ───────────────────
    let (node_a, _) = start_node(memory_a.clone(), id_a, Vec::new(), network).await?;
    let spy_a: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let spy_task_a = spawn_spy(&node_a._gossip, topic, spy_a.clone()).await?;

    // ── Phase 1: B starts while A is OFFLINE — B's address book knows
    //    nothing about A, so no mesh can form and B stays alone ───────────
    let (node_b, _) = start_node(memory_b.clone(), id_b, Vec::new(), network).await?;
    let spy_b: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
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

    // ── Phase 2: one reconnects — A gains B's address (the persisted
    //    known-peer path: a restarted node re-establishes a known friend's
    //    addresses before dialing) and dials B into the discovery mesh. ────
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
        "B's registry for A (post-reconnect presence)",
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
        "A's registry for B (post-reconnect presence)",
    )
    .await?;

    // ── No visible lobby chat, no chat payload anywhere ──────────────────
    assert_no_visible_lobby_chat(&node_a.store, &topic, "A");
    assert_no_visible_lobby_chat(&node_b.store, &topic, "B");
    let spy_a = spy_a.lock().expect("spy lock poisoned").clone();
    let spy_b = spy_b.lock().expect("spy lock poisoned").clone();
    assert_no_chat_payloads(&spy_a, "A spy");
    assert_no_chat_payloads(&spy_b, "B spy");

    node_a.service.shutdown().await;
    node_b.service.shutdown().await;
    spy_task_a.abort();
    spy_task_b.abort();
    Ok(())
}
