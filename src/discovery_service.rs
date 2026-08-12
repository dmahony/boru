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
//! 1. [`start`](DiscoveryService::start) / [`join`](DiscoveryService::join) /
//!    [`from_subscription`](DiscoveryService::from_subscription)
//!    — join the discovery gossip topic and start the receive drain.
//! 2. [`stop`](DiscoveryService::stop) / [`shutdown`](DiscoveryService::shutdown)
//!    — stop the receive drain and connectivity wiring.
//! 3. [`publish`](DiscoveryService::publish) — broadcast a
//!    [`DiscoveryMessage`] (Hello / Presence / PeerAdvertisement).
//! 4. [`send_control`](DiscoveryService::send_control) — broadcast a
//!    **control-plane** envelope ([`ControlEnvelope`], magic `BC`) on the
//!    discovery topic (BORU-CP-02).
//! 5. [`handle_incoming`](DiscoveryService::handle_incoming) — deserialise +
//!    dispatch one received payload. This is the pure receive-path core: it
//!    takes bytes (no network), so it is directly unit-testable. Control-plane
//!    envelopes are routed to [`ControlEvent`] subscribers; legacy discovery
//!    messages update the peer registry.
//! 6. [`peer_updates`](DiscoveryService::peer_updates) — a live stream of
//!    [`PeerUpdate`]s for callers, backed by the authoritative
//!    [`PeerRegistry`].
//! 7. [`control_events`](DiscoveryService::control_events) — a live stream of
//!    [`ControlEvent`]s (decoded control-plane envelopes), the explicit
//!    event-callback boundary demanded by PDF Task 1.2.
//! 8. [`announce_hello`](DiscoveryService::announce_hello) /
//!    [`announce_presence`](DiscoveryService::announce_presence) — throttled
//!    presence announcements (guarded by [`AnnounceThrottle`]).
//! 9. **Connectivity wiring (Phase 4, BORU-DISC-11)** — every newly
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
use crate::control_plane::message::{
    ControlEnvelope, ControlPlaneDecode, CONTROL_PLANE_MAGIC,
};
use crate::control_plane::privacy::{
    AdvertViolation, ControlPlaneGuard, GuardRejectReason, GuardVerdict, DEFAULT_PRESENCE_TTL,
    EXPIRY_SWEEP_INTERVAL,
};
use crate::diagnostics::{DiagnosticCounters, DIAGNOSTIC_COUNTERS};
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
    /// A previously-seen peer was removed from active presence by the
    /// TTL-based expiry sweep (BORU-CP-03): it was not heard from within the
    /// configured presence TTL. Emitted once per expired peer so callers
    /// (e.g. the Discover sidebar) can drop it from visible presence.
    Expired {
        /// The node that went stale.
        node_id: PublicKey,
    },
}

/// Live control-plane event notifications for callers of
/// [`DiscoveryService::control_events`].
///
/// The service's receive path routes **control-plane** envelopes (the
/// versioned "BC"-magic wire format from BORU-CP-01) to this stream. This
/// is the explicit event-callback boundary demanded by PDF Task 1.2:
/// control-plane messages are delivered to the service's own subscribers —
/// never to chat-message handlers, conversation state, unread counts, or
/// rendering. Unknown message types and malformed frames are dropped at the
/// receive gate (logged, never emitted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// A valid control-plane envelope was received on the discovery topic.
    Received(ControlEnvelope),
}

/// Outcome of [`DiscoveryService::handle_incoming`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingOutcome {
    /// The payload was accepted: the peer registry was updated and a
    /// [`PeerUpdate`] was emitted.
    Processed,
    /// A valid **control-plane** envelope (magic `BC`, BORU-CP-01) was
    /// decoded and routed to [`ControlEvent`] subscribers. The peer registry
    /// is intentionally NOT touched — control-plane traffic is the service
    /// boundary's own event stream (PDF Task 1.2).
    ControlMessage,
    /// A control-plane envelope with an unknown (future) `message_type` was
    /// ignored safely (forward compatibility — fail closed for that
    /// feature).
    UnknownControlType {
        /// The unknown message_type tag byte.
        message_type: u8,
    },
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
    /// A control-plane frame was dropped because the authenticated gossip
    /// delivery source differs from the envelope's claimed
    /// `sender_node_id` — a spoofing attempt (BORU-CP-03 attribution gate).
    SpoofedSender,
    /// A control-plane frame was dropped because the authenticated sender
    /// exceeded the per-sender frame rate limit (BORU-CP-03). Bounded
    /// logging: at most one warning per window per sender.
    RateLimited,
    /// A control-plane frame was dropped because it violates the
    /// minimal-advertisement whitelist / bounds (BORU-CP-03).
    AdvertViolation(AdvertViolation),
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
    /// Broadcast channel of control-plane events for callers (BORU-CP-02).
    /// Valid `ControlEnvelope`s decoded from the discovery topic are
    /// delivered here — the explicit event-callback boundary that keeps
    /// control-plane messages out of chat-message handlers.
    control_events_tx: broadcast::Sender<ControlEvent>,
    /// The BORU-CP-03 privacy/abuse guard: per-sender rate limiting,
    /// `(sender_node_id, sequence)` dedup, minimal-advertisement policy,
    /// sender attribution, and the TTL-expiring control-plane presence
    /// store. Shared between the receive path and the presence-expiry sweep.
    guard: Arc<Mutex<ControlPlaneGuard>>,
    /// Atomic discovery counters (BORU-DISC-20). Cloned from the global
    /// [`DIAGNOSTIC_COUNTERS`] by default so the frontend/MCP can read the
    /// same values; tests inject an isolated instance.
    counters: DiagnosticCounters,
}

impl ReceiveCore {
    /// Deserialise + dispatch one received discovery payload.
    ///
    /// The receive-path gate order is deliberately: deserialise → protocol
    /// version check → self-filter → registry update. Unknown versions and
    /// undecodable payloads are dropped (and logged), never interpreted.
    fn handle_incoming(&self, content: &[u8], delivered_from: PublicKey) -> IncomingOutcome {
        // BORU-CP-02: control-plane envelopes (magic "BC") are routed to
        // the control-plane event stream — never to the peer registry and
        // never to chat handling. The magic prefix is unambiguous: the
        // legacy DiscoveryMessage wire format starts with a postcard enum
        // tag (0..=2), so `0x42 0x43` can never be a discovery message.
        if content.starts_with(&CONTROL_PLANE_MAGIC) {
            return self.handle_control_incoming(content, delivered_from);
        }

        let message = match postcard::from_bytes::<DiscoveryMessage>(content) {
            Ok(message) => message,
            Err(error) => {
                self.counters.record_malformed_discovery_packet();
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
                self.counters.record_unsupported_version_packet();
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
                    self.counters.record_discovery_peer_seen();
                    info!(
                        node = %node_id.fmt_short(),
                        source = ?source,
                        topic = %self.topic,
                        "discovery: new peer seen",
                    );
                }
                UpsertOutcome::Refreshed => {
                    trace!(
                        node = %node_id.fmt_short(),
                        source = ?source,
                        topic = %self.topic,
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
            // BORU-DISC-20: log peer advertisements at debug level with the
            // sender node id + the advertised peer + the source topic. Never
            // log message contents or private chat data — discovery payloads
            // carry only node ids, never chat payloads.
            debug!(
                node = %node_id.fmt_short(),
                advertised = %advertised.fmt_short(),
                source_topic = %self.topic,
                "discovery: peer advertisement received",
            );
            if advertised != self.local_node && advertised != node_id {
                let _ = self.peer_updates_tx.send(PeerUpdate::Advertised {
                    node_id,
                    advertised,
                });
            }
        }

        IncomingOutcome::Processed
    }

    /// Deserialise + dispatch one received control-plane envelope (magic
    /// `BC`, BORU-CP-01 wire format).
    ///
    /// The control-plane gate order is: decode → protocol-version check →
    /// self-filter → dedup by `(sender_node_id, sequence)` → emit
    /// [`ControlEvent::Received`]. The peer registry is deliberately NOT
    /// touched: control-plane traffic is the service boundary's own event
    /// stream, never conversation/peer-registry state (PDF Task 1.2). A
    /// malformed, unknown-type, or unsupported-version frame is dropped
    /// (logged, counted) without panicking or affecting chat handling.
    fn handle_control_incoming(&self, content: &[u8], delivered_from: PublicKey) -> IncomingOutcome {
        let envelope = match ControlEnvelope::decode(content) {
            Ok(ControlPlaneDecode::Message(envelope)) => envelope,
            Ok(ControlPlaneDecode::UnknownType { message_type, .. }) => {
                debug!(
                    delivered_from = %delivered_from.fmt_short(),
                    message_type,
                    "discovery: unknown control message type ignored",
                );
                return IncomingOutcome::UnknownControlType { message_type };
            }
            Ok(ControlPlaneDecode::UnsupportedVersion { found, expected }) => {
                self.counters.record_unsupported_version_packet();
                warn!(
                    delivered_from = %delivered_from.fmt_short(),
                    found,
                    expected,
                    "discovery: unsupported control-plane protocol version dropped",
                );
                return IncomingOutcome::UnsupportedVersion { found, expected };
            }
            Err(error) => {
                self.counters.record_malformed_discovery_packet();
                debug!(
                    delivered_from = %delivered_from.fmt_short(),
                    error = %error,
                    "discovery: malformed control-plane envelope dropped",
                );
                return IncomingOutcome::Undecodable;
            }
        };

        if envelope.sender_node_id == self.local_node {
            trace!(node = %envelope.sender_node_id.fmt_short(), "discovery: ignoring self control message");
            return IncomingOutcome::SelfMessage;
        }

        // BORU-CP-03 privacy/abuse gates: rate limit (by the authenticated
        // delivery source) → attribution → minimal-advertisement policy →
        // dedup by (sender_node_id, sequence) → presence state update.
        let verdict = {
            let mut guard = self.guard.lock().expect("control-plane guard lock poisoned");
            guard.admit(&envelope, delivered_from, Instant::now())
        };
        match verdict {
            GuardVerdict::Accept => {
                info!(
                    sender = %envelope.sender_node_id.fmt_short(),
                    message_type = ?envelope.message_type,
                    sequence = envelope.sequence,
                    "discovery: control-plane message received",
                );
                let _ = self
                    .control_events_tx
                    .send(ControlEvent::Received(envelope));
                IncomingOutcome::ControlMessage
            }
            GuardVerdict::Reject(reason) => {
                // Log the state transition, never the message contents.
                // Each rejection is bounded by the rate limiter, so a
                // malicious peer cannot cause unbounded log spam.
                match reason {
                    GuardRejectReason::SpoofedSender => {
                        self.counters.record_malformed_discovery_packet();
                        warn!(
                            claimed = %envelope.sender_node_id.fmt_short(),
                            delivered_from = %delivered_from.fmt_short(),
                            "discovery: control envelope sender mismatch dropped",
                        );
                        IncomingOutcome::SpoofedSender
                    }
                    GuardRejectReason::RateLimited => {
                        warn!(
                            sender = %delivered_from.fmt_short(),
                            "discovery: control-plane rate limit exceeded",
                        );
                        IncomingOutcome::RateLimited
                    }
                    GuardRejectReason::Duplicate => {
                        trace!(
                            sender = %envelope.sender_node_id.fmt_short(),
                            sequence = envelope.sequence,
                            "discovery: duplicate control envelope ignored",
                        );
                        IncomingOutcome::Duplicate
                    }
                    GuardRejectReason::AdvertViolation(violation) => {
                        debug!(
                            sender = %envelope.sender_node_id.fmt_short(),
                            violation = ?violation,
                            "discovery: control advertisement rejected by minimal-content policy",
                        );
                        IncomingOutcome::AdvertViolation(violation)
                    }
                }
            }
        }
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
    /// Join handle of the presence-expiry sweep task (BORU-CP-03): removes
    /// peers not heard from within the configured presence TTL.
    expiry_task: JoinHandle<()>,
    /// Shared presence-expiry configuration (TTL + sweep interval) so the
    /// builder can tune it after construction and the sweep observes it.
    expiry_config: Arc<Mutex<PresenceExpiryConfig>>,
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

    /// Start the discovery service: join the fixed internal discovery topic
    /// and begin draining (PDF Task 1.2 `start()`).
    ///
    /// This is the explicit lifecycle entry point for the hidden discovery
    /// service boundary. It is exactly the startup call made by
    /// `examples/iced_chat/main.rs` — every Boru node joins the versioned
    /// internal discovery gossip topic at startup as networking
    /// infrastructure, without creating any conversation/UI state.
    ///
    /// Equivalent to [`join`](Self::join); the `start` name is provided so
    /// the PDF's lifecycle API (`start()` / [`stop()`](Self::stop) /
    /// [`send_control`](Self::send_control) / [`control_events`](Self::control_events))
    /// is explicit on the service.
    pub async fn start(
        gossip: &crate::net::Gossip,
        topic: TopicId,
        bootstrap: Vec<PublicKey>,
        local_node: PublicKey,
    ) -> Result<Self, ApiError> {
        Self::join(gossip, topic, bootstrap, local_node).await
    }

    /// Stop the discovery service: cancel the drain and connectivity tasks
    /// and await them (PDF Task 1.2 `stop()`).
    ///
    /// Equivalent to [`shutdown`](Self::shutdown); the `stop` name is
    /// provided so the PDF's lifecycle API is explicit on the service.
    pub async fn stop(self) {
        self.shutdown().await
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
        Self::from_subscription_with_counters(
            topic,
            sender,
            receiver,
            local_node,
            DIAGNOSTIC_COUNTERS.clone(),
        )
    }

    /// Build a running service with an explicit counter set.
    ///
    /// Production callers use [`from_subscription`](Self::from_subscription),
    /// which shares the global [`DIAGNOSTIC_COUNTERS`]; tests inject an
    /// isolated [`DiagnosticCounters`] so counter assertions never race with
    /// other tests or live app traffic.
    fn from_subscription_with_counters(
        topic: TopicId,
        sender: GossipSender,
        receiver: GossipReceiver,
        local_node: PublicKey,
        counters: DiagnosticCounters,
    ) -> Self {
        let registry = Arc::new(Mutex::new(PeerRegistry::new()));
        let (peer_updates_tx, _) = broadcast::channel(PEER_UPDATES_CAPACITY);
        let (control_events_tx, _) = broadcast::channel(PEER_UPDATES_CAPACITY);
        let guard = Arc::new(Mutex::new(ControlPlaneGuard::new()));
        let core = ReceiveCore {
            local_node,
            topic,
            registry,
            peer_updates_tx,
            control_events_tx,
            guard,
            counters,
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
        // BORU-CP-03: presence-expiry sweep — peers not heard from within
        // the configured presence TTL disappear from active presence
        // (legacy registry + control-plane presence store). State
        // transitions only, never message contents.
        let expiry_config = Arc::new(Mutex::new(PresenceExpiryConfig {
            ttl: DEFAULT_PRESENCE_TTL,
            sweep_interval: EXPIRY_SWEEP_INTERVAL,
        }));
        let expiry_cancel = cancel.clone();
        let expiry_task = tokio::spawn(presence_expiry_loop(
            expiry_config.clone(),
            core.registry.clone(),
            core.guard.clone(),
            core.peer_updates_tx.clone(),
            expiry_cancel,
        ));
        info!(topic = %topic, "discovery service joined");
        Self {
            topic,
            announce,
            core,
            cancel,
            task,
            connectivity_task,
            expiry_task,
            expiry_config,
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

    /// Override the presence TTL (BORU-CP-03).
    ///
    /// Peers not heard from within `ttl` are removed from active presence
    /// (both the legacy peer registry and the control-plane presence store)
    /// by the expiry sweep. Defaults to [`DEFAULT_PRESENCE_TTL`]. Tests use
    /// short TTLs to exercise expiry without sleeping.
    pub fn with_presence_ttl(self, ttl: Duration) -> Self {
        {
            let mut guard = self.core.guard.lock().expect("control-plane guard lock poisoned");
            guard.set_default_presence_ttl(ttl);
        }
        self.expiry_config.lock().expect("expiry config lock poisoned").ttl = ttl;
        self
    }

    /// Override the presence-expiry sweep interval (BORU-CP-03).
    ///
    /// Defaults to [`EXPIRY_SWEEP_INTERVAL`]. Tests use short intervals to
    /// exercise the sweep without sleeping.
    pub fn with_presence_sweep_interval(self, interval: Duration) -> Self {
        self.expiry_config
            .lock()
            .expect("expiry config lock poisoned")
            .sweep_interval = interval;
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

    /// Subscribe to live **control-plane** events (BORU-CP-02).
    ///
    /// The returned receiver observes [`ControlEvent::Received`] for every
    /// valid control-plane envelope decoded from the discovery topic (magic
    /// `BC`, BORU-CP-01 wire format). This is the explicit event-callback
    /// boundary demanded by PDF Task 1.2: control-plane messages are
    /// delivered to the service's own subscribers — never to chat-message
    /// handlers, conversation state, unread counts, or rendering. Unknown
    /// message types, unsupported protocol versions, and malformed frames
    /// are dropped at the receive gate (logged, never emitted).
    pub fn control_events(&self) -> broadcast::Receiver<ControlEvent> {
        self.core.control_events_tx.subscribe()
    }

    /// Send a control-plane envelope on the discovery topic (BORU-CP-02).
    ///
    /// Serialises `envelope` with the BORU-CP-01 wire format (magic `BC`)
    /// and broadcasts it through the gossip sender. This is the explicit
    /// control-plane outbound path — distinct from the legacy
    /// [`publish`](Self::publish) ([`DiscoveryMessage`]) path, and never a
    /// chat message. The event id / sequence is caller-supplied; receivers
    /// deduplicate by `(sender_node_id, sequence)`.
    pub async fn send_control(
        &self,
        envelope: ControlEnvelope,
    ) -> Result<(), DiscoveryServiceError> {
        let bytes = envelope.encode();
        self.announce
            .sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))
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

    /// Number of peers currently in the control-plane presence store
    /// (BORU-CP-03 active presence hints).
    pub fn control_presence_count(&self) -> usize {
        let guard = self.core.guard.lock().expect("control-plane guard lock poisoned");
        guard.presence_count()
    }

    /// Snapshot of the control-plane presence store (BORU-CP-03).
    ///
    /// Each entry is the metadata-only presence hint recorded from the
    /// peer's control-plane advertisements. This is a hint cache — it grants
    /// no authorisation and is never consulted by friendship/trust checks.
    pub fn control_presence_peers(&self) -> Vec<(PublicKey, crate::control_plane::privacy::PeerControlState)> {
        let guard = self.core.guard.lock().expect("control-plane guard lock poisoned");
        guard
            .presence()
            .peers()
            .map(|(node_id, state)| (*node_id, state.clone()))
            .collect()
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

    /// Shut down the service: cancel the drain, connectivity, and expiry
    /// tasks and await them.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task.await;
        let _ = self.connectivity_task.await;
        let _ = self.expiry_task.await;
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
                    Ok(PeerUpdate::Expired { .. }) => {
                        // The peer went stale (BORU-CP-03 TTL expiry). No
                        // dial action: it was already dialed when first
                        // seen, and expiry does not revoke connectivity —
                        // it only removes it from active presence.
                        trace!("discovery: expired peer ignored by connectivity loop");
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
// Presence expiry (BORU-CP-03)
// ---------------------------------------------------------------------------

/// Runtime-tunable presence-expiry configuration shared between the
/// [`DiscoveryService`] builders and the sweep task.
#[derive(Debug, Clone, Copy)]
struct PresenceExpiryConfig {
    /// Peers not heard from within this window are removed from active
    /// presence.
    ttl: Duration,
    /// How often the sweep runs.
    sweep_interval: Duration,
}

/// Background task that removes stale peers from active presence
/// (BORU-CP-03 TTL expiry).
///
/// Every `sweep_interval` it:
///
/// 1. Prunes the legacy discovery [`PeerRegistry`] of peers not heard from
///    within the configured TTL and emits [`PeerUpdate::Expired`] for each
///    (so the Discover sidebar can drop them from visible presence).
/// 2. Expires stale entries in the control-plane presence store (the
///    BORU-CP-03 hint cache).
///
/// Logs state transitions only, never message contents. The sweep interval
/// is re-read from the shared config before every sleep, so builder tuning
/// (e.g. short intervals in tests) takes effect immediately.
async fn presence_expiry_loop(
    config: Arc<Mutex<PresenceExpiryConfig>>,
    registry: Arc<Mutex<PeerRegistry>>,
    guard: Arc<Mutex<ControlPlaneGuard>>,
    peer_updates_tx: broadcast::Sender<PeerUpdate>,
    cancel: CancellationToken,
) {
    loop {
        // Read the current sweep interval each cycle so the builders can
        // tune it after construction (tests use short intervals).
        let sweep = config
            .lock()
            .expect("expiry config lock poisoned")
            .sweep_interval;

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery presence expiry loop cancelled");
                break;
            }
            _ = tokio::time::sleep(sweep) => {
                let ttl = config.lock().expect("expiry config lock poisoned").ttl;
                let now = Instant::now();

                // 1. Legacy discovery registry.
                let expired_registry: Vec<PublicKey> = {
                    let mut reg = registry.lock().expect("peer registry lock poisoned");
                    reg.prune_older_than(ttl)
                };
                for node in &expired_registry {
                    info!(
                        node = %node.fmt_short(),
                        ttl_secs = ttl.as_secs(),
                        "discovery: peer expired from active presence (TTL)",
                    );
                    let _ = peer_updates_tx.send(PeerUpdate::Expired { node_id: *node });
                }

                // 2. Control-plane presence store.
                let expired_control: Vec<PublicKey> = {
                    let mut g = guard.lock().expect("control-plane guard lock poisoned");
                    g.expire_stale(now)
                };
                for node in &expired_control {
                    info!(
                        node = %node.fmt_short(),
                        ttl_secs = ttl.as_secs(),
                        "control: presence expired from active presence (TTL)",
                    );
                }
            }
        }
    }
    debug!("discovery presence expiry loop exited");
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

    /// Build a running service with an ISOLATED counter set (BORU-DISC-20).
    ///
    /// Counter assertions read this instance directly, so they never race
    /// with other tests or live app traffic on the global
    /// [`DIAGNOSTIC_COUNTERS`].
    fn test_service_with_counters(
        local_node: PublicKey,
        counters: DiagnosticCounters,
    ) -> DiscoveryService {
        let (cmd_tx, _cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        DiscoveryService::from_subscription_with_counters(
            test_topic(),
            sender,
            receiver,
            local_node,
            counters,
        )
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

    // ── Discovery counters (BORU-DISC-20) ────────────────────────────

    /// A fresh peer registration increments the discovery-peers-seen counter
    /// and nothing else.
    #[tokio::test]
    async fn counters_new_peer_increments_peers_seen() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let counters = DiagnosticCounters::new();
        let service = test_service_with_counters(local, counters.clone());

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        assert_eq!(service.handle_incoming(&bytes, peer), IncomingOutcome::Processed);

        let snap = counters.snapshot();
        assert_eq!(snap.discovery_peers_seen, 1);
        assert_eq!(snap.malformed_discovery_packets, 0);
        assert_eq!(snap.unsupported_version_packets, 0);
        assert_eq!(snap.direct_topics_joined, 0);
        assert_eq!(snap.group_topics_joined, 0);
    }

    /// A malformed (undecodable) payload increments the malformed-packet
    /// counter but never the peers-seen counter (the peer is not registered).
    #[tokio::test]
    async fn counters_malformed_increments_malformed_only() {
        let local = test_key(0xAA);
        let counters = DiagnosticCounters::new();
        let service = test_service_with_counters(local, counters.clone());

        let outcome = service.handle_incoming(b"this is not a discovery message", test_key(0x42));
        assert_eq!(outcome, IncomingOutcome::Undecodable);

        let snap = counters.snapshot();
        assert_eq!(snap.malformed_discovery_packets, 1);
        assert_eq!(snap.discovery_peers_seen, 0);
        assert_eq!(snap.unsupported_version_packets, 0);
    }

    /// An unsupported-version payload increments the unsupported-version
    /// counter (the BORU-DISC-19 gate is observable) but is NOT counted as a
    /// malformed packet and never registers a peer.
    #[tokio::test]
    async fn counters_unsupported_version_increments_unsupported_only() {
        let local = test_key(0xAA);
        let peer = test_key(0x07);
        let counters = DiagnosticCounters::new();
        let service = test_service_with_counters(local, counters.clone());

        let bytes = hello_with_version(0x07, 99);
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(
            outcome,
            IncomingOutcome::UnsupportedVersion {
                found: 99,
                expected: crate::discovery_topic::BORU_DISCOVERY_PROTOCOL_VERSION,
            }
        );

        let snap = counters.snapshot();
        assert_eq!(snap.unsupported_version_packets, 1);
        assert_eq!(snap.discovery_peers_seen, 0);
        assert_eq!(snap.malformed_discovery_packets, 0);
    }

    /// A duplicate event id (same node, same event) is ignored and does NOT
    /// bump the peers-seen counter — dedup keeps the counter truthful.
    #[tokio::test]
    async fn counters_duplicate_does_not_increment_peers_seen() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let counters = DiagnosticCounters::new();
        let service = test_service_with_counters(local, counters.clone());

        let first = postcard::to_stdvec(&DiscoveryMessage::hello_with_event(peer, 42)).unwrap();
        assert_eq!(service.handle_incoming(&first, peer), IncomingOutcome::Processed);
        assert_eq!(counters.discovery_peers_seen(), 1);

        let second = postcard::to_stdvec(&DiscoveryMessage::presence_with_event(peer, 42)).unwrap();
        assert_eq!(service.handle_incoming(&second, peer), IncomingOutcome::Duplicate);
        assert_eq!(counters.discovery_peers_seen(), 1);
        assert_eq!(counters.malformed_discovery_packets(), 0);
        assert_eq!(counters.unsupported_version_packets(), 0);
    }

    /// A Presence refresh from an already-registered peer does NOT bump the
    /// peers-seen counter (only fresh registrations count as "seen").
    #[tokio::test]
    async fn counters_refresh_does_not_increment_peers_seen() {
        let local = test_key(0xAA);
        let peer = test_key(0xCC);
        let counters = DiagnosticCounters::new();
        let service = test_service_with_counters(local, counters.clone());

        let hello = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        assert_eq!(service.handle_incoming(&hello, peer), IncomingOutcome::Processed);
        assert_eq!(counters.discovery_peers_seen(), 1);

        let presence = postcard::to_stdvec(&DiscoveryMessage::presence(peer)).unwrap();
        assert_eq!(
            service.handle_incoming(&presence, peer),
            IncomingOutcome::Processed
        );
        assert_eq!(counters.discovery_peers_seen(), 1, "refresh is not a new peer");
    }

    /// A self-originated message never bumps any counter.
    #[tokio::test]
    async fn counters_self_message_increments_nothing() {
        let local = test_key(0xAA);
        let counters = DiagnosticCounters::new();
        let service = test_service_with_counters(local, counters.clone());

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(local)).unwrap();
        assert_eq!(service.handle_incoming(&bytes, local), IncomingOutcome::SelfMessage);

        let snap = counters.snapshot();
        assert_eq!(snap.discovery_peers_seen, 0);
        assert_eq!(snap.malformed_discovery_packets, 0);
        assert_eq!(snap.unsupported_version_packets, 0);
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

    // ── control-plane boundary (BORU-CP-02) ─────────────────────────

    /// Encode a `Hello` control-plane envelope for `sender`.
    fn control_hello(sender: PublicKey, sequence: u64) -> Vec<u8> {
        ControlEnvelope::hello(sender, sequence, 1_700_000_000, 1).encode()
    }

    /// A valid control-plane envelope is routed to [`ControlEvent`]
    /// subscribers and does NOT touch the peer registry (the boundary: the
    /// service's own event stream, never conversation/registry state).
    #[tokio::test]
    async fn handle_incoming_control_envelope_emits_control_event() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let bytes = control_hello(peer, 7);
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        // The control event was emitted with the decoded envelope.
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for control event")
            .expect("control event channel closed");
        match event {
            ControlEvent::Received(envelope) => {
                assert_eq!(envelope.sender_node_id, peer);
                assert_eq!(envelope.sequence, 7);
                assert_eq!(
                    envelope.message_type,
                    crate::control_plane::message::ControlMessageType::Hello
                );
            }
        }

        // The peer registry is NOT touched by control-plane traffic.
        assert_eq!(service.peer_count(), 0, "control plane must not register peers");
    }

    /// The same control-plane envelope (same sender + same sequence)
    /// re-delivered is deduplicated: one event, then a `Duplicate` outcome
    /// with no second event.
    #[tokio::test]
    async fn handle_incoming_control_duplicate_sequence_ignored() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let bytes = control_hello(peer, 7);
        assert_eq!(service.handle_incoming(&bytes, peer), IncomingOutcome::ControlMessage);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .is_ok(),
            "first control envelope must emit an event"
        );

        // Same (sender, sequence) re-delivered — ignored.
        assert_eq!(
            service.handle_incoming(&bytes, peer),
            IncomingOutcome::Duplicate,
            "duplicate control envelope must be deduplicated"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "duplicate control envelope must not emit a second event"
        );
    }

    /// A control-plane envelope from this node is ignored (self-filter).
    #[tokio::test]
    async fn handle_incoming_control_self_message_ignored() {
        let local = test_key(0xAA);
        let service = test_service(local);
        let mut events = service.control_events();

        let bytes = control_hello(local, 1);
        assert_eq!(service.handle_incoming(&bytes, local), IncomingOutcome::SelfMessage);
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "self control envelope must not emit an event"
        );
    }

    /// A control-plane envelope with an unknown (future) message_type is
    /// ignored safely (forward compatibility — fail closed for that
    /// feature).
    #[tokio::test]
    async fn handle_incoming_control_unknown_type_ignored() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let mut bytes = control_hello(peer, 1);
        bytes[3] = 0x7F; // rewrite the message_type byte to an unknown tag
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(outcome, IncomingOutcome::UnknownControlType { message_type: 0x7F });
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "unknown-type control envelope must not emit an event"
        );
    }

    /// A control-plane envelope speaking an unsupported protocol version is
    /// dropped (fail closed) without panicking.
    #[tokio::test]
    async fn handle_incoming_control_unsupported_version_fails_closed() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let mut bytes = control_hello(peer, 1);
        bytes[2] = 99; // rewrite the protocol-version byte
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(
            outcome,
            IncomingOutcome::UnsupportedVersion {
                found: 99,
                expected: crate::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION,
            }
        );
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "unsupported-version control envelope must not emit an event"
        );
    }

    /// A malformed control-plane frame (magic prefix + current version byte
    /// but garbage after it) is dropped without panicking or emitting an
    /// event. (A frame whose version byte is wrong hits the version gate
    /// first and is reported as `UnsupportedVersion` — the malformed branch
    /// is only reachable with a supported version.)
    #[tokio::test]
    async fn handle_incoming_control_malformed_dropped() {
        let local = test_key(0xAA);
        let service = test_service(local);
        let mut events = service.control_events();

        // Magic "BC" + supported version byte + garbage header/body.
        let mut bytes = CONTROL_PLANE_MAGIC.to_vec();
        bytes.push(crate::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION);
        bytes.extend_from_slice(b"garbage that is not a valid envelope");
        let outcome = service.handle_incoming(&bytes, test_key(0x42));
        assert_eq!(outcome, IncomingOutcome::Undecodable);
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "malformed control frame must not emit an event"
        );
    }

    /// `send_control` serialises a control-plane envelope (magic `BC`) and
    /// broadcasts it through the gossip sender — never a chat message.
    #[tokio::test]
    async fn send_control_serializes_and_broadcasts() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        let envelope = ControlEnvelope::hello(local, 42, 1_700_000_000, 1);
        service.send_control(envelope.clone()).await.unwrap();

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for broadcast command")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        // The wire bytes are a control-plane envelope (magic "BC"), not a
        // chat message and not a legacy DiscoveryMessage.
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(decoded) => assert_eq!(decoded, envelope),
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    /// The drain loop routes a received control-plane envelope to
    /// [`ControlEvent`] subscribers (the end-to-end service boundary: the
    /// receive task, not just the pure core).
    #[tokio::test]
    async fn drain_loop_forwards_control_events() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (cmd_tx, _cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);
        let mut events = service.control_events();

        let bytes = control_hello(peer, 3);
        ev_tx
            .send(Event::Received(GossipMessage {
                content: Bytes::from(bytes),
                scope: DeliveryScope::Neighbors,
                delivered_from: peer,
            }))
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for control event")
            .expect("control event channel closed");
        match event {
            ControlEvent::Received(envelope) => {
                assert_eq!(envelope.sender_node_id, peer);
                assert_eq!(envelope.sequence, 3);
            }
        }
        assert_eq!(service.peer_count(), 0, "control plane must not register peers");
    }

    // ── control-plane privacy/abuse guards (BORU-CP-03) ──────────────

    /// A control envelope whose claimed sender differs from the
    /// authenticated gossip delivery source is dropped as a spoof — no
    /// event, no presence entry.
    #[tokio::test]
    async fn handle_incoming_control_spoofed_sender_rejected() {
        let local = test_key(0xAA);
        let claimed = test_key(0xBB);
        let actual = test_key(0xCC); // authenticated delivery source
        let service = test_service(local);
        let mut events = service.control_events();

        let bytes = control_hello(claimed, 7);
        let outcome = service.handle_incoming(&bytes, actual);
        assert_eq!(
            outcome,
            IncomingOutcome::SpoofedSender,
            "a control envelope claiming a different identity must be dropped"
        );
        assert_eq!(service.peer_count(), 0);
        assert_eq!(service.control_presence_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "spoofed control envelope must not emit an event"
        );
    }

    /// A sender that exceeds the per-sender frame rate limit is dropped
    /// (bounded log spam + presence churn).
    #[tokio::test]
    async fn handle_incoming_control_rate_limited_sender_rejected() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        // Fill the default 60-frame / 10s window.
        let mut last_outcome = IncomingOutcome::SelfMessage;
        for seq in 0..60 {
            last_outcome = service.handle_incoming(&control_hello(peer, seq), peer);
        }
        assert_eq!(last_outcome, IncomingOutcome::ControlMessage);

        let outcome = service.handle_incoming(&control_hello(peer, 60), peer);
        assert_eq!(
            outcome,
            IncomingOutcome::RateLimited,
            "frames beyond the rate limit must be dropped"
        );
        // The peer is still present (accepted frames before the limit) but
        // no further frames get through.
        assert!(service.control_presence_count() <= 1);
    }

    /// A control advertisement that violates the minimal-content whitelist
    /// is dropped with no presence entry.
    #[tokio::test]
    async fn handle_incoming_control_advert_violation_rejected() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        // A PRESENCE envelope advertising an oversized TTL.
        let envelope = ControlEnvelope::presence(peer, 1, 1_700_000_000, Some(u32::MAX));
        let outcome = service.handle_incoming(&envelope.encode(), peer);
        assert!(
            matches!(outcome, IncomingOutcome::AdvertViolation(_)),
            "oversized presence TTL must be rejected by the minimal-content policy"
        );
        assert_eq!(service.control_presence_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "violating control envelope must not emit an event"
        );
    }

    /// Accepted control envelopes populate the TTL-expiring control-plane
    /// presence store (metadata-only hints).
    #[tokio::test]
    async fn handle_incoming_control_records_presence_hints() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        assert_eq!(
            service.handle_incoming(&control_hello(peer, 1), peer),
            IncomingOutcome::ControlMessage
        );
        assert_eq!(service.control_presence_count(), 1);
        let (node, state) = service.control_presence_peers().pop().unwrap();
        assert_eq!(node, peer);
        assert_eq!(
            state.protocol_version,
            crate::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION
        );
        assert_eq!(state.app_protocol_version, Some(1));

        // The legacy registry is untouched (control plane never registers
        // peers there).
        assert_eq!(service.peer_count(), 0);
    }

    /// The TTL-based expiry sweep removes stale peers from active presence:
    /// both the legacy registry (with a `PeerUpdate::Expired` event) and the
    /// control-plane presence store.
    #[tokio::test]
    async fn expiry_sweep_removes_stale_peers_from_active_presence() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local)
            .with_presence_ttl(Duration::from_millis(80))
            .with_presence_sweep_interval(Duration::from_millis(20));
        let mut updates = service.peer_updates();

        // A legacy discovery hello registers the peer in the registry.
        let legacy = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        assert_eq!(
            service.handle_incoming(&legacy, peer),
            IncomingOutcome::Processed
        );
        assert_eq!(service.peer_count(), 1);

        // A control-plane hello records presence in the control store.
        assert_eq!(
            service.handle_incoming(&control_hello(peer, 1), peer),
            IncomingOutcome::ControlMessage
        );
        assert_eq!(service.control_presence_count(), 1);

        // Wait past the TTL so the sweep expires the peer everywhere.
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert_eq!(
            service.peer_count(),
            0,
            "stale peer must disappear from active presence (registry)"
        );
        assert_eq!(
            service.control_presence_count(),
            0,
            "stale peer must disappear from active presence (control store)"
        );

        // The legacy-registry expiry emitted a PeerUpdate::Expired event.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_expired = false;
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), updates.recv()).await {
                Ok(Ok(PeerUpdate::Expired { node_id })) if node_id == peer => {
                    saw_expired = true;
                    break;
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) => break,
                Err(_) => {}
            }
        }
        assert!(saw_expired, "expiry sweep must emit PeerUpdate::Expired");
    }

    /// A refresh within the TTL keeps the peer in active presence (no
    /// spurious expiry).
    #[tokio::test]
    async fn expiry_sweep_keeps_refreshed_peers() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local)
            .with_presence_ttl(Duration::from_millis(80))
            .with_presence_sweep_interval(Duration::from_millis(20));

        let legacy = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        assert_eq!(
            service.handle_incoming(&legacy, peer),
            IncomingOutcome::Processed
        );
        assert_eq!(service.control_presence_count(), 0);
        assert_eq!(
            service.handle_incoming(&control_hello(peer, 1), peer),
            IncomingOutcome::ControlMessage
        );

        // Keep refreshing within the TTL.
        for seq in 2..6u64 {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert_eq!(
                service.handle_incoming(&control_hello(peer, seq), peer),
                IncomingOutcome::ControlMessage,
                "refreshed presence must stay accepted"
            );
        }

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(
            service.control_presence_count(),
            1,
            "a peer refreshed within its TTL must stay in active presence"
        );
    }
}
