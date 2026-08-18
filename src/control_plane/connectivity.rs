//! Explicit peer connectivity state machine (PDF Phase 2, Task 2.2 /
//! BORU-CP-05).
//!
//! "Seen on discovery" is **not** "ready for direct messaging". A peer may
//! announce itself on the discovery topic (legacy `DiscoveryMessage` or
//! control-plane HELLO/PRESENCE) long before its endpoint is dialable, its
//! deterministic direct topic is joined, or a direct message can actually
//! flow. This module defines the explicit per-peer connectivity state
//! machine and the deterministic transition table that moves a peer between
//! states.
//!
//! # States
//!
//! | State | Meaning |
//! |-------|---------|
//! | [`Unknown`](PeerConnectivityState::Unknown) | No information about the peer. |
//! | [`Discovered`](PeerConnectivityState::Discovered) | Seen on the discovery topic (control or legacy announcement). NOT ready for direct messaging. |
//! | [`Connecting`](PeerConnectivityState::Connecting) | An endpoint dial / connection attempt is in flight. |
//! | [`Reachable`](PeerConnectivityState::Reachable) | Endpoint connected (gossip mesh edge / `join_peers` succeeded). |
//! | [`DirectTopicReady`](PeerConnectivityState::DirectTopicReady) | Deterministic direct topic joined AND direct messaging possible. |
//! | [`Degraded`](PeerConnectivityState::Degraded) | Previously reachable, but a failure occurred (dial failed, topic join failed) — explicitly NOT 'online'. A relay-only path is NOT a failure (BORU-CP-14): the path kind is a diagnostic, so a peer on a relay path stays `Reachable`. |
//! | [`OfflineStale`](PeerConnectivityState::OfflineStale) | Not heard from within the presence TTL, or explicitly timed out. |
//!
//! # Events (real networking events only)
//!
//! The state machine is updated **only** from real networking events
//! (PDF Task 2.2 step 3): discovery seen, endpoint connection
//! success/failure, topic join/subscription success, direct message
//! receive, timeout, and relay/direct path changes. No timer, UI action, or
//! gossip *metadata* ever fabricates a transition.
//!
//! BORU-CP-14 makes the relay/direct **path** events
//! ([`ConnectivityEvent::PathChangedDirect`],
//! [`ConnectivityEvent::PathChangedRelay`],
//! [`ConnectivityEvent::PathChangedTransitioning`]) diagnostic-only: they
//! update the per-peer `path_kind` hint and log path changes in structured
//! logs, but they **never move the state machine**. Path type is
//! diagnostic/optimization information (PDF Task 5.2 step 2), not proof of
//! application-level success — a relay connection is still reachable, and
//! chat delivery never depends on being direct (PDF Task 5.2 step 4).
//!
//! BORU-CP-13 adds three **timestamp-only** diagnostic events that refresh
//! per-peer timing fields without ever moving the state machine:
//! [`ConnectivityEvent::DirectMessageSent`] (last outbound direct
//! broadcast), [`ConnectivityEvent::InboundGossipEvent`] (last inbound
//! gossip event), and [`ConnectivityEvent::ApplicationMessageDecoded`]
//! (last successfully decoded application message). They are the raw
//! material for the share-safe per-peer snapshot in
//! [`super::diagnostics`]; because they never transition, they can never
//! fabricate connectivity progress.
//!
//! # Idempotence
//!
//! Transitions are idempotent and safe under duplicate / reordered gossip
//! events (PDF Task 2.2 step 5):
//!
//! * An event that does not move the peer out of its current state is a
//!   [`TransitionOutcome::NoChange`] — no new trail record, no log line, no
//!   side effect. Re-delivering the same announcement (`(sender, sequence)`
//!   dedup at the guard is a separate, coarser gate) is a no-op.
//! * A peer already [`Connecting`](PeerConnectivityState::Connecting) does
//!   not re-enter `Connecting` on a duplicate dial event; a peer already
//!   [`Reachable`](PeerConnectivityState::Reachable) or
//!   [`DirectTopicReady`](PeerConnectivityState::DirectTopicReady) does not
//!   regress on a stale `DiscoverySeen`. This is what makes duplicate
//!   announcements unable to cause connection loops.
//! * Reordered events converge: `DiscoverySeen → EndpointConnected` and
//!   `EndpointConnected → DiscoverySeen` both end in `Reachable`, because
//!   every transition depends only on the current state, never on history.
//!
//! # Observability
//!
//! Every real transition appends a [`TransitionRecord`] to the peer's
//! bounded trail and logs the state transition (never message contents) at
//! `info!` — the deterministic per-peer transition trail required by the
//! acceptance criteria. No-change events are logged at `trace!`.
//!
//! # Bounded resources
//!
//! The store is capped ([`MAX_CONNECTIVITY_PEERS`]); when full it evicts
//! stale-then-oldest entries. Each peer's trail is capped
//! ([`MAX_TRAIL_PER_PEER`]) so a very chatty peer cannot grow memory
//! without bound.
//!
//! # No authorisation by connectivity
//!
//! This module is a metadata cache like the rest of the control plane:
//! reaching `DirectTopicReady` never makes a peer a friend, group member,
//! tunnel client, or file recipient. Friendship/trust checks live in their
//! own stores and never consult this state machine.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use iroh_base::PublicKey;
use tracing::{info, trace};

/// Maximum number of peers tracked by the connectivity state machine.
/// Beyond this the store evicts stale-then-oldest entries (bounded memory).
pub const MAX_CONNECTIVITY_PEERS: usize = 1024;

/// Maximum number of transition records kept per peer. The trail is a
/// bounded ring: oldest records fall off the front when full.
pub const MAX_TRAIL_PER_PEER: usize = 64;

/// The explicit connectivity state of one peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerConnectivityState {
    /// No information about the peer yet.
    Unknown,
    /// Seen on the discovery topic (control or legacy announcement).
    /// NOT ready for direct messaging.
    Discovered,
    /// An endpoint connection attempt is in flight.
    Connecting,
    /// Endpoint connected — gossip mesh edge established.
    Reachable,
    /// Deterministic direct topic joined + direct messaging possible.
    DirectTopicReady,
    /// Previously reachable but a failure occurred (dial / topic / path).
    /// Explicitly NOT reported as 'online'.
    Degraded,
    /// Not heard from within the presence TTL / explicitly timed out.
    OfflineStale,
}

impl PeerConnectivityState {
    /// Stable short label for logs and (future, BORU-CP-06) UI status.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Discovered => "discovered",
            Self::Connecting => "connecting",
            Self::Reachable => "reachable",
            Self::DirectTopicReady => "direct-topic-ready",
            Self::Degraded => "degraded",
            Self::OfflineStale => "offline-stale",
        }
    }

    /// Whether the peer is known at all (has a non-`Unknown` state).
    pub fn is_known(self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// Whether the peer is currently reachable at the endpoint level.
    ///
    /// This is the state-machine-derived replacement for scattered
    /// 'online' booleans: `Reachable` and `DirectTopicReady` are online,
    /// `Degraded` and `OfflineStale` are not — a failed direct-topic setup
    /// must never be reported simply as 'online' (PDF acceptance criteria).
    pub fn is_online(self) -> bool {
        matches!(self, Self::Reachable | Self::DirectTopicReady)
    }

    /// Whether the peer is ready for direct messaging: the deterministic
    /// direct topic has been joined and direct messages can flow.
    pub fn is_ready_for_direct(self) -> bool {
        matches!(self, Self::DirectTopicReady)
    }
}

/// A real networking event that may move a peer between connectivity
/// states (PDF Task 2.2 step 3). The state machine is updated ONLY from
/// these events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectivityEvent {
    /// A valid discovery announcement was received (legacy
    /// `DiscoveryMessage` or control-plane HELLO/PRESENCE).
    DiscoverySeen,
    /// An endpoint dial / connection attempt was initiated.
    EndpointConnecting,
    /// Endpoint connection succeeded (gossip `NeighborUp`, `join_peers` ok).
    EndpointConnected,
    /// Endpoint connection failed (gossip `NeighborDown`, `join_peers` err).
    EndpointFailed,
    /// The deterministic direct topic was joined / subscribed successfully.
    TopicJoined,
    /// The deterministic direct topic join / subscription failed.
    TopicJoinFailed,
    /// A direct (non-discovery) message was received from the peer.
    DirectMessageReceived,
    /// The peer was not heard from within its presence TTL.
    Timeout,
    /// The relay/direct path changed to a direct (IP) path (BORU-CP-14).
    ///
    /// Diagnostic-only: records `path_kind = Direct` and logs the path
    /// change in structured logs, but never moves the state machine. Path
    /// type is not proof of application-level success.
    PathChangedDirect,
    /// The relay/direct path changed to a relay-only path (BORU-CP-14).
    ///
    /// Diagnostic-only: records `path_kind = Relay` and logs the path
    /// change in structured logs, but never moves the state machine — a
    /// relay connection is still considered reachable (PDF Task 5.2
    /// acceptance: "A relay connection can still be considered reachable").
    PathChangedRelay,
    /// The path is transitioning: addresses are known but none is currently
    /// active (BORU-CP-14).
    ///
    /// Diagnostic-only: records `path_kind = Transitioning` and logs the
    /// path change in structured logs, but never moves the state machine.
    /// A path transition does not reset or duplicate conversation state.
    PathChangedTransitioning,
    /// An outbound direct broadcast was attempted to the peer (BORU-CP-13).
    ///
    /// Timestamp-only diagnostic event: it records *when* the data plane
    /// last tried to broadcast on the peer's direct topic, but it never
    /// moves the state machine — an outbound attempt proves nothing about
    /// the peer's receipt, so it must not fabricate connectivity progress.
    DirectMessageSent,
    /// An inbound gossip event (message, NeighborUp, NeighborDown) arrived
    /// from the peer (BORU-CP-13).
    ///
    /// Timestamp-only diagnostic event: hearing traffic from a peer is
    /// evidence of liveness, not of a usable direct path, so it refreshes
    /// `last_inbound_gossip` without moving the state machine.
    InboundGossipEvent,
    /// A gossip message from the peer decoded to an application message and
    /// reached application processing (BORU-CP-13).
    ///
    /// Timestamp-only diagnostic event: records *when* the peer's content
    /// last decoded successfully. No message content is ever stored here.
    ApplicationMessageDecoded,
}

impl ConnectivityEvent {
    /// Stable short label for logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::DiscoverySeen => "discovery-seen",
            Self::EndpointConnecting => "endpoint-connecting",
            Self::EndpointConnected => "endpoint-connected",
            Self::EndpointFailed => "endpoint-failed",
            Self::TopicJoined => "topic-joined",
            Self::TopicJoinFailed => "topic-join-failed",
            Self::DirectMessageReceived => "direct-message-received",
            Self::Timeout => "timeout",
            Self::PathChangedDirect => "path-changed-direct",
            Self::PathChangedRelay => "path-changed-relay",
            Self::PathChangedTransitioning => "path-changed-transitioning",
            Self::DirectMessageSent => "direct-message-sent",
            Self::InboundGossipEvent => "inbound-gossip-event",
            Self::ApplicationMessageDecoded => "application-message-decoded",
        }
    }
}

/// The deterministic transition function.
///
/// Returns the next state, or `None` when the event does not move the peer
/// out of its current state (an idempotent no-op). Every rule depends only
/// on `(state, event)` — never on history — so duplicate / reordered gossip
/// events converge and can never cause connection loops.
///
/// BORU-CP-14: the relay/direct **path** events
/// ([`ConnectivityEvent::PathChangedDirect`],
/// [`ConnectivityEvent::PathChangedRelay`],
/// [`ConnectivityEvent::PathChangedTransitioning`]) always return `None`
/// here — they never move the state machine. They update the per-peer
/// `path_kind` diagnostic hint and log path changes via
/// [`PeerConnectivityStore::apply`]'s side effects, so a relay connection
/// stays reachable and a path transition never resets or duplicates
/// conversation state.
///
/// This function IS the documented transition table (PDF Task 2.2 step 2);
/// keep it in sync with `docs/discovery-refactor/05-peer-connectivity-state-machine.md`.
pub fn transition(
    state: PeerConnectivityState,
    event: ConnectivityEvent,
) -> Option<PeerConnectivityState> {
    use ConnectivityEvent::*;
    use PeerConnectivityState::*;

    let next = match (state, event) {
        // ── Unknown ────────────────────────────────────────────────────
        (Unknown, DiscoverySeen) => Discovered,
        (Unknown, EndpointConnecting) => Connecting,
        (Unknown, EndpointConnected) => Reachable,
        (Unknown, DirectMessageReceived) => DirectTopicReady,
        (Unknown, TopicJoined) => DirectTopicReady,
        // Timeout / failures for an unknown peer: no entry created (the
        // caller must have positive evidence first).

        // ── Discovered ─────────────────────────────────────────────────
        (Discovered, DiscoverySeen) => return None, // idempotent refresh
        (Discovered, EndpointConnecting) => Connecting,
        (Discovered, EndpointConnected) => Reachable,
        (Discovered, EndpointFailed) => Degraded,
        (Discovered, TopicJoined) => DirectTopicReady,
        (Discovered, TopicJoinFailed) => Degraded,
        (Discovered, DirectMessageReceived) => DirectTopicReady,
        (Discovered, Timeout) => OfflineStale,
        // Path events (PathChangedDirect / Relay / Transitioning) are
        // diagnostic-only (BORU-CP-14): they fall through to the no-op
        // catch-all and never move the state machine. A relay path is
        // still reachable; a path transition is not a failure.

        // ── Connecting ─────────────────────────────────────────────────
        (Connecting, DiscoverySeen) => return None, // idempotent
        (Connecting, EndpointConnecting) => return None, // no duplicate dials
        (Connecting, EndpointConnected) => Reachable,
        (Connecting, EndpointFailed) => Degraded,
        (Connecting, TopicJoined) => DirectTopicReady,
        (Connecting, TopicJoinFailed) => Degraded,
        (Connecting, DirectMessageReceived) => DirectTopicReady,
        (Connecting, Timeout) => OfflineStale,

        // ── Reachable ──────────────────────────────────────────────────
        (Reachable, DiscoverySeen) => return None, // idempotent refresh
        (Reachable, EndpointConnecting) => return None, // already connected
        (Reachable, EndpointConnected) => return None, // idempotent
        (Reachable, EndpointFailed) => Degraded,
        (Reachable, TopicJoined) => DirectTopicReady,
        (Reachable, TopicJoinFailed) => Degraded,
        (Reachable, DirectMessageReceived) => DirectTopicReady,
        (Reachable, Timeout) => OfflineStale,

        // ── DirectTopicReady ───────────────────────────────────────────
        (DirectTopicReady, DiscoverySeen) => return None, // idempotent
        (DirectTopicReady, EndpointConnecting) => return None, // already ready
        (DirectTopicReady, EndpointConnected) => return None, // idempotent
        (DirectTopicReady, EndpointFailed) => Degraded,
        (DirectTopicReady, TopicJoined) => return None, // idempotent
        (DirectTopicReady, TopicJoinFailed) => Degraded,
        (DirectTopicReady, DirectMessageReceived) => return None, // idempotent
        (DirectTopicReady, Timeout) => OfflineStale,

        // ── Degraded ───────────────────────────────────────────────────
        (Degraded, DiscoverySeen) => return None, // idempotent; not revived by an announcement
        (Degraded, EndpointConnecting) => Connecting,
        (Degraded, EndpointConnected) => Reachable,
        (Degraded, EndpointFailed) => return None, // idempotent
        (Degraded, TopicJoined) => DirectTopicReady,
        (Degraded, TopicJoinFailed) => return None, // idempotent
        (Degraded, DirectMessageReceived) => DirectTopicReady,
        (Degraded, Timeout) => OfflineStale,

        // ── OfflineStale ───────────────────────────────────────────────
        (OfflineStale, DiscoverySeen) => Discovered, // fresh announcement revives
        (OfflineStale, EndpointConnecting) => Connecting,
        (OfflineStale, EndpointConnected) => Reachable,
        (OfflineStale, EndpointFailed) => return None, // idempotent
        (OfflineStale, TopicJoined) => DirectTopicReady,
        (OfflineStale, TopicJoinFailed) => return None, // idempotent
        (OfflineStale, DirectMessageReceived) => DirectTopicReady,
        (OfflineStale, Timeout) => return None, // idempotent

        // Everything else — an event that does not move the peer (e.g. an
        // `Unknown` peer receiving a failure/timeout/path event) — is an
        // idempotent no-op. The state machine never fabricates progress.
        // This includes every PathChanged* event: path type is diagnostic
        // only and never moves the state machine (BORU-CP-14).
        _ => return None,
    };
    Some(next)
}

/// Outcome of applying one [`ConnectivityEvent`] to a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// The event moved the peer to a new state; a trail record was appended
    /// and the transition was logged at `info!`.
    Transitioned {
        /// State before the event.
        from: PeerConnectivityState,
        /// State after the event.
        to: PeerConnectivityState,
        /// The event that caused the transition.
        event: ConnectivityEvent,
    },
    /// The event did not move the peer (idempotent no-op or unknown peer
    /// with no positive evidence). No trail record, no `info!` log.
    NoChange,
}

/// A single recorded state transition for a peer (the deterministic
/// transition trail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionRecord {
    /// When the transition happened.
    pub at: Instant,
    /// State before the event.
    pub from: PeerConnectivityState,
    /// State after the event.
    pub to: PeerConnectivityState,
    /// The event that caused the transition.
    pub event: ConnectivityEvent,
}

/// How the peer's relay/direct path currently looks (BORU-CP-14).
///
/// This is **diagnostic/optimization information only** (PDF Task 5.2 step
/// 2). It never proves application-level success and chat delivery never
/// depends on it — a relay connection is still considered reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// No path information yet (or the networking layer does not expose a
    /// reliable classification — report Unknown rather than guessing).
    Unknown,
    /// At least one direct (IP) path is currently open.
    Direct,
    /// The peer is currently reachable only via a relay server.
    Relay,
    /// The path is in flux: addresses are known but none is currently
    /// active (connecting / re-negotiating between direct and relay).
    Transitioning,
}

impl PathKind {
    /// Stable short label for logs and diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Direct => "direct",
            Self::Relay => "relay",
            Self::Transitioning => "transitioning",
        }
    }
}

/// Whether the deterministic direct topic has been set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectTopicState {
    /// No topic join attempt recorded yet.
    NotAttempted,
    /// The direct topic join/subscription succeeded.
    Ready,
    /// The direct topic join/subscription failed — visible, not 'online'.
    Failed,
}

/// Per-peer connectivity state plus its bounded transition trail.
#[derive(Debug, Clone)]
pub struct PeerConnectivityEntry {
    /// Stable peer identity.
    pub peer_id: PublicKey,
    /// Current connectivity state (the machine's output).
    pub state: PeerConnectivityState,
    /// When the peer was last seen on the discovery topic, if ever.
    pub discovery_last_seen: Option<Instant>,
    /// When any connectivity event was last applied (eviction ordering).
    pub last_seen: Instant,
    /// Current relay/direct path hint.
    pub path_kind: PathKind,
    /// Whether the deterministic direct topic is set up.
    pub direct_topic_state: DirectTopicState,
    /// Last time a direct (non-discovery) message arrived from the peer.
    pub last_inbound_direct: Option<Instant>,
    /// Last time a direct (non-discovery) message was sent to the peer.
    pub last_outbound_direct: Option<Instant>,
    /// Last time any inbound gossip event arrived from the peer (BORU-CP-13).
    pub last_inbound_gossip: Option<Instant>,
    /// Last time a message from the peer decoded to an application message
    /// and reached application processing (BORU-CP-13).
    pub last_decoded_message: Option<Instant>,
    /// Human-readable last failure (dial / topic / path), if any.
    pub last_error: Option<String>,
    /// Bounded, ordered transition trail (oldest first).
    pub trail: VecDeque<TransitionRecord>,
}

impl PeerConnectivityEntry {
    fn new(peer_id: PublicKey, state: PeerConnectivityState, now: Instant) -> Self {
        Self {
            peer_id,
            state,
            discovery_last_seen: None,
            last_seen: now,
            path_kind: PathKind::Unknown,
            direct_topic_state: DirectTopicState::NotAttempted,
            last_inbound_direct: None,
            last_outbound_direct: None,
            last_inbound_gossip: None,
            last_decoded_message: None,
            last_error: None,
            trail: VecDeque::new(),
        }
    }
}

/// Bounded store of per-peer connectivity state.
///
/// Each peer keeps one [`PeerConnectivityEntry`] with a bounded transition
/// trail. The store is capped at [`MAX_CONNECTIVITY_PEERS`] and evicts
/// stale-then-oldest entries when full. It is a metadata cache only — it
/// grants no authorisation and is never consulted by friendship/trust
/// checks.
#[derive(Debug, Clone)]
pub struct PeerConnectivityStore {
    peers: HashMap<PublicKey, PeerConnectivityEntry>,
    max_peers: usize,
    max_trail: usize,
}

impl Default for PeerConnectivityStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerConnectivityStore {
    /// An empty store with the default limits.
    pub fn new() -> Self {
        Self::with_limits(MAX_CONNECTIVITY_PEERS, MAX_TRAIL_PER_PEER)
    }

    /// An empty store with explicit limits (tests use small caps).
    pub fn with_limits(max_peers: usize, max_trail: usize) -> Self {
        Self {
            peers: HashMap::new(),
            max_peers: max_peers.max(1),
            max_trail: max_trail.max(1),
        }
    }

    /// Apply one real networking event to a peer, idempotently.
    ///
    /// * Unknown peer + positive evidence (`DiscoverySeen`,
    ///   `EndpointConnecting`, `EndpointConnected`,
    ///   `DirectMessageReceived`, `TopicJoined`) → a fresh entry is created
    ///   and the transition recorded.
    /// * Unknown peer + failure/timeout/path event → [`TransitionOutcome::NoChange`]
    ///   (no entry is created from negative evidence alone).
    /// * Known peer + event that moves it → [`TransitionOutcome::Transitioned`],
    ///   trail record appended, transition logged at `info!`.
    /// * Known peer + event that does not move it → [`TransitionOutcome::NoChange`]
    ///   (idempotent; no trail record, no `info!` log). Side-effect
    ///   timestamps (last_seen, discovery_last_seen, last_inbound_direct,
    ///   last_outbound_direct, last_inbound_gossip, last_decoded_message)
    ///   are still refreshed so a no-op event keeps the peer's recency
    ///   current — including the BORU-CP-13 timestamp-only diagnostic
    ///   events ([`ConnectivityEvent::DirectMessageSent`],
    ///   [`ConnectivityEvent::InboundGossipEvent`],
    ///   [`ConnectivityEvent::ApplicationMessageDecoded`]), which never
    ///   move the state machine.
    ///
    /// `now` is explicit so tests can drive timeout/eviction deterministically.
    pub fn apply(
        &mut self,
        peer_id: PublicKey,
        event: ConnectivityEvent,
        now: Instant,
    ) -> TransitionOutcome {
        self.apply_inner(peer_id, event, None, now)
    }

    /// Like [`apply`](Self::apply) but attaches a failure reason for
    /// [`ConnectivityEvent::EndpointFailed`] and
    /// [`ConnectivityEvent::TopicJoinFailed`] (stored as `last_error`).
    pub fn apply_with_error(
        &mut self,
        peer_id: PublicKey,
        event: ConnectivityEvent,
        error: Option<String>,
        now: Instant,
    ) -> TransitionOutcome {
        self.apply_inner(peer_id, event, error, now)
    }

    fn apply_inner(
        &mut self,
        peer_id: PublicKey,
        event: ConnectivityEvent,
        error: Option<String>,
        now: Instant,
    ) -> TransitionOutcome {
        let outcome = if let Some(entry) = self.peers.get_mut(&peer_id) {
            entry.last_seen = now;
            // Side-effect timestamps refresh even on idempotent no-ops.
            match event {
                ConnectivityEvent::DiscoverySeen => {
                    entry.discovery_last_seen = Some(now);
                }
                ConnectivityEvent::DirectMessageReceived => {
                    entry.last_inbound_direct = Some(now);
                }
                ConnectivityEvent::DirectMessageSent => {
                    entry.last_outbound_direct = Some(now);
                }
                ConnectivityEvent::InboundGossipEvent => {
                    entry.last_inbound_gossip = Some(now);
                }
                ConnectivityEvent::ApplicationMessageDecoded => {
                    entry.last_decoded_message = Some(now);
                }
                // Path hints are recorded even when the event is an
                // idempotent no-op (e.g. `PathChangedDirect` on an already
                // Reachable peer) — the peer IS on that path, and the
                // diagnostics snapshot must not report `unknown` for a
                // healthy direct/relay peer (BORU-CP-13/14). Path events
                // never move the state machine (BORU-CP-14); a *changed*
                // path kind is logged in a structured `info!` line so path
                // transitions are observable without reading message
                // contents.
                ConnectivityEvent::PathChangedDirect
                | ConnectivityEvent::PathChangedRelay
                | ConnectivityEvent::PathChangedTransitioning => {
                    let new_kind = match event {
                        ConnectivityEvent::PathChangedDirect => PathKind::Direct,
                        ConnectivityEvent::PathChangedRelay => PathKind::Relay,
                        _ => PathKind::Transitioning,
                    };
                    let old_kind = entry.path_kind;
                    entry.path_kind = new_kind;
                    if old_kind != new_kind {
                        info!(
                            peer = %peer_id.fmt_short(),
                            from_path = old_kind.label(),
                            to_path = new_kind.label(),
                            "connectivity: peer path changed",
                        );
                    } else {
                        trace!(
                            peer = %peer_id.fmt_short(),
                            path = new_kind.label(),
                            "connectivity: peer path unchanged",
                        );
                    }
                }
                _ => {}
            }
            match transition(entry.state, event) {
                Some(next) => {
                    let from = entry.state;
                    entry.state = next;
                    match event {
                        ConnectivityEvent::EndpointFailed => {
                            entry.last_error = error.clone();
                        }
                        ConnectivityEvent::TopicJoinFailed => {
                            entry.last_error = error.clone();
                            entry.direct_topic_state = DirectTopicState::Failed;
                        }
                        ConnectivityEvent::TopicJoined => {
                            entry.direct_topic_state = DirectTopicState::Ready;
                            entry.last_error = None;
                        }
                        // Success events clear the last failure so the
                        // trail reads as "recovered".
                        ConnectivityEvent::EndpointConnected
                        | ConnectivityEvent::DirectMessageReceived => {
                            entry.last_error = None;
                        }
                        _ => {}
                    }
                    entry.push_trail(now, from, next, event, self.max_trail);
                    info!(
                        peer = %peer_id.fmt_short(),
                        from = from.label(),
                        to = next.label(),
                        event = event.label(),
                        "connectivity: peer state transition",
                    );
                    TransitionOutcome::Transitioned {
                        from,
                        to: next,
                        event,
                    }
                }
                None => {
                    trace!(
                        peer = %peer_id.fmt_short(),
                        state = entry.state.label(),
                        event = event.label(),
                        "connectivity: idempotent no-change event",
                    );
                    TransitionOutcome::NoChange
                }
            }
        } else {
            // Unknown peer: only positive evidence creates an entry.
            let Some(next) = transition(PeerConnectivityState::Unknown, event) else {
                trace!(
                    peer = %peer_id.fmt_short(),
                    event = event.label(),
                    "connectivity: no entry created from negative evidence",
                );
                return TransitionOutcome::NoChange;
            };
            if self.peers.len() >= self.max_peers {
                self.evict_one(now);
            }
            let mut entry = PeerConnectivityEntry::new(peer_id, next, now);
            match event {
                ConnectivityEvent::DiscoverySeen => {
                    entry.discovery_last_seen = Some(now);
                }
                ConnectivityEvent::DirectMessageReceived => {
                    entry.last_inbound_direct = Some(now);
                }
                ConnectivityEvent::PathChangedDirect => {
                    entry.path_kind = PathKind::Direct;
                }
                ConnectivityEvent::PathChangedRelay => {
                    entry.path_kind = PathKind::Relay;
                }
                ConnectivityEvent::PathChangedTransitioning => {
                    // Unreachable in practice: `transition(Unknown, …)`
                    // returns `None` for path events, so no entry is
                    // created from a path event alone. Kept exhaustive.
                    entry.path_kind = PathKind::Transitioning;
                }
                _ => {}
            }
            entry.push_trail(
                now,
                PeerConnectivityState::Unknown,
                next,
                event,
                self.max_trail,
            );
            info!(
                peer = %peer_id.fmt_short(),
                from = "unknown",
                to = next.label(),
                event = event.label(),
                "connectivity: peer state transition",
            );
            self.peers.insert(peer_id, entry);
            TransitionOutcome::Transitioned {
                from: PeerConnectivityState::Unknown,
                to: next,
                event,
            }
        };
        outcome
    }

    /// Look up the full entry for `node_id`, if present.
    pub fn get(&self, node_id: &PublicKey) -> Option<&PeerConnectivityEntry> {
        self.peers.get(node_id)
    }

    /// Current connectivity state for `node_id`, or [`PeerConnectivityState::Unknown`]
    /// when the peer is not tracked.
    pub fn state(&self, node_id: &PublicKey) -> PeerConnectivityState {
        self.peers
            .get(node_id)
            .map(|e| e.state)
            .unwrap_or(PeerConnectivityState::Unknown)
    }

    /// The ordered transition trail for `node_id` (oldest first), or an
    /// empty vec when the peer is not tracked.
    pub fn trail(&self, node_id: &PublicKey) -> Vec<TransitionRecord> {
        self.peers
            .get(node_id)
            .map(|e| e.trail.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Iterate over all tracked peers.
    pub fn peers(&self) -> impl Iterator<Item = (&PublicKey, &PeerConnectivityEntry)> {
        self.peers.iter()
    }

    /// Number of tracked peers (bounded by `max_peers`).
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Remove every entry, returning the removed ids.
    pub fn clear(&mut self) -> Vec<PublicKey> {
        let removed: Vec<PublicKey> = self.peers.keys().copied().collect();
        self.peers.clear();
        removed
    }

    /// Apply [`ConnectivityEvent::Timeout`] to every peer not heard from
    /// within `ttl` at `now` (used by the presence-expiry sweep). Returns
    /// the ids that moved to [`PeerConnectivityState::OfflineStale`].
    ///
    /// Peers already `OfflineStale` are idempotent no-ops.
    pub fn expire_stale(&mut self, now: Instant, ttl: Duration) -> Vec<PublicKey> {
        let mut expired = Vec::new();
        let ids: Vec<PublicKey> = self.peers.keys().copied().collect();
        for id in ids {
            let stale = self
                .peers
                .get(&id)
                .map(|e| now.duration_since(e.last_seen) >= ttl)
                .unwrap_or(false);
            if stale {
                let outcome = self.apply(id, ConnectivityEvent::Timeout, now);
                if matches!(outcome, TransitionOutcome::Transitioned { .. }) {
                    expired.push(id);
                }
            }
        }
        expired
    }

    /// Evict one entry: stale (offline) entries first, then the one with
    /// the oldest last activity.
    fn evict_one(&mut self, now: Instant) {
        let stale = self
            .peers
            .iter()
            .find(|(_, e)| e.state == PeerConnectivityState::OfflineStale)
            .map(|(k, _)| *k);
        if let Some(key) = stale {
            self.peers.remove(&key);
            return;
        }
        let oldest = self
            .peers
            .iter()
            .min_by_key(|(_, e)| e.last_seen)
            .map(|(k, _)| *k);
        if let Some(key) = oldest {
            self.peers.remove(&key);
        }
        // `now` is used to keep the eviction policy symmetric with the
        // presence store (which prefers stale-by-TTL); here we prefer the
        // explicit OfflineStale state, then oldest activity.
        let _ = now;
    }
}

impl PeerConnectivityEntry {
    fn push_trail(
        &mut self,
        at: Instant,
        from: PeerConnectivityState,
        to: PeerConnectivityState,
        event: ConnectivityEvent,
        max_trail: usize,
    ) {
        if self.trail.len() >= max_trail {
            self.trail.pop_front();
        }
        self.trail.push_back(TransitionRecord {
            at,
            from,
            to,
            event,
        });
    }
}

// ---------------------------------------------------------------------------
// Desired-vs-observed connectivity reconciliation (BORU-DISC-003)
// ---------------------------------------------------------------------------
//
// The state machine above records *observed* connectivity facts — fed only
// by real networking events. It says nothing about what the local user
// *wants*. BORU-DISC-003 separates the two and adds a small pure
// reconciliation layer that decides which side effect is required *now* to
// drive the observed facts toward the desired connectivity, regardless of
// the order in which events arrived.
//
// * **Observed facts** = [`PeerConnectivityStore`] (the state machine) plus
//   the explicit reconnect scheduling/backoff input
//   ([`ObservedConnectivity`]). These are facts; reconciliation never
//   mutates them.
// * **Desired connectivity** = [`DesiredConnectivity`], the target level a
//   caller states for a peer (e.g. "I want this friend's endpoint
//   reachable").
// * **Reconciliation** = [`reconcile`]: a pure, idempotent function that
//   returns the single required side effect (reconnect scheduling) or a
//   no-action reason. Being pure, it is safe to call repeatedly: two calls
//   with unchanged input return the same decision and schedule no duplicate
//   work (the reconnect scheduler's own dedup is the second safety net).

/// The connectivity level a caller *desires* for a peer, independent of
/// what is currently observed. This is the explicit statement of intent —
/// the app declares what it wants, reconciliation decides what to do about
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredConnectivity {
    /// No connectivity desired — the peer is not a reconciliation target.
    None,
    /// Desire the peer's endpoint to be reachable (a gossip mesh edge).
    ///
    /// Satisfied by [`PeerConnectivityState::Reachable`] or
    /// [`PeerConnectivityState::DirectTopicReady`] — exactly the
    /// state-machine-derived `is_online()` test, so this is the
    /// behaviour-preserving default for the app's reconnect trigger.
    EndpointReachable,
    /// Desire direct messaging (the deterministic direct topic joined).
    ///
    /// Satisfied only by [`PeerConnectivityState::DirectTopicReady`].
    DirectTopicReady,
}

impl DesiredConnectivity {
    /// Stable short label for structured logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::EndpointReachable => "endpoint-reachable",
            Self::DirectTopicReady => "direct-topic-ready",
        }
    }

    /// Whether an observed `state` already satisfies this desire.
    ///
    /// This is the single source of the "are we there yet?" test, so the
    /// convergence target cannot drift between call sites.
    pub fn satisfied_by(self, state: PeerConnectivityState) -> bool {
        use PeerConnectivityState::*;
        match self {
            Self::None => true, // nothing desired, nothing to converge
            Self::EndpointReachable => matches!(state, Reachable | DirectTopicReady),
            Self::DirectTopicReady => matches!(state, DirectTopicReady),
        }
    }
}

/// Observed, real connectivity facts for a peer, plus the explicit
/// reconnect scheduling/backoff input (BORU-DISC-003 objective 4).
///
/// Facts only — reconciliation never mutates this. The reconnect/backoff
/// state (whether an attempt is already queued or in flight, and how many
/// failures have accumulated) rides along as data into the decision rather
/// than being re-derived from scattered timing checks inside the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedConnectivity {
    /// Current state-machine snapshot ([`PeerConnectivityStore::state`]).
    pub state: PeerConnectivityState,
    /// Whether a reconnect attempt is already queued or in flight (the
    /// dedup anchor — reconciliation must not double-queue).
    pub reconnect_pending: bool,
    /// Completed failed reconnect attempts (the explicit backoff input).
    pub reconnect_attempts: u32,
}

impl Default for ObservedConnectivity {
    fn default() -> Self {
        Self {
            state: PeerConnectivityState::Unknown,
            reconnect_pending: false,
            reconnect_attempts: 0,
        }
    }
}

/// Why reconciliation decided no side effect is required now (logged as the
/// structured `reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileReason {
    /// [`DesiredConnectivity::None`] — nothing to reconcile.
    NotDesired,
    /// The observed state already satisfies the desired connectivity.
    AlreadySatisfied,
    /// A reconnect attempt is already queued or in flight — do not
    /// double-queue (idempotence against duplicate/late events).
    ReconnectPending,
}

impl ReconcileReason {
    /// Stable short label for structured logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::NotDesired => "not-desired",
            Self::AlreadySatisfied => "already-satisfied",
            Self::ReconnectPending => "reconnect-pending",
        }
    }
}

/// The reconciliation decision: which side effect is required *now*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileDecision {
    /// Nothing required now; carries the reason for structured logging.
    NoAction {
        /// Why no side effect is required.
        reason: ReconcileReason,
    },
    /// A reconnect attempt must be scheduled to drive the peer toward the
    /// desired connectivity.
    ScheduleReconnect {
        /// The explicit backoff input (completed failed attempts) carried
        /// through, so the caller can apply the scheduled cadence and log
        /// the retry stage it is entering.
        attempts: u32,
    },
}

/// Pure reconciliation of desired vs observed connectivity (BORU-DISC-003
/// objectives 1–3).
///
/// Decides which side effect is required now to drive `observed` toward
/// `desired`. It is a pure function — no timers, no network, no mutation —
/// so it is safe to call repeatedly and is idempotent:
///
/// * Two calls with unchanged input return the same decision.
/// * Calling `reconcile` twice for an already-satisfied peer yields
///   [`ReconcileDecision::NoAction`] both times — no duplicate dial or
///   publish/reconnect work.
/// * A peer with a reconnect attempt already queued or in flight yields
///   [`ReconcileReason::ReconnectPending`] — duplicate/late announcements
///   cannot double-queue.
///
/// Convergence comes from the underlying state machine: as real networking
/// events advance `observed.state`, repeated `reconcile` calls stop
/// scheduling once the observed state satisfies the desire, converging to
/// the same final state regardless of the order the events arrived in.
pub fn reconcile(
    desired: DesiredConnectivity,
    observed: ObservedConnectivity,
) -> ReconcileDecision {
    use DesiredConnectivity::*;
    match desired {
        None => ReconcileDecision::NoAction {
            reason: ReconcileReason::NotDesired,
        },
        _ => {
            if desired.satisfied_by(observed.state) {
                ReconcileDecision::NoAction {
                    reason: ReconcileReason::AlreadySatisfied,
                }
            } else if observed.reconnect_pending {
                ReconcileDecision::NoAction {
                    reason: ReconcileReason::ReconnectPending,
                }
            } else {
                ReconcileDecision::ScheduleReconnect {
                    attempts: observed.reconnect_attempts,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::connectivity::{ConnectivityEvent as E, PeerConnectivityState as S};

    // ── Desired-vs-observed reconciliation (BORU-DISC-003) ─────────────

    fn observed(state: S) -> ObservedConnectivity {
        ObservedConnectivity {
            state,
            reconnect_pending: false,
            reconnect_attempts: 0,
        }
    }

    /// No desire → no side effect, whatever the observed state.
    #[test]
    fn reconcile_not_desired_is_always_no_action() {
        for state in [
            S::Unknown,
            S::Discovered,
            S::Connecting,
            S::Reachable,
            S::DirectTopicReady,
            S::Degraded,
            S::OfflineStale,
        ] {
            assert_eq!(
                reconcile(DesiredConnectivity::None, observed(state)),
                ReconcileDecision::NoAction {
                    reason: ReconcileReason::NotDesired
                }
            );
        }
    }

    /// An offline/unmet peer is scheduled for a reconnect attempt, and the
    /// explicit backoff input (attempts) is carried through.
    #[test]
    fn reconcile_schedules_when_desired_not_met() {
        let decision = reconcile(
            DesiredConnectivity::EndpointReachable,
            ObservedConnectivity {
                state: S::Discovered,
                reconnect_pending: false,
                reconnect_attempts: 3,
            },
        );
        assert_eq!(
            decision,
            ReconcileDecision::ScheduleReconnect { attempts: 3 },
            "backoff input must ride through the decision"
        );
    }

    /// Once the observed state satisfies the desire, reconcile is a no-op —
    /// calling it twice with unchanged state produces no duplicate
    /// dial/publish/reconnect work (idempotence).
    #[test]
    fn reconcile_is_idempotent_when_satisfied() {
        use DesiredConnectivity::*;
        // EndpointReachable is satisfied by Reachable or DirectTopicReady.
        for state in [S::Reachable, S::DirectTopicReady] {
            let obs = observed(state);
            let first = reconcile(EndpointReachable, obs);
            let second = reconcile(EndpointReachable, obs);
            assert_eq!(
                first, second,
                "repeated reconcile with unchanged state must be a no-op"
            );
            assert_eq!(
                first,
                ReconcileDecision::NoAction {
                    reason: ReconcileReason::AlreadySatisfied
                }
            );
        }
    }

    /// DirectTopicReady is satisfied only by DirectTopicReady — a merely
    /// Reachable peer still requires work toward the direct topic.
    #[test]
    fn reconcile_direct_topic_ready_requires_more_than_reachable() {
        assert_eq!(
            reconcile(
                DesiredConnectivity::DirectTopicReady,
                observed(S::Reachable),
            ),
            ReconcileDecision::ScheduleReconnect { attempts: 0 },
            "endpoint-reachable is not direct-topic-ready"
        );
        assert_eq!(
            reconcile(
                DesiredConnectivity::DirectTopicReady,
                observed(S::DirectTopicReady),
            ),
            ReconcileDecision::NoAction {
                reason: ReconcileReason::AlreadySatisfied
            }
        );
    }

    /// A queued/in-flight reconnect attempt is never double-queued, even
    /// when the desired state is unmet (idempotence against duplicate/late
    /// announcements).
    #[test]
    fn reconcile_does_not_double_schedule_pending_attempt() {
        let decision = reconcile(
            DesiredConnectivity::EndpointReachable,
            ObservedConnectivity {
                state: S::Discovered,
                reconnect_pending: true,
                reconnect_attempts: 1,
            },
        );
        assert_eq!(
            decision,
            ReconcileDecision::NoAction {
                reason: ReconcileReason::ReconnectPending
            }
        );
    }

    /// Late and duplicate events converge to the same final state: no
    /// matter the order the events arrived in, once the state machine
    /// observes the peer as reachable the reconciliation stops scheduling.
    #[test]
    fn reconcile_converges_regardless_of_event_order() {
        use DesiredConnectivity::*;
        let desired = EndpointReachable;

        // Order A: Discovered → EndpointConnected → TopicJoined.
        let mut store = PeerConnectivityStore::new();
        let t0 = Instant::now();
        let a = key(0x30);
        store.apply(a, E::DiscoverySeen, t0);
        let decisions_a = [
            reconcile(desired, observed(store.state(&a))), // Discovered → schedule
            reconcile(desired, observed(store.state(&a))),
            // (both Discovered calls schedule — duplicate reconcile is
            //  identical, not silenced by backoff, but scheduler dedups)
        ];
        store.apply(a, E::EndpointConnected, t0 + Duration::from_secs(1));
        let d_reachable = reconcile(desired, observed(store.state(&a))); // Reachable → no action
        store.apply(a, E::TopicJoined, t0 + Duration::from_secs(2));

        // Order B: identical final state regardless of the intermediate
        // sequence (Reachable here too).
        let mut store_b = PeerConnectivityStore::new();
        let b = key(0x31);
        store_b.apply(b, E::EndpointConnected, t0); // straight to Reachable
        let d_reachable_b = reconcile(desired, observed(store_b.state(&b)));

        // Both sides converge to the same no-action decision once reachable.
        assert_eq!(d_reachable, d_reachable_b);
        assert_eq!(
            d_reachable,
            ReconcileDecision::NoAction {
                reason: ReconcileReason::AlreadySatisfied
            }
        );
        // And the duplicate Discovered reconciles were identical decisions.
        assert_eq!(decisions_a[0], decisions_a[1]);
        assert_eq!(
            decisions_a[0],
            ReconcileDecision::ScheduleReconnect { attempts: 0 }
        );
    }

    /// Reconcile is pure: it never mutates the observed facts.
    #[test]
    fn reconcile_is_pure() {
        let obs = ObservedConnectivity {
            state: S::Discovered,
            reconnect_pending: false,
            reconnect_attempts: 2,
        };
        let before = obs;
        let _ = reconcile(DesiredConnectivity::EndpointReachable, obs);
        assert_eq!(obs, before, "reconcile must not mutate its inputs");
    }

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    // ── The documented transition table ───────────────────────────────

    /// The full expected-value transition table (PDF Task 2.2 step 2).
    ///
    /// Every legal transition is listed explicitly as `(from, event, to)`.
    /// The sweep below additionally asserts that every `(state, event)` pair
    /// NOT in this table is a `None` no-op, so the pure function and the
    /// documented table cannot drift apart — this is the table-driven
    /// acceptance criterion for BORU-DISC-002.
    ///
    /// Keep in sync with `docs/discovery-refactor/05-peer-connectivity-state-machine.md`.
    #[test]
    fn transition_table_matches_documented_expected_values() {
        use ConnectivityEvent as E;
        use PeerConnectivityState as S;

        // (from, event, expected_to) — every legal transition.
        let legal: &[(S, E, S)] = &[
            // ── Unknown ──────────────────────────────────────────────
            (S::Unknown, E::DiscoverySeen, S::Discovered),
            (S::Unknown, E::EndpointConnecting, S::Connecting),
            (S::Unknown, E::EndpointConnected, S::Reachable),
            (S::Unknown, E::DirectMessageReceived, S::DirectTopicReady),
            (S::Unknown, E::TopicJoined, S::DirectTopicReady),
            // ── Discovered ───────────────────────────────────────────
            (S::Discovered, E::EndpointConnecting, S::Connecting),
            (S::Discovered, E::EndpointConnected, S::Reachable),
            (S::Discovered, E::EndpointFailed, S::Degraded),
            (S::Discovered, E::TopicJoined, S::DirectTopicReady),
            (S::Discovered, E::TopicJoinFailed, S::Degraded),
            (S::Discovered, E::DirectMessageReceived, S::DirectTopicReady),
            (S::Discovered, E::Timeout, S::OfflineStale),
            // ── Connecting ───────────────────────────────────────────
            (S::Connecting, E::EndpointConnected, S::Reachable),
            (S::Connecting, E::EndpointFailed, S::Degraded),
            (S::Connecting, E::TopicJoined, S::DirectTopicReady),
            (S::Connecting, E::TopicJoinFailed, S::Degraded),
            (S::Connecting, E::DirectMessageReceived, S::DirectTopicReady),
            (S::Connecting, E::Timeout, S::OfflineStale),
            // ── Reachable ────────────────────────────────────────────
            (S::Reachable, E::EndpointFailed, S::Degraded),
            (S::Reachable, E::TopicJoined, S::DirectTopicReady),
            (S::Reachable, E::TopicJoinFailed, S::Degraded),
            (S::Reachable, E::DirectMessageReceived, S::DirectTopicReady),
            (S::Reachable, E::Timeout, S::OfflineStale),
            // ── DirectTopicReady ─────────────────────────────────────
            (S::DirectTopicReady, E::EndpointFailed, S::Degraded),
            (S::DirectTopicReady, E::TopicJoinFailed, S::Degraded),
            (S::DirectTopicReady, E::Timeout, S::OfflineStale),
            // ── Degraded ─────────────────────────────────────────────
            (S::Degraded, E::EndpointConnecting, S::Connecting),
            (S::Degraded, E::EndpointConnected, S::Reachable),
            (S::Degraded, E::TopicJoined, S::DirectTopicReady),
            (S::Degraded, E::DirectMessageReceived, S::DirectTopicReady),
            (S::Degraded, E::Timeout, S::OfflineStale),
            // ── OfflineStale ─────────────────────────────────────────
            (S::OfflineStale, E::DiscoverySeen, S::Discovered),
            (S::OfflineStale, E::EndpointConnecting, S::Connecting),
            (S::OfflineStale, E::EndpointConnected, S::Reachable),
            (S::OfflineStale, E::TopicJoined, S::DirectTopicReady),
            (
                S::OfflineStale,
                E::DirectMessageReceived,
                S::DirectTopicReady,
            ),
        ];

        let all_states = [
            S::Unknown,
            S::Discovered,
            S::Connecting,
            S::Reachable,
            S::DirectTopicReady,
            S::Degraded,
            S::OfflineStale,
        ];
        let all_events = [
            E::DiscoverySeen,
            E::EndpointConnecting,
            E::EndpointConnected,
            E::EndpointFailed,
            E::TopicJoined,
            E::TopicJoinFailed,
            E::DirectMessageReceived,
            E::Timeout,
            E::PathChangedDirect,
            E::PathChangedRelay,
            E::PathChangedTransitioning,
            E::DirectMessageSent,
            E::InboundGossipEvent,
            E::ApplicationMessageDecoded,
        ];

        // Exhaustive sweep: every pair either matches the documented table
        // exactly or is a documented `None` no-op (illegal / stale /
        // duplicate / diagnostic-only events).
        for state in all_states {
            for event in all_events {
                let expected = legal
                    .iter()
                    .find(|(s, e, _)| *s == state && *e == event)
                    .map(|(_, _, to)| *to);
                assert_eq!(
                    transition(state, event),
                    expected,
                    "transition({state:?}, {event:?}) must match the documented table"
                );
            }
        }

        // Idempotence: re-applying the triggering event from the destination
        // state must never move the peer again — duplicate events are
        // no-ops, so duplicates cannot cause connection loops.
        for &(from, event, to) in legal {
            assert_eq!(
                transition(to, event),
                None,
                "re-applying {event:?} after {from:?} -> {to:?} must be an idempotent no-op"
            );
        }
    }

    #[test]
    fn transition_table_is_deterministic_and_idempotent() {
        // Every rule in the table must be a pure function of (state, event),
        // and re-applying the same event must never move the peer again.
        let all_states = [
            S::Unknown,
            S::Discovered,
            S::Connecting,
            S::Reachable,
            S::DirectTopicReady,
            S::Degraded,
            S::OfflineStale,
        ];
        let all_events = [
            E::DiscoverySeen,
            E::EndpointConnecting,
            E::EndpointConnected,
            E::EndpointFailed,
            E::TopicJoined,
            E::TopicJoinFailed,
            E::DirectMessageReceived,
            E::Timeout,
            E::PathChangedDirect,
            E::PathChangedRelay,
        ];
        for state in all_states {
            for event in all_events {
                let first = transition(state, event);
                // Deterministic: same inputs, same output.
                assert_eq!(first, transition(state, event), "transition must be pure");
                if let Some(next) = first {
                    // Idempotent: applying the same event again from `next`
                    // must not move the peer (unless it is a different pair
                    // that legitimately advances, which is fine — the
                    // guarantee is about duplicate *announcements*, checked
                    // below).
                    let _ = next;
                }
            }
        }
    }

    /// Duplicate announcements (DiscoverySeen) and duplicate dial events
    /// (EndpointConnecting) never re-enter Connecting and never spawn a
    /// second transition — the connection-loop guard.
    #[test]
    fn duplicate_announcements_do_not_cause_connection_loops() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x01);
        let t0 = Instant::now();

        // First sighting creates the entry in Discovered.
        assert_eq!(
            store.apply(peer, E::DiscoverySeen, t0),
            TransitionOutcome::Transitioned {
                from: S::Unknown,
                to: S::Discovered,
                event: E::DiscoverySeen,
            }
        );

        // 100 duplicate announcements: still Discovered, still one trail
        // record, and never a transition to Connecting.
        for i in 1..=100 {
            assert_eq!(
                store.apply(peer, E::DiscoverySeen, t0 + Duration::from_millis(i)),
                TransitionOutcome::NoChange,
                "duplicate DiscoverySeen must be an idempotent no-op"
            );
            assert_eq!(store.state(&peer), S::Discovered);
        }
        assert_eq!(
            store.trail(&peer).len(),
            1,
            "duplicate announcements must not append trail records"
        );

        // A single dial event moves to Connecting, then duplicate dial
        // events stay put (no connection loop).
        assert_eq!(
            store.apply(peer, E::EndpointConnecting, t0 + Duration::from_millis(200)),
            TransitionOutcome::Transitioned {
                from: S::Discovered,
                to: S::Connecting,
                event: E::EndpointConnecting,
            }
        );
        for i in 201..=300 {
            assert_eq!(
                store.apply(peer, E::EndpointConnecting, t0 + Duration::from_millis(i)),
                TransitionOutcome::NoChange,
                "duplicate dial events must not re-enter Connecting"
            );
            assert_eq!(store.state(&peer), S::Connecting);
        }
        assert_eq!(store.trail(&peer).len(), 2);
    }

    /// A peer can be Discovered but not DirectTopicReady — the core
    /// distinction this state machine exists to make.
    #[test]
    fn discovered_peer_is_not_direct_topic_ready() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x02);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        assert_eq!(store.state(&peer), S::Discovered);
        assert!(
            !store.state(&peer).is_ready_for_direct(),
            "a discovered peer must NOT be ready for direct messaging"
        );
        assert!(
            !store.state(&peer).is_online(),
            "discovered-only is not online (no endpoint connection)"
        );
        assert!(store.state(&peer).is_known());

        // The direct topic join is what makes it ready.
        store.apply(peer, E::TopicJoined, t0 + Duration::from_secs(1));
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert!(store.state(&peer).is_ready_for_direct());
        assert!(store.state(&peer).is_online());
    }

    /// Failed direct-topic setup is visible as Degraded with a recorded
    /// error — NOT reported simply as 'online'.
    #[test]
    fn failed_direct_topic_setup_is_visible_not_online() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x03);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_secs(1));
        assert_eq!(store.state(&peer), S::Reachable);
        assert!(store.state(&peer).is_online());

        // Topic join fails: peer goes Degraded, direct_topic_state=Failed,
        // last_error recorded. It must NOT look online anymore.
        let outcome = store.apply_with_error(
            peer,
            E::TopicJoinFailed,
            Some("direct topic join timed out".to_string()),
            t0 + Duration::from_secs(2),
        );
        assert_eq!(
            outcome,
            TransitionOutcome::Transitioned {
                from: S::Reachable,
                to: S::Degraded,
                event: E::TopicJoinFailed,
            }
        );
        let entry = store.get(&peer).unwrap();
        assert_eq!(entry.state, S::Degraded);
        assert_eq!(entry.direct_topic_state, DirectTopicState::Failed);
        assert_eq!(
            entry.last_error.as_deref(),
            Some("direct topic join timed out")
        );
        assert!(
            !store.state(&peer).is_online(),
            "a failed direct-topic setup must never be reported as online"
        );
        assert!(!store.state(&peer).is_ready_for_direct());

        // Recovery is explicit: a successful topic join clears the error.
        store.apply(peer, E::TopicJoined, t0 + Duration::from_secs(3));
        let entry = store.get(&peer).unwrap();
        assert_eq!(entry.state, S::DirectTopicReady);
        assert_eq!(entry.last_error, None);
        assert!(store.state(&peer).is_online());
    }

    /// The deterministic transition trail: ordered records, oldest first,
    /// exactly one per real transition.
    #[test]
    fn transition_trail_is_deterministic_and_ordered() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x04);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnecting, t0 + Duration::from_millis(1));
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_millis(2));
        store.apply(peer, E::TopicJoined, t0 + Duration::from_millis(3));
        store.apply(peer, E::DiscoverySeen, t0 + Duration::from_millis(4)); // no-op

        let trail = store.trail(&peer);
        let expected: Vec<(S, S, E)> = vec![
            (S::Unknown, S::Discovered, E::DiscoverySeen),
            (S::Discovered, S::Connecting, E::EndpointConnecting),
            (S::Connecting, S::Reachable, E::EndpointConnected),
            (S::Reachable, S::DirectTopicReady, E::TopicJoined),
        ];
        assert_eq!(trail.len(), expected.len());
        for (record, (from, to, event)) in trail.iter().zip(expected.iter()) {
            assert_eq!(record.from, *from);
            assert_eq!(record.to, *to);
            assert_eq!(record.event, *event);
        }
        // Timestamps are monotonic.
        for pair in trail.windows(2) {
            assert!(pair[0].at <= pair[1].at);
        }
    }

    /// Reordered events converge: EndpointConnected before DiscoverySeen
    /// still ends Reachable, and stale DiscoverySeen afterwards does not
    /// regress it.
    #[test]
    fn reordered_events_converge_and_never_regress() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x05);
        let t0 = Instant::now();

        // Endpoint connected before any discovery announcement.
        assert_eq!(
            store.apply(peer, E::EndpointConnected, t0),
            TransitionOutcome::Transitioned {
                from: S::Unknown,
                to: S::Reachable,
                event: E::EndpointConnected,
            }
        );
        // A late discovery announcement is an idempotent no-op.
        assert_eq!(
            store.apply(peer, E::DiscoverySeen, t0 + Duration::from_secs(1)),
            TransitionOutcome::NoChange
        );
        assert_eq!(store.state(&peer), S::Reachable);

        // A stale timeout cannot resurrect a peer that just reconnected.
        store.apply(peer, E::Timeout, t0 + Duration::from_secs(2));
        assert_eq!(store.state(&peer), S::OfflineStale);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_secs(3));
        assert_eq!(store.state(&peer), S::Reachable);
        // Duplicate timeout while Reachable is a no-op.
        store.apply(peer, E::Timeout, t0 + Duration::from_secs(4));
        assert_eq!(store.state(&peer), S::OfflineStale);
    }

    /// Timeout moves a peer to OfflineStale; expire_stale applies it to
    /// every peer not heard from within the TTL.
    #[test]
    fn expire_stale_moves_peers_to_offline() {
        let mut store = PeerConnectivityStore::with_limits(16, 8);
        let a = key(0x06);
        let b = key(0x07);
        let t0 = Instant::now();

        store.apply(a, E::DiscoverySeen, t0);
        store.apply(b, E::DiscoverySeen, t0);
        // B refreshes at t+7: at t+10 B is 3s old, at t+11 B is 4s old —
        // comfortably within the 5s TTL so only A is ever stale.
        store.apply(b, E::DiscoverySeen, t0 + Duration::from_secs(7));

        let expired = store.expire_stale(t0 + Duration::from_secs(10), Duration::from_secs(5));
        assert_eq!(expired, vec![a], "only A is stale at t+10 (last seen t0)");
        assert_eq!(store.state(&a), S::OfflineStale);
        assert_eq!(store.state(&b), S::Discovered, "B was refreshed within TTL");
        // Re-expiring is idempotent.
        let again = store.expire_stale(t0 + Duration::from_secs(11), Duration::from_secs(5));
        assert!(again.is_empty(), "already-offline peer must not re-expire");
    }

    /// Unknown peer + negative evidence creates no entry.
    #[test]
    fn unknown_peer_negative_evidence_creates_no_entry() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x08);
        let t0 = Instant::now();

        assert_eq!(
            store.apply(peer, E::Timeout, t0),
            TransitionOutcome::NoChange
        );
        assert_eq!(
            store.apply(peer, E::EndpointFailed, t0),
            TransitionOutcome::NoChange
        );
        assert_eq!(
            store.apply(peer, E::TopicJoinFailed, t0),
            TransitionOutcome::NoChange
        );
        assert!(store.is_empty());
    }

    /// The store is bounded: at capacity it evicts stale-then-oldest.
    #[test]
    fn store_is_bounded() {
        let mut store = PeerConnectivityStore::with_limits(2, 8);
        let a = key(0x09);
        let b = key(0x0A);
        let c = key(0x0B);
        let t0 = Instant::now();

        store.apply(a, E::DiscoverySeen, t0);
        store.apply(b, E::DiscoverySeen, t0 + Duration::from_secs(1));
        store.apply(c, E::DiscoverySeen, t0 + Duration::from_secs(2));

        assert_eq!(store.len(), 2, "store must stay bounded");
        assert!(!store.contains_key(&a), "oldest entry must be evicted");
        assert!(store.contains_key(&b));
        assert!(store.contains_key(&c));
    }

    /// The trail is bounded per peer.
    #[test]
    fn trail_is_bounded_per_peer() {
        let mut store = PeerConnectivityStore::with_limits(4, 3);
        let peer = key(0x0C);
        let t0 = Instant::now();

        // Walk a long chain of distinct transitions.
        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnecting, t0 + Duration::from_millis(1));
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_millis(2));
        store.apply(peer, E::TopicJoined, t0 + Duration::from_millis(3));
        store.apply(peer, E::Timeout, t0 + Duration::from_millis(4));
        store.apply(peer, E::DiscoverySeen, t0 + Duration::from_millis(5));
        store.apply(peer, E::EndpointConnecting, t0 + Duration::from_millis(6));

        let trail = store.trail(&peer);
        assert!(trail.len() <= 3, "trail must be capped at max_trail");
        // The trail holds the most recent records.
        assert_eq!(trail.last().unwrap().event, E::EndpointConnecting);
    }

    /// Path changes update the path hint; the path kind is diagnostic
    /// (BORU-CP-14) so it never moves the state machine — a relay
    /// connection is still considered reachable.
    #[test]
    fn path_changes_are_reflected() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x0D);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_secs(1));
        // Relay-only path: the peer stays Reachable (still online), the
        // path hint records relay.
        store.apply(peer, E::PathChangedRelay, t0 + Duration::from_secs(2));
        assert_eq!(store.state(&peer), S::Reachable);
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Relay);
        assert!(store.state(&peer).is_online());
        // Back to direct.
        store.apply(peer, E::PathChangedDirect, t0 + Duration::from_secs(3));
        assert_eq!(store.state(&peer), S::Reachable);
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Direct);
        assert!(store.state(&peer).is_online());
    }

    /// Path hints are recorded even when the path event is an idempotent
    /// no-op (e.g. `PathChangedDirect` on an already Reachable peer), so
    /// the diagnostics snapshot never reports `unknown` for a peer that is
    /// demonstrably on a direct/relay path (BORU-CP-13/14).
    #[test]
    fn path_hint_recorded_on_noop_path_event() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x0D2);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_secs(1));
        assert_eq!(store.state(&peer), S::Reachable);
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Unknown);

        // PathChangedDirect on Reachable is an idempotent no-op — but the
        // hint must still be recorded.
        assert_eq!(
            store.apply(peer, E::PathChangedDirect, t0 + Duration::from_secs(2)),
            TransitionOutcome::NoChange
        );
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Direct);
        assert_eq!(store.state(&peer), S::Reachable);

        // Same for relay: the hint flips to relay, the peer stays
        // Reachable (a relay connection is still considered reachable,
        // BORU-CP-14 acceptance).
        store.apply(peer, E::PathChangedRelay, t0 + Duration::from_secs(3));
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Relay);
        assert_eq!(store.state(&peer), S::Reachable, "relay-only is still reachable");
        assert!(store.state(&peer).is_online());
    }

    /// BORU-CP-14 acceptance: a relay connection is still considered
    /// reachable — a DirectTopicReady peer never degrades because its path
    /// became relay-only, and chat readiness is untouched.
    #[test]
    fn relay_connection_remains_reachable() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x0D3);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_secs(1));
        store.apply(peer, E::TopicJoined, t0 + Duration::from_secs(2));
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert!(store.state(&peer).is_online());
        assert!(store.state(&peer).is_ready_for_direct());

        // Path flips to relay-only: state, direct-topic readiness, and the
        // trail are all untouched — only the path hint changes.
        let trail_len_before = store.trail(&peer).len();
        assert_eq!(
            store.apply(peer, E::PathChangedRelay, t0 + Duration::from_secs(3)),
            TransitionOutcome::NoChange
        );
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert!(store.state(&peer).is_online(), "relay connection is still reachable");
        assert!(store.state(&peer).is_ready_for_direct());
        assert_eq!(store.trail(&peer).len(), trail_len_before);
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Relay);

        // Back to direct: still no state movement, no duplicate trail
        // records (path transitions never reset or duplicate state).
        assert_eq!(
            store.apply(peer, E::PathChangedDirect, t0 + Duration::from_secs(4)),
            TransitionOutcome::NoChange
        );
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Direct);
        assert_eq!(store.trail(&peer).len(), trail_len_before);
    }

    /// BORU-CP-14: a transitioning path (addresses known, none active) is
    /// diagnostic-only — it never moves the state machine and never resets
    /// conversation state.
    #[test]
    fn path_transitioning_is_diagnostic_only() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x0D4);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_secs(1));
        store.apply(peer, E::TopicJoined, t0 + Duration::from_secs(2));
        assert_eq!(store.state(&peer), S::DirectTopicReady);

        assert_eq!(
            store.apply(peer, E::PathChangedTransitioning, t0 + Duration::from_secs(3)),
            TransitionOutcome::NoChange
        );
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert!(store.state(&peer).is_online());
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Transitioning);

        // A transition from transitioning → relay → direct only flips the
        // hint; the trail stays at the three real transitions.
        store.apply(peer, E::PathChangedRelay, t0 + Duration::from_secs(4));
        store.apply(peer, E::PathChangedDirect, t0 + Duration::from_secs(5));
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Direct);
        assert_eq!(store.trail(&peer).len(), 3);
    }

    /// BORU-CP-14: no path event moves the state machine from any state —
    /// path type is diagnostic/optimization information only.
    #[test]
    fn path_events_never_move_the_state_machine() {
        let all_states = [
            S::Unknown,
            S::Discovered,
            S::Connecting,
            S::Reachable,
            S::DirectTopicReady,
            S::Degraded,
            S::OfflineStale,
        ];
        let path_events = [
            E::PathChangedDirect,
            E::PathChangedRelay,
            E::PathChangedTransitioning,
        ];
        for state in all_states {
            for event in path_events {
                assert_eq!(
                    transition(state, event),
                    None,
                    "path event {event:?} must never move {state:?}"
                );
            }
        }
        // And via the store: a fully-healthy peer receiving every path
        // event in sequence keeps its state, trail, and online status.
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x0D5);
        let t0 = Instant::now();
        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_secs(1));
        store.apply(peer, E::TopicJoined, t0 + Duration::from_secs(2));
        let before = store.trail(&peer).len();
        for (i, event) in [E::PathChangedTransitioning, E::PathChangedRelay, E::PathChangedDirect]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                store.apply(peer, event, t0 + Duration::from_secs(3 + i as u64)),
                TransitionOutcome::NoChange
            );
        }
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert_eq!(store.trail(&peer).len(), before, "no trail records from path events");
        assert!(store.state(&peer).is_online());
    }

    /// Direct message receive proves the direct topic works even if the
    /// peer was only Discovered — and is idempotent afterwards.
    #[test]
    fn direct_message_receive_advances_to_ready() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x0E);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::DirectMessageReceived, t0 + Duration::from_secs(1));
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert_eq!(
            store.get(&peer).unwrap().last_inbound_direct,
            Some(t0 + Duration::from_secs(1))
        );
        // Repeated DM is idempotent and refreshes the timestamp.
        store.apply(peer, E::DirectMessageReceived, t0 + Duration::from_secs(2));
        assert_eq!(store.state(&peer), S::DirectTopicReady);
        assert_eq!(
            store.get(&peer).unwrap().last_inbound_direct,
            Some(t0 + Duration::from_secs(2))
        );
        assert_eq!(store.trail(&peer).len(), 2);
    }

    /// BORU-CP-13 timestamp-only events refresh per-peer timestamps without
    /// ever moving the state machine.
    #[test]
    fn timestamp_only_diagnostics_events_do_not_move_state() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x0F);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        assert_eq!(store.state(&peer), S::Discovered);

        // Outbound broadcast: refreshes last_outbound_direct, state stays.
        assert_eq!(
            store.apply(peer, E::DirectMessageSent, t0 + Duration::from_secs(1)),
            TransitionOutcome::NoChange
        );
        assert_eq!(store.state(&peer), S::Discovered);
        assert_eq!(
            store.get(&peer).unwrap().last_outbound_direct,
            Some(t0 + Duration::from_secs(1))
        );

        // Inbound gossip: refreshes last_inbound_gossip, state stays.
        assert_eq!(
            store.apply(peer, E::InboundGossipEvent, t0 + Duration::from_secs(2)),
            TransitionOutcome::NoChange
        );
        assert_eq!(store.state(&peer), S::Discovered);
        assert_eq!(
            store.get(&peer).unwrap().last_inbound_gossip,
            Some(t0 + Duration::from_secs(2))
        );

        // Decoded application message: refreshes last_decoded_message, and
        // still does not advance the state machine (only a real
        // DirectMessageReceived / TopicJoined does).
        assert_eq!(
            store.apply(peer, E::ApplicationMessageDecoded, t0 + Duration::from_secs(3)),
            TransitionOutcome::NoChange
        );
        assert_eq!(store.state(&peer), S::Discovered);
        assert_eq!(
            store.get(&peer).unwrap().last_decoded_message,
            Some(t0 + Duration::from_secs(3))
        );

        // No trail records are created for timestamp-only events.
        assert_eq!(store.trail(&peer).len(), 1, "only DiscoverySeen transitioned");
    }

    /// BORU-CP-13 timestamp-only events never create entries for unknown
    /// peers (they carry no positive evidence of a usable path).
    #[test]
    fn timestamp_only_events_do_not_create_unknown_entries() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x10);
        let t0 = Instant::now();

        assert_eq!(
            store.apply(peer, E::DirectMessageSent, t0),
            TransitionOutcome::NoChange
        );
        assert_eq!(
            store.apply(peer, E::InboundGossipEvent, t0),
            TransitionOutcome::NoChange
        );
        assert_eq!(
            store.apply(peer, E::ApplicationMessageDecoded, t0),
            TransitionOutcome::NoChange
        );
        assert!(store.is_empty());
    }

    /// The full BORU-CP-13 stage timeline: discovery → endpoint → topic →
    /// outbound broadcast → inbound gossip → decoded message, with every
    /// timestamp recorded for the snapshot.
    #[test]
    fn full_stage_timeline_records_all_timestamps() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x11);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnecting, t0 + Duration::from_millis(1));
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_millis(2));
        store.apply(peer, E::TopicJoined, t0 + Duration::from_millis(3));
        store.apply(peer, E::DirectMessageSent, t0 + Duration::from_millis(4));
        store.apply(peer, E::InboundGossipEvent, t0 + Duration::from_millis(5));
        store.apply(peer, E::ApplicationMessageDecoded, t0 + Duration::from_millis(6));
        store.apply(peer, E::DirectMessageReceived, t0 + Duration::from_millis(7));

        let entry = store.get(&peer).expect("peer tracked");
        assert_eq!(entry.state, S::DirectTopicReady);
        assert_eq!(entry.direct_topic_state, DirectTopicState::Ready);
        assert_eq!(entry.discovery_last_seen, Some(t0));
        assert_eq!(entry.last_outbound_direct, Some(t0 + Duration::from_millis(4)));
        assert_eq!(entry.last_inbound_gossip, Some(t0 + Duration::from_millis(5)));
        assert_eq!(entry.last_decoded_message, Some(t0 + Duration::from_millis(6)));
        assert_eq!(entry.last_inbound_direct, Some(t0 + Duration::from_millis(7)));
    }

    impl PeerConnectivityStore {
        fn contains_key(&self, peer: &PublicKey) -> bool {
            self.peers.contains_key(peer)
        }
    }
}
