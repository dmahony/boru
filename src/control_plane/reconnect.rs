//! Reconnection scheduler and signalling (PDF Phase 3, Task 3.1 /
//! BORU-CP-07).
//!
//! Automatic reconnection: a fresh discovery announcement is a reason to
//! re-establish required direct communication state. This module owns the
//! per-peer reconnect queue with **exponential backoff and a maximum retry
//! cadence**, the dedup guarantee (at most one active attempt per peer),
//! and the signal the data plane consumes to re-join the deterministic
//! direct topic.
//!
//! # Design rules (PDF Task 3.1 + cross-cutting guardrails)
//!
//! * **Re-use the existing connection path.** The reconnect loop dials via
//!   [`GossipSender::join_peers`] — the same authenticated Iroh
//!   endpoint/address mechanism mDNS/DHT and the BORU-DISC-11 connectivity
//!   wiring use. No second transport is invented.
//! * **Discovery is a trigger, never proof of message-path recovery.** A
//!   fresh announcement only *queues* an attempt. The [`ReconnectSignal`]
//!   is emitted only after a real successful dial, and retry/backoff state
//!   is cleared only by a real success event (endpoint connected, direct
//!   topic ready, direct message received). Discovery traffic alone never
//!   clears backoff and never emits the signal.
//! * **One active attempt per peer.** [`ReconnectScheduler::schedule`] is
//!   deduplicated: repeated announcements while an attempt is queued or in
//!   flight are no-ops. [`ReconnectScheduler::due`] marks attempts in
//!   flight under the same lock that selects them, so a concurrent
//!   announcement cannot spawn a second attempt.
//! * **No authorisation by presence.** Friendship stays in the app layer:
//!   the app decides which peers are reconnect-eligible
//!   ([`ReconnectHandle::queue_reconnect`] is called by the app for known
//!   friends). The data plane decides whether to (re)join the direct topic
//!   after a [`ReconnectSignal::PeerReachable`] — the discovery service
//!   never joins conversation topics itself (deterministic topic
//!   ownership; no control-plane/chat coupling).
//! * **Bounded resources.** The scheduler is capped at
//!   [`MAX_RECONNECT_PEERS`] and evicts the furthest-future not-in-flight
//!   entry when full.
//!
//! # Backoff
//!
//! The first attempt is immediate. After the Nth failure the next attempt
//! is scheduled at `initial_backoff * 2^(N-1)`, capped at
//! [`DEFAULT_RECONNECT_MAX_BACKOFF`] (the maximum retry cadence — repeated
//! failures never retry faster than this cap). A real success or presence
//! expiry clears the entry entirely, so the next fresh announcement starts
//! from an immediate attempt again.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh_base::PublicKey;
use tracing::{info, trace};

use crate::control_plane::connectivity::{ConnectivityEvent, PeerConnectivityStore};

/// Default initial backoff between reconnect attempts.
pub const DEFAULT_RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_secs(2);

/// Default maximum retry cadence: repeated failures never retry faster
/// than this interval, whatever the attempt count.
pub const DEFAULT_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Maximum number of peers with queued/in-flight reconnect state. Beyond
/// this the scheduler evicts the furthest-future not-in-flight entry
/// (bounded memory — a malicious discovery flood cannot grow the queue).
pub const MAX_RECONNECT_PEERS: usize = 256;

/// Snapshot of a peer's reconnect state (for diagnostics and tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectState {
    /// Number of completed failed attempts (0 = fresh queue).
    pub attempts: u32,
    /// Whether an attempt is currently in flight.
    pub in_flight: bool,
}

/// Per-peer reconnect entry.
#[derive(Debug, Clone)]
struct ReconnectEntry {
    /// Completed failed attempts (drives the exponential backoff).
    attempts: u32,
    /// When the next attempt is due.
    next_attempt_at: Instant,
    /// Whether an attempt is currently in flight (dedup anchor).
    in_flight: bool,
}

/// Pure, testable reconnection scheduler.
///
/// Owns no network state: it records who is queued for a reconnect attempt,
/// when the next attempt is due, and how many failures have accumulated.
/// The caller (the discovery-service reconnect loop) performs the actual
/// dials and feeds results back through
/// [`on_failure`](Self::on_failure) / [`reset`](Self::reset).
#[derive(Debug, Clone)]
pub struct ReconnectScheduler {
    peers: HashMap<PublicKey, ReconnectEntry>,
    max_peers: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl Default for ReconnectScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconnectScheduler {
    /// An empty scheduler with the default backoff and peer limits.
    pub fn new() -> Self {
        Self::with_limits(
            DEFAULT_RECONNECT_INITIAL_BACKOFF,
            DEFAULT_RECONNECT_MAX_BACKOFF,
            MAX_RECONNECT_PEERS,
        )
    }

    /// An empty scheduler with explicit limits (tests use small caps and
    /// short backoffs).
    pub fn with_limits(
        initial_backoff: Duration,
        max_backoff: Duration,
        max_peers: usize,
    ) -> Self {
        let initial_backoff = initial_backoff.max(Duration::ZERO);
        Self {
            peers: HashMap::new(),
            max_peers: max_peers.max(1),
            initial_backoff,
            max_backoff: max_backoff.max(initial_backoff),
        }
    }

    /// Update the backoff policy (builder/tests). `max` is clamped to at
    /// least `initial`.
    pub fn set_backoff(&mut self, initial: Duration, max: Duration) {
        self.initial_backoff = initial.max(Duration::ZERO);
        self.max_backoff = max.max(self.initial_backoff);
    }

    /// Exponential backoff for a peer that has already failed `attempts`
    /// times: `initial * 2^(attempts-1)`, capped at the maximum retry
    /// cadence. Attempt 0 (first queue) is immediate (zero delay).
    pub fn backoff_for(&self, attempts: u32) -> Duration {
        if attempts == 0 {
            return Duration::ZERO;
        }
        let shift = (attempts - 1).min(30);
        let multiplier = 1u64 << shift;
        let initial_ms = self.initial_backoff.as_millis().max(1) as u64;
        let max_ms = self.max_backoff.as_millis().max(1) as u64;
        Duration::from_millis(initial_ms.saturating_mul(multiplier).min(max_ms))
    }

    /// Queue an immediate reconnect attempt for `peer`.
    ///
    /// Deduplicated: at most one queued/in-flight entry per peer, so several
    /// discovery messages cannot queue duplicates (PDF Task 3.1 step 4).
    /// Returns `true` when a fresh attempt was queued, `false` when one is
    /// already queued or in flight.
    pub fn schedule(&mut self, peer: PublicKey, now: Instant) -> bool {
        if self.peers.contains_key(&peer) {
            return false;
        }
        if self.peers.len() >= self.max_peers {
            self.evict_one();
        }
        self.peers.insert(
            peer,
            ReconnectEntry {
                attempts: 0,
                next_attempt_at: now,
                in_flight: false,
            },
        );
        true
    }

    /// Peers due for an attempt at `now`: queued, not in flight, deadline
    /// reached. Marks each due peer **in flight** before returning, so a
    /// concurrent announcement or schedule cannot spawn a second attempt
    /// for the same peer (acceptance criterion: only one active reconnection
    /// attempt per peer).
    pub fn due(&mut self, now: Instant) -> Vec<PublicKey> {
        let mut due = Vec::new();
        for (peer, entry) in self.peers.iter_mut() {
            if !entry.in_flight && entry.next_attempt_at <= now {
                entry.in_flight = true;
                due.push(*peer);
            }
        }
        due
    }

    /// Record a failed attempt: the peer backs off exponentially
    /// (`initial * 2^(attempts-1)`, capped at the maximum retry cadence)
    /// before the next attempt.
    pub fn on_failure(&mut self, peer: &PublicKey, now: Instant) {
        // Compute the next backoff from the CURRENT attempts count before
        // mutating the entry (avoids overlapping borrows of `self`).
        let next_attempts = self
            .peers
            .get(peer)
            .map(|entry| entry.attempts.saturating_add(1))
            .unwrap_or(1);
        let backoff = self.backoff_for(next_attempts);
        let Some(entry) = self.peers.get_mut(peer) else {
            return;
        };
        entry.attempts = next_attempts;
        entry.in_flight = false;
        entry.next_attempt_at = now + backoff;
        trace!(
            peer = %peer.fmt_short(),
            attempts = entry.attempts,
            backoff_ms = backoff.as_millis() as u64,
            "reconnect: attempt failed, backed off",
        );
    }

    /// Clear a peer's retry/backoff state entirely.
    ///
    /// Called on a **real** success (endpoint connected, direct topic ready,
    /// direct message received) or on presence expiry (the peer went
    /// offline — a later fresh announcement starts from an immediate attempt
    /// again). Never called for discovery announcements alone.
    pub fn reset(&mut self, peer: &PublicKey) {
        if self.peers.remove(peer).is_some() {
            trace!(peer = %peer.fmt_short(), "reconnect: retry/backoff state cleared");
        }
    }

    /// Whether a reconnect attempt for `peer` is queued or in flight.
    pub fn is_queued(&self, peer: &PublicKey) -> bool {
        self.peers.contains_key(peer)
    }

    /// Whether an attempt for `peer` is currently in flight.
    pub fn is_in_flight(&self, peer: &PublicKey) -> bool {
        self.peers
            .get(peer)
            .map(|entry| entry.in_flight)
            .unwrap_or(false)
    }

    /// Snapshot of a peer's reconnect state, if queued.
    pub fn state(&self, peer: &PublicKey) -> Option<ReconnectState> {
        self.peers.get(peer).map(|entry| ReconnectState {
            attempts: entry.attempts,
            in_flight: entry.in_flight,
        })
    }

    /// Iterate over all queued peers (for diagnostics/tests).
    pub fn peers(&self) -> impl Iterator<Item = (&PublicKey, ReconnectState)> {
        self.peers.iter().map(|(peer, entry)| {
            (
                peer,
                ReconnectState {
                    attempts: entry.attempts,
                    in_flight: entry.in_flight,
                },
            )
        })
    }

    /// Number of queued/in-flight peers.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether no peer is queued.
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Remove every queued entry.
    pub fn clear(&mut self) {
        self.peers.clear();
    }

    /// Evict one entry when at capacity: prefer a not-in-flight entry with
    /// the furthest-future deadline (the least urgent attempt); fall back to
    /// the oldest entry if everything is in flight.
    fn evict_one(&mut self) {
        let victim = self
            .peers
            .iter()
            .filter(|(_, entry)| !entry.in_flight)
            .max_by_key(|(_, entry)| entry.next_attempt_at)
            .map(|(peer, _)| *peer)
            .or_else(|| {
                self.peers
                    .iter()
                    .min_by_key(|(_, entry)| entry.next_attempt_at)
                    .map(|(peer, _)| *peer)
            });
        if let Some(peer) = victim {
            self.peers.remove(&peer);
        }
    }
}

/// Signals emitted by the reconnection machinery for the data plane to
/// consume (PDF Task 3.1 step 3: re-join/subscribe the deterministic direct
/// topic after connectivity is re-established).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectSignal {
    /// The endpoint for `peer` was re-established — a reconnect attempt
    /// succeeded via the existing authenticated Iroh connection path.
    ///
    /// Emitted ONLY after a real successful dial. Discovery announcements
    /// alone never produce this signal (a fresh announcement only queues an
    /// attempt). The data plane owns the deterministic direct topic and
    /// decides whether to (re)join it; the discovery service never joins
    /// conversation topics itself.
    PeerReachable {
        /// The peer whose endpoint connectivity was re-established.
        peer: PublicKey,
    },
}

/// A cloneable handle for the reconnection subsystem, safe to hand to the
/// app layer (mirrors [`crate::discovery_service::DiscoveryJoiner`]).
///
/// The app uses it to:
///
/// * [`queue_reconnect`](Self::queue_reconnect) — the fresh-announcement
///   trigger for a **known friend** (the app owns friendship; this handle
///   never decides friend-ness).
/// * [`report_topic_ready`](Self::report_topic_ready) — report a REAL
///   direct-topic success from the data plane, which clears retry/backoff
///   state (acceptance criterion).
///
/// The handle never touches conversation/chat state and never joins topics;
/// it only feeds the control-plane scheduler and connectivity state machine.
#[derive(Clone, Debug)]
pub struct ReconnectHandle {
    scheduler: Arc<std::sync::Mutex<ReconnectScheduler>>,
    connectivity: Arc<std::sync::Mutex<PeerConnectivityStore>>,
}

impl ReconnectHandle {
    /// Create a handle sharing the given scheduler and connectivity store.
    pub fn new(
        scheduler: Arc<std::sync::Mutex<ReconnectScheduler>>,
        connectivity: Arc<std::sync::Mutex<PeerConnectivityStore>>,
    ) -> Self {
        Self {
            scheduler,
            connectivity,
        }
    }

    /// Queue ONE reconnection attempt for `peer` (PDF Task 3.1 step 1).
    ///
    /// Deduplicated: repeated announcements while an attempt is queued or in
    /// flight are no-ops (PDF Task 3.1 step 4). No-op when the peer is
    /// already online (`Reachable` / `DirectTopicReady`) — there is nothing
    /// to reconnect. Returns `true` when a fresh attempt was queued.
    pub fn queue_reconnect(&self, peer: PublicKey) -> bool {
        let online = {
            let store = self
                .connectivity
                .lock()
                .expect("connectivity store lock poisoned");
            store.state(&peer).is_online()
        };
        if online {
            trace!(peer = %peer.fmt_short(), "reconnect: peer already online, skipping queue");
            return false;
        }
        let mut scheduler = self
            .scheduler
            .lock()
            .expect("reconnect scheduler lock poisoned");
        let queued = scheduler.schedule(peer, Instant::now());
        if queued {
            info!(peer = %peer.fmt_short(), "reconnect: queued reconnection attempt for friend");
        } else {
            trace!(peer = %peer.fmt_short(), "reconnect: attempt already queued (dedup)");
        }
        queued
    }

    /// Report a REAL direct-topic success from the data plane.
    ///
    /// Advances the peer's connectivity state machine to
    /// [`PeerConnectivityState::DirectTopicReady`] (the deterministic direct
    /// topic was joined/subscribed or a direct message flowed) and clears
    /// any retry/backoff state (PDF Task 3.1 step 6 + acceptance criterion
    /// "successful direct-topic readiness clears retry/backoff state").
    ///
    /// Discovery announcements alone must NEVER call this — a mere
    /// announcement is not message-path recovery.
    pub fn report_topic_ready(&self, peer: PublicKey) {
        {
            let mut store = self
                .connectivity
                .lock()
                .expect("connectivity store lock poisoned");
            store.apply(peer, ConnectivityEvent::TopicJoined, Instant::now());
        }
        {
            let mut scheduler = self
                .scheduler
                .lock()
                .expect("reconnect scheduler lock poisoned");
            scheduler.reset(&peer);
        }
        info!(peer = %peer.fmt_short(), "reconnect: direct-topic readiness clears retry/backoff");
    }

    /// Whether a reconnect attempt for `peer` is queued or in flight.
    pub fn is_reconnect_pending(&self, peer: &PublicKey) -> bool {
        self.scheduler
            .lock()
            .expect("reconnect scheduler lock poisoned")
            .is_queued(peer)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    /// A fresh queue schedules an immediate attempt; a duplicate schedule is
    /// a no-op (dedup — several discovery messages queue one attempt).
    #[test]
    fn schedule_deduplicates_per_peer() {
        let mut scheduler = ReconnectScheduler::new();
        let peer = key(0x01);
        let t0 = Instant::now();

        assert!(scheduler.schedule(peer, t0), "first queue must succeed");
        assert!(
            !scheduler.schedule(peer, t0 + Duration::from_secs(1)),
            "duplicate queue must be a no-op"
        );
        assert!(
            !scheduler.schedule(peer, t0 + Duration::from_secs(5)),
            "later announcements must not queue duplicates"
        );
        assert_eq!(scheduler.len(), 1);
        assert_eq!(
            scheduler.state(&peer),
            Some(ReconnectState {
                attempts: 0,
                in_flight: false,
            })
        );
    }

    /// The first attempt is immediate; `due` marks it in flight; a second
    /// `due` returns nothing until the attempt completes.
    #[test]
    fn due_marks_in_flight_and_deduplicates() {
        let mut scheduler = ReconnectScheduler::new();
        let peer = key(0x02);
        let t0 = Instant::now();

        scheduler.schedule(peer, t0);
        assert_eq!(scheduler.due(t0), vec![peer], "due at/after deadline");
        assert_eq!(
            scheduler.state(&peer),
            Some(ReconnectState {
                attempts: 0,
                in_flight: true,
            })
        );
        assert!(
            scheduler.due(t0 + Duration::from_secs(60)).is_empty(),
            "an in-flight attempt is never due twice"
        );
        // A duplicate schedule while in flight is still a no-op.
        assert!(!scheduler.schedule(peer, t0 + Duration::from_secs(60)));
    }

    /// Exponential backoff with a maximum retry cadence.
    #[test]
    fn backoff_grows_exponentially_and_caps() {
        let scheduler = ReconnectScheduler::with_limits(
            Duration::from_secs(2),
            Duration::from_secs(300),
            16,
        );
        assert_eq!(scheduler.backoff_for(0), Duration::ZERO);
        assert_eq!(scheduler.backoff_for(1), Duration::from_secs(2));
        assert_eq!(scheduler.backoff_for(2), Duration::from_secs(4));
        assert_eq!(scheduler.backoff_for(3), Duration::from_secs(8));
        assert_eq!(scheduler.backoff_for(4), Duration::from_secs(16));
        assert_eq!(scheduler.backoff_for(8), Duration::from_secs(256));
        assert_eq!(
            scheduler.backoff_for(9),
            Duration::from_secs(300),
            "attempt 9 would be 512s but the max retry cadence caps at 300s"
        );
        assert_eq!(
            scheduler.backoff_for(100),
            Duration::from_secs(300),
            "repeated failures never retry faster than the cap"
        );
    }

    /// Repeated failures back off on the scheduler: each failure pushes the
    /// next attempt further out, capped at the max cadence.
    #[test]
    fn failures_back_off_with_cap() {
        let mut scheduler = ReconnectScheduler::with_limits(
            Duration::from_secs(2),
            Duration::from_secs(300),
            16,
        );
        let peer = key(0x03);
        let t0 = Instant::now();
        scheduler.schedule(peer, t0);

        // Attempt 1 fails → next at t0+2s (initial backoff).
        let due1 = scheduler.due(t0);
        assert_eq!(due1, vec![peer]);
        scheduler.on_failure(&peer, t0);
        assert_eq!(scheduler.state(&peer).unwrap().attempts, 1);
        assert!(!scheduler.state(&peer).unwrap().in_flight);

        // Not due before the first backoff deadline; due exactly at it.
        assert!(
            scheduler.due(t0 + Duration::from_secs(1)).is_empty(),
            "backed-off peer must not retry before its deadline"
        );
        assert!(scheduler.due(t0 + Duration::from_secs(2)).contains(&peer));

        // Attempt 2 fails at t0+2s → next at t0+6s (2s * 2^(2-1) = 4s later).
        scheduler.on_failure(&peer, t0 + Duration::from_secs(2));
        assert_eq!(scheduler.state(&peer).unwrap().attempts, 2);
        assert!(
            scheduler.due(t0 + Duration::from_secs(5)).is_empty(),
            "backed-off peer must not retry before its deadline"
        );
        assert!(scheduler.due(t0 + Duration::from_secs(6)).contains(&peer));

        // Attempt 3 fails at t0+6s → next at t0+14s (8s later).
        scheduler.on_failure(&peer, t0 + Duration::from_secs(6));
        assert_eq!(scheduler.state(&peer).unwrap().attempts, 3);

        // Drive enough failures to saturate the max retry cadence: each
        // iteration advances `now` to the next backoff deadline and fails.
        let mut now = t0 + Duration::from_secs(6);
        for _ in 0..12 {
            let attempts = scheduler.state(&peer).unwrap().attempts;
            now += scheduler.backoff_for(attempts + 1);
            assert!(
                scheduler.due(now).contains(&peer),
                "peer must be due exactly at its backoff deadline"
            );
            scheduler.on_failure(&peer, now);
        }
        assert_eq!(
            scheduler.backoff_for(scheduler.state(&peer).unwrap().attempts),
            Duration::from_secs(300),
            "backoff must saturate at the max retry cadence"
        );
    }

    /// A real success clears the retry/backoff state entirely; the next
    /// fresh announcement starts from an immediate attempt (no residual
    /// backoff).
    #[test]
    fn success_resets_backoff_state() {
        let mut scheduler = ReconnectScheduler::with_limits(
            Duration::from_secs(2),
            Duration::from_secs(300),
            16,
        );
        let peer = key(0x04);
        let t0 = Instant::now();
        scheduler.schedule(peer, t0);
        scheduler.due(t0);
        scheduler.on_failure(&peer, t0);
        scheduler.on_failure(&peer, t0 + Duration::from_secs(2));
        assert!(scheduler.is_queued(&peer));

        scheduler.reset(&peer);
        assert!(!scheduler.is_queued(&peer));
        assert!(scheduler.is_empty());

        // A fresh announcement after the success queues an immediate attempt
        // (attempts back at 0 — no residual backoff).
        assert!(scheduler.schedule(peer, t0 + Duration::from_secs(10)));
        assert_eq!(
            scheduler.state(&peer),
            Some(ReconnectState {
                attempts: 0,
                in_flight: false,
            })
        );
    }

    /// The scheduler is bounded: at capacity the least-urgent not-in-flight
    /// entry is evicted.
    #[test]
    fn scheduler_is_bounded() {
        let mut scheduler = ReconnectScheduler::with_limits(
            Duration::from_secs(2),
            Duration::from_secs(300),
            2,
        );
        let t0 = Instant::now();
        let a = key(0x10);
        let b = key(0x11);
        let c = key(0x12);

        assert!(scheduler.schedule(a, t0));
        assert!(scheduler.schedule(b, t0));
        assert_eq!(scheduler.len(), 2);
        // Third peer evicts one entry; the queue stays at the cap.
        assert!(scheduler.schedule(c, t0 + Duration::from_secs(1)));
        assert_eq!(scheduler.len(), 2);
        assert!(scheduler.is_queued(&c), "the newest entry survives");
    }

    /// `queue_reconnect` on the handle dedups and skips online peers.
    #[test]
    fn handle_queue_reconnect_skips_online_peers() {
        let scheduler = Arc::new(std::sync::Mutex::new(ReconnectScheduler::new()));
        let connectivity = Arc::new(std::sync::Mutex::new(PeerConnectivityStore::new()));
        let handle = ReconnectHandle::new(scheduler.clone(), connectivity.clone());
        let peer = key(0x20);

        // Unknown peer: queued.
        assert!(handle.queue_reconnect(peer));
        assert!(handle.is_reconnect_pending(&peer));

        // Duplicate queue: no-op.
        assert!(!handle.queue_reconnect(peer));
        assert!(handle.is_reconnect_pending(&peer));

        // Peer becomes online: queue is skipped.
        scheduler.lock().unwrap().reset(&peer);
        connectivity
            .lock()
            .unwrap()
            .apply(peer, ConnectivityEvent::EndpointConnected, Instant::now());
        assert!(
            !handle.queue_reconnect(peer),
            "an online peer must not be queued for reconnection"
        );
    }

    /// `report_topic_ready` advances the state machine to
    /// `DirectTopicReady` and clears queued retry state — a real
    /// message-path success, not a discovery announcement.
    #[test]
    fn handle_report_topic_ready_clears_retry_and_advances_state() {
        use crate::control_plane::connectivity::PeerConnectivityState;

        let scheduler = Arc::new(std::sync::Mutex::new(ReconnectScheduler::new()));
        let connectivity = Arc::new(std::sync::Mutex::new(PeerConnectivityStore::new()));
        let handle = ReconnectHandle::new(scheduler.clone(), connectivity.clone());
        let peer = key(0x21);

        assert!(handle.queue_reconnect(peer));
        assert!(handle.is_reconnect_pending(&peer));

        handle.report_topic_ready(peer);

        assert!(
            !handle.is_reconnect_pending(&peer),
            "direct-topic readiness must clear retry/backoff state"
        );
        assert_eq!(
            connectivity.lock().unwrap().state(&peer),
            PeerConnectivityState::DirectTopicReady
        );
    }
}
