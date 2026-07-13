//! Continuous public-room DHT discovery and publication.
//!
//! Spawns background tasks that periodically re-publish local presence and
//! discover new peers on the DHT.  Discovered peer IDs are forwarded through
//! an [`mpsc::Sender`] channel for the caller to join via
//! [`GossipSender::join_peers`].
//!
//! # Lifecycle
//!
//! 1. [`ContinuousTracker::start`] — spawns background tasks and returns a
//!    handle.
//! 2. Read from the provided [`mpsc::Receiver`] to get discovered peer
//!    batches.
//! 3. [`ContinuousTracker::shutdown`] — signals cancellation and awaits task
//!    completion.
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

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::{
    sync::mpsc,
    task::JoinHandle,
    time::{interval, sleep, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

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
    /// [`GossipSender::join_peers`].
    ///
    /// # Panics
    ///
    /// Panics if `config.max_candidates_per_cycle == 0`.
    pub fn start(
        tracker: PublicRoomTracker,
        config: ContinuousTrackerConfig,
        new_peers_tx: mpsc::Sender<Vec<EndpointId>>,
    ) -> Self {
        assert!(
            config.max_candidates_per_cycle > 0,
            "max_candidates_per_cycle must be > 0"
        );

        let tracker = Arc::new(tracker);
        let cancel = CancellationToken::new();

        let cancel_p = cancel.clone();
        let tracker_p = Arc::clone(&tracker);
        let cfg_p = config.clone();

        let cancel_d = cancel.clone();
        let tracker_d = Arc::clone(&tracker);
        let cfg_d = config;

        let task_handle = tokio::task::spawn(async move {
            // Spawn both loops as independent tasks so a publish failure
            // does not block discovery and vice versa.
            let publish_task = tokio::task::spawn(async move {
                publish_loop(tracker_p, cfg_p, cancel_p).await;
            });
            let discover_task = tokio::task::spawn(async move {
                discover_loop(tracker_d, cfg_d, new_peers_tx, cancel_d).await;
            });

            // Wait for either to finish (should only happen on cancellation).
            let _ = tokio::join!(publish_task, discover_task);
        });

        Self {
            cancel,
            task_handle,
            _tracker: tracker,
        }
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

    // Track peers we've already forwarded so we don't re-send them.  This set
    // is bounded by the session candidate cap; never insert candidates that
    // cannot be admitted.
    let mut known_peers: HashSet<EndpointId> = HashSet::new();

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
                        // Admit only candidates that fit both hard bounds.  A
                        // candidate is marked known only after admission, so a
                        // rate-limited item can be reconsidered on a later tick.
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
                            if known_peers.insert(peer) {
                                new_peers.push(peer);
                                attempt_times.push_back(now);
                            }
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

        // Wait a bit — subsequent ticks should NOT send Bob again
        // because the discovery loop's known_peers set remembers him.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Drain any messages that may have arrived (should be empty or none).
        // We use a tight timeout so we don't block if nothing arrives.
        loop {
            match tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
                Ok(Some(batch)) => {
                    assert!(
                        batch.is_empty(),
                        "expected empty batch (peer already known), got {batch:?}"
                    );
                }
                // Timeout or channel closed — no more messages, which is correct.
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
}
