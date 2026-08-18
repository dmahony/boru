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
    collections::HashSet,
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
use tokio::{sync::broadcast, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use crate::api::{ApiError, Event, GossipReceiver, GossipSender, Message as GossipMessage};
use crate::control_plane::advertisement::{
    AdvertisementAuth, PublicRoomAdvertisement as AdvertisementPayload, RoomVisibility,
};
use crate::control_plane::capabilities::{
    compatible_version, default_local_capabilities, CapabilitySet,
};
use crate::control_plane::connectivity::{
    ConnectivityEvent, PathKind, PeerConnectivityState, PeerConnectivityStore,
};
use crate::control_plane::extensions::{default_local_extensions, ExtensionsPayload};
use crate::control_plane::message::{
    ControlEnvelope, ControlPayload, ControlPlaneDecode, BORU_APP_PROTOCOL_VERSION,
    CONTROL_PLANE_MAGIC,
};
use crate::control_plane::privacy::{
    AdvertViolation, ControlPlaneGuard, GuardRejectReason, GuardVerdict, DEFAULT_PRESENCE_TTL,
    EXPIRY_SWEEP_INTERVAL,
};
pub use crate::control_plane::reconnect::ReconnectSignal;
use crate::control_plane::reconnect::{ReconnectHandle, ReconnectScheduler, ReconnectState};
// The peer registry + `(node_id, event_id)` dedup logic lives in its own
// focused module (BORU-DISC-004). Re-exported here so the public path
// `boru_core::discovery_service::PeerRegistry` / `PeerSource` / `UpsertOutcome`
// / `PeerRegistryEntry` (used by integration tests, doctor.rs, main.rs) stays
// stable — DiscoveryService keeps only the `Arc<Mutex<PeerRegistry>>` handle.
pub use crate::discovery::peer_registry::{PeerRegistry, PeerRegistryEntry, PeerSource, UpsertOutcome};
use crate::diagnostics::{DiagnosticCounters, DirectoryCounters, DIAGNOSTIC_COUNTERS, DIRECTORY_COUNTERS};
use crate::discovery_message::{check_discovery_version, DiscoveryMessage, DiscoveryVersionCheck};
use crate::proto::TopicId;
use crate::room_directory::{AdvertiseOutcome, RoomDirectory};

/// Capacity of the peer-update broadcast channel.
const PEER_UPDATES_CAPACITY: usize = 256;

/// Default minimum interval between discovery announcements (Hello /
/// Presence). Announcements are throttled to at most one per interval so a
/// join hello plus neighbour-up re-announcements cannot become an aggressive
/// broadcast loop on the discovery topic.
pub const DEFAULT_ANNOUNCE_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Default minimum interval between **control-plane** announcements
/// (HELLO / PRESENCE envelopes, BORU-CP-04). A separate throttle instance
/// from the legacy discovery announcements so the control-plane presence
/// refresh cannot be starved by legacy neighbour-up hellos (and vice
/// versa).
pub const DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Default base interval between control-plane PRESENCE refresh
/// announcements (BORU-CP-04 / PDF Task 2.1 step 3). Deliberately low
/// frequency and comfortably under [`DEFAULT_PRESENCE_TTL`] so a peer's
/// presence never goes stale between refreshes.
pub const DEFAULT_PRESENCE_REFRESH_INTERVAL: Duration = Duration::from_secs(120);

/// Default jitter added to each presence-refresh sleep. Randomising the
/// per-cycle delay desynchronises nodes so they do not announce in
/// synchronised bursts (PDF Task 2.1 step 3).
pub const DEFAULT_PRESENCE_REFRESH_JITTER: Duration = Duration::from_secs(60);

/// Announce CAPABILITIES every N-th presence-refresh tick (BORU-CP-11 /
/// PDF Task 4.2 step 2). Presence refreshes every
/// [`DEFAULT_PRESENCE_REFRESH_INTERVAL`], so this re-broadcasts the local
/// capability set roughly every 6 minutes at the default cadence — enough
/// for a peer that joined after our startup announcement to still learn the
/// current set, while remaining low-frequency (bounded resources). `0`
/// disables periodic capability refreshes entirely.
pub const DEFAULT_CAPABILITIES_REFRESH_EVERY: u32 = 3;

/// Announce EXTENSIONS every N-th presence-refresh tick (BORU-CP-16 / PDF
/// Phase 6). Mirrors [`DEFAULT_CAPABILITIES_REFRESH_EVERY`]: presence
/// refreshes every [`DEFAULT_PRESENCE_REFRESH_INTERVAL`], so this
/// re-broadcasts the local extensions advertisement roughly every 6 minutes
/// at the default cadence — enough for a peer that joined after our startup
/// announcement to still learn the current payload, while remaining
/// low-frequency (bounded resources). `0` disables periodic extensions
/// refreshes entirely.
pub const DEFAULT_EXTENSIONS_REFRESH_EVERY: u32 = 3;

/// How often the reconnection loop (BORU-CP-07) wakes to drain due
/// reconnect attempts and apply backoff. Queued attempts are picked up
/// within one tick; backoff deadlines are checked every tick.
pub const RECONNECT_LOOP_TICK: Duration = Duration::from_secs(1);

/// How long the reconnect loop waits for a queued dial to be CONFIRMED by
/// the network (a gossip `NeighborUp` → the peer reaches `Reachable`)
/// before treating the attempt as failed and backing off. A queued-but-
/// unconfirmed dial is never message-path recovery (PDF Task 3.1).
pub const RECONNECT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);

/// How often the room-directory TTL sweep (BORU-DIR-23, PDF Phase 8 test
/// matrix scenario \"Advertiser disappears\") wakes to evict expired room
/// advertisements.
///
/// Each cached advertisement carries its own `expires_after_secs` TTL
/// (policy minimum 60 s, default 1 h). The sweep runs every
/// [`DEFAULT_DIRECTORY_SWEEP_INTERVAL`] — comfortably under the policy
/// minimum TTL so a room whose advertiser disappears leaves the active
/// directory within one sweep of its expiry, while refreshes arriving
/// within the TTL keep it live (no flicker on temporary packet loss; PDF
/// Task 3.2 step 5). This is the production wiring for the cache's
/// [`evict_expired`](crate::room_directory::RoomDirectory::evict_expired)
/// — without it, expired entries would only be evicted as a side effect of
/// the *next* advertisement arriving.
pub const DEFAULT_DIRECTORY_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------


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
    /// A PUBLIC_ROOM_ADVERTISEMENT envelope was received and decoded into its
    /// typed room-discovery payload (BORU-DIR-01, PDF Phase 1 Task 1.1).
    ///
    /// This is the service-boundary decode path for room advertisements:
    /// the advertisement payload is interpreted **here** — inside the
    /// discovery/control-plane service — and surfaced to subscribers as a
    /// typed [`RoomAdvertisementEvent`]. It never joins a room, subscribes
    /// to a chat topic, downloads history, or grants permission (PDF Core
    /// rule); a malformed or oversized advertisement is rejected at the
    /// receive gate and never reaches this event. Fully separate from peer
    /// presence ([`ControlEvent::Received`] still carries PRESENCE
    /// envelopes) and from normal chat messages.
    RoomAdvertisement(RoomAdvertisementEvent),
    /// A PUBLIC_ROOM_WITHDRAWAL envelope was received, verified, and decoded
    /// into its typed room-withdrawal payload (BORU-DIR-09, PDF Phase 3
    /// Task 3.3).
    ///
    /// This is the service-boundary decode path for withdrawals: the
    /// payload is interpreted **here** — inside the discovery/control-plane
    /// service — and surfaced to subscribers as a typed
    /// [`RoomWithdrawalEvent`]. Only a withdrawal that verifies as signed
    /// by the room's designated authority is emitted; a spoofed,
    /// untrusted, or non-authoritative withdrawal is discarded at the
    /// receive gate and never reaches this event. Subscribers remove the
    /// matching advertisement immediately; TTL expiry remains the safety
    /// net if the withdrawal is missed.
    RoomWithdrawal(RoomWithdrawalEvent),
}

/// A decoded PUBLIC_ROOM_ADVERTISEMENT (BORU-DIR-01/02).
///
/// Carries the envelope metadata (sender, sequence, timestamp) plus the
/// typed, bounded advertisement payload (BORU-DIR-02 metadata model:
/// room_id, room_name, short_description, room_protocol_version,
/// owner_peer_id, visibility, TTL, and optional tags / activity /
/// member-count / avatar / feature flags) and the publisher-authentication
/// verdict (BORU-DIR-03).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomAdvertisementEvent {
    /// The node that published the advertisement (envelope sender).
    pub sender_node_id: PublicKey,
    /// Per-sender sequence counter (dedup key part).
    pub sequence: u64,
    /// Unix epoch seconds when the advertisement was created.
    pub timestamp_secs: u64,
    /// Publisher-authentication verdict (BORU-DIR-03): the advertisement is
    /// attributed to `sender_node_id` only when this is
    /// [`AdvertisementAuth::Verified`]. A [`AdvertisementAuth::MissingSignature`]
    /// advertisement is clearly untrusted (never canonical); an
    /// [`AdvertisementAuth::InvalidSignature`] advertisement never reaches
    /// this event — it is discarded at the receive gate.
    pub auth: AdvertisementAuth,
    /// The typed advertisement payload (BORU-DIR-02 metadata model).
    pub advert: crate::control_plane::advertisement::PublicRoomAdvertisement,
}

/// A decoded, **verified** PUBLIC_ROOM_WITHDRAWAL (BORU-DIR-09).
///
/// Carries the envelope metadata (sender, sequence, timestamp) plus the
/// typed withdrawal payload. Only a withdrawal that verified as signed by
/// the room's designated authority is emitted (the same authoritative
/// identity rules as advertisements, BORU-DIR-03): a spoofed, untrusted, or
/// non-authoritative withdrawal is discarded at the receive gate and never
/// reaches this event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomWithdrawalEvent {
    /// The node that published the withdrawal (envelope sender) — also the
    /// room's designated authority (`owner_peer_id`), which is why it was
    /// emitted.
    pub sender_node_id: PublicKey,
    /// Per-sender sequence counter (dedup key part).
    pub sequence: u64,
    /// Unix epoch seconds when the withdrawal was created.
    pub timestamp_secs: u64,
    /// The typed withdrawal payload. `auth` is always
    /// [`AdvertisementAuth::Verified`] for this event — the payload was
    /// verified and the publisher was the room authority before emission.
    pub withdrawal: crate::control_plane::advertisement::PublicRoomWithdrawal,
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
    /// A PUBLIC_ROOM_ADVERTISEMENT frame was dropped because its publisher
    /// signature did not verify against the envelope's claimed sender
    /// (BORU-DIR-03, PDF Task 1.3): the payload was forged or tampered
    /// with. Discarded — never enters the directory view, never affects
    /// gossip or chat processing.
    AdvertisementAuthRejected,
    /// A PUBLIC_ROOM_WITHDRAWAL frame was dropped because its publisher
    /// signature did not verify against the envelope's claimed sender
    /// (BORU-DIR-09, PDF Task 3.3): the withdrawal was forged, tampered
    /// with, or unsigned. Discarded — it can never remove an advertisement.
    WithdrawalAuthRejected,
    /// A PUBLIC_ROOM_WITHDRAWAL frame verified for its publisher, but that
    /// publisher is **not** the room's designated authority
    /// (`owner_peer_id`) — a verified-but-spoofed withdrawal attempt
    /// (BORU-DIR-09, same authoritative identity rules as advertisements).
    /// Discarded — it can never remove the room's advertisement.
    WithdrawalNotAuthoritative,
}

/// Outcome of a throttled discovery announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceOutcome {
    /// The announcement was broadcast to the discovery topic.
    Announced,
    /// The announcement was suppressed by the throttle (too soon since the
    /// last one); nothing was broadcast.
    Throttled,
    /// The announcement was a no-op: the payload (e.g. the local capability
    /// set) is byte-identical to the last announced one, so nothing was
    /// broadcast (BORU-CP-11 idempotence — no duplicate advertisements for
    /// an unchanged capability set).
    Unchanged,
    /// The announcement was refused by the visibility guard (BORU-DIR-04):
    /// a room advertisement for a Private or PublicUnlisted room was
    /// submitted, and only PublicDiscoverable rooms may be advertised.
    /// Nothing was broadcast.
    NotDiscoverable,
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
        self.state
            .lock()
            .expect("announce throttle lock poisoned")
            .min_interval
    }

    /// Update the minimum interval between announcements.
    ///
    /// Safe to call while the throttle is shared (the service handle and the
    /// drain loop use the same instance).
    pub fn set_min_interval(&self, min_interval: Duration) {
        self.state
            .lock()
            .expect("announce throttle lock poisoned")
            .min_interval = min_interval;
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
            // BORU-CP-07: seed the event-id counter RANDOMLY so a restarted
            // process (same identity) does not reuse the pre-restart id
            // space. The gossip actor dedups by message content (blake3,
            // plumtree `MessageId`), so a byte-identical HELLO from a
            // restarted peer is dropped at the gossip layer and never
            // reaches the discovery service — silently breaking the
            // automatic-reconnection trigger. A random start makes every
            // process incarnation's announcements distinct while keeping
            // within-process monotonicity for the (node_id, event_id)
            // dedup key.
            next_event_id: Arc::new(AtomicU64::new(rand::random::<u64>())),
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
        self.announce(|event_id| DiscoveryMessage::hello_with_event(self.local_node, event_id))
            .await
    }

    /// Announce this node with a `Presence` heartbeat carrying a fresh
    /// per-node event id.
    async fn announce_presence(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(|event_id| DiscoveryMessage::presence_with_event(self.local_node, event_id))
            .await
    }
}

/// Shared control-plane announcement state (BORU-CP-04 / BORU-CP-11): the
/// gossip sender, the local node identity, a per-sender monotonic sequence
/// counter (BORU-CP-01 dedup key), and throttles for control-plane
/// announcements.
///
/// Separate from the legacy [`AnnounceHandle`]: control-plane HELLO /
/// PRESENCE / CAPABILITIES / EXTENSIONS envelopes (magic `BC`) are a
/// different wire format with their own sequence namespace, and their
/// refresh cadence must not be starved by legacy neighbour-up hellos (or
/// vice versa).
///
/// Shares one per-sender sequence counter across all control-plane message
/// types so receivers' `(sender_node_id, sequence)` dedup stays monotonic
/// per sender. The legacy announce throttle and the control throttle are
/// separate instances so the legacy neighbour-up hellos cannot starve the
/// control-plane presence refresh (or vice versa). CAPABILITIES gets its
/// own throttle too: the join-time control HELLO fires immediately before
/// the join-time capabilities announcement, and a shared throttle would
/// suppress the second. EXTENSIONS (BORU-CP-16) follows the same pattern.
#[derive(Clone, Debug)]
struct ControlAnnounceHandle {
    sender: GossipSender,
    local_node: PublicKey,
    /// BORU-CP-17: the node's Ed25519 secret key, used to sign every
    /// outbound control envelope so receivers can attribute relayed
    /// envelopes to this node cryptographically. `None` (tests) keeps the
    /// legacy unsigned envelope format.
    local_secret: Option<iroh_base::SecretKey>,
    sequence: Arc<AtomicU64>,
    throttle: Arc<AnnounceThrottle>,
    /// Separate throttle for CAPABILITIES announcements (BORU-CP-11). The
    /// join-time HELLO and the join-time capabilities announcement fire
    /// back-to-back; sharing the control throttle would starve one of them.
    caps_throttle: Arc<AnnounceThrottle>,
    /// The last capability set actually broadcast, as its wire id list.
    /// Used to make `announce_capabilities(force = false)` a no-op for an
    /// unchanged set (idempotence — no duplicate advertisements).
    last_announced_caps: Arc<Mutex<Option<Vec<String>>>>,
    /// Separate throttle for EXTENSIONS announcements (BORU-CP-16, PDF
    /// Phase 6). The join-time HELLO + CAPABILITIES + EXTENSIONS burst
    /// fires back-to-back; sharing either throttle would starve one.
    extensions_throttle: Arc<AnnounceThrottle>,
    /// Separate throttle for PUBLIC_ROOM_ADVERTISEMENT announcements
    /// (BORU-DIR-03). Room advertisements are lower-frequency and must not
    /// be starved by (or starve) the presence/capabilities/extension
    /// cadence; Phase 3 (publish/refresh) will tune the interval per room.
    advert_throttle: Arc<AnnounceThrottle>,
    /// The last EXTENSIONS payload actually broadcast. Used to make
    /// `announce_extensions(force = false)` a no-op for an unchanged payload
    /// (idempotence — no duplicate advertisements).
    last_announced_extensions: Arc<Mutex<Option<ExtensionsPayload>>>,
}

impl ControlAnnounceHandle {
    fn new(
        sender: GossipSender,
        local_node: PublicKey,
        local_secret: Option<iroh_base::SecretKey>,
    ) -> Self {
        Self {
            sender,
            local_node,
            local_secret,
            // BORU-DIR-23: seed the sequence counter with wall-clock
            // seconds (monotonic per identity across restarts). The
            // original random seed made a restarted advertiser's fresh
            // sequence space collide with the pre-restart space at the
            // receive gate (`PeerControlStateStore::record` rejects any
            // sequence `<=` the last seen for that sender), so a restarted
            // room's re-announcement was silently dropped ~50% of the
            // time (matrix scenario "Advertiser restarts — advertisement
            // returns after discovery startup"). `now_secs` both avoids
            // the gossip actor's blake3 content dedup for byte-identical
            // frames (the original rationale) and guarantees the
            // post-restart sequence is higher than anything the same
            // identity broadcast before.
            sequence: Arc::new(AtomicU64::new(unix_now_secs())),
            throttle: Arc::new(AnnounceThrottle::with_min_interval(
                DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
            )),
            caps_throttle: Arc::new(AnnounceThrottle::with_min_interval(
                DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
            )),
            last_announced_caps: Arc::new(Mutex::new(None)),
            extensions_throttle: Arc::new(AnnounceThrottle::with_min_interval(
                DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
            )),
            advert_throttle: Arc::new(AnnounceThrottle::with_min_interval(
                DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
            )),
            last_announced_extensions: Arc::new(Mutex::new(None)),
        }
    }

    /// Allocate the next per-sender control-plane sequence (monotonic,
    /// starts at 0). Receivers dedup by `(sender_node_id, sequence)`.
    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    /// Update the minimum interval for CAPABILITIES announcements
    /// (BORU-CP-11). Tests use short intervals.
    fn set_caps_min_interval(&self, min_interval: Duration) {
        self.caps_throttle.set_min_interval(min_interval);
    }

    /// Update the minimum interval for EXTENSIONS announcements
    /// (BORU-CP-16). Tests use short intervals.
    fn set_extensions_min_interval(&self, min_interval: Duration) {
        self.extensions_throttle.set_min_interval(min_interval);
    }

    /// BORU-CP-17: sign `envelope` with the node's secret key when one is
    /// available. Without a key (tests) the envelope is returned unchanged
    /// (legacy unsigned format).
    fn signed(&self, mut envelope: ControlEnvelope) -> ControlEnvelope {
        if let Some(sk) = &self.local_secret {
            envelope.sign(sk);
        }
        envelope
    }

    /// Throttled announce of an arbitrary control-plane envelope.
    ///
    /// The sequence is allocated ONLY when the announcement passes the
    /// throttle — a suppressed announcement does not consume a sequence, so
    /// the sequence space tracks actually-broadcast envelopes.
    async fn announce<F>(&self, build: F) -> Result<AnnounceOutcome, DiscoveryServiceError>
    where
        F: FnOnce(u64) -> ControlEnvelope,
    {
        if !self.throttle.try_announce() {
            debug!("discovery: control announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self.signed(build(sequence)).encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce this node with a control-plane HELLO: the stable peer
    /// identity (envelope `sender_node_id`) plus the minimum protocol
    /// metadata ([`BORU_APP_PROTOCOL_VERSION`]) — PDF Task 2.1 step 1.
    async fn announce_hello(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(|sequence| {
            ControlEnvelope::hello(
                self.local_node,
                sequence,
                unix_now_secs(),
                BORU_APP_PROTOCOL_VERSION,
            )
        })
        .await
    }

    /// Announce a control-plane PRESENCE heartbeat suggesting our own
    /// default presence TTL (receivers clamp it to their own default —
    /// BORU-CP-03).
    async fn announce_presence(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.announce(|sequence| {
            ControlEnvelope::presence(
                self.local_node,
                sequence,
                unix_now_secs(),
                Some(DEFAULT_PRESENCE_TTL.as_secs() as u32),
            )
        })
        .await
    }

    /// Announce a control-plane CAPABILITIES envelope carrying `caps`
    /// (BORU-CP-11 / PDF Task 4.2 steps 1–2).
    ///
    /// * `force = false` is the explicit startup / material-change path: an
    ///   unchanged set (byte-identical to the last broadcast) is a no-op
    ///   returning [`AnnounceOutcome::Unchanged`] — no duplicate
    ///   advertisement for a capability set that has not materially changed.
    /// * `force = true` is the periodic-refresh path: the set is
    ///   re-broadcast even when unchanged so peers that joined after the
    ///   previous announcement still learn the current set (the gossip
    ///   actor dedups byte-identical payloads for neighbours that already
    ///   have them).
    /// * `bypass_throttle = true` is the neighbour-up path: a freshly
    ///   connected peer must learn the set immediately even when the
    ///   join-time burst happened within the 30s min-interval (the
    ///   join-time announce and the mesh edge forming are often <1s apart
    ///   after a restart, so the throttle would otherwise suppress the
    ///   re-announce and the peer waits for the periodic refresh). The
    ///   throttle's broadcast-loop protection is unnecessary here because
    ///   NeighborUp is a discrete endpoint event, not a loop.
    ///
    /// Either way the CAPABILITIES throttle bounds the rate (unless
    /// bypassed), the sequence is allocated only when a broadcast actually
    /// happens, and the broadcast is a control-plane envelope — never a
    /// chat message.
    async fn announce_capabilities(
        &self,
        caps: &CapabilitySet,
        force: bool,
        bypass_throttle: bool,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        let wire = caps.to_wire();
        if !force {
            let last = self
                .last_announced_caps
                .lock()
                .expect("last announced caps lock poisoned");
            if last.as_deref() == Some(wire.as_slice()) {
                return Ok(AnnounceOutcome::Unchanged);
            }
        }
        if !bypass_throttle && !self.caps_throttle.try_announce() {
            debug!("discovery: capabilities announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self
            .signed(ControlEnvelope::capabilities(
                self.local_node,
                sequence,
                unix_now_secs(),
                wire.clone(),
            ))
            .encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        *self
            .last_announced_caps
            .lock()
            .expect("last announced caps lock poisoned") = Some(wire);
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce a control-plane EXTENSIONS envelope carrying `payload`
    /// (BORU-CP-16 / PDF Phase 6).
    ///
    /// Mirrors [`announce_capabilities`](Self::announce_capabilities):
    /// * `force = false` is the explicit startup / material-change path: an
    ///   unchanged payload (equal to the last broadcast) is a no-op
    ///   returning [`AnnounceOutcome::Unchanged`].
    /// * `force = true` is the periodic-refresh path: the payload is
    ///   re-broadcast even when unchanged so peers that joined after the
    ///   previous announcement still learn it.
    ///
    /// The EXTENSIONS throttle bounds the rate, the sequence is allocated
    /// only when a broadcast actually happens, and the broadcast is a
    /// control-plane envelope — never a chat message.
    async fn announce_extensions(
        &self,
        payload: &ExtensionsPayload,
        force: bool,
        bypass_throttle: bool,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        if payload.is_empty() {
            // Nothing to advertise: an all-None payload is a no-op even on
            // the forced refresh path.
            return Ok(AnnounceOutcome::Unchanged);
        }
        if !force {
            let last = self
                .last_announced_extensions
                .lock()
                .expect("last announced extensions lock poisoned");
            if last.as_ref() == Some(payload) {
                return Ok(AnnounceOutcome::Unchanged);
            }
        }
        if !bypass_throttle && !self.extensions_throttle.try_announce() {
            debug!("discovery: extensions announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self
            .signed(ControlEnvelope::extensions(
                self.local_node,
                sequence,
                unix_now_secs(),
                payload.clone(),
            ))
            .encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        *self
            .last_announced_extensions
            .lock()
            .expect("last announced extensions lock poisoned") = Some(payload.clone());
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce a PUBLIC_ROOM_ADVERTISEMENT control-plane envelope carrying
    /// `advert` (BORU-DIR-03, PDF Phase 1 Task 1.3).
    ///
    /// The caller is responsible for building the advertisement and signing
    /// it with its node key ([`PublicRoomAdvertisement::sign`]) — the
    /// service does not hold a secret key. An unsigned advertisement is
    /// still broadcast (receivers mark it clearly untrusted, never
    /// canonical); a signed one lets receivers attribute the payload to
    /// this node.
    ///
    /// The room-advertisement throttle bounds the rate independently of the
    /// presence/capabilities/extension cadence, and the sequence is
    /// allocated only when a broadcast actually happens. The broadcast is a
    /// control-plane envelope — never a chat message, never a join.
    async fn announce_room_advertisement(
        &self,
        advert: AdvertisementPayload,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        // BORU-DIR-04 (PDF 2.1): only PublicDiscoverable rooms are ever
        // advertised. Private and PublicUnlisted rooms must not emit a
        // PUBLIC_ROOM_ADVERTISEMENT — this is the emit-site guard.
        if !advert.visibility.is_discoverable() {
            debug!(
                visibility = ?advert.visibility,
                "discovery: refusing to advertise non-discoverable room",
            );
            return Ok(AnnounceOutcome::NotDiscoverable);
        }
        if !self.advert_throttle.try_announce() {
            debug!("discovery: room-advertisement announcement throttled");
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self
            .signed(ControlEnvelope::public_room_advertisement(
                self.local_node,
                sequence,
                unix_now_secs(),
                advert,
            ))
            .encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(AnnounceOutcome::Announced)
    }

    /// Announce a PUBLIC_ROOM_WITHDRAWAL control-plane envelope carrying
    /// `withdrawal` (BORU-DIR-09, PDF Phase 3 Task 3.3).
    ///
    /// The caller is responsible for building the withdrawal and signing it
    /// with its node key ([`PublicRoomWithdrawal::sign`]) — the service
    /// does not hold a secret key. An unsigned withdrawal is still
    /// broadcast, but receivers discard it (never applied); a signed one
    /// lets receivers attribute the payload to this node and apply it only
    /// when this node is the room's designated authority (`owner_peer_id`).
    ///
    /// The same room-advertisement throttle bounds the rate, and the
    /// sequence is allocated only when a broadcast actually happens. The
    /// broadcast is a control-plane envelope — never a chat message, never
    /// a join.
    async fn announce_room_withdrawal(
        &self,
        withdrawal: crate::control_plane::advertisement::PublicRoomWithdrawal,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        if !self.advert_throttle.try_announce() {
            debug!(
                "discovery: room-withdrawal announcement throttled",
            );
            return Ok(AnnounceOutcome::Throttled);
        }
        let sequence = self.next_sequence();
        let bytes = self
            .signed(ControlEnvelope::public_room_withdrawal(
                self.local_node,
                sequence,
                unix_now_secs(),
                withdrawal,
            ))
            .encode();
        self.sender
            .broadcast(Bytes::from(bytes))
            .await
            .map_err(|source| e!(DiscoveryServiceError::Api { source }))?;
        Ok(AnnounceOutcome::Announced)
    }
}

/// Current unix epoch seconds; `0` (unknown) on clock failure, which the
/// envelope treats as "timestamp unknown".
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
    /// The BORU-CP-05 explicit peer connectivity state machine: per-peer
    /// connectivity state + deterministic transition trail, updated only
    /// from real networking events. Shared between the receive path, the
    /// connectivity loop, and the presence-expiry sweep.
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    /// The BORU-CP-07 reconnection scheduler: per-peer reconnect queue with
    /// exponential backoff and a maximum retry cadence. Shared between the
    /// reconnect loop, the drain loop (real connection success resets
    /// backoff), the expiry sweep (offline cancels pending attempts), and
    /// the report API (direct-topic readiness resets backoff).
    reconnect: Arc<Mutex<ReconnectScheduler>>,
    /// Broadcast channel of [`ReconnectSignal`]s (BORU-CP-07): emitted when
    /// a reconnect attempt succeeds, consumed by the app layer to re-join
    /// the deterministic direct topic.
    reconnect_tx: broadcast::Sender<ReconnectSignal>,
    /// Atomic discovery counters (BORU-DISC-20). Cloned from the global
    /// [`DIAGNOSTIC_COUNTERS`] by default so the frontend/MCP can read the
    /// same values; tests inject an isolated instance.
    counters: DiagnosticCounters,
    /// Atomic room-directory advertisement counters (BORU-DIR-22, PDF
    /// Phase 8 Task 8.1). Cloned from the global [`DIRECTORY_COUNTERS`] by
    /// default so the frontend/MCP can read the same values; tests inject
    /// an isolated instance. Deliberately separate from `counters` (which
    /// tracks discovery *peers* and *topics*) — directory diagnostics
    /// answer *"what happened to room advertisements"* and stay distinct
    /// from room-message diagnostics (PDF Core rule).
    directory_counters: DirectoryCounters,
    /// Bounded local room-directory cache (BORU-DIR-10 / PDF Phase 4 Task
    /// 4.1): keyed by stable room_id, stores the latest valid advertisement
    /// plus provenance (publisher, auth verdict, first/last seen, expiry,
    /// compatibility, local join state), enforces entry-count +
    /// metadata-size bounds, and merges duplicate/refresh advertisements
    /// deterministically. Maintained by the control-plane receive path;
    /// subscribers read snapshots via
    /// [`DiscoveryService::room_directory`](crate::discovery_service::DiscoveryService::room_directory).
    /// Never creates conversation records or subscribes to room topics
    /// (PDF Core rule).
    room_directory: Arc<Mutex<RoomDirectory>>,
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
                    // BORU-CP-07: a same-id announcement from a peer that is
                    // currently Degraded / OfflineStale is NOT a duplicate
                    // delivery — it is a RESTART re-discovery. A restarted
                    // node reuses its event-id counter from 0, so its fresh
                    // HELLO collides with the pre-restart id; treating it as
                    // a duplicate would silently swallow the announcement
                    // that must trigger the automatic-reconnection path
                    // (PDF Task 3.1 step 1). The registry's dedup exists for
                    // duplicate *deliveries* of the same advertisement over
                    // two paths while the peer is alive/online — a peer that
                    // went Degraded/OfflineStale cannot deliver anything, so
                    // a same-id message is a new process incarnation.
                    //
                    // We do NOT mutate `last_event_id` here: the restarted
                    // node's counter will produce fresh ids next, and the
                    // registry refresh below records the new last-seen.
                    let peer_lost = {
                        let connectivity = self
                            .connectivity
                            .lock()
                            .expect("connectivity store lock poisoned");
                        matches!(
                            connectivity.state(&node_id),
                            PeerConnectivityState::Degraded | PeerConnectivityState::OfflineStale
                        )
                    };
                    if peer_lost {
                        info!(
                            node = %node_id.fmt_short(),
                            "discovery: same-id announcement from a restarted (lost) peer treated as rediscovery",
                        );
                        registry.refresh_after_restart(node_id, source, self.topic);
                    } else {
                        trace!(
                            node = %node_id.fmt_short(),
                            event = ?event_id,
                            "discovery: duplicate event ignored",
                        );
                        return IncomingOutcome::Duplicate;
                    }
                }
            }
        }

        // The registry is authoritative; the channel is a live notification
        // stream. Send errors only mean a caller stopped listening.
        let _ = self
            .peer_updates_tx
            .send(PeerUpdate::Seen { node_id, source });

        // BORU-CP-05: a real discovery event — feed the peer connectivity
        // state machine. Duplicates are already filtered above (registry
        // dedup), and re-delivering the same event is an idempotent no-op
        // in the state machine, so this can never cause a connection loop.
        {
            let mut connectivity = self
                .connectivity
                .lock()
                .expect("connectivity store lock poisoned");
            connectivity.apply(node_id, ConnectivityEvent::DiscoverySeen, Instant::now());
        }

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
    fn handle_control_incoming(
        &self,
        content: &[u8],
        delivered_from: PublicKey,
    ) -> IncomingOutcome {
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
            let mut guard = self
                .guard
                .lock()
                .expect("control-plane guard lock poisoned");
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
                // BORU-CP-05: a real discovery event — feed the peer
                // connectivity state machine. The guard already deduplicated
                // by (sender, sequence), so a duplicate delivery is an
                // idempotent no-op here (never a connection loop).
                {
                    let mut connectivity = self
                        .connectivity
                        .lock()
                        .expect("connectivity store lock poisoned");
                    connectivity.apply(
                        envelope.sender_node_id,
                        ConnectivityEvent::DiscoverySeen,
                        Instant::now(),
                    );
                }
                // BORU-DIR-01: decode room advertisements ONLY here — at the
                // discovery/control-plane service boundary. A
                // PUBLIC_ROOM_ADVERTISEMENT envelope is interpreted into its
                // typed payload and emitted as the dedicated
                // `ControlEvent::RoomAdvertisement` event — never as a
                // generic `Received` envelope, never into peer-presence,
                // conversation, or chat handling. Malformed/oversized
                // advertisements are already rejected by decode + guard
                // above, so reaching this point means the advertisement is
                // well-formed, bounded, and attributed to its real sender
                // (the transport attribution gate bound the envelope's
                // `sender_node_id` to the authenticated gossip delivery
                // source).
                // BORU-DIR-03 (PDF Task 1.3): the advertisement must ALSO
                // carry a valid publisher signature before it may enter the
                // trusted directory view. Verification is against the
                // envelope's `sender_node_id` — the claimed publisher.
                // * Invalid signature → forged/tampered payload: DISCARD.
                // * Missing signature → clearly untrusted: emitted with
                //   [`AdvertisementAuth::MissingSignature`] so the directory
                //   can list it as unverified but never as canonical.
                // * Verified → emitted with [`AdvertisementAuth::Verified`];
                //   whether the publisher is the room authority (canonical
                //   metadata) is decided by
                //   [`PublicRoomAdvertisement::is_authoritative_publisher`].
                if let ControlPayload::PublicRoomAdvertisement(advert) = &envelope.payload {
                    // BORU-DIR-22 (PDF Task 8.1): a decoded, guard-admitted
                    // room advertisement was received. Count it before the
                    // auth verdict so "received" includes both accepted and
                    // rejected advertisements (the developer can tell a
                    // room was *seen* even when it never entered the cache).
                    self.directory_counters.record_advertisement_received();
                    let auth = advert.verify_signed(&envelope.sender_node_id);
                    match auth {
                        AdvertisementAuth::InvalidSignature => {
                            self.counters.record_malformed_discovery_packet();
                            // BORU-DIR-22: auth-failed advertisement counted
                            // as rejected (distinct from expired / withdrawn /
                            // never-advertised).
                            self.directory_counters.record_advertisement_rejected();
                            warn!(
                                sender = %envelope.sender_node_id.fmt_short(),
                                sequence = envelope.sequence,
                                "discovery: room advertisement signature verification failed; dropped",
                            );
                            return IncomingOutcome::AdvertisementAuthRejected;
                        }
                        AdvertisementAuth::Verified { .. } | AdvertisementAuth::MissingSignature => {
                            info!(
                                sender = %envelope.sender_node_id.fmt_short(),
                                sequence = envelope.sequence,
                                advert_version = advert.advert_version,
                                auth = ?auth,
                                "discovery: public-room advertisement received",
                            );
                            // BORU-DIR-10 (PDF Phase 4, Task 4.1): maintain
                            // the bounded local room-directory cache at the
                            // discovery/control-plane service boundary — the
                            // same place advertisements are decoded. The
                            // cache is keyed by stable room_id, stores the
                            // latest valid advertisement plus provenance
                            // (publisher, auth verdict, first/last seen,
                            // expiry, compatibility, local join state), is
                            // bounded (entry count + metadata bytes), and
                            // merges duplicate/refresh advertisements
                            // deterministically. It NEVER creates a
                            // Conversation record, subscribes to a room
                            // topic, downloads history, or grants permission
                            // (PDF Core rule) — pure cached discovery
                            // metadata.
                            // BORU-DIR-11 (PDF Task 4.2): the directory
                            // deduplicates identical advertisements and
                            // detects conflicting metadata. Only a real
                            // cache change (Added/Refreshed) emits the
                            // typed UI event — repeated gossip and
                            // deterministic no-ops must not churn
                            // subscribers. Conflicts are logged at debug
                            // level (short identities only), never surfaced
                            // as normal UI events.
                            let outcome = self
                                .room_directory
                                .lock()
                                .expect("room directory lock poisoned")
                                .apply_advertisement(
                                    advert.clone(),
                                    envelope.sender_node_id,
                                    auth,
                                    envelope.sequence,
                                    envelope.timestamp_secs,
                                );
                            match outcome {
                                AdvertiseOutcome::Added | AdvertiseOutcome::Refreshed => {
                                    // BORU-DIR-22: the advertisement entered
                                    // or refreshed the directory cache.
                                    self.directory_counters.record_advertisement_accepted();
                                    let _ = self
                                        .control_events_tx
                                        .send(ControlEvent::RoomAdvertisement(
                                            RoomAdvertisementEvent {
                                                sender_node_id: envelope.sender_node_id,
                                                sequence: envelope.sequence,
                                                timestamp_secs: envelope.timestamp_secs,
                                                auth,
                                                advert: advert.clone(),
                                            },
                                        ));
                                }
                                AdvertiseOutcome::Duplicate => {
                                    // BORU-DIR-22: a repeated/identical
                                    // advertisement was collapsed into the
                                    // existing entry (no second card).
                                    self.directory_counters.record_advertisement_deduplicated();
                                    trace!(
                                        sender = %envelope.sender_node_id.fmt_short(),
                                        sequence = envelope.sequence,
                                        "discovery: duplicate room advertisement deduplicated; no UI churn",
                                    );
                                }
                                AdvertiseOutcome::Conflict => {
                                    debug!(
                                        sender = %envelope.sender_node_id.fmt_short(),
                                        sequence = envelope.sequence,
                                        room = %advert.room_id,
                                        "discovery: conflicting room advertisement; deterministic winner retained, entry marked conflicted",
                                    );
                                }
                                AdvertiseOutcome::Unchanged => {
                                    trace!(
                                        sender = %envelope.sender_node_id.fmt_short(),
                                        sequence = envelope.sequence,
                                        "discovery: room advertisement was a deterministic no-op",
                                    );
                                }
                            }
                            return IncomingOutcome::ControlMessage;
                        }
                    }
                }
                // BORU-DIR-09 (PDF Task 3.3): a PUBLIC_ROOM_WITHDRAWAL
                // envelope is interpreted into its typed payload here — at
                // the discovery/control-plane service boundary — and
                // emitted as the dedicated `ControlEvent::RoomWithdrawal`
                // event, never as a generic `Received` envelope.
                //
                // The same authoritative identity rules as advertisements
                // (BORU-DIR-03) apply before a withdrawal may be applied:
                // * Invalid or missing signature → forged/tampered/untrusted:
                //   DISCARD. It can never remove an advertisement.
                // * Verified but NOT signed by the room's designated
                //   authority (`owner_peer_id`) → verified-but-spoofed
                //   withdrawal attempt: DISCARD.
                // * Verified AND authoritative → emitted as
                //   `ControlEvent::RoomWithdrawal`; directory clients
                //   remove the matching advertisement immediately. TTL
                //   expiry remains the safety net if it is missed.
                if let ControlPayload::PublicRoomWithdrawal(withdrawal) = &envelope.payload {
                    let auth = withdrawal.verify_signed(&envelope.sender_node_id);
                    match auth {
                        AdvertisementAuth::InvalidSignature | AdvertisementAuth::MissingSignature => {
                            self.counters.record_malformed_discovery_packet();
                            warn!(
                                sender = %envelope.sender_node_id.fmt_short(),
                                sequence = envelope.sequence,
                                "discovery: room withdrawal signature verification failed; dropped",
                            );
                            return IncomingOutcome::WithdrawalAuthRejected;
                        }
                        AdvertisementAuth::Verified { .. } => {
                            if !withdrawal.is_authoritative_publisher(&envelope.sender_node_id) {
                                warn!(
                                    sender = %envelope.sender_node_id.fmt_short(),
                                    sequence = envelope.sequence,
                                    "discovery: room withdrawal signed by non-authority publisher; dropped",
                                );
                                return IncomingOutcome::WithdrawalNotAuthoritative;
                            }
                            info!(
                                sender = %envelope.sender_node_id.fmt_short(),
                                sequence = envelope.sequence,
                                room = %withdrawal.room_id,
                                "discovery: public-room withdrawal received and verified",
                            );
                            // BORU-DIR-10: apply the verified, authoritative
                            // withdrawal to the bounded directory cache
                            // immediately — the directory removes the room's
                            // entry when the withdrawing authority matches
                            // the stored owner. TTL expiry remains the
                            // safety net if a withdrawal is missed.
                            let removed = self
                                .room_directory
                                .lock()
                                .expect("room directory lock poisoned")
                                .apply_withdrawal(withdrawal.room_id, withdrawal.owner_peer_id);
                            // BORU-DIR-22 (PDF Task 8.1): a listing removed
                            // by a verified authoritative withdrawal is
                            // counted as withdrawn (distinct from expired /
                            // rejected / never-advertised).
                            if removed {
                                self.directory_counters.record_advertisement_withdrawn();
                            }
                            let _ = self
                                .control_events_tx
                                .send(ControlEvent::RoomWithdrawal(RoomWithdrawalEvent {
                                    sender_node_id: envelope.sender_node_id,
                                    sequence: envelope.sequence,
                                    timestamp_secs: envelope.timestamp_secs,
                                    withdrawal: withdrawal.clone(),
                                }));
                            return IncomingOutcome::ControlMessage;
                        }
                    }
                }

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
                        // BORU-DIR-22 (PDF Task 8.1): count advertisement
                        // envelopes dropped by the per-sender rate limiter
                        // (distinct from rejected-by-auth advertisements —
                        // the rate limiter fires before decode/policy).
                        if matches!(
                            &envelope.payload,
                            ControlPayload::PublicRoomAdvertisement(_)
                        ) {
                            self.directory_counters.record_advertisement_rate_limited();
                        }
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
    /// Control-plane announcement handle (BORU-CP-04): HELLO / PRESENCE
    /// envelopes with their own sequence counter and throttle.
    control_announce: ControlAnnounceHandle,
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
    /// Join handle of the control-plane presence-refresh task (BORU-CP-04):
    /// low-frequency PRESENCE announcements with jitter.
    refresh_task: JoinHandle<()>,
    /// Join handle of the reconnection task (BORU-CP-07): drains queued
    /// reconnect attempts with exponential backoff and emits
    /// [`ReconnectSignal`]s when a dial succeeds.
    reconnect_task: JoinHandle<()>,
    /// Join handle of the room-directory TTL sweep task (BORU-DIR-23 / PDF
    /// Phase 8 test matrix \"Advertiser disappears\"): periodically evicts
    /// room advertisements whose TTL elapsed since the last valid refresh,
    /// so rooms whose advertiser disappears leave the active directory
    /// naturally (TTL remains the final cleanup mechanism, PDF Task 3.2).
    directory_expiry_task: JoinHandle<()>,
    /// Join handle of the path-refresh sweep (BORU-CP-14): periodically
    /// classifies each tracked peer's path (direct / relay / transitioning)
    /// from the iroh endpoint's `remote_info` snapshots. `None` until the
    /// app attaches the endpoint via [`with_endpoint`](Self::with_endpoint);
    /// without it path diagnostics stay `unknown`.
    path_task: Option<JoinHandle<()>>,
    /// Shared presence-expiry configuration (TTL + sweep interval) so the
    /// builder can tune it after construction and the sweep observes it.
    expiry_config: Arc<Mutex<PresenceExpiryConfig>>,
    /// Shared control-plane presence-refresh configuration (base interval +
    /// jitter) so the builder can tune it after construction and the
    /// refresh loop observes it.
    refresh_config: Arc<Mutex<PresenceRefreshConfig>>,
    /// Shared room-directory expiry configuration (sweep interval) so the
    /// builder can tune it after construction and the directory-expiry
    /// sweep observes it (BORU-DIR-23).
    directory_expiry_config: Arc<Mutex<DirectoryExpiryConfig>>,
    /// The local capability set this node advertises (BORU-CP-11 / PDF Task
    /// 4.2). Defaults to [`default_local_capabilities`]; the app replaces it
    /// via [`update_local_capabilities`](Self::update_local_capabilities)
    /// when locally enabled capabilities materially change. Shared with the
    /// periodic refresh loop so it always re-announces the current set.
    local_caps: Arc<Mutex<CapabilitySet>>,
    /// The local Phase 6 extensions advertisement this node advertises
    /// (BORU-CP-16 / PDF Phase 6). Defaults to [`default_local_extensions`];
    /// the app replaces it via
    /// [`update_local_extensions`](Self::update_local_extensions) when the
    /// locally derived extension metadata materially changes (e.g. group
    /// reachability from known local memberships, device identity, file
    /// readiness). Shared with the periodic refresh loop so it always
    /// re-announces the current payload.
    local_extensions: Arc<Mutex<ExtensionsPayload>>,
}

/// Read-only negotiated-capability view used to gate optional-feature
/// initiation (BORU-CP-12 / PDF Task 4.3).
///
/// Answers "does peer X support feature Y, and at which version?" before
/// the UI offers or attempts a feature. The concrete implementation is
/// [`DiscoveryCapabilityGate`], produced by
/// [`DiscoveryService::capability_gate`]; apps store it as
/// `Arc<dyn CapabilityGate>` so unit tests can inject a fixed view.
///
/// The view is metadata only: it grants no authorisation. Friendship,
/// group membership, and file-recipient permissions are still enforced
/// when a feature is invoked.
pub trait CapabilityGate: Send + Sync {
    /// The highest feature version both sides support for `feature`, or
    /// `None` when the peer is unknown/stale, does not advertise the
    /// feature, or shares no compatible version (fail closed).
    fn peer_supports(&self, node_id: &PublicKey, feature: &str) -> Option<u16>;

    /// The latest valid capability set cached for `peer` (metadata only).
    fn peer_capabilities(&self, node_id: &PublicKey) -> Option<CapabilitySet>;

    /// The local capability set this node currently advertises.
    fn local_capabilities(&self) -> CapabilitySet;
}

/// Live [`CapabilityGate`] backed by a discovery service's control-plane
/// state.
///
/// Cheap to clone: both halves are `Arc`-backed (the control-plane guard
/// and the local capability set), so the UI can hold one per conversation
/// without copying the registry.
#[derive(Clone, Debug)]
pub struct DiscoveryCapabilityGate {
    /// Receive-path core holding the shared control-plane guard.
    core: ReceiveCore,
    /// The local capability set this node advertises.
    local_caps: Arc<Mutex<CapabilitySet>>,
}

impl CapabilityGate for DiscoveryCapabilityGate {
    fn peer_supports(&self, node_id: &PublicKey, feature: &str) -> Option<u16> {
        let remote = self.peer_capabilities(node_id)?;
        let local = self.local_capabilities();
        compatible_version(&local, &remote, feature)
    }

    fn peer_capabilities(&self, node_id: &PublicKey) -> Option<CapabilitySet> {
        let guard = self
            .core
            .guard
            .lock()
            .expect("control-plane guard lock poisoned");
        let state = guard.presence().get_active(node_id, Instant::now())?;
        Some(state.capability_set())
    }

    fn local_capabilities(&self) -> CapabilitySet {
        self.local_caps
            .lock()
            .expect("local caps lock poisoned")
            .clone()
    }
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
    /// **control-plane HELLO** (BORU-CP-04, PDF Task 2.1 step 2) is
    /// published right after — the formal presence announcement carrying
    /// the stable peer identity + minimum protocol metadata, so peers'
    /// [`PeerControlStateStore`](crate::control_plane::privacy::PeerControlStateStore)
    /// learns this node without any chat message. A failed announcement is
    /// non-fatal: the receive path still works and the drain loop
    /// re-announces on neighbour-up.
    pub async fn join(
        gossip: &crate::net::Gossip,
        topic: TopicId,
        bootstrap: Vec<PublicKey>,
        local_node: PublicKey,
        local_secret: iroh_base::SecretKey,
    ) -> Result<Self, ApiError> {
        let subscription = gossip.subscribe(topic, bootstrap).await?;
        let (sender, receiver) = subscription.split();
        let service = Self::from_subscription_with_counters(
            topic,
            sender,
            receiver,
            local_node,
            Some(local_secret),
            DIAGNOSTIC_COUNTERS.clone(),
            DIRECTORY_COUNTERS.clone(),
        );
        match service.announce_hello().await {
            Ok(AnnounceOutcome::Announced) => {
                info!(topic = %topic, "discovery hello announced on join");
            }
            Ok(AnnounceOutcome::Throttled) => {
                debug!(topic = %topic, "discovery hello suppressed on join");
            }
            Ok(AnnounceOutcome::Unchanged) => {}
            Ok(_) => {}
            Err(error) => {
                warn!(
                    topic = %topic,
                    error = %error,
                    "discovery hello on join failed; continuing without it",
                );
            }
        }
        // BORU-CP-04: one control-plane HELLO shortly after the discovery
        // subscription becomes ready (PDF Task 2.1 step 2). Separate
        // throttle from the legacy hello, so both announcements pass.
        match service.announce_control_hello().await {
            Ok(AnnounceOutcome::Announced) => {
                info!(topic = %topic, "discovery control hello announced on join");
            }
            Ok(AnnounceOutcome::Throttled) => {
                debug!(topic = %topic, "discovery control hello suppressed on join");
            }
            Ok(AnnounceOutcome::Unchanged) => {}
            Ok(_) => {}
            Err(error) => {
                warn!(
                    topic = %topic,
                    error = %error,
                    "discovery control hello on join failed; continuing without it",
                );
            }
        }
        // BORU-CP-11: one CAPABILITIES announcement right after the
        // control HELLO (PDF Task 4.2 step 1: send capabilities on
        // startup). It has its own throttle, so the back-to-back HELLO +
        // CAPABILITIES burst passes. The periodic refresh loop keeps
        // re-announcing the set while running, so peers that join later
        // still learn it.
        match service.announce_capabilities().await {
            Ok(AnnounceOutcome::Announced) => {
                info!(topic = %topic, "discovery capabilities announced on join");
            }
            Ok(AnnounceOutcome::Throttled) => {
                debug!(topic = %topic, "discovery capabilities suppressed on join");
            }
            Ok(AnnounceOutcome::Unchanged) => {}
            Ok(_) => {}
            Err(error) => {
                warn!(
                    topic = %topic,
                    error = %error,
                    "discovery capabilities on join failed; continuing without it",
                );
            }
        }
        // BORU-CP-16: one EXTENSIONS announcement right after the
        // CAPABILITIES (PDF Phase 6). It has its own throttle, so the
        // back-to-back HELLO + CAPABILITIES + EXTENSIONS burst passes. The
        // periodic refresh loop keeps re-announcing the payload while
        // running, so peers that join later still learn it.
        match service.announce_extensions().await {
            Ok(AnnounceOutcome::Announced) => {
                info!(topic = %topic, "discovery extensions announced on join");
            }
            Ok(AnnounceOutcome::Throttled) => {
                debug!(topic = %topic, "discovery extensions suppressed on join");
            }
            Ok(AnnounceOutcome::Unchanged) => {}
            Ok(_) => {}
            Err(error) => {
                warn!(
                    topic = %topic,
                    error = %error,
                    "discovery extensions on join failed; continuing without it",
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
        local_secret: iroh_base::SecretKey,
    ) -> Result<Self, ApiError> {
        Self::join(gossip, topic, bootstrap, local_node, local_secret).await
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
            None,
            DIAGNOSTIC_COUNTERS.clone(),
            DIRECTORY_COUNTERS.clone(),
        )
    }

    /// Build a running service with explicit counter sets.
    ///
    /// Production callers use [`from_subscription`](Self::from_subscription),
    /// which shares the global [`DIAGNOSTIC_COUNTERS`] and
    /// [`DIRECTORY_COUNTERS`]; tests inject isolated instances so counter
    /// assertions never race with other tests or live app traffic.
    ///
    /// `local_secret` (BORU-CP-17): the node's Ed25519 secret key. When
    /// present, every control-plane envelope this service announces is
    /// signed so receivers can attribute relayed envelopes to this node.
    /// `None` keeps the legacy unsigned envelope format (tests / callers
    /// without a key).
    fn from_subscription_with_counters(
        topic: TopicId,
        sender: GossipSender,
        receiver: GossipReceiver,
        local_node: PublicKey,
        local_secret: Option<iroh_base::SecretKey>,
        counters: DiagnosticCounters,
        directory_counters: DirectoryCounters,
    ) -> Self {
        let registry = Arc::new(Mutex::new(PeerRegistry::new()));
        let (peer_updates_tx, _) = broadcast::channel(PEER_UPDATES_CAPACITY);
        let (control_events_tx, _) = broadcast::channel(PEER_UPDATES_CAPACITY);
        let (reconnect_tx, _) = broadcast::channel(PEER_UPDATES_CAPACITY);
        let guard = Arc::new(Mutex::new(ControlPlaneGuard::new()));
        let connectivity = Arc::new(Mutex::new(PeerConnectivityStore::new()));
        // BORU-CP-07: per-peer reconnect scheduler (exponential backoff +
        // maximum retry cadence, one active attempt per peer).
        let reconnect = Arc::new(Mutex::new(ReconnectScheduler::new()));
        // BORU-DIR-10: the bounded local room-directory cache, owned by the
        // discovery/control-plane layer (PDF Phase 4 Task 4.1).
        let room_directory = Arc::new(Mutex::new(RoomDirectory::new()));
        // BORU-DIR-22 (PDF Phase 8 Task 8.1): wire the TTL-expiry counter
        // into the cache so "expired advertisements" diagnostics are
        // truthful even though eviction runs inside the cache. The
        // directory is otherwise a pure cache with no diagnostics
        // dependency.
        room_directory
            .lock()
            .expect("room directory lock poisoned")
            .set_expired_sink(Some(directory_counters.expired_sink()));
        let core = ReceiveCore {
            local_node,
            topic,
            registry,
            peer_updates_tx,
            control_events_tx,
            guard,
            connectivity,
            reconnect,
            reconnect_tx,
            counters,
            directory_counters,
            room_directory,
        };
        let announce = AnnounceHandle::new(sender.clone(), local_node);
        let control_announce = ControlAnnounceHandle::new(sender, local_node, local_secret);
        let cancel = CancellationToken::new();
        let task_core = core.clone();
        let task_announce = announce.clone();
        let task_control = control_announce.clone();
        let task_cancel = cancel.clone();
        // BORU-CP-11/16: the local capability set and extensions payload,
        // shared with the drain loop so a NeighborUp can re-announce them
        // immediately (a peer that connects after our join announcement must
        // not wait for the periodic refresh cadence to learn what we
        // support). Created here, before the drain loop spawn.
        let local_caps = Arc::new(Mutex::new(default_local_capabilities()));
        let local_extensions = Arc::new(Mutex::new(default_local_extensions()));
        let task_caps = local_caps.clone();
        let task_extensions = local_extensions.clone();
        let task = tokio::spawn(drain_loop(
            receiver,
            task_core,
            task_announce,
            task_control,
            task_caps,
            task_extensions,
            task_cancel,
        ));
        // BORU-DISC-11: connectivity wiring — dial newly discovered peers
        // into the discovery gossip mesh via join_peers (the same mechanism
        // the mDNS/DHT paths use). This improves connectivity ONLY; it never
        // grants friendship/group membership or routes chat payloads.
        let connectivity_cancel = cancel.clone();
        let connectivity_task = tokio::spawn(connectivity_loop(
            announce.sender.clone(),
            core.peer_updates_tx.subscribe(),
            core.connectivity.clone(),
            core.reconnect.clone(),
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
            core.connectivity.clone(),
            core.reconnect.clone(),
            core.peer_updates_tx.clone(),
            expiry_cancel,
        ));
        // BORU-DIR-23 (PDF Phase 8 test matrix): room-directory TTL sweep —
        // every `directory_sweep_interval` evicts cached room advertisements
        // whose TTL elapsed since the last valid refresh, so rooms whose
        // advertiser disappears leave the active directory naturally (PDF
        // Task 3.2 step 4; TTL remains the final cleanup mechanism). The
        // sweep interval is re-read every cycle, so tests can tune it after
        // construction.
        let directory_expiry_config = Arc::new(Mutex::new(DirectoryExpiryConfig {
            sweep_interval: DEFAULT_DIRECTORY_SWEEP_INTERVAL,
        }));
        let directory_expiry_cancel = cancel.clone();
        let directory_expiry_task = tokio::spawn(directory_expiry_loop(
            directory_expiry_config.clone(),
            core.room_directory.clone(),
            directory_expiry_cancel,
        ));
        // BORU-CP-04: control-plane presence refresh — low-frequency
        // PRESENCE announcements with jitter so presence stays fresh without
        // synchronised bursts. The join-time HELLO already covers the
        // immediate announcement; this loop keeps it alive while running.
        let refresh_config = Arc::new(Mutex::new(PresenceRefreshConfig {
            interval: DEFAULT_PRESENCE_REFRESH_INTERVAL,
            jitter: DEFAULT_PRESENCE_REFRESH_JITTER,
            caps_every: DEFAULT_CAPABILITIES_REFRESH_EVERY,
            extensions_every: DEFAULT_EXTENSIONS_REFRESH_EVERY,
        }));
        // BORU-CP-11: the local capability set this node advertises, shared
        // with the refresh loop (and the drain loop's neighbor-up
        // re-announce) so periodic capability refreshes always carry the
        // current set (and the app can replace it on material change via
        // [`DiscoveryService::update_local_capabilities`]). The Arc is
        // created above, before the drain loop spawn.
        let refresh_cancel = cancel.clone();
        let refresh_task = tokio::spawn(presence_refresh_loop(
            control_announce.clone(),
            local_caps.clone(),
            local_extensions.clone(),
            refresh_config.clone(),
            refresh_cancel,
        ));
        // BORU-CP-07: reconnection task — drains queued reconnect attempts
        // (queued by the app for freshly-announced known friends) using the
        // existing authenticated connection path (`join_peers`), with
        // exponential backoff and a maximum retry cadence. Emits
        // [`ReconnectSignal::PeerReachable`] when a dial succeeds so the
        // data plane can re-join the deterministic direct topic.
        let reconnect_cancel = cancel.clone();
        let reconnect_task = tokio::spawn(reconnect_loop(
            announce.sender.clone(),
            core.reconnect.clone(),
            core.connectivity.clone(),
            core.reconnect_tx.clone(),
            reconnect_cancel,
        ));
        info!(topic = %topic, "discovery service joined");
        Self {
            topic,
            announce,
            control_announce,
            core,
            cancel,
            task,
            connectivity_task,
            expiry_task,
            refresh_task,
            reconnect_task,
            directory_expiry_task,
            path_task: None,
            expiry_config,
            refresh_config,
            directory_expiry_config,
            local_caps,
            local_extensions,
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

    /// Announce this node with a **control-plane HELLO** (BORU-CP-04).
    ///
    /// Broadcasts a [`ControlEnvelope`] HELLO carrying the stable peer
    /// identity (`sender_node_id`) plus the minimum protocol metadata
    /// ([`BORU_APP_PROTOCOL_VERSION`]) — the formal presence announcement
    /// from PDF Task 2.1 step 1. Peers record it in their
    /// [`PeerControlStateStore`](crate::control_plane::privacy::PeerControlStateStore);
    /// no chat message is created and nothing touches chat history.
    ///
    /// Throttled by the control-plane announce throttle
    /// ([`DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL`]); the join-time
    /// announcement always passes.
    pub async fn announce_control_hello(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.control_announce.announce_hello().await
    }

    /// Announce a periodic **control-plane PRESENCE** heartbeat (BORU-CP-04,
    /// PDF Task 2.1 step 3) — the refresh announcement that keeps this
    /// node's presence alive while it is running. Throttled like
    /// [`announce_control_hello`](Self::announce_control_hello).
    pub async fn announce_control_presence(
        &self,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.control_announce.announce_presence().await
    }

    // ── Capability negotiation (BORU-CP-11/12 / PDF Task 4.2-4.3) ────────

    /// The local capability set this node currently advertises.
    ///
    /// Defaults to [`default_local_capabilities`] (every well-known
    /// capability implemented in this build). The app replaces it via
    /// [`update_local_capabilities`](Self::update_local_capabilities) when
    /// locally enabled capabilities materially change.
    pub fn local_capabilities(&self) -> CapabilitySet {
        self.capability_gate_value().local_capabilities()
    }

    /// Replace the local capability set and announce it when it materially
    /// changed (BORU-CP-11 / PDF Task 4.2 step 2).
    ///
    /// The new set is stored; if it differs from the last announced set a
    /// CAPABILITIES envelope is broadcast on the discovery topic — a
    /// control-plane message, never a chat message. If the set is
    /// byte-identical to the last announced one, nothing is broadcast
    /// ([`AnnounceOutcome::Unchanged`]) — an idempotent no-op.
    pub async fn update_local_capabilities(
        &self,
        caps: CapabilitySet,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        {
            let mut local = self.local_caps.lock().expect("local caps lock poisoned");
            *local = caps.clone();
        }
        // PDF Task 6.2 step 2: keep the room directory's optional-feature
        // negotiation in sync with the local capability set — a room's
        // `feature_compat` is derived from these capabilities.
        self.core
            .room_directory
            .lock()
            .expect("room directory lock poisoned")
            .set_local_capabilities(caps);
        self.announce_capabilities().await
    }

    /// Broadcast the current local capability set (startup + material-change
    /// path, BORU-CP-11 / PDF Task 4.2 step 1).
    ///
    /// Returns [`AnnounceOutcome::Unchanged`] when the set is byte-identical
    /// to the last announced one (no duplicate advertisement for an
    /// unchanged set). The periodic refresh loop re-announces the set on its
    /// own cadence so peers that joined after the previous announcement
    /// still learn it.
    pub async fn announce_capabilities(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        let caps = self.local_capabilities();
        self.control_announce
            .announce_capabilities(&caps, false, false)
            .await
    }

    // ── Phase 6 extensions (BORU-CP-16 / PDF Phase 6) ──────────────────

    /// The local Phase 6 extensions advertisement this node currently
    /// advertises.
    ///
    /// Defaults to [`default_local_extensions`] (every capability-backed
    /// extension section this build implements). The app replaces it via
    /// [`update_local_extensions`](Self::update_local_extensions) when the
    /// locally derived extension metadata materially changes.
    pub fn local_extensions(&self) -> ExtensionsPayload {
        self.local_extensions
            .lock()
            .expect("local extensions lock poisoned")
            .clone()
    }

    /// Replace the local extensions advertisement and announce it when it
    /// materially changed (BORU-CP-16 / PDF Phase 6).
    ///
    /// The new payload is stored; if it differs from the last announced one
    /// an EXTENSIONS envelope is broadcast on the discovery topic — a
    /// control-plane message, never a chat message. If the payload is
    /// identical to the last announced one, nothing is broadcast
    /// ([`AnnounceOutcome::Unchanged`]) — an idempotent no-op. An all-`None`
    /// payload is never broadcast (nothing to advertise).
    pub async fn update_local_extensions(
        &self,
        payload: ExtensionsPayload,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        {
            let mut local = self
                .local_extensions
                .lock()
                .expect("local extensions lock poisoned");
            *local = payload;
        }
        self.announce_extensions().await
    }

    /// Broadcast the current local extensions advertisement (startup +
    /// material-change path, BORU-CP-16 / PDF Phase 6).
    ///
    /// Returns [`AnnounceOutcome::Unchanged`] when the payload is identical
    /// to the last announced one (no duplicate advertisement) or empty. The
    /// periodic refresh loop re-announces the payload on its own cadence so
    /// peers that joined after the previous announcement still learn it.
    pub async fn announce_extensions(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        let payload = self.local_extensions();
        self.control_announce
            .announce_extensions(&payload, false, false)
            .await
    }

    /// Broadcast a PUBLIC_ROOM_ADVERTISEMENT control-plane envelope carrying
    /// `advert` (BORU-DIR-03, PDF Phase 1 Task 1.3).
    ///
    /// The caller builds the advertisement and signs it with its node key
    /// ([`PublicRoomAdvertisement::sign`](crate::control_plane::advertisement::PublicRoomAdvertisement::sign))
    /// so receivers can attribute the payload to this node — the service
    /// itself never holds a secret key. An unsigned advertisement is still
    /// broadcast but receivers treat it as clearly untrusted (never
    /// canonical).
    ///
    /// Visibility guard (BORU-DIR-04, PDF Phase 2 Task 2.1): only
    /// [`RoomVisibility::PublicDiscoverable`] rooms are advertised. A
    /// Private or PublicUnlisted advertisement is refused with
    /// [`AnnounceOutcome::NotDiscoverable`] and nothing is broadcast.
    ///
    /// The room-advertisement throttle bounds the rate; the broadcast is a
    /// control-plane envelope, never a chat message, and never a room join
    /// (PDF Core rule: advertisements only advertise existence).
    pub async fn announce_room_advertisement(
        &self,
        advert: crate::control_plane::advertisement::PublicRoomAdvertisement,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        if !advert.visibility.is_discoverable() {
            debug!(
                visibility = ?advert.visibility,
                "discovery: refusing to announce non-discoverable room advertisement",
            );
            return Ok(AnnounceOutcome::NotDiscoverable);
        }
        self.control_announce
            .announce_room_advertisement(advert)
            .await
    }

    /// Announce a PUBLIC_ROOM_WITHDRAWAL control-plane envelope carrying
    /// `withdrawal` (BORU-DIR-09, PDF Phase 3 Task 3.3).
    ///
    /// The caller builds the withdrawal and signs it with its node key
    /// ([`PublicRoomWithdrawal::sign`](crate::control_plane::advertisement::PublicRoomWithdrawal::sign))
    /// so receivers can attribute the payload to this node — the service
    /// itself never holds a secret key. An unsigned withdrawal is still
    /// broadcast but receivers discard it (never applied).
    ///
    /// The room-advertisement throttle bounds the rate; the broadcast is a
    /// control-plane envelope, never a chat message. Directory clients
    /// remove the matching advertisement when the withdrawal verifies;
    /// TTL expiry remains the safety net if it is missed.
    pub async fn announce_room_withdrawal(
        &self,
        withdrawal: crate::control_plane::advertisement::PublicRoomWithdrawal,
    ) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.control_announce
            .announce_room_withdrawal(withdrawal)
            .await
    }

    /// The latest Phase 6 extensions advertisement cached for `peer`
    /// (BORU-CP-16 / PDF Phase 6).
    ///
    /// Returns `None` when the peer is unknown, when its presence has gone
    /// stale (beyond its TTL — stale extension data is never treated as
    /// current), or when the peer never advertised extensions. The payload
    /// is metadata only: it grants no authorisation — friendship, group
    /// membership, tunnel/file/call permissions are still enforced when a
    /// feature is invoked on the private path.
    pub fn peer_extensions(&self, node_id: &PublicKey) -> Option<ExtensionsPayload> {
        let guard = self
            .core
            .guard
            .lock()
            .expect("control-plane guard lock poisoned");
        guard.presence().extensions_of(node_id)
    }

    /// The latest valid capability set cached for `peer` (BORU-CP-11 / PDF
    /// Task 4.2 steps 3–4).
    ///
    /// Returns `None` when the peer is unknown, when its presence has gone
    /// stale (beyond its TTL — stale capability data is never treated as
    /// current), or when the peer never advertised capabilities. The set is
    /// metadata only: it grants no authorisation — friendship/permissions
    /// are still enforced when a feature is invoked.
    pub fn peer_capabilities(&self, node_id: &PublicKey) -> Option<CapabilitySet> {
        self.capability_gate_value().peer_capabilities(node_id)
    }

    /// The highest feature version both sides support for `feature`, or
    /// `None` when the peer is unknown/stale or shares no version (fail
    /// closed — BORU-CP-11 / PDF Task 4.2 acceptance: Alice can know whether
    /// Bob supports a feature before presenting/attempting it).
    ///
    /// This is the negotiated view: it intersects the *remote* peer's cached
    /// advertisement with the *local* capability set
    /// ([`compatible_version`]), so a remote-only capability never looks
    /// negotiable. `None` means "do not present/attempt this feature".
    pub fn peer_supports(&self, node_id: &PublicKey, feature: &str) -> Option<u16> {
        self.capability_gate_value().peer_supports(node_id, feature)
    }

    /// A read-only, clonable handle to this service's negotiated-capability
    /// view (BORU-CP-12 / PDF Task 4.3).
    ///
    /// The UI stores this handle (not the [`DiscoveryService`] itself, which
    /// owns background tasks and the discovery subscription) to gate
    /// optional-feature initiation on the peer's advertised support.
    pub fn capability_gate(&self) -> Arc<dyn CapabilityGate> {
        Arc::new(self.capability_gate_value())
    }

    /// Build a value-typed gate view sharing this service's control-plane
    /// state (cheap: both halves are `Arc`-backed clones).
    fn capability_gate_value(&self) -> DiscoveryCapabilityGate {
        DiscoveryCapabilityGate {
            core: self.core.clone(),
            local_caps: self.local_caps.clone(),
        }
    }

    /// Override the minimum interval between **CAPABILITIES** control-plane
    /// announcements (BORU-CP-11).
    ///
    /// Defaults to [`DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL`]. CAPABILITIES
    /// has its own throttle so the join-time HELLO + capabilities burst and
    /// the periodic presence refresh never starve each other. Tests use
    /// short intervals to exercise the throttle without sleeping.
    pub fn with_capabilities_announce_min_interval(self, min_interval: Duration) -> Self {
        self.control_announce.set_caps_min_interval(min_interval);
        self
    }

    /// Override how often the periodic refresh loop re-announces
    /// CAPABILITIES (BORU-CP-11).
    ///
    /// Defaults to [`DEFAULT_CAPABILITIES_REFRESH_EVERY`] (every 3rd
    /// presence-refresh tick). `0` disables periodic capability refreshes.
    pub fn with_capabilities_refresh_every(self, every: u32) -> Self {
        self.refresh_config
            .lock()
            .expect("refresh config lock poisoned")
            .caps_every = every;
        self
    }

    /// Override the minimum interval between **EXTENSIONS** control-plane
    /// announcements (BORU-CP-16).
    ///
    /// Defaults to [`DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL`]. EXTENSIONS
    /// has its own throttle so the join-time HELLO + capabilities +
    /// extensions burst and the periodic presence refresh never starve each
    /// other. Tests use short intervals to exercise the throttle without
    /// sleeping.
    pub fn with_extensions_announce_min_interval(self, min_interval: Duration) -> Self {
        self.control_announce
            .set_extensions_min_interval(min_interval);
        self
    }

    /// Override how often the periodic refresh loop re-announces EXTENSIONS
    /// (BORU-CP-16).
    ///
    /// Defaults to [`DEFAULT_EXTENSIONS_REFRESH_EVERY`] (every 3rd
    /// presence-refresh tick). `0` disables periodic extensions refreshes.
    pub fn with_extensions_refresh_every(self, every: u32) -> Self {
        self.refresh_config
            .lock()
            .expect("refresh config lock poisoned")
            .extensions_every = every;
        self
    }

    /// Override the minimum interval between **control-plane**
    /// announcements (BORU-CP-04).
    ///
    /// Defaults to [`DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL`]. Tests use
    /// short intervals to exercise the control-plane throttle without
    /// sleeping.
    pub fn with_control_announce_min_interval(self, min_interval: Duration) -> Self {
        self.control_announce
            .throttle
            .set_min_interval(min_interval);
        self
    }

    /// Override the minimum interval between room-advertisement /
    /// room-withdrawal announcements (BORU-DIR-03).
    ///
    /// Defaults to [`DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL`]. Tests use
    /// short intervals so re-announcements (e.g. after an advertiser
    /// restart) are not throttled — the production periodic refresh
    /// cadence is longer than the default throttle interval, so real
    /// re-announcements are never throttled either.
    pub fn with_advert_min_interval(self, min_interval: Duration) -> Self {
        self.control_announce
            .advert_throttle
            .set_min_interval(min_interval);
        self
    }

    /// Override the control-plane presence-refresh base interval
    /// (BORU-CP-04).
    ///
    /// Defaults to [`DEFAULT_PRESENCE_REFRESH_INTERVAL`]. Tests use short
    /// intervals to exercise the refresh loop without sleeping.
    pub fn with_presence_refresh_interval(self, interval: Duration) -> Self {
        self.refresh_config
            .lock()
            .expect("refresh config lock poisoned")
            .interval = interval;
        self
    }

    /// Override the control-plane presence-refresh jitter (BORU-CP-04).
    ///
    /// Defaults to [`DEFAULT_PRESENCE_REFRESH_JITTER`]. Each refresh sleep
    /// is `interval + random(0..=jitter)` so nodes desynchronise (PDF Task
    /// 2.1 step 3: avoid synchronised bursts). Tests use `Duration::ZERO`
    /// for deterministic timing.
    pub fn with_presence_refresh_jitter(self, jitter: Duration) -> Self {
        self.refresh_config
            .lock()
            .expect("refresh config lock poisoned")
            .jitter = jitter;
        self
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
            let mut guard = self
                .core
                .guard
                .lock()
                .expect("control-plane guard lock poisoned");
            guard.set_default_presence_ttl(ttl);
        }
        self.expiry_config
            .lock()
            .expect("expiry config lock poisoned")
            .ttl = ttl;
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

    /// Override the room-directory TTL sweep interval (BORU-DIR-23 / PDF
    /// Phase 8 test matrix "Advertiser disappears").
    ///
    /// Defaults to [`DEFAULT_DIRECTORY_SWEEP_INTERVAL`]. Tests use short
    /// intervals to exercise the sweep without sleeping.
    pub fn with_directory_sweep_interval(self, interval: Duration) -> Self {
        self.directory_expiry_config
            .lock()
            .expect("directory expiry config lock poisoned")
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

    /// Shared handle to the bounded room-directory cache (BORU-DIR-10, PDF
    /// Phase 4 Task 4.1).
    ///
    /// The control-plane receive path maintains this cache as room
    /// advertisements/withdrawals arrive; subscribers (e.g. the Phase 5
    /// Discover Rooms UI) read deterministic snapshots through it. The
    /// cache is pure discovery metadata — keyed by stable room_id, bounded,
    /// and never conversation state (no [`ConversationEntry`](crate::conversations::ConversationEntry)
    /// is ever created and no room gossip topic is ever subscribed).
    pub fn room_directory(&self) -> Arc<Mutex<RoomDirectory>> {
        self.core.room_directory.clone()
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
        let bytes = self.control_announce.signed(envelope).encode();
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
        let registry = self
            .core
            .registry
            .lock()
            .expect("peer registry lock poisoned");
        registry
            .peers()
            .map(|(node_id, entry)| (*node_id, entry.clone()))
            .collect()
    }

    /// Number of peers currently in the registry.
    pub fn peer_count(&self) -> usize {
        let registry = self
            .core
            .registry
            .lock()
            .expect("peer registry lock poisoned");
        registry.len()
    }

    /// Number of peers currently in the control-plane presence store
    /// (BORU-CP-03 active presence hints).
    pub fn control_presence_count(&self) -> usize {
        let guard = self
            .core
            .guard
            .lock()
            .expect("control-plane guard lock poisoned");
        guard.presence_count()
    }

    /// Snapshot of the control-plane presence store (BORU-CP-03).
    ///
    /// Each entry is the metadata-only presence hint recorded from the
    /// peer's control-plane advertisements. This is a hint cache — it grants
    /// no authorisation and is never consulted by friendship/trust checks.
    pub fn control_presence_peers(
        &self,
    ) -> Vec<(PublicKey, crate::control_plane::privacy::PeerControlState)> {
        let guard = self
            .core
            .guard
            .lock()
            .expect("control-plane guard lock poisoned");
        guard
            .presence()
            .peers()
            .map(|(node_id, state)| (*node_id, state.clone()))
            .collect()
    }

    /// Current connectivity state for `peer` (BORU-CP-05).
    ///
    /// Returns [`PeerConnectivityState::Unknown`] when the peer is not
    /// tracked. This is the state-machine-derived status that replaces
    /// scattered 'online' booleans — UI (BORU-CP-06) and diagnostics read
    /// from here, never from ad-hoc flags.
    pub fn connectivity_state(
        &self,
        peer: &PublicKey,
    ) -> crate::control_plane::connectivity::PeerConnectivityState {
        let store = self
            .core
            .connectivity
            .lock()
            .expect("connectivity store lock poisoned");
        store.state(peer)
    }

    /// The deterministic transition trail for `peer` (BORU-CP-05), oldest
    /// first. Empty when the peer is not tracked.
    pub fn connectivity_trail(
        &self,
        peer: &PublicKey,
    ) -> Vec<crate::control_plane::connectivity::TransitionRecord> {
        let store = self
            .core
            .connectivity
            .lock()
            .expect("connectivity store lock poisoned");
        store.trail(peer)
    }

    /// Snapshot of the full connectivity state machine (BORU-CP-05): every
    /// tracked peer with its state, path hint, direct-topic state, last
    /// errors, and transition trail.
    pub fn connectivity_peers(
        &self,
    ) -> Vec<(
        PublicKey,
        crate::control_plane::connectivity::PeerConnectivityEntry,
    )> {
        let store = self
            .core
            .connectivity
            .lock()
            .expect("connectivity store lock poisoned");
        store
            .peers()
            .map(|(node_id, entry)| (*node_id, entry.clone()))
            .collect()
    }

    /// BORU-CP-13: structured, share-safe per-peer diagnostic snapshots.
    ///
    /// Each snapshot lets a developer tell whether a failure is in
    /// discovery, endpoint connectivity, topic join / subscription, gossip,
    /// decoding, or application delivery — without needing chat contents or
    /// secrets. Peer ids are truncated and the direct-topic id is only a
    /// short hash prefix, so the output is safe to paste into a bug report
    /// after normal review.
    pub fn peer_diagnostics(
        &self,
    ) -> Vec<crate::control_plane::diagnostics::PeerDiagnosticsSnapshot> {
        let store = self
            .core
            .connectivity
            .lock()
            .expect("connectivity store lock poisoned");
        crate::control_plane::diagnostics::snapshots_for(
            &store,
            &self.core.local_node,
            Instant::now(),
        )
    }

    /// BORU-CP-13: log every tracked peer's diagnostic snapshot at `debug!`
    /// (stable `peer=… state=…` lines). No-op when no peers are tracked.
    pub fn log_peer_diagnostics(&self) {
        let snapshots = self.peer_diagnostics();
        if snapshots.is_empty() {
            debug!("diagnostics: no peers tracked");
            return;
        }
        for snap in &snapshots {
            debug!(%snap, "diagnostics: per-peer snapshot");
        }
    }

    /// Read handle to the shared BORU-CP-05 connectivity state machine
    /// store.
    ///
    /// The GUI (BORU-CP-06) holds this to render the optional presence
    /// indicator from the backend state machine without holding the whole
    /// discovery service. The handle is read-only from the UI's
    /// perspective — state transitions are fed only by the discovery
    /// service and the data-plane report API; the UI never writes.
    pub fn connectivity_store(
        &self,
    ) -> Arc<Mutex<crate::control_plane::connectivity::PeerConnectivityStore>> {
        Arc::clone(&self.core.connectivity)
    }

    /// Report a data-plane networking event into the connectivity state
    /// machine (BORU-CP-05).
    ///
    /// This is the explicit event feed for events the discovery service
    /// cannot observe itself: deterministic direct-topic join success /
    /// failure, direct (non-discovery) message receipt, and relay/direct
    /// path changes. The chat/data-plane layer calls these; the state
    /// machine is updated ONLY from real networking events (PDF Task 2.2
    /// step 3). The direction is data-plane → control-plane state; the
    /// discovery service never calls chat code.
    pub fn report_connectivity_event(
        &self,
        peer: PublicKey,
        event: crate::control_plane::connectivity::ConnectivityEvent,
    ) -> crate::control_plane::connectivity::TransitionOutcome {
        let outcome = {
            let mut store = self
                .core
                .connectivity
                .lock()
                .expect("connectivity store lock poisoned");
            store.apply(peer, event, Instant::now())
        };
        // BORU-CP-07: a REAL connection/topic success cancels/resets any
        // queued reconnect retry/backoff state (PDF Task 3.1 step 6). Only
        // real events do this — discovery announcements never reach this
        // path, so discovery traffic alone can never clear backoff.
        match event {
            crate::control_plane::connectivity::ConnectivityEvent::EndpointConnected
            | crate::control_plane::connectivity::ConnectivityEvent::TopicJoined
            | crate::control_plane::connectivity::ConnectivityEvent::DirectMessageReceived => {
                let mut scheduler = self
                    .core
                    .reconnect
                    .lock()
                    .expect("reconnect scheduler lock poisoned");
                scheduler.reset(&peer);
            }
            _ => {}
        }
        outcome
    }

    /// Report a data-plane failure event (topic join failed / endpoint
    /// failed) with an error string, into the connectivity state machine
    /// (BORU-CP-05). The failure is visible as `Degraded` with
    /// `last_error` — never reported simply as 'online'.
    pub fn report_connectivity_failure(
        &self,
        peer: PublicKey,
        event: crate::control_plane::connectivity::ConnectivityEvent,
        error: String,
    ) -> crate::control_plane::connectivity::TransitionOutcome {
        let mut store = self
            .core
            .connectivity
            .lock()
            .expect("connectivity store lock poisoned");
        store.apply_with_error(peer, event, Some(error), Instant::now())
    }

    /// BORU-CP-14: attach the iroh endpoint so the background path-refresh
    /// sweep can classify each tracked peer's current path (direct / relay
    /// / transitioning) from `Endpoint::remote_info` snapshots (PDF Task
    /// 5.2 step 1).
    ///
    /// The endpoint is Arc-backed and cheap to clone; the sweep holds its
    /// own clone for the service lifetime. Path type is
    /// **diagnostic/optimization metadata only** (PDF Task 5.2 step 2): it
    /// never moves the connectivity state machine and chat delivery never
    /// depends on it. If the app never calls this, path diagnostics stay
    /// `unknown` and the service works exactly as before (the attach call
    /// is the feature switch).
    pub fn with_endpoint(mut self, endpoint: iroh::Endpoint) -> Self {
        let cancel = self.cancel.clone();
        let connectivity = Arc::clone(&self.core.connectivity);
        let path_task = tokio::spawn(path_refresh_loop(endpoint, connectivity, cancel));
        self.path_task = Some(path_task);
        self
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

    /// A cloneable handle for the reconnection subsystem (BORU-CP-07),
    /// safe to hand to the app layer.
    ///
    /// The app uses it to queue a reconnect attempt for a freshly-announced
    /// **known friend** ([`ReconnectHandle::queue_reconnect`]) and to
    /// report real direct-topic readiness
    /// ([`ReconnectHandle::report_topic_ready`]). Friendship stays in the
    /// app layer — this handle never decides friend-ness (no authorisation
    /// by presence).
    pub fn reconnect_handle(&self) -> ReconnectHandle {
        ReconnectHandle::new(self.core.reconnect.clone(), self.core.connectivity.clone())
    }

    /// Subscribe to live reconnection signals (BORU-CP-07).
    ///
    /// [`ReconnectSignal::PeerReachable`] is emitted ONLY after a reconnect
    /// attempt succeeds (a real dial via the existing authenticated
    /// connection path) — the data plane consumes it to re-join the
    /// deterministic direct topic. Discovery announcements alone never
    /// produce a signal.
    pub fn reconnect_events(&self) -> broadcast::Receiver<ReconnectSignal> {
        self.core.reconnect_tx.subscribe()
    }

    /// Queue ONE reconnection attempt for `peer` (BORU-CP-07).
    ///
    /// Convenience wrapper over [`ReconnectHandle::queue_reconnect`] for
    /// callers that already hold the service. Deduplicated: repeated
    /// announcements while an attempt is queued or in flight are no-ops;
    /// an already-online peer is never queued.
    pub fn queue_reconnect(&self, peer: PublicKey) -> bool {
        self.reconnect_handle().queue_reconnect(peer)
    }

    /// Snapshot of a peer's reconnect state (BORU-CP-07), if queued.
    pub fn reconnect_state(&self, peer: &PublicKey) -> Option<ReconnectState> {
        self.core
            .reconnect
            .lock()
            .expect("reconnect scheduler lock poisoned")
            .state(peer)
    }

    /// Override the reconnect backoff policy (BORU-CP-07).
    ///
    /// Defaults to [`DEFAULT_RECONNECT_INITIAL_BACKOFF`] /
    /// [`DEFAULT_RECONNECT_MAX_BACKOFF`]. Tests use short values to
    /// exercise retries without sleeping.
    pub fn with_reconnect_backoff(self, initial: Duration, max: Duration) -> Self {
        self.core
            .reconnect
            .lock()
            .expect("reconnect scheduler lock poisoned")
            .set_backoff(initial, max);
        self
    }

    /// Shut down the service: cancel the drain, connectivity, expiry,
    /// presence-refresh, and reconnection tasks and await them.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task.await;
        let _ = self.connectivity_task.await;
        let _ = self.expiry_task.await;
        let _ = self.refresh_task.await;
        let _ = self.reconnect_task.await;
        let _ = self.directory_expiry_task.await;
        if let Some(path_task) = self.path_task {
            let _ = path_task.await;
        }
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
    control_announce: ControlAnnounceHandle,
    local_caps: Arc<Mutex<CapabilitySet>>,
    local_extensions: Arc<Mutex<ExtensionsPayload>>,
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
                        // BORU-CP-05: a real endpoint-connection success —
                        // feed the peer connectivity state machine.
                        {
                            let mut connectivity = core
                                .connectivity
                                .lock()
                                .expect("connectivity store lock poisoned");
                            connectivity.apply(
                                peer,
                                ConnectivityEvent::EndpointConnected,
                                Instant::now(),
                            );
                        }
                        // BORU-CP-07: a real connection event cancels/resets
                        // any queued reconnect retry/backoff state (PDF Task
                        // 3.1 step 6). NeighborUp is a real endpoint success,
                        // not discovery metadata.
                        //
                        // If a reconnect attempt was pending for this peer,
                        // the connection is exactly the recovery the
                        // reconnect machinery exists to produce — surface it
                        // to the data plane so it re-joins the deterministic
                        // direct topic (PDF Task 3.1 step 3). This covers the
                        // case where the mesh self-heals (the gossip actor's
                        // own retry succeeds) BEFORE the reconnect loop's next
                        // attempt: without this emit, a real connection
                        // success would silently clear the queue and the data
                        // plane would never re-join the direct topic.
                        {
                            let mut scheduler = core
                                .reconnect
                                .lock()
                                .expect("reconnect scheduler lock poisoned");
                            let had_pending = scheduler.is_queued(&peer);
                            scheduler.reset(&peer);
                            if had_pending {
                                let _ = core
                                    .reconnect_tx
                                    .send(ReconnectSignal::PeerReachable { peer });
                            }
                        }
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
                                Ok(AnnounceOutcome::Unchanged) => {}
                                Ok(_) => {}
                                Err(error) => {
                                    warn!(
                                        peer = %peer.fmt_short(),
                                        error = %error,
                                        "discovery: neighbor-up hello failed",
                                    );
                                }
                            }
                        });
                        // BORU-CP-11/16: a freshly connected peer must learn
                        // the local capability set and extensions IMMEDIATELY,
                        // not on the next periodic refresh (up to ~6-9
                        // minutes at the default cadence). The join-time
                        // announcement can be missed when the peer connects
                        // after our hello went out — the 09:54-09:55 FILES-v2
                        // negotiation lag. force=true rebroadcasts even when
                        // the set is unchanged so the late joiner still
                        // receives it; the caps/extensions throttles bound
                        // the rate. Fire and forget: never block the drain.
                        let control_announce_caps = control_announce.clone();
                        let caps = local_caps
                            .lock()
                            .expect("local caps lock poisoned")
                            .clone();
                        tokio::spawn(async move {
                            match control_announce_caps
                                .announce_capabilities(&caps, true, true)
                                .await
                            {
                                Ok(AnnounceOutcome::Announced) => {
                                    info!(
                                        peer = %peer.fmt_short(),
                                        caps_count = caps.len(),
                                        "discovery: re-announced capabilities after neighbor up",
                                    );
                                }
                                Ok(AnnounceOutcome::Throttled) => {
                                    debug!(
                                        peer = %peer.fmt_short(),
                                        "discovery: neighbor-up capabilities suppressed by throttle",
                                    );
                                }
                                Ok(AnnounceOutcome::Unchanged) => {}
                                Ok(_) => {}
                                Err(error) => {
                                    warn!(
                                        peer = %peer.fmt_short(),
                                        error = %error,
                                        "discovery: neighbor-up capabilities failed",
                                    );
                                }
                            }
                        });
                        let control_announce_ext = control_announce.clone();
                        let extensions = local_extensions
                            .lock()
                            .expect("local extensions lock poisoned")
                            .clone();
                        tokio::spawn(async move {
                            match control_announce_ext
                                .announce_extensions(&extensions, true, true)
                                .await
                            {
                                Ok(AnnounceOutcome::Announced) => {
                                    info!(
                                        peer = %peer.fmt_short(),
                                        "discovery: re-announced extensions after neighbor up",
                                    );
                                }
                                Ok(AnnounceOutcome::Throttled) => {
                                    debug!(
                                        peer = %peer.fmt_short(),
                                        "discovery: neighbor-up extensions suppressed by throttle",
                                    );
                                }
                                Ok(AnnounceOutcome::Unchanged) => {}
                                Ok(_) => {}
                                Err(error) => {
                                    warn!(
                                        peer = %peer.fmt_short(),
                                        error = %error,
                                        "discovery: neighbor-up extensions failed",
                                    );
                                }
                            }
                        });
                    }
                    Some(Ok(Event::NeighborDown(peer))) => {
                        trace!(peer = %peer.fmt_short(), "discovery: neighbor down");
                        // BORU-CP-05: a real endpoint-connection failure —
                        // feed the peer connectivity state machine. If the
                        // peer was DirectTopicReady this degrades it; if it
                        // was already Degraded this is an idempotent no-op.
                        {
                            let mut connectivity = core
                                .connectivity
                                .lock()
                                .expect("connectivity store lock poisoned");
                            connectivity.apply(
                                peer,
                                ConnectivityEvent::EndpointFailed,
                                Instant::now(),
                            );
                        }
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
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    reconnect: Arc<Mutex<ReconnectScheduler>>,
    local_node: PublicKey,
    cancel: CancellationToken,
) {
    let mut dialed: HashSet<iroh_base::EndpointId> = HashSet::new();
    // BORU-CP-13: a slow periodic debug dump of the per-peer diagnostic
    // snapshots, so `RUST_LOG=debug` shows the full stage timeline
    // (discovery / endpoint / path / topic / gossip / decode / delivery)
    // without any extra tooling. Guarded by `tracing::enabled!` so the
    // render cost is zero when debug logging is off.
    let mut dump_interval = tokio::time::interval(Duration::from_secs(60));
    dump_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    dump_interval.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery connectivity loop cancelled");
                break;
            }
            _ = dump_interval.tick() => {
                if tracing::enabled!(tracing::Level::DEBUG) {
                    let store = connectivity.lock().expect("connectivity store lock poisoned");
                    let snapshots =
                        crate::control_plane::diagnostics::snapshots_for(&store, &local_node, Instant::now());
                    for snap in &snapshots {
                        debug!(%snap, "diagnostics: per-peer snapshot");
                    }
                }
            }
            update = updates.recv() => {
                match update {
                    Ok(PeerUpdate::Seen { node_id, .. }) => {
                        maybe_dial(
                            &sender,
                            &connectivity,
                            &reconnect,
                            &mut dialed,
                            local_node,
                            node_id,
                        )
                        .await;
                    }
                    Ok(PeerUpdate::Advertised { advertised, .. }) => {
                        maybe_dial(
                            &sender,
                            &connectivity,
                            &reconnect,
                            &mut dialed,
                            local_node,
                            advertised,
                        )
                        .await;
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

// ---------------------------------------------------------------------------
// Path classification (BORU-CP-14)
// ---------------------------------------------------------------------------

/// BORU-CP-14: how often the path-refresh sweep re-classifies every tracked
/// peer's current path from the iroh endpoint (seconds). Diagnostic
/// cadence; not latency-critical.
const PATH_REFRESH_INTERVAL_SECS: u64 = 15;

/// The transport kind of one address in iroh's `remote_info` snapshot,
/// reduced for classification (BORU-CP-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathAddrKind {
    /// A direct IP transport address.
    Ip,
    /// A relay server address.
    Relay,
    /// Any other (custom) transport address.
    Other,
}

/// Pure classification of an iroh `remote_info` snapshot into a path kind
/// (BORU-CP-14, PDF Task 5.2 step 1). Testable without a live endpoint.
///
/// `addrs` is `(kind, active)` for every known transport address.
/// Classification:
///
/// * any **active IP** path → [`PathKind::Direct`] (a direct path is open;
///   a relay fallback may also be open),
/// * otherwise any **active relay** path → [`PathKind::Relay`] (the peer is
///   reachable via relay right now — still reachable),
/// * otherwise known addresses but **none active** → [`PathKind::Transitioning`]
///   (path in flux: connecting / re-negotiating between direct and relay),
/// * no addresses at all → [`PathKind::Unknown`] (no reliable
///   classification — report Unknown rather than guessing).
///
/// The result is diagnostic/optimization metadata only; it never proves
/// application-level success and chat delivery never depends on it.
fn classify_path_addrs(addrs: impl IntoIterator<Item = (PathAddrKind, bool)>) -> PathKind {
    let mut has_any = false;
    let mut has_active_relay = false;
    for (kind, active) in addrs {
        has_any = true;
        if active {
            match kind {
                PathAddrKind::Ip => return PathKind::Direct,
                PathAddrKind::Relay => has_active_relay = true,
                PathAddrKind::Other => {}
            }
        }
    }
    if has_active_relay {
        PathKind::Relay
    } else if has_any {
        PathKind::Transitioning
    } else {
        PathKind::Unknown
    }
}

/// Classify one peer's current path from iroh's `remote_info` snapshot.
/// `None` (no information for the peer in the endpoint's remote map) →
/// [`PathKind::Unknown`].
async fn classify_peer_path(endpoint: &iroh::Endpoint, peer: PublicKey) -> PathKind {
    let endpoint_id: iroh_base::EndpointId = peer.into();
    let Some(info) = endpoint.remote_info(endpoint_id).await else {
        return PathKind::Unknown;
    };
    classify_path_addrs(info.addrs().map(|addr| {
        let kind = if addr.addr().is_ip() {
            PathAddrKind::Ip
        } else if addr.addr().is_relay() {
            PathAddrKind::Relay
        } else {
            PathAddrKind::Other
        };
        (
            kind,
            matches!(addr.usage(), iroh::endpoint::TransportAddrUsage::Active),
        )
    }))
}

/// BORU-CP-14: periodic per-peer path classification sweep.
///
/// Every [`PATH_REFRESH_INTERVAL_SECS`] the loop asks iroh for each tracked
/// peer's current transport addresses and records the classified path
/// (direct / relay / transitioning) in the connectivity store via the
/// diagnostic-only path events. Path *changes* are logged in structured
/// logs (`connectivity: peer path changed` at `info!`, from
/// [`PeerConnectivityStore::apply`]); path events never move the state
/// machine and never reset or duplicate conversation state (PDF Task 5.2).
///
/// Peers with no information in iroh's remote map ([`PathKind::Unknown`])
/// are skipped entirely — a lack of information must never fabricate a path
/// label or refresh a peer's liveness (which would defeat TTL expiry).
async fn path_refresh_loop(
    endpoint: iroh::Endpoint,
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(PATH_REFRESH_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery path refresh cancelled");
                break;
            }
            _ = interval.tick() => {
                let peers: Vec<PublicKey> = {
                    let store = connectivity.lock().expect("connectivity store lock poisoned");
                    store.peers().map(|(pk, _)| *pk).collect()
                };
                for peer in peers {
                    let kind = classify_peer_path(&endpoint, peer).await;
                    let event = match kind {
                        PathKind::Direct => ConnectivityEvent::PathChangedDirect,
                        PathKind::Relay => ConnectivityEvent::PathChangedRelay,
                        PathKind::Transitioning => ConnectivityEvent::PathChangedTransitioning,
                        PathKind::Unknown => continue,
                    };
                    let mut store = connectivity.lock().expect("connectivity store lock poisoned");
                    store.apply(peer, event, Instant::now());
                }
            }
        }
    }
    debug!("discovery path refresh exited");
}

/// Dial `peer` into the discovery gossip mesh once (deduplicated).
///
/// Connectivity only: `join_peers` makes the gossip actor establish a mesh
/// edge / resolve the peer's address book entry through the existing
/// mechanisms — it never creates friends, groups, or conversations.
///
/// BORU-CP-05: feeds the peer connectivity state machine with the dial
/// result — [`ConnectivityEvent::EndpointConnecting`] before the dial and
/// [`ConnectivityEvent::EndpointConnected`] / [`ConnectivityEvent::EndpointFailed`]
/// afterwards. Duplicate dials are filtered by `dialed`, and a duplicate
/// `EndpointConnecting` is an idempotent no-op in the state machine, so a
/// flood of announcements can never cause a connection loop.
async fn maybe_dial(
    sender: &GossipSender,
    connectivity: &Arc<Mutex<PeerConnectivityStore>>,
    reconnect: &Arc<Mutex<ReconnectScheduler>>,
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
    {
        let mut store = connectivity
            .lock()
            .expect("connectivity store lock poisoned");
        store.apply(peer, ConnectivityEvent::EndpointConnecting, Instant::now());
    }
    match sender.join_peers(vec![endpoint]).await {
        Ok(()) => {
            info!(peer = %peer.fmt_short(), "discovery: dialed discovered peer for connectivity");
            {
                let mut store = connectivity
                    .lock()
                    .expect("connectivity store lock poisoned");
                store.apply(peer, ConnectivityEvent::EndpointConnected, Instant::now());
            }
            // BORU-CP-07: the endpoint dial succeeded — a real connection
            // event. Cancel any queued reconnect attempt for this peer so
            // the reconnect loop does not dial again redundantly.
            {
                let mut scheduler = reconnect.lock().expect("reconnect scheduler lock poisoned");
                scheduler.reset(&peer);
            }
        }
        Err(error) => {
            warn!(
                peer = %peer.fmt_short(),
                error = %error,
                "discovery: join_peers failed",
            );
            {
                let mut store = connectivity
                    .lock()
                    .expect("connectivity store lock poisoned");
                store.apply_with_error(
                    peer,
                    ConnectivityEvent::EndpointFailed,
                    Some(error.to_string()),
                    Instant::now(),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reconnection (BORU-CP-07)
// ---------------------------------------------------------------------------

/// Background task that drains queued reconnect attempts (PDF Task 3.1).
///
/// The app queues a reconnect attempt for a freshly-announced **known
/// friend** via [`ReconnectHandle::queue_reconnect`]. This loop wakes every
/// [`RECONNECT_LOOP_TICK`], takes every due attempt (deduplicated and
/// marked in-flight by the scheduler), and performs it with the **existing
/// authenticated connection path** — [`GossipSender::join_peers`], the
/// same mechanism mDNS/DHT and the BORU-DISC-11 wiring use. No second
/// transport is invented.
///
/// * **Success** — feeds `EndpointConnected` into the connectivity state
///   machine, clears the peer's retry/backoff state, and emits
///   [`ReconnectSignal::PeerReachable`] so the data plane can re-join the
///   deterministic direct topic.
/// * **Failure** — feeds `EndpointFailed` and backs the peer off
///   exponentially ([`ReconnectScheduler::on_failure`], capped at the
///   maximum retry cadence).
///
/// Discovery traffic alone never succeeds here: a fresh announcement only
/// *queues* an attempt, and only a real successful dial produces a signal
/// or clears backoff.
async fn reconnect_loop(
    sender: GossipSender,
    scheduler: Arc<Mutex<ReconnectScheduler>>,
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    reconnect_tx: broadcast::Sender<ReconnectSignal>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery reconnect loop cancelled");
                break;
            }
            // A fresh sleep future each iteration gives a deterministic
            // one-tick cadence: the first drain runs one tick after the
            // loop starts, subsequent drains one tick after the previous
            // drain finishes. (An `interval` fires its first tick
            // immediately, which made unit tests race the very first
            // drain.)
            _ = tokio::time::sleep(RECONNECT_LOOP_TICK) => {
                drain_reconnect_attempts(&sender, &scheduler, &connectivity, &reconnect_tx).await;
            }
        }
    }
    debug!("discovery reconnect loop exited");
}

/// Perform every due reconnect attempt (one per peer, already marked
/// in-flight by the scheduler).
async fn drain_reconnect_attempts(
    sender: &GossipSender,
    scheduler: &Arc<Mutex<ReconnectScheduler>>,
    connectivity: &Arc<Mutex<PeerConnectivityStore>>,
    reconnect_tx: &broadcast::Sender<ReconnectSignal>,
) {
    let due = {
        let mut sched = scheduler.lock().expect("reconnect scheduler lock poisoned");
        sched.due(Instant::now())
    };
    if due.is_empty() {
        return;
    }
    for peer in due {
        // Re-use the existing Iroh endpoint/address information and the
        // normal authenticated connection path — join_peers resolves and
        // dials the peer exactly as mDNS/DHT discovery does.
        let endpoint: iroh_base::EndpointId = peer.into();
        let now = Instant::now();
        match sender.join_peers(vec![endpoint]).await {
            Ok(()) => {
                // The dial was queued. Wait for the REAL connection to be
                // confirmed by the network (a gossip `NeighborUp` moves the
                // peer to `Reachable`) before declaring success — a
                // queued-but-unconnected dial is not message-path recovery,
                // and only a confirmed dial clears retry/backoff state.
                let confirmed =
                    wait_for_reconnect_confirmation(connectivity, &peer, RECONNECT_CONFIRM_TIMEOUT)
                        .await;
                if confirmed {
                    // The dial was confirmed by a real connection event. If
                    // the drain loop's NeighborUp handler already surfaced
                    // this recovery (it resets the entry AND emits
                    // PeerReachable when a pending reconnect exists), don't
                    // emit a duplicate. Exactly one signal per recovery.
                    let cleared = {
                        let mut sched =
                            scheduler.lock().expect("reconnect scheduler lock poisoned");
                        let had = sched.is_queued(&peer);
                        sched.reset(&peer);
                        had
                    };
                    if cleared {
                        info!(peer = %peer.fmt_short(), "reconnect: endpoint connectivity re-established");
                        // Tell the data plane the endpoint is reachable again
                        // so it can ensure the deterministic direct topic is
                        // joined/subscribed (friend-scoped; the app owns
                        // direct topics).
                        let _ = reconnect_tx.send(ReconnectSignal::PeerReachable { peer });
                    } else {
                        trace!(
                            peer = %peer.fmt_short(),
                            "reconnect: recovery already surfaced by the drain loop"
                        );
                    }
                } else {
                    warn!(
                        peer = %peer.fmt_short(),
                        "reconnect: dial not confirmed, backing off",
                    );
                    {
                        let mut store = connectivity
                            .lock()
                            .expect("connectivity store lock poisoned");
                        store.apply_with_error(
                            peer,
                            ConnectivityEvent::EndpointFailed,
                            Some("reconnect dial not confirmed".to_string()),
                            now,
                        );
                    }
                    {
                        let mut sched =
                            scheduler.lock().expect("reconnect scheduler lock poisoned");
                        sched.on_failure(&peer, now);
                    }
                }
            }
            Err(error) => {
                warn!(
                    peer = %peer.fmt_short(),
                    error = %error,
                    "reconnect: attempt failed, backing off",
                );
                {
                    let mut store = connectivity
                        .lock()
                        .expect("connectivity store lock poisoned");
                    store.apply_with_error(
                        peer,
                        ConnectivityEvent::EndpointFailed,
                        Some(error.to_string()),
                        now,
                    );
                }
                {
                    let mut sched = scheduler.lock().expect("reconnect scheduler lock poisoned");
                    sched.on_failure(&peer, now);
                }
            }
        }
    }
}

/// Poll the connectivity state machine until `peer` is online
/// (`Reachable` / `DirectTopicReady`) — i.e. the queued dial was confirmed
/// by a real gossip `NeighborUp` — or the timeout elapses.
async fn wait_for_reconnect_confirmation(
    connectivity: &Arc<Mutex<PeerConnectivityStore>>,
    peer: &PublicKey,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let online = {
            let store = connectivity
                .lock()
                .expect("connectivity store lock poisoned");
            store.state(peer).is_online()
        };
        if online {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
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
    connectivity: Arc<Mutex<PeerConnectivityStore>>,
    reconnect: Arc<Mutex<ReconnectScheduler>>,
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
                    // BORU-CP-05: the timeout event moves the peer to
                    // OfflineStale in the connectivity state machine.
                    {
                        let mut store = connectivity.lock().expect("connectivity store lock poisoned");
                        store.apply(*node, ConnectivityEvent::Timeout, now);
                    }
                    // BORU-CP-07: the peer went offline — cancel any queued
                    // reconnect attempt. A later fresh announcement will
                    // re-queue from an immediate attempt (no residual
                    // backoff).
                    {
                        let mut scheduler = reconnect.lock().expect("reconnect scheduler lock poisoned");
                        scheduler.reset(node);
                    }
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
                    // BORU-CP-05: feed the timeout into the connectivity
                    // state machine too (idempotent if already offline).
                    {
                        let mut store = connectivity.lock().expect("connectivity store lock poisoned");
                        store.apply(*node, ConnectivityEvent::Timeout, now);
                    }
                    // BORU-CP-07: offline cancels any queued reconnect.
                    {
                        let mut scheduler = reconnect.lock().expect("reconnect scheduler lock poisoned");
                        scheduler.reset(node);
                    }
                }
            }
        }
    }
    debug!("discovery presence expiry loop exited");
}

// ---------------------------------------------------------------------------
// Control-plane presence refresh (BORU-CP-04)
// ---------------------------------------------------------------------------

/// Runtime-tunable control-plane presence-refresh configuration shared
/// between the [`DiscoveryService`] builders and the refresh task.
#[derive(Debug, Clone, Copy)]
struct PresenceRefreshConfig {
    /// Base delay between PRESENCE refresh announcements.
    interval: Duration,
    /// Jitter added to each sleep: `sleep(interval + random(0..=jitter))`.
    jitter: Duration,
    /// Announce CAPABILITIES every N-th refresh tick (`0` = never).
    /// Defaults to [`DEFAULT_CAPABILITIES_REFRESH_EVERY`]; each
    /// capabilities announcement uses its own throttle so the periodic
    /// presence and capability refreshes never starve each other.
    caps_every: u32,
    /// Announce EXTENSIONS every N-th refresh tick (`0` = never).
    /// Defaults to [`DEFAULT_EXTENSIONS_REFRESH_EVERY`]; each extensions
    /// announcement uses its own throttle (BORU-CP-16).
    extensions_every: u32,
}

/// Background task that keeps this node's control-plane presence alive
/// (BORU-CP-04, PDF Task 2.1 step 3) and periodically re-advertises the
/// local capability set (BORU-CP-11, PDF Task 4.2 step 2).
///
/// Every `interval + random(0..=jitter)` it broadcasts one control-plane
/// PRESENCE envelope (magic `BC`), so peers refresh this node's entry in
/// their [`PeerControlStateStore`](crate::control_plane::privacy::PeerControlStateStore).
/// The join-time HELLO covers the immediate announcement; this loop is the
/// low-frequency refresh "while running".
///
/// Every `caps_every`-th tick it additionally re-broadcasts the current
/// local capability set ([`CapabilitySet`]) — even when unchanged — so a
/// peer that joined after our startup announcement still learns the set
/// within a bounded time (the gossip actor dedups byte-identical payloads
/// for neighbours that already have them). The capabilities announcement
/// uses its own throttle, so the periodic presence and capability refreshes
/// never starve each other; an unchanged explicit announcement between
/// ticks is still a no-op ([`AnnounceOutcome::Unchanged`]).
///
/// The per-cycle jitter desynchronises nodes so a fleet of clients does not
/// announce in synchronised bursts. The interval is deliberately well under
/// [`DEFAULT_PRESENCE_TTL`] so a peer's presence never goes stale between
/// refreshes. The announcement still passes the control-plane announce
/// throttle, so an explicit announce right before a tick suppresses that
/// tick (idempotence — no duplicate bursts).
///
/// The configured interval/jitter/cadence are re-read every cycle so builder
/// tuning (e.g. short intervals in tests) takes effect immediately. Logs
/// state transitions only, never message contents.
async fn presence_refresh_loop(
    control_announce: ControlAnnounceHandle,
    local_caps: Arc<Mutex<CapabilitySet>>,
    local_extensions: Arc<Mutex<ExtensionsPayload>>,
    config: Arc<Mutex<PresenceRefreshConfig>>,
    cancel: CancellationToken,
) {
    let mut tick: u64 = 0;
    loop {
        let (interval, jitter, caps_every, extensions_every) = {
            let cfg = config.lock().expect("refresh config lock poisoned");
            (
                cfg.interval,
                cfg.jitter,
                cfg.caps_every,
                cfg.extensions_every,
            )
        };
        let delay = interval + random_jitter(jitter);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery presence refresh loop cancelled");
                break;
            }
            _ = tokio::time::sleep(delay) => {
                tick = tick.wrapping_add(1);
                match control_announce.announce_presence().await {
                    Ok(AnnounceOutcome::Announced) => {
                        info!(
                            interval_secs = interval.as_secs(),
                            jitter_secs = jitter.as_secs(),
                            "control: presence refresh announced",
                        );
                    }
                    Ok(AnnounceOutcome::Throttled) => {
                        trace!("control: presence refresh suppressed by throttle");
                    }
                    Ok(AnnounceOutcome::Unchanged) => {}
                    Ok(_) => {}
                    Err(error) => {
                        warn!(
                            error = %error,
                            "control: presence refresh failed; continuing",
                        );
                    }
                }
                // BORU-CP-11: periodic capability refresh (force=true so an
                // unchanged set still reaches peers that joined late).
                if caps_every > 0 && tick % caps_every as u64 == 0 {
                    let caps = local_caps.lock().expect("local caps lock poisoned").clone();
                    match control_announce.announce_capabilities(&caps, true, false).await {
                        Ok(AnnounceOutcome::Announced) => {
                            info!(
                                caps_count = caps.len(),
                                "control: capabilities refresh announced",
                            );
                        }
                        Ok(AnnounceOutcome::Throttled) => {
                            trace!("control: capabilities refresh suppressed by throttle");
                        }
                        Ok(AnnounceOutcome::Unchanged) => {}
                        Ok(_) => {}
                        Err(error) => {
                            warn!(
                                error = %error,
                                "control: capabilities refresh failed; continuing",
                            );
                        }
                    }
                }
                // BORU-CP-16: periodic extensions refresh (force=true so an
                // unchanged payload still reaches peers that joined late).
                if extensions_every > 0 && tick % extensions_every as u64 == 0 {
                    let extensions = local_extensions
                        .lock()
                        .expect("local extensions lock poisoned")
                        .clone();
                    match control_announce.announce_extensions(&extensions, true, false).await {
                        Ok(AnnounceOutcome::Announced) => {
                            info!("control: extensions refresh announced");
                        }
                        Ok(AnnounceOutcome::Throttled) => {
                            trace!("control: extensions refresh suppressed by throttle");
                        }
                        Ok(AnnounceOutcome::Unchanged) => {}
                        Ok(_) => {}
                        Err(error) => {
                            warn!(
                                error = %error,
                                "control: extensions refresh failed; continuing",
                            );
                        }
                    }
                }
            }
        }
    }
    debug!("discovery presence refresh loop exited");
}

// ---------------------------------------------------------------------------
// Room-directory TTL expiry (BORU-DIR-23, PDF Phase 8 test matrix)
// ---------------------------------------------------------------------------

/// Runtime-tunable room-directory expiry configuration shared between the
/// [`DiscoveryService`] builders and the sweep task (BORU-DIR-23).
#[derive(Debug, Clone, Copy)]
struct DirectoryExpiryConfig {
    /// How often the sweep runs to evict expired room advertisements.
    sweep_interval: Duration,
}

/// Background task that evicts expired room advertisements from the
/// bounded room-directory cache (BORU-DIR-23 / PDF Task 3.2 step 4).
///
/// Every `sweep_interval` it calls
/// [`RoomDirectory::evict_expired`](crate::room_directory::RoomDirectory::evict_expired),
/// which removes every cached room whose TTL elapsed since the last valid
/// refresh. This is the production wiring for the matrix scenario
/// "Advertiser disappears — Room becomes stale and expires after TTL":
/// without this sweep, expired rooms would only leave the cache as a side
/// effect of the *next* advertisement arriving (the receive path evicts
/// expired entries before inserting a new room). Refreshes arriving within
/// the TTL keep entries live — the sweep only removes genuinely stale
/// rooms, so temporary packet loss does not cause room flicker (PDF Task
/// 3.2 step 5).
///
/// The sweep interval is re-read from the shared config before every
/// sleep, so builder tuning (e.g. short intervals in tests) takes effect
/// immediately. Logs state transitions only, never message contents.
async fn directory_expiry_loop(
    config: Arc<Mutex<DirectoryExpiryConfig>>,
    room_directory: Arc<Mutex<RoomDirectory>>,
    cancel: CancellationToken,
) {
    loop {
        let sweep = config
            .lock()
            .expect("directory expiry config lock poisoned")
            .sweep_interval;

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("discovery directory expiry loop cancelled");
                break;
            }
            _ = tokio::time::sleep(sweep) => {
                let evicted = {
                    let mut dir = room_directory.lock().expect("room directory lock poisoned");
                    dir.evict_expired()
                };
                if !evicted.is_empty() {
                    info!(
                        count = evicted.len(),
                        "discovery: evicted room advertisements whose TTL expired",
                    );
                }
            }
        }
    }
    debug!("discovery directory expiry loop exited");
}

/// Random delay in `0..=jitter` (0 when `jitter` is zero, so tests get
/// deterministic timing). `rand::random` is cryptographically seeded; the
/// distribution shape does not matter here, only that nodes desynchronise.
fn random_jitter(jitter: Duration) -> Duration {
    if jitter.is_zero() {
        return Duration::ZERO;
    }
    let millis = jitter.as_millis().max(1) as u64;
    Duration::from_millis(rand::random::<u64>() % millis)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Command;
    use crate::control_plane::capabilities::{features, ids};
    use crate::control_plane::extensions::{PathPreference, RelayHealthHint};
    use crate::control_plane::message::{ControlMessageType, ControlPayload};
    use crate::proto::DeliveryScope;
    use irpc::channel::mpsc as irpc_mpsc;
    use std::collections::BTreeSet;

    /// Deterministic test identity: a `SecretKey` seeded from a single byte
    /// produces a valid Ed25519 public key.
    fn test_key(byte: u8) -> PublicKey {
        test_secret_key(byte).public()
    }

    /// Deterministic test secret key seeded from a single byte (matches
    /// [`test_key`]).
    fn test_secret_key(byte: u8) -> iroh_base::SecretKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed)
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
            None,
            counters,
            DirectoryCounters::new(),
        )
    }

    /// Build a running service with an ISOLATED **directory** counter set
    /// (BORU-DIR-22, PDF Phase 8 Task 8.1). Counter assertions read this
    /// instance directly, so they never race with other tests or live app
    /// traffic on the global [`DIRECTORY_COUNTERS`].
    fn test_service_with_directory_counters(
        local_node: PublicKey,
        directory_counters: DirectoryCounters,
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
            None,
            DiagnosticCounters::new(),
            directory_counters,
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
        assert_eq!(
            service.handle_incoming(&hello, peer),
            IncomingOutcome::Processed
        );
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
        assert_eq!(
            after[0].1.source,
            PeerSource::Hello,
            "source must not refresh"
        );
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
        assert_eq!(
            service.handle_incoming(&bytes, peer),
            IncomingOutcome::Processed
        );

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
        assert_eq!(
            service.handle_incoming(&first, peer),
            IncomingOutcome::Processed
        );
        assert_eq!(counters.discovery_peers_seen(), 1);

        let second = postcard::to_stdvec(&DiscoveryMessage::presence_with_event(peer, 42)).unwrap();
        assert_eq!(
            service.handle_incoming(&second, peer),
            IncomingOutcome::Duplicate
        );
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
        assert_eq!(
            service.handle_incoming(&hello, peer),
            IncomingOutcome::Processed
        );
        assert_eq!(counters.discovery_peers_seen(), 1);

        let presence = postcard::to_stdvec(&DiscoveryMessage::presence(peer)).unwrap();
        assert_eq!(
            service.handle_incoming(&presence, peer),
            IncomingOutcome::Processed
        );
        assert_eq!(
            counters.discovery_peers_seen(),
            1,
            "refresh is not a new peer"
        );
    }

    /// A self-originated message never bumps any counter.
    #[tokio::test]
    async fn counters_self_message_increments_nothing() {
        let local = test_key(0xAA);
        let counters = DiagnosticCounters::new();
        let service = test_service_with_counters(local, counters.clone());

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(local)).unwrap();
        assert_eq!(
            service.handle_incoming(&bytes, local),
            IncomingOutcome::SelfMessage
        );

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
        assert_eq!(
            service.handle_incoming(&first, peer),
            IncomingOutcome::Processed
        );
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

    /// BORU-CP-07: a same-id announcement from a peer that has gone
    /// Degraded / OfflineStale is a RESTART re-discovery, not a duplicate
    /// delivery. A restarted node reuses its event-id counter from 0, so
    /// its fresh HELLO collides with the pre-restart id; treating it as a
    /// duplicate would swallow the announcement that must trigger automatic
    /// reconnection. When the peer is lost, the same-id message refreshes
    /// the registry entry and emits `PeerUpdate::Seen`.
    #[tokio::test]
    async fn handle_incoming_restart_rediscovery_when_peer_lost() {
        use crate::control_plane::connectivity::ConnectivityEvent as CE;

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut updates = service.peer_updates();

        // First contact: same event id as a restart would reuse (0).
        let first = postcard::to_stdvec(&DiscoveryMessage::hello_with_event(peer, 0)).unwrap();
        assert_eq!(
            service.handle_incoming(&first, peer),
            IncomingOutcome::Processed
        );
        assert_eq!(service.peer_count(), 1);
        assert_eq!(
            updates.try_recv(),
            Ok(PeerUpdate::Seen {
                node_id: peer,
                source: PeerSource::Hello,
            })
        );

        // The peer goes down (a restart equivalent): Degraded, NOT online.
        service.report_connectivity_failure(peer, CE::EndpointFailed, "peer down".to_string());
        assert!(!service.connectivity_state(&peer).is_online());

        // The restarted peer re-announces with the SAME event id. This is a
        // re-discovery, not a duplicate: the entry refreshes, a Seen update
        // fires (the reconnect trigger), and the message is processed.
        let second = postcard::to_stdvec(&DiscoveryMessage::hello_with_event(peer, 0)).unwrap();
        assert_eq!(
            service.handle_incoming(&second, peer),
            IncomingOutcome::Processed,
            "same-id announcement from a lost peer must be a rediscovery"
        );
        assert_eq!(service.peer_count(), 1);
        assert_eq!(
            updates.try_recv(),
            Ok(PeerUpdate::Seen {
                node_id: peer,
                source: PeerSource::Hello,
            }),
            "restart rediscovery must emit a Seen update for the reconnect trigger"
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
        assert_eq!(
            service.handle_incoming(&hello, peer),
            IncomingOutcome::Processed
        );
        let first_seen = service.known_peers()[0].1.last_seen;

        // A new event id (presence refresh) updates the same single entry.
        let presence =
            postcard::to_stdvec(&DiscoveryMessage::presence_with_event(peer, 2)).unwrap();
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
        assert_eq!(
            service.handle_incoming(&hello, peer),
            IncomingOutcome::Processed
        );
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
        // (BORU-DISC-17). The counter is seeded RANDOMLY (BORU-CP-07) so a
        // restarted process never reuses the pre-restart id space — assert
        // the id is present and the node is ours, not an exact value.
        assert_eq!(decoded.node_id(), local);
        assert!(decoded.event_id().is_some(), "hello carries an event id");

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
        // The service stamps a per-node event id on its announcements
        // (BORU-DISC-17). The counter is seeded RANDOMLY (BORU-CP-07) so a
        // restarted process never reuses the pre-restart id space — assert
        // the id is present, not an exact value.
        assert_eq!(decoded.node_id(), local);
        assert!(decoded.event_id().is_some(), "presence carries an event id");
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
        // The event-id counter is seeded RANDOMLY (BORU-CP-07) so a
        // restarted process never reuses the pre-restart id space; capture
        // the actual first id and assert the second announcement is
        // monotonic +1.
        let first_id = decoded.event_id().expect("hello carries an event id");
        assert_eq!(decoded.node_id(), local);
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
        assert_eq!(
            decoded.event_id(),
            Some(first_id + 1),
            "announcement event ids must be monotonic within a process"
        );
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

    // ── control-plane announce (BORU-CP-04) ──────────────────────────

    /// `announce_control_hello` broadcasts a control-plane HELLO envelope
    /// (magic "BC") carrying the stable peer identity + minimum protocol
    /// metadata — never a chat message and never a legacy DiscoveryMessage.
    #[tokio::test]
    async fn announce_control_hello_broadcasts_control_envelope() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        assert_eq!(
            service.announce_control_hello().await.unwrap(),
            AnnounceOutcome::Announced
        );

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for control hello broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                assert_eq!(env.sender_node_id, local);
                assert_eq!(
                    env.message_type,
                    crate::control_plane::message::ControlMessageType::Hello
                );
                // BORU-CP-07: control sequences are seeded RANDOMLY so a
                // restarted process never reuses the pre-restart sequence
                // space (the gossip actor dedups byte-identical payloads).
                // The exact value is not asserted — only that a sequence is
                // present and the envelope carries the protocol version.
                assert_eq!(
                    env.protocol_version,
                    crate::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION
                );
                assert_eq!(
                    env.payload,
                    crate::control_plane::message::ControlPayload::Hello(
                        crate::control_plane::message::HelloPayload {
                            app_protocol_version: BORU_APP_PROTOCOL_VERSION,
                        }
                    )
                );
                assert!(
                    env.timestamp_secs > 0,
                    "announcements carry a real timestamp"
                );
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&bytes).is_err(),
            "a control envelope must never decode as a legacy DiscoveryMessage"
        );
    }

    /// `announce_control_presence` broadcasts a control-plane PRESENCE
    /// envelope suggesting our default presence TTL.
    #[tokio::test]
    async fn announce_control_presence_broadcasts_control_envelope() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        assert_eq!(
            service.announce_control_presence().await.unwrap(),
            AnnounceOutcome::Announced
        );

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for control presence broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                assert_eq!(
                    env.message_type,
                    crate::control_plane::message::ControlMessageType::Presence
                );
                assert_eq!(
                    env.payload,
                    crate::control_plane::message::ControlPayload::Presence(
                        crate::control_plane::message::PresencePayload {
                            ttl_secs: Some(DEFAULT_PRESENCE_TTL.as_secs() as u32),
                        }
                    )
                );
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    /// Control-plane sequences are per-sender monotonic: HELLO then
    /// PRESENCE carry consecutive ids (seeded RANDOMLY per process,
    /// BORU-CP-07, so a restarted process never reuses the pre-restart
    /// sequence space) — receivers dedup on this.
    #[tokio::test]
    async fn control_announce_sequences_are_monotonic() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_control_announce_min_interval(Duration::ZERO);

        assert_eq!(
            service.announce_control_hello().await.unwrap(),
            AnnounceOutcome::Announced
        );
        assert_eq!(
            service.announce_control_presence().await.unwrap(),
            AnnounceOutcome::Announced
        );

        let first = next_command(&mut cmd_rx).await;
        let Command::Broadcast(first_bytes) = first else {
            panic!("expected Broadcast, got {first:?}");
        };
        let second = next_command(&mut cmd_rx).await;
        let Command::Broadcast(second_bytes) = second else {
            panic!("expected Broadcast, got {second:?}");
        };
        let (ControlPlaneDecode::Message(e1), ControlPlaneDecode::Message(e2)) = (
            ControlEnvelope::decode(&first_bytes).unwrap(),
            ControlEnvelope::decode(&second_bytes).unwrap(),
        ) else {
            panic!("both broadcasts must be control envelopes");
        };
        assert_eq!(e2.sender_node_id, local);
        assert_eq!(
            e2.sequence,
            e1.sequence.wrapping_add(1),
            "control sequences must be strictly monotonic within a process"
        );
    }

    /// The control-plane announce throttle suppresses a rapid repeat — one
    /// broadcast, one throttled outcome, no sequence consumed.
    #[tokio::test]
    async fn announce_control_throttle_suppresses_rapid_repeat() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_control_announce_min_interval(Duration::from_millis(60));

        assert_eq!(
            service.announce_control_hello().await.unwrap(),
            AnnounceOutcome::Announced
        );
        assert_eq!(
            service.announce_control_hello().await.unwrap(),
            AnnounceOutcome::Throttled
        );

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for control hello")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        assert!(
            tokio::time::timeout(Duration::from_millis(40), cmd_rx.recv())
                .await
                .is_err(),
            "throttled control announcement must not broadcast"
        );

        // The throttle is separate from the legacy announce throttle — a
        // legacy hello right after is NOT suppressed by it.
        assert_eq!(
            service.announce_hello().await.unwrap(),
            AnnounceOutcome::Announced,
            "legacy and control announce throttles must be independent"
        );
    }

    /// The presence-refresh loop broadcasts a control-plane PRESENCE every
    /// `interval` (jitter zero in tests), keeping presence alive while
    /// running.
    #[tokio::test]
    async fn presence_refresh_loop_publishes_periodic_control_presence() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_presence_refresh_interval(Duration::from_millis(40))
            .with_presence_refresh_jitter(Duration::ZERO)
            .with_control_announce_min_interval(Duration::ZERO);

        // The first refresh tick fires after ~40 ms and announces PRESENCE.
        let command = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
            .await
            .expect("timed out waiting for presence refresh")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                assert_eq!(env.sender_node_id, local);
                assert_eq!(
                    env.message_type,
                    crate::control_plane::message::ControlMessageType::Presence,
                    "the refresh loop must announce PRESENCE, not a chat message"
                );
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }

        service.shutdown().await;
    }

    /// Shutting the service down stops the presence-refresh loop: no more
    /// broadcasts after `shutdown` returns.
    #[tokio::test]
    async fn presence_refresh_loop_stops_on_shutdown() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_presence_refresh_interval(Duration::from_millis(30))
            .with_presence_refresh_jitter(Duration::ZERO)
            .with_control_announce_min_interval(Duration::ZERO);

        // Consume the first tick, then stop the service.
        let _ = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
            .await
            .expect("timed out waiting for first presence refresh")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        service.shutdown().await;

        // After shutdown the loop is cancelled AND the service's gossip
        // sender is dropped, so recv() either times out (Err) or observes
        // the closed channel (Ok(None)) — either way, no broadcast.
        let result = tokio::time::timeout(Duration::from_millis(120), cmd_rx.recv()).await;
        assert!(
            !matches!(result, Ok(Ok(Some(_)))),
            "no presence refresh may be broadcast after shutdown"
        );
    }

    /// Our own control-plane announcement echo is ignored by the receive
    /// path — we never record ourselves in the presence store.
    #[tokio::test]
    async fn control_announce_own_echo_is_ignored() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        assert_eq!(
            service.announce_control_hello().await.unwrap(),
            AnnounceOutcome::Announced
        );
        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for control hello")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };

        // The gossip mesh echoes our own broadcast back; the receive path
        // must ignore it so we never register ourselves.
        assert_eq!(
            service.handle_incoming(&bytes, local),
            IncomingOutcome::SelfMessage
        );
        assert_eq!(service.control_presence_count(), 0);
        assert_eq!(service.peer_count(), 0);
    }

    /// A received control-plane HELLO records discovery_seen_at,
    /// protocol_version, and app_protocol_version in the in-memory
    /// peer-state cache (PDF Task 2.1 step 5), and presence is derived from
    /// activity (Active now, Stale past the TTL) — never persisted.
    #[tokio::test]
    async fn handle_incoming_control_hello_sets_discovery_seen_at_and_protocol() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        assert_eq!(
            service.handle_incoming(&control_hello(peer, 1), peer),
            IncomingOutcome::ControlMessage
        );
        let (node, state) = service.control_presence_peers().pop().unwrap();
        assert_eq!(node, peer);
        assert_eq!(
            state.protocol_version,
            crate::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION
        );
        assert_eq!(
            state.app_protocol_version,
            Some(BORU_APP_PROTOCOL_VERSION),
            "HELLO must record the peer's application protocol version"
        );
        assert!(
            state.discovery_seen_at <= state.last_seen,
            "discovery_seen_at is the first sighting and never later than last_seen"
        );
        assert_eq!(
            state.presence_state(Instant::now()),
            crate::control_plane::privacy::PresenceState::Active,
            "a freshly seen peer is Active"
        );
        assert!(
            state.discovery_seen_at.elapsed() < Duration::from_secs(5),
            "discovery_seen_at must be the moment the announcement was accepted"
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
        // It ALSO re-announces capabilities and extensions (BORU-CP-11/16),
        // so the peer learns the full control plane immediately (the
        // 09:54-09:55 FILES-v2 negotiation lag after a restart). The three
        // fire-and-forget broadcasts race; drain until all three arrive.
        let peer_endpoint: iroh_base::EndpointId = peer.into();
        ev_tx.send(Event::NeighborUp(peer_endpoint)).await.unwrap();

        let deadline = tokio::time::timeout(Duration::from_secs(5), async {
            let mut saw_hello = false;
            let mut saw_capabilities = false;
            let mut saw_extensions = false;
            while !(saw_hello && saw_capabilities && saw_extensions) {
                let command = cmd_rx
                    .recv()
                    .await
                    .expect("channel receive failed")
                    .expect("channel closed before broadcast");
                let Command::Broadcast(bytes) = command else {
                    panic!("expected Broadcast command, got {command:?}");
                };
                if bytes.starts_with(&CONTROL_PLANE_MAGIC) {
                    match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
                        ControlPlaneDecode::Message(env) => {
                            if env.sender_node_id != local {
                                continue;
                            }
                            match env.message_type {
                                crate::control_plane::message::ControlMessageType::Capabilities => {
                                    saw_capabilities = true;
                                }
                                crate::control_plane::message::ControlMessageType::Extensions => {
                                    saw_extensions = true;
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                let decoded: DiscoveryMessage = postcard::from_bytes(&bytes).unwrap();
                if decoded.node_id() == local && decoded.event_id().is_some() {
                    saw_hello = true;
                }
            }
        })
        .await;
        deadline.expect("timed out waiting for neighbor-up hello");
        // BORU-CP-07: the event-id counter is seeded RANDOMLY per process
        // so a restarted node never reuses the pre-restart id space. The
        // hello re-announcement is the drain-loop event we observed in
        // production; caps/extensions re-announcements are the new
        // behaviour (asserted above via the collected envelopes).
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
        let service =
            DiscoveryService::from_subscription(test_topic(), sender, receiver, local_node);
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
            other => panic!("expected Received(Hello), got {other:?}"),
        }

        // The peer registry is NOT touched by control-plane traffic.
        assert_eq!(
            service.peer_count(),
            0,
            "control plane must not register peers"
        );
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
        assert_eq!(
            service.handle_incoming(&bytes, peer),
            IncomingOutcome::ControlMessage
        );
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
        assert_eq!(
            service.handle_incoming(&bytes, local),
            IncomingOutcome::SelfMessage
        );
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
        assert_eq!(
            outcome,
            IncomingOutcome::UnknownControlType { message_type: 0x7F }
        );
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

    // ── BORU-DIR-01: PUBLIC_ROOM_ADVERTISEMENT decode path ─────────────

    /// A valid, bounded, discoverable room advertisement for tests.
    ///
    /// Unsigned by default — the receive path therefore surfaces it with
    /// [`AdvertisementAuth::MissingSignature`] (clearly untrusted). Tests
    /// that need a trusted advertisement call [`test_advert_signed`].
    fn test_advert() -> crate::control_plane::advertisement::PublicRoomAdvertisement {
        crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            "Test Room".into(),
            test_key(0x42).as_bytes().to_owned(),
        )
    }

    /// A valid advertisement signed by the publisher whose key equals its
    /// `owner_peer_id` (the room authority). Verifies as
    /// [`AdvertisementAuth::Verified`] for that publisher.
    fn test_advert_signed(
        publisher_byte: u8,
    ) -> crate::control_plane::advertisement::PublicRoomAdvertisement {
        let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            "Test Room".into(),
            test_key(publisher_byte).as_bytes().to_owned(),
        );
        advert.sign(&test_secret_key(publisher_byte));
        advert
    }

    /// A valid PUBLIC_ROOM_ADVERTISEMENT envelope is decoded **only inside
    /// the discovery/control-plane service** and surfaced as the dedicated
    /// `ControlEvent::RoomAdvertisement` event — never as a generic
    /// `Received` envelope, never into the peer registry, and never into
    /// chat handling.
    #[tokio::test]
    async fn handle_incoming_room_advertisement_emits_dedicated_event() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 7, 1_700_000_000, test_advert())
                .encode();
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        // The dedicated typed event carries the decoded advertisement.
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for room advertisement event")
            .expect("control event channel closed");
        match event {
            ControlEvent::RoomAdvertisement(ad) => {
                assert_eq!(ad.sender_node_id, peer);
                assert_eq!(ad.sequence, 7);
                assert_eq!(ad.timestamp_secs, 1_700_000_000);
                assert_eq!(ad.advert.advert_version, 1);
                // Unsigned test advertisement → clearly untrusted, but
                // still emitted (BORU-DIR-03).
                assert_eq!(
                    ad.auth,
                    AdvertisementAuth::MissingSignature,
                    "an unsigned advertisement is clearly untrusted"
                );
            }
            other => panic!("expected RoomAdvertisement, got {other:?}"),
        }

        // The peer registry is NOT touched by room advertisements either.
        assert_eq!(
            service.peer_count(),
            0,
            "room advertisements must not register peers"
        );
    }

    /// A malformed room advertisement (a PUBLIC_ROOM_ADVERTISEMENT header
    /// whose payload section is garbage) is rejected safely at the receive
    /// gate: `Undecodable`, no event, no panic, no registry change — chat
    /// and gossip processing are unaffected.
    #[tokio::test]
    async fn handle_incoming_malformed_room_advertisement_dropped() {
        let local = test_key(0xAA);
        let service = test_service(local);
        let mut events = service.control_events();

        // Magic + version + a header that claims PUBLIC_ROOM_ADVERTISEMENT
        // with a garbage payload section.
        let mut bytes = CONTROL_PLANE_MAGIC.to_vec();
        bytes.push(crate::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION);
        let header = postcard::to_stdvec(&crate::control_plane::message::WireHeader {
            message_type: ControlMessageType::PublicRoomAdvertisement.to_u8(),
            sender_node_id: *test_key(0xBB).as_bytes(),
            sequence: 1,
            timestamp_secs: 1_700_000_000,
            payload_len: 3,
        })
        .unwrap();
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // garbage payload

        let outcome = service.handle_incoming(&bytes, test_key(0xBB));
        assert_eq!(outcome, IncomingOutcome::Undecodable);
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "malformed room advertisement must not emit an event"
        );
    }

    /// Unknown future advertisement fields are ignored safely end-to-end:
    /// a newer sender appends metadata fields after the known payload; the
    /// older client decodes the known prefix, discards the trailing bytes,
    /// and emits the advertisement event unchanged.
    #[tokio::test]
    async fn handle_incoming_room_advertisement_unknown_fields_tolerated() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        // Encode the payload alone to compute its exact length, then build a
        // frame whose payload section claims 4 extra trailing bytes.
        let payload =
            postcard::to_stdvec(&ControlPayload::PublicRoomAdvertisement(test_advert())).unwrap();
        let mut bytes = CONTROL_PLANE_MAGIC.to_vec();
        bytes.push(crate::control_plane::message::CONTROL_PLANE_PROTOCOL_VERSION);
        let header = postcard::to_stdvec(&crate::control_plane::message::WireHeader {
            message_type: ControlMessageType::PublicRoomAdvertisement.to_u8(),
            sender_node_id: *peer.as_bytes(),
            sequence: 9,
            timestamp_secs: 1_700_000_000,
            payload_len: payload.len() as u32 + 4,
        })
        .unwrap();
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&[0x01, 0x00, 0x2A, 0x7F]); // future fields

        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(
            outcome,
            IncomingOutcome::ControlMessage,
            "advertisement with unknown future fields must be accepted"
        );
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for room advertisement event")
            .expect("control event channel closed");
        match event {
            ControlEvent::RoomAdvertisement(ad) => {
                assert_eq!(ad.sender_node_id, peer);
                assert_eq!(ad.sequence, 9);
                assert_eq!(
                    ad.advert.advert_version, 1,
                    "future fields must not change the decoded advertisement"
                );
            }
            other => panic!("expected RoomAdvertisement, got {other:?}"),
        }
        assert_eq!(service.peer_count(), 0);
    }

    // ── BORU-DIR-03: publisher signature verification ─────────────────

    /// A signed room advertisement from the claimed publisher is emitted
    /// with [`AdvertisementAuth::Verified`] — the advertisement can be
    /// attributed to its publisher before it enters the trusted directory
    /// view. When the publisher is the room authority (owner_peer_id), the
    /// advertisement is canonical-eligible.
    #[tokio::test]
    async fn handle_incoming_signed_room_advertisement_verified() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            "Test Room".into(),
            peer.as_bytes().to_owned(),
        );
        advert.sign(&test_secret_key(0xBB));

        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 11, 1_700_000_000, advert).encode();
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for room advertisement event")
            .expect("control event channel closed");
        match event {
            ControlEvent::RoomAdvertisement(ad) => {
                assert_eq!(
                    ad.auth,
                    AdvertisementAuth::Verified { publisher: peer },
                    "a valid publisher signature attributes the advertisement"
                );
                assert!(
                    ad.advert.is_authoritative_publisher(&peer),
                    "an owner-signed advertisement is canonical-eligible"
                );
            }
            other => panic!("expected RoomAdvertisement, got {other:?}"),
        }
    }

    /// A tampered advertisement (payload mutated after signing) is
    /// DISCARDED at the receive gate: [`IncomingOutcome::AdvertisementAuthRejected`],
    /// no event, no panic, no registry change.
    #[tokio::test]
    async fn handle_incoming_tampered_room_advertisement_rejected() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            "Test Room".into(),
            peer.as_bytes().to_owned(),
        );
        advert.sign(&test_secret_key(0xBB));
        // Tamper with the payload WITHOUT re-signing: the signature is now
        // stale, so verification must fail.
        advert.room_name = "Tampered Room".into();

        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 12, 1_700_000_000, advert).encode();
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(
            outcome,
            IncomingOutcome::AdvertisementAuthRejected,
            "a tampered advertisement must be discarded"
        );
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "tampered advertisement must not emit an event"
        );
    }

    /// An advertisement signed by a DIFFERENT key than the claimed
    /// publisher (the envelope sender) is a forgery: verification fails and
    /// the advertisement is discarded.
    #[tokio::test]
    async fn handle_incoming_wrong_publisher_room_advertisement_rejected() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB); // envelope sender = claimed publisher
        let service = test_service(local);
        let mut events = service.control_events();

        let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            "Test Room".into(),
            peer.as_bytes().to_owned(),
        );
        // The attacker signs, but the envelope claims publisher == peer.
        advert.sign(&test_secret_key(0xCC));

        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 13, 1_700_000_000, advert).encode();
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(
            outcome,
            IncomingOutcome::AdvertisementAuthRejected,
            "a wrong-publisher signature must be discarded"
        );
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "forged advertisement must not emit an event"
        );
    }

    /// An unsigned advertisement is emitted in a CLEARLY UNTRUSTED state
    /// ([`AdvertisementAuth::MissingSignature`]) — it may be listed as
    /// unverified but can never be canonical metadata.
    #[tokio::test]
    async fn handle_incoming_unsigned_room_advertisement_is_untrusted() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        // test_advert() is unsigned and claims owner == test_key(0x42).
        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 14, 1_700_000_000, test_advert())
                .encode();
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for room advertisement event")
            .expect("control event channel closed");
        match event {
            ControlEvent::RoomAdvertisement(ad) => {
                assert_eq!(ad.auth, AdvertisementAuth::MissingSignature);
                // owner_peer_id alone is descriptive: even though the
                // payload names an owner, the missing signature means the
                // publisher is NOT cryptographically proven to be the owner.
                assert!(
                    !ad.auth.is_verified(),
                    "an unsigned advertisement is never trusted"
                );
            }
            other => panic!("expected RoomAdvertisement, got {other:?}"),
        }
    }

    /// Failed advertisement verification does NOT crash or affect gossip
    /// processing: after a forged advertisement is rejected, a subsequent
    /// control-plane message still decodes and emits normally.
    #[tokio::test]
    async fn handle_incoming_advertisement_rejection_does_not_affect_gossip() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        // 1. Forged advertisement → rejected, no panic.
        let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            "Test Room".into(),
            peer.as_bytes().to_owned(),
        );
        advert.sign(&test_secret_key(0xCC)); // wrong publisher
        let bad = ControlEnvelope::public_room_advertisement(peer, 15, 1_700_000_000, advert)
            .encode();
        assert_eq!(
            service.handle_incoming(&bad, peer),
            IncomingOutcome::AdvertisementAuthRejected
        );

        // 2. A normal control-plane message (HELLO) from the same peer is
        // still processed — rejection of the bad advertisement did not
        // corrupt the receive path.
        let hello = ControlEnvelope::hello(peer, 16, 1_700_000_000, 1).encode();
        assert_eq!(
            service.handle_incoming(&hello, peer),
            IncomingOutcome::ControlMessage
        );
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for control event")
            .expect("control event channel closed");
        assert!(
            matches!(event, ControlEvent::Received(env) if env.message_type == ControlMessageType::Hello),
            "normal control processing must continue after an advertisement rejection"
        );
    }

    /// `announce_room_advertisement` broadcasts a PUBLIC_ROOM_ADVERTISEMENT
    /// control-plane envelope carrying the signed advertisement — never a
    /// chat message, never a legacy discovery message.
    #[tokio::test]
    async fn announce_room_advertisement_broadcasts_signed_envelope() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        // Owner == publisher == local node.
        let advert = test_advert_signed(0xAA);
        assert_eq!(
            service
                .announce_room_advertisement(advert.clone())
                .await
                .unwrap(),
            AnnounceOutcome::Announced
        );

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for room advertisement broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&bytes).is_err(),
            "a room advertisement must never decode as a legacy DiscoveryMessage"
        );
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                assert_eq!(env.sender_node_id, local);
                assert_eq!(
                    env.message_type,
                    ControlMessageType::PublicRoomAdvertisement
                );
                let ControlPayload::PublicRoomAdvertisement(payload) = &env.payload else {
                    panic!(
                        "expected PublicRoomAdvertisement payload, got {:?}",
                        env.payload
                    );
                };
                assert!(
                    payload.signature.is_some(),
                    "the announced advertisement must be signed"
                );
                assert_eq!(
                    payload.verify_signed(&local),
                    AdvertisementAuth::Verified { publisher: local },
                    "receivers can attribute the announced advertisement to this node"
                );
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    /// BORU-DIR-04 (PDF 2.1): the advertisement emit site refuses
    /// Private and PublicUnlisted rooms — only PublicDiscoverable rooms
    /// may emit a PUBLIC_ROOM_ADVERTISEMENT.
    #[tokio::test]
    async fn announce_room_advertisement_refuses_non_discoverable() {
        use crate::control_plane::advertisement::{PublicRoomAdvertisement, RoomVisibility};

        // Helper: a minimal advertisement with a chosen visibility.
        fn advert_with(visibility: RoomVisibility) -> PublicRoomAdvertisement {
            let mut advert = PublicRoomAdvertisement::minimal(
                crate::proto::state::TopicId::from_bytes([0x42; 32]),
                "Room".into(),
                test_key(0xAA).as_bytes().to_owned(),
            );
            advert.visibility = visibility;
            advert
        }

        for visibility in [RoomVisibility::Private, RoomVisibility::PublicUnlisted] {
            let local = test_key(0xAA);
            let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
            let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
            let sender = GossipSender::new(cmd_tx);
            let receiver = GossipReceiver::new(ev_rx);
            let service =
                DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

            let outcome = service
                .announce_room_advertisement(advert_with(visibility))
                .await
                .expect("guard returns ok outcome, not an error");
            assert_eq!(
                outcome,
                AnnounceOutcome::NotDiscoverable,
                "{visibility:?} rooms must never be advertised"
            );
            // Nothing was broadcast.
            assert!(
                tokio::time::timeout(Duration::from_millis(200), cmd_rx.recv())
                    .await
                    .is_err(),
                "{visibility:?} advertisement must not be emitted"
            );
        }

        // PublicDiscoverable still announces (the positive path is fully
        // covered by announce_room_advertisement_broadcasts_signed_envelope).
        let local = test_key(0xAA);
        let (cmd_tx, cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);
        assert_eq!(
            service
                .announce_room_advertisement(advert_with(RoomVisibility::PublicDiscoverable))
                .await
                .unwrap(),
            AnnounceOutcome::Announced
        );
        let _ = cmd_rx;
    }

    // ── BORU-DIR-09 (PDF Task 3.3): withdrawal / tombstone ────────────

    /// A valid withdrawal signed by the room authority is emitted as the
    /// dedicated `ControlEvent::RoomWithdrawal` event — the signal
    /// directory clients consume to remove the matching advertisement
    /// immediately.
    #[tokio::test]
    async fn handle_incoming_verified_room_withdrawal_emits_dedicated_event() {
        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let mut withdrawal =
            crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
                crate::proto::state::TopicId::from_bytes([0x41; 32]),
                owner.as_bytes().to_owned(),
            );
        withdrawal.sign(&test_secret_key(0xBB));

        let bytes = ControlEnvelope::public_room_withdrawal(owner, 21, 1_700_000_000, withdrawal)
            .encode();
        let outcome = service.handle_incoming(&bytes, owner);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for room withdrawal event")
            .expect("control event channel closed");
        match event {
            ControlEvent::RoomWithdrawal(w) => {
                assert_eq!(w.sender_node_id, owner);
                assert_eq!(w.sequence, 21);
                assert_eq!(w.timestamp_secs, 1_700_000_000);
                assert_eq!(
                    w.withdrawal.room_id,
                    crate::proto::state::TopicId::from_bytes([0x41; 32])
                );
                assert!(
                    w.withdrawal.signature.is_some(),
                    "the emitted withdrawal carries its signature"
                );
            }
            other => panic!("expected RoomWithdrawal, got {other:?}"),
        }

        // The peer registry is NOT touched by withdrawals either.
        assert_eq!(service.peer_count(), 0);
    }

    /// A tampered withdrawal (payload mutated after signing) is DISCARDED
    /// at the receive gate: [`IncomingOutcome::WithdrawalAuthRejected`], no
    /// event — it can never remove an advertisement.
    #[tokio::test]
    async fn handle_incoming_tampered_room_withdrawal_rejected() {
        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let mut withdrawal =
            crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
                crate::proto::state::TopicId::from_bytes([0x41; 32]),
                owner.as_bytes().to_owned(),
            );
        withdrawal.sign(&test_secret_key(0xBB));
        // Tamper WITHOUT re-signing: the signature is now stale.
        withdrawal.room_id = crate::proto::state::TopicId::from_bytes([0x99; 32]);

        let bytes = ControlEnvelope::public_room_withdrawal(owner, 22, 1_700_000_000, withdrawal)
            .encode();
        let outcome = service.handle_incoming(&bytes, owner);
        assert_eq!(
            outcome,
            IncomingOutcome::WithdrawalAuthRejected,
            "a tampered withdrawal must be discarded"
        );
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "tampered withdrawal must not emit an event"
        );
    }

    /// An unsigned withdrawal is untrusted and discarded — a withdrawal
    /// can never remove an advertisement without a valid signature.
    #[tokio::test]
    async fn handle_incoming_unsigned_room_withdrawal_rejected() {
        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let service = test_service(local);
        let mut events = service.control_events();

        let withdrawal = crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            owner.as_bytes().to_owned(),
        );

        let bytes =
            ControlEnvelope::public_room_withdrawal(owner, 23, 1_700_000_000, withdrawal).encode();
        let outcome = service.handle_incoming(&bytes, owner);
        assert_eq!(
            outcome,
            IncomingOutcome::WithdrawalAuthRejected,
            "an unsigned withdrawal must be discarded"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "unsigned withdrawal must not emit an event"
        );
    }

    /// A withdrawal signed by a NON-authority member verifies for its
    /// publisher but is NOT the room's designated authority — the
    /// verified-but-spoofed withdrawal is discarded and can never remove
    /// the room's advertisement (same authoritative identity rules as
    /// advertisements, BORU-DIR-03).
    #[tokio::test]
    async fn handle_incoming_non_authoritative_room_withdrawal_rejected() {
        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let member = test_key(0xCC); // a room member, not the authority
        let service = test_service(local);
        let mut events = service.control_events();

        let mut withdrawal =
            crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
                crate::proto::state::TopicId::from_bytes([0x41; 32]),
                owner.as_bytes().to_owned(),
            );
        // The MEMBER signs, but the room authority is `owner`.
        withdrawal.sign(&test_secret_key(0xCC));

        let bytes =
            ControlEnvelope::public_room_withdrawal(member, 24, 1_700_000_000, withdrawal).encode();
        let outcome = service.handle_incoming(&bytes, member);
        assert_eq!(
            outcome,
            IncomingOutcome::WithdrawalNotAuthoritative,
            "a verified-but-non-authority withdrawal must be discarded"
        );
        assert_eq!(service.peer_count(), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "non-authoritative withdrawal must not emit an event"
        );
    }

    /// `announce_room_withdrawal` broadcasts a PUBLIC_ROOM_WITHDRAWAL
    /// control-plane envelope carrying the signed withdrawal — never a
    /// chat message, never a legacy discovery message.
    #[tokio::test]
    async fn announce_room_withdrawal_broadcasts_signed_envelope() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        // Owner == publisher == local node.
        let mut withdrawal =
            crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
                crate::proto::state::TopicId::from_bytes([0x41; 32]),
                local.as_bytes().to_owned(),
            );
        withdrawal.sign(&test_secret_key(0xAA));
        assert_eq!(
            service
                .announce_room_withdrawal(withdrawal.clone())
                .await
                .unwrap(),
            AnnounceOutcome::Announced
        );

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for room withdrawal broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&bytes).is_err(),
            "a room withdrawal must never decode as a legacy DiscoveryMessage"
        );
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                assert_eq!(env.sender_node_id, local);
                assert_eq!(
                    env.message_type,
                    ControlMessageType::PublicRoomWithdrawal
                );
                let ControlPayload::PublicRoomWithdrawal(payload) = &env.payload else {
                    panic!(
                        "expected PublicRoomWithdrawal payload, got {:?}",
                        env.payload
                    );
                };
                assert!(
                    payload.signature.is_some(),
                    "the announced withdrawal must be signed"
                );
                assert_eq!(
                    payload.verify_signed(&local),
                    AdvertisementAuth::Verified { publisher: local },
                    "receivers can attribute the announced withdrawal to this node"
                );
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    // ── BORU-DIR-10 (PDF Phase 4, Task 4.1): bounded room directory ─────

    /// A decoded PUBLIC_ROOM_ADVERTISEMENT populates the bounded
    /// room-directory cache at the service boundary — keyed by stable
    /// room_id, carrying publisher + auth verdict + compatibility, with no
    /// peer-registry side effect.
    #[tokio::test]
    async fn handle_incoming_room_advertisement_populates_directory() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let mut advert = test_advert();
        advert.sign(&test_secret_key(0xBB)); // verified for `peer`
        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 7, 1_700_000_000, advert.clone())
                .encode();
        let outcome = service.handle_incoming(&bytes, peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        assert_eq!(guard.len(), 1, "advertisement cached");
        let entry = guard.get(&advert.room_id).expect("room cached by stable room_id");
        assert_eq!(entry.advert, advert, "latest valid advertisement stored");
        assert_eq!(entry.publisher, peer, "advertiser identity stored");
        assert_eq!(
            entry.auth,
            AdvertisementAuth::Verified { publisher: peer },
            "auth verdict stored"
        );
        assert_eq!(
            entry.compatibility,
            crate::room_directory::RoomCompatibility::Compatible
        );
        assert_eq!(
            entry.local_join_state,
            crate::room_directory::LocalJoinState::NotJoined,
            "local join state defaults to NotJoined"
        );
        drop(guard);

        // The peer registry is NOT touched by room advertisements.
        assert_eq!(service.peer_count(), 0);
    }

    /// Duplicate advertisements for the same room merge into ONE directory
    /// entry: an exact duplicate (same sender + sequence) is dropped by the
    /// control-plane guard, and a refresh (new sequence, same publisher)
    /// updates the existing entry without growing the cache.
    #[tokio::test]
    async fn handle_incoming_duplicate_advertisements_merge() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let mut advert = test_advert();
        advert.sign(&test_secret_key(0xBB));

        // First announcement.
        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, peer), IncomingOutcome::ControlMessage);
        // Exact duplicate (same sender + sequence) — guard dedup.
        assert_eq!(service.handle_incoming(&bytes, peer), IncomingOutcome::Duplicate);
        // Refresh (new sequence, same content) — merges into the one entry.
        let refresh =
            ControlEnvelope::public_room_advertisement(peer, 8, 1_700_000_060, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&refresh, peer), IncomingOutcome::ControlMessage);

        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        assert_eq!(guard.len(), 1, "duplicates merge into a single card");
        assert_eq!(guard.get(&advert.room_id).unwrap().sequence, 8);
        assert_eq!(service.peer_count(), 0);
    }

    /// Identical content re-advertised by a DIFFERENT publisher is a
    /// directory-level dedup (same room_id + advert version + content
    /// digest): it does NOT emit a second `ControlEvent::RoomAdvertisement`
    /// (PDF Task 4.2 — repeated gossip must not cause UI churn).
    #[tokio::test]
    async fn handle_incoming_identical_content_different_publisher_no_second_event() {
        let local = test_key(0xAA);
        let peer_a = test_key(0xBB);
        let peer_b = test_key(0xCC);
        let service = test_service(local);
        let mut events = service.control_events();

        let mut advert = test_advert();
        advert.sign(&test_secret_key(0xBB));

        // First publisher announces → Added → one event.
        let first =
            ControlEnvelope::public_room_advertisement(peer_a, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&first, peer_a), IncomingOutcome::ControlMessage);
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for first room advertisement event")
            .expect("control event channel closed");
        assert!(matches!(event, ControlEvent::RoomAdvertisement(_)));

        // A second publisher re-advertises byte-identical content (same
        // room_id + advert version + content digest) — directory dedup:
        // no second event, no UI churn.
        let mut same_content = test_advert();
        same_content.sign(&test_secret_key(0xCC));
        let second =
            ControlEnvelope::public_room_advertisement(peer_b, 1, 1_700_000_100, same_content)
                .encode();
        assert_eq!(
            service.handle_incoming(&second, peer_b),
            IncomingOutcome::ControlMessage
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.recv())
                .await
                .is_err(),
            "identical-content advertisement must not emit a UI event"
        );

        // Single directory entry, no conflict.
        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert!(!guard.get(&test_advert().room_id).unwrap().conflict);
    }

    /// A conflicting advertisement from a different non-authority source is
    /// applied deterministically: the directory retains a single entry with
    /// the deterministic winner flagged as conflicted, and no extra identity
    /// leaks into the normal event stream.
    #[tokio::test]
    async fn handle_incoming_conflicting_advertisements_merge_with_conflict_flag() {
        let local = test_key(0xAA);
        let peer_a = test_key(0xBB);
        let peer_b = test_key(0xCC);
        let service = test_service(local);

        let mut advert_a = test_advert();
        advert_a.sign(&test_secret_key(0xBB));
        let mut advert_b = test_advert();
        advert_b.room_name = "Other Name".into();
        advert_b.sign(&test_secret_key(0xCC));

        // Two DIFFERENT verified members advertise conflicting metadata.
        let first =
            ControlEnvelope::public_room_advertisement(peer_a, 7, 1_700_000_000, advert_a.clone())
                .encode();
        assert_eq!(service.handle_incoming(&first, peer_a), IncomingOutcome::ControlMessage);

        let second =
            ControlEnvelope::public_room_advertisement(peer_b, 9, 1_700_000_100, advert_b.clone())
                .encode();
        assert_eq!(service.handle_incoming(&second, peer_b), IncomingOutcome::ControlMessage);

        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        assert_eq!(guard.len(), 1, "conflicting ads still collapse to one entry");
        let entry = guard.get(&advert_a.room_id).expect("room cached");
        assert!(
            entry.conflict,
            "conflicting metadata with no canonical authority is flagged"
        );
        assert_eq!(
            entry.advert.room_name, "Other Name",
            "newer envelope is the deterministic winner"
        );
    }

    /// A verified, authoritative PUBLIC_ROOM_WITHDRAWAL removes the room
    /// from the bounded directory cache immediately (TTL remains the
    /// safety net if a withdrawal is missed).
    #[tokio::test]
    async fn handle_incoming_verified_withdrawal_removes_directory_entry() {
        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let service = test_service(local);

        // Advertisement signed by the room authority (owner == publisher).
        let advert = test_advert_signed(0xBB);
        let bytes =
            ControlEnvelope::public_room_advertisement(owner, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);
        assert_eq!(service.room_directory().lock().unwrap().len(), 1);

        // Withdrawal signed by the authority.
        let mut withdrawal = crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
            advert.room_id,
            owner.as_bytes().to_owned(),
        );
        withdrawal.sign(&test_secret_key(0xBB));
        let bytes =
            ControlEnvelope::public_room_withdrawal(owner, 21, 1_700_000_100, withdrawal).encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);

        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        assert!(guard.is_empty(), "withdrawal removed the directory entry");
        assert_eq!(service.peer_count(), 0);
    }

    /// A withdrawal signed by a NON-authority never reaches the directory
    /// (the receive gate discards it) — the cached entry survives.
    #[tokio::test]
    async fn handle_incoming_non_authoritative_withdrawal_keeps_directory_entry() {
        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let member = test_key(0xCC);
        let service = test_service(local);

        let advert = test_advert_signed(0xBB);
        let bytes =
            ControlEnvelope::public_room_advertisement(owner, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);
        assert_eq!(service.room_directory().lock().unwrap().len(), 1);

        // A member signs a withdrawal claiming the room's authority.
        let mut withdrawal = crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
            advert.room_id,
            owner.as_bytes().to_owned(),
        );
        withdrawal.sign(&test_secret_key(0xCC));
        let bytes =
            ControlEnvelope::public_room_withdrawal(member, 24, 1_700_000_100, withdrawal).encode();
        assert_eq!(
            service.handle_incoming(&bytes, member),
            IncomingOutcome::WithdrawalNotAuthoritative
        );

        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        assert_eq!(guard.len(), 1, "non-authoritative withdrawal cannot remove the room");
        assert_eq!(service.peer_count(), 0);
    }

    /// Discovering a room never subscribes to its gossip topic and never
    /// creates a conversation (PDF Core rule / Task 4.1 acceptance). The
    /// advertisement is decoded into the directory cache only: the
    /// service's own subscription stays exactly the discovery topic, the
    /// peer registry is untouched, and the cached entry is metadata-only
    /// (NotJoined, no conversation handle).
    #[tokio::test]
    async fn handle_incoming_room_advertisement_never_subscribes_or_creates_conversations() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let mut advert = test_advert();
        advert.sign(&test_secret_key(0xBB));
        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, peer), IncomingOutcome::ControlMessage);

        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        assert_eq!(guard.len(), 1, "room cached");
        // The room's gossip topic is NOT subscribed: the only topic the
        // service ever joins is the discovery topic.
        assert_eq!(service.topic(), test_topic());
        assert_ne!(
            service.topic(),
            advert.room_id,
            "the advertised room topic is never a service subscription"
        );
        // No peer-registry entry and no conversation record is created.
        assert_eq!(service.peer_count(), 0);
        // The cached entry is pure discovery metadata.
        assert_eq!(
            guard.get(&advert.room_id).unwrap().local_join_state,
            crate::room_directory::LocalJoinState::NotJoined
        );
    }

    // ── BORU-DIR-22 (PDF Phase 8 Task 8.1): directory diagnostics ─────

    /// A decoded, guard-admitted advertisement bumps `received`; a cache
    /// insert bumps `accepted`. The counters stay separate from the
    /// discovery-peer counters.
    #[tokio::test]
    async fn directory_counters_valid_advertisement_increments_received_and_accepted() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        let mut advert = test_advert();
        advert.sign(&test_secret_key(0xBB));
        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, peer), IncomingOutcome::ControlMessage);

        let snap = directory_counters.snapshot();
        assert_eq!(snap.advertisements_received, 1);
        assert_eq!(snap.advertisements_accepted, 1);
        assert_eq!(snap.advertisements_rejected, 0);
        assert_eq!(snap.advertisements_expired, 0);
        assert_eq!(snap.advertisements_withdrawn, 0);
        assert_eq!(snap.advertisements_deduplicated, 0);
        assert_eq!(snap.advertisements_rate_limited, 0);
    }

    /// An advertisement whose signature fails verification is counted as
    /// received AND rejected (the developer can tell the room was *seen*
    /// but never entered the cache).
    #[tokio::test]
    async fn directory_counters_invalid_signature_increments_rejected() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        // Sign with a DIFFERENT key than the envelope sender → invalid.
        let mut advert = test_advert();
        advert.sign(&test_secret_key(0xCC));
        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 7, 1_700_000_000, advert).encode();
        assert_eq!(
            service.handle_incoming(&bytes, peer),
            IncomingOutcome::AdvertisementAuthRejected
        );

        let snap = directory_counters.snapshot();
        assert_eq!(snap.advertisements_received, 1);
        assert_eq!(snap.advertisements_rejected, 1);
        assert_eq!(snap.advertisements_accepted, 0);
        assert_eq!(service.room_directory().lock().unwrap().len(), 0);
    }

    /// A repeated/identical advertisement (same content, different
    /// publisher) is deduplicated — `deduplicated` bumps, `accepted` does
    /// not (the entry already existed).
    #[tokio::test]
    async fn directory_counters_duplicate_increments_deduplicated() {
        let local = test_key(0xAA);
        let peer_a = test_key(0xBB);
        let peer_b = test_key(0xCC);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        let mut advert = test_advert();
        advert.sign(&test_secret_key(0xBB));
        let first =
            ControlEnvelope::public_room_advertisement(peer_a, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&first, peer_a), IncomingOutcome::ControlMessage);

        // Same content from a different publisher → directory-level dedup.
        let mut same = test_advert();
        same.sign(&test_secret_key(0xCC));
        let second =
            ControlEnvelope::public_room_advertisement(peer_b, 1, 1_700_000_100, same).encode();
        assert_eq!(service.handle_incoming(&second, peer_b), IncomingOutcome::ControlMessage);

        let snap = directory_counters.snapshot();
        assert_eq!(snap.advertisements_received, 2);
        assert_eq!(snap.advertisements_accepted, 1, "only the first insert is accepted");
        assert_eq!(snap.advertisements_deduplicated, 1);
        assert_eq!(service.room_directory().lock().unwrap().len(), 1);
    }

    /// A verified authoritative withdrawal that removes a listing bumps
    /// `withdrawn` (distinct from `expired`).
    #[tokio::test]
    async fn directory_counters_withdrawal_increments_withdrawn() {
        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        let mut advert = test_advert_signed(0xBB);
        let bytes =
            ControlEnvelope::public_room_advertisement(owner, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);
        assert_eq!(service.room_directory().lock().unwrap().len(), 1);

        let mut withdrawal = crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
            advert.room_id,
            owner.as_bytes().to_owned(),
        );
        withdrawal.sign(&test_secret_key(0xBB));
        let bytes =
            ControlEnvelope::public_room_withdrawal(owner, 8, 1_700_000_100, withdrawal).encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);
        assert_eq!(service.room_directory().lock().unwrap().len(), 0);

        let snap = directory_counters.snapshot();
        assert_eq!(snap.advertisements_withdrawn, 1);
        assert_eq!(snap.advertisements_expired, 0);
    }

    /// A non-authoritative withdrawal cannot remove the listing, so
    /// `withdrawn` does NOT bump.
    #[tokio::test]
    async fn directory_counters_non_authoritative_withdrawal_does_not_count() {
        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let member = test_key(0xCC);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        let mut advert = test_advert_signed(0xBB);
        let bytes =
            ControlEnvelope::public_room_advertisement(owner, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);

        let mut withdrawal = crate::control_plane::advertisement::PublicRoomWithdrawal::minimal(
            advert.room_id,
            owner.as_bytes().to_owned(),
        );
        withdrawal.sign(&test_secret_key(0xCC));
        let bytes =
            ControlEnvelope::public_room_withdrawal(member, 24, 1_700_000_100, withdrawal).encode();
        assert_eq!(
            service.handle_incoming(&bytes, member),
            IncomingOutcome::WithdrawalNotAuthoritative
        );

        let snap = directory_counters.snapshot();
        assert_eq!(snap.advertisements_withdrawn, 0);
        assert_eq!(service.room_directory().lock().unwrap().len(), 1);
    }

    /// Advertisements beyond the per-sender rate limit are counted as
    /// rate-limited (the developer can tell a flood was dropped, distinct
    /// from auth rejection).
    #[tokio::test]
    async fn directory_counters_rate_limited_increments_on_flood() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        // Fill the default 60-frame / 10s window with ad envelopes.
        let mut last_outcome = IncomingOutcome::SelfMessage;
        for seq in 0..60u64 {
            let mut advert = test_advert();
            advert.sign(&test_secret_key(0xBB));
            let bytes = ControlEnvelope::public_room_advertisement(
                peer,
                seq,
                1_700_000_000,
                advert,
            )
            .encode();
            last_outcome = service.handle_incoming(&bytes, peer);
        }
        assert_eq!(last_outcome, IncomingOutcome::ControlMessage);

        let mut extra = test_advert();
        extra.sign(&test_secret_key(0xBB));
        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 60, 1_700_000_000, extra).encode();
        assert_eq!(service.handle_incoming(&bytes, peer), IncomingOutcome::RateLimited);

        let snap = directory_counters.snapshot();
        assert_eq!(snap.advertisements_rate_limited, 1);
        assert_eq!(snap.advertisements_rejected, 0, "rate limit != auth rejection");
    }

    /// Expired advertisements are counted by the cache-side TTL sink wired
    /// from [`DirectoryCounters`] (the service passes its own instance).
    #[tokio::test]
    async fn directory_counters_expired_increments_via_cache_sink() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        // Minimum admissible TTL is 60s (guard policy); keep it valid so
        // the advertisement is cached, then evict past the TTL.
        let mut advert = test_advert();
        advert.expires_after_secs = 60;
        advert.sign(&test_secret_key(0xBB));
        let bytes =
            ControlEnvelope::public_room_advertisement(peer, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, peer), IncomingOutcome::ControlMessage);
        assert_eq!(service.room_directory().lock().unwrap().len(), 1);

        // The service's cache has the sink wired; evict past the TTL.
        let dir = service.room_directory();
        let mut dir = dir.lock().unwrap();
        let evicted = dir.evict_expired_at(std::time::Instant::now() + Duration::from_secs(61));
        assert_eq!(evicted, vec![advert.room_id]);
        drop(dir);

        let snap = directory_counters.snapshot();
        assert_eq!(snap.advertisements_expired, 1);
        assert_eq!(snap.advertisements_withdrawn, 0);
    }

    /// BORU-DIR-23 (PDF Phase 8 test matrix "Advertiser disappears"): the
    /// room-directory TTL sweep wired into the service evicts expired room
    /// advertisements on its periodic tick — rooms whose advertiser
    /// disappears leave the active directory naturally after their TTL,
    /// without waiting for the *next* advertisement to arrive.
    #[tokio::test]
    async fn directory_expiry_sweep_evicts_expired_entries() {
        let local = test_key(0xAA);
        let service = test_service(local)
            .with_directory_sweep_interval(Duration::from_millis(50));

        // A cached advertisement with a short TTL (applied directly to the
        // cache — the receive gate enforces a 60s minimum, but the cache
        // itself trusts its caller, which is exactly how the production
        // sweep sees entries).
        let room_id = crate::proto::state::TopicId::from_bytes([0x77; 32]);
        let owner = test_secret_key(0x42);
        let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            room_id,
            "Swept Room".into(),
            *owner.public().as_bytes(),
        );
        advert.expires_after_secs = 1;
        let outcome = service
            .room_directory()
            .lock()
            .unwrap()
            .apply_advertisement(
                advert,
                owner.public(),
                AdvertisementAuth::Verified {
                    publisher: owner.public(),
                },
                1,
                1_700_000_000,
            );
        assert_eq!(outcome, AdvertiseOutcome::Added);
        assert_eq!(service.room_directory().lock().unwrap().len(), 1);

        // The sweep removes the entry once its TTL elapses (well under the
        // test timeout).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if !service.room_directory().lock().unwrap().contains(&room_id) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the directory TTL sweep to evict the expired advertisement"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            service.room_directory().lock().unwrap().is_empty(),
            "the expired advertisement is gone from the active directory"
        );
    }

    /// The per-room diagnostics view surfaces last_seen, expiry,
    /// compatibility, auth status, and local membership state for every
    /// cached room (including hidden ones) using safe shortened ids — and
    /// contains NO chat contents, message bodies, or private room history.
    #[tokio::test]
    async fn directory_diagnostics_snapshot_is_metadata_only_with_short_ids() {
        use crate::room_directory::{LocalJoinState, RoomCompatibility};

        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        // Build + sign the advert in one step: modifying content AFTER
        // signing would invalidate the canonical-bytes signature.
        let mut advert = crate::control_plane::advertisement::PublicRoomAdvertisement::minimal(
            crate::proto::state::TopicId::from_bytes([0x41; 32]),
            "Test Room".into(),
            owner.as_bytes().to_owned(),
        );
        advert.short_description = "this is public metadata".into();
        advert.sign(&test_secret_key(0xBB));
        let bytes =
            ControlEnvelope::public_room_advertisement(owner, 7, 1_700_000_000, advert.clone())
                .encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);

        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        let rows = guard.diagnostics_snapshot();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        // Safe shortened identifier: prefix + ellipsis, never the full id.
        assert!(row.room_id_short.ends_with('…'));
        assert_ne!(row.room_id_short, advert.room_id.to_string());
        // Per-room diagnostics required by PDF Task 8.1 step 2.
        assert_eq!(row.compatibility, RoomCompatibility::Compatible);
        assert!(row.auth.is_verified());
        assert!(row.is_authority);
        assert_eq!(row.local_join_state, LocalJoinState::NotJoined);
        assert!(!row.conflict);
        // Recency/expiry are populated (>= 0 age, > 0 remaining TTL).
        assert!(row.expires_in_secs > 0);
        // Metadata-level only: no chat contents, no private history.
        let debug = format!("{rows:?}");
        assert!(!debug.contains("chat contents"));
        assert!(!debug.contains("private history"));
        assert!(!debug.contains("signature"));
    }

    /// A developer can tell whether a room was never advertised, rejected,
    /// expired, or simply failed to join (PDF Task 8.1 acceptance):
    /// * never advertised → absent from the diagnostics view;
    /// * rejected → the rejected counter is non-zero and it is not cached;
    /// * expired → the expired counter is non-zero after TTL eviction;
    /// * failed to join → still cached with `local_join_state` NotJoined.
    #[tokio::test]
    async fn directory_diagnostics_distinguishes_room_outcomes() {
        use crate::room_directory::LocalJoinState;

        let local = test_key(0xAA);
        let owner = test_key(0xBB);
        let directory_counters = DirectoryCounters::new();
        let service = test_service_with_directory_counters(local, directory_counters.clone());

        // A room advertised, accepted, then expired (min TTL 60s valid).
        let mut expired_advert = test_advert();
        expired_advert.expires_after_secs = 60;
        let expired_room_id = expired_advert.room_id;
        expired_advert.sign(&test_secret_key(0xBB));
        let bytes = ControlEnvelope::public_room_advertisement(
            owner,
            7,
            1_700_000_000,
            expired_advert,
        )
        .encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);
        // A room that failed verification (rejected).
        let mut bad = test_advert();
        bad.room_id = crate::proto::state::TopicId::from_bytes([0x66; 32]);
        let bad_room_id = bad.room_id;
        bad.sign(&test_secret_key(0xCC));
        let bytes =
            ControlEnvelope::public_room_advertisement(owner, 8, 1_700_000_000, bad).encode();
        assert_eq!(
            service.handle_incoming(&bytes, owner),
            IncomingOutcome::AdvertisementAuthRejected
        );

        // Still-cached room (never joined, still advertised).
        let mut live_advert = test_advert();
        live_advert.room_id = crate::proto::state::TopicId::from_bytes([0x77; 32]);
        let live_room_id = live_advert.room_id;
        live_advert.sign(&test_secret_key(0xBB));
        let bytes = ControlEnvelope::public_room_advertisement(
            owner,
            9,
            1_700_000_000,
            live_advert,
        )
        .encode();
        assert_eq!(service.handle_incoming(&bytes, owner), IncomingOutcome::ControlMessage);

        // Expire the first room.
        {
            let dir = service.room_directory();
            let mut dir = dir.lock().unwrap();
            dir.evict_expired_at(std::time::Instant::now() + Duration::from_secs(61));
        }

        let snap = directory_counters.snapshot();
        let dir = service.room_directory();
        let guard = dir.lock().unwrap();
        let rows = guard.diagnostics_snapshot();
        let short_ids: Vec<&str> = rows.iter().map(|r| r.room_id_short.as_str()).collect();

        // Expired room: was advertised+accepted, now gone, expired counter
        // incremented — distinguishable from "never advertised".
        assert!(snap.advertisements_expired >= 1);
        assert!(!short_ids
            .iter()
            .any(|s| *s == crate::room_directory::short_room_id(&expired_room_id)));
        // Rejected room: never entered the cache, rejected counter non-zero.
        assert!(snap.advertisements_rejected >= 1);
        assert!(!short_ids
            .iter()
            .any(|s| *s == crate::room_directory::short_room_id(&bad_room_id)));
        // Live room: still cached, NotJoined — "failed to join" (or never
        // attempted) is visible in the diagnostics view.
        assert!(short_ids
            .iter()
            .any(|s| *s == crate::room_directory::short_room_id(&live_room_id)));
        let live_row = rows
            .iter()
            .find(|r| r.room_id_short == crate::room_directory::short_room_id(&live_room_id))
            .expect("live room in diagnostics");
        assert_eq!(live_row.local_join_state, LocalJoinState::NotJoined);
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

    // ── Capability negotiation (BORU-CP-11 / PDF Task 4.2) ───────────────

    /// Encode a `Capabilities` control-plane envelope for `sender`.
    fn control_caps(sender: PublicKey, sequence: u64, caps: Vec<String>) -> Vec<u8> {
        ControlEnvelope::capabilities(sender, sequence, 1_700_000_000, caps).encode()
    }

    /// `announce_capabilities` broadcasts a CAPABILITIES control-plane
    /// envelope carrying the current local capability set — a control-plane
    /// message, never a chat message.
    #[tokio::test]
    async fn announce_capabilities_broadcasts_capabilities_envelope() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        assert_eq!(
            service.announce_capabilities().await.unwrap(),
            AnnounceOutcome::Announced
        );

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for capabilities broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        // Control-plane envelope, never a chat message or legacy discovery
        // message (capability changes do not require sending a chat message).
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&bytes).is_err(),
            "a capabilities envelope must never decode as a legacy DiscoveryMessage"
        );
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                assert_eq!(env.sender_node_id, local);
                assert_eq!(
                    env.message_type,
                    crate::control_plane::message::ControlMessageType::Capabilities
                );
                let crate::control_plane::message::ControlPayload::Capabilities(payload) =
                    &env.payload
                else {
                    panic!("expected Capabilities payload, got {:?}", env.payload);
                };
                // The wire ids equal the default local set's wire form.
                let local_set = service.local_capabilities();
                assert_eq!(payload.capabilities, local_set.to_wire());
                assert!(payload.capabilities.contains(&"files-v2".to_string()));
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    /// Re-announcing the SAME local capability set is an idempotent no-op:
    /// [`AnnounceOutcome::Unchanged`] and no second broadcast (BORU-CP-11
    /// idempotence — no duplicate advertisements for an unchanged set).
    #[tokio::test]
    async fn announce_capabilities_dedups_unchanged_set() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_capabilities_announce_min_interval(Duration::ZERO);

        assert_eq!(
            service.announce_capabilities().await.unwrap(),
            AnnounceOutcome::Announced
        );
        let first = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for first capabilities broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        assert!(matches!(first, Command::Broadcast(_)));

        // Same set again — no duplicate broadcast.
        assert_eq!(
            service.announce_capabilities().await.unwrap(),
            AnnounceOutcome::Unchanged
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(80), cmd_rx.recv())
                .await
                .is_err(),
            "unchanged capability set must not be re-broadcast"
        );
    }

    /// Replacing the local capability set broadcasts the NEW set (the
    /// "locally enabled capabilities materially change" path) without any
    /// chat message; `local_capabilities()` reflects the change.
    #[tokio::test]
    async fn update_local_capabilities_announces_material_change() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_capabilities_announce_min_interval(Duration::ZERO);

        // Shrink the local set to files-v2 only (e.g. a feature was
        // disabled) and announce it.
        let shrunk = CapabilitySet::from_wire(vec!["files-v2".to_string()]);
        assert_eq!(
            service
                .update_local_capabilities(shrunk.clone())
                .await
                .unwrap(),
            AnnounceOutcome::Announced
        );
        assert_eq!(service.local_capabilities(), shrunk);

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for updated capabilities broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                let crate::control_plane::message::ControlPayload::Capabilities(payload) =
                    &env.payload
                else {
                    panic!("expected Capabilities payload, got {:?}", env.payload);
                };
                assert_eq!(payload.capabilities, vec!["files-v2".to_string()]);
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }

        // Re-updating to the SAME set is a no-op.
        assert_eq!(
            service.update_local_capabilities(shrunk).await.unwrap(),
            AnnounceOutcome::Unchanged
        );
    }

    /// Alice can know whether Bob supports a feature before presenting or
    /// attempting it: a peer's CAPABILITIES advertisement is cached per
    /// peer, exposed as a typed set, and negotiated with the local set via
    /// `peer_supports` (fail closed = None).
    #[tokio::test]
    async fn peer_capabilities_and_peer_supports_query() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        // Unknown peer: no capability data at all.
        assert_eq!(service.peer_capabilities(&peer), None);
        assert_eq!(service.peer_supports(&peer, "files"), None);

        // Bob advertises files-v2 + a future feature.
        let bob_caps = vec!["files-v2".to_string(), "hologram-v3".to_string()];
        let outcome = service.handle_incoming(&control_caps(peer, 1, bob_caps.clone()), peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        // Alice now knows Bob's capabilities — including unknown futures
        // (forward compatibility: unknown ids preserved, never fatal).
        let cached = service
            .peer_capabilities(&peer)
            .expect("caps must be cached");
        assert!(cached.has_feature("files"));
        assert_eq!(cached.versions_of("files"), Some(&BTreeSet::from([2u16])));
        assert!(cached.to_wire().iter().any(|id| id == "hologram-v3"));

        // Negotiation: Bob supports files-v2 and the local client also
        // advertises files-v2 -> compatible version 2. A feature only Bob
        // (or only Alice) has is NOT negotiable.
        assert_eq!(service.peer_supports(&peer, "files"), Some(2));
        assert_eq!(service.peer_supports(&peer, "hologram"), None);
        assert_eq!(service.peer_supports(&peer, "voice"), None);
    }

    /// Stale capability data is not treated as current indefinitely: once
    /// the peer's presence expires past its TTL, `peer_capabilities`
    /// returns None (the capability cache dies with presence).
    #[tokio::test]
    async fn capabilities_expire_with_presence_ttl() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local)
            .with_presence_ttl(Duration::from_millis(60))
            .with_presence_sweep_interval(Duration::from_millis(20));

        let outcome =
            service.handle_incoming(&control_caps(peer, 1, vec!["files-v2".to_string()]), peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);
        assert!(
            service.peer_capabilities(&peer).is_some(),
            "capabilities must be current while presence is active"
        );

        // Wait well past the TTL (60ms) with several sweep ticks (20ms).
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert_eq!(
            service.peer_capabilities(&peer),
            None,
            "stale capability data must not be treated as current"
        );
        assert_eq!(service.peer_supports(&peer, "files"), None);
    }

    /// A CAPABILITIES advertisement is metadata only: it never registers
    /// the peer in the legacy registry and never grants authorisation (the
    /// peer is not a friend/group member/file recipient by virtue of
    /// advertising capabilities).
    #[tokio::test]
    async fn capabilities_advertisement_never_authorises() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let outcome = service.handle_incoming(
            &control_caps(peer, 1, vec!["files-v2".into(), "tunnels-v1".into()]),
            peer,
        );
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        // Control-plane traffic does not touch the peer registry.
        assert_eq!(
            service.peer_count(),
            0,
            "capabilities must not register peers"
        );
        // The capability cache is a hint store with no authorisation
        // surface: nothing here creates a friendship or a transfer.
        assert!(service.peer_capabilities(&peer).is_some());
    }

    /// BORU-CP-12: the read-only `CapabilityGate` handle (what the UI
    /// stores) mirrors the service's negotiated view — a compatible peer
    /// yields the shared version, an unknown/old peer fails closed to
    /// `None`, and the handle is object-safe (`Arc<dyn CapabilityGate>`).
    #[tokio::test]
    async fn capability_gate_handle_reflects_negotiated_support() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);
        let gate: Arc<dyn CapabilityGate> = service.capability_gate();

        // Unknown peer: nothing cached -> fail closed.
        assert_eq!(gate.peer_supports(&peer, features::VOICE), None);

        // Old client: present but advertises NO capabilities (no
        // CAPABILITIES envelope) -> fail closed, never attempt.
        let presence = ControlEnvelope::presence(peer, 1, 1_700_000_000, Some(300));
        let outcome = service.handle_incoming(&presence.encode(), peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);
        assert!(gate.peer_capabilities(&peer).is_some());
        assert_eq!(gate.peer_supports(&peer, features::VOICE), None);

        // New client: advertises voice-v1 + files-v2; local advertises both.
        let outcome = service.handle_incoming(
            &control_caps(
                peer,
                2,
                vec![
                    ids::VOICE_V1.to_string(),
                    ids::FILES_V2.to_string(),
                    "hologram-v3".to_string(),
                ],
            ),
            peer,
        );
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        // Compatible features negotiate to the shared version.
        assert_eq!(gate.peer_supports(&peer, features::VOICE), Some(1));
        assert_eq!(gate.peer_supports(&peer, features::FILES), Some(2));
        // Remote-only / unknown future features are not negotiable.
        assert_eq!(gate.peer_supports(&peer, "hologram"), None);
        assert_eq!(gate.peer_supports(&peer, features::SCREEN_SHARE), None);

        // The gate view equals the service view (single source of truth).
        assert_eq!(
            gate.peer_supports(&peer, features::VOICE),
            service.peer_supports(&peer, features::VOICE)
        );
        assert_eq!(gate.local_capabilities(), service.local_capabilities());
    }

    // ── Phase 6 extensions (BORU-CP-16 / PDF Phase 6) ─────────────────

    /// Encode an EXTENSIONS control-plane envelope for `sender`.
    fn control_extensions(sender: PublicKey, sequence: u64, payload: ExtensionsPayload) -> Vec<u8> {
        ControlEnvelope::extensions(sender, sequence, 1_700_000_000, payload).encode()
    }

    fn sample_extensions() -> ExtensionsPayload {
        ExtensionsPayload {
            group: Some(crate::control_plane::extensions::GroupHints { available: true }),
            file: Some(crate::control_plane::extensions::FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: true,
            }),
            tunnel: Some(crate::control_plane::extensions::TunnelCapability {
                protocol_versions: vec!["v1".into()],
            }),
            call: Some(crate::control_plane::extensions::CallCapability {
                protocol_versions: vec!["v1".into()],
                availability: Some(crate::control_plane::extensions::CallAvailability::Available),
            }),
            screen_share: Some(crate::control_plane::extensions::ScreenShareCapability {
                protocol_versions: vec!["v1".into()],
            }),
            identity: Some(crate::control_plane::extensions::MultiDeviceIdentity {
                identity_id: "user-alice".into(),
                device_id: "dev-phone".into(),
                active_device: true,
            }),
            path_preference: Some(PathPreference::DirectPreferred),
            relay_health: Some(RelayHealthHint::Healthy),
        }
    }

    /// `announce_extensions` broadcasts an EXTENSIONS control-plane envelope
    /// carrying the current local extensions payload — a control-plane
    /// message, never a chat message.
    #[tokio::test]
    async fn announce_extensions_broadcasts_extensions_envelope() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_extensions_announce_min_interval(Duration::ZERO);

        assert_eq!(
            service.announce_extensions().await.unwrap(),
            AnnounceOutcome::Announced
        );

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for extensions broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        // Control-plane envelope, never a chat message or legacy discovery
        // message.
        assert!(bytes.starts_with(&CONTROL_PLANE_MAGIC));
        assert!(
            postcard::from_bytes::<DiscoveryMessage>(&bytes).is_err(),
            "an extensions envelope must never decode as a legacy DiscoveryMessage"
        );
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                assert_eq!(env.sender_node_id, local);
                assert_eq!(
                    env.message_type,
                    crate::control_plane::message::ControlMessageType::Extensions
                );
                let crate::control_plane::message::ControlPayload::Extensions(payload) =
                    &env.payload
                else {
                    panic!("expected Extensions payload, got {:?}", env.payload);
                };
                // The wire payload equals the default local extensions.
                let local_payload = service.local_extensions();
                assert_eq!(payload, &local_payload);
                assert!(payload.file.is_some());
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    /// Re-announcing the SAME local extensions payload is an idempotent
    /// no-op: [`AnnounceOutcome::Unchanged`] and no second broadcast. An
    /// all-`None` payload is never broadcast (nothing to advertise).
    #[tokio::test]
    async fn announce_extensions_dedups_unchanged_and_empty() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_extensions_announce_min_interval(Duration::ZERO);

        assert_eq!(
            service.announce_extensions().await.unwrap(),
            AnnounceOutcome::Announced
        );
        let first = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for first extensions broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        assert!(matches!(first, Command::Broadcast(_)));

        // Same payload again — no duplicate broadcast.
        assert_eq!(
            service.announce_extensions().await.unwrap(),
            AnnounceOutcome::Unchanged
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(80), cmd_rx.recv())
                .await
                .is_err(),
            "unchanged extensions payload must not be re-broadcast"
        );

        // Replacing the local payload with an all-None payload is stored but
        // never broadcast (nothing to advertise).
        assert_eq!(
            service
                .update_local_extensions(ExtensionsPayload::default())
                .await
                .unwrap(),
            AnnounceOutcome::Unchanged
        );
        assert!(service.local_extensions().is_empty());
    }

    /// Replacing the local extensions payload broadcasts the NEW payload
    /// (the "locally derived extension metadata materially changes" path)
    /// without any chat message; `local_extensions()` reflects the change.
    #[tokio::test]
    async fn update_local_extensions_announces_material_change() {
        let local = test_key(0xAA);
        let (cmd_tx, mut cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (_ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local)
            .with_extensions_announce_min_interval(Duration::ZERO);

        let full = sample_extensions();
        assert_eq!(
            service.update_local_extensions(full.clone()).await.unwrap(),
            AnnounceOutcome::Announced
        );
        assert_eq!(service.local_extensions(), full);

        let command = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timed out waiting for extensions broadcast")
            .expect("channel receive failed")
            .expect("channel closed before broadcast");
        let Command::Broadcast(bytes) = command else {
            panic!("expected Broadcast command, got {command:?}");
        };
        match ControlEnvelope::decode(&bytes).expect("control envelope decodes") {
            ControlPlaneDecode::Message(env) => {
                let crate::control_plane::message::ControlPayload::Extensions(payload) =
                    &env.payload
                else {
                    panic!("expected Extensions payload, got {:?}", env.payload);
                };
                assert_eq!(payload, &full);
            }
            other => panic!("expected decoded envelope, got {other:?}"),
        }
    }

    /// `peer_extensions` reads the peer's cached Phase 6 extensions
    /// advertisement; unknown peers and peers that never advertised have
    /// none.
    #[tokio::test]
    async fn peer_extensions_reads_cached_advertisement() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        // Unknown peer: no extensions.
        assert_eq!(service.peer_extensions(&peer), None);

        // Bob advertises a full extensions payload.
        let full = sample_extensions();
        let outcome = service.handle_incoming(&control_extensions(peer, 1, full.clone()), peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);
        assert_eq!(service.peer_extensions(&peer), Some(full.clone()));

        // A newer extensions advertisement replaces the cached one.
        let updated = ExtensionsPayload {
            file: Some(crate::control_plane::extensions::FileReadiness {
                protocol_versions: vec!["v2".into()],
                can_receive: false,
            }),
            ..Default::default()
        };
        let outcome = service.handle_incoming(&control_extensions(peer, 2, updated.clone()), peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);
        assert_eq!(service.peer_extensions(&peer), Some(updated));
    }

    /// Stale extensions data is not treated as current: once the peer's
    /// presence expires past its TTL, `peer_extensions` returns None.
    #[tokio::test]
    async fn extensions_expire_with_presence_ttl() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local)
            .with_presence_ttl(Duration::from_millis(60))
            .with_presence_sweep_interval(Duration::from_millis(20));

        let outcome =
            service.handle_incoming(&control_extensions(peer, 1, sample_extensions()), peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);
        assert!(
            service.peer_extensions(&peer).is_some(),
            "extensions must be current while presence is active"
        );

        // Wait well past the TTL (60ms) with several sweep ticks (20ms).
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert_eq!(
            service.peer_extensions(&peer),
            None,
            "stale extensions data must not be treated as current"
        );
    }

    /// An EXTENSIONS advertisement is metadata only: it never registers the
    /// peer in the legacy registry and never grants authorisation (the peer
    /// is not a friend/group member/tunnel client/file recipient by virtue
    /// of advertising extensions).
    #[tokio::test]
    async fn extensions_advertisement_never_authorises() {
        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let outcome =
            service.handle_incoming(&control_extensions(peer, 1, sample_extensions()), peer);
        assert_eq!(outcome, IncomingOutcome::ControlMessage);

        // Control-plane traffic does not touch the peer registry.
        assert_eq!(
            service.peer_count(),
            0,
            "extensions must not register peers"
        );
        // The extensions cache is a hint store with no authorisation
        // surface: nothing here creates a friendship, group membership,
        // tunnel, or transfer.
        assert!(service.peer_extensions(&peer).is_some());
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
            other => panic!("expected Received, got {other:?}"),
        }
        assert_eq!(
            service.peer_count(),
            0,
            "control plane must not register peers"
        );
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

    // ── BORU-CP-05: peer connectivity state machine wiring ─────────────

    /// A legacy discovery message moves the peer to Discovered in the
    /// connectivity state machine — but NOT DirectTopicReady.
    #[tokio::test]
    async fn connectivity_legacy_discovery_marks_peer_discovered_not_ready() {
        use crate::control_plane::connectivity::PeerConnectivityState;

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        assert_eq!(
            service.handle_incoming(&bytes, peer),
            IncomingOutcome::Processed
        );

        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Discovered,
            "a peer seen on discovery must be Discovered, not DirectTopicReady"
        );
        assert!(!service.connectivity_state(&peer).is_ready_for_direct());
        assert!(!service.connectivity_state(&peer).is_online());

        // The deterministic transition trail records exactly one transition.
        let trail = service.connectivity_trail(&peer);
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0].event, ConnectivityEvent::DiscoverySeen);
        assert_eq!(trail[0].from, PeerConnectivityState::Unknown);
        assert_eq!(trail[0].to, PeerConnectivityState::Discovered);
    }

    /// A control-plane HELLO also feeds the state machine (Discovered),
    /// and duplicate announcements are idempotent no-ops.
    #[tokio::test]
    async fn connectivity_control_hello_marks_peer_discovered_idempotently() {
        use crate::control_plane::connectivity::PeerConnectivityState;

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        assert_eq!(
            service.handle_incoming(&control_hello(peer, 1), peer),
            IncomingOutcome::ControlMessage
        );
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Discovered
        );

        // A duplicate announcement (different sequence, same sender) is a
        // NoChange — the state machine never re-enters Connecting, so no
        // connection loop.
        for seq in 2..12u64 {
            assert_eq!(
                service.handle_incoming(&control_hello(peer, seq), peer),
                IncomingOutcome::ControlMessage
            );
            assert_eq!(
                service.connectivity_state(&peer),
                PeerConnectivityState::Discovered,
                "duplicate announcements must not advance or loop the state machine"
            );
        }
        assert_eq!(
            service.connectivity_trail(&peer).len(),
            1,
            "duplicate announcements must not append trail records"
        );
    }

    /// Reporting a failed direct-topic join makes the failure VISIBLE as
    /// Degraded with a recorded error — never reported simply as 'online'.
    #[tokio::test]
    async fn connectivity_failed_direct_topic_is_visible_not_online() {
        use crate::control_plane::connectivity::{ConnectivityEvent as CE, PeerConnectivityState};

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        // Discovered -> Reachable (endpoint connected).
        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        service.handle_incoming(&bytes, peer);
        service.report_connectivity_event(peer, CE::EndpointConnected);
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Reachable
        );
        assert!(service.connectivity_state(&peer).is_online());

        // Topic join fails -> Degraded, last_error recorded, NOT online.
        let outcome = service.report_connectivity_failure(
            peer,
            CE::TopicJoinFailed,
            "direct topic subscribe timed out".to_string(),
        );
        assert!(matches!(
            outcome,
            crate::control_plane::connectivity::TransitionOutcome::Transitioned { .. }
        ));
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Degraded
        );
        assert!(
            !service.connectivity_state(&peer).is_online(),
            "a failed direct-topic setup must never be reported as online"
        );
        let entry = service
            .connectivity_peers()
            .into_iter()
            .find(|(id, _)| *id == peer)
            .unwrap()
            .1;
        assert_eq!(
            entry.direct_topic_state,
            crate::control_plane::connectivity::DirectTopicState::Failed
        );
        assert_eq!(
            entry.last_error.as_deref(),
            Some("direct topic subscribe timed out")
        );
        assert_eq!(
            entry.path_kind,
            crate::control_plane::connectivity::PathKind::Unknown
        );
    }

    /// The expiry sweep moves a peer to OfflineStale in the connectivity
    /// state machine (the timeout event is a real networking event).
    #[tokio::test]
    async fn connectivity_expiry_sweep_marks_peer_offline_stale() {
        use crate::control_plane::connectivity::PeerConnectivityState;

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local)
            .with_presence_ttl(Duration::from_millis(60))
            .with_presence_sweep_interval(Duration::from_millis(20));

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        service.handle_incoming(&bytes, peer);
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Discovered
        );

        tokio::time::sleep(Duration::from_millis(150)).await;

        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::OfflineStale,
            "expiry sweep must feed the timeout event into the state machine"
        );
        let trail = service.connectivity_trail(&peer);
        assert_eq!(
            trail.last().unwrap().event,
            ConnectivityEvent::Timeout,
            "the transition trail must show the timeout"
        );
    }

    /// NeighborUp / NeighborDown events from the drain loop feed the state
    /// machine (endpoint connected / failed).
    #[tokio::test]
    async fn connectivity_drain_loop_neighbor_events_feed_state_machine() {
        use crate::control_plane::connectivity::PeerConnectivityState;

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (cmd_tx, _cmd_rx) = irpc_mpsc::channel::<Command>(64);
        let (ev_tx, ev_rx) = irpc_mpsc::channel::<Event>(64);
        let sender = GossipSender::new(cmd_tx);
        let receiver = GossipReceiver::new(ev_rx);
        let service = DiscoveryService::from_subscription(test_topic(), sender, receiver, local);

        // First discover the peer so the state machine has an entry.
        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        service.handle_incoming(&bytes, peer);

        // NeighborUp through the drain loop -> Reachable.
        ev_tx.send(Event::NeighborUp(peer)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Reachable
        );

        // NeighborDown through the drain loop -> Degraded (not 'online').
        ev_tx.send(Event::NeighborDown(peer)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Degraded
        );
        assert!(!service.connectivity_state(&peer).is_online());
    }

    /// A direct-message receive (reported by the data plane) advances the
    /// state machine to DirectTopicReady even if only Discovered — the
    /// message proves the direct topic works.
    #[tokio::test]
    async fn connectivity_direct_message_receive_reports_topic_ready() {
        use crate::control_plane::connectivity::{ConnectivityEvent as CE, PeerConnectivityState};

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        service.handle_incoming(&bytes, peer);
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Discovered
        );

        service.report_connectivity_event(peer, CE::DirectMessageReceived);
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::DirectTopicReady
        );
        assert!(service.connectivity_state(&peer).is_ready_for_direct());
        assert!(service.connectivity_state(&peer).is_online());
    }

    // ── BORU-CP-14 path classification ────────────────────────────────

    /// Any active IP path classifies Direct, even when a relay path is
    /// also open.
    #[test]
    fn classify_direct_when_any_active_ip_path() {
        use crate::control_plane::connectivity::PathKind;
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Relay, true), (PathAddrKind::Ip, true),]),
            PathKind::Direct
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, true), (PathAddrKind::Relay, true),]),
            PathKind::Direct
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, true)]),
            PathKind::Direct
        );
    }

    /// No active IP path but an active relay path classifies Relay — a
    /// relay connection is still considered reachable (BORU-CP-14).
    #[test]
    fn classify_relay_when_only_active_relay_path() {
        use crate::control_plane::connectivity::PathKind;
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, false), (PathAddrKind::Relay, true),]),
            PathKind::Relay
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Relay, true), (PathAddrKind::Other, true),]),
            PathKind::Relay,
            "custom transports never beat an active relay path"
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Relay, true)]),
            PathKind::Relay
        );
    }

    /// Known addresses but none active classify Transitioning (path in
    /// flux: connecting / re-negotiating).
    #[test]
    fn classify_transitioning_when_no_active_path() {
        use crate::control_plane::connectivity::PathKind;
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, false)]),
            PathKind::Transitioning
        );
        assert_eq!(
            classify_path_addrs([(PathAddrKind::Ip, false), (PathAddrKind::Relay, false),]),
            PathKind::Transitioning
        );
    }

    /// No addresses at all classify Unknown — report Unknown rather than
    /// guessing.
    #[test]
    fn classify_unknown_when_no_addresses() {
        use crate::control_plane::connectivity::PathKind;
        assert_eq!(
            classify_path_addrs([] as [(PathAddrKind, bool); 0]),
            PathKind::Unknown
        );
    }

    /// BORU-CP-14: a relay-only path recorded through the service keeps the
    /// peer reachable — the acceptance criterion at the service boundary.
    #[tokio::test]
    async fn service_path_relay_keeps_peer_reachable() {
        use crate::control_plane::connectivity::{ConnectivityEvent as CE, PeerConnectivityState};

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        service.handle_incoming(&bytes, peer);
        service.report_connectivity_event(peer, CE::EndpointConnected);
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Reachable
        );

        // Path becomes relay-only: state stays Reachable, path hint relays.
        service.report_connectivity_event(peer, CE::PathChangedRelay);
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Reachable
        );
        assert!(service.connectivity_state(&peer).is_online());
        let entry = service
            .connectivity_peers()
            .into_iter()
            .find(|(id, _)| *id == peer)
            .unwrap()
            .1;
        assert_eq!(
            entry.path_kind,
            crate::control_plane::connectivity::PathKind::Relay
        );
    }

    /// BORU-CP-14: `with_endpoint` attaches the path-refresh sweep; without
    /// it the service still works and path diagnostics stay unknown (the
    /// attach call is the feature switch).
    #[tokio::test]
    async fn with_endpoint_starts_path_refresh_and_shutdown_awaits_it() {
        let local = test_key(0xAA);
        let service = test_service(local);
        // Before attaching: no path task.
        assert!(service.path_task.is_none());

        // A real iroh endpoint (relay-disabled, never used for dialing) is
        // cheap to bind; the sweep polls it and the shutdown path awaits it.
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("bind test endpoint");
        let service = service.with_endpoint(endpoint);
        assert!(service.path_task.is_some());

        // Shutdown cancels and joins the sweep without hanging.
        service.shutdown().await;
    }

    /// BORU-CP-13: `peer_diagnostics()` exposes a share-safe per-peer
    /// snapshot covering every stage, and the timestamp-only events from
    /// the data plane show up in it.
    #[tokio::test]
    async fn peer_diagnostics_snapshot_covers_every_stage() {
        use crate::control_plane::connectivity::ConnectivityEvent as CE;

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        service.handle_incoming(&bytes, peer);
        service.report_connectivity_event(peer, CE::EndpointConnected);
        service.report_connectivity_event(peer, CE::TopicJoined);
        service.report_connectivity_event(peer, CE::DirectMessageSent);
        service.report_connectivity_event(peer, CE::InboundGossipEvent);
        service.report_connectivity_event(peer, CE::ApplicationMessageDecoded);

        let snapshots = service.peer_diagnostics();
        assert_eq!(snapshots.len(), 1, "one tracked peer");
        let snap = &snapshots[0];

        assert_eq!(snap.state, "direct-topic-ready");
        assert_eq!(snap.endpoint, "connected");
        assert_eq!(snap.topic_join_status, "ready");
        assert!(snap.subscription_ready);
        assert!(snap.discovery_last_seen_ms.is_some());
        assert!(
            snap.last_outbound_direct_ms.is_some(),
            "outbound broadcast recorded"
        );
        assert!(
            snap.last_inbound_gossip_ms.is_some(),
            "inbound gossip recorded"
        );
        assert!(
            snap.last_decoded_message_ms.is_some(),
            "decoded message recorded"
        );
        assert!(
            snap.direct_topic_id_prefix.is_some(),
            "direct-topic hash prefix present for debugging"
        );
        assert!(snap.last_error.is_none());
        assert!(snap.trail.len() >= 3);

        // Share-safety: the rendered snapshot never contains the full peer id.
        let full_hex = hex::encode(peer.as_bytes());
        assert!(!snap.render().contains(&full_hex));
    }

    // ── BORU-CP-07: reconnection triggered by discovery events ──────────

    /// `queue_reconnect` queues ONE attempt per peer (dedup), and never
    /// queues an already-online peer.
    #[tokio::test]
    async fn reconnect_queue_queues_once_and_skips_online() {
        use crate::control_plane::connectivity::ConnectivityEvent as CE;

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        // Fresh queue.
        assert!(service.queue_reconnect(peer));
        assert_eq!(
            service.reconnect_state(&peer),
            Some(ReconnectState {
                attempts: 0,
                in_flight: false,
            })
        );

        // Duplicate queue is a no-op — several discovery messages queue one
        // reconnection attempt.
        assert!(!service.queue_reconnect(peer));
        assert!(!service.queue_reconnect(peer));

        // An online peer is never queued (already Reachable).
        service.report_connectivity_event(peer, CE::EndpointConnected);
        assert!(
            !service.queue_reconnect(peer),
            "an online peer must not be queued for reconnection"
        );
    }

    /// The reconnect loop performs the queued attempt via the existing
    /// connection path (`join_peers`), emits `PeerReachable` on success,
    /// and clears the retry/backoff state.
    #[tokio::test]
    async fn reconnect_loop_dials_queued_peer_and_emits_signal() {
        use crate::control_plane::connectivity::PeerConnectivityState;

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (service, mut cmd_rx, ev_tx) = test_service_with_cmd(local);
        let mut signals = service.reconnect_events();

        assert!(service.queue_reconnect(peer));

        // The reconnect loop (1s tick) drains the queue and dials the peer
        // with the existing join_peers path.
        let command = next_command(&mut cmd_rx).await;
        let Command::JoinPeers(peers) = command else {
            panic!("expected JoinPeers reconnect command, got {command:?}");
        };
        let expected: iroh_base::EndpointId = peer.into();
        assert_eq!(peers, vec![expected]);
        // The mesh confirms the dial with a gossip NeighborUp (feeds
        // EndpointConnected → Reachable, resets the scheduler). Without this
        // the reconnect loop's confirmation poll times out and backs off.
        ev_tx
            .send(Event::NeighborUp(expected))
            .await
            .expect("send neighbor-up confirmation");

        // Only ONE attempt: the scheduler entry is cleared after success.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if service.reconnect_state(&peer).is_none() {
                break;
            }
            if Instant::now() >= deadline {
                panic!("reconnect state not cleared after successful dial");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // A real success signal was emitted for the data plane.
        let signal = tokio::time::timeout(Duration::from_secs(5), signals.recv())
            .await
            .expect("timed out waiting for reconnect signal")
            .expect("reconnect signal channel closed");
        assert_eq!(signal, ReconnectSignal::PeerReachable { peer });

        // The state machine reflects the real connection.
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Reachable
        );
        assert!(service.connectivity_state(&peer).is_online());
    }

    /// A real direct-topic readiness report (the data plane's
    /// `report_connectivity_event(TopicJoined)` path) clears queued
    /// retry/backoff state and advances the peer to DirectTopicReady.
    #[tokio::test]
    async fn reconnect_direct_topic_readiness_clears_backoff() {
        use crate::control_plane::connectivity::{ConnectivityEvent as CE, PeerConnectivityState};

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let service = test_service(local);

        assert!(service.queue_reconnect(peer));
        assert!(service.reconnect_state(&peer).is_some());

        service.report_connectivity_event(peer, CE::TopicJoined);

        assert!(
            service.reconnect_state(&peer).is_none(),
            "successful direct-topic readiness must clear retry/backoff state"
        );
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::DirectTopicReady
        );
        assert!(service.connectivity_state(&peer).is_ready_for_direct());
    }

    /// Discovery traffic ALONE is never treated as message-path recovery:
    /// a fresh announcement neither emits `PeerReachable` nor clears queued
    /// retry/backoff state. Only a real dial (the reconnect loop's
    /// `join_peers`) recovers the path.
    #[tokio::test]
    async fn reconnect_discovery_traffic_alone_never_recovers() {
        use crate::control_plane::connectivity::{ConnectivityEvent as CE, PeerConnectivityState};

        let local = test_key(0xAA);
        let peer = test_key(0xBB);
        let (service, mut cmd_rx, ev_tx) = test_service_with_cmd(local);
        let mut signals = service.reconnect_events();

        // Phase 0 — first contact: the connectivity loop performs its
        // once-per-lifetime dial (the harness join_peers always succeeds)
        // and the peer becomes Reachable.
        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        deliver(&ev_tx, peer, bytes).await;
        let command = next_command(&mut cmd_rx).await;
        assert!(matches!(command, Command::JoinPeers(_)));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if service.connectivity_state(&peer) == PeerConnectivityState::Reachable {
                break;
            }
            if Instant::now() >= deadline {
                panic!("peer never reached Reachable after first-contact dial");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Phase 1 — the peer goes down (a restart equivalent): Degraded,
        // explicitly NOT online.
        service.report_connectivity_failure(peer, CE::EndpointFailed, "peer down".to_string());
        assert!(!service.connectivity_state(&peer).is_online());

        // The app queues ONE reconnect attempt for its known friend.
        assert!(service.queue_reconnect(peer));
        assert!(service.reconnect_state(&peer).is_some());

        // Phase 2 — a fresh announcement arrives (the restarted peer
        // re-announces). Processing it must NOT by itself clear
        // retry/backoff state or emit a recovery signal: the peer was
        // already dialed once, so the connectivity loop does not re-dial,
        // and the receive path only feeds the state machine. (The
        // reconnect loop's next tick fires ~1s after service creation — the
        // assertions below complete well before it.)
        let bytes = postcard::to_stdvec(&DiscoveryMessage::hello(peer)).unwrap();
        deliver(&ev_tx, peer, bytes).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(
            service.reconnect_state(&peer).is_some(),
            "a fresh announcement alone must never clear retry/backoff state"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), signals.recv())
                .await
                .is_err(),
            "a fresh announcement alone must never produce a recovery signal"
        );

        // Phase 3 — the reconnect loop's REAL dial (the existing
        // authenticated connection path) recovers the path: it emits
        // PeerReachable and clears the queue.
        let command = next_command(&mut cmd_rx).await;
        assert!(matches!(command, Command::JoinPeers(_)));
        // The mesh confirms the dial with a gossip NeighborUp (feeds
        // EndpointConnected → Reachable, resets the scheduler). Without this
        // the reconnect loop's confirmation poll times out and backs off.
        let endpoint: iroh_base::EndpointId = peer.into();
        ev_tx
            .send(Event::NeighborUp(endpoint))
            .await
            .expect("send neighbor-up confirmation");
        let signal = tokio::time::timeout(Duration::from_secs(5), signals.recv())
            .await
            .expect("timed out waiting for reconnect signal")
            .expect("reconnect signal channel closed");
        assert_eq!(signal, ReconnectSignal::PeerReachable { peer });
        assert!(
            service.reconnect_state(&peer).is_none(),
            "a real successful dial must clear retry/backoff state"
        );
        assert_eq!(
            service.connectivity_state(&peer),
            PeerConnectivityState::Reachable
        );
    }
}
