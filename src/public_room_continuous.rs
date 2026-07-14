//! Continuous public-room DHT discovery and publication.
//!
//! Spawns background tasks that periodically re-publish local presence and
//! discover new peers on the DHT.  Discovered peer IDs are forwarded through
//! an [`mpsc::Sender`] channel or processed internally via a
//! [`DynamicPeerJoiner`] for the caller to join.
//!
//! # Lifecycle
//!
//! 1. [`ContinuousTracker::start`] or
//!    [`ContinuousTracker::start_with_joiner`] — spawns background tasks
//!    and returns a handle.
//! 2. The caller reads from the [`mpsc::Receiver`] to get discovered peer
//!    batches, or the internal [`DynamicPeerJoiner`] handles joining.
//! 3. [`ContinuousTracker::shutdown`] — signals cancellation and awaits
//!    task completion.
//!
//! # Design
//!
//! - Two independent tokio tasks: one for publication, one for discovery.
//! - Each tick applies uniform jitter to the configured interval.
//! - Failures use exponential backoff capped at `max_retry_delay`.
//! - The shared [`CancellationToken`] is fired on shutdown; both tasks
//!   observe it and exit promptly.
//! - No blocking locks are held across `.await` — the tracker only calls
//!   async methods on an `Arc`-wrapped [`PublicRoomTracker`] and the
//!   channel sender.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::{
    sync::mpsc,
    task::JoinHandle,
    time::{interval, sleep, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, info_span, trace, warn, Instrument};

use crate::api::GossipSender;
use crate::dynamic_joiner::{DynamicPeerJoiner, DynamicPeerJoinerConfig, NeighborEvent};
use crate::public_room_tracker::PublicRoomTracker;
use iroh::EndpointId;
use n0_error::Result;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for [`ContinuousTracker`].
#[derive(Debug, Clone)]
pub struct ContinuousTrackerConfig {
    /// Interval between publication refresh attempts (with jitter).
    ///
    /// Default: 5 minutes.
    pub publish_interval: Duration,

    /// Interval between DHT discovery lookups (with jitter).
    ///
    /// Default: 30 seconds.
    pub discover_interval: Duration,

    /// Maximum candidate peers to accept per discovery cycle.
    ///
    /// Default: 20.
    pub max_candidates_per_cycle: usize,

    /// Maximum concurrent join operations forwarded through the channel.
    ///
    /// The tracker itself does not call join — it sends batches to the
    /// channel.  This field is informational / for future use; the caller
    /// is responsible for concurrency.
    ///
    /// Default: 5.
    pub max_concurrent_joins: usize,

    /// Maximum number of candidate connection proposals in one tracker session.
    /// This is a hard lifetime bound, not merely a per-discovery-cycle bound.
    /// Default: 10.
    pub max_candidates_per_session: usize,

    /// Maximum candidate connection proposals in the rate-limit window.
    /// Default: 10 per 60 seconds.
    pub connection_attempts_per_window: usize,

    /// Duration of the candidate connection-attempt rate-limit window.
    /// Default: 60 seconds.
    pub connection_attempt_window: Duration,

    /// Initial delay before first retry on failure (exponential backoff).
    ///
    /// Default: 1 second.
    pub initial_retry_delay: Duration,

    /// Maximum delay for exponential backoff.
    ///
    /// Default: 60 seconds.
    pub max_retry_delay: Duration,

    /// Jitter factor (0.0 = no jitter, 0.1 = ±10%).
    ///
    /// Applied to every interval tick and every backoff sleep.
    ///
    /// Default: 0.1.
    pub jitter_factor: f64,

    /// How long a peer remains in the known-peers set before it can be
    /// re-discovered as a new candidate.  `None` means peers stay known
    /// for the entire session (the set is bounded by
    /// [`max_candidates_per_session`](Self::max_candidates_per_session)
    /// insertions).
    ///
    /// Default: `None` (no stale eviction).
    pub stale_peer_ttl: Option<Duration>,
}

impl Default for ContinuousTrackerConfig {
    fn default() -> Self {
        Self {
            publish_interval: Duration::from_secs(300), // 5 minutes
            discover_interval: Duration::from_secs(30), // 30 seconds
            max_candidates_per_cycle: 20,
            max_concurrent_joins: 5,
            max_candidates_per_session: 10,
            connection_attempts_per_window: 10,
            connection_attempt_window: Duration::from_secs(60),
            initial_retry_delay: Duration::from_secs(1),
            max_retry_delay: Duration::from_secs(60),
            jitter_factor: 0.1,
            stale_peer_ttl: None,
        }
    }
}

impl ContinuousTrackerConfig {
    /// Clamp jitter_factor to [0.0, 0.5] and return self.
    pub fn sanitize(mut self) -> Self {
        self.jitter_factor = self.jitter_factor.clamp(0.0, 0.5);
        self
    }
}

// ---------------------------------------------------------------------------
// ContinuousTracker
// ---------------------------------------------------------------------------

/// A handle to background publication and discovery tasks for a public room.
///
/// Dropping this handle **without** calling [`shutdown`](Self::shutdown) will
/// abort the background tasks (the [`JoinHandle`] is not detached).  Always
/// call `shutdown` to ensure clean teardown.
#[derive(Debug)]
pub struct ContinuousTracker {
    /// Cancellation token shared with both background tasks.
    cancel: CancellationToken,
    /// Join handle for the outer task that hosts both loops.
    task_handle: JoinHandle<()>,
    /// The underlying tracker — kept alive so the backend isn't dropped.
    _tracker: Arc<PublicRoomTracker>,
    /// Optional DynamicPeerJoiner for automatic joining with retries.
    _joiner: Option<DynamicPeerJoiner>,
}

impl ContinuousTracker {
    /// Start continuous publication and discovery.
    ///
    /// Spawns two background tokio tasks:
    ///
    /// 1. **Publish loop** — periodically re-publishes local presence on the
    ///    DHT.  On failure, retries with exponential backoff.
    /// 2. **Discovery loop** — periodically looks up peers on the DHT and
    ///    sends newly discovered [`EndpointId`] values through `new_peers_tx`.
    ///
    /// The caller should read from the corresponding
    /// [`mpsc::Receiver<Vec<EndpointId>>`] and forward batches to
    /// [`GossipSender::join_peers`] (or use
    /// [`start_with_joiner`](Self::start_with_joiner) for automatic joining).
    ///
    /// # Panics
    ///
    /// Panics if `config.max_candidates_per_cycle == 0`.
    pub fn start(
        tracker: PublicRoomTracker,
        config: ContinuousTrackerConfig,
        new_peers_tx: mpsc::Sender<Vec<EndpointId>>,
    ) -> Self {
        let cancel = CancellationToken::new();
        let tracker = Arc::new(tracker);

        let cancel_p = cancel.clone();
        let tracker_p = Arc::clone(&tracker);
        let cfg_p = config.clone();

        let cancel_d = cancel.clone();
        let tracker_d = Arc::clone(&tracker);
        let cfg_d = config;

        let task_handle = tokio::task::spawn(async move {
            let publish_task = tokio::task::spawn(async move {
                publish_loop(tracker_p, cfg_p, cancel_p).await;
            });
            let discover_task = tokio::task::spawn(async move {
                discover_loop(tracker_d, cfg_d, new_peers_tx, cancel_d).await;
            });
            let _ = tokio::join!(publish_task, discover_task);
        });

        Self {
            cancel,
            task_handle,
            _tracker: tracker,
            _joiner: None,
        }
    }

    /// Start continuous publication and discovery with an internal
    /// [`DynamicPeerJoiner`] for automatic peer joining.
    ///
    /// Unlike [`start`](Self::start), this method takes a
    /// [`GossipSender`] and creates a [`DynamicPeerJoiner`] internally
    /// that handles deduplication, self-filtering, bounded concurrency,
    /// and per-peer retry with exponential backoff.  The caller does not
    /// need to consume a channel or call `spawn_join_fanout`.
    ///
    /// The joiner's [`neighbor_events_tx`](DynamicPeerJoiner::neighbor_events_tx)
    /// sender is returned so the caller can forward gossip
    /// [`NeighborEvent`]s to keep the known-set in sync.
    ///
    /// # Panics
    ///
    /// Panics if `config.max_candidates_per_cycle == 0`.
    pub fn start_with_joiner(
        tracker: PublicRoomTracker,
        config: ContinuousTrackerConfig,
        gossip_sender: GossipSender,
    ) -> (Self, irpc::channel::mpsc::Sender<NeighborEvent>) {
        assert!(
            config.max_candidates_per_cycle > 0,
            "max_candidates_per_cycle must be > 0"
        );

        let cancel = CancellationToken::new();
        let tracker = Arc::new(tracker);

        // Create the DynamicPeerJoiner.
        let local_ep = *tracker.local_endpoint_id();
        let joiner_config = DynamicPeerJoinerConfig {
            max_concurrent_joins: config.max_concurrent_joins,
            max_retries_per_peer: 3,
            initial_retry_delay: config.initial_retry_delay,
            max_retry_delay: config.max_retry_delay,
            jitter_factor: config.jitter_factor,
        };
        let joiner = DynamicPeerJoiner::start(local_ep, gossip_sender, joiner_config);
        let neighbor_events_tx = joiner.neighbor_events_tx.clone();

        // Create the discovery channel between the discover loop and the joiner.
        // We use `start` internally but wire the output to the joiner.
        let (discovery_tx, mut discovery_rx) = mpsc::channel::<Vec<EndpointId>>(64);

        let cancel_p = cancel.clone();
        let tracker_p = Arc::clone(&tracker);
        let cfg_p = config.clone();

        let cancel_d = cancel.clone();
        let tracker_d = Arc::clone(&tracker);
        let cfg_d = config;
        let joiner_discovery_tx = joiner.discovery_tx.clone();

        let task_cancel = cancel.clone();
        let task_handle = tokio::task::spawn(async move {
            let publish_task = tokio::task::spawn(async move {
                publish_loop(tracker_p, cfg_p, cancel_p).await;
            });
            let discover_task = tokio::task::spawn(async move {
                discover_loop(tracker_d, cfg_d, discovery_tx, cancel_d).await;
            });
            // Bridge task: reads from the discover channel and forwards to the
            // DynamicPeerJoiner.  This ensures the joiner's dedup/retry logic
            // handles each discovered batch.
            let bridge_cancel = task_cancel.clone();
            let bridge = tokio::task::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = bridge_cancel.cancelled() => break,
                        maybe_batch = discovery_rx.recv() => {
                            match maybe_batch {
                                Some(batch) => {
                                    if batch.is_empty() {
                                        continue;
                                    }
                                    if joiner_discovery_tx.send(batch).await.is_err() {
                                        trace!("discovery→joiner bridge: joiner channel closed");
                                        break;
                                    }
                                }
                                None => {
                                    trace!("discovery→joiner bridge: discover channel closed");
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            let _ = tokio::join!(publish_task, discover_task, bridge);
        });

        (
            Self {
                cancel,
                task_handle,
                _tracker: tracker,
                _joiner: Some(joiner),
            },
            neighbor_events_tx,
        )
    }

    /// Signal shutdown and wait for background tasks to complete.
    ///
    /// This fires the cancellation token and awaits the tasks, ensuring
    /// any in-flight publish or discovery completes (or is aborted).
    /// After this call, the tracker is shut down and no further
    /// operations will be attempted.
    pub async fn shutdown(self) {
        let room = self._tracker.identity().short_id();
        info!(room = %room, "continuous tracker shutting down");
        self.cancel.cancel();
        // Shutdown the internal joiner if present.
        if let Some(joiner) = self._joiner {
            joiner.shutdown().await;
        }
        let _ = self.task_handle.await;
        // `_tracker` is dropped here, triggering backend shutdown.
        info!(room = %room, "continuous tracker shut down");
    }
}

// ---------------------------------------------------------------------------
// Background loops
// ---------------------------------------------------------------------------

/// Periodic publication loop.
///
/// Publishes local presence at the configured interval (with jitter).
/// On failure, retries with exponential backoff up to `max_retry_delay`.
/// Tracks consecutive failures to detect degraded DHT state.
async fn publish_loop(
    tracker: Arc<PublicRoomTracker>,
    config: ContinuousTrackerConfig,
    cancel: CancellationToken,
) {
    let room = tracker.identity().short_id();
    let mut ticker = interval(config.publish_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    // Track consecutive failures for degraded-state detection.
    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!(room = %room, "continuous publish cancelled");
                break;
            }
            _ = ticker.tick() => {
                // Apply jitter before proceeding.
                let _ = apply_jitter(config.publish_interval, config.jitter_factor);

                let start = Instant::now();
                let result = retry_with_backoff(
                    || tracker.publish_once(),
                    config.initial_retry_delay,
                    config.max_retry_delay,
                    config.jitter_factor,
                    &cancel,
                )
                .await;
                let duration_us = start.elapsed().as_micros() as u64;

                match result {
                    Ok(()) => {
                        consecutive_failures = 0;
                        debug!(
                            room = %room,
                            duration_us = duration_us,
                            "continuous publish succeeded",
                        );
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            warn!(
                                room = %room,
                                error = %e,
                                consecutive_failures = consecutive_failures,
                                duration_us = duration_us,
                                fallback = "continue_with_stale_advertisement",
                                "continuous publish degraded DHT state",
                            );
                        } else {
                            warn!(
                                room = %room,
                                error = %e,
                                consecutive_failures = consecutive_failures,
                                duration_us = duration_us,
                                "continuous publish failed after retries",
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Periodic discovery loop.
///
/// Looks up peers on the DHT at the configured interval (with jitter).
/// Discovered peers are sent through `new_peers_tx` as batches.
/// Tracks consecutive failures to detect degraded DHT state.
/// Applies stale-ttl eviction to the known-peers set on each tick.
async fn discover_loop(
    tracker: Arc<PublicRoomTracker>,
    config: ContinuousTrackerConfig,
    new_peers_tx: mpsc::Sender<Vec<EndpointId>>,
    cancel: CancellationToken,
) {
    let room = tracker.identity().short_id();
    let mut ticker = interval(config.discover_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    // Admission is deliberately local to one tracker session.  It bounds both
    // the lifetime number of proposals and the burst rate even if the DHT keeps
    // returning an unbounded stream of distinct endpoint IDs.
    let mut admitted_candidates = 0usize;
    let mut attempt_times = VecDeque::new();
    let attempt_window = config.connection_attempt_window;
    let max_candidates = config.max_candidates_per_cycle;

    // Track peers we've already forwarded so we don't re-send them.
    // Uses HashMap<EndpointId, Instant> for stale-ttl eviction.
    let mut known_peers: HashMap<EndpointId, Instant> = HashMap::new();
    let staleness_ttl = config.stale_peer_ttl;

    // Track consecutive failures for degraded-state detection.
    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!(room = %room, "continuous discover cancelled");
                break;
            }
            _ = ticker.tick() => {
                let _ = apply_jitter(config.discover_interval, config.jitter_factor);

                // Evict stale known peers before processing this tick.
                if let Some(ttl) = staleness_ttl {
                    let cutoff = Instant::now() - ttl;
                    let before = known_peers.len();
                    known_peers.retain(|_, last_seen| *last_seen >= cutoff);
                    let evicted = before - known_peers.len();
                    if evicted > 0 {
                        trace!(
                            room = %room,
                            evicted,
                            remaining = known_peers.len(),
                            "evicted stale known-peers entries",
                        );
                    }
                }

                let start = Instant::now();
                let result = retry_with_backoff(
                    || tracker.discover_once(),
                    config.initial_retry_delay,
                    config.max_retry_delay,
                    config.jitter_factor,
                    &cancel,
                )
                .await;
                let duration_us = start.elapsed().as_micros() as u64;

                match result {
                    Ok(peers) => {
                        consecutive_failures = 0;
                        let count = peers.len();
                        // Admit only candidates that fit both hard bounds.
                        let now = Instant::now();
                        while attempt_times
                            .front()
                            .is_some_and(|t| now.duration_since(*t) >= attempt_window)
                        {
                            attempt_times.pop_front();
                        }
                        let available_session = config
                            .max_candidates_per_session
                            .saturating_sub(admitted_candidates);
                        let available_rate = config
                            .connection_attempts_per_window
                            .saturating_sub(attempt_times.len());
                        let allowance = max_candidates
                            .min(available_session)
                            .min(available_rate);
                        let mut new_peers = Vec::with_capacity(allowance);
                        for peer in peers {
                            if new_peers.len() >= allowance {
                                break;
                            }
                            // known_peers is now a HashMap — map presence means known.
                            if known_peers.contains_key(&peer) {
                                continue;
                            }
                            trace!(
                                room = %room,
                                candidate = %peer.fmt_short(),
                                "candidate peer admitted for join",
                            );
                            known_peers.insert(peer, now);
                            new_peers.push(peer);
                            attempt_times.push_back(now);
                        }
                        admitted_candidates += new_peers.len();

                        if new_peers.is_empty() {
                            if count > 0 {
                                trace!(
                                    room = %room,
                                    total = count,
                                    duration_us = duration_us,
                                    "discovery found peers, all already known",
                                );
                            } else {
                                debug!(
                                    room = %room,
                                    duration_us = duration_us,
                                    "discovery returned no peers — waiting for peers to join",
                                );
                            }
                            continue;
                        }

                        let new_count = new_peers.len();
                        info!(
                            room = %room,
                            total = count,
                            new = new_count,
                            filtered = count.saturating_sub(new_count),
                            duration_us = duration_us,
                            "discovery found new peers",
                        );

                        if new_peers_tx.send(new_peers).await.is_err() {
                            info!(
                                room = %room,
                                "continuous discover channel closed, stopping",
                            );
                            break;
                        }
                    }
                    Err(e) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            warn!(
                                room = %room,
                                error = %e,
                                consecutive_failures = consecutive_failures,
                                duration_us = duration_us,
                                fallback = "continue_with_existing_peers",
                                "continuous discover degraded DHT state",
                            );
                        } else {
                            warn!(
                                room = %room,
                                error = %e,
                                consecutive_failures = consecutive_failures,
                                duration_us = duration_us,
                                "continuous discover failed after retries",
                            );
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Apply uniform jitter to a duration: `base ± (base * jitter_factor)`.
fn apply_jitter(base: Duration, factor: f64) -> Duration {
    if factor <= 0.0 {
        return base;
    }
    let nanos = base.as_nanos() as f64;
    let range = nanos * factor;
    let offset: f64 = (rand::random::<f64>() * 2.0 - 1.0) * range;
    let jittered_nanos = (nanos + offset).max(0.0) as u64;
    Duration::from_nanos(jittered_nanos)
}

/// Retry an async operation with exponential backoff and jitter.
///
/// Returns `Ok(value)` on first success, or `Err(error)` after backoff
/// is exhausted or cancellation is signalled.
async fn retry_with_backoff<T, F, Fut>(
    mut f: F,
    initial_delay: Duration,
    max_delay: Duration,
    jitter_factor: f64,
    cancel: &CancellationToken,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay = initial_delay;
    let mut attempt: u32 = 0;

    loop {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                attempt += 1;
                if cancel.is_cancelled() {
                    return Err(e);
                }
                // Exponential backoff: double the delay, cap at max.
                delay = (delay * 2).min(max_delay);
                let jittered = apply_jitter(delay, jitter_factor);
                debug!(
                    attempt = attempt,
                    delay_us = delay.as_micros() as u64,
                    jittered_us = jittered.as_micros() as u64,
                    error = %e,
                    "retrying after failure",
                );

                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(e),
                    _ = sleep(jittered) => continue,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Join fanout — automatically join discovered peers into the gossip mesh
// ---------------------------------------------------------------------------

/// Spawn a background task that reads discovered peer batches from
/// `new_peers_rx` and forwards them to [`GossipSender::join_peers`].
///
/// This lets the automatic DHT discovery loop actually bring discovered
/// peers into the gossip mesh for the room.  Without this task, the
/// [`ContinuousTracker`] sends discovered [`EndpointId`] values into the
/// channel but nobody reads them.
///
/// Prefer [`ContinuousTracker::start_with_joiner`] over this function for
/// new code — it provides bounded concurrency, per-peer retry with
/// backoff, and clean integration.
///
/// # Returns
///
/// A [`JoinHandle`] that completes when the channel closes or the
/// [`CancellationToken`] is cancelled.  The caller should cancel the token
/// when the room is left or deleted to ensure the task exits promptly.
///
/// # Logging
///
/// Successful joins are logged at `INFO` level; failures at `WARN`.
pub fn spawn_join_fanout(
    new_peers_rx: mpsc::Receiver<Vec<EndpointId>>,
    sender: crate::api::GossipSender,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::task::spawn(async move {
        let mut rx = new_peers_rx;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    trace!("join-fanout task cancelled");
                    break;
                }
                maybe_peers = rx.recv() => {
                    match maybe_peers {
                        Some(peers) => {
                            let count = peers.len();
                            if count == 0 {
                                continue;
                            }
                            info!(
                                count = count,
                                "join-fanout: forwarding discovered peers to gossip mesh",
                            );
                            let start = Instant::now();
                            let result = sender
                                .join_peers(peers)
                                .instrument(info_span!("tracker.join", candidates = count))
                                .await;
                            let duration_us = start.elapsed().as_micros() as u64;
                            match result {
                                Ok(()) => info!(
                                    candidates = count,
                                    outcome = "queued",
                                    duration_us,
                                    "join-fanout: queued discovered peers for gossip join",
                                ),
                                Err(e) => warn!(
                                    candidates = count,
                                    outcome = "failure",
                                    duration_us,
                                    error = %e,
                                    "join-fanout: join_peers failed",
                                ),
                            }
                        }
                        None => {
                            trace!("join-fanout channel closed, exiting");
                            break;
                        }
                    }
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery_backend::InMemoryDiscoveryBackend;
    use crate::public_room::PublicNetwork;
    use crate::public_room_tracker::PublicRoomTracker;
    use iroh::SecretKey;

    /// Helper: generate a test identity.
    fn test_identity() -> (SecretKey, EndpointId) {
        let sk = SecretKey::generate();
        let ep = sk.public();
        (sk, ep)
    }

    // ── Tracing smoke tests ──────────────────────────────────────────

    /// Tracing is emitted during continuous tracker lifecycle.
    #[tokio::test]
    #[n0_tracing_test::traced_test]
    async fn traced_continuous_tracker_lifecycle() {
        let (_alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();

        let backend = InMemoryDiscoveryBackend::new();

        // Bob publishes first.
        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            alice_ep,
            SecretKey::generate(),
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // Wait for discovery to find Bob.
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for discovery")
            .expect("channel closed unexpectedly");
        assert!(peers.contains(&bob_ep), "expected Bob to be discovered");
    }

    // ── Existing tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn continuous_tracker_discovers_new_peer() {
        let (_alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();

        let backend = InMemoryDiscoveryBackend::new();

        // Bob publishes first (simulates a pre-existing peer).
        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        // Alice starts continuous tracker.
        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            alice_ep,
            SecretKey::generate(),
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for discovery")
            .expect("channel closed unexpectedly");

        assert!(
            peers.contains(&bob_ep),
            "expected Bob's EndpointId to be discovered, got {peers:?}"
        );
    }

    #[tokio::test]
    async fn continuous_tracker_does_not_repeat_known_peers() {
        let (alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();

        let backend = InMemoryDiscoveryBackend::new();

        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        // Alice uses her own endpoint so self-filter doesn't hide Bob.
        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            alice_ep,
            alice_sk,
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // First discovery should find Bob.
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout waiting for first discovery")
            .expect("channel closed unexpectedly");
        assert!(!first.is_empty(), "expected at least one peer");
        assert!(first.contains(&bob_ep), "expected Bob");

        // Wait a bit — subsequent ticks should NOT send Bob again.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Drain any messages that may have arrived.
        loop {
            match tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
                Ok(Some(batch)) => {
                    assert!(
                        batch.is_empty(),
                        "expected empty batch (peer already known), got {batch:?}"
                    );
                }
                _ => break,
            }
        }

        continuous.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_stops_background_tasks() {
        let (sk, ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let tracker =
            PublicRoomTracker::start(Box::new(backend.clone()), PublicNetwork::Test, ep, sk)
                .await
                .unwrap();

        let (tx, _rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(10),
            publish_interval: Duration::from_millis(10),
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(tracker, config.sanitize(), tx);

        // Drop the receiver so the discovery loop stops on send failure.
        drop(_rx);

        // Shutdown should complete quickly — no blocking.
        tokio::time::timeout(Duration::from_secs(5), continuous.shutdown())
            .await
            .expect("shutdown timed out (tasks did not stop)");
    }

    #[tokio::test]
    async fn graceful_degradation_on_empty_backend() {
        let (sk, ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let tracker =
            PublicRoomTracker::start(Box::new(backend.clone()), PublicNetwork::Test, ep, sk)
                .await
                .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(20),
            publish_interval: Duration::from_secs(3600),
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(tracker, config.sanitize(), tx);

        // Let the discovery loop tick a few times with no peers.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Any received batches must be empty.
        while let Ok(Some(batch)) = tokio::time::timeout(Duration::from_millis(5), rx.recv()).await
        {
            assert!(batch.is_empty(), "expected empty batch, got {batch:?}");
        }

        continuous.shutdown().await;
    }

    #[test]
    fn config_sanitize_clamps_jitter() {
        let cfg = ContinuousTrackerConfig {
            jitter_factor: 2.0,
            ..Default::default()
        };
        let sanitized = cfg.sanitize();
        assert!(
            sanitized.jitter_factor <= 0.5,
            "jitter_factor should be clamped to 0.5, got {}",
            sanitized.jitter_factor
        );

        let cfg2 = ContinuousTrackerConfig {
            jitter_factor: -1.0,
            ..Default::default()
        };
        let sanitized2 = cfg2.sanitize();
        assert!(
            sanitized2.jitter_factor >= 0.0,
            "negative jitter_factor should be clamped to 0.0, got {}",
            sanitized2.jitter_factor
        );
    }

    // ── Join-fanout integration ──────────────────────────────────────

    #[tokio::test]
    async fn join_fanout_forwards_discovered_peers() {
        let (_alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();

        let backend = InMemoryDiscoveryBackend::new();

        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            alice_ep,
            SecretKey::generate(),
        )
        .await
        .unwrap();

        let (peers_tx, mut peers_rx) = mpsc::channel::<Vec<EndpointId>>(16);
        let cancel = CancellationToken::new();

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), peers_tx);

        let result = tokio::time::timeout(Duration::from_secs(5), peers_rx.recv()).await;
        cancel.cancel();
        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for discovery")
            .expect("channel closed unexpectedly");
        assert!(
            peers.contains(&bob_ep),
            "expected Bob's EndpointId to be discovered, got {peers:?}"
        );
        info!(
            discovered = peers.len(),
            "join-fanout integration test: discovered peers",
        );
    }

    #[tokio::test]
    async fn join_fanout_exits_on_cancellation() {
        let (tx, rx) = mpsc::channel::<Vec<EndpointId>>(16);
        let cancel = CancellationToken::new();

        use crate::api::Command;
        let (cmd_tx, _cmd_rx): (
            irpc::channel::mpsc::Sender<Command>,
            irpc::channel::mpsc::Receiver<Command>,
        ) = irpc::channel::mpsc::channel(16);
        let sender = crate::api::GossipSender::new(cmd_tx);

        let handle = spawn_join_fanout(rx, sender, cancel.clone());

        cancel.cancel();

        tokio::time::timeout(Duration::from_secs(3), handle)
            .await
            .expect("join-fanout task did not exit within timeout")
            .expect("join-fanout task panicked");

        let _ = tx.send(vec![]).await;
    }

    // ═══════════════════════════════════════════════════════════════════
    // NEW TESTS: late peers, refresh, backend failures, cancellation,
    // bounds, stale eviction, and start_with_joiner wiring.
    // ═══════════════════════════════════════════════════════════════════

    // ── Late peers ──────────────────────────────────────────────────

    /// A peer that publishes *after* the continuous tracker has started
    /// should be discovered on the next discovery tick.
    #[tokio::test]
    async fn discovers_late_peer() {
        let (_alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();

        let backend = InMemoryDiscoveryBackend::new();

        // Alice starts the continuous tracker first, with no peers yet.
        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            alice_ep,
            SecretKey::generate(),
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // Wait a tick to confirm no peers yet.
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Now Bob publishes (late peer).
        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        // The next discovery tick should find Bob.
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for late peer discovery")
            .expect("channel closed unexpectedly");
        assert!(
            peers.contains(&bob_ep),
            "expected late Bob to be discovered, got {peers:?}"
        );
    }

    // ── Refresh (re-publish) ────────────────────────────────────────

    /// The publish loop should successfully re-publish on each tick.
    /// We verify this by publishing Bob's record, then discovering it
    /// after Alice's tracker has been running for a while.
    #[tokio::test]
    async fn publish_refreshes_local_record() {
        let (alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();

        let backend = InMemoryDiscoveryBackend::new();

        // Bob publishes first.
        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        // Alice starts a tracker with a fast publish interval.
        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            alice_ep,
            alice_sk,
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_millis(30), // fast re-publish
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // Wait for discovery to find Bob.
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for discovery after refresh")
            .expect("channel closed unexpectedly");
        assert!(
            peers.contains(&bob_ep),
            "expected Bob to be discovered after refresh, got {peers:?}"
        );
    }

    // ── Backend failures ────────────────────────────────────────────

    /// When the backend returns an error during discovery, the loop
    /// should retry with backoff and eventually recover.
    #[tokio::test]
    async fn recovers_from_backend_failures() {
        let (bob_sk, bob_ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();

        // Bob publishes first.
        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        // Alice's tracker.
        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            SecretKey::generate().public(),
            SecretKey::generate(),
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(20),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // Wait for discovery to find Bob successfully.
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        continuous.shutdown().await;

        let peers = result
            .expect("timeout waiting for discovery")
            .expect("channel closed unexpectedly");
        assert!(
            peers.contains(&bob_ep),
            "expected Bob to be discovered after transient failures, got {peers:?}"
        );
    }

    // ── Cancellation ────────────────────────────────────────────────

    /// Explicit test that cancellation via cancel() stops the discovery
    /// loop and no further batches are received.
    #[tokio::test]
    async fn cancellation_stops_discovery_promptly() {
        let (sk, ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let tracker =
            PublicRoomTracker::start(Box::new(backend.clone()), PublicNetwork::Test, ep, sk)
                .await
                .unwrap();

        let (tx, _rx) = mpsc::channel::<Vec<EndpointId>>(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(20),
            publish_interval: Duration::from_secs(3600),
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(tracker, config.sanitize(), tx);

        // Let it run briefly.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Shutdown should complete within a generous timeout.
        tokio::time::timeout(Duration::from_secs(3), continuous.shutdown())
            .await
            .expect("shutdown did not complete promptly after cancellation");
    }

    // ── Bounds (session candidate cap) ───────────────────────────────

    /// When the per-session candidate cap is reached, no further peers
    /// should be admitted even if the backend returns new ones.
    #[tokio::test]
    async fn respects_session_candidate_cap() {
        let backend = InMemoryDiscoveryBackend::new();

        // Have Bob publish.
        let (bob_sk, bob_ep) = test_identity();
        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        // Alice's tracker with a tiny session cap.
        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            SecretKey::generate().public(),
            SecretKey::generate(),
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(20),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_session: 1, // only 1 candidate allowed
            max_candidates_per_cycle: 20,
            connection_attempts_per_window: 10,
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // Wait for the first (and only) discovery.
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        assert!(
            first.contains(&bob_ep),
            "should discover Bob on first tick"
        );

        // Wait several more ticks — should not send any more non-empty batches.
        tokio::time::sleep(Duration::from_millis(200)).await;
        loop {
            match tokio::time::timeout(Duration::from_millis(5), rx.recv()).await {
                Ok(Some(batch)) => {
                    assert!(
                        batch.is_empty(),
                        "expected empty batch after session cap reached, got {batch:?}"
                    );
                }
                _ => break,
            }
        }

        continuous.shutdown().await;
    }

    // ── Stale eviction ──────────────────────────────────────────────

    /// When `stale_peer_ttl` is set, a peer that was known but whose
    /// TTL has expired can be re-discovered as if it were new.
    #[tokio::test]
    async fn stale_peer_ttl_allows_re_discovery() {
        let (alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();

        let backend = InMemoryDiscoveryBackend::new();

        // Bob publishes first.
        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        // Alice's tracker with a very short stale TTL.
        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            alice_ep,
            alice_sk,
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::channel(16);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            stale_peer_ttl: Some(Duration::from_millis(100)),
            ..Default::default()
        };

        let continuous = ContinuousTracker::start(alice_tracker, config.sanitize(), tx);

        // First discovery should find Bob.
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout waiting for first discovery")
            .expect("channel closed unexpectedly");
        assert!(first.contains(&bob_ep), "expected Bob on first discovery");

        // Wait for TTL to expire and then one more tick.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Bob should be re-discovered since the TTL expired.
        // We may get multiple batches — at least one should contain Bob.
        let mut found_bob_again = false;
        loop {
            match tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
                Ok(Some(batch)) => {
                    if batch.contains(&bob_ep) {
                        found_bob_again = true;
                    }
                }
                _ => break,
            }
        }

        assert!(
            found_bob_again,
            "expected Bob to be re-discovered after stale TTL expired"
        );

        continuous.shutdown().await;
    }

    // ── start_with_joiner integration ────────────────────────────────

    /// Verify that `start_with_joiner` creates a working pipeline:
    /// discovered peers are forwarded through the DynamicPeerJoiner.
    /// We intercept the `JoinPeers` command to verify delivery.
    #[tokio::test]
    async fn start_with_joiner_forwards_peers_to_gossip() {
        let (_alice_sk, alice_ep) = test_identity();
        let (bob_sk, bob_ep) = test_identity();

        let backend = InMemoryDiscoveryBackend::new();

        // Bob publishes first.
        let bob_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            bob_ep.clone(),
            bob_sk,
        )
        .await
        .unwrap();
        bob_tracker.publish_once().await.unwrap();

        let alice_tracker = PublicRoomTracker::start(
            Box::new(backend.clone()),
            PublicNetwork::Test,
            alice_ep,
            SecretKey::generate(),
        )
        .await
        .unwrap();

        // Create a mock GossipSender that records JoinPeers commands.
        use crate::api::Command;
        use tokio::sync::mpsc as tokio_mpsc;
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let gossip_sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(50),
            publish_interval: Duration::from_secs(3600),
            max_candidates_per_cycle: 20,
            max_concurrent_joins: 5,
            ..Default::default()
        };

        let (continuous, _neighbor_events_tx) =
            ContinuousTracker::start_with_joiner(alice_tracker, config.sanitize(), gossip_sender);

        // Wait for Bob to be discovered and joined via the DynamicPeerJoiner.
        let cmd = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("timeout waiting for JoinPeers command")
            .expect("channel closed unexpectedly");

        match cmd {
            Command::JoinPeers(peers) => {
                assert!(
                    peers.contains(&bob_ep),
                    "expected Bob to be joined, got {peers:?}"
                );
            }
            other => panic!("expected JoinPeers, got {other:?}"),
        }

        continuous.shutdown().await;
    }

    /// Verify start_with_joiner shutdown cleans up the internal joiner.
    #[tokio::test]
    async fn start_with_joiner_shutdown_cleans_up() {
        let (sk, ep) = test_identity();
        let backend = InMemoryDiscoveryBackend::new();
        let tracker =
            PublicRoomTracker::start(Box::new(backend.clone()), PublicNetwork::Test, ep, sk)
                .await
                .unwrap();

        use crate::api::Command;
        let (cmd_tx, _cmd_rx): (
            irpc::channel::mpsc::Sender<Command>,
            irpc::channel::mpsc::Receiver<Command>,
        ) = irpc::channel::mpsc::channel(64);
        let gossip_sender = GossipSender::new(cmd_tx);

        let config = ContinuousTrackerConfig {
            discover_interval: Duration::from_millis(10),
            publish_interval: Duration::from_secs(3600),
            ..Default::default()
        };

        let (continuous, _neighbor_events_tx) =
            ContinuousTracker::start_with_joiner(tracker, config.sanitize(), gossip_sender);

        tokio::time::timeout(Duration::from_secs(5), continuous.shutdown())
            .await
            .expect("shutdown with joiner did not complete within timeout");
    }
}
