//! Client-side companion reconnection state and recovery policy.
//!
//! The client never reports `Connected` merely because a transport exists:
//! authentication and catch-up must both complete.  This module is transport
//! independent so the Iroh client, UI, and tests share one set of recovery
//! rules.

#![allow(missing_docs)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The externally visible companion connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionConnectionState {
    /// No durable host registration has been established.
    Unpaired,
    /// A pairing claim exists but local approval has not completed.
    PendingApproval,
    /// A transport connection or handshake is being established.
    Connecting,
    /// Authentication succeeded and the client is applying a complete snapshot
    /// or a bounded change stream.
    Syncing,
    /// Authentication and catch-up both completed for the current registration.
    Connected,
    /// The connection was lost and a bounded retry is scheduled.
    OfflineRetrying,
    /// The durable registration was revoked; automatic retries are disabled.
    Revoked,
    /// The peer cannot speak a mutually supported protocol version.
    Incompatible,
}

impl CompanionConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unpaired => "unpaired",
            Self::PendingApproval => "pending_approval",
            Self::Connecting => "connecting",
            Self::Syncing => "syncing",
            Self::Connected => "connected",
            Self::OfflineRetrying => "offline_retrying",
            Self::Revoked => "revoked",
            Self::Incompatible => "incompatible",
        }
    }

    pub fn retries_stopped(self) -> bool {
        matches!(self, Self::Unpaired | Self::Revoked | Self::Incompatible)
    }
}

/// Bounded exponential retry policy with caller-supplied jitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub initial: Duration,
    pub maximum: Duration,
    pub jitter_max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            maximum: Duration::from_secs(60),
            jitter_max: Duration::from_millis(500),
        }
    }
}

impl RetryPolicy {
    fn delay(self, failures: u32, jitter: Duration) -> Duration {
        let shift = failures.saturating_sub(1).min(30);
        let exponential = self
            .initial
            .checked_mul(1u32 << shift)
            .unwrap_or(self.maximum)
            .min(self.maximum);
        exponential
            .saturating_add(jitter.min(self.jitter_max))
            .min(self.maximum)
    }
}

/// A staged snapshot. It is not visible to callers until committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedSnapshot {
    pub id: String,
    pub records: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingOperation {
    registration: String,
    canonical_message_id: Option<String>,
}

/// Small, deterministic client recovery coordinator.
#[derive(Debug, Clone)]
pub struct CompanionReconnection {
    state: CompanionConnectionState,
    policy: RetryPolicy,
    failures: u32,
    next_retry_at: Option<Instant>,
    registration: Option<String>,
    authenticated: bool,
    caught_up: bool,
    sync_data_applied: bool,
    last_successful_sync: Option<Instant>,
    cache_generation: u64,
    cache: Vec<String>,
    staged_snapshot: Option<StagedSnapshot>,
    drafts: HashMap<String, String>,
    operations: HashMap<String, PendingOperation>,
}

impl CompanionReconnection {
    pub fn new(policy: RetryPolicy) -> Self {
        Self {
            state: CompanionConnectionState::Unpaired,
            policy: RetryPolicy {
                initial: policy.initial,
                maximum: policy.maximum.max(policy.initial),
                jitter_max: policy.jitter_max,
            },
            failures: 0,
            next_retry_at: None,
            registration: None,
            authenticated: false,
            caught_up: false,
            sync_data_applied: false,
            last_successful_sync: None,
            cache_generation: 0,
            cache: Vec::new(),
            staged_snapshot: None,
            drafts: HashMap::new(),
            operations: HashMap::new(),
        }
    }

    pub fn state(&self) -> CompanionConnectionState {
        self.state
    }
    pub fn failures(&self) -> u32 {
        self.failures
    }
    pub fn next_retry_at(&self) -> Option<Instant> {
        self.next_retry_at
    }
    pub fn last_successful_sync(&self) -> Option<Instant> {
        self.last_successful_sync
    }
    pub fn cache_generation(&self) -> u64 {
        self.cache_generation
    }
    pub fn cache(&self) -> &[String] {
        &self.cache
    }
    pub fn drafts(&self) -> &HashMap<String, String> {
        &self.drafts
    }

    /// Establish or restore a durable registration. A new registration cannot
    /// inherit operations from an old one.
    pub fn set_registration(&mut self, registration: impl Into<String>) {
        let registration = registration.into();
        if self.registration.as_deref() != Some(registration.as_str()) {
            self.operations.clear();
            self.staged_snapshot = None;
            self.caught_up = false;
            self.sync_data_applied = false;
        }
        self.registration = Some(registration);
        if self.state == CompanionConnectionState::Unpaired {
            self.state = CompanionConnectionState::OfflineRetrying;
        }
    }

    pub fn mark_pending_approval(&mut self) {
        self.state = CompanionConnectionState::PendingApproval;
        self.next_retry_at = None;
    }

    pub fn begin_connecting(&mut self, now: Instant) -> bool {
        if self.registration.is_none() || self.state.retries_stopped() {
            return false;
        }
        self.state = CompanionConnectionState::Connecting;
        self.next_retry_at = None;
        self.authenticated = false;
        self.caught_up = false;
        self.sync_data_applied = false;
        let _ = now;
        true
    }

    /// A protocol mismatch is terminal until the client is upgraded.
    pub fn protocol_incompatible(&mut self) {
        self.state = CompanionConnectionState::Incompatible;
        self.next_retry_at = None;
        self.authenticated = false;
        self.caught_up = false;
    }

    pub fn permanent_denial(&mut self) {
        self.protocol_incompatible();
    }

    pub fn revoked(&mut self) {
        self.state = CompanionConnectionState::Revoked;
        self.next_retry_at = None;
        self.authenticated = false;
        self.caught_up = false;
        self.operations.clear();
        self.staged_snapshot = None;
    }

    /// Authentication alone is not readiness; it advances only to syncing.
    pub fn authenticated(&mut self) -> bool {
        if !matches!(
            self.state,
            CompanionConnectionState::Connecting | CompanionConnectionState::OfflineRetrying
        ) {
            return false;
        }
        self.authenticated = true;
        self.state = CompanionConnectionState::Syncing;
        self.caught_up = false;
        self.sync_data_applied = false;
        true
    }

    pub fn stage_snapshot(&mut self, id: impl Into<String>, records: Vec<String>) -> bool {
        if !self.authenticated || self.state != CompanionConnectionState::Syncing {
            return false;
        }
        self.staged_snapshot = Some(StagedSnapshot {
            id: id.into(),
            records,
        });
        true
    }

    /// Atomically replaces the visible cache only after the full snapshot is
    /// complete. Drafts are intentionally stored separately and untouched.
    pub fn complete_snapshot(&mut self, id: &str) -> bool {
        let Some(snapshot) = self.staged_snapshot.take() else {
            return false;
        };
        if snapshot.id != id {
            self.staged_snapshot = Some(snapshot);
            return false;
        }
        self.cache = snapshot.records;
        self.cache_generation = self.cache_generation.saturating_add(1);
        self.sync_data_applied = true;
        true
    }

    /// Mark an incremental change stream as fully applied for this reconnect.
    pub fn complete_changes(&mut self) -> bool {
        if self.authenticated && self.state == CompanionConnectionState::Syncing {
            self.sync_data_applied = true;
            true
        } else {
            false
        }
    }

    /// Catch-up completion is the only transition into Connected.
    pub fn complete_catch_up(&mut self, now: Instant) -> bool {
        if self.authenticated
            && self.state == CompanionConnectionState::Syncing
            && self.sync_data_applied
        {
            self.caught_up = true;
            self.state = CompanionConnectionState::Connected;
            self.failures = 0;
            self.next_retry_at = None;
            self.last_successful_sync = Some(now);
            true
        } else {
            false
        }
    }

    pub fn connection_lost(&mut self, now: Instant, jitter: Duration) {
        if self.state.retries_stopped() {
            return;
        }
        self.authenticated = false;
        self.caught_up = false;
        self.sync_data_applied = false;
        self.failures = self.failures.saturating_add(1);
        self.state = CompanionConnectionState::OfflineRetrying;
        self.next_retry_at = Some(now + self.policy.delay(self.failures, jitter));
    }

    /// Resume and network changes bypass backoff, but never bypass terminal
    /// revocation/incompatibility/absence of a registration.
    pub fn retry_immediately(&mut self, now: Instant) -> bool {
        if self.registration.is_none() || self.state.retries_stopped() {
            return false;
        }
        self.state = CompanionConnectionState::OfflineRetrying;
        self.next_retry_at = Some(now);
        true
    }

    pub fn retry_due(&mut self, now: Instant) -> bool {
        if self.state == CompanionConnectionState::OfflineRetrying
            && self.next_retry_at.is_some_and(|at| at <= now)
        {
            return self.begin_connecting(now);
        }
        false
    }

    pub fn save_draft(&mut self, id: impl Into<String>, text: impl Into<String>) {
        self.drafts.insert(id.into(), text.into());
    }

    /// Register an operation under its original caller ID. It is retried only
    /// with the same registration and is resolved by canonical message ID.
    pub fn register_operation(&mut self, original_id: impl Into<String>) -> bool {
        let Some(registration) = self.registration.clone() else {
            return false;
        };
        let original_id = original_id.into();
        if self.operations.contains_key(&original_id) {
            return false;
        }
        self.operations.insert(
            original_id,
            PendingOperation {
                registration,
                canonical_message_id: None,
            },
        );
        true
    }

    pub fn resolve_operation(
        &mut self,
        original_id: &str,
        canonical_message_id: impl Into<String>,
    ) -> bool {
        let canonical_message_id = canonical_message_id.into();
        let Some(operation) = self.operations.get_mut(original_id) else {
            return false;
        };
        if self.registration.as_deref() != Some(operation.registration.as_str()) {
            return false;
        }
        if let Some(existing) = operation.canonical_message_id.as_deref() {
            return existing == canonical_message_id;
        }
        operation.canonical_message_id = Some(canonical_message_id);
        true
    }

    pub fn canonical_message_id(&self, original_id: &str) -> Option<&str> {
        self.operations
            .get(original_id)?
            .canonical_message_id
            .as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> CompanionReconnection {
        CompanionReconnection::new(RetryPolicy {
            initial: Duration::from_secs(2),
            maximum: Duration::from_secs(10),
            jitter_max: Duration::from_secs(1),
        })
    }

    #[test]
    fn connected_requires_authentication_and_complete_catch_up() {
        let t = Instant::now();
        let mut c = client();
        c.set_registration("r1");
        assert!(c.begin_connecting(t));
        assert_eq!(c.state(), CompanionConnectionState::Connecting);
        assert!(!c.complete_catch_up(t));
        assert!(c.authenticated());
        assert_eq!(c.state(), CompanionConnectionState::Syncing);
        assert!(!c.complete_catch_up(t));
        assert!(c.stage_snapshot("s1", vec!["a".into()]));
        assert!(c.complete_snapshot("s1"));
        assert!(c.complete_catch_up(t));
        assert_eq!(c.state(), CompanionConnectionState::Connected);
        assert_eq!(c.last_successful_sync(), Some(t));
    }

    #[test]
    fn backoff_is_bounded_and_network_change_is_immediate() {
        let t = Instant::now();
        let mut c = client();
        c.set_registration("r1");
        c.connection_lost(t, Duration::from_secs(1));
        assert_eq!(c.next_retry_at(), Some(t + Duration::from_secs(3)));
        c.connection_lost(t + Duration::from_secs(3), Duration::from_secs(1));
        assert_eq!(c.next_retry_at(), Some(t + Duration::from_secs(8)));
        c.retry_immediately(t + Duration::from_secs(4));
        assert_eq!(c.next_retry_at(), Some(t + Duration::from_secs(4)));
        assert!(c.retry_due(t + Duration::from_secs(4)));
        c.connection_lost(t + Duration::from_secs(4), Duration::from_secs(1));
        for _ in 0..10 {
            c.connection_lost(t + Duration::from_secs(20), Duration::from_secs(1));
        }
        assert!(c.next_retry_at().unwrap() <= t + Duration::from_secs(31));
    }

    #[test]
    fn terminal_states_stop_retries_and_new_registration_drops_old_operations() {
        let t = Instant::now();
        let mut c = client();
        c.set_registration("old");
        assert!(c.register_operation("op-1"));
        c.resolve_operation("op-1", "msg-1");
        c.revoked();
        assert!(!c.retry_immediately(t));
        c.set_registration("new");
        assert_eq!(c.canonical_message_id("op-1"), None);
        assert!(c.register_operation("op-2"));
        assert!(c.resolve_operation("op-2", "msg-2"));
        assert!(c.resolve_operation("op-2", "msg-2"));
        assert!(!c.resolve_operation("op-2", "different"));
        c.protocol_incompatible();
        assert!(!c.retry_immediately(t));
    }

    #[test]
    fn incomplete_snapshot_does_not_replace_cache_or_drafts() {
        let t = Instant::now();
        let mut c = client();
        c.set_registration("r1");
        c.save_draft("draft", "keep me");
        c.begin_connecting(t);
        c.authenticated();
        c.stage_snapshot("s1", vec!["new".into()]);
        assert!(!c.complete_snapshot("wrong"));
        assert!(c.cache().is_empty());
        assert_eq!(c.drafts().get("draft").map(String::as_str), Some("keep me"));
        assert!(c.complete_snapshot("s1"));
        assert_eq!(c.cache(), ["new"]);
    }
}
