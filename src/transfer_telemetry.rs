//! Transfer lifecycle telemetry — per-transfer sequence tracking + event emission.
//!
//! Implements the v1 event contract described in
//! `docs/design/transfer-lifecycle-events.md`.
//!
//! # Architecture
//!
//! [`TransferTelemetry`] is a small wrapper that owns per-transfer sequence
//! counters and emits structured [`TransferLifecycleEvent`]s into the shared
//! [`Diagnostics`] store via [`DiagnosticEventKind::TransferLifecycle`].
//!
//! Each public method accepts the minimal context needed (download id, byte
//! counts, error categories) and builds the correct envelope.  No filenames,
//! paths, hashes, or peer identifiers appear in event payloads.
//!
//! # Restrictions
//!
//! - `event_id` is a locally-generated opaque hex string (truncated SHA-256 of
//!   `transfer_id || sequence || occurred_at_ms`).
//! - `transfer_id` follows the existing [`short_transfer_id`] policy: at most
//!   8 ASCII chars plus `…` when shortened.
//! - No forbidden fields (filename, path, URL, peer, hash, token, etc.) may
//!   be passed to these methods — the caller is responsible for respecting
//!   the privacy contract.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::diagnostics::{
    event_names, short_transfer_id, DiagnosticEventKind, Diagnostics, ErrorCategory,
    TransferLifecycleEvent,
};

// =============================================================================
// TransferTelemetry — per-transfer sequence tracking + event emission
// =============================================================================

/// Helper that tracks per-transfer sequence numbers and emits structured
/// lifecycle events into the shared [`Diagnostics`] store.
///
/// All public methods accept the raw transfer id (`i64`) and any
/// event-specific payload fields.  Non-sensitive metadata only —
/// no filenames, paths, hashes, or peer identifiers.
#[derive(Debug)]
pub struct TransferTelemetry {
    /// The shared diagnostics store.
    diagnostics: Diagnostics,
    /// Per-transfer sequence counters.  Keyed by the durable download row id.
    sequences: Mutex<HashMap<i64, SequenceState>>,
}

/// Per-transfer sequence and attempt state.
#[derive(Debug, Clone)]
struct SequenceState {
    /// Next sequence number to assign (monotonic within this transfer).
    next_seq: u64,
    /// Current attempt number.  Set externally via [`set_attempt`].
    attempt: u32,
}

impl SequenceState {
    fn new() -> Self {
        Self {
            next_seq: 0,
            attempt: 1,
        }
    }

    fn allocate_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }
}

impl TransferTelemetry {
    /// Create a new telemetry helper wrapping the shared diagnostics store.
    pub fn new(diagnostics: Diagnostics) -> Self {
        Self {
            diagnostics,
            sequences: Mutex::new(HashMap::new()),
        }
    }

    /// Set the attempt number for a transfer before emitting events.
    /// Default is 1.  Call this when a retry starts.
    pub fn set_attempt(&self, transfer_id: i64, attempt: u32) {
        let mut seqs = self.sequences.lock().expect("sequences lock");
        let state = seqs.entry(transfer_id).or_insert_with(SequenceState::new);
        state.attempt = attempt;
    }

    /// Remove all tracking state for a completed or cancelled transfer.
    pub fn forget(&self, transfer_id: i64) {
        self.sequences
            .lock()
            .expect("sequences lock")
            .remove(&transfer_id);
    }

    // ── Per-transfer attempt ────────────────────────────────────────────

    /// Return the current attempt number for this transfer.
    fn attempt(&self, transfer_id: i64) -> u32 {
        let seqs = self.sequences.lock().expect("sequences lock");
        seqs.get(&transfer_id).map(|s| s.attempt).unwrap_or(1)
    }

    /// Allocate the next sequence number for this transfer.
    fn allocate_seq(&self, transfer_id: i64) -> u64 {
        let mut seqs = self.sequences.lock().expect("sequences lock");
        let state = seqs.entry(transfer_id).or_insert_with(SequenceState::new);
        state.allocate_seq()
    }

    /// Generate an opaque event id from (transfer_id, sequence, timestamp).
    fn generate_event_id(transfer_id: &str, sequence: u64, now_ms: u64) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(transfer_id.as_bytes());
        hasher.update(sequence.to_le_bytes());
        hasher.update(now_ms.to_le_bytes());
        let hash = hasher.finalize();
        // First 16 hex chars — unique within the retention period and
        // short enough for log readability.
        hex::encode(&hash[..8])
    }

    /// Millisecond timestamp helper.
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    // ── Core emission method ────────────────────────────────────────────

    /// Build and record a [`TransferLifecycleEvent`] into the diagnostics store.
    fn emit(&self, transfer_id: i64, event_name: &'static str, payload: Option<serde_json::Value>) {
        let tid_short = short_transfer_id(transfer_id);
        let now_ms = Self::now_ms();
        let sequence = self.allocate_seq(transfer_id);
        let attempt = self.attempt(transfer_id);
        let event_id = Self::generate_event_id(&tid_short, sequence, now_ms);

        let event = TransferLifecycleEvent {
            schema_version: 1,
            event_id,
            event_name: event_name.to_string(),
            transfer_id: tid_short.clone(),
            sequence,
            occurred_at_ms: now_ms,
            attempt,
            payload,
        };

        self.diagnostics
            .record(None, DiagnosticEventKind::TransferLifecycle(event));
    }

    // ── Per-event emission methods ──────────────────────────────────────

    /// The download was durably queued.
    pub fn download_queued(&self, transfer_id: i64, total_bytes: u64, queue_depth: Option<u32>) {
        let mut payload = serde_json::json!({
            "total_bytes": total_bytes,
        });
        if let Some(depth) = queue_depth {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("queue_depth".to_string(), serde_json::json!(depth)));
        }
        self.emit(transfer_id, event_names::DOWNLOAD_QUEUED, Some(payload));
    }

    /// An access/permission request was sent to the remote peer.
    pub fn access_requested(&self, transfer_id: i64, request_kind: &str) {
        let payload = serde_json::json!({
            "request_kind": request_kind,
        });
        self.emit(transfer_id, event_names::ACCESS_REQUESTED, Some(payload));
    }

    /// The access response authorised the transfer.
    pub fn access_granted(&self, transfer_id: i64, grant_ttl_ms: Option<u64>) {
        let payload = grant_ttl_ms.map(|ttl| serde_json::json!({ "grant_ttl_ms": ttl }));
        self.emit(transfer_id, event_names::ACCESS_GRANTED, payload);
    }

    /// Byte transfer began for the current attempt.
    pub fn transfer_started(&self, transfer_id: i64, total_bytes: u64, resumed_bytes: Option<u64>) {
        let mut payload = serde_json::json!({
            "total_bytes": total_bytes,
        });
        if let Some(rb) = resumed_bytes {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("resumed_bytes".to_string(), serde_json::json!(rb)));
        }
        self.emit(transfer_id, event_names::TRANSFER_STARTED, Some(payload));
    }

    /// A sampled cumulative progress checkpoint.
    pub fn progress_checkpoint(
        &self,
        transfer_id: i64,
        bytes_transferred: u64,
        total_bytes: u64,
        bytes_delta: Option<u64>,
        checkpoint_interval_ms: Option<u64>,
        rate_bytes_per_sec: Option<u64>,
    ) {
        let percent_millis = if total_bytes > 0 {
            (bytes_transferred as u128)
                .saturating_mul(1_000_000)
                .checked_div(total_bytes as u128)
                .map(|v| v as u64)
                .unwrap_or(0)
        } else {
            0
        };
        let mut payload = serde_json::json!({
            "bytes_transferred": bytes_transferred,
            "total_bytes": total_bytes,
            "percent_millis": percent_millis,
        });
        if let Some(d) = bytes_delta {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("bytes_delta".to_string(), serde_json::json!(d)));
        }
        if let Some(i) = checkpoint_interval_ms {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("checkpoint_interval_ms".to_string(), serde_json::json!(i)));
        }
        if let Some(r) = rate_bytes_per_sec {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("rate_bytes_per_sec".to_string(), serde_json::json!(r)));
        }
        self.emit(transfer_id, event_names::PROGRESS_CHECKPOINT, Some(payload));
    }

    /// Work was deliberately suspended.
    pub fn pause(&self, transfer_id: i64, reason: &str, bytes_transferred: Option<u64>) {
        let mut payload = serde_json::json!({
            "reason": reason,
        });
        if let Some(bt) = bytes_transferred {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("bytes_transferred".to_string(), serde_json::json!(bt)));
        }
        self.emit(transfer_id, event_names::PAUSE, Some(payload));
    }

    /// A paused logical transfer was resumed (not a retry).
    pub fn resume(&self, transfer_id: i64, reason: &str, bytes_transferred: Option<u64>) {
        let mut payload = serde_json::json!({
            "reason": reason,
        });
        if let Some(bt) = bytes_transferred {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("bytes_transferred".to_string(), serde_json::json!(bt)));
        }
        self.emit(transfer_id, event_names::RESUME, Some(payload));
    }

    /// Local size/integrity verification finished.
    pub fn verification(
        &self,
        transfer_id: i64,
        result: &str,
        bytes_transferred: Option<u64>,
        total_bytes: Option<u64>,
    ) {
        let mut payload = serde_json::json!({
            "result": result,
        });
        if let Some(bt) = bytes_transferred {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("bytes_transferred".to_string(), serde_json::json!(bt)));
        }
        if let Some(tb) = total_bytes {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("total_bytes".to_string(), serde_json::json!(tb)));
        }
        self.emit(transfer_id, event_names::VERIFICATION, Some(payload));
    }

    /// Verified content was installed and the download reached its successful
    /// terminal state.
    pub fn completion(&self, transfer_id: i64, bytes_transferred: u64, duration_ms: Option<u64>) {
        let mut payload = serde_json::json!({
            "bytes_transferred": bytes_transferred,
        });
        if let Some(d) = duration_ms {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("duration_ms".to_string(), serde_json::json!(d)));
        }
        self.emit(transfer_id, event_names::COMPLETION, Some(payload));
        self.forget(transfer_id);
    }

    /// The current attempt failed.
    pub fn failure(
        &self,
        transfer_id: i64,
        error_category: ErrorCategory,
        retryable: bool,
        bytes_transferred: Option<u64>,
        will_retry: Option<bool>,
        retry_delay_ms: Option<u64>,
    ) {
        let mut payload = serde_json::json!({
            "error_category": error_category.as_str(),
            "retryable": retryable,
        });
        if let Some(bt) = bytes_transferred {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("bytes_transferred".to_string(), serde_json::json!(bt)));
        }
        if let Some(wr) = will_retry {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("will_retry".to_string(), serde_json::json!(wr)));
        }
        if let Some(rd) = retry_delay_ms {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("retry_delay_ms".to_string(), serde_json::json!(rd)));
        }
        self.emit(transfer_id, event_names::FAILURE, Some(payload));
    }

    /// The transfer was cancelled and reached its terminal cancelled state.
    pub fn cancellation(&self, transfer_id: i64, reason: &str, bytes_transferred: Option<u64>) {
        let mut payload = serde_json::json!({
            "reason": reason,
        });
        if let Some(bt) = bytes_transferred {
            payload
                .as_object_mut()
                .map(|obj| obj.insert("bytes_transferred".to_string(), serde_json::json!(bt)));
        }
        self.emit(transfer_id, event_names::CANCELLATION, Some(payload));
        self.forget(transfer_id);
    }
}

// =============================================================================
// Helpers for classifying errors into categories
// =============================================================================

/// Map common error messages to their error category.
pub fn classify_permission_error(error_msg: &str) -> ErrorCategory {
    let lower = error_msg.to_lowercase();
    if lower.contains("permission denied") || lower.contains("denied") {
        ErrorCategory::PermissionDenied
    } else if lower.contains("not found") {
        ErrorCategory::NotFound
    } else if lower.contains("busy") || lower.contains("unavailable") {
        ErrorCategory::PeerUnavailable
    } else if lower.contains("rate limit") || lower.contains("rate_limited") {
        ErrorCategory::RateLimited
    } else if lower.contains("version mismatch") || lower.contains("changed") {
        ErrorCategory::VersionMismatch
    } else if lower.contains("expired") || lower.contains("nonce") {
        ErrorCategory::PermissionDenied
    } else if lower.contains("timeout") {
        ErrorCategory::Timeout
    } else {
        ErrorCategory::ProtocolError
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Diagnostics;

    fn new_telemetry() -> TransferTelemetry {
        let diagnostics = Diagnostics::new();
        TransferTelemetry::new(diagnostics)
    }

    #[test]
    fn emits_download_queued_with_optional_queue_depth() {
        let t = new_telemetry();
        t.download_queued(42, 1024, Some(3));
    }

    #[test]
    fn emits_access_requested() {
        let t = new_telemetry();
        t.access_requested(42, "initial");
    }

    #[test]
    fn emits_access_granted_with_ttl() {
        let t = new_telemetry();
        t.access_granted(42, Some(30_000));
    }

    #[test]
    fn emits_transfer_started_with_resumed() {
        let t = new_telemetry();
        t.transfer_started(42, 2048, Some(512));
    }

    #[test]
    fn emits_progress_checkpoint() {
        let t = new_telemetry();
        t.progress_checkpoint(42, 1024, 2048, Some(512), Some(250), Some(2_000_000));
    }

    #[test]
    fn progress_percent_millis_correct() {
        let t = new_telemetry();
        t.progress_checkpoint(42, 500, 1000, None, None, None);
    }

    #[test]
    fn progress_zero_total_does_not_div_by_zero() {
        let t = new_telemetry();
        t.progress_checkpoint(42, 0, 0, None, None, None);
    }

    #[test]
    fn emits_pause_with_reason() {
        let t = new_telemetry();
        t.pause(42, "user", Some(1024));
    }

    #[test]
    fn emits_resume_with_reason() {
        let t = new_telemetry();
        t.resume(42, "user", Some(1024));
    }

    #[test]
    fn emits_verification_passed() {
        let t = new_telemetry();
        t.verification(42, "passed", Some(2048), Some(2048));
    }

    #[test]
    fn emits_verification_failed() {
        let t = new_telemetry();
        t.verification(42, "failed", Some(512), Some(2048));
    }

    #[test]
    fn emits_completion_with_duration() {
        let t = new_telemetry();
        t.completion(42, 2048, Some(1_234));
    }

    #[test]
    fn emits_failure_with_category() {
        let t = new_telemetry();
        t.failure(
            42,
            ErrorCategory::PeerUnavailable,
            true,
            Some(0),
            Some(true),
            Some(5_000),
        );
    }

    #[test]
    fn emits_cancellation() {
        let t = new_telemetry();
        t.cancellation(42, "user", None);
    }

    #[test]
    fn sequences_are_monotonic_per_transfer() {
        let diag = Diagnostics::new();
        let t = TransferTelemetry::new(diag.clone());
        t.download_queued(1, 100, None);
        t.access_requested(1, "initial");
        t.access_granted(1, None);
        t.transfer_started(1, 100, None);

        // events_since(0) skips the first global sequence (seq=0) because
        // the filter is `sequence > since_sequence`.
        let events = diag.events_since(0, 100, None);
        let lifecycle_events: Vec<&TransferLifecycleEvent> = events
            .iter()
            .filter_map(|e| match &e.kind {
                DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
                _ => None,
            })
            .collect();

        // 4 events emitted, events_since(0) returns 3 (seq 1, 2, 3).
        assert_eq!(lifecycle_events.len(), 3);
        // Per-transfer sequences within transfer 1 are monotonically increasing.
        assert_eq!(lifecycle_events[0].sequence, 1);
        assert_eq!(lifecycle_events[1].sequence, 2);
        assert_eq!(lifecycle_events[2].sequence, 3);
    }

    #[test]
    fn independent_transfers_have_independent_sequences() {
        let diag = Diagnostics::new();
        let t = TransferTelemetry::new(diag.clone());
        t.download_queued(1, 100, None); // transfer 1: seq 0
        t.download_queued(2, 200, None); // transfer 2: seq 0 (independent)
        t.access_requested(1, "initial"); // transfer 1: seq 1
        t.access_requested(2, "initial"); // transfer 2: seq 1 (independent)

        // events_since(0) skips the first global event (seq=0) because
        // the filter is `sequence > since_sequence`. We get 3 events
        // (global seq 1: t2:seq=0, seq 2: t1:seq=1, seq 3: t2:seq=1).
        let events = diag.events_since(0, 100, None);
        let lifecycle_events: Vec<&TransferLifecycleEvent> = events
            .iter()
            .filter_map(|e| match &e.kind {
                DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
                _ => None,
            })
            .collect();

        assert_eq!(lifecycle_events.len(), 3);
        // Events: t2:seq=0, t1:seq=1, t2:seq=1
        assert_eq!(lifecycle_events[0].sequence, 0);
        assert_eq!(lifecycle_events[0].transfer_id, "2");
        assert_eq!(lifecycle_events[1].sequence, 1);
        assert_eq!(lifecycle_events[1].transfer_id, "1");
        assert_eq!(lifecycle_events[2].sequence, 1);
        assert_eq!(lifecycle_events[2].transfer_id, "2");
    }

    #[test]
    fn forget_removes_state() {
        let t = new_telemetry();
        t.download_queued(99, 100, None);
        t.forget(99);
        t.download_queued(99, 100, None); // should work without panic
    }

    #[test]
    fn terminal_events_call_forget() {
        let diag = Diagnostics::new();
        let t = TransferTelemetry::new(diag.clone());

        t.completion(10, 100, None); // completion: seq 0, then forget
                                     // After forget, a new event for the same transfer starts at seq 0.
        t.download_queued(10, 100, None); // seq 0 again (new lifecycle)

        // events_since(0) skips the first global event (seq=0), so we only
        // see the second event (global seq=1: download_queued seq=0).
        let events = diag.events_since(0, 100, None);
        let lifecycle_events: Vec<&TransferLifecycleEvent> = events
            .iter()
            .filter_map(|e| match &e.kind {
                DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
                _ => None,
            })
            .collect();

        // Only the second event is visible; both have per-transfer seq 0 but
        // belong to independent lifecycles.
        assert_eq!(lifecycle_events.len(), 1);
        assert_eq!(lifecycle_events[0].sequence, 0);
        assert_eq!(lifecycle_events[0].event_name, "download_queued");
    }

    #[test]
    fn error_category_as_str() {
        assert_eq!(
            ErrorCategory::PermissionDenied.as_str(),
            "permission_denied"
        );
        assert_eq!(ErrorCategory::PeerUnavailable.as_str(), "peer_unavailable");
        assert_eq!(ErrorCategory::Unknown.as_str(), "unknown");
    }

    #[test]
    fn emits_resume_with_user_reason() {
        let t = new_telemetry();
        t.resume(42, "user", Some(2048));
    }

    #[test]
    fn emits_resume_with_restart_recovery_reason() {
        let t = new_telemetry();
        t.resume(42, "restart_recovery", None);
    }

    #[test]
    fn emits_failure_with_all_fields() {
        let t = new_telemetry();
        t.failure(
            42,
            ErrorCategory::PermissionDenied,
            false,
            Some(512),
            Some(false),
            Some(0),
        );
    }

    #[test]
    fn emits_failure_retryable_with_will_retry() {
        let t = new_telemetry();
        t.failure(
            42,
            ErrorCategory::Timeout,
            true,
            None,
            Some(true),
            Some(5_000),
        );
    }

    #[test]
    fn emits_completion_calls_forget() {
        let diag = Diagnostics::new();
        let t = TransferTelemetry::new(diag.clone());
        t.completion(77, 4096, Some(2_000));

        // After forget, a new download_queued for the same id should
        // start at sequence 0 — no panic or stale state.
        t.download_queued(77, 100, None);

        let events = diag.events_since(0, 100, None);
        let lifecycle_events: Vec<&TransferLifecycleEvent> = events
            .iter()
            .filter_map(|e| match &e.kind {
                DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
                _ => None,
            })
            .collect();
        // events_since(0) skips the first global event, so we see the
        // download_queued event after the completion+forget cycle.
        assert_eq!(lifecycle_events.len(), 1);
        assert_eq!(lifecycle_events[0].event_name, "download_queued");
    }

    #[test]
    fn emits_cancellation_calls_forget() {
        let diag = Diagnostics::new();
        let t = TransferTelemetry::new(diag.clone());
        t.cancellation(88, "user", None);

        // After forget, a new event should work cleanly.
        t.download_queued(88, 100, None);

        let events = diag.events_since(0, 100, None);
        let lifecycle_events: Vec<&TransferLifecycleEvent> = events
            .iter()
            .filter_map(|e| match &e.kind {
                DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
                _ => None,
            })
            .collect();
        assert_eq!(lifecycle_events.len(), 1);
        assert_eq!(lifecycle_events[0].event_name, "download_queued");
    }

    #[test]
    fn emit_progress_checkpoint() {
        let t = new_telemetry();
        t.progress_checkpoint(42, 500, 1000, Some(500), Some(250), Some(2_000_000));
    }

    #[test]
    fn verify_resume_payload_contract() {
        let diag = Diagnostics::new();
        let t = TransferTelemetry::new(diag.clone());

        // Emit two events so events_since(0) has at least one visible.
        t.download_queued(1, 100, None);
        t.resume(1, "restart_recovery", Some(1024));

        let events = diag.events_since(0, 100, None);
        let ev: Vec<&TransferLifecycleEvent> = events
            .iter()
            .filter_map(|e| match &e.kind {
                DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
                _ => None,
            })
            .collect();
        // events_since(0) skips the first event (global seq=0).
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].event_name, "resume");
        assert_eq!(ev[0].schema_version, 1);
        assert_eq!(ev[0].attempt, 1);
        let payload = ev[0].payload.as_ref().unwrap();
        assert_eq!(payload["reason"], "restart_recovery");
        assert_eq!(payload["bytes_transferred"], 1024);
    }

    #[test]
    fn verify_failure_payload_contract() {
        let diag = Diagnostics::new();
        let t = TransferTelemetry::new(diag.clone());

        // Emit two events so events_since(0) has at least one visible.
        t.download_queued(1, 100, None);
        t.failure(
            1,
            ErrorCategory::PeerUnavailable,
            true,
            Some(0),
            Some(true),
            Some(5_000),
        );

        let events = diag.events_since(0, 100, None);
        let ev: Vec<&TransferLifecycleEvent> = events
            .iter()
            .filter_map(|e| match &e.kind {
                DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
                _ => None,
            })
            .collect();
        // events_since(0) skips the first event.
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].event_name, "failure");
        let payload = ev[0].payload.as_ref().unwrap();
        assert_eq!(payload["error_category"], "peer_unavailable");
        assert_eq!(payload["retryable"], true);
        assert_eq!(payload["bytes_transferred"], 0);
        assert_eq!(payload["will_retry"], true);
        assert_eq!(payload["retry_delay_ms"], 5_000);
    }

    #[test]
    fn verify_two_transfers_have_different_short_ids() {
        let diag = Diagnostics::new();
        let t = TransferTelemetry::new(diag.clone());

        t.download_queued(1, 100, None);
        t.download_queued(999999999, 200, None);

        let events = diag.events_since(0, 100, None);
        let lifecycle_events: Vec<&TransferLifecycleEvent> = events
            .iter()
            .filter_map(|e| match &e.kind {
                DiagnosticEventKind::TransferLifecycle(ev) => Some(ev),
                _ => None,
            })
            .collect();
        // events_since(0) skips first global event.
        assert_eq!(lifecycle_events.len(), 1);
        // The second event (first visible) has a truncated id: 8 chars + 3-byte ellipsis.
        assert!(lifecycle_events[0].transfer_id.len() <= 11);
    }
}
