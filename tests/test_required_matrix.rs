#![cfg(feature = "net")]

//! # BORU-CP-17: Required test matrix before merge (PDF Phase 7)
//!
//! The PDF's *"Required test matrix before merge"* section lists 12
//! scenarios that must hold before the hidden-discovery control plane is
//! merged. Most scenarios are already covered by the BORU-DISC/BORU-CP
//! suites (see `docs/control-plane/test-matrix.md` for the full
//! scenario → test map). This file adds the integration-level tests for the
//! acceptance criteria that the earlier suites only proved at unit level:
//!
//! | # | PDF scenario | This file |
//! |---|--------------|-----------|
//! | 3/4 | Restart (B, then A) — presence refresh triggers **exactly one** reconnect; direct chat works both directions again | [`restart_triggers_exactly_one_reconnect_and_chat_resumes`] |
//! | 5 | Relay-only path — chat remains bidirectional | [`relay_only_path_chat_bidirectional`] |
//! | 7 | Old client / new client — unknown capabilities ignored; supported baseline chat still works | [`mixed_version_old_client_still_chats`] |
//! | 9 | Duplicate presence flood — no state explosion | [`duplicate_presence_flood_bounded`] |
//! | 11 | Blocked/deleted peer — discovery does not recreate trust or conversation | [`blocked_peer_not_resurrected`] |
//!
//! Everything else maps to existing suites:
//!
//! | # | PDF scenario | Covering test |
//! |---|--------------|---------------|
//! | 1 | Fresh A + Fresh B — both discover, direct-topic ready, A→B and B→A | `test_reconnect_asymmetric::reconnect_asymmetric_messages_flow_both_directions` phase 0; `test_discovery_e2e_matrix::scenario_1/2`; `test_discovery_two_node` |
//! | 2 | B starts later — A discovers B and reconnects automatically; no lobby | `test_discovery_e2e_matrix::scenario_1_a_starts_first_then_b`; `test_reconnect_asymmetric` phase 0 watcher |
//! | 6 | Direct/LAN path — diagnostics show direct | `test_discovery_e2e_matrix::scenario_4_lan_direct_path_available`; `classify_direct_when_any_active_ip_path`; `test_health_view` |
//! | 8 | Malformed discovery packet — dropped safely, bounded logging | `handle_incoming_undecodable_ignored`, `handle_incoming_truncated_payload_ignored_without_panic`, `handle_incoming_control_malformed_dropped`, `counters_malformed_increments_malformed_only`; `test_discovery_ui_isolation`; `test_hostile_input` |
//! | 10 | Peer goes silent — stale/offline after TTL | `expiry_sweep_removes_stale_peers_from_active_presence`; `connectivity_expiry_sweep_marks_peer_offline_stale`; `presence_store_expires_stale_peers_after_ttl` |
//! | 12 | Feature unsupported remotely — declined cleanly, chat unaffected | `capability_gate_handle_reflects_negotiated_support`; app.rs `voice_call_blocked_when_peer_lacks_capability`, `file_send_blocked_when_peer_lacks_capability` |

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use boru_core::{
    api::{Event as GossipEvent, GossipReceiver, GossipSender, GossipTopic},
    chat_callbacks::ChatCallbacks,
    chat_core::{handle_net_event, Message, MessageHash, NetEvent, SignedMessage},
    contact::direct_topic,
    control_plane::message::{ControlEnvelope, CONTROL_PLANE_MAGIC},
    conversations::ConversationStore,
    discovery_message::DiscoveryMessage,
    discovery_service::{DiscoveryService, PeerUpdate, ReconnectSignal},
    discovery_topic::{discovery_topic, is_discovery_topic, topic_kind, TopicKind},
    friends::{FriendId, FriendRecord, FriendRelationship},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room::PublicNetwork,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, tls::CaTlsConfig,
    Endpoint, PublicKey, RelayMode, SecretKey,
};
use n0_error::{bail_any, Result};
use n0_future::StreamExt;
use tempfile::TempDir;
use tokio::{sync::broadcast, task::JoinHandle};

/// How long a two-node mesh may take to (re)form (dial + topic join +
/// rediscovery + reconnect). Generous for CI, but every poll loop exits as
/// soon as its condition is satisfied.
const MESH_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting for the mesh / delivery / state.
const POLL_TICK: Duration = Duration::from_millis(100);
/// How long the survivor gets to process a peer's graceful disconnect
/// (NeighborDown → Degraded) before the restarted node comes back.
const DISCONNECT_WINDOW: Duration = Duration::from_millis(300);
/// Quiet window after a reconnect: no second `PeerReachable` may arrive.
const QUIET_WINDOW: Duration = Duration::from_secs(2);

/// Deterministic 32-byte identity seed from a single byte.
fn seed(byte: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[0] = byte;
    s
}

// ---------------------------------------------------------------------------
// Node spawn helpers
// ---------------------------------------------------------------------------

/// Spawn a fresh in-process node with an **explicit identity** (the same
/// `SecretKey` is reused across a restart): real iroh endpoint (no relay,
/// loopback) with the given address book, plus a gossip actor and protocol
/// router. Mirrors the deterministic harness node setup.
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

/// Spawn a node against a **local relay server** with `RelayMode::Custom` —
/// the relay-only path scenario. The endpoint must reach the relay
/// (`online`) so the relay can route to it.
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
    tokio::time::timeout(Duration::from_secs(10), ep.online())
        .await
        .map_err(|_| n0_error::anyerr!("endpoint did not reach the local relay in time"))?;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), gossip, pk))
}

// ---------------------------------------------------------------------------
// Application layer — a ChatCallbacks recorder
// ---------------------------------------------------------------------------

/// The application layer under test: a [`ChatCallbacks`] implementor that
/// records every remote message delivered by [`handle_net_event`] (the same
/// trait entry point the GUI/TUI frontends use), plus direct-topic neighbor
/// state used as the subscription-readiness signal.
struct Recorder {
    local: PublicKey,
    neighbors: HashSet<PublicKey>,
    /// App-layer delivered texts (push_remote), in arrival order.
    delivered: Vec<String>,
}

impl Recorder {
    fn new(local: PublicKey) -> Self {
        Self {
            local,
            neighbors: HashSet::new(),
            delivered: Vec::new(),
        }
    }
}

impl ChatCallbacks for Recorder {
    fn local_public(&self) -> PublicKey {
        self.local
    }
    fn set_name(&mut self, _peer: PublicKey, name: String) -> Option<String> {
        Some(name)
    }
    fn is_friend(&self, _peer: &PublicKey) -> bool {
        false
    }
    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {}
    fn push_system(&mut self, _text: String) {}
    fn push_remote(
        &mut self,
        _peer: PublicKey,
        _label: String,
        text: String,
        _hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
        self.delivered.push(text);
    }
    fn set_pending_file(
        &mut self,
        _name: String,
        _ticket: String,
        _size: u64,
        _thumbnail_hash: Option<MessageHash>,
        _sender_label: Option<String>,
    ) {
    }
    fn set_pending_image(&mut self, _name: String, _hash: MessageHash, _from: PublicKey) {}
    fn has_message(&self, _hash: &MessageHash) -> bool {
        false
    }
    fn edit_message(&mut self, _hash: &MessageHash, _new_text: String) {}
    fn delete_message(&mut self, _hash: &MessageHash) {}
    fn add_reaction(&mut self, _hash: &MessageHash, _emoji: String) {}
    fn on_neighbor_up(&mut self, _peer: PublicKey) {}
    fn on_neighbor_down(&mut self, _peer: PublicKey) {}
    fn record_activity(&mut self, _peer: PublicKey) {}
    fn request_quit(&mut self) {}
}

/// Drain a gossip receiver into the application layer: decode every payload
/// as a [`SignedMessage`] and route the resulting [`NetEvent`] through
/// [`handle_net_event`] (the frontends' shared entry point). Neighbor events
/// update the recorder's neighbor set.
async fn run_app_layer(mut receiver: GossipReceiver, app: Arc<tokio::sync::Mutex<Recorder>>) {
    while let Ok(Some(event)) = receiver.try_next().await {
        match event {
            GossipEvent::Received(msg) => {
                if let Ok((from, message, sent_at)) = SignedMessage::verify_and_decode(&msg.content)
                {
                    let net_event = NetEvent::Message {
                        from,
                        message,
                        sent_at,
                    };
                    let mut guard = app.lock().await;
                    let _ = handle_net_event(net_event, &mut *guard);
                }
            }
            GossipEvent::NeighborUp(id) => {
                app.lock().await.neighbors.insert(id);
            }
            GossipEvent::NeighborDown(id) => {
                app.lock().await.neighbors.remove(&id);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Node under test (restart scenario)
// ---------------------------------------------------------------------------

/// A live node under test for the restart scenarios: network half + discovery
/// service + the app's BORU-CP-07/08 reconnect wiring (friend watcher →
/// `queue_reconnect` → `PeerReachable` → join the peer back into the live
/// direct-topic sender).
struct TestNode {
    _router: Router,
    endpoint: Endpoint,
    gossip: Gossip,
    service: DiscoveryService,
    sk: SecretKey,
    pk: PublicKey,
    app: Arc<tokio::sync::Mutex<Recorder>>,
    /// The direct-topic sender, shared with the reconnect forwarder task.
    direct_sender: Arc<tokio::sync::Mutex<Option<GossipSender>>>,
    _delivery_task: Option<JoinHandle<()>>,
    _watcher: JoinHandle<()>,
    _forwarder: JoinHandle<()>,
}

/// Start a node with `identity`, join it to the internal discovery topic,
/// and wire the app's automatic-reconnection triggers for `friend` — the
/// startup sequence `src/bin/boru/main.rs` performs on every launch.
///
/// `friend` is the peer this node treats as a known friend (the direct-topic
/// subscription IS the friendship in the restart harness).
async fn start_node(
    memory: MemoryLookup,
    identity: [u8; 32],
    bootstrap: Vec<PublicKey>,
    network: PublicNetwork,
    friend: PublicKey,
) -> Result<TestNode> {
    let sk = SecretKey::from_bytes(&identity);
    let pk = sk.public();
    let (router, ep, gossip, _) = spawn_node(memory.clone(), sk.clone()).await?;
    // A restart gives the endpoint a fresh transient address; the shared
    // address book must learn it (replacing the stale pre-restart entry).
    memory.set_endpoint_info(ep.addr());

    let service = DiscoveryService::join(&gossip, discovery_topic(network), bootstrap, pk, sk.clone())
        .await
        .expect("node joins the internal discovery topic")
        .with_announce_min_interval(Duration::ZERO)
        .with_control_announce_min_interval(Duration::ZERO)
        .with_reconnect_backoff(Duration::from_millis(100), Duration::from_secs(1));

    let app = Arc::new(tokio::sync::Mutex::new(Recorder::new(pk)));
    let direct_sender: Arc<tokio::sync::Mutex<Option<GossipSender>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // ── BORU-CP-07 friend watcher (mirror of main.rs): a fresh discovery
    //    announcement of the known friend queues ONE reconnect attempt.
    let reconnect_handle = service.reconnect_handle();
    let mut peer_updates = service.peer_updates();
    let watcher = tokio::spawn(async move {
        loop {
            match peer_updates.recv().await {
                Ok(PeerUpdate::Seen { node_id, .. }) if node_id == friend => {
                    reconnect_handle.queue_reconnect(node_id);
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // ── BORU-CP-07/08 reconnect signal forwarder (mirror of main.rs +
    //    ReconnectPeerReady): on PeerReachable, join the friend back into
    //    the live direct-topic sender (the data-plane action the discovery
    //    service never performs itself).
    let mut reconnect_events = service.reconnect_events();
    let fwd_sender = direct_sender.clone();
    let forwarder = tokio::spawn(async move {
        loop {
            match reconnect_events.recv().await {
                Ok(ReconnectSignal::PeerReachable { peer }) => {
                    if let Some(sender) = fwd_sender.lock().await.clone() {
                        let _ = sender.join_peers(vec![peer]).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    Ok(TestNode {
        _router: router,
        endpoint: ep,
        gossip,
        service,
        sk,
        pk,
        app,
        direct_sender,
        _delivery_task: None,
        _watcher: watcher,
        _forwarder: forwarder,
    })
}

impl TestNode {
    /// Subscribe to the deterministic direct topic — the OpenFriendChat →
    /// BackgroundSubscribe pattern, step 1. Both sides must be subscribed
    /// before either side's join can complete (the swarm edge forms only
    /// when both actors know the topic).
    async fn begin_direct(&self, topic: TopicId, bootstrap: Vec<PublicKey>) -> Result<GossipTopic> {
        Ok(self.gossip.subscribe(topic, bootstrap).await?)
    }

    /// Finish a direct-topic subscription: wait for the swarm join, then
    /// split into the sender (kept for broadcast + reconnects) and the
    /// app-layer delivery task.
    async fn finish_direct(&mut self, mut sub: GossipTopic) -> Result<()> {
        match tokio::time::timeout(MESH_TIMEOUT, sub.joined()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => bail_any!("direct-topic join failed: {e}"),
            Err(_) => bail_any!("timed out waiting for direct-topic join"),
        }
        let (sender, receiver) = sub.split();
        *self.direct_sender.lock().await = Some(sender.clone());
        {
            let mut guard = self.app.lock().await;
            for id in receiver.neighbors() {
                guard.neighbors.insert(id);
            }
        }
        let app = self.app.clone();
        let task = tokio::spawn(run_app_layer(receiver, app));
        self._delivery_task = Some(task);
        Ok(())
    }

    /// Graceful shutdown (the app's normal stop path).
    async fn shutdown(self) {
        let TestNode {
            _router,
            endpoint,
            gossip,
            service,
            sk: _sk,
            pk: _pk,
            app: _app,
            direct_sender,
            _delivery_task,
            _watcher,
            _forwarder,
        } = self;
        if let Some(task) = _delivery_task {
            task.abort();
        }
        drop(direct_sender);
        service.shutdown().await;
        let _ = gossip.shutdown().await;
        let _ = endpoint.close().await;
        drop(_watcher);
        drop(_forwarder);
        drop((_router, endpoint));
    }
}

// ---------------------------------------------------------------------------
// Wait / assertion helpers
// ---------------------------------------------------------------------------

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

/// Wait until a gossip topic subscription has joined the swarm (at least one
/// `NeighborUp`) so broadcasts are not lost to the empty-mesh trap.
async fn wait_for_joined(sub: &mut GossipTopic, what: &str) -> Result<()> {
    match tokio::time::timeout(MESH_TIMEOUT, sub.joined()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => bail_any!("timed out waiting for {what} to join"),
    }
}

/// Wait until the node's direct-topic neighbor set contains `peer` — the
/// subscription-readiness signal that the direct-topic mesh edge exists.
async fn wait_for_neighbor(node: &TestNode, peer: PublicKey, what: &str) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if node.app.lock().await.neighbors.contains(&peer) {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    let neigh: Vec<String> = node
        .app
        .lock()
        .await
        .neighbors
        .iter()
        .map(|id| id.fmt_short().to_string())
        .collect();
    bail_any!("timed out waiting for {what}: direct-topic neighbors {neigh:?}")
}

/// Sign a text message as `from`, broadcast it on `from`'s direct-topic
/// sender, then wait until `to`'s application layer has delivered it.
async fn send_and_assert(
    from: &TestNode,
    to: &TestNode,
    text: &str,
    direction: &str,
) -> Result<()> {
    let payload = SignedMessage::sign_and_encode(&from.sk, &Message::Message { text: text.into() })
        .map_err(|e| n0_error::AnyError::from_string(format!("{direction}: sign failed: {e}")))?;
    let sender = from.direct_sender.lock().await.clone().ok_or_else(|| {
        n0_error::AnyError::from_string(format!(
            "{direction}: no direct-topic sender — subscription not ready"
        ))
    })?;
    sender.broadcast(payload).await.map_err(|e| {
        n0_error::AnyError::from_string(format!("{direction}: broadcast attempt failed: {e}"))
    })?;

    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if to.app.lock().await.delivered.iter().any(|t| t == text) {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    bail_any!("{direction}: application delivery timed out for {text:?}")
}

/// Assert the restart-reconnect acceptance on the survivor side:
///
/// 1. The restarted peer comes back **online automatically** (the
///    reconnect happened — either the app's queued reconnect loop or the
///    mesh self-heal via the restarted peer's bootstrap dial; both are
///    legitimate "one reconnect" paths).
/// 2. There is **exactly one** connectivity entry for the peer (no
///    duplicate connections / state).
/// 3. No **second** `PeerReachable` signal arrives within
///    [`QUIET_WINDOW`] — presence refresh is deduplicated (the "exactly
///    one reconnect" acceptance; unit-level dedup is proven by
///    `reconnect_queue_queues_once_and_skips_online` and
///    `schedule_deduplicates_per_peer`).
async fn expect_reconnected_once(
    node: &TestNode,
    events: &mut broadcast::Receiver<ReconnectSignal>,
    peer: PublicKey,
    what: &str,
) -> Result<()> {
    // (1) Automatic reconnection: the peer's connectivity returns online.
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if node.service.connectivity_state(&peer).is_online() {
            break;
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    assert!(
        node.service.connectivity_state(&peer).is_online(),
        "{what}: restarted peer must come back online automatically"
    );

    // (2) Exactly one connectivity entry — no duplicate connections.
    let connectivity = node.service.connectivity_peers();
    let conn: Vec<_> = connectivity.iter().filter(|(id, _)| *id == peer).collect();
    assert_eq!(
        conn.len(),
        1,
        "{what}: exactly one connectivity entry for the restarted peer, got {}",
        conn.len()
    );

    // (3) No second reconnect signal in the quiet window (dedup). A first
    // signal may or may not have fired depending on which reconnect path
    // won the race; a SECOND one is the violation.
    let quiet = Instant::now() + QUIET_WINDOW;
    while Instant::now() < quiet {
        match tokio::time::timeout(POLL_TICK, events.recv()).await {
            Ok(Ok(ReconnectSignal::PeerReachable { peer: p })) if p == peer => {
                bail_any!("{what}: second reconnect signal arrived (dedup violated)")
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
    Ok(())
}

// =========================================================================
// Scenarios 3 + 4 — restart triggers exactly one reconnect; chat resumes
// =========================================================================

/// A and B start, discover, and chat both directions. B restarts (same
/// identity); the survivor A receives **exactly one** `PeerReachable`
/// reconnect signal (presence refresh is deduplicated), and direct chat
/// works both directions again. Then A restarts; the cycle repeats with the
/// roles reversed.
#[tokio::test]
async fn restart_triggers_exactly_one_reconnect_and_chat_resumes() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .try_init();

    let network = PublicNetwork::Test;
    let id_a = seed(0xD1);
    let id_b = seed(0xD2);
    let pk_a = SecretKey::from_bytes(&id_a).public();
    let pk_b = SecretKey::from_bytes(&id_b).public();
    let direct = direct_topic(&pk_a, &pk_b);

    let memory = MemoryLookup::new();

    // ── Phase 0: A and B start ─────────────────────────────────────────
    let mut node_a = start_node(memory.clone(), id_a, Vec::new(), network, pk_b).await?;
    let mut node_b = start_node(memory.clone(), id_b, vec![pk_a], network, pk_a).await?;

    wait_for_peer(&node_a.service, pk_b, "A to learn B (phase 0)").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A (phase 0)").await?;

    let sub_a = node_a.begin_direct(direct, vec![pk_b]).await?;
    let sub_b = node_b.begin_direct(direct, vec![pk_a]).await?;
    node_a.finish_direct(sub_a).await?;
    node_b.finish_direct(sub_b).await?;
    wait_for_neighbor(&node_a, pk_b, "A direct-topic neighbor B (phase 0)").await?;
    wait_for_neighbor(&node_b, pk_a, "B direct-topic neighbor A (phase 0)").await?;

    send_and_assert(&node_a, &node_b, "phase0 A→B", "phase 0 A→B").await?;
    send_and_assert(&node_b, &node_a, "phase0 B→A", "phase 0 B→A").await?;

    // ── Phase 1: B restarts; exactly one reconnect on A ────────────────
    // Subscribe BEFORE the restart so no PeerReachable signal is missed.
    let mut reconnect_events_a = node_a.service.reconnect_events();
    node_b.shutdown().await;
    tokio::time::sleep(DISCONNECT_WINDOW).await;

    let mut node_b = start_node(memory.clone(), id_b, vec![pk_a], network, pk_a).await?;
    wait_for_peer(&node_b.service, pk_a, "B2 to rediscover A (phase 1)").await?;
    wait_for_peer(&node_a.service, pk_b, "A to rediscover B (phase 1)").await?;

    // The auto-reconnect path fires once (dedup), then chat resumes.
    expect_reconnected_once(
        &node_a,
        &mut reconnect_events_a,
        pk_b,
        "A reconnect for B (phase 1)",
    )
    .await?;

    let sub_b2 = node_b.begin_direct(direct, vec![pk_a]).await?;
    node_b.finish_direct(sub_b2).await?;
    wait_for_neighbor(&node_a, pk_b, "A direct-topic neighbor B2 (phase 1)").await?;
    wait_for_neighbor(&node_b, pk_a, "B2 direct-topic neighbor A (phase 1)").await?;

    send_and_assert(&node_a, &node_b, "phase1 A→B", "phase 1 A→B").await?;
    send_and_assert(&node_b, &node_a, "phase1 B→A", "phase 1 B→A").await?;

    // ── Phase 2: A restarts; exactly one reconnect on B ────────────────
    // Subscribe BEFORE the restart so no PeerReachable signal is missed.
    let mut reconnect_events_b = node_b.service.reconnect_events();
    node_a.shutdown().await;
    tokio::time::sleep(DISCONNECT_WINDOW).await;

    let mut node_a = start_node(memory.clone(), id_a, vec![pk_b], network, pk_b).await?;
    wait_for_peer(&node_a.service, pk_b, "A2 to rediscover B (phase 2)").await?;
    wait_for_peer(&node_b.service, pk_a, "B to rediscover A (phase 2)").await?;

    expect_reconnected_once(
        &node_b,
        &mut reconnect_events_b,
        pk_a,
        "B reconnect for A (phase 2)",
    )
    .await?;

    let sub_a2 = node_a.begin_direct(direct, vec![pk_b]).await?;
    node_a.finish_direct(sub_a2).await?;
    wait_for_neighbor(&node_a, pk_b, "A2 direct-topic neighbor B (phase 2)").await?;
    wait_for_neighbor(&node_b, pk_a, "B direct-topic neighbor A2 (phase 2)").await?;

    send_and_assert(&node_a, &node_b, "phase2 A→B", "phase 2 A→B").await?;
    send_and_assert(&node_b, &node_a, "phase2 B→A", "phase 2 B→A").await?;

    // ── Domain separation: the direct topic is NOT the discovery topic ──
    let discovery = discovery_topic(network);
    assert_ne!(direct, discovery);
    assert_eq!(topic_kind(direct), TopicKind::Conversation);
    assert!(is_discovery_topic(discovery));

    node_a.shutdown().await;
    node_b.shutdown().await;
    Ok(())
}

// =========================================================================
// Scenario 5 — relay-only path, chat remains bidirectional
// =========================================================================

/// Both nodes run against a local relay server with ONLY relay addresses in
/// the shared address book (LAN direct structurally unavailable). Discovery
/// works over the relay, both direct-topic subscriptions join through the
/// relay, and A→B / B→A chat messages are delivered at the application
/// layer.
#[tokio::test]
async fn relay_only_path_chat_bidirectional() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let (relay_map, _relay_url, _relay_guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("local relay server");
    let memory = MemoryLookup::new();

    let id_a = seed(0xE1);
    let id_b = seed(0xE2);
    let pk_a = SecretKey::from_bytes(&id_a).public();
    let pk_b = SecretKey::from_bytes(&id_b).public();
    let direct = direct_topic(&pk_a, &pk_b);

    let (router_a, ep_a, gossip_a, _) = spawn_node_relay(
        memory.clone(),
        SecretKey::from_bytes(&id_a),
        relay_map.clone(),
    )
    .await?;
    let (router_b, ep_b, gossip_b, _) = spawn_node_relay(
        memory.clone(),
        SecretKey::from_bytes(&id_b),
        relay_map.clone(),
    )
    .await?;

    // Relay-only proof: the address book knows only relay addresses.
    for (ep, name) in [(&ep_a, "A"), (&ep_b, "B")] {
        let mut addr = ep.addr();
        addr.addrs.retain(|a| a.is_relay());
        assert!(
            !addr.addrs.is_empty() && addr.addrs.iter().all(|a| a.is_relay()),
            "{name}: address book must be relay-only"
        );
        memory.set_endpoint_info(addr);
    }

    let service_a = DiscoveryService::join(&gossip_a, topic, Vec::new(), pk_a, SecretKey::from_bytes(&id_a))
        .await?
        .with_announce_min_interval(Duration::ZERO)
        .with_control_announce_min_interval(Duration::ZERO);
    let service_b = DiscoveryService::join(&gossip_b, topic, vec![ep_a.id()], pk_b, SecretKey::from_bytes(&id_b))
        .await?
        .with_announce_min_interval(Duration::ZERO)
        .with_control_announce_min_interval(Duration::ZERO);

    wait_for_peer(&service_a, pk_b, "A to discover B via the relay").await?;
    wait_for_peer(&service_b, pk_a, "B to discover A via the relay").await?;

    // Both sides subscribe the direct topic (through the relay).
    let mut sub_a = gossip_a.subscribe(direct, vec![ep_b.id()]).await?;
    let mut sub_b = gossip_b.subscribe(direct, vec![ep_a.id()]).await?;
    wait_for_joined(&mut sub_a, "A direct-topic join over relay").await?;
    wait_for_joined(&mut sub_b, "B direct-topic join over relay").await?;
    let (sender_a, mut rx_a) = sub_a.split();
    let (sender_b, mut rx_b) = sub_b.split();

    // Bidirectional chat at the wire level (signed messages arrive).
    let text_ab = "relay A→B";
    sender_a
        .broadcast(
            SignedMessage::sign_and_encode(
                &SecretKey::from_bytes(&id_a),
                &Message::Message {
                    text: text_ab.into(),
                },
            )
            .expect("sign")
            .into(),
        )
        .await?;
    let deadline = Instant::now() + MESH_TIMEOUT;
    let mut got_ab = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(POLL_TICK, rx_b.try_next()).await {
            Ok(Ok(Some(GossipEvent::Received(msg)))) => {
                if let Ok((_, Message::Message { text }, _)) =
                    SignedMessage::verify_and_decode(&msg.content)
                {
                    if text == text_ab {
                        got_ab = true;
                        break;
                    }
                }
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {}
        }
    }
    assert!(got_ab, "B must receive A's message over the relay path");
    drop(rx_b);

    let text_ba = "relay B→A";
    sender_b
        .broadcast(
            SignedMessage::sign_and_encode(
                &SecretKey::from_bytes(&id_b),
                &Message::Message {
                    text: text_ba.into(),
                },
            )
            .expect("sign")
            .into(),
        )
        .await?;
    let deadline = Instant::now() + MESH_TIMEOUT;
    let mut got_ba = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(POLL_TICK, rx_a.try_next()).await {
            Ok(Ok(Some(GossipEvent::Received(msg)))) => {
                if let Ok((_, Message::Message { text }, _)) =
                    SignedMessage::verify_and_decode(&msg.content)
                {
                    if text == text_ba {
                        got_ba = true;
                        break;
                    }
                }
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {}
        }
    }
    assert!(got_ba, "A must receive B's message over the relay path");
    drop(rx_a);

    service_a.shutdown().await;
    service_b.shutdown().await;
    drop((router_a, router_b, ep_a, ep_b, gossip_a, gossip_b));
    Ok(())
}

// =========================================================================
// Scenario 7 — old client / new client: unknown capabilities ignored
// =========================================================================

/// A is a full (new) client with control-plane capabilities. B is an **old
/// client**: it speaks only the legacy discovery protocol (raw
/// [`DiscoveryMessage`] Hello/Presence on the discovery topic, no
/// control-plane envelopes, no capabilities). A discovers B, A's capability
/// gate fails closed for B (no capabilities → `None`), and the supported
/// baseline chat still works in both directions on the deterministic direct
/// topic.
#[tokio::test]
async fn mixed_version_old_client_still_chats() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    let id_a = seed(0xF1);
    let id_b = seed(0xF2);
    let pk_a = SecretKey::from_bytes(&id_a).public();
    let pk_b = SecretKey::from_bytes(&id_b).public();
    let direct = direct_topic(&pk_a, &pk_b);

    // ── New client A: full DiscoveryService ────────────────────────────
    let (router_a, ep_a, gossip_a, _) =
        spawn_node(memory.clone(), SecretKey::from_bytes(&id_a)).await?;
    memory.set_endpoint_info(ep_a.addr());
    let service_a = DiscoveryService::join(&gossip_a, topic, Vec::new(), pk_a, SecretKey::from_bytes(&id_a))
        .await?
        .with_announce_min_interval(Duration::ZERO)
        .with_control_announce_min_interval(Duration::ZERO);

    // ── Old client B: raw gossip only, legacy discovery messages ───────
    let (router_b, ep_b, gossip_b, _) =
        spawn_node(memory.clone(), SecretKey::from_bytes(&id_b)).await?;
    memory.set_endpoint_info(ep_b.addr());
    let mut old_b = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;
    // Wait for the swarm edge so the legacy broadcast actually reaches A.
    wait_for_joined(&mut old_b, "old client B discovery join").await?;
    let (old_b_sender, _old_b_rx) = old_b.split();

    // B announces via the LEGACY wire protocol only (Hello + Presence, no
    // control-plane envelopes — an old client cannot speak BORU-CP-01+).
    old_b_sender
        .broadcast(
            postcard::to_stdvec(&DiscoveryMessage::hello_with_event(pk_b, 1))
                .unwrap()
                .into(),
        )
        .await?;
    old_b_sender
        .broadcast(
            postcard::to_stdvec(&DiscoveryMessage::presence_with_event(pk_b, 2))
                .unwrap()
                .into(),
        )
        .await?;

    // A discovers B through the legacy path.
    wait_for_peer(&service_a, pk_b, "A to discover the old client B").await?;

    // A's capability gate fails closed: B never advertised capabilities.
    assert_eq!(
        service_a.peer_capabilities(&pk_b).map(|_| ()),
        None,
        "old client must have no capability set cached"
    );
    assert_eq!(
        service_a.peer_supports(
            &pk_b,
            boru_core::control_plane::capabilities::features::VOICE
        ),
        None,
        "feature negotiation must fail closed for an old client"
    );

    // ── Baseline chat works both directions ────────────────────────────
    // A subscribes the direct topic; B subscribes it as a raw old client.
    // Both subscriptions join the swarm first (both actors must know the
    // topic before either side's mesh edge forms).
    let mut sub_direct_a = gossip_a.subscribe(direct, vec![ep_b.id()]).await?;
    let mut sub_direct_b = gossip_b.subscribe(direct, vec![ep_a.id()]).await?;
    wait_for_joined(&mut sub_direct_a, "A direct-topic join (mixed version)").await?;
    wait_for_joined(&mut sub_direct_b, "B direct-topic join (mixed version)").await?;
    let (sender_a, mut rx_a) = sub_direct_a.split();
    let (sender_b_old, mut rx_b) = sub_direct_b.split();

    // A→B: A signs and broadcasts; B (raw) verifies and decodes.
    sender_a
        .broadcast(
            SignedMessage::sign_and_encode(
                &SecretKey::from_bytes(&id_a),
                &Message::Message {
                    text: "new→old".into(),
                },
            )
            .expect("sign")
            .into(),
        )
        .await?;
    let deadline = Instant::now() + MESH_TIMEOUT;
    let mut got_ab = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(POLL_TICK, rx_b.try_next()).await {
            Ok(Ok(Some(GossipEvent::Received(msg)))) => {
                if let Ok((_, Message::Message { text }, _)) =
                    SignedMessage::verify_and_decode(&msg.content)
                {
                    if text == "new→old" {
                        got_ab = true;
                        break;
                    }
                }
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {}
        }
    }
    assert!(
        got_ab,
        "old client B must receive A's baseline chat message"
    );
    drop(rx_b);

    // B→A: the old client signs and broadcasts; A decodes (the app layer
    // accepts any valid SignedMessage regardless of capabilities).
    sender_b_old
        .broadcast(
            SignedMessage::sign_and_encode(
                &SecretKey::from_bytes(&id_b),
                &Message::Message {
                    text: "old→new".into(),
                },
            )
            .expect("sign")
            .into(),
        )
        .await?;
    let deadline = Instant::now() + MESH_TIMEOUT;
    let mut got_ba = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(POLL_TICK, rx_a.try_next()).await {
            Ok(Ok(Some(GossipEvent::Received(msg)))) => {
                if let Ok((_, Message::Message { text }, _)) =
                    SignedMessage::verify_and_decode(&msg.content)
                {
                    if text == "old→new" {
                        got_ba = true;
                        break;
                    }
                }
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {}
        }
    }
    assert!(got_ba, "new client A must receive the old client's message");

    drop(sender_a);
    drop(sender_b_old);
    drop(old_b_sender);
    service_a.shutdown().await;
    drop((router_a, router_b, ep_a, ep_b, gossip_a, gossip_b));
    Ok(())
}

// =========================================================================
// Scenario 9 — duplicate presence flood: bounded state
// =========================================================================

/// A runs a full discovery service. A raw attacker node floods the
/// discovery topic with duplicate control-plane PRESENCE envelopes (same
/// sender + sequence) plus a legacy Hello. Dedup + the per-sender rate
/// limiter keep the state bounded: exactly one legacy registry entry, one
/// control-plane presence entry, and one connectivity entry — no duplicate
/// connections, no state explosion.
#[tokio::test]
async fn duplicate_presence_flood_bounded() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    let id_a = seed(0xA1);
    let id_b = seed(0xA2);
    let pk_a = SecretKey::from_bytes(&id_a).public();
    let pk_b = SecretKey::from_bytes(&id_b).public();

    let (router_a, ep_a, gossip_a, _) =
        spawn_node(memory.clone(), SecretKey::from_bytes(&id_a)).await?;
    memory.set_endpoint_info(ep_a.addr());
    let service_a = DiscoveryService::join(&gossip_a, topic, Vec::new(), pk_a, SecretKey::from_bytes(&id_a))
        .await?
        .with_announce_min_interval(Duration::ZERO)
        .with_control_announce_min_interval(Duration::ZERO);

    // ── Attacker node B: raw gossip, floods the discovery topic ─────────
    let (router_b, ep_b, gossip_b, _) =
        spawn_node(memory.clone(), SecretKey::from_bytes(&id_b)).await?;
    memory.set_endpoint_info(ep_b.addr());
    let mut flood = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;
    // Wait for the swarm edge so the flood actually reaches A.
    wait_for_joined(&mut flood, "flood sender discovery join").await?;
    let (flood_sender, _flood_rx) = flood.split();

    // One legacy Hello registers B once.
    flood_sender
        .broadcast(
            postcard::to_stdvec(&DiscoveryMessage::hello_with_event(pk_b, 1))
                .unwrap()
                .into(),
        )
        .await?;
    wait_for_peer(&service_a, pk_b, "A to register the flooding peer").await?;

    // 200 duplicate control-plane PRESENCE envelopes, same (sender,
    // sequence) — the dedup key. The per-sender rate limiter (60 frames /
    // 10s window) also rejects the surplus.
    let envelope = ControlEnvelope::presence(pk_b, 0xC0FFEE, 1_700_000_000, Some(300)).encode();
    assert!(
        envelope.starts_with(&CONTROL_PLANE_MAGIC),
        "flood payload must be a control-plane envelope"
    );
    for _ in 0..200 {
        flood_sender.broadcast(envelope.clone().into()).await?;
    }

    // Give A's drain loop a moment to process the flood.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Bounded state: exactly one registry entry, one control presence
    // entry, one connectivity entry.
    let known = service_a.known_peers();
    let b_entries: Vec<_> = known.iter().filter(|(id, _)| *id == pk_b).collect();
    assert_eq!(
        b_entries.len(),
        1,
        "duplicate presence flood must not duplicate registry entries"
    );
    assert_eq!(service_a.peer_count(), 1, "registry must stay bounded");

    let control = service_a.control_presence_peers();
    let b_control: Vec<_> = control.iter().filter(|(id, _)| *id == pk_b).collect();
    assert_eq!(
        b_control.len(),
        1,
        "duplicate control presence flood must dedup to one entry"
    );

    let connectivity = service_a.connectivity_peers();
    let b_conn: Vec<_> = connectivity.iter().filter(|(id, _)| *id == pk_b).collect();
    assert_eq!(
        b_conn.len(),
        1,
        "duplicate presence flood must not create duplicate connectivity entries"
    );

    service_a.shutdown().await;
    drop(flood_sender);
    drop((router_a, router_b, ep_a, ep_b, gossip_a, gossip_b));
    Ok(())
}

// =========================================================================
// Scenario 11 — blocked/deleted peer is not resurrected
// =========================================================================

/// A has B **blocked** (friend record `Blocked`). B joins the discovery
/// topic and announces. A's friend watcher (the main.rs mirror: only
/// message-capable friends queue reconnects) must NOT queue a reconnect, no
/// `PeerReachable` signal may fire, and no conversation may be created —
/// discovery never resurrects a blocked trust relationship.
#[tokio::test]
async fn blocked_peer_not_resurrected() -> Result<()> {
    let network = PublicNetwork::Test;
    let topic = discovery_topic(network);
    let memory = MemoryLookup::new();

    let id_a = seed(0xB1);
    let id_b = seed(0xB2);
    let pk_a = SecretKey::from_bytes(&id_a).public();
    let pk_b = SecretKey::from_bytes(&id_b).public();

    // ── Node A: full service + blocked friend record + main.rs watcher ──
    let (router_a, ep_a, gossip_a, _) =
        spawn_node(memory.clone(), SecretKey::from_bytes(&id_a)).await?;
    memory.set_endpoint_info(ep_a.addr());
    let service_a = DiscoveryService::join(&gossip_a, topic, Vec::new(), pk_a, SecretKey::from_bytes(&id_a))
        .await?
        .with_announce_min_interval(Duration::ZERO)
        .with_control_announce_min_interval(Duration::ZERO);

    // A's friend store: B is BLOCKED.
    let mut blocked_record = FriendRecord::default();
    blocked_record.relationship = FriendRelationship::Blocked;
    let friends: Arc<std::sync::Mutex<HashMap<FriendId, FriendRecord>>> =
        Arc::new(std::sync::Mutex::new(
            [(FriendId::from_public_key(pk_b), blocked_record)]
                .into_iter()
                .collect(),
        ));

    // main.rs watcher mirror: queue reconnect ONLY for message-capable
    // friends. Blocked → never queued.
    let reconnect_handle = service_a.reconnect_handle();
    let mut peer_updates = service_a.peer_updates();
    let watcher_friends = friends.clone();
    let watcher = tokio::spawn(async move {
        loop {
            match peer_updates.recv().await {
                Ok(PeerUpdate::Seen { node_id, .. }) => {
                    let can_message = watcher_friends
                        .lock()
                        .expect("friends lock")
                        .get(&FriendId::from_public_key(node_id))
                        .is_some_and(|record| record.relationship.can_message());
                    if can_message {
                        reconnect_handle.queue_reconnect(node_id);
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // ── Node B: joins the discovery topic and announces (fresh presence) ─
    let (router_b, ep_b, gossip_b, _) =
        spawn_node(memory.clone(), SecretKey::from_bytes(&id_b)).await?;
    memory.set_endpoint_info(ep_b.addr());
    let service_b = DiscoveryService::join(&gossip_b, topic, vec![ep_a.id()], pk_b, SecretKey::from_bytes(&id_b))
        .await?
        .with_announce_min_interval(Duration::ZERO)
        .with_control_announce_min_interval(Duration::ZERO);

    // B is seen by A (discovery works — it just must not resurrect trust).
    wait_for_peer(&service_a, pk_b, "A to see the blocked peer B").await?;

    // Give the watcher + reconnect machinery time to (not) act.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // No reconnect queued for the blocked peer.
    assert!(
        service_a.reconnect_state(&pk_b).is_none(),
        "blocked peer must never be queued for reconnection"
    );
    // No PeerReachable signal ever fired.
    let mut events = service_a.reconnect_events();
    let silent = tokio::time::timeout(Duration::from_millis(500), events.recv()).await;
    assert!(
        matches!(silent, Err(_)),
        "no reconnect signal may fire for a blocked peer"
    );

    // No conversation was created by discovery.
    let store = ConversationStore::empty_at(TempDir::new().expect("temp dir").path());
    assert_eq!(store.len(), 0, "discovery must never create a conversation");

    // And the peer's connectivity state, if any, is not 'ready for direct'
    // (discovery alone grants nothing).
    let conn = service_a.connectivity_state(&pk_b);
    assert!(
        !conn.is_ready_for_direct(),
        "discovery must not make a blocked peer direct-topic ready"
    );

    service_a.shutdown().await;
    service_b.shutdown().await;
    watcher.abort();
    drop((router_a, router_b, ep_a, ep_b, gossip_a, gossip_b));
    Ok(())
}
