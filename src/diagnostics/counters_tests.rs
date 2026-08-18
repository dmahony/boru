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
