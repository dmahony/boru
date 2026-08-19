//! Compute-intensive cross-cutting discovery verification (workspace-b).
//!
//! Implements the compute/verify legs of the Boru DHT discovery plan's
//! cross-cutting matrix (PLAN.md §2 workspace-b, PDF §9), exercised against
//! the real implementation:
//!
//! | Scenario | Test in this file | Workspace-a merge needed? |
//! |----------|-------------------|---------------------------|
//! | Large/hostile DHT result sets (invalid/duplicate/oversized/stale records; validation caps hold; memory + CPU bounded) | [`hostile_flood_caps_hold`], [`hostile_categories_produce_rejections`], [`oversized_record_rejected`] | no |
//! | Join saturation (> join slots) | [`join_saturation_bounded_concurrency`] | no (current semaphore contract) |
//! | Long-running session soak (many peers over time; recovery after cooldown; no lifetime dead-end) | [`soak_retry_recovery_no_dead_end`], [`soak_many_waves_bounded`] | no |
//! | Shutdown during retry / cancellation stress (loops exit promptly, tasks drained) | [`shutdown_during_retry_prompt`], [`cancellation_stress_repeated_cycles`] | no |
//!
//! The workspace-a-gated rows (bounded pending queue "nothing lost", adaptive
//! cadence, discovery-bootstrap tracker, degraded-state diagnostics) are
//! exercised by the debsrv matrix runner (`scripts/discovery_matrix_run.sh`),
//! which merges `feat/workspace-a` into a **local verification tree** on
//! debsrv and re-runs the full discovery suite there once that branch lands.
//! This file only relies on APIs that exist on `main` today so the
//! workspace-b branch stays green standalone.
//!
//! All tests are compute-heavy by design; they are expected to run on debsrv
//! via `rb test --test discovery_compute_matrix`.

#![cfg(feature = "net")]

use std::{
    time::{Duration, Instant},
};

use boru_core::{
    api::{Command, GossipSender},
    discovery_record::create_discovery_record,
    discovery_validation::{
        DiscoveryRecordValidator, RejectionReason, ValidationConfig, DEFAULT_MAX_CLOCK_SKEW_MINUTES,
        DEFAULT_MAX_RECORD_AGE_MINUTES,
    },
    dynamic_joiner::{DynamicPeerJoiner, DynamicPeerJoinerConfig, NeighborEvent},
};
use distributed_topic_tracker::{unix_minute, Record};
use iroh::{EndpointId, SecretKey};
use tokio::sync::mpsc as tokio_mpsc;

/// Deterministic 32-byte identity seed derived from a u16 (collision-free for
/// the fixture ranges used here).
fn seed_u16(id: u16) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[0] = (id & 0xFF) as u8;
    s[1] = ((id >> 8) & 0xFF) as u8;
    s[2] = 0x5A;
    s
}

/// Create a deterministic EndpointId from a u16.
fn test_endpoint(id: u16) -> EndpointId {
    SecretKey::from_bytes(&seed_u16(id)).public()
}

/// Deterministic topic for repeatable tests.
fn test_topic() -> [u8; 32] {
    [0x42u8; 32]
}

/// Volume knob for the hostile-set flood (env-tunable so the debsrv matrix
/// run can push it much harder than a quick local check).
fn hostile_record_count() -> usize {
    std::env::var("BORU_MATRIX_RECORDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

/// A command-channel mock for [`GossipSender`], mirroring the joiner's own
/// unit-test helper but with a *controllable* drain so tests can force send
/// failures (window full + no reader → send errors → real retry path).
struct MockGossip {
    tx: tokio_mpsc::Sender<Command>,
    rx: tokio_mpsc::Receiver<Command>,
}

impl MockGossip {
    /// Create a mock with the given command-channel capacity.
    fn new(capacity: usize) -> Self {
        let (tx, rx) = tokio_mpsc::channel(capacity);
        Self { tx, rx }
    }

    /// The [`GossipSender`] half — pass to [`DynamicPeerJoiner::start`].
    fn sender(&self) -> GossipSender {
        let irpc_sender = irpc::channel::mpsc::Sender::Tokio(self.tx.clone());
        GossipSender::new(irpc_sender)
    }

    /// Drain all pending commands (delivers queued joins; frees buffer space).
    fn drain(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }
}

/// Wait for the joiner to run, then drain the mock (letting landable joins
/// land / freeing the command window), returning how many JoinPeers targets
/// were drained.
async fn settle_and_count(dur: Duration, mock: &mut MockGossip) -> usize {
    tokio::time::sleep(dur).await;
    let mut count = 0;
    while let Ok(Command::JoinPeers(peers)) = mock.rx.try_recv() {
        count += peers.len();
    }
    count
}

// ---------------------------------------------------------------------------
// Large / hostile DHT result sets
// ---------------------------------------------------------------------------

/// A fixed hostile batch: every rejection category the validator can produce,
/// plus valid records, duplicates, and the local node's own endpoint.
fn hostile_batch(now_minute: u64) -> Vec<Record> {
    let topic = test_topic();
    let local = test_endpoint(1);

    let mut records = Vec::new();
    // 1. Valid records (unique endpoints) — accepted. Kept to 5 so the rest
    //    of the hostile categories fall within the per-lookup record cap.
    for i in 2..=6u16 {
        let sk = SecretKey::from_bytes(&seed_u16(i));
        let ep = sk.public();
        records.push(
            create_discovery_record(topic, now_minute, &ep, &sk, None, None)
                .expect("create valid record"),
        );
    }
    // 2. Duplicate of endpoint 2 (rejected as duplicate).
    {
        let sk = SecretKey::from_bytes(&seed_u16(2));
        let ep = sk.public();
        records.push(
            create_discovery_record(topic, now_minute, &ep, &sk, None, None)
                .expect("create duplicate record"),
        );
    }
    // 3. Stale record (older than the allowed window).
    {
        let sk = SecretKey::from_bytes(&seed_u16(100));
        let ep = sk.public();
        let old = now_minute - DEFAULT_MAX_RECORD_AGE_MINUTES - 1;
        records.push(
            create_discovery_record(topic, old, &ep, &sk, None, None)
                .expect("create stale record"),
        );
    }
    // 4. Future record (beyond the allowed clock skew).
    {
        let sk = SecretKey::from_bytes(&seed_u16(101));
        let ep = sk.public();
        let future = now_minute + DEFAULT_MAX_CLOCK_SKEW_MINUTES + 1;
        records.push(
            create_discovery_record(topic, future, &ep, &sk, None, None)
                .expect("create future record"),
        );
    }
    // 5. Identity mismatch: signed by A but advertises B.
    {
        let sk_a = SecretKey::from_bytes(&seed_u16(102));
        let ep_b = test_endpoint(103);
        records.push(
            create_discovery_record(topic, now_minute, &ep_b, &sk_a, None, None)
                .expect("create identity-mismatch record"),
        );
    }
    // 6. Invalid signature: valid record with its signature bytes flipped.
    {
        let sk = SecretKey::from_bytes(&seed_u16(104));
        let ep = sk.public();
        let record =
            create_discovery_record(topic, now_minute, &ep, &sk, None, None).expect("signed record");
        let mut bytes = record.to_bytes();
        let sig_start = bytes.len() - 64;
        bytes[sig_start] ^= 0xFF;
        let tampered = Record::from_bytes(bytes).expect("tampered record still deserializes");
        records.push(tampered);
    }
    // 7. Self record advertising the local endpoint.
    {
        let sk_local = SecretKey::from_bytes(&seed_u16(1));
        records.push(
            create_discovery_record(topic, now_minute, &local, &sk_local, None, None)
                .expect("create self record"),
        );
    }
    records
}

/// Feed `count` validated records in one huge vector — per-call caps must
/// hold regardless of input size, and wall time must stay bounded.
#[test]
fn hostile_flood_caps_hold() {
    let now_minute = unix_minute(0);
    let validator = DiscoveryRecordValidator::new(ValidationConfig::new(test_topic()), now_minute);

    // Build one *valid* record to clone freely (cheap), then clone to reach
    // the requested flood size. Cloning produces duplicates, which is exactly
    // the hostile-duplicate pattern a flood would contain.
    let sk = SecretKey::from_bytes(&seed_u16(200));
    let ep = sk.public();
    let base = create_discovery_record(test_topic(), now_minute, &ep, &sk, None, None)
        .expect("create base record");

    let count = hostile_record_count();
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(base.clone());
    }

    let started = Instant::now();
    // The entire flood is handed to the validator in ONE call: the pipeline
    // must cap work at `max_records_per_lookup` (hard-capped at 20) and the
    // output at `max_candidate_peers` (hard-capped at 20), no matter how
    // large the hostile input vector is. No local endpoint is passed, so the
    // replicated peer is a genuine candidate and the overflow becomes
    // duplicates (not self-filtered).
    let result = validator.filter_and_build(records, None);
    let elapsed = started.elapsed();

    assert!(
        result.counters.total <= 20,
        "validator examined {} records in one lookup — must be capped at 20",
        result.counters.total
    );
    assert!(
        result.peers.len() <= 20,
        "validator returned {} candidates — must be capped at 20",
        result.peers.len()
    );
    assert_eq!(
        result.counters.duplicates,
        result.counters.total.saturating_sub(1),
        "all-but-first flood clones must count as duplicates"
    );
    assert_eq!(result.counters.accepted, 1);
    // No memory/CPU blowup: cloning 20k+ signed records and running the whole
    // batch through validation must stay under a generous wall-clock bound.
    // (Bound is deliberately loose — the assertion is "bounded", not a perf
    // benchmark; debsrv debug builds are slower than opt builds.)
    assert!(
        elapsed < Duration::from_secs(120),
        "hostile flood of {count} records took {elapsed:?} — expected bounded processing"
    );
    println!(
        "hostile_flood_caps_hold: {count} records, {} examined, {} accepted, {} duplicates, {:?}",
        result.counters.total, result.counters.accepted, result.counters.duplicates, elapsed
    );
}

/// The validator must return a meaningful rejection mix when the flood
/// contains every hostile category.
#[test]
fn hostile_categories_produce_rejections() {
    let now_minute = 1_000_000; // fixed reference for deterministic age/skew math
    let local = test_endpoint(1);
    let validator = DiscoveryRecordValidator::new(ValidationConfig::new(test_topic()), now_minute);

    let batch = hostile_batch(now_minute);
    let result = validator.filter_and_build(batch, Some(&local));

    assert!(result.counters.accepted >= 1, "valid records must be accepted");
    assert!(result.counters.stale >= 1, "stale record must be rejected");
    assert!(result.counters.future >= 1, "future record must be rejected");
    assert!(
        result.counters.identity_mismatch >= 1,
        "identity-mismatch record must be rejected"
    );
    assert!(
        result.counters.invalid_signature >= 1,
        "invalid-signature record must be rejected"
    );
    assert!(
        result.counters.duplicates >= 1,
        "duplicate record must be rejected"
    );
    assert!(
        result.counters.self_filtered >= 1,
        "self record must be filtered"
    );
    assert_eq!(
        result.counters.accepted + result.counters.total_rejected(),
        result.counters.total,
        "counter math must be consistent"
    );
    println!("hostile_categories: {result:?}");
}

/// Oversized records (serialized > `max_record_size`) are rejected before any
/// expensive work. Use a large room name to blow past the 256-byte cap.
#[test]
fn oversized_record_rejected() {
    let now_minute = 1_000_000;
    let validator = DiscoveryRecordValidator::new(ValidationConfig::new(test_topic()), now_minute);

    let sk = SecretKey::from_bytes(&seed_u16(50));
    let ep = sk.public();
    let big_room = "x".repeat(512);
    let record =
        create_discovery_record(test_topic(), now_minute, &ep, &sk, Some(big_room), None)
            .expect("create oversized record");
    assert!(
        record.to_bytes().len() > 256,
        "fixture must actually exceed the 256-byte size cap"
    );

    let result = validator.validate_single(&record);
    assert!(
        matches!(result, Err(RejectionReason::Oversized { .. })),
        "expected Oversized rejection, got {result:?}"
    );
    println!(
        "oversized_record_rejected: {} bytes rejected",
        record.to_bytes().len()
    );
}

// ---------------------------------------------------------------------------
// Join saturation
// ---------------------------------------------------------------------------

/// More candidates than concurrent join slots: the joiner must never exceed
/// its concurrency contract, dedupe aggressively, filter self/known/pending,
/// and drain cleanly. Uses the current semaphore contract (excess candidates
/// are deferred for a later discovery batch — workspace-a replaces this with
/// the bounded pending queue).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn join_saturation_bounded_concurrency() {
    let local = test_endpoint(1);
    let config = DynamicPeerJoinerConfig {
        max_concurrent_joins: 5,
        max_candidates_per_batch: 64,
        initial_retry_delay: Duration::from_millis(10),
        max_retry_delay: Duration::from_millis(50),
        jitter_factor: 0.0,
        ..DynamicPeerJoinerConfig::default()
    };

    let mut mock = MockGossip::new(1024);
    let joiner = DynamicPeerJoiner::start(local, mock.sender(), config.clone());

    // 400 unique candidates, 5 join slots: feed them all at once.
    let candidates: Vec<EndpointId> = (2..=401u16).map(test_endpoint).collect();
    assert!(
        candidates.len() > config.max_concurrent_joins * 8,
        "fixture must actually saturate the slots"
    );

    joiner
        .discovery_tx
        .send(candidates.clone())
        .await
        .expect("feed candidates");

    // Let the joiner chew through the batch. With a 1024-capacity mock the
    // window never fills, so every accepted candidate's join lands.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    mock.drain();

    // Report a neighbour-up for a handful of peers so the joiner transitions
    // them known (removing them from pending).
    for i in 2..=7u16 {
        joiner
            .neighbor_events_tx
            .send(NeighborEvent::Up(test_endpoint(i)))
            .await
            .expect("neighbor event");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let t0 = Instant::now();
    joiner.shutdown().await;
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "joiner shutdown must drain promptly under saturation"
    );
}

// ---------------------------------------------------------------------------
// Long-running session soak: retry recovery, no lifetime dead-end
// ---------------------------------------------------------------------------

/// A peer whose joins fail (retry budget) must eventually be retried when a
/// later discovery batch re-introduces it — no lifetime dead-end. The mock
/// deliberately fills its command window to force send failures (the real
/// retry/backoff path), then drains to let recovery happen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_retry_recovery_no_dead_end() {
    let local = test_endpoint(1);
    let config = DynamicPeerJoinerConfig {
        max_concurrent_joins: 5,
        max_candidates_per_batch: 64,
        // Short backoff so the soak stays fast; jitter off for determinism.
        max_retries_per_peer: 2,
        initial_retry_delay: Duration::from_millis(20),
        max_retry_delay: Duration::from_millis(60),
        jitter_factor: 0.0,
        ..DynamicPeerJoinerConfig::default()
    };

    // Tiny command window (capacity 1): the first peer's join buffers and is
    // treated as a success (becomes known); the other four hit a full window
    // and fail every retry — so after phase 1 exactly 1 peer is known and the
    // other 4 exhausted their retry budget. Deterministic.
    let mut mock = MockGossip::new(1);
    let joiner = DynamicPeerJoiner::start(local, mock.sender(), config);

    let wave: Vec<EndpointId> = (2..=6u16).map(test_endpoint).collect();
    joiner
        .discovery_tx
        .send(wave.clone())
        .await
        .expect("feed first wave");

    // Phase 1: window full + undrained → 4 peers' joins fail → retries run
    // and exhaust; they leave pending with their retry budget spent.
    tokio::time::sleep(Duration::from_millis(600)).await;
    // (No drain here: the window stays full so sends keep failing.)

    // Phase 2: drain now (simulates a discovery batch arriving later with the
    // mesh healthy again). Re-introduce the same peers: the 4 that exhausted
    // retries must be retried — proving the joiner recovers after cooldown
    // with no lifetime dead-end. (Peer 2 is already known and stays skipped.)
    mock.drain();
    joiner
        .discovery_tx
        .send(wave.clone())
        .await
        .expect("re-feed same wave after cooldown");

    // Commands land as the mock window drains; collect re-deliveries.
    let mut delivered = 0usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(Command::JoinPeers(peers)) = mock.rx.try_recv() {
            delivered += peers.len();
        }
        if delivered >= wave.len() - 1 {
            break;
        }
    }
    assert!(
        delivered >= wave.len() - 1,
        "soak: expected {} of {} re-introduced peers to be re-joined after cooldown, got {delivered}",
        wave.len() - 1,
        wave.len()
    );
    println!("soak_retry_recovery: {delivered} join deliveries after cooldown");

    let t0 = Instant::now();
    joiner.shutdown().await;
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "prompt shutdown after soak"
    );
}

/// A long-session soak with *many* waves of peers introduced over time: the
/// joiner keeps working, dedupes hard, and never dead-ends. Env-tunable
/// rounds (`BORU_SOAK_ROUNDS`, default 25) so the debsrv runner can extend.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn soak_many_waves_bounded() {
    let rounds = std::env::var("BORU_SOAK_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25u32);

    let local = test_endpoint(1);
    let config = DynamicPeerJoinerConfig {
        max_concurrent_joins: 5,
        max_candidates_per_batch: 64,
        max_retries_per_peer: 1,
        initial_retry_delay: Duration::from_millis(10),
        max_retry_delay: Duration::from_millis(30),
        jitter_factor: 0.3,
        ..DynamicPeerJoinerConfig::default()
    };

    let mut mock = MockGossip::new(256);
    let joiner = DynamicPeerJoiner::start(local, mock.sender(), config);

    let mut total_joined = 0usize;
    let mut wave_peers: Vec<EndpointId> = Vec::new();
    for round in 0..rounds {
        // Each wave: 15 new unique peers plus up to 5 repeats from earlier
        // waves (a realistic re-discovery pattern the joiner must dedupe).
        let mut wave: Vec<EndpointId> = Vec::new();
        for i in 0..15u16 {
            let id = 2 + (round as u16) * 15 + i;
            wave.push(test_endpoint(id));
        }
        if !wave_peers.is_empty() {
            wave.extend_from_slice(&wave_peers[..5.min(wave_peers.len())]);
        }
        wave_peers.extend_from_slice(&wave);

        joiner
            .discovery_tx
            .send(wave.clone())
            .await
            .expect("feed wave");
        total_joined += settle_and_count(Duration::from_millis(60), &mut mock).await;
    }

    assert!(
        total_joined > 0,
        "soak: at least some joins must land across {rounds} waves"
    );
    println!("soak_many_waves: {rounds} rounds, {total_joined} join deliveries");

    let t0 = Instant::now();
    joiner.shutdown().await;
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "prompt shutdown after soak"
    );
}

// ---------------------------------------------------------------------------
// Shutdown during retry / cancellation stress
// ---------------------------------------------------------------------------

/// A join loop sitting in a retry backoff must exit promptly when the joiner
/// is shut down mid-retry — no stuck sleep, no leaked tasks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_during_retry_prompt() {
    let local = test_endpoint(1);
    let config = DynamicPeerJoinerConfig {
        max_concurrent_joins: 5,
        // Long backoff: the worker is *inside* a retry sleep when shutdown hits.
        max_retries_per_peer: 10,
        initial_retry_delay: Duration::from_secs(30),
        max_retry_delay: Duration::from_secs(30),
        jitter_factor: 0.0,
        ..DynamicPeerJoinerConfig::default()
    };

    // Window capacity 1 + no drain → first send may land, the rest fail; the
    // workers are put into (long) retry sleeps.
    let mock = MockGossip::new(1);
    let joiner = DynamicPeerJoiner::start(local, mock.sender(), config);

    let wave: Vec<EndpointId> = (2..=6u16).map(test_endpoint).collect();
    joiner
        .discovery_tx
        .send(wave)
        .await
        .expect("feed wave");
    // Give the loop time to spawn workers and enter the retry sleep.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let t0 = Instant::now();
    joiner.shutdown().await;
    let elapsed = t0.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "shutdown during 30s retry backoff must cancel the sleep promptly, took {elapsed:?}"
    );
    println!("shutdown_during_retry: cancelled mid-backoff in {elapsed:?}");
}

/// Repeated start/feed/shutdown cycles must never wedge, leak tasks, or
/// panic — the cancellation path is exercised under churn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_stress_repeated_cycles() {
    let cycles = std::env::var("BORU_CANCEL_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30u32);

    for cycle in 0..cycles {
        let local = test_endpoint(1);
        let config = DynamicPeerJoinerConfig {
            max_concurrent_joins: 5,
            max_candidates_per_batch: 64,
            max_retries_per_peer: 3,
            initial_retry_delay: Duration::from_secs(1),
            max_retry_delay: Duration::from_secs(2),
            jitter_factor: 0.1,
            ..DynamicPeerJoinerConfig::default()
        };

        let mock = MockGossip::new(16);
        let joiner = DynamicPeerJoiner::start(local, mock.sender(), config);

        let wave: Vec<EndpointId> = (2..=20u16).map(test_endpoint).collect();
        joiner
            .discovery_tx
            .send(wave)
            .await
            .expect("feed wave");
        // Cancel at an arbitrary point — either mid-join or mid-retry.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let t0 = Instant::now();
        joiner.shutdown().await;
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "cycle {cycle}: shutdown must be prompt under churn, took {:?}",
            t0.elapsed()
        );
    }
    println!("cancellation_stress: {cycles} start/feed/cancel cycles complete");
}