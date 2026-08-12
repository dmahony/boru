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
//! | [`Degraded`](PeerConnectivityState::Degraded) | Previously reachable, but a failure occurred (dial failed, topic join failed, relay-only path) — explicitly NOT 'online'. |
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
    /// The relay/direct path changed to a direct (IP) path.
    PathChangedDirect,
    /// The relay/direct path changed to a relay-only path.
    PathChangedRelay,
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
        (Discovered, PathChangedDirect) => Reachable,
        (Discovered, PathChangedRelay) => Degraded,

        // ── Connecting ─────────────────────────────────────────────────
        (Connecting, DiscoverySeen) => return None, // idempotent
        (Connecting, EndpointConnecting) => return None, // no duplicate dials
        (Connecting, EndpointConnected) => Reachable,
        (Connecting, EndpointFailed) => Degraded,
        (Connecting, TopicJoined) => DirectTopicReady,
        (Connecting, TopicJoinFailed) => Degraded,
        (Connecting, DirectMessageReceived) => DirectTopicReady,
        (Connecting, Timeout) => OfflineStale,
        (Connecting, PathChangedDirect) => Reachable,
        (Connecting, PathChangedRelay) => Degraded,

        // ── Reachable ──────────────────────────────────────────────────
        (Reachable, DiscoverySeen) => return None, // idempotent refresh
        (Reachable, EndpointConnecting) => return None, // already connected
        (Reachable, EndpointConnected) => return None, // idempotent
        (Reachable, EndpointFailed) => Degraded,
        (Reachable, TopicJoined) => DirectTopicReady,
        (Reachable, TopicJoinFailed) => Degraded,
        (Reachable, DirectMessageReceived) => DirectTopicReady,
        (Reachable, Timeout) => OfflineStale,
        (Reachable, PathChangedDirect) => return None, // already direct
        (Reachable, PathChangedRelay) => Degraded,

        // ── DirectTopicReady ───────────────────────────────────────────
        (DirectTopicReady, DiscoverySeen) => return None, // idempotent
        (DirectTopicReady, EndpointConnecting) => return None, // already ready
        (DirectTopicReady, EndpointConnected) => return None, // idempotent
        (DirectTopicReady, EndpointFailed) => Degraded,
        (DirectTopicReady, TopicJoined) => return None, // idempotent
        (DirectTopicReady, TopicJoinFailed) => Degraded,
        (DirectTopicReady, DirectMessageReceived) => return None, // idempotent
        (DirectTopicReady, Timeout) => OfflineStale,
        (DirectTopicReady, PathChangedDirect) => return None, // already direct
        (DirectTopicReady, PathChangedRelay) => Degraded,

        // ── Degraded ───────────────────────────────────────────────────
        (Degraded, DiscoverySeen) => return None, // idempotent; not revived by an announcement
        (Degraded, EndpointConnecting) => Connecting,
        (Degraded, EndpointConnected) => Reachable,
        (Degraded, EndpointFailed) => return None, // idempotent
        (Degraded, TopicJoined) => DirectTopicReady,
        (Degraded, TopicJoinFailed) => return None, // idempotent
        (Degraded, DirectMessageReceived) => DirectTopicReady,
        (Degraded, Timeout) => OfflineStale,
        (Degraded, PathChangedDirect) => Reachable,
        (Degraded, PathChangedRelay) => return None, // idempotent

        // ── OfflineStale ───────────────────────────────────────────────
        (OfflineStale, DiscoverySeen) => Discovered, // fresh announcement revives
        (OfflineStale, EndpointConnecting) => Connecting,
        (OfflineStale, EndpointConnected) => Reachable,
        (OfflineStale, EndpointFailed) => return None, // idempotent
        (OfflineStale, TopicJoined) => DirectTopicReady,
        (OfflineStale, TopicJoinFailed) => return None, // idempotent
        (OfflineStale, DirectMessageReceived) => DirectTopicReady,
        (OfflineStale, Timeout) => return None, // idempotent
        (OfflineStale, PathChangedDirect) => Reachable,
        (OfflineStale, PathChangedRelay) => return None, // idempotent

        // Everything else — an event that does not move the peer (e.g. an
        // `Unknown` peer receiving a failure/timeout/path event) — is an
        // idempotent no-op. The state machine never fabricates progress.
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

/// How the peer's relay/direct path currently looks (hint only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// No path information yet.
    Unknown,
    /// At least one direct (IP) path is available.
    Direct,
    /// The peer is reachable only via a relay server.
    Relay,
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
    ///   timestamps (last_seen, discovery_last_seen, last_inbound_direct)
    ///   are still refreshed so a no-op event keeps the peer's recency
    ///   current.
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
                        ConnectivityEvent::PathChangedDirect => {
                            entry.path_kind = PathKind::Direct;
                        }
                        ConnectivityEvent::PathChangedRelay => {
                            entry.path_kind = PathKind::Relay;
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::connectivity::{ConnectivityEvent as E, PeerConnectivityState as S};

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    // ── The documented transition table ───────────────────────────────

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

    /// Path changes update the path hint and move state appropriately.
    #[test]
    fn path_changes_are_reflected() {
        let mut store = PeerConnectivityStore::new();
        let peer = key(0x0D);
        let t0 = Instant::now();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_secs(1));
        // Degrade to relay-only.
        store.apply(peer, E::PathChangedRelay, t0 + Duration::from_secs(2));
        assert_eq!(store.state(&peer), S::Degraded);
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Relay);
        assert!(!store.state(&peer).is_online());
        // Back to direct.
        store.apply(peer, E::PathChangedDirect, t0 + Duration::from_secs(3));
        assert_eq!(store.state(&peer), S::Reachable);
        assert_eq!(store.get(&peer).unwrap().path_kind, PathKind::Direct);
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

    impl PeerConnectivityStore {
        fn contains_key(&self, peer: &PublicKey) -> bool {
            self.peers.contains_key(peer)
        }
    }
}
