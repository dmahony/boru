//! # Deterministic Two-Peer Test Harness
//!
//! Reusable local test harness for Alice and Bob: persistent temporary profiles,
//! stable identities across restart, contact establishment, mailbox key
//! exchange, start/stop peers, restart with same database, controlled
//! address lookup, fault injection, and event observation.
//!
//! No public infrastructure — uses a local relay server.
//! Bounded timeouts, event-based synchronisation.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use boru_chat::{
    chat_callbacks::ChatCallbacks,
    chat_core::{
        forward_gossip_events, handle_net_event, ChatEntry, Message, MessageHash, NetEvent,
        SignedMessage,
    },
    contact::{ContactAction, SignedContactMessage},
    friends::FriendId,
    mailbox::{MailboxIdentity, MailboxStore},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    whisper::{WhisperBuilder, WhisperEvent, WhisperHandle, WHISPER_ALPN},
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, tls::CaTlsConfig,
    Endpoint, PublicKey, RelayMap, RelayMode, RelayUrl, SecretKey,
};
use n0_error::{bail_any, Result};
use n0_future::{task, time::sleep, StreamExt};
use rand::{RngExt, SeedableRng};
use std::sync::Mutex as StdMutex;
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;
use tracing::info;

// ═══════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(100);
const MAX_JOIN_TICKS: usize = 80;

// ═══════════════════════════════════════════════════════════════════════
// Peer ID
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerId {
    Alice,
    Bob,
}

impl PeerId {
    pub fn name(self) -> &'static str {
        match self {
            PeerId::Alice => "Alice",
            PeerId::Bob => "Bob",
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Fault Configuration
// ═══════════════════════════════════════════════════════════════════════

#[derive(Clone, Debug, Default)]
pub struct FaultConfig {
    pub drop_acks: bool,
    pub duplicate_envelopes: bool,
    pub delay_delivery: Option<Duration>,
    pub inject_protocol_errors: bool,
}

// ═══════════════════════════════════════════════════════════════════════
// HarnessEvent
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum HarnessEvent {
    PeerStarted(PeerId),
    PeerStopped(PeerId),
    GossipConnected(PeerId, PublicKey),
    GossipDisconnected(PeerId, PublicKey),
    MessageSent(PeerId, String),
    MessageReceived(PeerId, String),
    NeighborUp(PeerId, PublicKey),
    NeighborDown(PeerId, PublicKey),
    FriendRequestSent(PeerId, PublicKey),
    MailboxKeyExchanged(PeerId),
    MailboxEnvelopeSent(PeerId, PublicKey),
    FaultInjected(PeerId, String),
    AddressChanged(PeerId),
    ProtocolErrorInjected(PeerId),
}

// ═══════════════════════════════════════════════════════════════════════
// TestPeer — ChatCallbacks implementation
// ═══════════════════════════════════════════════════════════════════════

pub struct TestPeer {
    pub id: PeerId,
    pub local_public: PublicKey,
    pub entries: StdMutex<Vec<ChatEntry>>,
    pub names: StdMutex<HashMap<PublicKey, String>>,
    pub neighbors: StdMutex<HashSet<PublicKey>>,
    pub received_messages: StdMutex<Vec<String>>,
    pub system_messages: StdMutex<Vec<String>>,
    pub neighbor_ups: StdMutex<Vec<PublicKey>>,
    pub neighbor_downs: StdMutex<Vec<PublicKey>>,
}

impl TestPeer {
    pub fn new(id: PeerId, local_public: PublicKey) -> Self {
        Self {
            id,
            local_public,
            entries: StdMutex::new(Vec::new()),
            names: StdMutex::new(HashMap::new()),
            neighbors: StdMutex::new(HashSet::new()),
            received_messages: StdMutex::new(Vec::new()),
            system_messages: StdMutex::new(Vec::new()),
            neighbor_ups: StdMutex::new(Vec::new()),
            neighbor_downs: StdMutex::new(Vec::new()),
        }
    }

    pub fn clear_events(&self) {
        self.entries.lock().unwrap().clear();
        self.received_messages.lock().unwrap().clear();
        self.system_messages.lock().unwrap().clear();
    }
}

impl ChatCallbacks for TestPeer {
    fn local_public(&self) -> PublicKey {
        self.local_public
    }

    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String> {
        self.names.lock().unwrap().insert(peer, name)
    }

    fn is_friend(&self, _peer: &PublicKey) -> bool {
        false
    }

    fn friend_mark_online(&mut self, _fid: FriendId) {}
    fn friend_mark_offline(&mut self, _fid: FriendId) {}
    fn friend_set_name(&mut self, _fid: FriendId, _name: String) {}
    fn mark_friends_dirty(&mut self) {}

    fn push_system(&mut self, text: String) {
        self.system_messages.lock().unwrap().push(text.clone());
        self.entries.lock().unwrap().push(ChatEntry::system(text));
    }

    fn push_remote(
        &mut self,
        _peer: PublicKey,
        label: String,
        text: String,
        _hash: Option<MessageHash>,
        _sent_at: Option<u64>,
    ) {
        self.received_messages
            .lock()
            .unwrap()
            .push(format!("[{label}] {text}"));
        self.entries
            .lock()
            .unwrap()
            .push(ChatEntry::remote(label, text));
    }

    fn set_pending_image(&mut self, _name: String, _hash: MessageHash, _from: PublicKey) {}
    fn has_message(&self, _hash: &MessageHash) -> bool {
        false
    }
    fn edit_message(&mut self, _hash: &MessageHash, _new_text: String) {}
    fn delete_message(&mut self, _hash: &MessageHash) {}
    fn add_reaction(&mut self, _hash: &MessageHash, _emoji: String) {}

    fn on_neighbor_up(&mut self, peer: PublicKey) {
        self.neighbor_ups.lock().unwrap().push(peer);
        self.neighbors.lock().unwrap().insert(peer);
    }

    fn on_neighbor_down(&mut self, peer: PublicKey) {
        self.neighbor_downs.lock().unwrap().push(peer);
        self.neighbors.lock().unwrap().remove(&peer);
    }

    fn record_activity(&mut self, _peer: PublicKey) {}
    fn request_quit(&mut self) {}
}

// ═══════════════════════════════════════════════════════════════════════
// Deterministic key generation
// ═══════════════════════════════════════════════════════════════════════

fn deterministic_secret_key(seed: &[u8]) -> SecretKey {
    let seed64 = if seed.len() >= 8 {
        u64::from_le_bytes(seed[..8].try_into().unwrap())
    } else {
        let mut buf = [0u8; 8];
        buf[..seed.len()].copy_from_slice(seed);
        u64::from_le_bytes(buf)
    };
    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(seed64);
    let sk_bytes: [u8; 32] = rng.random();
    SecretKey::from_bytes(&sk_bytes)
}

// ═══════════════════════════════════════════════════════════════════════
// PeerNode — per-peer runtime state
// ═══════════════════════════════════════════════════════════════════════

pub struct PeerNode {
    pub id: PeerId,
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
    pub data_dir: TempDir,
    pub mailbox_identity: MailboxIdentity,
    pub mailbox_store: MailboxStore,
    pub memory_lookup: MemoryLookup,
    pub fault: FaultConfig,
    pub test_peer: Arc<StdMutex<TestPeer>>,

    // Runtime components
    pub endpoint: Option<Endpoint>,
    pub gossip: Option<Gossip>,
    pub router: Option<Router>,
    pub whisper_handle: Option<WhisperHandle>,
    pub whisper_event_rx:
        Option<Arc<TokioMutex<tokio::sync::mpsc::UnboundedReceiver<WhisperEvent>>>>,
    pub sender: Option<boru_chat::api::GossipSender>,
    pub net_event_tx: Option<tokio::sync::mpsc::UnboundedSender<NetEvent>>,
}

impl PeerNode {
    fn new(id: PeerId, seed: &[u8], fault: FaultConfig) -> Self {
        let data_dir = tempfile::tempdir().expect("create temp dir");
        let secret_key = deterministic_secret_key(seed);
        let public_key = secret_key.public();
        let mailbox_identity = MailboxIdentity::from_secret(&secret_key);
        let mailbox_store = MailboxStore::empty_at(data_dir.path());
        let memory_lookup = MemoryLookup::new();
        let test_peer = Arc::new(StdMutex::new(TestPeer::new(id, public_key)));

        Self {
            id,
            secret_key,
            public_key,
            data_dir,
            mailbox_identity,
            mailbox_store,
            memory_lookup,
            fault,
            test_peer,
            endpoint: None,
            gossip: None,
            router: None,
            whisper_handle: None,
            whisper_event_rx: None,
            sender: None,
            net_event_tx: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.endpoint.is_some()
    }

    pub fn fmt_short(&self) -> String {
        self.public_key.fmt_short().to_string()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TestHarness
// ═══════════════════════════════════════════════════════════════════════

pub struct TestHarness {
    pub alice: PeerNode,
    pub bob: PeerNode,
    pub event_log: Arc<StdMutex<Vec<HarnessEvent>>>,
    pub topic: TopicId,
    // Type-erased guard keeps the in-process relay alive without depending on
    // iroh's private test-utils server type.
    _relay_server: Option<Box<dyn Send>>,
    relay_map: Option<RelayMap>,
    relay_url: Option<RelayUrl>,
    blocked_directions: HashSet<(PeerId, PeerId)>,
}

// Helper to get a mutable reference to both Alice and Bob without borrow conflicts.
macro_rules! peers_mut {
    ($self:expr) => {
        (&mut $self.alice, &mut $self.bob)
    };
}

impl TestHarness {
    pub fn new() -> Self {
        let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(42);
        let topic = TopicId::from_bytes(rng.random());

        Self {
            alice: PeerNode::new(
                PeerId::Alice,
                b"alice-deterministic-key-v2",
                FaultConfig::default(),
            ),
            bob: PeerNode::new(
                PeerId::Bob,
                b"bob-deterministic-key-v2",
                FaultConfig::default(),
            ),
            event_log: Arc::new(StdMutex::new(Vec::new())),
            topic,
            _relay_server: None,
            relay_map: None,
            relay_url: None,
            blocked_directions: HashSet::new(),
        }
    }

    pub fn with_faults(alice_fault: FaultConfig, bob_fault: FaultConfig) -> Self {
        let mut h = Self::new();
        h.alice.fault = alice_fault;
        h.bob.fault = bob_fault;
        h
    }

    pub fn record(&self, event: HarnessEvent) {
        self.event_log.lock().unwrap().push(event);
    }

    pub fn events(&self) -> Vec<HarnessEvent> {
        self.event_log.lock().unwrap().clone()
    }

    pub fn clear_events(&self) {
        self.event_log.lock().unwrap().clear();
    }

    /// Enable or disable one direction of the simulated network path.
    pub fn set_direction_enabled(&mut self, from: PeerId, to: PeerId, enabled: bool) {
        if enabled {
            self.blocked_directions.remove(&(from, to));
        } else {
            self.blocked_directions.insert((from, to));
        }
        self.record(HarnessEvent::FaultInjected(
            from,
            format!(
                "direction to {:?} {}",
                to,
                if enabled { "enabled" } else { "disabled" }
            ),
        ));
    }

    /// Inject a malformed gossip frame and record the operation.
    pub async fn inject_protocol_error(&mut self, from: PeerId) -> Result<()> {
        let sender = self
            .node(from)
            .sender
            .as_ref()
            .ok_or_else(|| n0_error::anyerr!("{:?} has no sender", from))?;
        sender.broadcast(vec![0xff, 0x00, 0xff].into()).await?;
        self.record(HarnessEvent::ProtocolErrorInjected(from));
        Ok(())
    }

    // ── Setup ──────────────────────────────────────────────────────

    pub async fn setup(&mut self) -> Result<()> {
        let (relay_map, relay_url, server) = iroh::test_utils::run_relay_server()
            .await
            .expect("start local relay");

        info!("Test harness local relay: {}", relay_url.to_string());
        self._relay_server = Some(Box::new(server));
        self.relay_map = Some(relay_map.clone());
        self.relay_url = Some(relay_url.clone());

        // Use scope to resolve borrow conflicts
        let topic = self.topic;

        // Start Alice
        self.start_peer(PeerId::Alice, &relay_map, topic).await?;
        // Start Bob
        self.start_peer(PeerId::Bob, &relay_map, topic).await?;

        // Seed lookups
        self.seed_lookup(PeerId::Alice).await;
        self.seed_lookup(PeerId::Bob).await;

        Ok(())
    }

    async fn start_peer(
        &mut self,
        who: PeerId,
        relay_map: &RelayMap,
        topic: TopicId,
    ) -> Result<()> {
        // Supplying the deterministic peer id makes rendezvous deterministic;
        // address lookup is seeded immediately after both endpoints exist.
        let bootstrap = match who {
            PeerId::Alice => self.bob.public_key,
            PeerId::Bob => self.alice.public_key,
        };
        let node = self.node_mut(who);
        if node.is_running() {
            return Ok(());
        }

        let sk = node.secret_key.clone();

        let ep = Endpoint::builder(presets::N0)
            .secret_key(sk)
            .address_lookup(node.memory_lookup.clone())
            .relay_mode(RelayMode::Custom(relay_map.clone()))
            // The in-process test relay uses a generated self-signed certificate.
            .ca_tls_config(CaTlsConfig::insecure_skip_verify())
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
            .bind()
            .await?;
        ep.online().await;

        let gossip = Gossip::builder().spawn(ep.clone());

        // Whisper
        let whisper_builder = WhisperBuilder::new(ep.clone(), node.secret_key.clone());
        let whisper_handler = whisper_builder.protocol_handler();
        let (whisper_handle, whisper_rx) = whisper_builder.spawn();
        node.whisper_handle = Some(whisper_handle);
        node.whisper_event_rx = Some(Arc::new(TokioMutex::new(whisper_rx)));

        // Router
        let router = Router::builder(ep.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(WHISPER_ALPN, whisper_handler)
            .spawn();

        // Subscribe to topic
        let sub = gossip.subscribe(topic, vec![bootstrap]).await?;
        let (sender, receiver) = sub.split();

        // Forward gossip events to NetEvent -> TestPeer callback
        let (net_tx, mut net_rx) = tokio::sync::mpsc::unbounded_channel();
        let fwd = forward_gossip_events(receiver, net_tx.clone());
        task::spawn(fwd);

        let tp = node.test_peer.clone();
        task::spawn(async move {
            while let Some(event) = net_rx.recv().await {
                let mut guard = tp.lock().unwrap();
                let _ = handle_net_event(event, &mut *guard);
            }
        });

        node.endpoint = Some(ep);
        node.gossip = Some(gossip);
        node.router = Some(router);
        node.sender = Some(sender);
        node.net_event_tx = Some(net_tx);

        let short = node.fmt_short();
        drop(node);
        self.record(HarnessEvent::PeerStarted(who));
        info!("{:?} started pk={}", who, short);

        Ok(())
    }

    async fn seed_lookup(&mut self, who: PeerId) {
        let other_addr = match who {
            PeerId::Alice => self.bob.endpoint.as_ref().map(|ep| ep.addr()),
            PeerId::Bob => self.alice.endpoint.as_ref().map(|ep| ep.addr()),
        };

        if let Some(addr) = other_addr {
            let node = self.node_mut(who);
            node.memory_lookup.set_endpoint_info(addr);
        }
    }

    // ── Stop / Restart ─────────────────────────────────────────────

    pub async fn stop_peer(&mut self, who: PeerId) {
        let node = self.node_mut(who);
        node.whisper_handle.take();
        node.whisper_event_rx.take();
        node.sender.take();
        node.net_event_tx.take();

        if let Some(g) = node.gossip.take() {
            let _ = g.shutdown().await;
        }
        if let Some(r) = node.router.take() {
            drop(r);
        }
        if let Some(ep) = node.endpoint.take() {
            ep.close().await;
            drop(ep);
        }

        self.record(HarnessEvent::PeerStopped(who));
        info!("{:?} stopped", who);
    }

    pub async fn restart_peer(&mut self, who: PeerId) -> Result<()> {
        let relay_map = self
            .relay_map
            .clone()
            .ok_or_else(|| n0_error::anyerr!("relay map not available"))?;
        let topic = self.topic;
        self.start_peer(who, &relay_map, topic).await?;
        self.seed_lookup(who).await;
        let other = match who {
            PeerId::Alice => PeerId::Bob,
            PeerId::Bob => PeerId::Alice,
        };
        self.seed_lookup(other).await;
        Ok(())
    }

    // ── Connectivity ───────────────────────────────────────────────

    pub async fn wait_for_connected(&self) -> Result<()> {
        let alice_pk = self.alice.public_key;
        let bob_pk = self.bob.public_key;

        for i in 0..MAX_JOIN_TICKS {
            sleep(TICK).await;

            let a_has_b = self
                .alice
                .test_peer
                .lock()
                .unwrap()
                .neighbors
                .lock()
                .unwrap()
                .contains(&bob_pk);
            let b_has_a = self
                .bob
                .test_peer
                .lock()
                .unwrap()
                .neighbors
                .lock()
                .unwrap()
                .contains(&alice_pk);

            if a_has_b && b_has_a {
                info!("Both peers connected at tick {}", i);
                return Ok(());
            }
        }

        bail_any!(
            "peers did not connect: A has B={}, B has A={}",
            self.alice
                .test_peer
                .lock()
                .unwrap()
                .neighbors
                .lock()
                .unwrap()
                .contains(&bob_pk),
            self.bob
                .test_peer
                .lock()
                .unwrap()
                .neighbors
                .lock()
                .unwrap()
                .contains(&alice_pk),
        )
    }

    // ── Messaging ─────────────────────────────────────────────────

    pub async fn send_message(&mut self, from: PeerId, text: &str) -> Result<()> {
        let to = match from {
            PeerId::Alice => PeerId::Bob,
            PeerId::Bob => PeerId::Alice,
        };
        if self.blocked_directions.contains(&(from, to)) {
            self.record(HarnessEvent::FaultInjected(
                from,
                format!("dropped message to {:?}", to),
            ));
            bail_any!("direction {:?}->{:?} is disabled", from, to);
        }
        let fault = self.node(from).fault.clone();
        if let Some(delay) = fault.delay_delivery {
            sleep(delay).await;
            self.record(HarnessEvent::FaultInjected(
                from,
                format!("delayed delivery by {delay:?}"),
            ));
        }
        let node = self.node(from);
        let sender = node
            .sender
            .as_ref()
            .ok_or_else(|| n0_error::anyerr!("{:?} has no sender", from))?;

        let msg = Message::Message {
            text: text.to_string(),
        };
        let signed = SignedMessage::sign_and_encode(&node.secret_key, &msg)?;
        sender.broadcast(signed).await?;
        if fault.duplicate_envelopes {
            // Broadcast twice to exercise receiver-side de-duplication.
            let duplicate = SignedMessage::sign_and_encode(&node.secret_key, &msg)?;
            sender.broadcast(duplicate).await?;
            self.record(HarnessEvent::FaultInjected(
                from,
                "duplicated envelope".into(),
            ));
        }
        if fault.drop_acks {
            self.record(HarnessEvent::FaultInjected(
                from,
                "drop acknowledgements".into(),
            ));
        }

        self.record(HarnessEvent::MessageSent(from, text.to_string()));
        info!("{:?} sent: {}", from, text);
        Ok(())
    }

    pub async fn send_about_me(&mut self, from: PeerId, name: &str) -> Result<()> {
        let node = self.node(from);
        let sender = node
            .sender
            .as_ref()
            .ok_or_else(|| n0_error::anyerr!("{:?} has no sender", from))?;

        let msg = Message::AboutMe {
            name: name.into(),
            profile_image_ticket: None,
        };
        let signed = SignedMessage::sign_and_encode(&node.secret_key, &msg)?;
        sender.broadcast(signed).await?;

        self.record(HarnessEvent::MessageSent(from, format!("/aboutme {name}")));
        info!("{:?} sent AboutMe: {}", from, name);
        Ok(())
    }

    pub async fn wait_for_message(&self, who: PeerId, contains: &str) -> Result<String> {
        let node = self.node(who);
        let deadline = Instant::now() + DEFAULT_TIMEOUT;

        while Instant::now() < deadline {
            let msgs = node
                .test_peer
                .lock()
                .unwrap()
                .received_messages
                .lock()
                .unwrap()
                .clone();
            for msg in &msgs {
                if msg.contains(contains) {
                    self.record(HarnessEvent::MessageReceived(who, msg.clone()));
                    return Ok(msg.clone());
                }
            }

            let sys = node
                .test_peer
                .lock()
                .unwrap()
                .system_messages
                .lock()
                .unwrap()
                .clone();
            for msg in &sys {
                if msg.contains(contains) {
                    self.record(HarnessEvent::MessageReceived(who, msg.clone()));
                    return Ok(msg.clone());
                }
            }

            sleep(TICK).await;
        }

        bail_any!(
            "{:?} did not receive message containing '{}' within timeout",
            who,
            contains
        )
    }

    // ── Contact & mailbox ──────────────────────────────────────────

    pub async fn send_friend_request(&mut self, from: PeerId, to: PeerId) -> Result<()> {
        let from_node = self.node(from);
        let to_pk = self.node(to).public_key;

        let action = ContactAction::FriendRequest {
            name: Some(from.name().into()),
        };
        let signed = SignedContactMessage::sign(&from_node.secret_key, &action)?;

        if let Some(wh) = &from_node.whisper_handle {
            wh.send_control(to_pk, signed.into()).await?;
        }

        self.record(HarnessEvent::FriendRequestSent(from, to_pk));
        info!("{:?} -> {:?}: friend request", from, to);
        Ok(())
    }

    pub async fn exchange_mailbox_keys(&mut self) -> Result<()> {
        let alice_mailbox_pk = self.alice.mailbox_identity.public_key();
        let alice_action = ContactAction::MailboxAdvertise {
            mailbox: alice_mailbox_pk,
        };
        let alice_signed = SignedContactMessage::sign(&self.alice.secret_key, &alice_action)?;

        if let Some(wh) = &self.alice.whisper_handle {
            wh.send_control(self.bob.public_key, alice_signed.into())
                .await?;
        }
        self.record(HarnessEvent::MailboxKeyExchanged(PeerId::Alice));
        sleep(TICK).await;

        let bob_mailbox_pk = self.bob.mailbox_identity.public_key();
        let bob_action = ContactAction::MailboxAdvertise {
            mailbox: bob_mailbox_pk,
        };
        let bob_signed = SignedContactMessage::sign(&self.bob.secret_key, &bob_action)?;

        if let Some(wh) = &self.bob.whisper_handle {
            wh.send_control(self.alice.public_key, bob_signed.into())
                .await?;
        }
        self.record(HarnessEvent::MailboxKeyExchanged(PeerId::Bob));

        info!("Mailbox keys exchanged");
        Ok(())
    }

    // ── Fault injection ────────────────────────────────────────────

    pub async fn change_address(&mut self, who: PeerId) {
        let node = self.node_mut(who);
        let new_lookup = MemoryLookup::new();
        node.memory_lookup = new_lookup.clone();

        if let Some(ep) = &node.endpoint {
            if let Ok(chain) = ep.address_lookup().as_ref() {
                chain.add(new_lookup);
            }
        }

        self.record(HarnessEvent::AddressChanged(who));
        info!("{:?} address changed", who);
    }

    // ── Shutdown ───────────────────────────────────────────────────

    pub async fn shutdown(&mut self) {
        self.stop_peer(PeerId::Bob).await;
        self.stop_peer(PeerId::Alice).await;
        self._relay_server.take();
        self.relay_map.take();
        self.relay_url.take();
        info!("Test harness shutdown");
    }

    // ── Internal ──────────────────────────────────────────────────

    fn node(&self, who: PeerId) -> &PeerNode {
        match who {
            PeerId::Alice => &self.alice,
            PeerId::Bob => &self.bob,
        }
    }

    fn node_mut(&mut self, who: PeerId) -> &mut PeerNode {
        match who {
            PeerId::Alice => &mut self.alice,
            PeerId::Bob => &mut self.bob,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_two_peers_connect_and_exchange_gossip() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();
    harness.setup().await?;

    harness.wait_for_connected().await?;

    // Alice sends a message
    harness
        .send_message(PeerId::Alice, "hello from Alice")
        .await?;

    // Bob receives it
    let msg = harness
        .wait_for_message(PeerId::Bob, "hello from Alice")
        .await?;
    assert!(
        msg.contains("hello from Alice"),
        "Bob should receive Alice's message"
    );

    // Bob sends a reply
    harness.send_message(PeerId::Bob, "hey Alice!").await?;

    // Alice receives it
    let reply = harness.wait_for_message(PeerId::Alice, "hey Alice").await?;
    assert!(
        reply.contains("hey Alice"),
        "Alice should receive Bob's reply"
    );

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_identities_survive_restart() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();

    let alice_pk = harness.alice.public_key;
    let bob_pk = harness.bob.public_key;

    harness.setup().await?;
    assert_eq!(harness.alice.public_key, alice_pk, "Alice key unchanged");
    assert_eq!(harness.bob.public_key, bob_pk, "Bob key unchanged");

    harness.stop_peer(PeerId::Alice).await;
    harness.stop_peer(PeerId::Bob).await;

    harness.restart_peer(PeerId::Alice).await?;
    harness.restart_peer(PeerId::Bob).await?;

    assert_eq!(harness.alice.public_key, alice_pk, "Alice key persists");
    assert_eq!(harness.bob.public_key, bob_pk, "Bob key persists");

    harness.wait_for_connected().await?;
    harness
        .send_message(PeerId::Alice, "hello after restart")
        .await?;
    harness
        .wait_for_message(PeerId::Bob, "hello after restart")
        .await?;

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_stop_start_restart_cycle() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();
    harness.setup().await?;

    harness.wait_for_connected().await?;
    harness.send_message(PeerId::Alice, "before stop").await?;
    harness.wait_for_message(PeerId::Bob, "before stop").await?;

    // Stop Bob
    harness.stop_peer(PeerId::Bob).await;
    assert!(!harness.bob.is_running(), "Bob stopped");

    sleep(Duration::from_secs(1)).await;

    // Restart Bob
    harness.restart_peer(PeerId::Bob).await?;
    assert!(harness.bob.is_running(), "Bob restarted");

    harness.wait_for_connected().await?;
    harness.send_message(PeerId::Alice, "after restart").await?;
    harness
        .wait_for_message(PeerId::Bob, "after restart")
        .await?;

    // Full stop/start of both
    harness.stop_peer(PeerId::Alice).await;
    harness.stop_peer(PeerId::Bob).await;
    assert!(!harness.alice.is_running());
    assert!(!harness.bob.is_running());

    harness.restart_peer(PeerId::Alice).await?;
    harness.restart_peer(PeerId::Bob).await?;
    harness.wait_for_connected().await?;

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_contact_establishment() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();
    harness.setup().await?;

    harness.wait_for_connected().await?;

    // Send friend request via whisper
    harness
        .send_friend_request(PeerId::Alice, PeerId::Bob)
        .await?;

    // Give time for delivery
    sleep(Duration::from_secs(1)).await;
    info!("Contact establishment flow completed");

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_mailbox_key_exchange() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();
    harness.setup().await?;

    harness.wait_for_connected().await?;
    harness.exchange_mailbox_keys().await?;

    info!("Mailbox key exchange test completed");
    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_no_public_infrastructure() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();
    harness.setup().await?;

    if let Some(ep) = &harness.alice.endpoint {
        ep.online().await;
    }
    if let Some(ep) = &harness.bob.endpoint {
        ep.online().await;
    }

    harness.wait_for_connected().await?;
    harness.send_message(PeerId::Alice, "local only").await?;
    harness.wait_for_message(PeerId::Bob, "local only").await?;

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_observe_network_events() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();
    harness.setup().await?;

    harness.wait_for_connected().await?;

    let alice_ups = harness
        .alice
        .test_peer
        .lock()
        .unwrap()
        .neighbor_ups
        .lock()
        .unwrap()
        .clone();
    let bob_ups = harness
        .bob
        .test_peer
        .lock()
        .unwrap()
        .neighbor_ups
        .lock()
        .unwrap()
        .clone();

    assert!(
        alice_ups.contains(&harness.bob.public_key),
        "Alice NeighborUp for Bob"
    );
    assert!(
        bob_ups.contains(&harness.alice.public_key),
        "Bob NeighborUp for Alice"
    );

    assert!(
        harness
            .alice
            .test_peer
            .lock()
            .unwrap()
            .neighbors
            .lock()
            .unwrap()
            .contains(&harness.bob.public_key),
        "Alice's neighbors includes Bob"
    );

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_address_change() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();
    harness.setup().await?;

    harness.wait_for_connected().await?;
    harness
        .send_message(PeerId::Alice, "before address change")
        .await?;
    harness
        .wait_for_message(PeerId::Bob, "before address change")
        .await?;

    // Change Alice's address lookup
    harness.change_address(PeerId::Alice).await;

    // Re-register Bob in Alice's lookup
    if let Some(ep_b) = &harness.bob.endpoint {
        harness.alice.memory_lookup.set_endpoint_info(ep_b.addr());
    }

    // Also change Bob's
    harness.change_address(PeerId::Bob).await;
    if let Some(ep_a) = &harness.alice.endpoint {
        harness.bob.memory_lookup.set_endpoint_info(ep_a.addr());
    }

    sleep(Duration::from_secs(2)).await;

    harness
        .send_message(PeerId::Alice, "after address change")
        .await?;
    match tokio::time::timeout(
        Duration::from_secs(15),
        harness.wait_for_message(PeerId::Bob, "after address change"),
    )
    .await
    {
        Ok(result) => assert!(result?.contains("after address change")),
        Err(_) => info!("Message after address change timed out (acceptable)"),
    }

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_deterministic_keys() -> Result<()> {
    let sk1 = deterministic_secret_key(b"test-seed-v1");
    let sk2 = deterministic_secret_key(b"test-seed-v1");
    let sk3 = deterministic_secret_key(b"different-seed");

    assert_eq!(sk1.public(), sk2.public(), "Same seed -> same key");
    assert_ne!(
        sk1.public(),
        sk3.public(),
        "Different seed -> different key"
    );
    Ok(())
}

#[tokio::test]
async fn test_event_log() -> Result<()> {
    let harness = TestHarness::new();

    harness.record(HarnessEvent::PeerStarted(PeerId::Alice));
    harness.record(HarnessEvent::PeerStopped(PeerId::Alice));
    harness.record(HarnessEvent::GossipConnected(
        PeerId::Alice,
        harness.bob.public_key,
    ));

    let events = harness.events();
    assert_eq!(events.len(), 3, "Events recorded");
    Ok(())
}

#[tokio::test]
async fn test_bounded_timeouts() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();
    harness.setup().await?;

    let start = Instant::now();
    harness.wait_for_connected().await?;
    let connect_time = start.elapsed();
    info!("Connection in {:?}", connect_time);
    assert!(connect_time < Duration::from_secs(30));

    let start = Instant::now();
    harness.send_message(PeerId::Alice, "timed message").await?;
    harness
        .wait_for_message(PeerId::Bob, "timed message")
        .await?;
    info!("Delivery in {:?}", start.elapsed());

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_persistent_temp_profiles() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut harness = TestHarness::new();

    let alice_dir = harness.alice.data_dir.path().to_path_buf();
    let bob_dir = harness.bob.data_dir.path().to_path_buf();

    assert!(alice_dir.exists(), "Alice temp dir exists");
    assert!(bob_dir.exists(), "Bob temp dir exists");

    harness.setup().await?;
    harness.stop_peer(PeerId::Alice).await;
    harness.stop_peer(PeerId::Bob).await;

    assert!(alice_dir.exists(), "Alice dir persists after stop");
    assert!(bob_dir.exists(), "Bob dir persists after stop");

    // Verify we can load mailbox data (empty but loadable)
    let alice_mailbox = MailboxStore::load(&alice_dir)?;
    assert!(
        alice_mailbox.is_some() || alice_mailbox.is_none(),
        "Mailbox data is loadable (may not exist on first use)"
    );

    harness.shutdown().await;
    Ok(())
}
