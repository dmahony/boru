#![cfg(feature = "net")]

//! # Reconnect asymmetric-message test (PDF Task 3.3 / BORU-CP-09)
//!
//! Two real in-process nodes, A and B, start with **fixed identities** (the
//! same `SecretKey` is reused across a restart — a real restart keeps the
//! node's key), discover each other on the internal discovery gossip topic,
//! and both subscribe to the deterministic direct topic
//! [`direct_topic`]`(pk_a, pk_b)` — the data plane. The test then proves
//! **bidirectional direct-topic delivery at the application layer** (a
//! decoded [`SignedMessage`] routed through [`handle_net_event`] into a
//! [`ChatCallbacks`] recorder) before AND after either node restarts:
//!
//! 1. **Phase 0** — A and B start, rediscover via the discovery mesh, and
//!    both direct-topic subscriptions become ready. A→B and B→A are both
//!    asserted at the application layer.
//! 2. **Phase 1** — B restarts (graceful shutdown, same identity, fresh
//!    endpoint). The restarted B2 rejoins discovery and re-subscribes the
//!    direct topic (the startup path); A mirrors the app's
//!    `ReconnectPeerReady` handler (BORU-CP-07/08) and re-joins the friend
//!    into its live direct-topic sender. Rediscovery is asserted, then both
//!    directions are repeated and asserted again.
//! 3. **Phase 2** — A restarts; the same rediscovery/reconnect/delivery
//!    cycle runs with the roles reversed.
//!
//! ## Per-side diagnostics
//!
//! Every node records a stage log covering the PDF's required stages —
//! **discovery**, **endpoint connectivity** (the automatic
//! `PeerReachable` reconnect signal and the `ReconnectPeerReady` join),
//! **topic join** (NeighborUp on the direct topic), **subscription
//! readiness**, **broadcast attempt**, **gossip receipt**, **decode**, and
//! **application delivery** (`push_remote`). On failure the error identifies
//! the exact stage where a direction stopped.
//!
//! ## No discovery-as-delivery
//!
//! The application layer only ever reads the **direct-topic** gossip
//! receiver; the discovery topic is used strictly as control-plane
//! infrastructure (registry + reconnect trigger). A delivered message can
//! only have arrived on the direct topic — discovery messages are never a
//! substitute for direct-topic delivery. The domain-separation assertion at
//! the end re-checks the two topics are distinct classes.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use boru_core::{
    api::{Event as GossipEvent, GossipReceiver, GossipSender, GossipTopic},
    chat_callbacks::ChatCallbacks,
    chat_core::{handle_net_event, Message, MessageHash, NetEvent, SignedMessage},
    contact::direct_topic,
    discovery_service::{DiscoveryService, PeerSource, PeerUpdate, ReconnectSignal},
    discovery_topic::{discovery_topic, is_discovery_topic, topic_kind, TopicKind},
    friends::FriendId,
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
use tokio::{sync::broadcast, task::JoinHandle};

/// How long a two-node mesh may take to (re)form (dial + topic join +
/// rediscovery + reconnect). Generous for CI, but every poll loop exits as
/// soon as its condition is satisfied.
const MESH_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting for the mesh / delivery.
const POLL_TICK: Duration = Duration::from_millis(100);
/// How long the survivor gets to process a peer's graceful disconnect
/// (NeighborDown → Degraded) before the restarted node comes back.
const DISCONNECT_WINDOW: Duration = Duration::from_millis(300);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Deterministic 32-byte identity seed from a single byte.
fn seed(byte: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[0] = byte;
    s
}

/// Append a human-readable stage line to a node's diagnostic log.
fn log_push(log: &Arc<Mutex<Vec<String>>>, stage: &str, detail: impl AsRef<str>) {
    log.lock()
        .expect("stage log lock poisoned")
        .push(format!("{stage}: {}", detail.as_ref()));
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

// ---------------------------------------------------------------------------
// Application layer — a ChatCallbacks recorder
// ---------------------------------------------------------------------------

/// The application layer under test: a [`ChatCallbacks`] implementor that
/// records every remote message delivered by [`handle_net_event`] (the same
/// trait entry point the GUI/TUI frontends use), plus direct-topic neighbor
/// state used as the subscription-readiness signal.
struct Recorder {
    local: PublicKey,
    names: HashMap<PublicKey, String>,
    neighbors: HashSet<PublicKey>,
    /// App-layer delivered texts (push_remote), in arrival order.
    delivered: Vec<String>,
    sys: Vec<String>,
    log: Option<Arc<Mutex<Vec<String>>>>,
}

impl Recorder {
    fn new(local: PublicKey, log: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            local,
            names: HashMap::new(),
            neighbors: HashSet::new(),
            delivered: Vec::new(),
            sys: Vec::new(),
            log: Some(log),
        }
    }
}

impl ChatCallbacks for Recorder {
    fn local_public(&self) -> PublicKey {
        self.local
    }
    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.insert(peer, name)
    }
    fn is_friend(&self, _peer: &PublicKey) -> bool {
        // The test's friendship is expressed by the direct-topic
        // subscription itself (OpenFriendChat → BackgroundSubscribe), not
        // by the friend store — `push_remote` is unconditional for a
        // signed chat message.
        false
    }
    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {}
    fn push_system(&mut self, text: String) {
        self.sys.push(text);
    }
    fn push_remote(
        &mut self,
        peer: PublicKey,
        _label: String,
        text: String,
        _hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
        self.delivered.push(text.clone());
        if let Some(log) = &self.log {
            log.lock().expect("stage log lock poisoned").push(format!(
                "application_delivery: push_remote from {} text={text:?}",
                peer.fmt_short()
            ));
        }
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
        let _ = _hash;
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

/// Drain the direct-topic gossip receiver into the application layer:
/// decode every payload as a [`SignedMessage`], route the resulting
/// [`NetEvent`] through [`handle_net_event`], and record the per-stage
/// diagnostics (gossip receipt → decode → application delivery). Neighbor
/// events update the recorder's neighbor set (subscription-readiness /
/// topic-join signal).
async fn run_app_layer(
    mut receiver: GossipReceiver,
    app: Arc<tokio::sync::Mutex<Recorder>>,
    log: Arc<Mutex<Vec<String>>>,
) {
    while let Ok(Some(event)) = receiver.try_next().await {
        match event {
            GossipEvent::Received(msg) => {
                log_push(
                    &log,
                    "gossip_receipt",
                    format!("{} bytes on the direct topic", msg.content.len()),
                );
                match SignedMessage::verify_and_decode(&msg.content) {
                    Ok((from, message, sent_at)) => {
                        log_push(
                            &log,
                            "decode",
                            format!("SignedMessage verified, from {}", from.fmt_short()),
                        );
                        let net_event = NetEvent::Message {
                            from,
                            message,
                            sent_at,
                        };
                        let mut guard = app.lock().await;
                        if let Err(e) = handle_net_event(net_event, &mut *guard) {
                            log_push(
                                &log,
                                "application_delivery",
                                format!("handle_net_event error: {e}"),
                            );
                        }
                    }
                    Err(e) => {
                        log_push(&log, "decode", format!("verify_and_decode FAILED: {e}"));
                    }
                }
            }
            GossipEvent::NeighborUp(id) => {
                app.lock().await.neighbors.insert(id);
                log_push(
                    &log,
                    "topic_join",
                    format!("neighbor up {}", id.fmt_short()),
                );
            }
            GossipEvent::NeighborDown(id) => {
                app.lock().await.neighbors.remove(&id);
                log_push(
                    &log,
                    "topic_down",
                    format!("neighbor down {}", id.fmt_short()),
                );
            }
            _ => {}
        }
    }
    log_push(
        &log,
        "topic_down",
        "app-layer task exited (receiver closed)",
    );
}

// ---------------------------------------------------------------------------
// Node under test
// ---------------------------------------------------------------------------

/// A live node under test: network half (endpoint + gossip + discovery
/// service), the direct-topic subscription (sender + app-layer delivery
/// task), and the app's BORU-CP-07/08 reconnect wiring (friend watcher →
/// `queue_reconnect` → `PeerReachable` → join the peer back into the direct
/// sender).
struct TestNode {
    _router: Router,
    endpoint: Endpoint,
    gossip: Gossip,
    service: DiscoveryService,
    sk: SecretKey,
    pk: PublicKey,
    app: Arc<tokio::sync::Mutex<Recorder>>,
    log: Arc<Mutex<Vec<String>>>,
    /// The direct-topic sender, shared with the reconnect forwarder task.
    direct_sender: Arc<tokio::sync::Mutex<Option<GossipSender>>>,
    _delivery_task: Option<JoinHandle<()>>,
    _watcher: JoinHandle<()>,
    _forwarder: JoinHandle<()>,
}

/// Start a node with `identity`, join it to the internal discovery topic,
/// and wire the app's automatic-reconnection triggers for `friend` — the
/// startup sequence `examples/iced_chat/main.rs` performs on every launch,
/// including after a restart.
async fn start_node(
    memory: MemoryLookup,
    identity: [u8; 32],
    bootstrap: Vec<PublicKey>,
    network: PublicNetwork,
    friend: PublicKey,
) -> Result<TestNode> {
    let sk = SecretKey::from_bytes(&identity);
    let pk = sk.public();
    let (router, ep, gossip, _) = spawn_node_with_key(memory.clone(), sk.clone()).await?;
    // A restart gives the endpoint a fresh transient address; the shared
    // address book must learn it (replacing the stale pre-restart entry).
    memory.set_endpoint_info(ep.addr());

    let service = DiscoveryService::join(&gossip, discovery_topic(network), bootstrap, pk)
        .await
        .expect("node joins the internal discovery topic")
        .with_announce_min_interval(Duration::ZERO)
        .with_control_announce_min_interval(Duration::ZERO)
        .with_reconnect_backoff(Duration::from_millis(100), Duration::from_secs(1));

    let log = Arc::new(Mutex::new(Vec::new()));
    let app = Arc::new(tokio::sync::Mutex::new(Recorder::new(pk, log.clone())));
    let direct_sender: Arc<tokio::sync::Mutex<Option<GossipSender>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // ── BORU-CP-07 friend watcher (mirror of main.rs): a fresh discovery
    //    announcement of the known friend queues ONE reconnect attempt. The
    //    test already knows the peer is a friend (the direct-topic
    //    subscription IS the friendship), so the app-layer friendship check
    //    is `node_id == friend`.
    let reconnect_handle = service.reconnect_handle();
    let mut peer_updates = service.peer_updates();
    let watcher_log = log.clone();
    let watcher = tokio::spawn(async move {
        loop {
            match peer_updates.recv().await {
                Ok(PeerUpdate::Seen { node_id, .. }) if node_id == friend => {
                    log_push(
                        &watcher_log,
                        "discovery",
                        format!(
                            "fresh announcement of friend {} → queue_reconnect",
                            node_id.fmt_short()
                        ),
                    );
                    reconnect_handle.queue_reconnect(node_id);
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // ── BORU-CP-07/08 reconnect signal forwarder (mirror of main.rs +
    //    ReconnectPeerReady): on PeerReachable, record endpoint
    //    connectivity and join the friend back into the live direct-topic
    //    sender (the data-plane action the discovery service never
    //    performs itself).
    let mut reconnect_events = service.reconnect_events();
    let fwd_log = log.clone();
    let fwd_sender = direct_sender.clone();
    let forwarder = tokio::spawn(async move {
        loop {
            match reconnect_events.recv().await {
                Ok(ReconnectSignal::PeerReachable { peer }) => {
                    log_push(
                        &fwd_log,
                        "endpoint_connectivity",
                        format!("PeerReachable {}", peer.fmt_short()),
                    );
                    if let Some(sender) = fwd_sender.lock().await.clone() {
                        match sender.join_peers(vec![peer]).await {
                            Ok(()) => log_push(
                                &fwd_log,
                                "endpoint_connectivity",
                                format!("join_peers -> {} queued", peer.fmt_short()),
                            ),
                            Err(e) => log_push(
                                &fwd_log,
                                "endpoint_connectivity",
                                format!("join_peers failed: {e}"),
                            ),
                        }
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
        log,
        direct_sender,
        _delivery_task: None,
        _watcher: watcher,
        _forwarder: forwarder,
    })
}

impl TestNode {
    /// Subscribe to the deterministic direct topic — the OpenFriendChat →
    /// BackgroundSubscribe pattern, step 1: create the subscription and
    /// return it WITHOUT waiting for the mesh join. Both sides must be
    /// subscribed before either side's join can complete (the swarm edge
    /// forms only when both actors know the topic), so the test calls
    /// [`begin_direct`](Self::begin_direct) on both nodes first, then
    /// [`finish_direct`](Self::finish_direct) on both.
    async fn begin_direct(&self, topic: TopicId, bootstrap: Vec<PublicKey>) -> Result<GossipTopic> {
        log_push(
            &self.log,
            "subscription_ready",
            "direct-topic subscription requested",
        );
        let sub = self.gossip.subscribe(topic, bootstrap).await?;
        log_push(
            &self.log,
            "topic_join",
            "direct-topic subscription created (waiting for mesh join)",
        );
        Ok(sub)
    }

    /// Finish a direct-topic subscription: wait for the swarm join
    /// (NeighborUp), then split into the sender (kept for broadcast +
    /// reconnects) and the app-layer delivery task.
    async fn finish_direct(&mut self, mut sub: GossipTopic) -> Result<()> {
        match tokio::time::timeout(MESH_TIMEOUT, sub.joined()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => bail_any!("direct-topic join failed: {e}"),
            Err(_) => bail_any!("timed out waiting for direct-topic join"),
        }
        log_push(
            &self.log,
            "topic_join",
            "direct-topic subscription joined (NeighborUp)",
        );
        let (sender, receiver) = sub.split();
        *self.direct_sender.lock().await = Some(sender.clone());
        // `joined()` above consumed the initial NeighborUp event from the
        // stream, so the app-layer task would never see it. The receiver
        // still tracks the neighbor internally — seed the recorder's
        // neighbor set (the subscription-readiness signal) from it.
        {
            let mut guard = self.app.lock().await;
            for id in receiver.neighbors() {
                guard.neighbors.insert(id);
            }
        }
        let app = self.app.clone();
        let log = self.log.clone();
        let task = tokio::spawn(run_app_layer(receiver, app, log));
        self._delivery_task = Some(task);
        log_push(
            &self.log,
            "subscription_ready",
            "direct-topic sender + app-layer delivery task active",
        );
        Ok(())
    }

    /// Mirror the app's `ReconnectPeerReady` handler (BORU-CP-08): after a
    /// peer becomes reachable again, join it back into the live direct-topic
    /// sender so the direct-topic mesh edge re-forms deterministically
    /// instead of waiting for the gossip dial cooldown.
    async fn reconnect_peer_ready(&self, peer: PublicKey) -> Result<()> {
        log_push(
            &self.log,
            "endpoint_connectivity",
            format!("ReconnectPeerReady for {}", peer.fmt_short()),
        );
        if let Some(sender) = self.direct_sender.lock().await.clone() {
            sender
                .join_peers(vec![peer])
                .await
                .map_err(|e| n0_error::AnyError::from_string(format!("join_peers failed: {e}")))?;
            log_push(
                &self.log,
                "endpoint_connectivity",
                format!("join_peers -> {} queued", peer.fmt_short()),
            );
        } else {
            log_push(
                &self.log,
                "endpoint_connectivity",
                "no live direct sender yet — the restarted side's subscription will bootstrap",
            );
        }
        Ok(())
    }

    /// Graceful shutdown (the app's normal stop path): abort the delivery
    /// task, drop the direct subscription, then shut the discovery service
    /// and gossip actor down cleanly so the survivor processes the
    /// disconnect (NeighborDown → Degraded) instead of a QUIC idle timeout.
    async fn shutdown(self) {
        let TestNode {
            _router,
            endpoint,
            gossip,
            service,
            sk: _sk,
            pk: _pk,
            app: _app,
            log: _log,
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
/// sender, then wait until `to`'s application layer has delivered it. On
/// timeout the error identifies the exact stage where the direction stopped
/// (the receiver's diagnostic log is included).
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
    log_push(
        &from.log,
        "broadcast_attempt",
        format!("{direction}: broadcast sent ({text:?})"),
    );

    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if to.app.lock().await.delivered.iter().any(|t| t == text) {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    let snapshot = to.log.lock().expect("stage log lock poisoned").clone();
    let tail: Vec<String> = snapshot.iter().rev().take(30).cloned().collect();
    bail_any!(
        "{direction}: application delivery timed out for {text:?}. Receiver's last diagnostics:\n  {}",
        tail.join("\n  ")
    );
}

// =========================================================================
// The test — bidirectional delivery before and after either node restarts
// =========================================================================

/// A and B start, discover each other, and both direct-topic subscriptions
/// become ready. A→B and B→A both reach the application layer. B restarts
/// (same identity); rediscovery + automatic reconnection are allowed, then
/// both directions are asserted again. A restarts; the cycle repeats with
/// the roles reversed. Per-side diagnostics are recorded for every stage
/// and printed at the end; a failure identifies the exact stage where a
/// direction stopped.
#[tokio::test]
async fn reconnect_asymmetric_messages_flow_both_directions() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .try_init();

    let network = PublicNetwork::Test;
    let id_a = seed(0xE1);
    let id_b = seed(0xE2);
    let pk_a = SecretKey::from_bytes(&id_a).public();
    let pk_b = SecretKey::from_bytes(&id_b).public();
    let direct = direct_topic(&pk_a, &pk_b);
    assert_eq!(
        direct,
        direct_topic(&pk_b, &pk_a),
        "direct topic must be order-independent (both sides derive the same)"
    );

    // Shared in-memory address book: both endpoints can dial each other by
    // endpoint id; a restart replaces the stale entry with the fresh addr.
    let memory = MemoryLookup::new();

    // ── Phase 0: A and B start ─────────────────────────────────────────
    let mut node_a = start_node(memory.clone(), id_a, Vec::new(), network, pk_b).await?;
    assert_eq!(node_a.pk, pk_a, "A identity");
    let mut node_b = start_node(memory.clone(), id_b, vec![pk_a], network, pk_a).await?;
    assert_eq!(node_b.pk, pk_b, "B identity");

    // Discovery path: both registries learn the other via the join Hello.
    wait_for_peer(&node_a.service, pk_b, "A to learn B (phase 0)").await?;
    wait_for_peer(&node_b.service, pk_a, "B to learn A (phase 0)").await?;
    log_push(&node_a.log, "discovery", "A knows B (phase 0)");
    log_push(&node_b.log, "discovery", "B knows A (phase 0)");

    // Direct-topic subscriptions ready on both sides. Both subscriptions
    // are created before either join is awaited (the swarm edge forms only
    // when both actors know the topic).
    let sub_a = node_a.begin_direct(direct, vec![pk_b]).await?;
    let sub_b = node_b.begin_direct(direct, vec![pk_a]).await?;
    node_a.finish_direct(sub_a).await?;
    node_b.finish_direct(sub_b).await?;
    wait_for_neighbor(&node_a, pk_b, "A direct-topic neighbor B (phase 0)").await?;
    wait_for_neighbor(&node_b, pk_a, "B direct-topic neighbor A (phase 0)").await?;

    // Bidirectional messaging reaches the application layer.
    send_and_assert(&node_a, &node_b, "phase0 A→B", "phase 0 A→B").await?;
    send_and_assert(&node_b, &node_a, "phase0 B→A", "phase 0 B→A").await?;

    // ── Phase 1: restart B; allow rediscovery/reconnect; repeat ────────
    node_b.shutdown().await;
    // Give A's gossip actor a moment to process B's disconnect (NeighborDown
    // → Degraded, so B is no longer online from A's perspective).
    tokio::time::sleep(DISCONNECT_WINDOW).await;

    let mut node_b = start_node(memory.clone(), id_b, vec![pk_a], network, pk_a).await?;
    assert_eq!(node_b.pk, pk_b, "restarted B must keep its identity");

    // Rediscovery: B2 (a fresh process) must learn A from scratch; A must
    // have B back in its registry (refreshed by B2's post-restart Hello).
    wait_for_peer(&node_b.service, pk_a, "B2 to rediscover A (phase 1)").await?;
    wait_for_source(
        &node_b.service,
        pk_a,
        PeerSource::Hello,
        "B2's registry entry for A (post-restart Hello, phase 1)",
    )
    .await?;
    wait_for_peer(&node_a.service, pk_b, "A to rediscover B (phase 1)").await?;
    log_push(
        &node_b.log,
        "discovery",
        "B2 re-learned A after restart (phase 1)",
    );
    log_push(
        &node_a.log,
        "discovery",
        "A re-learned B after restart (phase 1)",
    );

    // Reconnect: B2 subscribes the direct topic at startup (the app's
    // auto-subscribe path — its bootstrap dial re-forms the mesh); A mirrors
    // ReconnectPeerReady and joins B back into its live direct-topic sender.
    let sub_b2 = node_b.begin_direct(direct, vec![pk_a]).await?;
    node_b.finish_direct(sub_b2).await?;
    node_a.reconnect_peer_ready(pk_b).await?;
    wait_for_neighbor(&node_a, pk_b, "A direct-topic neighbor B2 (phase 1)").await?;
    wait_for_neighbor(&node_b, pk_a, "B2 direct-topic neighbor A (phase 1)").await?;

    // Both directions again.
    send_and_assert(&node_a, &node_b, "phase1 A→B", "phase 1 A→B").await?;
    send_and_assert(&node_b, &node_a, "phase1 B→A", "phase 1 B→A").await?;

    // ── Phase 2: restart A; allow rediscovery/reconnect; repeat ────────
    node_a.shutdown().await;
    tokio::time::sleep(DISCONNECT_WINDOW).await;

    let mut node_a = start_node(memory.clone(), id_a, vec![pk_b], network, pk_b).await?;
    assert_eq!(node_a.pk, pk_a, "restarted A must keep its identity");

    // Rediscovery (roles reversed).
    wait_for_peer(&node_a.service, pk_b, "A2 to rediscover B (phase 2)").await?;
    wait_for_source(
        &node_a.service,
        pk_b,
        PeerSource::Hello,
        "A2's registry entry for B (post-restart Hello, phase 2)",
    )
    .await?;
    wait_for_peer(&node_b.service, pk_a, "B to rediscover A (phase 2)").await?;
    log_push(
        &node_a.log,
        "discovery",
        "A2 re-learned B after restart (phase 2)",
    );
    log_push(
        &node_b.log,
        "discovery",
        "B re-learned A after restart (phase 2)",
    );

    // Reconnect: A2 subscribes the direct topic; B mirrors ReconnectPeerReady.
    let sub_a2 = node_a.begin_direct(direct, vec![pk_b]).await?;
    node_a.finish_direct(sub_a2).await?;
    node_b.reconnect_peer_ready(pk_a).await?;
    wait_for_neighbor(&node_a, pk_b, "A2 direct-topic neighbor B (phase 2)").await?;
    wait_for_neighbor(&node_b, pk_a, "B direct-topic neighbor A2 (phase 2)").await?;

    // Both directions again.
    send_and_assert(&node_a, &node_b, "phase2 A→B", "phase 2 A→B").await?;
    send_and_assert(&node_b, &node_a, "phase2 B→A", "phase 2 B→A").await?;

    // ── Domain separation: the direct topic is NOT the discovery topic ──
    let discovery = discovery_topic(network);
    assert_ne!(
        direct, discovery,
        "direct topic must differ from the discovery topic"
    );
    assert_eq!(
        topic_kind(direct),
        TopicKind::Conversation,
        "direct topic is a conversation topic"
    );
    assert!(
        is_discovery_topic(discovery),
        "discovery topic is control-plane infrastructure"
    );

    // ── Per-side diagnostic records (the PDF's stage list) ─────────────
    println!(
        "=== A stage log ===\n{}",
        node_a.log.lock().unwrap().join("\n")
    );
    println!(
        "=== B stage log ===\n{}",
        node_b.log.lock().unwrap().join("\n")
    );

    node_a.shutdown().await;
    node_b.shutdown().await;
    Ok(())
}
