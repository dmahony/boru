//! DiagnosticCounters / DirectoryCounters tests.

use super::*;

/// Every record_* method bumps exactly its own counter, and the snapshot
/// reflects the four required discovery counters plus the
/// unsupported-version counter.
#[test]
fn diagnostic_counters_record_and_snapshot() {
    let counters = DiagnosticCounters::new();
    assert_eq!(counters.snapshot(), DiagnosticCountersSnapshot::default());

    counters.record_discovery_peer_seen();
    counters.record_direct_topic_joined();
    counters.record_group_topic_joined();
    counters.record_malformed_discovery_packet();
    counters.record_unsupported_version_packet();

    let snap = counters.snapshot();
    assert_eq!(snap.discovery_peers_seen, 1);
    assert_eq!(snap.direct_topics_joined, 1);
    assert_eq!(snap.group_topics_joined, 1);
    assert_eq!(snap.malformed_discovery_packets, 1);
    assert_eq!(snap.unsupported_version_packets, 1);

    // A second bump accumulates.
    counters.record_direct_topic_joined();
    assert_eq!(counters.direct_topics_joined(), 2);
    assert_eq!(counters.group_topics_joined(), 1);
}

/// Clones share the same underlying atomics — the global
/// [`DIAGNOSTIC_COUNTERS`] can be bumped from the discovery service and
/// read from the frontend without a lock.
#[test]
fn diagnostic_counters_clone_shares_atomics() {
    let a = DiagnosticCounters::new();
    let b = a.clone();
    b.record_direct_topic_joined();
    b.record_group_topic_joined();
    assert_eq!(a.direct_topics_joined(), 1);
    assert_eq!(a.group_topics_joined(), 1);
}

/// The snapshot type derives PartialEq/Eq so assertions read cleanly.
#[test]
fn diagnostic_counters_snapshot_eq() {
    let a = DiagnosticCounters::new().snapshot();
    let b = DiagnosticCounters::new().snapshot();
    assert_eq!(a, b);
}

// ── Directory counters (BORU-DIR-22, PDF Phase 8 Task 8.1) ────────

/// Every directory `record_*` method bumps exactly its own counter, and
/// the snapshot reflects all seven required advertisement counters.
#[test]
fn directory_counters_record_and_snapshot() {
    let counters = DirectoryCounters::new();
    assert_eq!(counters.snapshot(), DirectoryCountersSnapshot::default());

    counters.record_advertisement_received();
    counters.record_advertisement_accepted();
    counters.record_advertisement_rejected();
    counters.record_advertisement_expired();
    counters.record_advertisement_withdrawn();
    counters.record_advertisement_deduplicated();
    counters.record_advertisement_rate_limited();

    let snap = counters.snapshot();
    assert_eq!(snap.advertisements_received, 1);
    assert_eq!(snap.advertisements_accepted, 1);
    assert_eq!(snap.advertisements_rejected, 1);
    assert_eq!(snap.advertisements_expired, 1);
    assert_eq!(snap.advertisements_withdrawn, 1);
    assert_eq!(snap.advertisements_deduplicated, 1);
    assert_eq!(snap.advertisements_rate_limited, 1);

    // A second bump accumulates independently.
    counters.record_advertisement_received();
    counters.record_advertisement_accepted();
    assert_eq!(counters.advertisements_received(), 2);
    assert_eq!(counters.advertisements_accepted(), 2);
    assert_eq!(counters.advertisements_rejected(), 1);
    assert_eq!(counters.advertisements_expired(), 1);
}

/// Directory counter clones share the same underlying atomics — the
/// discovery service and the room-directory cache can bump the same
/// global [`DIRECTORY_COUNTERS`] and the frontend reads one value.
#[test]
fn directory_counters_clone_shares_atomics() {
    let a = DirectoryCounters::new();
    let b = a.clone();
    b.record_advertisement_received();
    b.record_advertisement_expired();
    assert_eq!(a.advertisements_received(), 1);
    assert_eq!(a.advertisements_expired(), 1);
}

/// The `expired_sink` handle shares the expired counter's atomic — the
/// room-directory cache can bump the same counter the service reads.
#[test]
fn directory_counters_expired_sink_shares_atomic() {
    let counters = DirectoryCounters::new();
    let sink = counters.expired_sink();
    sink.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(counters.advertisements_expired(), 1);
    assert_eq!(counters.snapshot().advertisements_expired, 1);
}

// ── DHT effectiveness counters (BORU-DHT-08) ──────────────────────

/// A `record_lookup` bumps `lookup_cycles`, `records_received`,
/// `records_valid`, and each rejected-by-reason counter from the supplied
/// `DhtLookupCounts`.  The snapshot carries every pipeline stage.
#[test]
fn dht_counters_record_lookup_and_snapshot() {
    let counters = DhtCounters::new();
    let snap0 = counters.snapshot();
    assert_eq!(snap0.lookup_cycles, 0);
    assert_eq!(snap0.records_received, 0);
    assert_eq!(snap0.records_valid, 0);

    // First lookup: 10 encrypted received, 2 valid, various rejections.
    counters.record_lookup(
        10,
        DhtLookupCounts {
            valid: 2,
            oversized: 1,
            stale: 1,
            future: 1,
            decode: 1,
            identity: 1,
            signature: 1,
            own: 1,
            duplicate: 1,
            ..Default::default()
        },
    );
    // Second lookup: no records at all.
    counters.record_lookup(0, DhtLookupCounts::default());

    let snap = counters.snapshot();
    assert_eq!(snap.lookup_cycles, 2);
    assert_eq!(snap.records_received, 10);
    assert_eq!(snap.records_valid, 2);
    assert_eq!(snap.rejected_oversized, 1);
    assert_eq!(snap.rejected_stale, 1);
    assert_eq!(snap.rejected_future, 1);
    assert_eq!(snap.rejected_decode, 1);
    assert_eq!(snap.rejected_identity, 1);
    assert_eq!(snap.rejected_signature, 1);
    assert_eq!(snap.rejected_self, 1);
    assert_eq!(snap.rejected_duplicate, 1);

    // A rejected-only batch does not change valid/cycles incorrectly.
    counters.record_lookup_failure();
    assert_eq!(counters.snapshot().lookup_failures, 1);
    assert_eq!(counters.snapshot().lookup_cycles, 2);
}

/// Recording new unique candidates accumulates the count and sets the
/// time-to-first-candidate exactly once (a 0-count is a no-op).
#[test]
fn dht_counters_unique_candidates_sets_first_candidate_timing() {
    let counters = DhtCounters::new();
    assert_eq!(counters.snapshot().first_candidate_at_ms, 0);

    counters.record_unique_candidates(0);
    assert_eq!(counters.snapshot().unique_candidates, 0);
    assert_eq!(counters.snapshot().first_candidate_at_ms, 0);

    counters.record_unique_candidates(3);
    let after_first = counters.snapshot().first_candidate_at_ms;
    assert!(after_first > 0, "first-candidate timing should be set");
    assert_eq!(counters.snapshot().unique_candidates, 3);

    // A later batch does not overwrite the first-candidate timestamp.
    counters.record_unique_candidates(2);
    assert_eq!(counters.snapshot().unique_candidates, 5);
    assert_eq!(counters.snapshot().first_candidate_at_ms, after_first);
}

/// Queue, dropped, join-attempt, join-success, degraded-transition and
/// neighbour-up counters each bump independently; neighbour-up sets the
/// time-to-first-neighbour exactly once.
#[test]
fn dht_counters_queue_join_neighbour_and_degraded() {
    let counters = DhtCounters::new();

    counters.record_queued(2);
    counters.record_dropped(1);
    counters.record_join_attempt(4);
    counters.record_join_success(3);
    counters.record_degraded_transition();
    counters.record_degraded_transition();

    let snap = counters.snapshot();
    assert_eq!(snap.queued, 2);
    assert_eq!(snap.dropped, 1);
    assert_eq!(snap.join_attempts, 4);
    assert_eq!(snap.join_successes, 3);
    assert_eq!(snap.degraded_transitions, 2);
    assert_eq!(snap.first_neighbour_at_ms, 0);

    counters.record_neighbour_up();
    let after = counters.snapshot().first_neighbour_at_ms;
    assert!(after > 0, "first-neighbour timing should be set");
    counters.record_neighbour_up();
    assert_eq!(counters.snapshot().first_neighbour_at_ms, after);
}

/// DHT counter clones share the same underlying atomics — the trackers and
/// the MCP layer read one shared value.
#[test]
fn dht_counters_clone_shares_atomics() {
    let a = DhtCounters::new();
    let b = a.clone();
    b.record_lookup(
        5,
        DhtLookupCounts {
            valid: 1,
            ..Default::default()
        },
    );
    b.record_join_success(2);
    assert_eq!(a.snapshot().records_received, 5);
    assert_eq!(a.snapshot().records_valid, 1);
    assert_eq!(a.snapshot().join_successes, 2);
}

/// The snapshot is serializable (for MCP/doctor tooling) and round-trips.
#[test]
fn dht_effectiveness_snapshot_serializes() {
    let counters = DhtCounters::new();
    counters.record_lookup(
        3,
        DhtLookupCounts {
            valid: 1,
            ..Default::default()
        },
    );
    counters.record_unique_candidates(1);
    counters.record_join_attempt(1);
    counters.record_neighbour_up();

    let snap = counters.snapshot();
    let json = serde_json::to_string(&snap).expect("snapshot serializes");
    let back: DhtEffectivenessSnapshot =
        serde_json::from_str(&json).expect("snapshot deserializes");
    assert_eq!(back.lookup_cycles, snap.lookup_cycles);
    assert_eq!(back.unique_candidates, snap.unique_candidates);
    assert_eq!(back.first_neighbour_at_ms, snap.first_neighbour_at_ms);
}
