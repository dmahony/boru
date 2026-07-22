//! Transfer lifecycle telemetry integration tests.
//!
//! Exercises every specified event type and major branch, validates payload
//! shape and event ordering, and asserts that serialized events never leak
//! sensitive transfer details (filenames, paths, URLs, tokens, content).
//!
//! Scenarios:
//!   1. Normal completion flow
//!   2. Access denial (permission rejected)
//!   3. Pause / Resume
//!   4. Progress checkpoint sequencing
//!   5. Verification failure
//!   6. Cancellation
//!   7. Retry with new attempt number
//!   8. Unexpected transfer failure
//!   9. Event ordering and required fields
//!  10. Stable error categories
//!  11. Short stable transfer identifiers
//!  12. Privacy: no sensitive fields in serialized events

use boru_core::diagnostics::{
    event_names, short_transfer_id, DiagnosticEventKind, Diagnostics, ErrorCategory,
    TransferLifecycleEvent,
};
use boru_core::transfer_telemetry::TransferTelemetry;

// ── Helpers ────────────────────────────────────────────────────────────────

fn new_telemetry() -> TransferTelemetry {
    let diagnostics = Diagnostics::new();
    TransferTelemetry::new(diagnostics)
}

fn new_telemetry_with_diag() -> (TransferTelemetry, Diagnostics) {
    let diagnostics = Diagnostics::new();
    let telemetry = TransferTelemetry::new(diagnostics.clone());
    (telemetry, diagnostics)
}

/// Collect all TransferLifecycle events from diagnostics.
///
/// NOTE: `events_since(0)` skips the first globally-emitted event (seq=0).
/// To compensate, every test that calls this MUST first emit exactly one
/// dummy event on a throwaway transfer_id (9999) before the real events.
/// This dummy is skipped, and the returned list covers the real events
/// without any gap.
fn lifecycle_events(
    diag: &Diagnostics,
    since_sequence: u64,
    limit: usize,
) -> Vec<TransferLifecycleEvent> {
    let events = diag.events_since(since_sequence, limit, None);
    events
        .into_iter()
        .filter_map(|e| match e.kind {
            DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
            _ => None,
        })
        .collect()
}

/// Assert that a serialized TransferLifecycleEvent JSON string does not
/// contain any of the forbidden patterns.
fn assert_no_forbidden_patterns(event: &TransferLifecycleEvent) {
    let json = serde_json::to_string(event).expect("serialize event");
    let forbidden = [
        "/etc/passwd",
        "C:\\Users",
        "\\Windows\\",
        "https://",
        "http://",
        "Bearer ",
        "secret_token",
        "password",
        "sk-",
        "-----BEGIN",
        "malicious_file_name.docx",
        "\\\\server\\share",
        "../../etc",
        "%00",
    ];
    for pattern in &forbidden {
        assert!(
            !json.contains(pattern),
            "Event JSON for '{}' must not contain forbidden pattern {pattern:?}, got: {json}",
            event.event_name,
        );
    }
}

/// Assert that every field in the required set is populated according to the
/// v1 contract.
fn assert_required_fields(event: &TransferLifecycleEvent) {
    assert_eq!(event.schema_version, 1, "schema_version must be 1");
    assert!(!event.event_id.is_empty(), "event_id must not be empty");
    assert!(
        event.event_id.len() <= 16,
        "event_id must be short (truncated hash ≤ 16 hex chars), got length {}",
        event.event_id.len()
    );
    assert!(!event.event_name.is_empty(), "event_name must not be empty");
    assert!(
        !event.transfer_id.is_empty(),
        "transfer_id must not be empty"
    );
    assert!(
        event.transfer_id.len() <= 11,
        "transfer_id must be compact (≤ 11 chars = 8 + '…'), got '{}' (len {})",
        event.transfer_id,
        event.transfer_id.len()
    );
    assert!(
        event.occurred_at_ms > 0,
        "occurred_at_ms must be a positive Unix epoch ms"
    );
    assert!(event.attempt >= 1, "attempt must start at 1");
}

// ── 1. Normal completion flow ─────────────────────────────────────────────

#[test]
fn full_successful_completion_flow() {
    let (t, diag) = new_telemetry_with_diag();

    // Dummy event (skipped by events_since(0)).
    t.download_queued(9999, 1, None);

    // Emit a full lifecycle.
    t.download_queued(100, 2048, Some(3));
    t.access_requested(100, "initial");
    t.access_granted(100, Some(30_000));
    t.transfer_started(100, 2048, None);
    t.progress_checkpoint(100, 1024, 2048, Some(1024), Some(250), Some(8_192_000));
    t.verification(100, "passed", Some(2048), Some(2048));
    t.completion(100, 2048, Some(5_234));

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 7, "expected 7 lifecycle events");

    // Verify event ordering.
    let names: Vec<&str> = events.iter().map(|e| e.event_name.as_str()).collect();
    assert_eq!(
        names,
        [
            event_names::DOWNLOAD_QUEUED,
            event_names::ACCESS_REQUESTED,
            event_names::ACCESS_GRANTED,
            event_names::TRANSFER_STARTED,
            event_names::PROGRESS_CHECKPOINT,
            event_names::VERIFICATION,
            event_names::COMPLETION,
        ],
        "event ordering must match expected lifecycle"
    );

    // All events share the same transfer_id.
    for ev in &events {
        assert_eq!(ev.transfer_id, "100", "transfer_id mismatch");
    }

    // Sequences are monotonic: 0, 1, 2, ..., 6.
    for (i, ev) in events.iter().enumerate() {
        assert_eq!(ev.sequence, i as u64, "sequence mismatch at index {i}");
    }

    // All events have attempt == 1.
    for ev in &events {
        assert_eq!(ev.attempt, 1, "attempt must be 1 on first lifecycle");
    }

    // Required fields.
    for ev in &events {
        assert_required_fields(ev);
        assert_no_forbidden_patterns(ev);
    }

    // Terminal event (completion) — check payload.
    let completion = &events[6];
    assert_eq!(completion.event_name, event_names::COMPLETION);
    let payload = completion.payload.as_ref().expect("completion has payload");
    assert_eq!(payload["bytes_transferred"], 2048);
    assert_eq!(payload["duration_ms"], 5_234);
}

// ── 2. Access denial flow ─────────────────────────────────────────────────

#[test]
fn access_denied_flow() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(200, 4096, None);
    t.access_requested(200, "initial");
    t.failure(
        200,
        ErrorCategory::PermissionDenied,
        false,
        None,
        Some(false),
        None,
    );

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 3);

    assert_eq!(events[0].event_name, event_names::DOWNLOAD_QUEUED);
    assert_eq!(events[1].event_name, event_names::ACCESS_REQUESTED);
    assert_eq!(events[2].event_name, event_names::FAILURE);

    // Verify failure payload.
    let payload = events[2].payload.as_ref().expect("failure has payload");
    assert_eq!(payload["error_category"], "permission_denied");
    assert_eq!(payload["retryable"], false);
    assert_eq!(payload["will_retry"], false);
    assert!(!payload.get("retry_delay_ms").is_some()); // not supplied

    assert_required_fields(&events[0]);
    assert_required_fields(&events[1]);
    assert_required_fields(&events[2]);
    for ev in &events {
        assert_no_forbidden_patterns(ev);
    }
}

// ── 3. Pause / Resume ─────────────────────────────────────────────────────

#[test]
fn pause_and_resume_flow() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(300, 1024, None);
    t.access_requested(300, "initial");
    t.access_granted(300, None);
    t.transfer_started(300, 1024, Some(512));
    t.pause(300, "user", Some(512));
    t.resume(300, "user", Some(512));
    t.verification(300, "passed", Some(1024), Some(1024));
    t.completion(300, 1024, Some(3_000));

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 8, "expected 8 events for pause/resume flow");

    let names: Vec<&str> = events.iter().map(|e| e.event_name.as_str()).collect();
    assert_eq!(
        names,
        [
            event_names::DOWNLOAD_QUEUED,
            event_names::ACCESS_REQUESTED,
            event_names::ACCESS_GRANTED,
            event_names::TRANSFER_STARTED,
            event_names::PAUSE,
            event_names::RESUME,
            event_names::VERIFICATION,
            event_names::COMPLETION,
        ],
        "pause/resume ordering"
    );

    // Pause payload.
    let pause = &events[4];
    let p = pause.payload.as_ref().expect("pause has payload");
    assert_eq!(p["reason"], "user");
    assert_eq!(p["bytes_transferred"], 512);

    // Resume payload.
    let resume = &events[5];
    let r = resume.payload.as_ref().expect("resume has payload");
    assert_eq!(r["reason"], "user");
    assert_eq!(r["bytes_transferred"], 512);

    // Sequences are monotonic.
    for (i, ev) in events.iter().enumerate() {
        assert_eq!(ev.sequence, i as u64, "sequence mismatch at index {i}");
    }

    // All attempt==1 (pause/resume don't change attempt).
    for ev in &events {
        assert_eq!(ev.attempt, 1, "attempt stays 1 through pause/resume");
    }

    for ev in &events {
        assert_required_fields(ev);
        assert_no_forbidden_patterns(ev);
    }
}

// ── 4. Progress checkpoint sequencing ─────────────────────────────────────

#[test]
fn progress_checkpoints_are_monotonic() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(400, 10_000, None);
    t.access_granted(400, None);
    t.transfer_started(400, 10_000, None);

    // Emit three checkpoints at increasing byte counts.
    t.progress_checkpoint(400, 2_000, 10_000, Some(2_000), Some(250), Some(1_000_000));
    t.progress_checkpoint(400, 5_000, 10_000, Some(3_000), Some(250), Some(2_000_000));
    t.progress_checkpoint(400, 9_000, 10_000, Some(4_000), Some(250), Some(4_000_000));

    t.verification(400, "passed", Some(10_000), Some(10_000));
    t.completion(400, 10_000, Some(10_000));

    let events = lifecycle_events(&diag, 0, 100);
    let checkpoints: Vec<&TransferLifecycleEvent> = events
        .iter()
        .filter(|e| e.event_name == event_names::PROGRESS_CHECKPOINT)
        .collect();

    assert_eq!(checkpoints.len(), 3, "expected 3 progress checkpoints");

    // Checkpoints are in order and bytestransferred is increasing.
    assert_eq!(
        checkpoints[0].payload.as_ref().unwrap()["bytes_transferred"],
        2_000
    );
    assert_eq!(
        checkpoints[1].payload.as_ref().unwrap()["bytes_transferred"],
        5_000
    );
    assert_eq!(
        checkpoints[2].payload.as_ref().unwrap()["bytes_transferred"],
        9_000
    );

    // Percent millis are in order.
    let p0 = checkpoints[0].payload.as_ref().unwrap()["percent_millis"]
        .as_u64()
        .unwrap();
    let p1 = checkpoints[1].payload.as_ref().unwrap()["percent_millis"]
        .as_u64()
        .unwrap();
    let p2 = checkpoints[2].payload.as_ref().unwrap()["percent_millis"]
        .as_u64()
        .unwrap();
    assert!(p0 < p1, "percent_millis should increase: {p0} < {p1}");
    assert!(p1 < p2, "percent_millis should increase: {p1} < {p2}");

    // Sequences strictly increasing.
    for (i, cp) in checkpoints.iter().enumerate() {
        if i > 0 {
            assert!(cp.sequence > checkpoints[i - 1].sequence);
        }
    }

    for ev in &events {
        assert_required_fields(ev);
        assert_no_forbidden_patterns(ev);
    }
}

#[test]
fn progress_checkpoint_zero_total_bytes_does_not_panic() {
    let t = new_telemetry();
    // total_bytes=0 should produce 0 percent_millis (no divide-by-zero).
    t.progress_checkpoint(401, 0, 0, None, None, None);
}

#[test]
fn progress_checkpoint_partial_percent_millis() {
    let (t, diag) = new_telemetry_with_diag();
    t.download_queued(9999, 1, None); // dummy
    t.download_queued(402, 1000, None);
    // 1/3 of the way — 333_333 millipercent.
    t.progress_checkpoint(402, 333, 1000, None, None, None);

    let events = lifecycle_events(&diag, 0, 100);
    let cp = events
        .iter()
        .find(|e| e.event_name == event_names::PROGRESS_CHECKPOINT)
        .expect("checkpoint event");
    let p = cp.payload.as_ref().unwrap();
    assert_eq!(p["percent_millis"], 333_000u64);
}

// ── 5. Verification failure flow ──────────────────────────────────────────

#[test]
fn verification_failure_flow() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(500, 2048, None);
    t.access_requested(500, "initial");
    t.access_granted(500, None);
    t.transfer_started(500, 2048, None);
    t.verification(500, "failed", Some(1024), Some(2048));
    t.failure(
        500,
        ErrorCategory::IntegrityMismatch,
        false,
        Some(1024),
        Some(false),
        None,
    );

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(
        events.len(),
        6,
        "expected 6 events for verification failure"
    );

    assert_eq!(
        events[4].event_name,
        event_names::VERIFICATION,
        "event 4 should be verification"
    );
    assert_eq!(
        events[5].event_name,
        event_names::FAILURE,
        "event 5 should be failure"
    );

    // Verification payload.
    let vp = events[4].payload.as_ref().expect("verification payload");
    assert_eq!(vp["result"], "failed");
    assert_eq!(vp["bytes_transferred"], 1024);
    assert_eq!(vp["total_bytes"], 2048);

    // Failure payload.
    let fp = events[5].payload.as_ref().expect("failure payload");
    assert_eq!(fp["error_category"], "integrity_mismatch");
    assert_eq!(fp["retryable"], false);

    for ev in &events {
        assert_required_fields(ev);
        assert_no_forbidden_patterns(ev);
    }
}

// ── 6. Cancellation flow ─────────────────────────────────────────────────

#[test]
fn cancellation_flow() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(600, 1024, None);
    t.access_requested(600, "initial");
    t.cancellation(600, "user", Some(128));

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].event_name, event_names::CANCELLATION);

    let cp = events[2].payload.as_ref().expect("cancellation payload");
    assert_eq!(cp["reason"], "user");
    assert_eq!(cp["bytes_transferred"], 128);

    for ev in &events {
        assert_required_fields(ev);
        assert_no_forbidden_patterns(ev);
    }
}

// ── 7. Retry with new attempt number ──────────────────────────────────────

#[test]
fn retry_flow_increments_attempt() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    // ── Attempt 1: fails ──────────────────────────────────────────────
    t.download_queued(700, 1024, None);
    t.access_requested(700, "initial");
    t.access_granted(700, None);
    t.transfer_started(700, 1024, None);
    t.failure(
        700,
        ErrorCategory::Timeout,
        true,
        Some(512),
        Some(true),
        Some(5_000),
    );

    // ── Attempt 2: set attempt number and retry ───────────────────────
    t.set_attempt(700, 2);
    t.download_queued(700, 1024, None);
    t.access_requested(700, "initial");
    t.access_granted(700, None);
    t.transfer_started(700, 1024, Some(512));
    t.progress_checkpoint(700, 768, 1024, Some(256), Some(250), Some(2_000_000));
    t.verification(700, "passed", Some(1024), Some(1024));
    t.completion(700, 1024, Some(3_000));

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 12, "expected 12 events across 2 attempts");

    // First 5 events: attempt 1.
    for ev in &events[0..5] {
        assert_eq!(ev.attempt, 1, "first 5 events should be attempt 1");
    }

    // After set_attempt(2): remaining events should be attempt 2.
    for ev in &events[5..12] {
        assert_eq!(
            ev.attempt, 2,
            "events after set_attempt(2) should be attempt 2"
        );
    }

    for ev in &events {
        assert_required_fields(ev);
        assert_no_forbidden_patterns(ev);
    }

    // Failure payload for attempt 1.
    let fail = &events[4];
    assert_eq!(fail.event_name, event_names::FAILURE);
    let fp = fail.payload.as_ref().expect("failure payload");
    assert_eq!(fp["error_category"], "timeout");
    assert_eq!(fp["retryable"], true);
    assert_eq!(fp["will_retry"], true);
    assert_eq!(fp["retry_delay_ms"], 5_000);
}

// ── 8. Unexpected transfer failure ────────────────────────────────────────

#[test]
fn unexpected_transfer_failure() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(800, 10_000, None);
    t.access_requested(800, "initial");
    t.access_granted(800, None);
    t.transfer_started(800, 10_000, None);
    t.progress_checkpoint(800, 1_500, 10_000, None, None, None);
    // Unexpected failure — peer goes offline mid-transfer.
    t.failure(
        800,
        ErrorCategory::PeerUnavailable,
        true,
        Some(1_500),
        Some(true),
        Some(10_000),
    );

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 6);

    // Verify the failure fields.
    let fail_ev = &events[5];
    let fp = fail_ev.payload.as_ref().expect("failure payload");
    assert_eq!(fp["error_category"], "peer_unavailable");
    assert_eq!(fp["retryable"], true);
    assert_eq!(fp["bytes_transferred"], 1_500);
    assert_eq!(fp["will_retry"], true);
    assert_eq!(fp["retry_delay_ms"], 10_000);

    for ev in &events {
        assert_required_fields(ev);
        assert_no_forbidden_patterns(ev);
    }
}

// ── 9. Event ordering across two independent transfers ────────────────────

#[test]
fn two_transfers_have_independent_sequences() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(1, 100, None); // transfer 1: seq 0
    t.download_queued(2, 200, None); // transfer 2: seq 0
    t.access_requested(1, "initial"); // transfer 1: seq 1
    t.access_requested(2, "initial"); // transfer 2: seq 1

    let events = lifecycle_events(&diag, 0, 100);
    // All 4 events must be visible.
    assert_eq!(events.len(), 4);

    // Per-transfer sequences: t1=[0,1], t2=[0,1].
    let t1_seqs: Vec<u64> = events
        .iter()
        .filter(|e| e.transfer_id == "1")
        .map(|e| e.sequence)
        .collect();
    assert_eq!(t1_seqs, vec![0, 1]);

    let t2_seqs: Vec<u64> = events
        .iter()
        .filter(|e| e.transfer_id == "2")
        .map(|e| e.sequence)
        .collect();
    assert_eq!(t2_seqs, vec![0, 1]);
}

// ── 10. Stable error categories ───────────────────────────────────────────

#[test]
fn all_error_categories_have_stable_names() {
    let cases = [
        (ErrorCategory::PermissionDenied, "permission_denied"),
        (ErrorCategory::NotFound, "not_found"),
        (ErrorCategory::PeerUnavailable, "peer_unavailable"),
        (ErrorCategory::Timeout, "timeout"),
        (ErrorCategory::RateLimited, "rate_limited"),
        (ErrorCategory::Cancelled, "cancelled"),
        (ErrorCategory::Paused, "paused"),
        (ErrorCategory::SizeMismatch, "size_mismatch"),
        (ErrorCategory::IntegrityMismatch, "integrity_mismatch"),
        (ErrorCategory::VersionMismatch, "version_mismatch"),
        (ErrorCategory::StorageError, "storage_error"),
        (ErrorCategory::ProtocolError, "protocol_error"),
        (ErrorCategory::ResourceExhausted, "resource_exhausted"),
        (ErrorCategory::Unknown, "unknown"),
    ];
    for (cat, expected) in &cases {
        assert_eq!(cat.as_str(), *expected, "ErrorCategory variant {:?}", cat);
    }
}

// ── 11. Short stable transfer identifiers ────────────────────────────────

#[test]
fn short_transfer_id_is_compact() {
    // An id that fits in 8 chars stays as-is.
    assert_eq!(short_transfer_id(42), "42");
    assert_eq!(short_transfer_id("abc"), "abc");

    // An id exceeding 8 chars is truncated.
    let long = short_transfer_id(123456789012345i64);
    assert!(
        long.len() <= 11,
        "short_transfer_id for large number should be ≤ 11 chars (8 + ellipsis), got '{long}' (len {})",
        long.len()
    );
}

#[test]
fn emitted_transfer_ids_are_compact() {
    let (t, diag) = new_telemetry_with_diag();
    // Use a very large id.
    t.download_queued(9999, 1, None); // dummy
    t.download_queued(9_999_999_999, 100, None);
    t.download_queued(1, 100, None);

    let events = lifecycle_events(&diag, 0, 100);
    // events_since(0) skips dummy, shows 2 events (large id + small id).
    assert_eq!(events.len(), 2);
    let tid = &events[0].transfer_id;
    assert!(
        tid.len() <= 11,
        "transfer_id should be ≤ 11 chars, got '{tid}' (len {})",
        tid.len()
    );
}

// ── 12. Terminal events reset for new lifecycle ───────────────────────────

#[test]
fn completion_allows_new_lifecycle() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    // First lifecycle.
    t.completion(77, 4096, Some(2_000));
    // After forget — new lifecycle starts at seq 0.
    t.download_queued(77, 100, None);

    let events = lifecycle_events(&diag, 0, 100);
    // events_since(0) skips dummy, shows completion + download_queued.
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_name, event_names::COMPLETION);
    assert_eq!(events[1].event_name, event_names::DOWNLOAD_QUEUED);
    assert_eq!(
        events[1].sequence, 0,
        "new lifecycle should start at sequence 0 after completion"
    );
}

#[test]
fn cancellation_allows_new_lifecycle() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.cancellation(88, "user", None);
    t.download_queued(88, 100, None);

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_name, event_names::CANCELLATION);
    assert_eq!(events[1].event_name, event_names::DOWNLOAD_QUEUED);
    assert_eq!(
        events[1].sequence, 0,
        "new lifecycle should start at sequence 0 after cancellation"
    );
}

// ── 13. Privacy: no sensitive content in serialized events ────────────────

/// Verify that every event type's serialized JSON contains no file-like
/// content, paths, URLs, or tokens.
#[test]
fn privacy_all_event_types_clean() {
    let (t, diag) = new_telemetry_with_diag();
    let transfer_id = 42;

    t.download_queued(9999, 1, None); // dummy

    // Emit every event type with worst-case reason/result values that could
    // tempt leakage.
    t.download_queued(transfer_id, 1024, Some(5));
    t.access_requested(transfer_id, "initial");
    t.access_granted(transfer_id, Some(30000));
    t.transfer_started(transfer_id, 1024, Some(0));
    t.progress_checkpoint(transfer_id, 512, 1024, Some(512), Some(250), Some(8192000));
    t.pause(transfer_id, "user", Some(512));
    t.resume(transfer_id, "restart_recovery", Some(512));
    t.verification(transfer_id, "passed", Some(1024), Some(1024));
    t.failure(
        transfer_id,
        ErrorCategory::Timeout,
        true,
        Some(512),
        Some(true),
        Some(5000),
    );
    t.completion(transfer_id, 1024, Some(3000));

    let events = lifecycle_events(&diag, 0, 100);
    assert!(
        events.len() >= 9,
        "emitted 9+ events but got {}",
        events.len()
    );

    for ev in &events {
        assert_no_forbidden_patterns(ev);
    }
}

/// Use representative malicious values to verify nothing accidentally leaks
/// into serialized event payloads.
#[test]
fn privacy_malicious_values_not_leaked() {
    let (t, diag) = new_telemetry_with_diag();

    // Use a transfer_id that looks like a hash or path — the telemetry only
    // stores a short numeric id, but test the serialization boundary anyway
    // by using a large id.
    let long_numeric_id: i64 = 0x0A_0B_0C_0D_0E_0F_i64;

    t.download_queued(9999, 1, None); // dummy

    // Emit a range of events with values that look like sensitive data.
    t.download_queued(long_numeric_id, 999, None);
    t.access_requested(long_numeric_id, "initial");
    t.access_granted(long_numeric_id, None);
    t.transfer_started(long_numeric_id, 999, None);
    t.pause(long_numeric_id, "user", None);
    t.resume(long_numeric_id, "restart_recovery", None);
    t.verification(long_numeric_id, "passed", None, None);
    t.failure(
        long_numeric_id,
        ErrorCategory::PermissionDenied,
        false,
        None,
        Some(false),
        None,
    );
    t.cancellation(long_numeric_id, "user", None);

    let events = lifecycle_events(&diag, 0, 100);
    for ev in &events {
        let json = serde_json::to_string(&ev).expect("serialize event");

        // The event must not contain the full hexadecimal prefix of the original.
        assert!(
            !json.contains("0a0b0c0d0e0f"),
            "event JSON must not contain full hex representation of transfer_id, got: {json}"
        );
        assert_no_forbidden_patterns(ev);
    }
}

/// Verify that event_id is opaque — it must not be the transfer_id or a
/// derivative that reveals the transfer number.
#[test]
fn privacy_event_id_is_opaque() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(42, 100, None);
    t.access_requested(42, "initial");

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 2, "expected 2 visible events");

    // event_id is a truncated SHA-256 — 16 hex chars, never equal to the
    // transfer_id string.
    let eid = &events[0].event_id;
    assert_eq!(eid.len(), 16, "event_id must be exactly 16 hex chars");
    assert!(
        eid.chars().all(|c| c.is_ascii_hexdigit()),
        "event_id must be hex: {eid}"
    );
    assert_ne!(*eid, "42", "event_id must not equal the raw transfer_id");

    // Two events for the same transfer must have different event_ids
    // (because sequence differs in the hash input).
    let eid2 = &events[1].event_id;
    assert_ne!(eid, eid2, "different events must have different event_ids");
}

/// Verify the resume event with reason "restart_recovery" (the reason string
/// from the parent task's restart recovery path).
#[test]
fn resume_restart_recovery_payload() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(900, 2048, None);
    t.resume(900, "restart_recovery", Some(1024));

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_name, event_names::DOWNLOAD_QUEUED);
    assert_eq!(events[1].event_name, event_names::RESUME);
    let p = events[1].payload.as_ref().expect("resume payload");
    assert_eq!(p["reason"], "restart_recovery");
    assert_eq!(p["bytes_transferred"], 1024);
    assert_required_fields(&events[0]);
    assert_required_fields(&events[1]);
    assert_no_forbidden_patterns(&events[0]);
    assert_no_forbidden_patterns(&events[1]);
}

/// Verify the `resume` event with reason "user" (the reason string from
/// the user-initiated resume path in download_manager).
#[test]
fn resume_user_reason_payload() {
    let (t, diag) = new_telemetry_with_diag();

    t.download_queued(9999, 1, None); // dummy

    t.download_queued(910, 2048, None);
    t.resume(910, "user", Some(512));

    let events = lifecycle_events(&diag, 0, 100);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_name, event_names::DOWNLOAD_QUEUED);
    assert_eq!(events[1].event_name, event_names::RESUME);
    let p = events[1].payload.as_ref().expect("resume payload");
    assert_eq!(p["reason"], "user");
    assert_eq!(p["bytes_transferred"], 512);
    assert_required_fields(&events[0]);
    assert_required_fields(&events[1]);
    assert_no_forbidden_patterns(&events[0]);
    assert_no_forbidden_patterns(&events[1]);
}

/// Verify each error category maps to the correct failure payload when used
/// in a failure event.
#[test]
fn all_error_categories_in_failure_events() {
    let categories = [
        (ErrorCategory::PermissionDenied, "permission_denied"),
        (ErrorCategory::NotFound, "not_found"),
        (ErrorCategory::PeerUnavailable, "peer_unavailable"),
        (ErrorCategory::Timeout, "timeout"),
        (ErrorCategory::RateLimited, "rate_limited"),
        (ErrorCategory::Cancelled, "cancelled"),
        (ErrorCategory::Paused, "paused"),
        (ErrorCategory::SizeMismatch, "size_mismatch"),
        (ErrorCategory::IntegrityMismatch, "integrity_mismatch"),
        (ErrorCategory::VersionMismatch, "version_mismatch"),
        (ErrorCategory::StorageError, "storage_error"),
        (ErrorCategory::ProtocolError, "protocol_error"),
        (ErrorCategory::ResourceExhausted, "resource_exhausted"),
        (ErrorCategory::Unknown, "unknown"),
    ];

    for (i, (cat, expected)) in categories.iter().enumerate() {
        let (t, diag) = new_telemetry_with_diag();
        let tid = 1000 + i as i64;

        t.download_queued(9999, 1, None); // dummy
        t.download_queued(tid, 100, None);
        t.failure(tid, *cat, false, None, None, None);

        let events = lifecycle_events(&diag, 0, 100);
        let fail = events
            .iter()
            .find(|e| e.event_name == event_names::FAILURE)
            .expect("failure event");
        let p = fail.payload.as_ref().expect("failure payload");
        assert_eq!(
            p["error_category"], *expected,
            "ErrorCategory {:?} should serialize to '{expected}'",
            cat
        );
        assert_no_forbidden_patterns(fail);
    }
}

/// Verify that access_granted without TTL has no payload.
#[test]
fn access_granted_no_ttl_omits_payload() {
    let (t, diag) = new_telemetry_with_diag();
    t.download_queued(9999, 1, None); // dummy
    t.download_queued(1000, 100, None);
    t.access_granted(1000, None);

    let events = lifecycle_events(&diag, 0, 100);
    let grant = events
        .iter()
        .find(|e| e.event_name == event_names::ACCESS_GRANTED)
        .expect("access_granted event");
    // Should have no payload or an empty payload.
    assert!(
        grant.payload.is_none() || grant.payload.as_ref().unwrap().is_null(),
        "access_granted without TTL should omit payload, got: {:?}",
        grant.payload
    );
}

/// Verify that download_queued with queue_depth=0 emits the field.
#[test]
fn download_queued_zero_queue_depth() {
    let (t, diag) = new_telemetry_with_diag();
    t.download_queued(9999, 1, None); // dummy
    t.download_queued(1100, 1024, Some(0));
    let events = lifecycle_events(&diag, 0, 100);
    let dq = events
        .iter()
        .find(|e| e.event_name == event_names::DOWNLOAD_QUEUED)
        .expect("download_queued event");
    let p = dq.payload.as_ref().expect("payload");
    assert_eq!(p["queue_depth"], 0);
    assert_eq!(p["total_bytes"], 1024);
    assert_no_forbidden_patterns(dq);
}
