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
//! 5. [`announce_hello`](DiscoveryService::announce_hello) /
//!    [`announce_presence`](DiscoveryService::announce_presence) — throttled
//!    presence announcements (guarded by [`AnnounceThrottle`]).
//! 6. **Connectivity wiring (Phase 4, BORU-DISC-11)** — every newly
//!    discovered peer (Hello / Presence / PeerAdvertisement) is dialed into
//!    the discovery gossip mesh via [`GossipSender::join_peers`], the same
//!    mechanism the mDNS and DHT discovery paths use today. This improves
//!    connectivity ONLY: it never creates a friendship, a group membership,
//!    or a conversation, and no chat payload is ever routed through the
//!    discovery topic.
//!
//! # Connectivity wiring (BORU-DISC-11)
//!
//! Discovery is networking infrastructure: when the service learns about a
//! valid peer it updates the networking peer/address book through the
//! existing trusted node identity mechanism ([`GossipSender::join_peers`],
//! exactly what `main.rs`'s mDNS handler and [`DynamicPeerJoiner`] do for
//! mDNS/DHT results). The wiring task subscribes to the [`peer_updates`]
//! broadcast and dials each newly seen/advertised peer once (deduplicated by
//! endpoint id). Friendship state stays in Boru's friend/request model,
//! group membership determines which group topics are joined, and public
//! room membership remains explicit — discovery never grants any of them.
//!
//! [`DynamicPeerJoiner`]: crate::dynamic_joiner::DynamicPeerJoiner
//! [`peer_updates`]: DiscoveryService::peer_updates
//!
//! # Peer registry
//!
//! The registry maps `node_id` → last-seen / source-topic metadata. It is
//! the dedup anchor later discovery tasks (e.g. BORU-DISC-17) build on: a
//! node already registered is not re-announced as new.
//!
//! # Announcement policy (BORU-DISC-09)
//!
//! The service announces its presence with a minimal `Hello` (protocol
//! version + node id, 34 bytes on the wire):
//!
//! * **On join** — [`join`](DiscoveryService::join) publishes one `Hello`
//!   immediately after the subscription succeeds, so existing nodes on the
//!   discovery topic learn about the new node without any chat message
//!   being created.
//! * **On neighbour-up** — the drain loop re-announces a `Hello` when a new
//!   gossip neighbour joins the mesh (reconnect / late-joiner path), so a
//!   neighbour that connected after our join hello still hears us.
//! * **Throttle** — every announcement passes through a minimum-interval
//!   throttle ([`AnnounceThrottle`], default
//!   [`DEFAULT_ANNOUNCE_MIN_INTERVAL`] = 30 s). The first announcement
//!   always passes; later announcements within the interval are suppressed
//!   ([`AnnounceOutcome::Throttled`]). This prevents aggressive broadcast
//!   loops under neighbour churn while still guaranteeing one hello per
//!   join.
//! * **Self-filter** — the receive path ignores messages whose node id
//!   equals the local identity ([`IncomingOutcome::SelfMessage`]), so the
//!   gossip mesh's echo of our own hello never registers us in the peer
//!   registry (mirroring `chat_core`'s `local_public()` self filter).

use std::{
    collections::{HashMap, HashSet},
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

/// Default minimum interval between discovery announcements (Hello /
/// Presence). Announcements are throttled to at most one per interval so a
/// join hello plus neighbour-up re-announcements cannot become an aggressive
/// broadcast loop on the discovery topic.
pub const DEFAULT_ANNOUNCE_MIN_INTERVAL: Duration = Duration::from_secs(30);

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

/// Outcome of a throttled discovery announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceOutcome {
    /// The announcement was broadcast to the discovery topic.
    Announced,
    /// The announcement was suppressed by the throttle (too soon since the
    /// last one); nothing was broadcast.
    Throttled,
}

/// Minimum-interval throttle for discovery announcements.
///
/// At most one announcement is broadcast per [`min_interval`](Self::min_interval)
/// (default [`DEFAULT_ANNOUNCE_MIN_INTERVAL`]). The very first announcement
/// always passes; later attempts within the interval are suppressed. This
/// prevents aggressive broadcast loops (join + neighbour-up + presence must
/// not spam the discovery topic) while still guaranteeing one hello per
/// join.
///
/// The throttle is cheaply shareable (`Arc`): the service handle and the
/// drain loop use the same instance, so join-time and neighbour-up
/// announcements share one policy.
#[derive(Debug)]
pub struct AnnounceThrottle {
    state: Mutex<AnnounceThrottleState>,
}

#[derive(Debug)]
struct AnnounceThrottleState {
    /// Minimum spacing between allowed announcements.
    min_interval: Duration,
    /// When the last announcement was broadcast (`None` = never yet).
    last_announce: Option<Instant>,
}

impl AnnounceThrottle {
    /// A throttle using the default interval
    /// ([`DEFAULT_ANNOUNCE_MIN_INTERVAL`]).
    pub fn new() -> Self {
        Self::with_min_interval(DEFAULT_ANNOUNCE_MIN_INTERVAL)
    }

    /// A throttle with a custom minimum interval (tests use short intervals
    /// to exercise the throttle without sleeping).
    pub fn with_min_interval(min_interval: Duration) -> Self {
        Self {
            state: Mutex::new(AnnounceThrottleState {
                min_interval,
                last_announce: None,
            }),
        }
    }

    /// The configured minimum interval between announcements.
    pub fn min_interval(&self) -> Duration {
        self.state.lock().expect("announce throttle lock poisoned").min_interval
    }

    /// Update the minimum interval between announcements.
    ///
    /// Safe to call while the throttle is shared (the service handle and the
    /// drain loop use the same instance).
    pub fn set_min_interval(&self, min_interval: Duration) {
        self.state.lock().expect("announce throttle lock poisoned").min_interval = min_interval;
    }

    /// Whether an announcement is allowed right now.
    ///
    /// When allowed, records the announcement time; the caller must only
    /// broadcast if this returns `true`.
    pub fn try_announce(&self) -> bool {
        let mut state = self.state.lock().expect("announce throttle lock poisoned");
        let now = Instant::now();
        let allowed = match state.last_announce {
            Some(prev) => now.duration_since(prev) >= state.min_interval,
            None => true,
        };
        if allowed {
            state.last_announce = Some(now);
        }
        allowed
    }
}

impl Default for AnnounceThrottle {
    fn default() -> Self {
        Self::new()
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
// Announcement handle (sender + throttle + local identity)
// ---------------------------------------------------------------------------

/// Shared announcement state: the gossip sender, the local node identity,
/// and the announcement throttle.
///
/// Cloned into the drain loop so neighbour-up events can re-announce
/// presence. All clones share one [`AnnounceThrottle`] via `Arc`, so
/// join-time and neighbour-up announcements observe the same minimum-interval
/// policy.
#[derive(Clone, Debug)]
struct AnnounceHandle {
    sender: GossipSender,
    local_node: PublicKey,
    throttle: Arc<AnnounceThrottle>,
}

impl AnnounceHandle {
    fn new(sender: GossipSender, local_node: PublicKey) -> Self {
        Self {
            sender,
            local_node,
            throttle: Arc::new(AnnounceThrottle::new()),
        }
    }

    /// Raw, unthrottled publish (used by [`DiscoveryService::publish`]).
    async fn publish(&self, message: DiscoveryMessage) -> Result<(), DiscoveryServiceError> {
        let bytes = postcard::to_stdvec(&message)
            .map_err(|source| e!(DiscoveryServiceError::Serialize { source }))?;
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(())
    }

    /// Throttled announce of an arbitrary discovery message.
    async fn announce(&self, message: DiscoveryMessage) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        if !self.throttle.try_announce() {
            debug!(message = ?message, "discovery: announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        self.publish(message).await?;
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce this node with a `Hello`.
    async fn announce_hello(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(DiscoveryMessage::hello(self.local_node)).await
    }

    /// Announce this node with a `Presence` heartbeat.
    async fn announce_presence(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(DiscoveryMessage::presence(self.local_node)).await
    }
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
    /// Announcement handle: gossip sender + throttle + local identity. The
    /// sender half keeps the discovery topic joined for the service lifetime.
    announce: AnnounceHandle,
    /// Receive-path core (registry + update channel + dispatch logic).
    core: ReceiveCore,
    /// Cancellation token shared with the drain and connectivity tasks.
    cancel: CancellationToken,
    /// Join handle of the background drain task.
    task: JoinHandle<()>,
    /// Join handle of the connectivity wiring task (BORU-DISC-11): dials
    /// newly discovered peers into the discovery gossip mesh.
    connectivity_task: JoinHandle<()>,
}

impl DiscoveryService {
    /// Join the internal discovery gossip topic and start the service.
    ///
    /// Subscribes `gossip` to `topic` with the given `bootstrap` peers, then
    /// splits the subscription and spawns the receive drain. Equivalent to
    /// calling [`from_subscription`](Self::from_subscription) on the result
    /// of `gossip.subscribe(topic, bootstrap)`.
    ///
    /// Immediately after the subscription succeeds, a throttled `Hello` is
    /// published so existing nodes on the discovery topic learn about this
    /// node (the join-time announcement; see the module-level
    /// [announcement policy](self#announcement-policy-boru-disc-09)). A
    /// failed announcement is non-fatal: the receive path still works and
    /// the drain loop re-announces on neighbour-up.
    pub async fn join(
        gossip: &crate::net::Gossip,
        topic: TopicId,
        bootstrap: Vec<PublicKey>,
        local_node: PublicKey,
    ) -> Result<Self, ApiError> {
        let subscription = gossip.subscribe(topic, bootstrap).await?;
        let (sender, receiver) = subscription.split();
        let service = Self::from_subscription(topic, sender, receiver, local_node);
        match service.announce_hello().await {
            Ok(AnnounceOutcome::Announced) => {
                info!(topic = %topic, "discovery hello announced on join");
            }
            Ok(AnnounceOutcome::Throttled) => {
                debug!(topic = %topic, "discovery hello suppressed on join");
            }
            Err(error) => {
                warn!(
                    topic = %topic,
                    error = %error,
                    "discovery hello on join failed; continuing without it",
                );
            }
        }
        Ok(service)
    }

    /// Build a running service from an already-created subscription.
    ///
    /// Splits the [`GossipSender`] / [`GossipReceiver`] halves, spawns the
    /// background drain task, and returns the service handle. This is the
    /// offline-friendly constructor used by tests and by callers that already
    /// hold a subscription.
    ///
    /// Does **not** announce presence automatically — call
    /// [`announce_hello`](Self::announce_hello) explicitly if the join hello
    /// is wanted (the async [`join`](Self::join) path does this itself).
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
        let announce = AnnounceHandle::new(sender, local_node);
        let cancel = CancellationToken::new();
        let task_core = core.clone();
        let task_announce = announce.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(drain_loop(receiver, task_core, task_announce, task_cancel));
        // BORU-DISC-11: connectivity wiring — dial newly discovered peers
        // into the discovery gossip mesh via join_peers (the same mechanism
        // the mDNS/DHT paths use). This improves connectivity ONLY; it never
        // grants friendship/group membership or routes chat payloads.
        let connectivity_cancel = cancel.clone();
        let connectivity_task = tokio::spawn(connectivity_loop(
            announce.sender.clone(),
            core.peer_updates_tx.subscribe(),
            local_node,
            connectivity_cancel,
        ));
        info!(topic = %topic, "discovery service joined");
        Self {
            topic,
            announce,
            core,
            cancel,
            task,
            connectivity_task,
        }
    }

    /// The discovery topic this service is joined to.
    pub fn topic(&self) -> TopicId {
        self.topic
    }

    /// Publish a discovery message to the discovery topic.
    ///
    /// Serialises with postcard and broadcasts through the gossip sender.
    /// This is the raw, **unthrottled** path — prefer
    /// [`announce_hello`](Self::announce_hello) /
    /// [`announce_presence`](Self::announce_presence) for presence
    /// announcements so the minimum-interval throttle applies.
    pub async fn publish(&self, message: DiscoveryMessage) -> Result<(), DiscoveryServiceError> {
        self.announce.publish(message).await
    }

    /// Announce this node's presence with a `Hello` on the discovery topic.
    ///
    /// The broadcast is throttled: at most one announcement per
    /// [`DEFAULT_ANNOUNCE_MIN_INTERVAL`], so repeated calls (join,
    /// neighbour-up, periodic presence) cannot produce a broadcast loop. The
    /// first announcement always passes.
    pub async fn announce_hello(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce.announce_hello().await
    }

    /// Announce a periodic `Presence` heartbeat, throttled like
    /// [`announce_hello`](Self::announce_hello).
    pub async fn announce_presence(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce.announce_presence().await
    }

    /// Override the minimum interval between announcements.
    ///
    /// Defaults to [`DEFAULT_ANNOUNCE_MIN_INTERVAL`]. Tests use short
    /// intervals to exercise the throttle without sleeping. Applies to the
    /// shared throttle used by both the service handle and the drain loop.
    pub fn with_announce_min_interval(self, min_interval: Duration) -> Self {
        self.announce.throttle.set_min_interval(min_interval);
        self
    }

    /// The minimum interval between announcements (the throttle policy).
    pub fn announce_min_interval(&self) -> Duration {
        self.announce.throttle.min_interval()
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

    /// Shut down the service: cancel the drain and connectivity tasks and
    /// await them.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task.await;
        let _ = self.connectivity_task.await;
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
    announce: AnnounceHandle,
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
                        // A new gossip neighbour joined the mesh — re-announce
                        // our presence so it can discover this node even if
                        // the join-time hello was broadcast before the
                        // neighbour connected (reconnect / late-joiner path).
                        // The minimum-interval throttle prevents neighbour
                        // churn from becoming a broadcast loop. Fire and
                        // forget: never block the receive drain on a publish.
                        let announce = announce.clone();
                        tokio::spawn(async move {
                            match announce.announce_hello().await {
                                Ok(AnnounceOutcome::Announced) => {
                                    info!(
                                        peer = %peer.fmt_short(),
                                        "discovery: re-announced hello after neighbor up",
                                    );
                                }
                                Ok(AnnounceOutcome::Throttled) => {}
                                Err(error) => {
                                    warn!(
                                        peer = %peer.fmt_short(),
                                        error = %error,
                                        "discovery: neighbor-up hello failed",
                                    );
                                }
                            }
                        });
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
// Connectivity wiring (BORU-DISC-11)
// ---------------------------------------------------------------------------

/// Background task that turns discovery peer updates into connectivity
/// actions: every newly discovered peer is dialed into the discovery gossip
/// mesh via [`GossipSender::join_peers`].
///
/// This is the Phase-4 "use discovery only to improve connectivity" wiring:
/// the same mechanism the mDNS handler in `main.rs` and
/// [`DynamicPeerJoiner`](crate::dynamic_joiner::DynamicPeerJoiner) use for
/// mDNS/DHT results. Dialing a peer improves the mesh/address book but never
/// grants friendship, group membership, or a conversation — no
/// [`FriendsStore`](crate::friends::FriendsStore), no
/// [`ConversationStore`](crate::conversations::ConversationStore), and no
/// chat payload ever crosses the discovery topic.
///
/// Deduplication: each peer is dialed at most once per service lifetime
/// (tracked by endpoint id). A `PeerUpdate::Seen` refresh or repeat
/// advertisement does not re-dial. The local node is never dialed.
async fn connectivity_loop(
    sender: GossipSender,
    mut updates: broadcast::Receiver<PeerUpdate>,
    local_node: PublicKey,
    cancel: CancellationToken,
) {
    let mut dialed: HashSet<iroh_base::EndpointId> = HashSet::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery connectivity loop cancelled");
                break;
            }
            update = updates.recv() => {
                match update {
                    Ok(PeerUpdate::Seen { node_id, .. }) => {
                        maybe_dial(&sender, &mut dialed, local_node, node_id).await;
                    }
                    Ok(PeerUpdate::Advertised { advertised, .. }) => {
                        maybe_dial(&sender, &mut dialed, local_node, advertised).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        debug!("discovery connectivity loop lagged");
                    }
                    Err(_) => break,
                }
            }
        }
    }
    debug!("discovery connectivity loop exited");
}

/// Dial `peer` into the discovery gossip mesh once (deduplicated).
///
/// Connectivity only: `join_peers` makes the gossip actor establish a mesh
/// edge / resolve the peer's address book entry through the existing
/// mechanisms — it never creates friends, groups, or conversations.
async fn maybe_dial(
    sender: &GossipSender,
    dialed: &mut HashSet<iroh_base::EndpointId>,
    local_node: PublicKey,
    peer: PublicKey,
) {
    if peer == local_node {
        trace!(peer = %peer.fmt_short(), "discovery: not dialing self");
        return;
    }
    let endpoint: iroh_base::EndpointId = peer.into();
    if !dialed.insert(endpoint) {
        trace!(peer = %peer.fmt_short(), "discovery: peer already dialed");
        return;
    }
    match sender.join_peers(vec![endpoint]).await {
        Ok(()) => {
            info!(peer = %peer.fmt_short(), "discovery: dialed discovered peer for connectivity");
        }
        Err(error) => {
            warn!(
                peer = %peer.fmt_short(),
                error = %error,
                "discovery: join_peers failed",
            );
        }
    }
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

    // ── announce (throttled presence) ─────────────────────────────────

    #[tokio::test]
    async fn announce_hello_publishes_hello_and_own_echo_is_ignored() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        let outcome = service.announce_hello().await.unwrap();
        assert_eq!(outcome, AnnounceOutcome::Announced);

        // The hello was broadcast as a DiscoveryMessage carrying this node.
        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for hello broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, DiscoveryMessage::hello(local));
        assert_eq!(decoded.node_id(), local);

        // The gossip mesh echoes our own broadcast back; the receive path
        // must ignore it so we never register ourselves.
        let outcome = service.handle_incoming(&bytes, local);
        assert_eq!(outcome, IncomingOutcome::SelfMessage);
        assert_eq!(service.peer_count(), 0);
    }

    #[tokio::test]
    async fn announce_presence_publishes_presence() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        let outcome = service.announce_presence().await.unwrap();
        assert_eq!(outcome, AnnounceOutcome::Announced);

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for presence broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, DiscoveryMessage::presence(local));
    }

    #[tokio::test]
    async fn announce_throttle_suppresses_rapid_repeat() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_announce_min_interval(Duration::from_millis(60));

        // First announcement passes the throttle.
        assert_eq!(
            service.announce_hello().await.unwrap(),
            AnnounceOutcome::Announced
        );
        // An immediate repeat is suppressed — no second broadcast.
        assert_eq!(
            service.announce_hello().await.unwrap(),
            AnnounceOutcome::Throttled
        );

        // Exactly one Broadcast command was produced.
        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for hello broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, DiscoveryMessage::hello(local));
        assert!(
            tokio::time::timeout(Duration::from_millis(40), cmd_rx.recv())
                .await
                .is_err(),
            "throttled announcement must not broadcast"
        );

        // After the interval elapses, the next announcement passes again.
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            service.announce_hello().await.unwrap(),
            AnnounceOutcome::Announced
        );
        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for second hello")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(_) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
    }

    #[test]
    fn announce_throttle_first_passes_then_suppresses_then_recovers() {
        let throttle = AnnounceThrottle::with_min_interval(Duration::from_millis(50));
        assert!(throttle.try_announce());
        assert!(!throttle.try_announce());
        std::thread::sleep(Duration::from_millis(70));
        assert!(throttle.try_announce());
    }

    #[test]
    fn announce_throttle_default_interval_is_documented() {
        assert_eq!(
            AnnounceThrottle::new().min_interval(),
            DEFAULT_ANNOUNCE_MIN_INTERVAL
        );
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

    #[tokio::test]
    async fn drain_loop_reannounces_hello_on_neighbor_up() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        // The service handle stays alive for the whole test (the drain task
        // owns clones of the receiver/announce state, so this also documents
        // that the loop runs independently of the handle).
        let _service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_announce_min_interval(Duration::from_millis(60));

        // A new gossip neighbour joins the mesh (reconnect / late-joiner
        // path) — the drain loop re-announces our hello so the neighbour can
        // discover us even if our join-time hello predates the connection.
        let peer_endpoint: iroh_base::EndpointId = peer.into();
        ev_tx.send(Event::NeighborUp(peer_endpoint)).await.unwrap();

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for neighbor-up hello")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, DiscoveryMessage::hello(local));
    }

    // ── connectivity wiring (BORU-DISC-11) ─────────────────────────────

    /// Build a running service over offline channels, keeping the command
    /// receiver live so tests can observe what the service sends to the
    /// gossip actor (connectivity side effects).
    fn test_service_with_cmd(
        local_node: PublicKey,
    ) -> (
        DiscoveryService,
        irpc_mpsc::Receiver<Command>,
        irpc_mpsc::Sender<Event>,
    ) {
        let (cmd_tx, cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local_node);
        (service, cmd_rx, ev_tx)
    }

    /// Deliver a discovery payload through the drain loop as a received
    /// gossip event, the way the mesh would.
    async fn deliver(ev_tx: &irpc_mpsc::Sender<Event>, peer: PublicKey, bytes: Vec<u8>) {
        ev_tx
            .send(Event::Received(GossipMessage {
                content: Bytes::from(bytes),
                scope: DeliveryScope::Neighbors,
                delivered_from: peer,
            }))
            .await
            .expect("send discovery event");
    }

    /// Await the next command from the gossip actor (5s timeout).
    async fn next_command(cmd_rx: &mut irpc_mpsc::Receiver<Command>) -> Command {
        tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for gossip command")
            .expect("channel receive failed")
            .expect("channel closed before command")
    }

    /// A valid Hello from a newly discovered peer produces exactly ONE
    /// connectivity command (`Command::JoinPeers`) and nothing else — no
    /// chat broadcast, no friendship/group/conversation mutation. This is
    /// the Phase-4 "discovery updates connectivity only" invariant.
    #[tokio::test]
    async fn discovery_updates_connectivity_only() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (service, mut cmd_rx, ev_tx) = test_service_with_cmd(local);

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        deliver(&ev_tx, peer, bytes).await;

        // The ONLY command sent to the gossip actor is the connectivity dial.
        let command = next_command(&mut cmd_rx).await;
        let Command::JoinPeers(peers) = command else {
            panic!("expected only JoinPeers connectivity command, got {command:?}");
        };
        let expected: iroh_base::EndpointId = peer.into();
        assert_eq!(peers, vec![expected]);

        // No further commands: discovery never broadcasts chat payloads nor
        // mutates friend/group/conversation state through the gossip actor.
        assert!(
            tokio::time::timeout(Duration::from_millis(80), cmd_rx.recv())
                .await
                .is_err(),
            "discovery must not produce any further gossip commands"
        );

        // Presence is tracked in the discovery registry only, with no
        // friendship or conversation metadata.
        assert_eq!(service.peer_count(), 1);
        let known = service.known_peers();
        assert_eq!(known[0].0, peer);
        assert_eq!(known[0].1.source, PeerSource::Hello);
    }

    /// A `PeerAdvertisement` dials BOTH the advertising sender (seen) and the
    /// advertised peer — both are connectivity candidates. Still no chat
    /// broadcast and no friend/group/conversation side effects.
    #[tokio::test]
    async fn discovery_advertisement_dials_sender_and_advertised() {
        let local = test_key(0xAA);
        let sender_pk = test_key(0xDD);
        let advertised = test_key(0xEE);
        let (service, mut cmd_rx, ev_tx) = test_service_with_cmd(local);

        let bytes =
            postcard::to_stdvec(&DiscoveryMessage::peer_advertisement(sender_pk, advertised))
                .unwrap();
        deliver(&ev_tx, sender_pk, bytes).await;

        // Two dials: the sender (via Seen) then the advertised peer.
        let command = next_command(&mut cmd_rx).await;
        let Command::JoinPeers(peers) = command else {
            panic!("expected JoinPeers command, got {command:?}");
        };
        let sender_endpoint: iroh_base::EndpointId = sender_pk.into();
        let advertised_endpoint: iroh_base::EndpointId = advertised.into();
        assert_eq!(peers, vec![sender_endpoint]);

        let command = next_command(&mut cmd_rx).await;
        let Command::JoinPeers(peers) = command else {
            panic!("expected JoinPeers command, got {command:?}");
        };
        assert_eq!(peers, vec![advertised_endpoint]);

        // Only the sender is registered (presence); the advertised peer is a
        // dial candidate, not a registered peer.
        assert_eq!(service.peer_count(), 1);
        assert_eq!(service.known_peers()[0].0, sender_pk);

        // Nothing further (no chat payload broadcast).
        assert!(
            tokio::time::timeout(Duration::from_millis(80), cmd_rx.recv())
                .await
                .is_err(),
            "discovery advertisement must not produce further gossip commands"
        );
    }

    /// Repeats (Presence refresh, duplicate advertisement) do NOT re-dial an
    /// already-dialed peer — the connectivity loop deduplicates by endpoint.
    #[tokio::test]
    async fn connectivity_loop_deduplicates_peer_dials() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (_service, mut cmd_rx, ev_tx) = test_service_with_cmd(local);

        // First Hello dials the peer.
        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        deliver(&ev_tx, peer, bytes).await;
        let command = next_command(&mut cmd_rx).await;
        assert!(
            matches!(command, Command::JoinPeers(_)),
            "expected JoinPeers command, got {command:?}"
        );

        // A Presence refresh for the same peer must not re-dial.
        let bytes = postcard::to_stdvec(&DiscoveryMessage::presence(peer)).unwrap();
        deliver(&ev_tx, peer, bytes).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(80), cmd_rx.recv())
                .await
                .is_err(),
            "presence refresh must not re-dial an already-dialed peer"
        );
    }

    /// A self-originated discovery message never produces a connectivity
    /// dial (the receive path already filters self messages; the wiring adds
    /// a second guard).
    #[tokio::test]
    async fn connectivity_loop_never_dials_self() {
        let local = test_key(0xAA);
        let (_service, mut cmd_rx, ev_tx) = test_service_with_cmd(local);

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(local)).unwrap();
        deliver(&ev_tx, local, bytes).await;

        assert!(
            tokio::time::timeout(Duration::from_millis(80), cmd_rx.recv())
                .await
                .is_err(),
            "self discovery message must not produce a connectivity dial"
        );
        assert_eq!(_service.peer_count(), 0);
    }

    /// A malformed / non-discovery payload produces no registry update and
    /// no connectivity command (drop at the receive gate).
    #[tokio::test]
    async fn undecodable_payload_produces_no_connectivity() {
        let local = test_key(0xAA);
        let (_service, mut cmd_rx, ev_tx) = test_service_with_cmd(local);

        let peer = test_key(0x42);
        deliver(&ev_tx, peer, b"this is not a discovery message".to_vec()).await;

        assert_eq!(_service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), cmd_rx.recv())
                .await
                .is_err(),
            "undecodable payload must not produce any gossip command"
        );
    }
}
