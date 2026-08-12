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
//! The registry maps `node_id` → last-seen / source-topic metadata, and is
//! the **dedup anchor** (BORU-DISC-17): a node already registered is not
//! re-announced as new. Dedup is keyed by `(node_id, event_id)` — the same
//! peer discovered on two paths (e.g. the internal discovery topic and any
//! legacy/compat path that forwards the same advertisement) is represented
//! once:
//!
//! * **By node identity** — the map key itself. A peer seen on topic A and
//!   again on topic B still occupies a single entry (its `source_topic`
//!   updates to the latest hop).
//! * **By event id** — each message carries a per-node monotonic
//!   [`event_id`](crate::discovery_message::DiscoveryMessage::event_id).
//!   Re-delivering the same event (same node, same id) leaves the registry
//!   untouched ([`UpsertOutcome::Duplicate`]); a new event id from a known
//!   node refreshes `last_seen` ([`UpsertOutcome::Refreshed`]).
//! * **Legacy senders** (no event id on the wire) always refresh — they are
//!   never deduplicated, preserving BORU-DISC-06 behaviour exactly.
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
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
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
    /// The payload duplicated an already-accepted event from the same node
    /// (same `node_id` + same `event_id`, e.g. the same advertisement
    /// delivered over two discovery paths). It was ignored — no registry
    /// change and no [`PeerUpdate`] was emitted (BORU-DISC-17 dedup).
    Duplicate,
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

/// Outcome of [`PeerRegistry::upsert`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// The peer was not registered before — a fresh entry was created.
    New,
    /// The peer was already registered and its metadata was refreshed
    /// (last-seen / source / source-topic). A distinct event id from a
    /// known node updates last-seen.
    Refreshed,
    /// The peer was already registered AND the incoming event id equals the
    /// last accepted event id — the same advertisement delivered twice (e.g.
    /// over two discovery paths). The registry was **not** modified.
    Duplicate,
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
    /// The event id of the most recently accepted message from this peer
    /// (`None` = only legacy, event-id-less messages seen). The second half
    /// of the dedup key: a re-delivered message with the same id is a
    /// duplicate and does not refresh the entry (BORU-DISC-17).
    pub last_event_id: Option<u64>,
}

/// In-process registry of peers seen on the internal discovery topic.
///
/// Maps `node_id` → last-seen / source-topic metadata. This is the dedup
/// anchor: a node that has already been registered is not re-announced as
/// new. Dedup is keyed by `(node_id, event_id)` (BORU-DISC-17) — the same
/// peer discovered on two paths is represented once; a duplicate event id
/// leaves the entry untouched.
#[derive(Debug, Default, Clone)]
pub struct PeerRegistry {
    peers: HashMap<PublicKey, PeerRegistryEntry>,
}

impl PeerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or refresh a peer entry, deduplicating by `(node_id, event_id)`.
    ///
    /// * **New peer** — creates a fresh entry ([`UpsertOutcome::New`]).
    /// * **Known peer, new event id** — refreshes `last_seen`/`source`/
    ///   `source_topic` ([`UpsertOutcome::Refreshed`]).
    /// * **Known peer, same event id** — duplicate delivery; the entry is
    ///   **not** modified ([`UpsertOutcome::Duplicate`]).
    ///
    /// Legacy senders (no event id on the wire, `event_id == None`) always
    /// refresh — they are never deduplicated, preserving BORU-DISC-06
    /// behaviour.
    pub fn upsert(
        &mut self,
        node_id: PublicKey,
        source: PeerSource,
        source_topic: TopicId,
        event_id: Option<u64>,
    ) -> UpsertOutcome {
        if let Some(entry) = self.peers.get_mut(&node_id) {
            // Duplicate event id from an already-known node: the same event
            // re-delivered (e.g. over two discovery paths). Leave the entry
            // untouched.
            if let Some(id) = event_id {
                if entry.last_event_id == Some(id) {
                    return UpsertOutcome::Duplicate;
                }
            }
            entry.last_seen = Instant::now();
            entry.source = source;
            entry.source_topic = source_topic;
            if event_id.is_some() {
                entry.last_event_id = event_id;
            }
            UpsertOutcome::Refreshed
        } else {
            self.peers.insert(
                node_id,
                PeerRegistryEntry {
                    last_seen: Instant::now(),
                    source,
                    source_topic,
                    last_event_id: event_id,
                },
            );
            UpsertOutcome::New
        }
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
/// the announcement throttle, and the per-node event-id counter.
///
/// Cloned into the drain loop so neighbour-up events can re-announce
/// presence. All clones share one [`AnnounceThrottle`] via `Arc`, so
/// join-time and neighbour-up announcements observe the same minimum-interval
/// policy. The event-id counter is shared the same way (BORU-DISC-17): every
/// announcement gets a fresh, monotonically increasing id so receivers can
/// dedup by `(node_id, event_id)`.
#[derive(Clone, Debug)]
struct AnnounceHandle {
    sender: GossipSender,
    local_node: PublicKey,
    throttle: Arc<AnnounceThrottle>,
    next_event_id: Arc<AtomicU64>,
}

impl AnnounceHandle {
    fn new(sender: GossipSender, local_node: PublicKey) -> Self {
        Self {
            sender,
            local_node,
            throttle: Arc::new(AnnounceThrottle::new()),
            next_event_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Allocate the next per-node event id (monotonic, starts at 0).
    fn next_event_id(&self) -> u64 {
        self.next_event_id.fetch_add(1, Ordering::Relaxed)
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
    ///
    /// The event id is allocated ONLY when the announcement passes the
    /// throttle — a suppressed announcement does not consume an id, so the
    /// id space tracks actually-broadcast events (BORU-DISC-17).
    async fn announce<F>(&self, build: F) -> Result<AnnounceOutcome, DiscoveryServiceError>
    where
        F: FnOnce(u64) -> DiscoveryMessage,
    {
        if !self.throttle.try_announce() {
            debug!("discovery: announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let event_id = self.next_event_id();
        self.publish(build(event_id)).await?;
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce this node with a `Hello` carrying a fresh per-node event id.
    async fn announce_hello(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(|event_id| {
            DiscoveryMessage::hello_with_event(self.local_node, event_id)
        })
        .await
    }

    /// Announce this node with a `Presence` heartbeat carrying a fresh
    /// per-node event id.
    async fn announce_presence(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(|event_id| {
            DiscoveryMessage::presence_with_event(self.local_node, event_id)
        })
        .await
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
        let event_id = message.event_id();
        {
            let mut registry = self.registry.lock().expect("peer registry lock poisoned");
            match registry.upsert(node_id, source, self.topic, event_id) {
                UpsertOutcome::New => {
                    info!(
                        node = %node_id.fmt_short(),
                        source = ?source,
                        "discovery: new peer seen",
                    );
                }
                UpsertOutcome::Refreshed => {
                    trace!(
                        node = %node_id.fmt_short(),
                        source = ?source,
                        "discovery: peer refresh",
                    );
                }
                UpsertOutcome::Duplicate => {
                    trace!(
                        node = %node_id.fmt_short(),
                        event = ?event_id,
                        "discovery: duplicate event ignored",
                    );
                    return IncomingOutcome::Duplicate;
                }
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

/// A cloneable handle to join peers into the discovery gossip mesh.
///
/// Background discovery sources that outlive the service handle (e.g. the
/// mDNS event loop in `main.rs`) use this to form a mesh edge on the
/// discovery topic. Joining a peer only updates networking connectivity —
/// it never creates a friendship, a group, or a conversation, and no chat
/// payload is routed through the discovery topic (BORU-DISC-11/12).
#[derive(Clone, Debug)]
pub struct DiscoveryJoiner {
    sender: GossipSender,
}

impl DiscoveryJoiner {
    /// Join one or more peers into the discovery gossip mesh.
    pub async fn join_peers(&self, peers: Vec<PublicKey>) -> Result<(), ApiError> {
        self.sender.join_peers(peers).await
    }
}

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

    /// Return a cloneable joiner handle bound to the discovery gossip
    /// sender.
    ///
    /// Long-lived background discovery sources (e.g. the mDNS event loop in
    /// `main.rs`) hold this to join peers into the discovery mesh without
    /// needing the whole service handle. Joining only forms a gossip mesh
    /// edge — it never creates a friend/group/conversation and never routes
    /// chat payloads (BORU-DISC-12).
    pub fn joiner(&self) -> DiscoveryJoiner {
        DiscoveryJoiner {
            sender: self.announce.sender.clone(),
        }
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

    /// Rewrite the protocol-version byte (index 1 — after the variant tag,
    /// the `DiscoveryHeader` is the first field of every variant) of an
    /// already-encoded discovery message.
    fn with_protocol_version(bytes: Vec<u8>, version: u8) -> Vec<u8> {
        let mut bytes = bytes;
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
        registry.upsert(node, PeerSource::Hello, topic, None);

        assert!(registry.contains(&node));
        assert_eq!(registry.len(), 1);
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Hello);
        assert_eq!(entry.source_topic, topic);
        assert!(registry.last_seen(&node).is_some());
        assert_eq!(entry.last_event_id, None);

        // Refresh with a different source updates the entry, not the count.
        registry.upsert(node, PeerSource::Presence, topic, None);
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
        registry.upsert(stale, PeerSource::Hello, topic, None);
        registry
            .peers
            .get_mut(&stale)
            .unwrap()
            .last_seen = Instant::now() - Duration::from_secs(3600);
        registry.upsert(fresh, PeerSource::Presence, topic, None);

        let removed = registry.prune_older_than(Duration::from_secs(60));
        assert_eq!(removed, vec![stale]);
        assert!(!registry.contains(&stale));
        assert!(registry.contains(&fresh));
    }

    // ── Dedup by node id + event id (BORU-DISC-17) ────────────────────

    /// Same peer advertised on two topics yields ONE registry entry: the
    /// node-identity key dominates, and the entry's source-topic metadata
    /// updates to the latest hop.
    #[test]
    fn registry_same_peer_two_topics_is_one_entry() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x21);
        let topic_a = test_topic();
        let topic_b = crate::discovery_topic::discovery_topic(
            crate::public_room::PublicNetwork::Development,
        );
        assert_ne!(topic_a, topic_b);

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic_a, Some(1)),
            UpsertOutcome::New
        );
        // The same peer arrives on a second discovery path with a new event.
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic_b, Some(2)),
            UpsertOutcome::Refreshed
        );

        // Still exactly ONE entry — a peer discovered on both paths is
        // represented once.
        assert_eq!(registry.len(), 1);
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Presence);
        assert_eq!(entry.source_topic, topic_b);
        assert_eq!(entry.last_event_id, Some(2));
    }

    /// A duplicate event id from the same node is ignored: the entry is
    /// untouched (no last-seen/source/source-topic change).
    #[test]
    fn registry_duplicate_event_id_ignored() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x22);
        let topic = test_topic();

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic, Some(7)),
            UpsertOutcome::New
        );
        let first_seen = registry.get(&node).unwrap().last_seen;

        // Same node, same event id re-delivered (e.g. the same advertisement
        // forwarded over two paths) — must NOT refresh the entry.
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic, Some(7)),
            UpsertOutcome::Duplicate
        );
        assert_eq!(registry.len(), 1);
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Hello, "source must not change");
        assert_eq!(entry.source_topic, topic);
        assert_eq!(
            entry.last_seen, first_seen,
            "last_seen must not change on duplicate"
        );
        assert_eq!(entry.last_event_id, Some(7));
    }

    /// Distinct event ids from the same node update last-seen.
    #[test]
    fn registry_distinct_event_ids_update_last_seen() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x23);
        let topic = test_topic();

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic, Some(1)),
            UpsertOutcome::New
        );
        let first_seen = registry.get(&node).unwrap().last_seen;

        // A new event id (a real refresh/presence) updates last_seen.
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic, Some(2)),
            UpsertOutcome::Refreshed
        );
        let entry = registry.get(&node).unwrap();
        assert_eq!(entry.source, PeerSource::Presence);
        assert!(entry.last_seen > first_seen);
        assert_eq!(entry.last_event_id, Some(2));
    }

    /// Legacy senders (no event id on the wire) always refresh — they are
    /// never deduplicated. Two distinct legacy messages from the same node
    /// produce Refreshed, never Duplicate.
    #[test]
    fn registry_legacy_messages_never_deduplicated() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x24);
        let topic = test_topic();

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic, None),
            UpsertOutcome::New
        );
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic, None),
            UpsertOutcome::Refreshed
        );
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get(&node).unwrap().source, PeerSource::Presence);
        assert_eq!(registry.get(&node).unwrap().last_event_id, None);
    }

    /// Mixing: a legacy message does not erase the tracked event id, and a
    /// later duplicate of a previously-seen event id is still ignored.
    #[test]
    fn registry_event_id_survives_legacy_message() {
        let mut registry = PeerRegistry::new();
        let node = test_key(0x25);
        let topic = test_topic();

        assert_eq!(
            registry.upsert(node, PeerSource::Hello, topic, Some(5)),
            UpsertOutcome::New
        );
        // A legacy (event-id-less) message refreshes but keeps the id.
        assert_eq!(
            registry.upsert(node, PeerSource::Presence, topic, None),
            UpsertOutcome::Refreshed
        );
        assert_eq!(registry.get(&node).unwrap().last_event_id, Some(5));
        // The duplicate of event 5 is still ignored even after the legacy
        // refresh.
        assert_eq!(
            registry.upsert(node, PeerSource::PeerAdvertisement, topic, Some(5)),
            UpsertOutcome::Duplicate
        );
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

    /// Every discovery message variant with an unknown (higher) protocol
    /// version is dropped before its payload is interpreted — none may
    /// register a peer or emit a peer update (BORU-DISC-19).
    #[tokio::test]
    async fn handle_incoming_unknown_version_all_variants_ignored() {
        let local = test_key(0xAA);
        let peer = test_key(0x07);
        let advertised = test_key(0x08);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        for bytes in [
            with_protocol_version(
                postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap(),
                99,
            ),
            with_protocol_version(
                postcard::to_stdvec(&DiscoveryMessage::presence(peer)).unwrap(),
                99,
            ),
            with_protocol_version(
                postcard::to_stdvec(&DiscoveryMessage::peer_advertisement(peer, advertised))
                    .unwrap(),
                99,
            ),
        ] {
            assert_eq!(
                service.handle_incoming(&bytes, peer),
                IncomingOutcome::UnsupportedVersion {
                    found: 99,
                    expected: crate::discovery_topic::BORU_DISCOVERY_PROTOCOL_VERSION,
                },
                "unknown-version payload must be dropped before interpretation"
            );
        }
        assert_eq!(service.peer_count(), 0, "no variant may register a peer");
        assert!(
            updates.try_recv().is_err(),
            "no variant may emit a PeerUpdate"
        );
    }

    /// Protocol version 0 is as unknown as any future version: rejected with
    /// the strict `found != expected` gate (BORU-DISC-19).
    #[tokio::test]
    async fn handle_incoming_version_zero_ignored() {
        let local = test_key(0xAA);
        let peer = test_key(0x09);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let bytes = with_protocol_version(
            postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap(),
            0,
        );
        assert_eq!(
            service.handle_incoming(&bytes, peer),
            IncomingOutcome::UnsupportedVersion {
                found: 0,
                expected: crate::discovery_topic::BORU_DISCOVERY_PROTOCOL_VERSION,
            }
        );
        assert_eq!(service.peer_count(), 0);
        assert!(updates.try_recv().is_err());
    }

    /// An unknown-version payload from an already-known peer must NOT mutate
    /// the registry: `last_seen`/`source` stay exactly as they were and no
    /// `PeerUpdate` is emitted — the gate runs before any state write
    /// (BORU-DISC-19).
    #[tokio::test]
    async fn handle_incoming_unknown_version_does_not_mutate_existing_peer() {
        let local = test_key(0xAA);
        let peer = test_key(0x0B);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        // Register the peer with a current-version hello.
        let hello = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        assert_eq!(service.handle_incoming(&hello, peer), IncomingOutcome::Processed);
        let known = service.known_peers();
        assert_eq!(known.len(), 1);
        let first_seen = known[0].1.last_seen;
        assert_eq!(known[0].1.source, PeerSource::Hello);
        // Consume the Seen update from the registration.
        assert!(updates.try_recv().is_ok());

        // Same peer now speaks an unknown protocol version — dropped before
        // the registry is touched.
        let bogus = with_protocol_version(
            postcard::to_stdvec(&DiscoveryMessage::presence(peer)).unwrap(),
            99,
        );
        assert_eq!(
            service.handle_incoming(&bogus, peer),
            IncomingOutcome::UnsupportedVersion {
                found: 99,
                expected: crate::discovery_topic::BORU_DISCOVERY_PROTOCOL_VERSION,
            }
        );

        let after = service.known_peers();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].1.source, PeerSource::Hello, "source must not refresh");
        assert_eq!(
            after[0].1.last_seen, first_seen,
            "last_seen must not refresh"
        );
        assert!(
            updates.try_recv().is_err(),
            "unknown-version payload must not emit a PeerUpdate"
        );
    }

    /// A truncated (mid-field) discovery payload is ignored without panicking
    /// and without touching registry state (mirrors the hostile-input
    /// malformed-envelope handling for chat).
    #[tokio::test]
    async fn handle_incoming_truncated_payload_ignored_without_panic() {
        let local = test_key(0xAA);
        let peer = test_key(0x0C);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let full = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        let truncated = full[..full.len() / 2].to_vec();
        assert!(!truncated.is_empty());

        let outcome = service.handle_incoming(&truncated, peer);
        assert_eq!(outcome, IncomingOutcome::Undecodable);
        assert_eq!(service.peer_count(), 0);
        assert!(updates.try_recv().is_err());
    }

    /// An out-of-range enum discriminant (postcard varint 128 — the same
    /// hostile input the chat layer rejects) is ignored without panicking:
    /// it can never deserialise into a `DiscoveryMessage`.
    #[tokio::test]
    async fn handle_incoming_unknown_discriminant_ignored_without_panic() {
        let local = test_key(0xAA);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        for bytes in [vec![0x80u8, 0x01], vec![0x03u8], vec![0xFFu8, 0xFFu8]] {
            let outcome = service.handle_incoming(&bytes, test_key(0x0D));
            assert_eq!(outcome, IncomingOutcome::Undecodable);
        }
        assert_eq!(service.peer_count(), 0);
        assert!(updates.try_recv().is_err());
    }

    /// The empty payload is the degenerate malformed input: ignored, never
    /// interpreted, never panics.
    #[tokio::test]
    async fn handle_incoming_empty_payload_ignored_without_panic() {
        let local = test_key(0xAA);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let outcome = service.handle_incoming(b"", test_key(0x0E));
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

    // ── Dedup in the receive path (BORU-DISC-17) ─────────────────────

    /// The same event id from the same node (the same advertisement
    /// delivered twice, e.g. over two discovery paths) is processed once and
    /// ignored on re-delivery: `handle_incoming` returns `Duplicate`, the
    /// registry is unchanged, and NO `PeerUpdate` is emitted.
    #[tokio::test]
    async fn handle_incoming_duplicate_event_id_ignored() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let first = postcard::to_stdvec(&DiscoveryMessage::hello_with_event(peer, 42)).unwrap();
        assert_eq!(service.handle_incoming(&first, peer), IncomingOutcome::Processed);
        assert_eq!(service.peer_count(), 1);

        // A duplicate Seen update was emitted for the first delivery.
        assert_eq!(
            updates.try_recv(),
            Ok(PeerUpdate::Seen {
                node_id: peer,
                source: PeerSource::Hello,
            })
        );

        // Re-deliver the same event (same node, same event id) — ignored.
        let second = postcard::to_stdvec(&DiscoveryMessage::presence_with_event(peer, 42)).unwrap();
        assert_eq!(
            service.handle_incoming(&second, peer),
            IncomingOutcome::Duplicate
        );
        assert_eq!(service.peer_count(), 1);
        // No further update was emitted for the duplicate.
        assert!(
            updates.try_recv().is_err(),
            "duplicate event must not emit a PeerUpdate"
        );
    }

    /// Distinct event ids from the same node update last-seen: the peer stays
    /// one registry entry, but its source/`last_seen` refresh.
    #[tokio::test]
    async fn handle_incoming_distinct_event_ids_refresh() {
        let local = test_key(0xAA);
        let peer = test_key(0xCC);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        let hello = postcard::to_stdvec(&DiscoveryMessage::hello_with_event(peer, 1)).unwrap();
        assert_eq!(service.handle_incoming(&hello, peer), IncomingOutcome::Processed);
        let first_seen = service.known_peers()[0].1.last_seen;

        // A new event id (presence refresh) updates the same single entry.
        let presence = postcard::to_stdvec(&DiscoveryMessage::presence_with_event(peer, 2)).unwrap();
        assert_eq!(
            service.handle_incoming(&presence, peer),
            IncomingOutcome::Processed
        );
        assert_eq!(service.peer_count(), 1);
        assert_eq!(service.known_peers()[0].1.source, PeerSource::Presence);
        assert!(service.known_peers()[0].1.last_seen >= first_seen);

        // A Seen update is emitted for each distinct event.
        assert_eq!(
            updates.try_recv(),
            Ok(PeerUpdate::Seen {
                node_id: peer,
                source: PeerSource::Hello,
            })
        );
        assert_eq!(
            updates.try_recv(),
            Ok(PeerUpdate::Seen {
                node_id: peer,
                source: PeerSource::Presence,
            })
        );
    }

    /// A legacy (no event id) message always refreshes, never dedups — an old
    /// sender's Hello then Presence both update last-seen (BORU-DISC-06
    /// behaviour preserved).
    #[tokio::test]
    async fn handle_incoming_legacy_messages_never_dedup() {
        let local = test_key(0xAA);
        let peer = test_key(0xDD);
        let service = test_service(local);

        let hello = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        assert_eq!(service.handle_incoming(&hello, peer), IncomingOutcome::Processed);
        let presence = postcard::to_stdvec(&DiscoveryMessage::presence(peer)).unwrap();
        assert_eq!(
            service.handle_incoming(&presence, peer),
            IncomingOutcome::Processed,
            "legacy messages must never be treated as duplicates"
        );
        assert_eq!(service.peer_count(), 1);
        assert_eq!(service.known_peers()[0].1.source, PeerSource::Presence);
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
        // The service stamps a per-node event id on its announcements
        // (BORU-DISC-17): the first hello carries event id 0.
        assert_eq!(decoded, DiscoveryMessage::hello_with_event(local, 0));
        assert_eq!(decoded.node_id(), local);
        assert_eq!(decoded.event_id(), Some(0));

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
        assert_eq!(decoded, DiscoveryMessage::presence_with_event(local, 0));
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
        assert_eq!(decoded, DiscoveryMessage::hello_with_event(local, 0));
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
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        // The second announcement carries the next monotonic event id.
        let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, DiscoveryMessage::hello_with_event(local, 1));
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
        assert_eq!(decoded, DiscoveryMessage::hello_with_event(local, 0));
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
