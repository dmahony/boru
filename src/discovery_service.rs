//! Internal discovery subsystem — the service API for the hidden discovery
//! gossip topic.
//!
//! Every Boru node joins one internal discovery topic at startup purely as
//! **networking infrastructure** (peer discovery, presence, connectivity
//! bootstrapping). This module owns that join, the publish path, the
//! receive-path dispatch, and a small in-process peer registry — WITHOUT
//! creating or touching conversation state
//! ([`ConversationEntry`](crate::conversations::ConversationEntry) /
//! [`ConversationStore`](crate::conversations::ConversationStore)), and
//! without touching chat persistence, notifications, or rendering.
//!
//! # Deliberate separation from conversation code
//!
//! * The service never creates a conversation, never inserts into the
//!   conversation store, and never renders anything.
//! * Private direct messages and normal chat payloads must **never** be
//!   routed through the discovery topic; the discovery message types
//!   ([`DiscoveryMessage`](crate::discovery_message::DiscoveryMessage)) are
//!   a dedicated enum whose wire format cannot be confused with chat
//!   payloads.
//! * Discovery state (the [`PeerRegistry`]) is owned here and stays separate
//!   from conversation state.
//!
//! # API surfaces
//!
//! 1. [`join`](DiscoveryService::join) / [`from_subscription`](DiscoveryService::from_subscription)
//!    — join the discovery gossip topic and start the receive drain.
//! 2. [`publish`](DiscoveryService::publish) — broadcast a
//!    [`DiscoveryMessage`] (Hello / Presence / PeerAdvertisement).
//! 3. [`handle_incoming`](DiscoveryService::handle_incoming) — deserialise +
//!    dispatch one received payload. This is the pure receive-path core: it
//!    takes bytes (no network), so it is directly unit-testable.
//! 4. [`peer_updates`](DiscoveryService::peer_updates) — a live stream of
//!    [`PeerUpdate`]s for callers, backed by the authoritative
//!    [`PeerRegistry`].
//!
//! # Peer registry
//!
//! The registry maps `node_id` → last-seen / source-topic metadata. It is
//! the dedup anchor later discovery tasks (e.g. BORU-DISC-17) build on: a
//! node already registered is not re-announced as new.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bytes::Bytes;
use iroh_base::PublicKey;
use n0_error::{e, stack_error};
use n0_future::StreamExt;
use tokio::{
    sync::broadcast,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use crate::api::{ApiError, Event, GossipReceiver, GossipSender, Message as GossipMessage};
use crate::discovery_message::{check_discovery_version, DiscoveryMessage, DiscoveryVersionCheck};
use crate::proto::TopicId;

/// Capacity of the peer-update broadcast channel.
const PEER_UPDATES_CAPACITY: usize = 256;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Which discovery message kind announced a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerSource {
    /// The peer announced itself with a `Hello` after joining the topic.
    Hello,
    /// The peer sent a periodic `Presence` heartbeat.
    Presence,
    /// The peer was the sender of a `PeerAdvertisement`.
    PeerAdvertisement,
}

impl PeerSource {
    /// Classify a discovery message by its kind.
    pub fn from_message(message: &DiscoveryMessage) -> Self {
        match message {
            DiscoveryMessage::Hello { .. } => Self::Hello,
            DiscoveryMessage::Presence { .. } => Self::Presence,
            DiscoveryMessage::PeerAdvertisement { .. } => Self::PeerAdvertisement,
        }
    }
}

/// Live peer-discovery notifications for callers of
/// [`DiscoveryService::peer_updates`].
///
/// The service's [`PeerRegistry`] is the authoritative state; this channel is
/// a lossy notification stream (lagged subscribers miss events, as with any
/// `broadcast` channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerUpdate {
    /// A peer was seen on the discovery topic (first time or a refresh).
    Seen {
        /// The node that sent a discovery message.
        node_id: PublicKey,
        /// Which message kind announced it.
        source: PeerSource,
    },
    /// A peer was advertised by another peer — a direct-dial candidate.
    Advertised {
        /// The node that sent the advertisement.
        node_id: PublicKey,
        /// The advertised peer (candidate for a direct dial).
        advertised: PublicKey,
    },
}

/// Outcome of [`DiscoveryService::handle_incoming`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingOutcome {
    /// The payload was accepted: the peer registry was updated and a
    /// [`PeerUpdate`] was emitted.
    Processed,
    /// The payload could not be deserialised as a
    /// [`DiscoveryMessage`](crate::discovery_message::DiscoveryMessage) and
    /// was dropped.
    Undecodable,
    /// The payload spoke an unsupported protocol version and was dropped.
    UnsupportedVersion {
        /// Version found on the wire.
        found: u8,
        /// Version this node understands.
        expected: u8,
    },
    /// The payload originated from this node and was ignored.
    SelfMessage,
}

/// Per-peer metadata held in the [`PeerRegistry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRegistryEntry {
    /// When this peer was last heard from on the discovery topic.
    pub last_seen: Instant,
    /// Which discovery message kind most recently announced this peer.
    pub source: PeerSource,
    /// The gossip topic this peer was heard on.
    pub source_topic: TopicId,
}

/// In-process registry of peers seen on the internal discovery topic.
///
/// Maps `node_id` → last-seen / source-topic metadata. This is the dedup
/// anchor later discovery tasks build on: a node that has already been
/// registered is not re-announced as new.
#[derive(Debug, Default, Clone)]
pub struct PeerRegistry {
    peers: HashMap<PublicKey, PeerRegistryEntry>,
}

impl PeerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or refresh a peer entry.
    ///
    /// If the peer is already known its `last_seen` and `source` are updated;
    /// otherwise a fresh entry is created.
    pub fn upsert(&mut self, node_id: PublicKey, source: PeerSource, source_topic: TopicId) {
        self.peers.insert(
            node_id,
            PeerRegistryEntry {
                last_seen: Instant::now(),
                source,
                source_topic,
            },
        );
    }

    /// Whether `node_id` is currently registered.
    pub fn contains(&self, node_id: &PublicKey) -> bool {
        self.peers.contains_key(node_id)
    }

    /// Look up the entry for `node_id`.
    pub fn get(&self, node_id: &PublicKey) -> Option<&PeerRegistryEntry> {
        self.peers.get(node_id)
    }

    /// Last time `node_id` was heard from, if it is registered.
    pub fn last_seen(&self, node_id: &PublicKey) -> Option<Instant> {
        self.peers.get(node_id).map(|entry| entry.last_seen)
    }

    /// Iterate over all registered peers.
    pub fn peers(&self) -> impl Iterator<Item = (&PublicKey, &PeerRegistryEntry)> {
        self.peers.iter()
    }

    /// Number of registered peers.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether the registry has no peers.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Remove peers not heard from within `max_age`, returning their ids.
    ///
    /// Used by the (later) presence-expiry loop; kept here so the expiry
    /// policy is unit-testable in isolation.
    pub fn prune_older_than(&mut self, max_age: Duration) -> Vec<PublicKey> {
        let cutoff = Instant::now() - max_age;
        let mut removed = Vec::new();
        self.peers.retain(|node_id, entry| {
            let keep = entry.last_seen >= cutoff;
            if !keep {
                removed.push(*node_id);
            }
            keep
        });
        removed
    }

    /// Remove every peer, returning the removed ids.
    pub fn clear(&mut self) -> Vec<PublicKey> {
        let removed: Vec<PublicKey> = self.peers.keys().copied().collect();
        self.peers.clear();
        removed
    }
}

/// Errors returned by [`DiscoveryService`] operations.
#[stack_error(derive, add_meta, from_sources)]
#[non_exhaustive]
pub enum DiscoveryServiceError {
    /// The underlying gossip API failed.
    #[error("gossip API error")]
    Api {
        /// The gossip API error.
        #[error(std_err)]
        source: ApiError,
    },
    /// Discovery message serialisation failed.
    #[error("discovery message serialisation failed")]
    Serialize {
        /// The postcard serialisation error.
        #[error(std_err)]
        source: postcard::Error,
    },
}

// ---------------------------------------------------------------------------
// Receive core (pure, offline-testable)
// ---------------------------------------------------------------------------

/// Receive-path state shared between the drain task and the service handle.
#[derive(Clone, Debug)]
struct ReceiveCore {
    /// Local node identity — used to ignore self-originated messages.
    local_node: PublicKey,
    /// The discovery topic this core is bound to.
    topic: TopicId,
    /// Shared peer registry.
    registry: Arc<Mutex<PeerRegistry>>,
    /// Broadcast channel of peer updates for callers.
    peer_updates_tx: broadcast::Sender<PeerUpdate>,
}

impl ReceiveCore {
    /// Deserialise + dispatch one received discovery payload.
    ///
    /// The receive-path gate order is deliberately: deserialise → protocol
    /// version check → self-filter → registry update. Unknown versions and
    /// undecodable payloads are dropped (and logged), never interpreted.
    fn handle_incoming(&self, content: &[u8], delivered_from: PublicKey) -> IncomingOutcome {
        let message = match postcard::from_bytes::<DiscoveryMessage>(content) {
            Ok(message) => message,
            Err(error) => {
                debug!(
                    delivered_from = %delivered_from.fmt_short(),
                    error = %error,
                    "discovery: undecodable payload dropped",
                );
                return IncomingOutcome::Undecodable;
            }
        };

        match check_discovery_version(message.protocol_version()) {
            DiscoveryVersionCheck::Supported => {}
            DiscoveryVersionCheck::Unsupported { found, expected } => {
                warn!(
                    delivered_from = %delivered_from.fmt_short(),
                    found,
                    expected,
                    "discovery: unsupported protocol version dropped",
                );
                return IncomingOutcome::UnsupportedVersion { found, expected };
            }
        }

        let node_id = message.node_id();
        if node_id == self.local_node {
            trace!(node = %node_id.fmt_short(), "discovery: ignoring self message");
            return IncomingOutcome::SelfMessage;
        }

        let source = PeerSource::from_message(&message);
        {
            let mut registry = self.registry.lock().expect("peer registry lock poisoned");
            let was_new = !registry.contains(&node_id);
            registry.upsert(node_id, source, self.topic);
            if was_new {
                info!(
                    node = %node_id.fmt_short(),
                    source = ?source,
                    "discovery: new peer seen",
                );
            } else {
                trace!(
                    node = %node_id.fmt_short(),
                    source = ?source,
                    "discovery: peer refresh",
                );
            }
        }

        // The registry is authoritative; the channel is a live notification
        // stream. Send errors only mean a caller stopped listening.
        let _ = self
            .peer_updates_tx
            .send(PeerUpdate::Seen { node_id, source });

        if let DiscoveryMessage::PeerAdvertisement { advertised, .. } = message {
            if advertised != self.local_node && advertised != node_id {
                let _ = self.peer_updates_tx.send(PeerUpdate::Advertised {
                    node_id,
                    advertised,
                });
            }
        }

        IncomingOutcome::Processed
    }
}

// ---------------------------------------------------------------------------
// DiscoveryService
// ---------------------------------------------------------------------------

/// The internal discovery subsystem.
///
/// Owns the discovery gossip topic subscription (join), message publishing,
/// receive-path dispatch, and a small in-process peer registry. The service
/// is deliberately independent of `Conversation`, `AppState`, and
/// `ChatCallbacks`: it never creates a conversation, never touches chat
/// persistence or rendering, and its receive path is testable without any
/// network (feed postcard bytes into [`DiscoveryService::handle_incoming`]).
///
/// # Lifecycle
///
/// 1. [`join`](Self::join) or [`from_subscription`](Self::from_subscription)
///    — subscribe to the discovery topic and start the drain task.
/// 2. [`publish`](Self::publish) — broadcast discovery messages.
/// 3. [`handle_incoming`](Self::handle_incoming) — process received payloads
///    (the drain task calls this; tests call it directly).
/// 4. [`peer_updates`](Self::peer_updates) — subscribe to live peer updates.
/// 5. [`shutdown`](Self::shutdown) — cancel the drain task and await it.
///
/// Dropping the handle **without** calling `shutdown` aborts the drain task.
#[derive(Debug)]
pub struct DiscoveryService {
    /// Topic this service joined.
    topic: TopicId,
    /// Sender half of the gossip subscription — kept alive by the service so
    /// the discovery topic stays joined.
    sender: GossipSender,
    /// Receive-path core (registry + update channel + dispatch logic).
    core: ReceiveCore,
    /// Cancellation token shared with the drain task.
    cancel: CancellationToken,
    /// Join handle of the background drain task.
    task: JoinHandle<()>,
}

impl DiscoveryService {
    /// Join the internal discovery gossip topic and start the service.
    ///
    /// Subscribes `gossip` to `topic` with the given `bootstrap` peers, then
    /// splits the subscription and spawns the receive drain. Equivalent to
    /// calling [`from_subscription`](Self::from_subscription) on the result
    /// of `gossip.subscribe(topic, bootstrap)`.
    pub async fn join(
        gossip: &crate::net::Gossip,
        topic: TopicId,
        bootstrap: Vec<PublicKey>,
        local_node: PublicKey,
    ) -> Result<Self, ApiError> {
        let subscription = gossip.subscribe(topic, bootstrap).await?;
        let (sender, receiver) = subscription.split();
        Ok(Self::from_subscription(topic, sender, receiver, local_node))
    }

    /// Build a running service from an already-created subscription.
    ///
    /// Splits the [`GossipSender`] / [`GossipReceiver`] halves, spawns the
    /// background drain task, and returns the service handle. This is the
    /// offline-friendly constructor used by tests and by callers that already
    /// hold a subscription.
    pub fn from_subscription(
        topic: TopicId,
        sender: GossipSender,
        receiver: GossipReceiver,
        local_node: PublicKey,
    ) -> Self {
        let registry = Arc::new(Mutex::new(PeerRegistry::new()));
        let (peer_updates_tx, _) = broadcast::channel(PEER_UPDATES_CAPACITY);
        let core = ReceiveCore {
            local_node,
            topic,
            registry,
            peer_updates_tx,
        };
        let cancel = CancellationToken::new();
        let task_core = core.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(drain_loop(receiver, task_core, task_cancel));
        info!(topic = %topic, "discovery service joined");
        Self {
            topic,
            sender,
            core,
            cancel,
            task,
        }
    }

    /// The discovery topic this service is joined to.
    pub fn topic(&self) -> TopicId {
        self.topic
    }

    /// Publish a discovery message to the discovery topic.
    ///
    /// Serialises with postcard and broadcasts through the gossip sender.
    pub async fn publish(&self, message: DiscoveryMessage) -> Result<(), DiscoveryServiceError> {
        let bytes = postcard::to_stdvec(&message)
            .map_err(|source| e!(DiscoveryServiceError::Serialize { source }))?;
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(())
    }

    /// Handle one received discovery-topic payload.
    ///
    /// Deserialises `content` as a [`DiscoveryMessage`], applies the protocol
    /// version gate, ignores self-originated and undecodable payloads, updates
    /// the peer registry, and emits [`PeerUpdate`]s to subscribers. This is
    /// the pure receive-path core: it does not touch the network, so it can be
    /// unit-tested directly.
    pub fn handle_incoming(&self, content: &[u8], delivered_from: PublicKey) -> IncomingOutcome {
        self.core.handle_incoming(content, delivered_from)
    }

    /// Subscribe to live peer-discovery updates.
    ///
    /// The returned receiver observes [`PeerUpdate::Seen`] and
    /// [`PeerUpdate::Advertised`] events. The [`PeerRegistry`] remains the
    /// authoritative snapshot; this channel is a live notification stream.
    pub fn peer_updates(&self) -> broadcast::Receiver<PeerUpdate> {
        self.core.peer_updates_tx.subscribe()
    }

    /// Snapshot of the currently known peers.
    ///
    /// Each entry pairs the node id with its registry metadata. Use
    /// [`peer_count`](Self::peer_count) for a cheap size check.
    pub fn known_peers(&self) -> Vec<(PublicKey, PeerRegistryEntry)> {
        let registry = self.core.registry.lock().expect("peer registry lock poisoned");
        registry.peers().map(|(node_id, entry)| (*node_id, entry.clone())).collect()
    }

    /// Number of peers currently in the registry.
    pub fn peer_count(&self) -> usize {
        let registry = self.core.registry.lock().expect("peer registry lock poisoned");
        registry.len()
    }

    /// Shut down the service: cancel the drain task and await it.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task.await;
        info!(topic = %self.topic, "discovery service shut down");
    }
}

// ---------------------------------------------------------------------------
// Drain loop
// ---------------------------------------------------------------------------

/// Background task that drains the gossip receiver and feeds every received
/// payload through the receive core.
async fn drain_loop(
    mut receiver: GossipReceiver,
    core: ReceiveCore,
    cancel: CancellationToken,
) {
    info!("discovery service drain loop started");
    let mut event_count: u64 = 0;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery service drain cancelled");
                break;
            }
            event = receiver.next() => {
                match event {
                    Some(Ok(Event::Received(GossipMessage { content, delivered_from, .. }))) => {
                        event_count += 1;
                        let outcome = core.handle_incoming(&content, delivered_from);
                        trace!(outcome = ?outcome, "discovery: received event handled");
                    }
                    Some(Ok(Event::NeighborUp(peer))) => {
                        trace!(peer = %peer.fmt_short(), "discovery: neighbor up");
                    }
                    Some(Ok(Event::NeighborDown(peer))) => {
                        trace!(peer = %peer.fmt_short(), "discovery: neighbor down");
                    }
                    Some(Ok(Event::Lagged)) => {
                        warn!("discovery receiver lagged — events dropped");
                    }
                    Some(Ok(Event::MissingMessages { since_round, from_peer })) => {
                        debug!(
                            ?since_round,
                            from_peer = %from_peer.fmt_short(),
                            "discovery: missing-message gap",
                        );
                    }
                    Some(Err(error)) => {
                        warn!(error = %error, "discovery: receiver error, exiting drain");
                        break;
                    }
                    None => {
                        debug!("discovery receiver closed");
                        break;
                    }
                }
            }
        }
    }

    info!(event_count, "discovery service drain loop exited");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Command;
    use crate::proto::DeliveryScope;
    use irpc::channel::mpsc as irpc_mpsc;

    /// Deterministic test identity: a `SecretKey` seeded from a single byte
    /// produces a valid Ed25519 public key.
    fn test_key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        let sk = iroh_base::SecretKey::from_bytes(&seed);
        sk.public()
    }

    fn test_topic() -> TopicId {
        crate::discovery_topic::discovery_topic(crate::public_room::PublicNetwork::Test)
    }

    /// Build a running service over offline (never-fed) gossip channels.
    fn test_service(local_node: PublicKey) -> DiscoveryService {
        let (cmd_tx, _cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        DiscoveryService::from_subscription(test_topic(), sender, receiver, local_node)
    }

    /// Encode a `Hello` with a mutated protocol-version byte.
    fn hello_with_version(byte: u8, version: u8) -> Vec<u8> {
        let mut bytes = postcard::to_stdvec(&DiscoveryMessage::hello(test_key(byte))).unwrap();
        bytes[1] = version;
        bytes
    }

    // ── Registry ──────────────────────────────────────────────────────

    #[test]
    fn registry_upsert_and_accessors() {
        let mut registry = PeerRegistry::new();
        assert!(registry.is_empty());

        let node = test_key(0x01);
        let topic = test_topic();
        assert!(!registry.contains(&node));
        registry.upsert(node, PeerSource::Hello, topic);

        assert!(registry.contains(&node));
        assert_eq!(registry.len(), 1);
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Hello);
        assert_eq!(entry.source_topic, topic);
        assert!(registry.last_seen(&node).is_some());

        // Refresh with a different source updates the entry, not the count.
        registry.upsert(node, PeerSource::Presence, topic);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get(&node).unwrap().source, PeerSource::Presence);

        let collected: Vec<PublicKey> = registry.peers().map(|(id, _)| *id).collect();
        assert_eq!(collected, vec![node]);
    }

    #[test]
    fn registry_prune_older_than_removes_only_stale() {
        let mut registry = PeerRegistry::new();
        let topic = test_topic();
        let fresh = test_key(0x10);
        let stale = test_key(0x11);

        // Insert the "stale" peer, then backdate it (tests are a child
        // module, so they can reach the private map directly).
        registry.upsert(stale, PeerSource::Hello, topic);
        registry
            .peers
            .get_mut(&stale)
            .unwrap()
            .last_seen = Instant::now() - Duration::from_secs(3600);
        registry.upsert(fresh, PeerSource::Presence, topic);

        let removed = registry.prune_older_than(Duration::from_secs(60));
        assert_eq!(removed, vec![stale]);
        assert!(!registry.contains(&stale));
        assert!(registry.contains(&fresh));
    }

    // ── handle_incoming (pure, offline) ───────────────────────────────

    #[tokio::test]
    async fn handle_incoming_hello_registers_peer() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(outcome, IncomingOutcome::Processed);

        assert_eq!(service.peer_count(), 1);
        let known = service.known_peers();
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].0, peer);
        assert_eq!(known[0].1.source, PeerSource::Hello);
        assert_eq!(known[0].1.source_topic, test_topic());

        // A Seen update was emitted.
        assert_eq!(
            updates.try_recv(),
            Ok(PeerUpdate::Seen {
                node_id: peer,
                source: PeerSource::Hello,
            })
        );
    }

    #[tokio::test]
    async fn handle_incoming_presence_refreshes_existing_peer() {
        let local = test_key(0xAA);
        let peer = test_key(0xCC);
        let service = test_service(local);

        let hello = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        service.handle_incoming(&hello, peer);
        let first_seen = service.known_peers()[0].1.last_seen;

        let presence = postcard::to_stdvec(&DiscoveryMessage::presence(peer)).unwrap();
        let outcome = service.handle_incoming(&presence, peer);
        assert_eq!(outcome, IncomingOutcome::Processed);
        assert_eq!(service.peer_count(), 1);
        assert_eq!(service.known_peers()[0].1.source, PeerSource::Presence);
        assert!(service.known_peers()[0].1.last_seen >= first_seen);
    }

    #[tokio::test]
    async fn handle_incoming_peer_advertisement_registers_sender_and_emits_advertised() {
        let local = test_key(0xAA);
        let sender = test_key(0xDD);
        let advertised = test_key(0xEE);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let bytes =
            postcard::to_stdvec(&DiscoveryMessage::peer_advertisement(sender, advertised)).unwrap();
        let outcome = service.handle_incoming(&bytes, sender);
        assert_eq!(outcome, IncomingOutcome::Processed);

        // Sender is registered with source PeerAdvertisement; the advertised
        // peer is NOT registered (it is only a dial candidate).
        assert_eq!(service.peer_count(), 1);
        assert_eq!(service.known_peers()[0].0, sender);
        assert_eq!(
            service.known_peers()[0].1.source,
            PeerSource::PeerAdvertisement
        );

        // Both a Seen and an Advertised update were emitted.
        assert_eq!(
            updates.try_recv(),
            Ok(PeerUpdate::Seen {
                node_id: sender,
                source: PeerSource::PeerAdvertisement,
            })
        );
        assert_eq!(
            updates.try_recv(),
            Ok(PeerUpdate::Advertised {
                node_id: sender,
                advertised,
            })
        );
    }

    #[tokio::test]
    async fn handle_incoming_unknown_version_ignored() {
        let local = test_key(0xAA);
        let peer = test_key(0x07);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let bytes = hello_with_version(0x07, 99);
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(
            outcome,
            IncomingOutcome::UnsupportedVersion {
                found: 99,
                expected: crate::discovery_topic::BORU_DISCOVERY_PROTOCOL_VERSION,
            }
        );
        assert_eq!(service.peer_count(), 0);
        assert!(updates.try_recv().is_err());
    }

    #[tokio::test]
    async fn handle_incoming_undecodable_ignored() {
        let local = test_key(0xAA);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let outcome = service.handle_incoming(b"this is not a discovery message", test_key(0x42));
        assert_eq!(outcome, IncomingOutcome::Undecodable);
        assert_eq!(service.peer_count(), 0);
        assert!(updates.try_recv().is_err());
    }

    #[tokio::test]
    async fn handle_incoming_self_message_ignored() {
        let local = test_key(0xAA);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(local)).unwrap();
        let outcome = service.handle_incoming(&bytes, local);
        assert_eq!(outcome, IncomingOutcome::SelfMessage);
        assert_eq!(service.peer_count(), 0);
        assert!(updates.try_recv().is_err());
    }

    // ── publish (offline over a channel) ──────────────────────────────

    #[tokio::test]
    async fn publish_serializes_and_broadcasts() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        let peer = test_key(0xBB);
        service
            .publish(DiscoveryMessage::hello(peer))
            .await
            .unwrap();

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for broadcast command")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, DiscoveryMessage::hello(peer));
    }

    // ── drain loop (offline over a channel) ───────────────────────────

    #[tokio::test]
    async fn drain_loop_forwards_received_events() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (cmd_tx, _cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);
        let mut updates = service.peer_updates();

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        ev_tx
            .send(Event::Received(GossipMessage {
                content: Bytes::from(bytes),
                scope: DeliveryScope::Neighbors,
                delivered_from: peer,
            }))
            .await
            .unwrap();

        // The drain task processes the event and emits a Seen update.
        let update = tokio::time::timeout(Duration::from_secs(5), updates.recv())
            .await
            .expect("timed out waiting for peer update")
            .expect("update channel closed");
        assert_eq!(
            update,
            PeerUpdate::Seen {
                node_id: peer,
                source: PeerSource::Hello,
            }
        );
        assert_eq!(service.peer_count(), 1);
    }
}
