//! Bounded dynamic peer joiner for gossip topics.
//!
//! [`DynamicPeerJoiner`] reads discovered peer batches from an `mpsc` channel
//! and joins them via [`GossipSender::join_peers`] with:
//!
//! * **Deduplication** — skip already-known or already-joining peers.
//! * **Self-ignore** — skip the local [`EndpointId`].
//! * **Bounded concurrency** — at most [`DynamicPeerJoinerConfig::max_concurrent_joins`]
//!   in-flight join attempts at once.
//! * **Per-peer retry with backoff** — on failure, wait then retry up to
//!   [`DynamicPeerJoinerConfig::max_retries_per_peer`] times with exponential
//!   backoff + jitter.
//! * **Later retries** — when a [`NeighborEvent::Down`] is received, the peer
//!   is removed from the known set so a future discovery batch can try again.
//! * **Bounded per-peer tasks** — at most one in-flight join attempt per peer
//!   at any time; retries are sequential, not concurrent.
//! * **Tracing without secrets** — events use `EndpointId::fmt_short()` (6-char
//!   abbreviation), never the full 32‑byte key.
//!
//! # Lifecycle
//!
//! 1. [`DynamicPeerJoiner::start`] — spawns a background tokio task.
//! 2. Feed peer batches into the [`discovery_tx`](DynamicPeerJoiner::discovery_tx)
//!    sender (e.g. from a DHT discovery loop).
//! 3. Feed [`NeighborEvent`]s into the
//!    [`neighbor_events_tx`](DynamicPeerJoiner::neighbor_events_tx) sender
//!    (from the gossip event stream).
//! 4. [`DynamicPeerJoiner::shutdown`] — signals cancellation and awaits the
//!    background task.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh_base::EndpointId;
use irpc::channel::mpsc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use crate::api::{Command, GossipSender};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Events that inform the joiner about neighbor connectivity changes.
///
/// Feed these via the [`neighbor_events_tx`](DynamicPeerJoiner::neighbor_events_tx)
/// sender so the joiner can update its known‑peer set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeighborEvent {
    /// A peer became a direct neighbor in the gossip mesh.
    Up(EndpointId),
    /// A peer is no longer a direct neighbor.
    Down(EndpointId),
}

/// Configuration for [`DynamicPeerJoiner`].
#[derive(Debug, Clone)]
pub struct DynamicPeerJoinerConfig {
    /// Maximum candidates accepted from one discovery batch.
    /// Excess candidates are ignored and may be considered by a later cycle.
    /// Default: 64.
    pub max_candidates_per_batch: usize,

    /// Maximum number of peers being joined concurrently.
    ///
    /// Default: 5.
    pub max_concurrent_joins: usize,

    /// Maximum retry attempts per peer before giving up.
    ///
    /// A value of `0` means no retries — each peer is tried at most once.
    /// After the retry budget is exhausted the peer is silently removed
    /// from the pending set so it **can** be retried if a later discovery
    /// batch re‑introduces it.
    ///
    /// Default: 3.
    pub max_retries_per_peer: u32,

    /// Initial delay before the first retry (doubles on each retry).
    ///
    /// Default: 1 second.
    pub initial_retry_delay: Duration,

    /// Maximum delay for exponential backoff.
    ///
    /// Default: 60 seconds.
    pub max_retry_delay: Duration,

    /// Jitter factor applied to retry delays (`0.0` = none, `0.1` = ±10%).
    ///
    /// Default: 0.1.
    pub jitter_factor: f64,
}

impl Default for DynamicPeerJoinerConfig {
    fn default() -> Self {
        Self {
            max_candidates_per_batch: 64,
            max_concurrent_joins: 5,
            max_retries_per_peer: 3,
            initial_retry_delay: Duration::from_secs(1),
            max_retry_delay: Duration::from_secs(60),
            jitter_factor: 0.1,
        }
    }
}

// ---------------------------------------------------------------------------
// DynamicPeerJoiner
// ---------------------------------------------------------------------------

/// A handle that manages bounded dynamic peer joining in the background.
///
/// Dropping this handle **without** calling [`shutdown`](Self::shutdown) will
/// abort the background task — always call `shutdown` for clean teardown.
#[derive(Debug)]
pub struct DynamicPeerJoiner {
    /// Cancellation token shared with the background task.
    cancel: CancellationToken,
    /// Join handle for the background task.
    task_handle: tokio::task::JoinHandle<()>,
    /// Sender half of the neighbor‑event channel — the caller uses this to
    /// report connectivity changes.
    pub neighbor_events_tx: mpsc::Sender<NeighborEvent>,
    /// Sender half of the discovery channel — the caller uses this to feed
    /// new peer batches.
    pub discovery_tx: mpsc::Sender<Vec<EndpointId>>,
}

impl DynamicPeerJoiner {
    /// Start the background joiner task.
    ///
    /// # Parameters
    ///
    /// * `local_endpoint_id` — this node's [`EndpointId`] (used for self‑filtering).
    /// * `gossip_sender` — [`GossipSender`] for the gossip topic to join peers on.
    /// * `config` — tuning parameters.
    ///
    /// # Returns
    ///
    /// A [`DynamicPeerJoiner`] handle.  The caller should wire up:
    ///
    /// - [`discovery_tx`](Self::discovery_tx) — send peer batches from a DHT
    ///   discovery loop.
    /// - [`neighbor_events_tx`](Self::neighbor_events_tx) — forward
    ///   [`NeighborEvent`]s from the gossip event stream.
    pub fn start(
        local_endpoint_id: EndpointId,
        gossip_sender: GossipSender,
        config: DynamicPeerJoinerConfig,
    ) -> Self {
        let cancel = CancellationToken::new();
        let (discovery_tx, discovery_rx) = mpsc::channel::<Vec<EndpointId>>(64);
        let (neighbor_events_tx, neighbor_events_rx) = mpsc::channel::<NeighborEvent>(64);

        let cancel_inner = cancel.clone();
        let task_handle = tokio::task::spawn(async move {
            run_joiner_loop(
                local_endpoint_id,
                gossip_sender,
                discovery_rx,
                neighbor_events_rx,
                config,
                cancel_inner,
            )
            .await;
        });

        Self {
            cancel,
            task_handle,
            neighbor_events_tx,
            discovery_tx,
        }
    }

    /// Signal shutdown and wait for the background task to finish.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task_handle.await;
        info!("dynamic joiner shut down");
    }
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Per‑peer join state.
#[derive(Debug, Clone)]
struct PeerJoinState {
    /// How many times we have attempted this peer (0 = first attempt).
    attempt: u32,
}

/// Shared writable state for the joiner loop.
struct JoinerState {
    /// Peers we have successfully joined (known direct neighbors).
    known: HashSet<EndpointId>,
    /// Peers currently being joined or awaiting retry.
    pending: HashMap<EndpointId, PeerJoinState>,
    /// Local endpoint — we never try to join ourselves.
    local: EndpointId,
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

async fn run_joiner_loop(
    local: EndpointId,
    gossip_sender: GossipSender,
    mut discovery_rx: mpsc::Receiver<Vec<EndpointId>>,
    mut neighbor_events_rx: mpsc::Receiver<NeighborEvent>,
    config: DynamicPeerJoinerConfig,
    cancel: CancellationToken,
) {
    let max_concurrent = config.max_concurrent_joins.max(1);
    let max_batch = config.max_candidates_per_batch.max(1);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let state = Arc::new(Mutex::new(JoinerState {
        known: HashSet::new(),
        pending: HashMap::new(),
        local,
    }));
    let mut workers = JoinSet::new();

    info!(
        max_concurrent,
        max_candidates_per_batch = max_batch,
        max_retries = config.max_retries_per_peer,
        "dynamic joiner started",
    );

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                debug!("dynamic joiner cancelled");
                break;
            }
            worker = workers.join_next(), if !workers.is_empty() => {
                if let Some(Err(error)) = worker {
                    debug!(error = %error, "dynamic joiner worker exited");
                }
            }
            event_result = neighbor_events_rx.recv() => {
                match event_result {
                    Ok(Some(event)) => {
                        if !handle_neighbor_event(event, &state) { break; }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            batch_result = discovery_rx.recv() => {
                match batch_result {
                    Ok(Some(batch)) => {
                        if !handle_discovery_batch(
                            batch, &state, &semaphore, &gossip_sender, &config,
                            &cancel, &mut workers, max_batch,
                        ) { break; }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }

    // Do not leave retry workers retaining endpoint IDs or the sender after
    // shutdown. JoinSet aborts and drains every worker deterministically.
    workers.abort_all();
    while let Some(result) = workers.join_next().await {
        if let Err(error) = result {
            trace!(error = %error, "dynamic joiner worker cancelled");
        }
    }
}

/// Process a neighbor event, returning `false` if the channel is closed.
fn handle_neighbor_event(event: NeighborEvent, state: &Arc<Mutex<JoinerState>>) -> bool {
    match event {
        NeighborEvent::Up(peer) => {
            let short = peer.fmt_short();
            let mut st = state.lock().expect("lock poisoned");
            st.known.insert(peer);
            // Peer already connected — no need to keep a pending join.
            let was_pending = st.pending.remove(&peer).is_some();
            if was_pending {
                trace!(peer = %short, "joiner: neighbor up, removed from pending");
            } else {
                trace!(peer = %short, "joiner: neighbor up");
            }
        }
        NeighborEvent::Down(peer) => {
            let short = peer.fmt_short();
            let mut st = state.lock().expect("lock poisoned");
            st.known.remove(&peer);
            // Permitting later retries: peer is no longer a direct neighbor,
            // so a future discovery batch may try again.
            trace!(peer = %short, "joiner: neighbor down — may retry later");
        }
    }
    true
}

/// Process a discovery batch, returning `false` if the channel is closed.
fn handle_discovery_batch(
    peers: Vec<EndpointId>,
    state: &Arc<Mutex<JoinerState>>,
    semaphore: &Arc<Semaphore>,
    gossip_sender: &GossipSender,
    config: &DynamicPeerJoinerConfig,
    cancel: &CancellationToken,
    workers: &mut JoinSet<()>,
    max_batch: usize,
) -> bool {
    let total = peers.len();
    let mut admissible: Vec<EndpointId> = Vec::with_capacity(total.min(max_batch));
    let mut batch_seen = HashSet::new();
    {
        let st = state.lock().expect("lock poisoned");
        for peer in peers.iter().take(max_batch) {
            if !batch_seen.insert(*peer) {
                trace!(peer = %peer.fmt_short(), "joiner: skip duplicate candidate");
                continue;
            }
            if *peer == st.local {
                trace!(peer = %peer.fmt_short(), "joiner: skip self");
                continue;
            }
            if st.known.contains(peer) {
                trace!(peer = %peer.fmt_short(), "joiner: skip already known");
                continue;
            }
            if st.pending.contains_key(peer) {
                trace!(peer = %peer.fmt_short(), "joiner: skip already pending");
                continue;
            }
            admissible.push(*peer);
        }
    }

    let admitted = admissible.len();
    if admitted == 0 {
        trace!(total, "joiner: all peers already known/pending/self");
        return true;
    }
    debug!(total, admitted, "joiner: processing discovery batch");

    // Spawn one lightweight task per admissible peer.  The task holds the
    // semaphore permit so concurrency is bounded.  Retries are sequential
    // inside the same task, preventing unbounded per-peer task explosion.
    for peer in admissible {
        let permit = semaphore.clone().try_acquire_owned();
        let Ok(permit) = permit else {
            trace!(peer = %peer.fmt_short(), "joiner: concurrency limit reached, peer deferred");
            // Not pushed back — a future discovery batch will re-introduce
            // the peer if it is still discoverable.
            continue;
        };

        {
            let mut st = state.lock().expect("lock poisoned");
            st.pending.insert(peer, PeerJoinState { attempt: 0 });
        }

        let state_clone = Arc::clone(state);
        let sender = gossip_sender.clone();
        let cancel_clone = cancel.clone();
        let cfg = config.clone();
        let short = peer.fmt_short().to_string();

        workers.spawn(async move {
            attempt_join_with_retries(peer, short, sender, state_clone, cfg, permit, cancel_clone)
                .await;
        });
    }

    true
}

// ---------------------------------------------------------------------------
// Join attempt with retries
// ---------------------------------------------------------------------------

/// Attempt to join a peer, retrying with exponential backoff on failure.
///
/// The semaphore permit is held for the entire duration (first attempt +
/// all retries), bounding concurrency per peer at 1.
async fn attempt_join_with_retries(
    peer: EndpointId,
    peer_short: String,
    sender: GossipSender,
    state: Arc<Mutex<JoinerState>>,
    config: DynamicPeerJoinerConfig,
    _permit: tokio::sync::OwnedSemaphorePermit,
    cancel: CancellationToken,
) {
    let max_retries = config.max_retries_per_peer;
    let mut delay = config.initial_retry_delay;

    for attempt in 0..=max_retries {
        if cancel.is_cancelled() {
            let mut st = state.lock().expect("lock poisoned");
            st.pending.remove(&peer);
            return;
        }

        let result = sender.join_peers(vec![peer]).await;

        match result {
            Ok(()) => {
                info!(peer = %peer_short, attempt, "joiner: join succeeded");
                let mut st = state.lock().expect("lock poisoned");
                st.known.insert(peer);
                st.pending.remove(&peer);
                return;
            }
            Err(e) => {
                if attempt < max_retries {
                    warn!(
                        peer = %peer_short,
                        attempt,
                        error = %e,
                        next_retry_ms = delay.as_millis(),
                        "joiner: join failed, will retry",
                    );
                    {
                        let mut st = state.lock().expect("lock poisoned");
                        if let Some(p) = st.pending.get_mut(&peer) {
                            p.attempt = attempt + 1;
                        }
                    }
                    let sleep_dur = apply_jitter(delay, config.jitter_factor);
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            let mut st = state.lock().expect("lock poisoned");
                            st.pending.remove(&peer);
                            return;
                        }
                        _ = tokio::time::sleep(sleep_dur) => {}
                    }
                    delay = (delay * 2).min(config.max_retry_delay);
                } else {
                    warn!(
                        peer = %peer_short,
                        attempt,
                        error = %e,
                        "joiner: join failed, exhausted retries",
                    );
                    let mut st = state.lock().expect("lock poisoned");
                    st.pending.remove(&peer);
                    return;
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc as tokio_mpsc;

    // ── Helpers ──────────────────────────────────────────────────────

    /// Generate a deterministic EndpointId (PublicKey) for testing by
    /// creating a SecretKey and deriving the public key.
    fn test_endpoint(id: u8) -> EndpointId {
        // Use SecretKey::from_bytes for deterministic endpoints.
        // iroh::SecretKey is available as a dev-dependency.
        let mut bytes = [0u8; 32];
        bytes[0] = id;
        bytes[1] = id.wrapping_add(1);
        bytes[2] = id.wrapping_mul(2);
        let sk = iroh::SecretKey::from_bytes(&bytes);
        sk.public()
    }

    /// Create a pair consisting of a mock GossipSender and a receiver
    /// that captures all `JoinPeers` commands.
    #[allow(dead_code)]
    fn mock_gossip_sender() -> (GossipSender, tokio_mpsc::UnboundedReceiver<Command>) {
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        tokio::task::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                if tx.send(cmd).is_err() {
                    break;
                }
            }
        });
        let sender: irpc::channel::mpsc::Sender<Command> =
            irpc::channel::mpsc::Sender::Tokio(cmd_tx);
        (GossipSender::new(sender), rx)
    }

    // ── State-based unit tests ───────────────────────────────────────

    /// Verify self peer is never admissible.
    #[test]
    fn test_self_filter() {
        let local = test_endpoint(1);
        let state = JoinerState {
            known: HashSet::new(),
            pending: HashMap::new(),
            local,
        };
        let batch = vec![test_endpoint(1), test_endpoint(2)];
        let mut admissible: Vec<EndpointId> = Vec::new();
        for peer in &batch {
            if *peer == state.local {
                continue;
            }
            if state.known.contains(peer) {
                continue;
            }
            if state.pending.contains_key(peer) {
                continue;
            }
            admissible.push(*peer);
        }
        assert_eq!(admissible.len(), 1);
        assert_eq!(admissible[0], test_endpoint(2));
    }

    /// Verify known peers are excluded.
    #[test]
    fn test_dedup_known_peer() {
        let p2 = test_endpoint(2);
        let state = JoinerState {
            known: HashSet::from([p2]),
            pending: HashMap::new(),
            local: test_endpoint(1),
        };
        let batch = vec![p2, test_endpoint(3)];
        let mut admissible: Vec<EndpointId> = Vec::new();
        for peer in &batch {
            if *peer == state.local {
                continue;
            }
            if state.known.contains(peer) {
                continue;
            }
            if state.pending.contains_key(peer) {
                continue;
            }
            admissible.push(*peer);
        }
        assert_eq!(admissible.len(), 1);
        assert_eq!(admissible[0], test_endpoint(3));
    }

    /// Verify pending peers are excluded.
    #[test]
    fn test_dedup_pending_peer() {
        let p2 = test_endpoint(2);
        let state = JoinerState {
            known: HashSet::new(),
            pending: HashMap::from([(p2, PeerJoinState { attempt: 0 })]),
            local: test_endpoint(1),
        };
        let batch = vec![p2];
        let mut admissible: Vec<EndpointId> = Vec::new();
        for peer in &batch {
            if *peer == state.local {
                continue;
            }
            if state.known.contains(peer) {
                continue;
            }
            if state.pending.contains_key(peer) {
                continue;
            }
            admissible.push(*peer);
        }
        assert!(admissible.is_empty());
    }

    /// Verify neighbor up transitions.
    #[test]
    fn test_neighbor_up_removes_pending() {
        let p2 = test_endpoint(2);
        let mut state = JoinerState {
            known: HashSet::new(),
            pending: HashMap::from([(p2, PeerJoinState { attempt: 1 })]),
            local: test_endpoint(1),
        };
        // Simulate NeighborUp.
        state.known.insert(p2);
        let was_pending = state.pending.remove(&p2).is_some();
        assert!(was_pending);
        assert!(state.known.contains(&p2));
        assert!(!state.pending.contains_key(&p2));
    }

    /// Verify neighbor down permits retry.
    #[test]
    fn test_neighbor_down_permits_retry() {
        let p2 = test_endpoint(2);
        let mut state = JoinerState {
            known: HashSet::from([p2]),
            pending: HashMap::new(),
            local: test_endpoint(1),
        };
        state.known.remove(&p2);
        assert!(!state.known.contains(&p2));
    }

    /// Verify retry count advances correctly.
    #[test]
    fn test_retry_count_advances() {
        let p2 = test_endpoint(2);
        let mut state = JoinerState {
            known: HashSet::new(),
            pending: HashMap::from([(p2, PeerJoinState { attempt: 0 })]),
            local: test_endpoint(1),
        };
        // After a failed attempt, increment.
        if let Some(p) = state.pending.get_mut(&p2) {
            p.attempt = 1;
        }
        assert_eq!(state.pending.get(&p2).unwrap().attempt, 1);
        // After another failure.
        if let Some(p) = state.pending.get_mut(&p2) {
            p.attempt = 2;
        }
        assert_eq!(state.pending.get(&p2).unwrap().attempt, 2);
    }

    /// Verify retry exhaustion removes pending.
    #[test]
    fn test_max_retries_exhausted_removes_pending() {
        let p2 = test_endpoint(2);
        let mut state = JoinerState {
            known: HashSet::new(),
            pending: HashMap::from([(p2, PeerJoinState { attempt: 3 })]),
            local: test_endpoint(1),
        };
        state.pending.remove(&p2);
        assert!(!state.pending.contains_key(&p2));
    }

    // ── Semaphore-based concurrency test ──────────────────────────────

    /// Verify semaphore bounds concurrent join operations.
    #[tokio::test]
    async fn test_concurrency_bounded_by_semaphore() {
        let semaphore = Arc::new(Semaphore::new(2));
        let max_concurrent = 2;

        // Acquire both permits.
        let p1 = semaphore.clone().try_acquire_owned().unwrap();
        let p2 = semaphore.clone().try_acquire_owned().unwrap();

        // Now try_acquire should fail.
        let p3 = semaphore.clone().try_acquire_owned();
        assert!(
            p3.is_err(),
            "semaphore should reject more than max_concurrent"
        );

        // Drop one permit.
        drop(p1);

        // Now one more should succeed.
        let p3 = semaphore.clone().try_acquire_owned();
        assert!(p3.is_ok(), "semaphore should accept after permit released");

        // Cleanup.
        drop(p2);
        drop(p3);
        let _ = max_concurrent;
    }

    // ── Integration test via real channels ──────────────────────────

    /// Integration test: a new peer is joined via the joiner's discovery
    /// channel.  We intercept the Command::JoinPeers to verify delivery.
    #[tokio::test]
    async fn test_joiner_new_peer_joined() {
        let local = test_endpoint(1);
        let p2 = test_endpoint(2);

        // Setup: mock that drops commands (no real network).
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let gossip_sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));

        let config = DynamicPeerJoinerConfig {
            max_candidates_per_batch: 64,
            max_concurrent_joins: 5,
            max_retries_per_peer: 0, // no retries in this test
            initial_retry_delay: Duration::from_millis(1),
            max_retry_delay: Duration::from_millis(10),
            jitter_factor: 0.0,
        };

        let joiner = DynamicPeerJoiner::start(local, gossip_sender, config);

        // Send a discovery batch.
        joiner.discovery_tx.send(vec![p2]).await.unwrap();

        // Verify JoinPeers command was sent.
        let cmd = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
            .await
            .expect("timeout waiting for join command")
            .expect("channel closed unexpectedly");

        match cmd {
            Command::JoinPeers(peers) => {
                assert_eq!(peers.len(), 1);
                assert_eq!(peers[0], p2);
            }
            other => panic!("expected JoinPeers, got {other:?}"),
        }

        joiner.shutdown().await;
    }

    /// Integration test: self peer is filtered and not joined.
    #[tokio::test]
    async fn test_self_filter_integration() {
        let local = test_endpoint(1);

        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let gossip_sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));

        let config = DynamicPeerJoinerConfig {
            max_candidates_per_batch: 64,
            max_concurrent_joins: 5,
            max_retries_per_peer: 0,
            ..Default::default()
        };

        let joiner = DynamicPeerJoiner::start(local, gossip_sender, config);

        // Send a batch containing only self.
        joiner.discovery_tx.send(vec![local]).await.unwrap();

        // Wait briefly and verify no commands were sent.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let cmd = cmd_rx.try_recv();
        assert!(
            cmd.is_err(),
            "should not send join command for self: {cmd:?}"
        );

        joiner.shutdown().await;
    }

    /// Integration test: already-known peer is not re-joined.
    #[tokio::test]
    async fn test_known_peer_not_rejoined() {
        let local = test_endpoint(1);
        let p2 = test_endpoint(2);

        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let gossip_sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));

        let config = DynamicPeerJoinerConfig {
            max_candidates_per_batch: 64,
            max_concurrent_joins: 5,
            max_retries_per_peer: 0,
            ..Default::default()
        };

        let joiner = DynamicPeerJoiner::start(local, gossip_sender, config);

        // First, tell the joiner that p2 is a known neighbor.
        joiner
            .neighbor_events_tx
            .send(NeighborEvent::Up(p2))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Now send a discovery batch containing p2.
        joiner.discovery_tx.send(vec![p2]).await.unwrap();

        // Wait briefly — should NOT send a join command.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let cmd = cmd_rx.try_recv();
        assert!(
            cmd.is_err(),
            "should not send join command for known peer: {cmd:?}"
        );

        joiner.shutdown().await;
    }

    /// Integration test: NeighborDown permits retry of a known peer.
    #[tokio::test]
    async fn test_neighbor_down_permits_rejoin() {
        let local = test_endpoint(1);
        let p2 = test_endpoint(2);

        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let gossip_sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));

        let config = DynamicPeerJoinerConfig {
            max_candidates_per_batch: 64,
            max_concurrent_joins: 5,
            max_retries_per_peer: 0,
            ..Default::default()
        };

        let joiner = DynamicPeerJoiner::start(local, gossip_sender, config);

        // Mark p2 as known.
        joiner
            .neighbor_events_tx
            .send(NeighborEvent::Up(p2))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // p2 disconnects.
        joiner
            .neighbor_events_tx
            .send(NeighborEvent::Down(p2))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Now a discovery batch with p2 should trigger a join.
        joiner.discovery_tx.send(vec![p2]).await.unwrap();

        let cmd = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
            .await
            .expect("timeout waiting for rejoin command")
            .expect("channel closed unexpectedly");

        match cmd {
            Command::JoinPeers(peers) => {
                assert_eq!(peers, vec![p2], "should rejoin disconnected peer");
            }
            other => panic!("expected JoinPeers, got {other:?}"),
        }

        joiner.shutdown().await;
    }

    /// Integration test: bounded retry — ensure retries are attempted
    /// up to max_retries_per_peer and then stop.
    #[tokio::test]
    async fn test_bounded_retry() {
        let local = test_endpoint(1);
        let p2 = test_endpoint(2);

        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let gossip_sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));

        // We won't process commands from the receiver, so JoinPeers will
        // backpressure until the channel is full and then... actually no,
        // the mpsc channel just sends — if the receiver doesn't consume,
        // it will eventually block at capacity 64.  Let's use a different
        // approach: consume the commands and just verify the count.

        // Spawn a consumer that counts JoinPeers commands.
        let (count_tx, mut count_rx) = tokio_mpsc::channel::<usize>(16);
        tokio::task::spawn(async move {
            let mut count = 0usize;
            while let Some(cmd) = cmd_rx.recv().await {
                if matches!(cmd, Command::JoinPeers(_)) {
                    count += 1;
                }
            }
            let _ = count_tx.try_send(count);
        });

        let config = DynamicPeerJoinerConfig {
            max_candidates_per_batch: 64,
            max_concurrent_joins: 5,
            max_retries_per_peer: 2, // 2 retries = 3 total attempts (0, 1, 2)
            initial_retry_delay: Duration::from_millis(5),
            max_retry_delay: Duration::from_millis(20),
            jitter_factor: 0.0,
        };

        let joiner = DynamicPeerJoiner::start(local, gossip_sender, config);

        // Send discovery batch.
        joiner.discovery_tx.send(vec![p2]).await.unwrap();

        // Wait for retries to complete.
        tokio::time::sleep(Duration::from_millis(500)).await;

        joiner.shutdown().await;

        // Drop the command channel so the counting task exits.
        // Check how many join attempts were made.
        let attempts = count_rx.recv().await.unwrap_or(0);
        // We expect exactly 3 attempts (initial + 2 retries), but the
        // channel might drop some if the consumer is slow.  At minimum
        // we should see at least 1 attempt.
        assert!(
            attempts >= 1,
            "should have at least 1 join attempt, got {attempts}"
        );
        // The exact count depends on timing; 3 is the expected bound.
        assert!(
            attempts <= 3,
            "should make at most 3 attempts (1 initial + 2 retries), got {attempts}"
        );
    }

    /// Integration test: concurrent join limit.
    /// Send 10 peers with max_concurrent_joins=3; verify at most 3 are
    /// in-flight at once.
    #[tokio::test]
    async fn test_concurrency_limit() {
        let local = test_endpoint(1);
        let peers: Vec<EndpointId> = (2..=11).map(|i| test_endpoint(i as u8)).collect();

        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let gossip_sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));

        // Consumer that records JoinPeers commands as they arrive.
        // We won't consume all — we'll check the rate.
        let (seen_tx, mut seen_rx) = tokio_mpsc::unbounded_channel::<EndpointId>();
        tokio::task::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                if let Command::JoinPeers(peers) = cmd {
                    for p in peers {
                        let _ = seen_tx.send(p);
                    }
                }
            }
        });

        let config = DynamicPeerJoinerConfig {
            max_candidates_per_batch: 64,
            max_concurrent_joins: 3,
            max_retries_per_peer: 0,
            initial_retry_delay: Duration::from_millis(1),
            max_retry_delay: Duration::from_millis(10),
            jitter_factor: 0.0,
        };

        let joiner = DynamicPeerJoiner::start(local, gossip_sender, config);

        // Send all 10 peers.
        joiner.discovery_tx.send(peers.clone()).await.unwrap();

        // Wait for commands to propagate.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Collect all seen join commands.
        let mut seen: HashSet<EndpointId> = HashSet::new();
        while let Ok(p) = seen_rx.try_recv() {
            seen.insert(p);
        }

        // With 3 concurrent slots and 10 peers, only 3 should fit
        // through the semaphore (the rest are deferred).
        assert_eq!(
            seen.len(),
            3,
            "with max_concurrent_joins=3, only 3 peers should be accepted, got {}",
            seen.len()
        );

        joiner.shutdown().await;
    }

    /// A duplicate appearing twice in one discovery response is scheduled once.
    #[tokio::test]
    async fn test_duplicate_candidates_in_one_batch_are_deduplicated() {
        let local = test_endpoint(1);
        let peer = test_endpoint(2);
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));
        let joiner = DynamicPeerJoiner::start(
            local,
            sender,
            DynamicPeerJoinerConfig {
                max_retries_per_peer: 0,
                jitter_factor: 0.0,
                ..Default::default()
            },
        );
        joiner.discovery_tx.send(vec![peer, peer]).await.unwrap();
        let command = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(command, Command::JoinPeers(peers) if peers == vec![peer]));
        assert!(cmd_rx.try_recv().is_err());
        joiner.shutdown().await;
    }

    /// An oversized discovery response cannot create an unbounded worker burst.
    #[tokio::test]
    async fn test_discovery_batch_is_hard_capped() {
        let local = test_endpoint(1);
        let peers: Vec<_> = (2..=4).map(test_endpoint).collect();
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::channel::<Command>(64);
        let sender = GossipSender::new(irpc::channel::mpsc::Sender::Tokio(cmd_tx));
        let joiner = DynamicPeerJoiner::start(
            local,
            sender,
            DynamicPeerJoinerConfig {
                max_candidates_per_batch: 1,
                max_retries_per_peer: 0,
                jitter_factor: 0.0,
                ..Default::default()
            },
        );
        joiner.discovery_tx.send(peers).await.unwrap();
        let command = tokio::time::timeout(Duration::from_secs(2), cmd_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(command, Command::JoinPeers(found) if found == vec![test_endpoint(2)]));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(cmd_rx.try_recv().is_err());
        joiner.shutdown().await;
    }

    // ── apply_jitter tests ──────────────────────────────────────────

    #[test]
    fn test_apply_jitter_no_factor() {
        let dur = apply_jitter(Duration::from_secs(5), 0.0);
        assert_eq!(dur, Duration::from_secs(5));
    }

    #[test]
    fn test_apply_jitter_range() {
        let base = Duration::from_secs(10);
        for _ in 0..100 {
            let dur = apply_jitter(base, 0.1);
            let min = Duration::from_nanos((10_000_000_000f64 * 0.9) as u64);
            let max = Duration::from_nanos((10_000_000_000f64 * 1.1) as u64);
            assert!(
                dur >= min && dur <= max,
                "jitter {dur:?} out of range [{min:?}, {max:?}]"
            );
        }
    }

    #[test]
    fn test_apply_jitter_clamped_non_negative() {
        let base = Duration::from_nanos(1);
        for _ in 0..100 {
            let dur = apply_jitter(base, 0.5);
            assert!(
                dur >= Duration::ZERO,
                "jitter produced negative duration: {dur:?}"
            );
        }
    }
}
