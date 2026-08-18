#![cfg(feature = "net")]

//! # UI isolation test — discovery packets never render as chat
//!
//! BORU-DISC-25 (PDF task 22): inject **valid** discovery packets
//! (`Hello` / `Presence` / `PeerAdvertisement` on the internal discovery
//! gossip topic) AND **malformed** discovery packets (garbage bytes, an
//! unknown protocol version, a truncated postcard payload) into the app/UI
//! path. The acceptance criterion is that **no chat row, history record,
//! unread count, notification, typing state, or attachment entry is
//! produced** — discovery is networking infrastructure and must never leak
//! into conversation state (the BORU-DISC-13 persistence/rendering block,
//! proven end-to-end at the UI level).
//!
//! The app state is driven directly (the deterministic, fast path the PDF
//! prefers): a `UiState` implementing [`ChatCallbacks`] mirrors the six
//! user-visible surfaces from task 22, exactly like the headless
//! `TestChat` in `tests/test_hostile_input.rs` drives
//! [`handle_net_event`](boru_core::chat_core::handle_net_event).
//!
//! ## What the tests prove
//!
//! 1. **`valid_and_malformed_discovery_traffic_produces_no_ui_state`** —
//!    end-to-end over a real loopback gossip mesh. Node A (the peer) and
//!    node B (the app under test) both join the internal discovery topic
//!    through [`DiscoveryService::join`] — the exact startup path from
//!    `examples/iced_chat/main.rs`. A sends the full discovery traffic
//!    mix: valid `Hello` / `Presence` / `PeerAdvertisement` via
//!    `DiscoveryService::publish` and malformed payloads as raw gossip
//!    broadcasts. B's discovery service consumes them (registry updates,
//!    `PeerUpdate::Advertised` for the advertised peer) while B's
//!    user-visible state stays untouched: zero chat rows, zero history
//!    records, zero unread, zero notifications, zero typing state, zero
//!    attachment entries, an empty conversation store, and no
//!    `ChatCallbacks` neighbor activity. A wire spy on B additionally
//!    proves no payload that crossed the discovery topic verifies as a
//!    chat [`SignedMessage`].
//! 2. **`conversation_forwarder_drops_discovery_topic_events`** — the
//!    defense-in-depth boundary. Even if a conversation forwarder is
//!    (mis)wired to the discovery topic, [`spawn_conversation_forwarder`]
//!    refuses to forward: every discovery payload (valid and malformed) is
//!    drained and dropped, and no [`ConversationNetEvent`] ever reaches the
//!    app's net channel. This is the BORU-DISC-10 routing guard at the
//!    forwarder-spawn boundary, asserted directly.
//! 3. **`handle_incoming_rejects_malformed_discovery_payloads`** — the
//!    hostile-input half at the receive-path core: garbage bytes and a
//!    truncated postcard payload yield [`IncomingOutcome::Undecodable`], an
//!    unknown protocol version yields
//!    [`IncomingOutcome::UnsupportedVersion`], a self-originated message
//!    yields [`IncomingOutcome::SelfMessage`], and a duplicate event id
//!    yields [`IncomingOutcome::Duplicate`] — no panic, no registry
//!    mutation (mirroring the `tests/test_hostile_input.rs` patterns:
//!    assert no panic, assert no state mutation).
//!
//! Typing-state note: the chat protocol's typing indicator was removed
//! from [`Message`](boru_core::chat_core::Message) in an earlier refactor,
//! so the typing category is structurally empty — the test still models
//! and asserts it (nothing may ever set `typing_peers`).

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use boru_core::{
    api::{Event as GossipEvent, GossipTopic},
    chat_callbacks::ChatCallbacks,
    chat_core::{ChatEntry, MessageHash, SignedMessage},
    chat_history::{ChatHistoryStore, DeliveryState},
    control_plane::message::{ControlEnvelope, ControlPlaneDecode, CONTROL_PLANE_MAGIC},
    conversations::{spawn_conversation_forwarder, ConversationNetEvent, ConversationStore},
    discovery_message::DiscoveryMessage,
    discovery_service::{ControlEvent, DiscoveryService, IncomingOutcome, PeerSource, PeerUpdate},
    discovery_topic::{discovery_topic, BORU_DISCOVERY_PROTOCOL_VERSION},
    friends::{FriendId, FriendsStore},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    public_room::PublicNetwork,
    room_docs::{create_metadata_doc, create_roster_doc, RoomMetadata},
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

/// How long a two-node mesh may take to form (dial + topic joins + gossip
/// handshakes). Generous for CI, but every poll loop exits as soon as its
/// condition is satisfied.
const MESH_TIMEOUT: Duration = Duration::from_secs(20);
/// Poll interval while waiting for the mesh / spies / updates.
const POLL_TICK: Duration = Duration::from_millis(100);
/// How long to let malformed broadcasts drain through the gossip mesh and
/// the receive drain before asserting (they are dropped, so the assertion
/// is time-independent on the receiving side — this only bounds the test).
const MALFORMED_DRAIN: Duration = Duration::from_millis(600);

// ---------------------------------------------------------------------------
// Network helpers (mirror the deterministic two-node pattern used by the
// sibling discovery suites)
// ---------------------------------------------------------------------------

/// Spawn a fresh in-process node: real iroh endpoint (no relay, loopback)
/// with the shared in-memory address book, plus a gossip actor and protocol
/// router.
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

/// A valid `Hello` for `node` serialised with the current protocol version.
fn valid_hello(node: PublicKey, event_id: u64) -> Vec<u8> {
    postcard::to_stdvec(&DiscoveryMessage::hello_with_event(node, event_id)).unwrap()
}

/// A `Hello` payload speaking an UNKNOWN protocol version (`0x7F`).
///
/// The wire layout is `variant tag (0x00) || protocol_version (u8) || node
/// (32 bytes) || event id`, so flipping byte index 1 to an unsupported
/// version keeps the payload fully decodable — it must then be rejected by
/// the protocol version gate, never interpreted.
fn unknown_version_hello(node: PublicKey, event_id: u64) -> Vec<u8> {
    let mut bytes = valid_hello(node, event_id);
    bytes[1] = 0x7F;
    bytes
}

/// A truncated discovery payload: the first `len` bytes of a valid `Hello`
/// (cut mid-node-id, so postcard cannot deserialise it).
fn truncated_hello(node: PublicKey, len: usize) -> Vec<u8> {
    let bytes = valid_hello(node, 1);
    bytes[..len].to_vec()
}

/// Spawn a raw spy subscription on `topic`: it captures every payload that
/// crossed the mesh on that topic (in addition to any service's own
/// subscription) so the test can assert the wire-level isolation guarantee.
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

/// Wait until a gossip topic subscription is joined — its stream has
/// processed at least one `NeighborUp` and the swarm edge exists — so a
/// broadcast is not lost to the empty-mesh trap.
async fn wait_for_joined(sub: &mut GossipTopic, what: &str) -> Result<()> {
    match tokio::time::timeout(MESH_TIMEOUT, sub.joined()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => bail_any!("timed out waiting for {what} to join"),
    }
}

// ---------------------------------------------------------------------------
// UI harness — the six user-visible surfaces from PDF task 22
// ---------------------------------------------------------------------------

/// Mirrors the app's user-visible surfaces that must stay untouched by
/// discovery traffic:
///
/// * **chat rows** — `entries` (pushed by `push_remote` / `push_system`)
/// * **history records** — `history` + `history_persist_count` (the
///   durable store; `persist_remote_message` / `persist_remote_file_share`
///   overrides record every write attempt)
/// * **unread count** — `unread` (bumped on every new chat surface)
/// * **notifications** — `notifications` (bumped alongside chat rows)
/// * **typing state** — `typing_peers` (structurally empty: the protocol
///   removed typing indicators; nothing may ever set it)
/// * **attachment entries** — `pending_file` / `pending_images` (set by
///   `set_pending_file` / `set_pending_image`)
///
/// Plus the conversation store (`ConversationStore`), the durable peer
/// name map, and neighbor activity counters to catch any leakage into the
/// `ChatCallbacks` layer.
struct UiState {
    local_public: PublicKey,
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    friends: FriendsStore,
    unread: u64,
    notifications: Vec<String>,
    typing_peers: HashSet<PublicKey>,
    pending_file: Option<(String, String)>,
    pending_images: Vec<(String, MessageHash, PublicKey)>,
    history: ChatHistoryStore,
    history_persist_count: usize,
    should_quit: bool,
    neighbor_ups: usize,
    neighbor_downs: usize,
    activity_count: usize,
}

impl UiState {
    fn new(local_public: PublicKey, data_dir: &std::path::Path) -> Self {
        Self {
            local_public,
            entries: Vec::new(),
            names: HashMap::new(),
            friends: FriendsStore::default(),
            unread: 0,
            notifications: Vec::new(),
            typing_peers: HashSet::new(),
            pending_file: None,
            pending_images: Vec::new(),
            history: ChatHistoryStore::empty_at(data_dir),
            history_persist_count: 0,
            should_quit: false,
            neighbor_ups: 0,
            neighbor_downs: 0,
            activity_count: 0,
        }
    }
}

impl ChatCallbacks for UiState {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }

    fn resolve_name(&self, peer: &PublicKey) -> String {
        let fid = FriendId::from_public_key(*peer);
        if let Some(record) = self.friends.get(&fid) {
            if let Some(label) = &record.label {
                return label.clone();
            }
            if let Some(name) = &record.last_announced_name {
                return name.clone();
            }
        }
        self.names
            .get(peer)
            .cloned()
            .unwrap_or_else(|| peer.fmt_short().to_string())
    }

    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.insert(peer, name)
    }

    fn is_friend(&self, peer: &PublicKey) -> bool {
        let fid = FriendId::from_public_key(*peer);
        self.friends.get(&fid).is_some()
    }

    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {}

    fn set_pending_file(
        &mut self,
        name: String,
        ticket: String,
        _size: u64,
        _thumbnail: Option<MessageHash>,
        _sender_label: Option<String>,
    ) {
        self.pending_file = Some((name, ticket));
    }

    fn push_system(&mut self, text: String) {
        self.entries.push(ChatEntry::system(text.clone()));
        self.unread += 1;
        self.notifications.push(text);
    }

    fn push_remote(
        &mut self,
        _peer: PublicKey,
        label: String,
        text: String,
        hash: Option<MessageHash>,
        sent_at: Option<u64>,
    ) {
        let mut entry = ChatEntry::remote(label.clone(), text);
        if let Some(secs) = sent_at {
            entry = entry.with_timestamp(Some(secs * 1000));
        }
        if let Some(h) = hash {
            entry = entry.with_message_hash(h);
        }
        self.entries.push(entry);
        self.unread += 1;
        self.notifications.push(label);
    }

    /// A durable history record write — the "history record" surface from
    /// task 22. Overridden so the test can count (and therefore forbid)
    /// every attempt to persist an incoming message.
    fn persist_remote_message(
        &mut self,
        _topic: Option<TopicId>,
        _peer: PublicKey,
        _hash: MessageHash,
        _sent_at: u64,
        _text: &str,
        _signed_bytes: Option<Vec<u8>>,
    ) {
        self.history_persist_count += 1;
    }

    /// A durable history record write for a file-share announcement (the
    /// attachment-entry history surface). Same counting as
    /// [`persist_remote_message`](Self::persist_remote_message).
    fn persist_remote_file_share(
        &mut self,
        _topic: Option<TopicId>,
        _peer: PublicKey,
        _hash: MessageHash,
        _sent_at: u64,
        _name: &str,
        _signed_bytes: Option<Vec<u8>>,
    ) {
        self.history_persist_count += 1;
    }

    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey) {
        self.pending_images.push((name, hash, from));
    }

    fn has_message(&self, hash: &MessageHash) -> bool {
        self.entries
            .iter()
            .any(|e| e.message_hash.as_ref() == Some(hash))
    }

    fn edit_message(&mut self, hash: &MessageHash, new_text: String) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.message_hash.as_ref() == Some(hash))
        {
            entry.body = new_text;
            entry.edited = true;
        }
    }

    fn delete_message(&mut self, hash: &MessageHash) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.message_hash == Some(*hash))
        {
            entry.body = "[message deleted]".to_string();
            entry.edited = false;
            entry.reactions.clear();
        }
    }

    fn add_reaction(&mut self, hash: &MessageHash, emoji: String) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.message_hash == Some(*hash))
        {
            entry.reactions.push(emoji);
        }
    }

    fn on_neighbor_up(&mut self, _peer: PublicKey) {
        self.neighbor_ups += 1;
    }

    fn on_neighbor_down(&mut self, _peer: PublicKey) {
        self.neighbor_downs += 1;
    }

    fn record_activity(&mut self, _peer: PublicKey) {
        self.activity_count += 1;
    }

    fn request_quit(&mut self) {
        self.should_quit = true;
    }

    fn event_id_for_hash(&self, _hash: &MessageHash) -> Option<u64> {
        None
    }

    fn update_delivery_state(&mut self, _event_id: u64, _state: DeliveryState) {}
}

/// Assert the task-22 acceptance criterion on one UI harness: no chat row,
/// history record, unread count, notification, typing state, or attachment
/// entry was produced, and the conversation store stayed empty.
fn assert_ui_isolated(ui: &UiState, store: &ConversationStore, topic: &TopicId, who: &str) {
    assert!(
        ui.entries.is_empty(),
        "{who}: discovery traffic must not create chat rows (got {})",
        ui.entries.len()
    );
    assert_eq!(
        ui.unread, 0,
        "{who}: discovery traffic must not create an unread count"
    );
    assert!(
        ui.notifications.is_empty(),
        "{who}: discovery traffic must not create notifications (got {})",
        ui.notifications.len()
    );
    assert!(
        ui.typing_peers.is_empty(),
        "{who}: discovery traffic must not create typing state"
    );
    assert!(
        ui.pending_file.is_none(),
        "{who}: discovery traffic must not create an attachment entry"
    );
    assert!(
        ui.pending_images.is_empty(),
        "{who}: discovery traffic must not create image attachment entries (got {})",
        ui.pending_images.len()
    );
    assert_eq!(
        ui.history_persist_count, 0,
        "{who}: discovery traffic must not write a history record"
    );
    assert!(
        ui.history.is_empty(),
        "{who}: the durable history store must stay empty (got {})",
        ui.history.len()
    );
    assert_eq!(
        ui.neighbor_ups, 0,
        "{who}: discovery mesh neighbor events must not reach ChatCallbacks::on_neighbor_up"
    );
    assert_eq!(
        ui.neighbor_downs, 0,
        "{who}: discovery mesh neighbor events must not reach ChatCallbacks::on_neighbor_down"
    );
    assert_eq!(
        ui.activity_count, 0,
        "{who}: discovery traffic must not reach ChatCallbacks::record_activity"
    );
    assert!(
        ui.names.is_empty(),
        "{who}: discovery traffic must not set peer display names (got {})",
        ui.names.len()
    );
    assert!(
        !ui.should_quit,
        "{who}: discovery traffic must not request a quit"
    );
    assert!(
        store.is_empty(),
        "{who}: the conversation store must stay empty (got {})",
        store.len()
    );
    assert!(
        store.find(topic).is_none(),
        "{who}: the discovery topic must never become a conversation entry"
    );
}

// =========================================================================
// 1. End-to-end: valid + malformed discovery traffic produces no UI state
// =========================================================================

/// A node under test: its network half is kept alive for the whole test.
struct UiNode {
    _router: Router,
    _endpoint: Endpoint,
    _gossip: Gossip,
    service: DiscoveryService,
}

/// A two-node UI-isolation harness: A is the peer sending discovery
/// traffic; B is the app under test (DiscoveryService + user-visible state).
struct UiIsolationHarness {
    a: UiNode,
    b: UiNode,
    /// A raw broadcaster on A for the malformed (raw-byte) payloads.
    raw_a: GossipTopic,
    /// B's user-visible state (task-22 surfaces).
    ui: UiState,
    /// B's conversation store (must never gain a discovery entry).
    store: ConversationStore,
    /// B's wire spy — captures everything that crossed the discovery topic.
    spy_b: Arc<Mutex<Vec<Vec<u8>>>>,
    _spy_task_b: JoinHandle<()>,
    _dir: TempDir,
    topic: TopicId,
    pk_a: PublicKey,
    pk_b: PublicKey,
}

impl UiIsolationHarness {
    async fn spawn(rng: &mut impl rand::Rng, network: PublicNetwork) -> Result<Self> {
        let topic = discovery_topic(network);

        let memory = MemoryLookup::new();
        let (router_a, ep_a, sk_a, gossip_a) = spawn_node(rng, memory.clone()).await?;
        let (router_b, ep_b, sk_b, gossip_b) = spawn_node(rng, memory.clone()).await?;
        memory.add_endpoint_info(ep_a.addr());
        memory.add_endpoint_info(ep_b.addr());

        let pk_a = sk_a.public();
        let pk_b = sk_b.public();

        let dir = TempDir::new().expect("temp dir for B's UI state");
        let ui = UiState::new(pk_b, dir.path());
        let store = ConversationStore::empty_at(dir.path());

        // B's wire spy subscribes before the service so nothing is missed.
        let spy_b: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let spy_task_b = spawn_spy(&gossip_b, topic, spy_b.clone()).await?;

        // The startup path from `examples/iced_chat/main.rs`: join the
        // internal discovery topic via DiscoveryService::join. B
        // bootstraps to A so the swarm completes its join handshake.
        let service_a = DiscoveryService::join(&gossip_a, topic, Vec::new(), pk_a, sk_a.clone())
            .await
            .expect("A joins the internal discovery topic")
            .with_announce_min_interval(Duration::ZERO);
        let service_b = DiscoveryService::join(&gossip_b, topic, vec![ep_a.id()], pk_b, sk_b.clone())
            .await
            .expect("B joins the internal discovery topic")
            .with_announce_min_interval(Duration::ZERO);

        // A's raw broadcaster: an extra subscription used to inject
        // malformed (raw-byte) payloads that `publish` cannot express.
        let raw_a = gossip_a.subscribe(topic, Vec::new()).await?;

        Ok(Self {
            a: UiNode {
                _router: router_a,
                _endpoint: ep_a,
                _gossip: gossip_a,
                service: service_a,
            },
            b: UiNode {
                _router: router_b,
                _endpoint: ep_b,
                _gossip: gossip_b,
                service: service_b,
            },
            raw_a,
            ui,
            store,
            spy_b,
            _spy_task_b: spy_task_b,
            _dir: dir,
            topic,
            pk_a,
            pk_b,
        })
    }

    /// Broadcast raw bytes on the discovery topic (the malformed
    /// injection path — A's side).
    async fn broadcast_raw(&mut self, payload: &[u8]) -> Result<()> {
        self.raw_a.broadcast(Bytes::from(payload.to_vec())).await?;
        Ok(())
    }

    /// Stop the spy and shut both discovery services down cleanly.
    async fn shutdown(self) {
        self._spy_task_b.abort();
        self.a.service.shutdown().await;
        self.b.service.shutdown().await;
    }
}

/// A and B run the internal discovery topic as networking infrastructure.
/// A injects the full discovery traffic mix — valid `Hello` / `Presence` /
/// `PeerAdvertisement` packets AND malformed packets (garbage bytes, an
/// unknown protocol version, a truncated postcard payload) — while B's
/// discovery service consumes them. B's user-visible state (chat rows,
/// history records, unread count, notifications, typing state, attachment
/// entries, conversation store) stays completely untouched, and a wire spy
/// proves no chat [`SignedMessage`] ever crossed the discovery topic.
#[tokio::test]
async fn valid_and_malformed_discovery_traffic_produces_no_ui_state() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C25A1); // BORU-DISC-25
    let mut harness = UiIsolationHarness::spawn(&mut rng, PublicNetwork::Test).await?;

    // ── Mesh forms: both discovery services see each other ────────────
    wait_for_peer(&harness.b.service, harness.pk_a, "B to learn A").await?;
    wait_for_peer(&harness.a.service, harness.pk_b, "A to learn B").await?;

    // Subscribe to B's peer-update stream BEFORE the valid traffic is
    // published — broadcast channels only deliver to receivers that are
    // subscribed at send time, so subscribing after the advertisement
    // would miss it.
    let mut updates = harness.b.service.peer_updates();

    // ── Valid discovery packets (Hello / Presence / PeerAdvertisement) ─
    harness
        .a
        .service
        .publish(DiscoveryMessage::hello_with_event(harness.pk_a, 1))
        .await?;
    harness
        .a
        .service
        .publish(DiscoveryMessage::presence_with_event(harness.pk_a, 2))
        .await?;
    let pk_c = test_key(0xC0);
    harness
        .a
        .service
        .publish(DiscoveryMessage::peer_advertisement_with_event(
            harness.pk_a,
            pk_c,
            3,
        ))
        .await?;

    // The last valid message processed: B's registry now reports A as seen
    // via the PeerAdvertisement.
    wait_for_source(
        &harness.b.service,
        harness.pk_a,
        PeerSource::PeerAdvertisement,
        "B registry for A",
    )
    .await?;

    // B's discovery service emits the Advertised dial candidate for C
    // (BORU-DISC-11 connectivity wiring input) — still no UI state.
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

    // ── Malformed discovery packets (raw bytes on the discovery topic) ─
    let garbage: &[u8] = b"this is not a discovery message - pure garbage bytes\x00\xff\xfe";
    let unknown_version = unknown_version_hello(harness.pk_a, 4);
    let truncated = truncated_hello(harness.pk_a, 5); // 5 of the 34-byte Hello
    assert!(postcard::from_bytes::<DiscoveryMessage>(garbage).is_err());
    assert!(postcard::from_bytes::<DiscoveryMessage>(&unknown_version).is_ok());
    assert!(postcard::from_bytes::<DiscoveryMessage>(&truncated).is_err());
    harness.broadcast_raw(garbage).await?;
    harness.broadcast_raw(&unknown_version).await?;
    harness.broadcast_raw(&truncated).await?;

    // Let the malformed broadcasts drain through the mesh and the receive
    // drain (they are dropped by B's DiscoveryService).
    tokio::time::sleep(MALFORMED_DRAIN).await;

    // ── UI isolation: none of the six surfaces was touched ────────────
    assert_ui_isolated(&harness.ui, &harness.store, &harness.topic, "B");

    // ── Wire-level isolation: no chat payload crossed the discovery topic
    let spy_b = harness.spy_b.lock().expect("spy lock poisoned").clone();
    assert!(
        !spy_b.is_empty(),
        "B spy must have captured the discovery exchange"
    );
    for content in &spy_b {
        // Every sample is either a decodable discovery payload (valid
        // Hello/Presence/PeerAdvertisement, or the unknown-version variant
        // which is fully decodable before the version gate) or an
        // undecodable malformed payload — never a chat SignedMessage.
        let decodes_as_discovery = postcard::from_bytes::<DiscoveryMessage>(content).is_ok();
        assert!(
            SignedMessage::verify_and_decode(content).is_err(),
            "B spy: discovery topic carried a chat payload (SignedMessage): {}",
            if decodes_as_discovery {
                "valid discovery bytes"
            } else {
                "malformed bytes"
            }
        );
    }

    harness.shutdown().await;
    Ok(())
}

// =========================================================================
// 2. Defense in depth: the conversation forwarder refuses the discovery
//    topic (BORU-DISC-10 guard at the forwarder-spawn boundary)
// =========================================================================

/// Even if a conversation forwarder is (mis)wired to the internal
/// discovery topic, it must refuse to forward: every discovery payload —
/// valid and malformed — is drained and dropped, and no
/// [`ConversationNetEvent`] ever reaches the app's net channel.
#[tokio::test]
async fn conversation_forwarder_drops_discovery_topic_events() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C25A2);
    let topic = discovery_topic(PublicNetwork::Test);

    let memory = MemoryLookup::new();
    let (router_a, ep_a, sk_a, gossip_a) = spawn_node(&mut rng, memory.clone()).await?;
    let (router_b, ep_b, sk_b, gossip_b) = spawn_node(&mut rng, memory.clone()).await?;
    memory.add_endpoint_info(ep_a.addr());
    memory.add_endpoint_info(ep_b.addr());
    let pk_a = sk_a.public();
    let pk_b = sk_b.public();

    // A: raw broadcaster on the discovery topic.
    let mut sub_a = gossip_a.subscribe(topic, vec![ep_b.id()]).await?;
    // B: the mis-wiring scenario — a conversation forwarder on the
    // discovery topic. Wait for the swarm edge before splitting so the
    // receiver handed to the forwarder is live.
    let mut sub_b = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;
    wait_for_joined(&mut sub_a, "A discovery subscription").await?;
    wait_for_joined(&mut sub_b, "B discovery subscription").await?;

    let (sender_b, receiver_b) = sub_b.split();
    let metadata = create_metadata_doc(topic, &sender_b, RoomMetadata::empty()).await?;
    let roster = create_roster_doc(
        topic,
        &sender_b,
        hex::encode(pk_b.as_bytes()),
        "B".to_string(),
    )
    .await?;
    let (net_tx, mut net_rx) = tokio::sync::mpsc::channel::<ConversationNetEvent>(16);
    let forwarder = spawn_conversation_forwarder(topic, metadata, roster, receiver_b, net_tx, None);

    // ── Inject valid + malformed discovery payloads on the topic ───────
    sub_a
        .broadcast(Bytes::from(valid_hello(pk_a, 1)))
        .await?;
    sub_a
        .broadcast(Bytes::from_static(
            b"garbage bytes, not a discovery message\x00\xff",
        ))
        .await?;
    sub_a
        .broadcast(Bytes::from(unknown_version_hello(pk_a, 2)))
        .await?;
    sub_a
        .broadcast(Bytes::from(truncated_hello(pk_a, 5)))
        .await?;

    // The guard drains-and-drops: no ConversationNetEvent may ever arrive.
    tokio::time::sleep(MALFORMED_DRAIN).await;
    assert!(
        net_rx.try_recv().is_err(),
        "discovery-topic events must never become conversation events"
    );

    // The forwarder task is still alive (it is draining, not panicked).
    assert!(
        !forwarder.is_finished(),
        "the discovery-dropping forwarder must keep running"
    );

    drop((router_a, ep_a, router_b, ep_b));
    Ok(())
}

// =========================================================================
// 3. Hostile input at the receive-path core: malformed discovery packets
//    are dropped, never interpreted, never mutating state
// =========================================================================

/// Feed the exact hostile-input patterns from `tests/test_hostile_input.rs`
/// (garbage bytes, unknown protocol version, truncated postcard payload)
/// directly into [`DiscoveryService::handle_incoming`]. Every malformed
/// payload is rejected with the expected outcome, no panic occurs, and the
/// peer registry is never mutated by malformed traffic.
#[tokio::test]
async fn handle_incoming_rejects_malformed_discovery_payloads() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xD15C25A3);
    let topic = discovery_topic(PublicNetwork::Test);
    let (router, ep, sk, gossip) = spawn_node(&mut rng, MemoryLookup::new()).await?;
    let local = sk.public();
    let service = DiscoveryService::join(&gossip, topic, Vec::new(), local, sk.clone())
        .await
        .expect("node joins the discovery topic")
        .with_announce_min_interval(Duration::ZERO);

    let peer = test_key(0xAA);

    // ── Valid Hello → Processed; registry gains exactly the peer ──────
    let hello = valid_hello(peer, 1);
    assert_eq!(
        service.handle_incoming(&hello, local),
        IncomingOutcome::Processed
    );
    assert_eq!(service.peer_count(), 1);

    // ── Valid Presence (new event id) → Processed; source refreshes ───
    let presence = postcard::to_stdvec(&DiscoveryMessage::presence_with_event(peer, 2)).unwrap();
    assert_eq!(
        service.handle_incoming(&presence, local),
        IncomingOutcome::Processed
    );
    assert_eq!(service.peer_count(), 1);
    assert_eq!(
        service.known_peers()[0].1.source,
        PeerSource::Presence,
        "presence must refresh the peer's source"
    );

    // ── Garbage bytes → Undecodable; no registry change ───────────────
    let garbage: &[u8] = b"garbage, not a discovery message\x00\xff\xfe";
    assert_eq!(
        service.handle_incoming(garbage, local),
        IncomingOutcome::Undecodable
    );
    assert_eq!(service.peer_count(), 1);

    // ── Unknown protocol version → UnsupportedVersion; no registry change
    let unknown_version = unknown_version_hello(peer, 3);
    assert_eq!(
        service.handle_incoming(&unknown_version, local),
        IncomingOutcome::UnsupportedVersion {
            found: 0x7F,
            expected: BORU_DISCOVERY_PROTOCOL_VERSION,
        }
    );
    assert_eq!(service.peer_count(), 1);

    // ── Truncated postcard payload → Undecodable; no registry change ──
    let truncated = truncated_hello(peer, 5);
    assert_eq!(
        service.handle_incoming(&truncated, local),
        IncomingOutcome::Undecodable
    );
    assert_eq!(service.peer_count(), 1);

    // ── Duplicate event id → Duplicate; no registry change ────────────
    // The last accepted event was `presence` (event id 2); re-delivering
    // the SAME event id is a duplicate (BORU-DISC-17 dedup).
    assert_eq!(
        service.handle_incoming(&presence, local),
        IncomingOutcome::Duplicate
    );
    assert_eq!(service.peer_count(), 1);

    // ── Self-originated message → SelfMessage; no registry change ─────
    let self_hello = valid_hello(local, 9);
    assert_eq!(
        service.handle_incoming(&self_hello, local),
        IncomingOutcome::SelfMessage
    );
    assert_eq!(service.peer_count(), 1);

    // Registry still holds exactly the one valid peer — malformed traffic
    // never mutated discovery state.
    let known: Vec<PublicKey> = service.known_peers().iter().map(|(id, _)| *id).collect();
    assert_eq!(known, vec![peer], "only the valid peer may be registered");

    service.shutdown().await;
    drop((router, ep));
    Ok(())
}

// =========================================================================
// 4. Control-plane boundary (BORU-CP-02): control envelopes route to
//    DiscoveryService events, never to chat/UI
// =========================================================================

/// BORU-CP-02 (PDF Task 1.2): a **control-plane** envelope (magic "BC",
/// BORU-CP-01 wire format) sent by A via `DiscoveryService::send_control`
/// over a real loopback mesh is received by B's DiscoveryService through
/// its `control_events()` callback stream — the explicit event-callback
/// boundary. B's user-visible state (chat rows, history, unread,
/// notifications, conversation store) stays untouched, and the wire spy
/// proves the control envelope never decodes as a chat `SignedMessage`.
#[tokio::test]
async fn control_plane_envelope_routes_to_service_and_ui_stays_isolated() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xC0DEC0DE); // BORU-CP-02
    let mut harness = UiIsolationHarness::spawn(&mut rng, PublicNetwork::Test).await?;

    // ── Mesh forms: both discovery services see each other ────────────
    wait_for_peer(&harness.b.service, harness.pk_a, "B to learn A").await?;
    wait_for_peer(&harness.a.service, harness.pk_b, "A to learn B").await?;

    // B subscribes to the control-plane event stream BEFORE A sends.
    let mut control_events = harness.b.service.control_events();

    // ── A sends a valid control-plane envelope via send_control ───────
    let envelope = ControlEnvelope::hello(harness.pk_a, 42, 1_700_000_000, 1);
    harness
        .a
        .service
        .send_control(envelope.clone())
        .await
        .expect("A sends a control-plane envelope");

    // ── B's DiscoveryService receives it as a ControlEvent::Received ──
    let deadline = Instant::now() + MESH_TIMEOUT;
    let mut received = None;
    while Instant::now() < deadline {
        match tokio::time::timeout(POLL_TICK, control_events.recv()).await {
            Ok(Ok(ControlEvent::Received(env))) => {
                received = Some(env);
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {} // poll timeout — keep waiting
        }
    }
    let received = received.expect("B must receive the control-plane envelope");
    assert_eq!(received.sender_node_id, harness.pk_a, "sender node id");
    assert_eq!(received.sequence, 42, "sequence round-trip");
    assert_eq!(
        received.message_type,
        boru_core::control_plane::message::ControlMessageType::Hello,
        "message type round-trip"
    );

    // ── UI isolation: none of the six surfaces was touched ────────────
    assert_ui_isolated(&harness.ui, &harness.store, &harness.topic, "B");

    // ── Wire-level isolation: the control envelope never decodes as chat
    let spy_b = harness.spy_b.lock().expect("spy lock poisoned").clone();
    assert!(
        !spy_b.is_empty(),
        "B spy must have captured the control-plane exchange"
    );
    let envelope_bytes = envelope.encode();
    assert!(
        spy_b.iter().any(|content| content == &envelope_bytes),
        "the control envelope must be on the wire"
    );
    // The control envelope's own bytes never decode as a legacy
    // DiscoveryMessage (the two wire formats are disjoint — CP-01 unit
    // proof, re-asserted here on the exact bytes that crossed the mesh).
    assert!(
        postcard::from_bytes::<DiscoveryMessage>(&envelope_bytes).is_err(),
        "a control-plane envelope must never decode as a legacy DiscoveryMessage"
    );
    // Nothing on the wire ever decodes as a chat SignedMessage. (Legacy
    // DiscoveryMessage hellos are expected — A's own join announcement —
    // but chat payloads are never routed through the discovery topic.)
    for content in &spy_b {
        assert!(
            SignedMessage::verify_and_decode(content).is_err(),
            "B spy: discovery topic carried a chat payload (SignedMessage)"
        );
    }

    harness.shutdown().await;
    Ok(())
}

/// BORU-CP-02: a **malformed** control-plane frame (magic "BC" + garbage)
/// over a real loopback mesh is dropped by B's DiscoveryService — no
/// `ControlEvent`, no UI state, no panic — while a follow-up valid control
/// envelope is still processed (fail-closed per-feature, not per-client).
#[tokio::test]
async fn malformed_control_frame_dropped_but_valid_still_processed() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(0xC0DEC0DF); // BORU-CP-02
    let mut harness = UiIsolationHarness::spawn(&mut rng, PublicNetwork::Test).await?;

    wait_for_peer(&harness.b.service, harness.pk_a, "B to learn A").await?;
    wait_for_peer(&harness.a.service, harness.pk_b, "A to learn B").await?;

    let mut control_events = harness.b.service.control_events();

    // Malformed control frame: magic "BC" + supported version byte + garbage
    // header/body (the version gate passes, the header parse fails).
    let mut malformed: Vec<u8> = CONTROL_PLANE_MAGIC.to_vec();
    malformed.push(boru_core::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION);
    malformed.extend_from_slice(b"not a valid envelope at all\x00\xff");
    assert!(
        !matches!(ControlEnvelope::decode(&malformed), Ok(ControlPlaneDecode::Message(_))),
        "fixture must be malformed"
    );
    harness.broadcast_raw(&malformed).await?;

    // Let the malformed broadcast drain.
    tokio::time::sleep(MALFORMED_DRAIN).await;

    // No control event was emitted for the malformed frame.
    assert!(
        tokio::time::timeout(Duration::from_millis(80), control_events.recv())
            .await
            .is_err(),
        "malformed control frame must not emit a ControlEvent"
    );
    assert_ui_isolated(&harness.ui, &harness.store, &harness.topic, "B");

    // A valid control envelope is still processed (fail closed per feature).
    let envelope = ControlEnvelope::presence(harness.pk_a, 7, 1_700_000_000, Some(60));
    harness
        .a
        .service
        .send_control(envelope)
        .await
        .expect("A sends a valid control-plane envelope");

    let deadline = Instant::now() + MESH_TIMEOUT;
    let mut received = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(POLL_TICK, control_events.recv()).await {
            Ok(Ok(ControlEvent::Received(env))) => {
                assert_eq!(env.sender_node_id, harness.pk_a);
                received = true;
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
    assert!(received, "valid control envelope after a malformed frame");
    assert_ui_isolated(&harness.ui, &harness.store, &harness.topic, "B");

    harness.shutdown().await;
    Ok(())
}
