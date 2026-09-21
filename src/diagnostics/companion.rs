//! Bounded, privacy-preserving diagnostics for desktop/companion links.
//!
//! This module intentionally stores counters and coarse states rather than
//! request bodies, paths, endpoint addresses, invitation material, or keys.
//! A support export is therefore useful for distinguishing a permission,
//! version, storage, or connectivity failure without becoming a data export.
#![allow(missing_docs)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Stable coarse categories used by companion diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompanionFailureCategory {
    Permission,
    Version,
    Storage,
    Connectivity,
    Timeout,
    Protocol,
    Resource,
    Unknown,
}

/// Evidence-backed state of the local companion host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompanionHostState {
    Disabled,
    Starting,
    Ready,
    PairingPending,
    Connected,
    StorageUnavailable,
    ProtocolIncompatible,
}

/// Evidence-backed state of the remote client's reachability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompanionClientReachability {
    Unknown,
    Reachable,
    Unreachable,
}

impl Default for CompanionClientReachability {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Pairing result bucket. The invitation, device name, and comparison code
/// are never retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingOutcome {
    Started,
    Pending,
    Approved,
    Rejected,
    Invalid,
    Expired,
    Disabled,
    StorageFailure,
}

/// Bounded snapshot suitable for MCP/support export.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionDiagnosticSnapshot {
    pub host_state: Option<CompanionHostState>,
    pub client_reachability: CompanionClientReachability,
    pub sessions_started: u64,
    pub sessions_closed: u64,
    pub denied_requests: u64,
    pub pairing_started: u64,
    pub pairing_pending: u64,
    pub pairing_approved: u64,
    pub pairing_rejected: u64,
    pub pairing_invalid: u64,
    pub pairing_expired: u64,
    pub pairing_disabled: u64,
    pub pairing_storage_failures: u64,
    pub sync_lag_samples: u64,
    pub sync_lag_max_ms: u64,
    pub resyncs: u64,
    pub dedup_hits: u64,
    pub notification_queue_failures: u64,
    pub last_failure_category: Option<CompanionFailureCategory>,
    pub last_correlation_id: Option<String>,
}

#[derive(Debug, Default)]
struct CompanionDiagnosticsInner {
    host_state: Mutex<Option<CompanionHostState>>,
    client_reachability: Mutex<CompanionClientReachability>,
    sessions_started: AtomicU64,
    sessions_closed: AtomicU64,
    denied_requests: AtomicU64,
    pairing_started: AtomicU64,
    pairing_pending: AtomicU64,
    pairing_approved: AtomicU64,
    pairing_rejected: AtomicU64,
    pairing_invalid: AtomicU64,
    pairing_expired: AtomicU64,
    pairing_disabled: AtomicU64,
    pairing_storage_failures: AtomicU64,
    sync_lag_samples: AtomicU64,
    sync_lag_max_ms: AtomicU64,
    resyncs: AtomicU64,
    dedup_hits: AtomicU64,
    notification_queue_failures: AtomicU64,
    last_failure_category: Mutex<Option<CompanionFailureCategory>>,
    last_correlation_id: Mutex<Option<String>>,
}

/// Thread-safe bounded companion diagnostics. Clones share one counter set.
#[derive(Debug, Clone, Default)]
pub struct CompanionDiagnostics {
    inner: Arc<CompanionDiagnosticsInner>,
}

impl CompanionDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_host_state(&self, state: CompanionHostState) {
        *self.inner.host_state.lock().expect("host state lock") = Some(state);
    }

    pub fn set_client_reachability(&self, state: CompanionClientReachability) {
        *self
            .inner
            .client_reachability
            .lock()
            .expect("reachability lock") = state;
    }

    pub fn record_session_started(&self) {
        self.inner.sessions_started.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_session_closed(&self) {
        self.inner.sessions_closed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a denied operation and retain only a stable category and opaque
    /// correlation identifier derived from the caller's local token.
    pub fn record_denied(&self, category: CompanionFailureCategory, correlation: &str) {
        self.inner.denied_requests.fetch_add(1, Ordering::Relaxed);
        self.record_failure(category, correlation);
    }

    pub fn record_pairing(&self, outcome: PairingOutcome, correlation: &str) {
        let counter = match outcome {
            PairingOutcome::Started => &self.inner.pairing_started,
            PairingOutcome::Pending => &self.inner.pairing_pending,
            PairingOutcome::Approved => &self.inner.pairing_approved,
            PairingOutcome::Rejected => &self.inner.pairing_rejected,
            PairingOutcome::Invalid => &self.inner.pairing_invalid,
            PairingOutcome::Expired => &self.inner.pairing_expired,
            PairingOutcome::Disabled => &self.inner.pairing_disabled,
            PairingOutcome::StorageFailure => &self.inner.pairing_storage_failures,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        if matches!(
            outcome,
            PairingOutcome::Rejected
                | PairingOutcome::Invalid
                | PairingOutcome::Expired
                | PairingOutcome::Disabled
        ) {
            self.record_failure(CompanionFailureCategory::Permission, correlation);
        }
        if outcome == PairingOutcome::StorageFailure {
            self.record_failure(CompanionFailureCategory::Storage, correlation);
        }
    }

    pub fn record_sync_lag(&self, lag_ms: u64) {
        self.inner.sync_lag_samples.fetch_add(1, Ordering::Relaxed);
        let mut current = self.inner.sync_lag_max_ms.load(Ordering::Relaxed);
        while lag_ms > current {
            match self.inner.sync_lag_max_ms.compare_exchange_weak(
                current,
                lag_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
    }

    pub fn record_resync(&self) {
        self.inner.resyncs.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_dedup_hit(&self) {
        self.inner.dedup_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_notification_queue_failure(&self, correlation: &str) {
        self.inner
            .notification_queue_failures
            .fetch_add(1, Ordering::Relaxed);
        self.record_failure(CompanionFailureCategory::Resource, correlation);
    }

    /// Remote timeout means the client is unreachable; it is not evidence that
    /// the desktop is asleep.
    pub fn record_remote_timeout(&self, correlation: &str) {
        self.set_client_reachability(CompanionClientReachability::Unreachable);
        self.record_failure(CompanionFailureCategory::Timeout, correlation);
    }

    fn record_failure(&self, category: CompanionFailureCategory, correlation: &str) {
        *self
            .inner
            .last_failure_category
            .lock()
            .expect("failure lock") = Some(category);
        *self
            .inner
            .last_correlation_id
            .lock()
            .expect("correlation lock") = Some(safe_correlation_id(correlation));
    }

    pub fn snapshot(&self) -> CompanionDiagnosticSnapshot {
        CompanionDiagnosticSnapshot {
            host_state: *self.inner.host_state.lock().expect("host state lock"),
            client_reachability: *self
                .inner
                .client_reachability
                .lock()
                .expect("reachability lock"),
            sessions_started: self.inner.sessions_started.load(Ordering::Relaxed),
            sessions_closed: self.inner.sessions_closed.load(Ordering::Relaxed),
            denied_requests: self.inner.denied_requests.load(Ordering::Relaxed),
            pairing_started: self.inner.pairing_started.load(Ordering::Relaxed),
            pairing_pending: self.inner.pairing_pending.load(Ordering::Relaxed),
            pairing_approved: self.inner.pairing_approved.load(Ordering::Relaxed),
            pairing_rejected: self.inner.pairing_rejected.load(Ordering::Relaxed),
            pairing_invalid: self.inner.pairing_invalid.load(Ordering::Relaxed),
            pairing_expired: self.inner.pairing_expired.load(Ordering::Relaxed),
            pairing_disabled: self.inner.pairing_disabled.load(Ordering::Relaxed),
            pairing_storage_failures: self.inner.pairing_storage_failures.load(Ordering::Relaxed),
            sync_lag_samples: self.inner.sync_lag_samples.load(Ordering::Relaxed),
            sync_lag_max_ms: self.inner.sync_lag_max_ms.load(Ordering::Relaxed),
            resyncs: self.inner.resyncs.load(Ordering::Relaxed),
            dedup_hits: self.inner.dedup_hits.load(Ordering::Relaxed),
            notification_queue_failures: self
                .inner
                .notification_queue_failures
                .load(Ordering::Relaxed),
            last_failure_category: *self
                .inner
                .last_failure_category
                .lock()
                .expect("failure lock"),
            last_correlation_id: self
                .inner
                .last_correlation_id
                .lock()
                .expect("correlation lock")
                .clone(),
        }
    }

    /// Export only bounded, serialized diagnostics. No caller-supplied text is
    /// included, so QR secrets, keys, tokens, bodies, paths, and frames cannot leak.
    pub fn export_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(&self.snapshot())
    }

    pub fn recovery_guidance(category: CompanionFailureCategory) -> &'static str {
        match category {
            CompanionFailureCategory::Permission => {
                "Approve this device on the desktop, then retry."
            }
            CompanionFailureCategory::Version => {
                "Update both devices to compatible Boru versions."
            }
            CompanionFailureCategory::Storage => {
                "Check the desktop database and disk, then restart Boru."
            }
            CompanionFailureCategory::Connectivity | CompanionFailureCategory::Timeout => {
                "Check the network and retry; Desktop unreachable means the remote client did not respond, not that it is asleep."
            }
            CompanionFailureCategory::Protocol => {
                "Restart both clients and update them if the error persists."
            }
            CompanionFailureCategory::Resource => {
                "Wait briefly and retry; inspect queue limits if it continues."
            }
            CompanionFailureCategory::Unknown => {
                "Retry once, then export these redacted diagnostics for support."
            }
        }
    }
}

fn safe_correlation_id(value: &str) -> String {
    hex::encode(&blake3::hash(value.as_bytes()).as_bytes()[..6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_is_bounded_and_redacts_canaries() {
        let diagnostics = CompanionDiagnostics::new();
        diagnostics.set_host_state(CompanionHostState::Connected);
        diagnostics.record_denied(
            CompanionFailureCategory::Permission,
            "QR_SECRET canary /home/user/body",
        );
        diagnostics.record_remote_timeout("push-token canary");
        diagnostics.record_pairing(PairingOutcome::Approved, "private-key canary");
        let exported = diagnostics.export_json().unwrap();
        assert!(!exported.contains("QR_SECRET"));
        assert!(!exported.contains("/home/user"));
        assert!(!exported.contains("push-token"));
        assert!(!exported.contains("private-key"));
        assert!(exported.contains("unreachable"));
        assert_eq!(diagnostics.snapshot().denied_requests, 1);
    }

    #[test]
    fn counters_and_guidance_distinguish_failures() {
        let d = CompanionDiagnostics::new();
        d.record_pairing(PairingOutcome::Rejected, "a");
        d.record_pairing(PairingOutcome::StorageFailure, "b");
        d.record_sync_lag(10);
        d.record_sync_lag(30);
        d.record_resync();
        d.record_dedup_hit();
        d.record_notification_queue_failure("c");
        let s = d.snapshot();
        assert_eq!(s.pairing_rejected, 1);
        assert_eq!(s.pairing_storage_failures, 1);
        assert_eq!(s.sync_lag_max_ms, 30);
        assert_eq!(s.resyncs, 1);
        assert_eq!(s.dedup_hits, 1);
        assert_eq!(
            CompanionDiagnostics::recovery_guidance(CompanionFailureCategory::Version),
            "Update both devices to compatible Boru versions."
        );
    }
}
