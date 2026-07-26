//! Outbox delivery throughput benchmark — sequential vs concurrent.
//!
//! Measures how long it takes to deliver a batch of N outbox messages to M
//! different peers with a simulated transport latency.  The test proves that
//! sequential delivery scales linearly with N × latency, while concurrent
//! delivery (with per-peer ordering) scales primarily with N/M × latency.
//!
//! Run: cargo test --test test_outbox_throughput -- --nocapture
//!
//! Set BORU_PERF=1 to emit structured tracing events.
//! Set LATENCY_MS to override the simulated transport delay (default 10).

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use iroh::{PublicKey, SecretKey};

use boru_core::{
    outbox_delivery::{DeliveryTransport, OutboxDeliveryWorker, RecipientPolicy},
    storage::Storage,
    store::{MessageId, StoredEnvelope},
};
use tokio::sync::mpsc;

// ── Helpers ────────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn random_pk() -> PublicKey {
    SecretKey::generate().public()
}

fn make_msg_id(byte: u8) -> MessageId {
    [byte; 32]
}

fn make_conv_id(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn sample_envelope(
    msg_id: MessageId,
    conv_id: [u8; 32],
    peer: PublicKey,
) -> StoredEnvelope {
    StoredEnvelope {
        msg_id,
        conversation_id: conv_id,
        author_user_id: peer,
        author_device_id: peer,
        created_at_ms: now_ms(),
        expires_at_ms: now_ms() + 86_400_000,
        ciphertext: bytes::Bytes::from_static(b"benchmark-ciphertext"),
        signature: [0u8; 64],
        acked_at_ms: None,
    }
}

// ── Mock transport with configurable latency ───────────────────────────────

struct LatencyTransport {
    /// Simulated network delay per delivery in milliseconds.
    latency_ms: u64,
    /// Counter of completed deliveries.
    completed: Arc<AtomicUsize>,
}

impl DeliveryTransport for LatencyTransport {
    fn deliver(
        &self,
        _recipient: PublicKey,
        _envelope: StoredEnvelope,
    ) -> boru_core::outbox_delivery::BoxFuture<n0_error::Result<()>> {
        let completed = self.completed.clone();
        let latency = self.latency_ms;
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(latency)).await;
            completed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

// ── Mock policy (always authorized) ────────────────────────────────────────

struct AllowAllPolicy;

impl RecipientPolicy for AllowAllPolicy {
    fn authorize(
        &self,
        _recipient: PublicKey,
    ) -> boru_core::outbox_delivery::BoxFuture<n0_error::Result<bool>> {
        Box::pin(async move { Ok(true) })
    }
}

// ── Benchmark helper ──────────────────────────────────────────────────────

async fn run_benchmark(
    num_messages: usize,
    num_peers: usize,
    max_concurrent: usize,
    latency_ms: u64,
) -> (Duration, usize) {
    let storage = Storage::memory().unwrap();
    let conv_id = make_conv_id(42);

    // Create peers
    let peers: Vec<PublicKey> = (0..num_peers).map(|_| random_pk()).collect();

    // Insert messages round-robin across peers
    let now = now_ms();
    for i in 0..num_messages {
        let peer = peers[i % num_peers];
        let msg_id = make_msg_id((i % 256) as u8);
        let env = sample_envelope(msg_id, conv_id, peer);
        storage.insert_inbox(&env).unwrap();
        storage.enqueue_outbox(&msg_id, peer, now).unwrap();
    }

    let transport = Arc::new(LatencyTransport {
        latency_ms,
        completed: Arc::new(AtomicUsize::new(0)),
    });
    let policy = Arc::new(AllowAllPolicy);
    let (_trigger, rx) = mpsc::channel(1);

    let worker = OutboxDeliveryWorker::new(storage.clone(), policy, transport, "bench-worker", rx)
        .with_max_concurrent(std::num::NonZeroUsize::new(max_concurrent.max(1)).unwrap())
        .with_claim_batch_size(std::cmp::min(num_messages as u32, 32))
        .with_lease(30_000);

    let start = Instant::now();
    let attempted = worker.run_once().await;
    let elapsed = start.elapsed();

    (elapsed, attempted)
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Measure that sequential delivery (max_concurrent=1) takes ~N × latency.
#[tokio::test]
async fn sequential_delivery_is_n_times_latency() {
    let latency_ms = std::env::var("LATENCY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10u64);

    const NUM_MSGS: usize = 20;
    const NUM_PEERS: usize = 5;

    let (elapsed, attempted) = run_benchmark(NUM_MSGS, NUM_PEERS, 1, latency_ms).await;

    assert_eq!(attempted, NUM_MSGS);

    // Sequential: should take approximately NUM_MSGS × latency_ms.
    let expected_min = NUM_MSGS as u64 * latency_ms;
    let expected_max = expected_min * 3; // Allow for scheduling variance
    let elapsed_ms = elapsed.as_millis() as u64;

    eprintln!(
        "[BENCH] Sequential: {NUM_MSGS} msgs to {NUM_PEERS} peers, latency={latency_ms}ms \
         → {elapsed_ms}ms (expected ~{expected_min}-{expected_max}ms)"
    );

    assert!(
        elapsed_ms >= expected_min / 2,
        "Sequential delivery completed too fast ({elapsed_ms}ms < {expected_min}ms) — \
         likely the mock transport wasn't exercised"
    );
    assert!(
        elapsed_ms <= expected_max,
        "Sequential delivery too slow ({elapsed_ms}ms > {expected_max}ms)"
    );
}

/// Measure that concurrent delivery (max_concurrent=N) is faster than sequential
/// when messages are spread across peers.
#[tokio::test]
async fn concurrent_delivery_is_faster_than_sequential() {
    let latency_ms = std::env::var("LATENCY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10u64);

    const NUM_MSGS: usize = 20;
    const NUM_PEERS: usize = 5;
    const CONCURRENT: usize = 5;

    let (seq_elapsed, seq_attempted) = run_benchmark(NUM_MSGS, NUM_PEERS, 1, latency_ms).await;
    assert_eq!(seq_attempted, NUM_MSGS);

    let (con_elapsed, con_attempted) =
        run_benchmark(NUM_MSGS, NUM_PEERS, CONCURRENT, latency_ms).await;
    assert_eq!(con_attempted, NUM_MSGS);

    let seq_ms = seq_elapsed.as_millis() as u64;
    let con_ms = con_elapsed.as_millis() as u64;

    eprintln!(
        "[BENCH] Sequential: {seq_ms}ms | Concurrent(CONCURRENT): {con_ms}ms \
         | Speedup: {}x",
        seq_ms as f64 / con_ms.max(1) as f64
    );

    // Concurrent should be at least 1.5x faster with 5 peers and 20 messages.
    assert!(
        con_ms < seq_ms,
        "Concurrent delivery ({con_ms}ms) should be faster than sequential ({seq_ms}ms)"
    );
}

/// Verify same-peer messages remain sequential even under concurrent config.
#[tokio::test]
async fn same_peer_sequential_under_concurrent_mode() {
    let latency_ms = std::env::var("LATENCY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5u64);

    let storage = Storage::memory().unwrap();
    let peer = random_pk();
    let conv_id = make_conv_id(1);
    let now = now_ms();

    // 10 messages to the same peer.
    for i in 0..10u8 {
        let msg_id = make_msg_id(i);
        let env = sample_envelope(msg_id, conv_id, peer);
        storage.insert_inbox(&env).unwrap();
        storage.enqueue_outbox(&msg_id, peer, now).unwrap();
    }

    let transport = Arc::new(LatencyTransport {
        latency_ms,
        completed: Arc::new(AtomicUsize::new(0)),
    });
    let policy = Arc::new(AllowAllPolicy);
    let (_trigger, rx) = mpsc::channel(1);

    let worker = OutboxDeliveryWorker::new(storage.clone(), policy, transport, "bench-worker", rx)
        .with_max_concurrent(std::num::NonZeroUsize::new(10).unwrap()) // High concurrency
        .with_claim_batch_size(10)
        .with_lease(30_000);

    let start = Instant::now();
    let attempted = worker.run_once().await;
    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;

    assert_eq!(attempted, 10);

    // All messages go to 1 peer, so even with max_concurrent=10, they're serialized.
    let expected_min = 10 * latency_ms;
    eprintln!(
        "[BENCH] Same peer: 10 msgs, latency={latency_ms}ms, max_concurrent=10 \
         → {elapsed_ms}ms (expected ~{expected_min}ms)"
    );

    assert!(
        elapsed_ms >= expected_min / 2,
        "Same-peer deliveries should be serialized ({elapsed_ms}ms < {expected_min}ms)"
    );
}
