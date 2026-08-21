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
//! # Facade shape (BORU-DISC-010)
//!
//! This module is now a **facade / coordinator**: it keeps the lifecycle
//! (join / start / stop / shutdown), the high-level subscription wiring (the
//! `drain_loop` that reads the gossip receiver and dispatches each frame), and
//! the module composition (construction of the shared handles and spawn of
//! each background task). The cohesive, owned-state concerns each live in a
//! dedicated `src/discovery/` module and are only *spawned / driven* here:
//!
//! * peer registry + `(node_id, event_id)` dedup — [`crate::discovery::peer_registry`];
//! * announcement / presence scheduling (throttles, announce handles,
//!   presence refresh/expiry loops) — [`crate::discovery::presence_scheduler`];
//! * capabilities / extensions advertisement — [`crate::discovery::caps_advertise`];
//! * room-directory lifecycle (cache, advert/withdrawal announce, TTL sweep) —
//!   [`crate::discovery::directory_lifecycle`];
//! * connectivity wiring (dial discovered peers into the mesh, BORU-DISC-11) +
//!   the deduplicated single dial — [`crate::discovery::connectivity`];
//! * per-peer path classification sweep (BORU-CP-14) —
//!   [`crate::discovery::path_refresh`];
//! * control-plane receive dispatch (decode → validate → emit) —
//!   [`crate::control_plane::dispatch`];
//! * reconnect scheduler + loop — [`crate::control_plane::reconnect`].
//!
//! The receive path is intentionally short: [`ReceiveCore::handle_incoming`]
//! sniffs the magic byte, routes `BC` control envelopes to the
//! [`ControlPlaneDispatcher`](crate::control_plane::dispatch::ControlPlaneDispatcher),
//! and legacy `DiscoveryMessage`s through the registry + connectivity store —
//! no scanning of hundreds of unrelated functions required. The final data
//! flow is documented in `docs/architecture-refactor/discovery-facade.md`.
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
    sync::{Arc, Mutex},
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
use crate::control_plane::advertisement::AdvertisementAuth;
use crate::control_plane::capabilities::{compatible_version, CapabilitySet};
use crate::control_plane::connectivity::{
    ConnectivityEvent, PathKind, PeerConnectivityState, PeerConnectivityStore,
};
use crate::control_plane::extensions::ExtensionsPayload;
use crate::control_plane::message::{ControlEnvelope, CONTROL_PLANE_MAGIC};
use crate::control_plane::privacy::{
    AdvertViolation, ControlPlaneGuard, DEFAULT_PRESENCE_TTL, EXPIRY_SWEEP_INTERVAL,
};
pub use crate::control_plane::reconnect::ReconnectSignal;
// The reconnect loop, backoff and confirmation-timeout integration
// (BORU-CP-07 / BORU-DISC-006) lives in its own focused module
// (src/control_plane/reconnect.rs). DiscoveryService re-exports the
// scheduling constants so the public path
// `boru_core::discovery_service::RECONNECT_LOOP_TICK` /
// `RECONNECT_CONFIRM_TIMEOUT` stays stable, spawns the loop, and
// delegates its reconnect_* facade to the shared scheduler handle.
use crate::control_plane::reconnect::{
    reconnect_loop, ReconnectHandle, ReconnectScheduler, ReconnectState,
};
pub use crate::control_plane::reconnect::{RECONNECT_CONFIRM_TIMEOUT, RECONNECT_LOOP_TICK};
// The control-plane receive dispatch (decode → validate → event emission,
// BORU-CP-02 / BORU-DISC-007) lives in its own focused, owned-state module
// (src/control_plane/dispatch.rs). DiscoveryService constructs a shared
// [`ControlPlaneDispatcher`] over the ReceiveCore's guard/connectivity/
// directory/event-channel handles and delegates every received control frame
// to it; the pure decode/validate/emit pipeline no longer lives here.
use crate::control_plane::dispatch::ControlPlaneDispatcher;
// The peer registry + `(node_id, event_id)` dedup logic lives in its own
// focused module (BORU-DISC-004). Re-exported here so the public path
// `boru_core::discovery_service::PeerRegistry` / `PeerSource` / `UpsertOutcome`
// / `PeerRegistryEntry` (used by integration tests, doctor.rs, main.rs) stays
// stable — DiscoveryService keeps only the `Arc<Mutex<PeerRegistry>>` handle.
pub use crate::discovery::peer_registry::{
    PeerRegistry, PeerRegistryEntry, PeerSource, UpsertOutcome,
};
// The capabilities/extensions advertisement (the local capability set +
// Phase 6 extensions payload and the update/announce + neighbour-up wiring,
// BORU-DISC-008) lives in its own focused guide: src/discovery/caps_advertise.rs.
// DiscoveryService keeps only a single `caps_advert: CapsAdvertiser` field
// and delegates its `*_capabilities` / `*_extensions` facades to it; the
// single mutable store is created+owned by that module and shared with the
// presence-refresh loop / drain loop via `Arc` clones (no duplicate mutable
// state). The public API and wire format are unchanged.
use crate::discovery::caps_advertise::CapsAdvertiser;
// The room-directory lifecycle (BORU-DISC-009) — the bounded room-directory
// cache, the outbound room advertisement / withdrawal announce paths, and the
// TTL expiry sweep — lives in its own focused module. DiscoveryService
// delegates its `announce_room_advertisement`, `announce_room_withdrawal`,
// `room_directory` and `with_directory_sweep_interval` facades to the shared
// [`RoomDirectoryLifecycle`] handle and re-exports the sweep-constant so the
// public path `boru_core::discovery_service::DEFAULT_DIRECTORY_SWEEP_INTERVAL`
// (used by docs) stays stable; it no longer owns the cache or the expiry
// loop config.
use crate::discovery::directory_lifecycle::RoomDirectoryLifecycle;
pub use crate::discovery::directory_lifecycle::DEFAULT_DIRECTORY_SWEEP_INTERVAL;
// The announcement/presence scheduling (announce throttles, legacy/control
// announce handles, presence refresh/expiry timers) lives in its own focused
// module (BORU-DISC-005). DiscoveryService imports the handles/loops/configs
// to delegate its `announce_*` facade and spawn the presence timers, and
// re-exports the public announce types + scheduling constants so the paths
// `boru_core::discovery_service::{AnnounceOutcome, AnnounceThrottle,
// DEFAULT_ANNOUNCE_MIN_INTERVAL, ...}` (used by integration tests) stay
// stable — DiscoveryService keeps only the handle/config Arc fields.
use crate::discovery::presence_scheduler::{
    presence_expiry_loop, presence_refresh_loop, AnnounceHandle, ControlAnnounceHandle,
    PresenceExpiryConfig, PresenceRefreshConfig,
};
// Connectivity wiring (drag discovered peers into the mesh, BORU-DISC-11)
// and the per-peer path classification sweep (BORU-CP-14) live in focused
// discovery modules. DiscoveryService only spawns the loops here.
use crate::diagnostics::{
    DiagnosticCounters, DirectoryCounters, DIAGNOSTIC_COUNTERS, DIRECTORY_COUNTERS,
};
use crate::discovery::connectivity::connectivity_loop;
use crate::discovery::path_refresh::path_refresh_loop;
pub use crate::discovery::presence_scheduler::{
    AnnounceOutcome, AnnounceThrottle, DEFAULT_ANNOUNCE_MIN_INTERVAL,
    DEFAULT_CAPABILITIES_REFRESH_EVERY, DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL,
    DEFAULT_EXTENSIONS_REFRESH_EVERY, DEFAULT_PRESENCE_REFRESH_INTERVAL,
    DEFAULT_PRESENCE_REFRESH_JITTER,
};
use crate::discovery_message::{check_discovery_version, DiscoveryMessage, DiscoveryVersionCheck};
use crate::proto::TopicId;
use crate::room_directory::{AdvertiseOutcome, RoomDirectory};

/// Capacity of the peer-update broadcast channel.
const PEER_UPDATES_CAPACITY: usize = 256;

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
    /// The BORU-CP-02 control-plane receive dispatcher (BORU-DISC-007):
    /// owns the decode → validate → event-emission pipeline for control
    /// envelopes received on the discovery topic. Constructed over the
    /// shared guard/connectivity/directory/event-channel handles above and
    /// invoked from [`ReceiveCore::handle_incoming`] for every `BC`-magic
    /// frame — the pure control-plane dispatch no longer lives here.
    dispatcher: ControlPlaneDispatcher,
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
            // BORU-DISC-007: control-envelope decode/validate/emit pipeline
            // lives in the focused control_plane::dispatch module.
            return self.dispatcher.handle_incoming(content, delivered_from);
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

/// A cloneable sink for feeding global-DHT bootstrap candidates (BORU-DHT-01)
/// into the discovery connectivity path.
///
/// Obtained from [`DiscoveryService::bootstrap_sink`]. Cheap to clone and safe
/// to hand to a background bootstrap loop. Each candidate is published as a
/// [`PeerUpdate::Advertised`] event, which the single deduplicated
/// connectivity loop (BORU-DISC-11) consumes and dials into the discovery mesh
/// via the existing `join_peers` path. Connectivity only — never friendship,
/// group membership, or a conversation.
#[derive(Clone, Debug)]
pub struct DiscoveryBootstrapSink {
    tx: tokio::sync::broadcast::Sender<PeerUpdate>,
    local_node: PublicKey,
}

impl DiscoveryBootstrapSink {
    /// Report validated bootstrap candidates for mesh dialing.
    ///
    /// The local node itself is never reported. A send error only means a
    /// subscriber stopped listening (the broadcast is lossy by design).
    pub fn report(&self, candidates: Vec<PublicKey>) {
        for peer in candidates {
            if peer == self.local_node {
                continue;
            }
            let _ = self.tx.send(PeerUpdate::Advertised {
                node_id: self.local_node,
                advertised: peer,
            });
        }
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
    /// Shared room-directory TTL-expiry configuration (sweep interval) so
    /// the builder can tune it after construction and the directory-expiry
    /// sweep observes it (BORU-DIR-23). Owned by the focused
    /// [`RoomDirectoryLifecycle`] module (BORU-DISC-009), which also owns the
    /// bounded room-directory cache and the expiry sweep task.
    directory_lifecycle: RoomDirectoryLifecycle,
    /// The capabilities/extensions advertisement (BORU-DISC-008): owns the
    /// local capability set ([`CapabilitySet`], BORU-CP-11 / PDF Task 4.2)
    /// and the local Phase 6 extensions payload ([`ExtensionsPayload`],
    /// BORU-CP-16 / PDF Phase 6), plus the update/announce logic and the
    /// neighbour-up re-announce wiring. DiscoveryService delegates its
    /// `local_capabilities` / `update_local_capabilities` /
    /// `announce_capabilities` / `local_extensions` /
    /// `update_local_extensions` / `announce_extensions` facades here. The
    /// single mutable store is created+owned by this module; the drain loop
    /// and the presence-refresh loop share it via `Arc` clones — no duplicate
    /// mutable state.
    caps_advert: CapsAdvertiser,
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
    /// Update the optional coarse metadata attached to local presence
    /// heartbeats. `None` is valid when GeoIP is unavailable.
    pub fn set_coarse_presence(
        &self,
        coarse: Option<crate::control_plane::message::CoarsePresence>,
    ) {
        self.control_announce.set_coarse_presence(coarse);
    }

    /// Return a cloneable sink for the endpoint address watcher. The sink only
    /// updates the next normal presence heartbeat and uses its existing throttle.
    pub fn coarse_presence_sink(
        &self,
    ) -> std::sync::Arc<dyn Fn(Option<crate::control_plane::message::CoarsePresence>) + Send + Sync>
    {
        let announce = self.control_announce.clone();
        std::sync::Arc::new(move |coarse| announce.set_coarse_presence(coarse))
    }

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
    /// `src/bin/boru/main.rs` — every Boru node joins the versioned
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
        // BORU-DISC-009: the room-directory lifecycle owns the bounded
        // local room-directory cache (BORU-DIR-10 / PDF Phase 4 Task 4.1),
        // the outbound room advertisement / withdrawal announce paths, and
        // the TTL expiry sweep (BORU-DIR-23). Built here, before the
        // receive core and the spawns, so the single cache instance is
        // shared via `Arc` clones with the receive dispatcher, the
        // capabilities advertiser, and the app read handle — no duplicate
        // mutable state.
        let announce = AnnounceHandle::new(sender.clone(), local_node);
        let control_announce = ControlAnnounceHandle::new(sender, local_node, local_secret);
        let directory_lifecycle =
            RoomDirectoryLifecycle::new(control_announce.clone(), directory_counters.clone());
        let room_directory = directory_lifecycle.room_directory();
        let core = ReceiveCore {
            local_node,
            topic,
            registry,
            peer_updates_tx,
            // BORU-DISC-007: the originating shared handles are moved into
            // the control-plane dispatcher below; the service keeps its own
            // clones of the same underlying state (Arcs / cheaply-cloneable
            // atomic counters / broadcast sender), so there is exactly one
            // mutable control-plane state — no duplication — while the
            // decode/validate/emit pipeline lives in control_plane::dispatch.
            control_events_tx: control_events_tx.clone(),
            guard: guard.clone(),
            connectivity: connectivity.clone(),
            reconnect,
            reconnect_tx,
            counters: counters.clone(),
            directory_counters: directory_counters.clone(),
            room_directory: room_directory.clone(),
            dispatcher: ControlPlaneDispatcher::new(
                local_node,
                guard,
                connectivity,
                room_directory,
                counters,
                directory_counters,
                control_events_tx,
            ),
        };
        let cancel = CancellationToken::new();
        let task_core = core.clone();
        let task_announce = announce.clone();
        let task_cancel = cancel.clone();
        // BORU-DISC-008: the capabilities/extensions advertisement — owns the
        // local capability set + extensions payload and the update/announce +
        // neighbour-up wiring. Shared with the drain loop (neighbour-up
        // re-announce) and the presence-refresh loop (periodic re-announce)
        // via `Arc` clones of the same stores — no duplicate mutable state.
        let caps_advert =
            CapsAdvertiser::new(control_announce.clone(), core.room_directory.clone());
        let task = tokio::spawn(drain_loop(
            receiver,
            task_core,
            task_announce,
            caps_advert.clone(),
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
        // Task 3.2 step 4; TTL remains the final cleanup mechanism). Owned
        // by the focused [`RoomDirectoryLifecycle`] module (BORU-DISC-009);
        // the sweep interval is re-read every cycle, so tests can tune it
        // after construction via `with_directory_sweep_interval`.
        let directory_expiry_cancel = cancel.clone();
        let directory_expiry_task = directory_lifecycle.spawn_expiry_loop(directory_expiry_cancel);
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
            caps_advert.caps_handle(),
            caps_advert.extensions_handle(),
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
            directory_lifecycle,
            caps_advert,
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
        self.caps_advert.local_capabilities()
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
        // BORU-DISC-008: the store + room-directory sync + announce now live
        // in discovery::caps_advertise (CapsAdvertiser).
        self.caps_advert.update_local_capabilities(caps).await
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
        self.caps_advert.announce_capabilities().await
    }

    // ── Phase 6 extensions (BORU-CP-16 / PDF Phase 6) ──────────────────

    /// The local Phase 6 extensions advertisement this node currently
    /// advertises.
    ///
    /// Defaults to `default_local_extensions` (every capability-backed
    /// extension section this build implements). The app replaces it via
    /// [`update_local_extensions`](Self::update_local_extensions) when the
    /// locally derived extension metadata materially changes.
    pub fn local_extensions(&self) -> ExtensionsPayload {
        self.caps_advert.local_extensions()
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
        self.caps_advert.update_local_extensions(payload).await
    }

    /// Broadcast the current local extensions advertisement (startup +
    /// material-change path, BORU-CP-16 / PDF Phase 6).
    ///
    /// Returns [`AnnounceOutcome::Unchanged`] when the payload is identical
    /// to the last announced one (no duplicate advertisement) or empty. The
    /// periodic refresh loop re-announces the payload on its own cadence so
    /// peers that joined after the previous announcement still learn it.
    pub async fn announce_extensions(&self) -> Result<AnnounceOutcome, DiscoveryServiceError> {
        self.caps_advert.announce_extensions().await
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
        self.directory_lifecycle
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
        self.directory_lifecycle
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
            local_caps: self.caps_advert.caps_handle(),
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
        self.directory_lifecycle.set_sweep_interval(interval);
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

    /// Return a deterministic, render-ready projection of active presence.
    ///
    /// This snapshot derives online counts, metadata statistics, and map
    /// points from the same control-plane presence store used by expiry. The
    /// caller supplies `now` so stale records are excluded even before the
    /// next periodic expiry sweep runs.
    pub fn network_map_state(&self, now: Instant) -> crate::network_map::NetworkMapState {
        let guard = self
            .core
            .guard
            .lock()
            .expect("control-plane guard poisoned");
        crate::network_map::NetworkMapState::from_presence(guard.presence(), now)
    }

    /// Return a read-only live projection callback for UI consumers.
    pub fn network_map_source(
        &self,
    ) -> Arc<dyn Fn(Instant) -> crate::network_map::NetworkMapState + Send + Sync> {
        let core = self.core.clone();
        Arc::new(move |now| {
            let guard = core.guard.lock().expect("control-plane guard poisoned");
            crate::network_map::NetworkMapState::from_presence(guard.presence(), now)
        })
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
        self.directory_lifecycle.room_directory()
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

    /// A cloneable sink for feeding global-DHT bootstrap candidates (BORU-DHT-01)
    /// into the discovery connectivity path.
    ///
    /// The returned handle can be given to a background bootstrap loop. Each
    /// candidate is published as a [`PeerUpdate::Advertised`] event that the
    /// single deduplicated connectivity loop dials into the discovery mesh.
    pub fn bootstrap_sink(&self) -> DiscoveryBootstrapSink {
        DiscoveryBootstrapSink {
            tx: self.core.peer_updates_tx.clone(),
            local_node: self.core.local_node,
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
    caps_advert: CapsAdvertiser,
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
                        // BORU-DISC-008: a freshly connected peer must learn the
                        // local capability set and extensions IMMEDIATELY, not on
                        // the next periodic refresh (up to ~6-9 minutes at the
                        // default cadence). The neighbour-up re-announce wiring
                        // lives in discovery::caps_advertise (CapsAdvertiser).
                        caps_advert.reannounce_on_neighbor_up(peer);
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "discovery_service_tests.rs"]
mod tests;
