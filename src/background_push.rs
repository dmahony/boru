//! Privacy-preserving boundary for optional background push delivery.
//!
//! This module is a broker boundary, not an APNs/FCM implementation.  The core
//! deliberately exposes no callback URL and no provider credentials.  A future
//! mobile/infrastructure adapter may implement [`NotificationSink`] behind this
//! boundary; until then [`DisabledNotificationSink`] honestly reports
//! `background_push=unavailable`.  Connected activity alerts remain separate
//! and continue to use `crate::activity_alerts`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Maximum number of hints retained by the recording broker.
pub const MAX_PENDING_HINTS: usize = 64;
/// Hints older than this are not delivered.
pub const HINT_TTL_SECS: u64 = 300;
/// Failed deliveries are retried at most this many times.
pub const MAX_DELIVERY_ATTEMPTS: u8 = 3;

/// User-visible capability wording.  Do not call this feature "enabled" until
/// a real provider is configured by a platform/infrastructure integration.
pub const BACKGROUND_PUSH_UNAVAILABLE: &str = "background_push=unavailable";

/// The only payload kinds permitted across the background boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericHintKind {
    /// One or more new messages are available.
    NewActivity,
}

/// A deliberately generic hint.  It contains no body, sender, room, filename,
/// preview, or URL.  The foreground app must fetch and authorize details later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericPushHint {
    /// Generic event category.
    pub kind: GenericHintKind,
    /// Number of events represented, without identifying details.
    pub count: u32,
    /// Absolute expiry time in Unix seconds.
    pub expires_at_secs: u64,
}

impl GenericPushHint {
    /// Construct a bounded-lifetime generic hint.
    pub fn new(kind: GenericHintKind, count: u32, now_secs: u64) -> Self {
        Self {
            kind,
            count: count.max(1),
            expires_at_secs: now_secs.saturating_add(HINT_TTL_SECS),
        }
    }

    fn is_expired(self, now_secs: u64) -> bool {
        now_secs >= self.expires_at_secs
    }
}

/// An opaque registration bound to one authorization grant.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[must_use]
pub struct RegistrationHandle {
    nonce: u128,
    grant: [u8; 32],
}

impl std::fmt::Debug for RegistrationHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistrationHandle").finish_non_exhaustive()
    }
}

/// A grant identifier supplied by the authorization layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PushGrant(pub [u8; 32]);

/// Result of attempting to enqueue a hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    /// The hint entered the bounded broker queue.
    Queued,
    /// No provider is configured.
    RejectedUnavailable,
    /// The registration was revoked or unknown.
    RejectedRevoked,
    /// The bounded queue has no capacity.
    RejectedFull,
    /// The hint was already expired.
    RejectedExpired,
}

/// Capability state exposed to UI and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundPushCapability {
    /// No platform provider is configured.
    Unavailable,
    /// Test-only in-memory recording is available.
    RecordingOnly,
}

/// A brokered hint ready for a provider adapter.  The handle is retained so a
/// revoked grant can never be sent after the item was claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushDelivery {
    /// Stable broker-local identifier for retry accounting.
    pub delivery_id: u64,
    /// Grant-bound registration that authorized this delivery.
    pub registration: RegistrationHandle,
    /// Privacy-preserving generic payload.
    pub hint: GenericPushHint,
    /// One-based attempt number.
    pub attempts: u8,
}

/// Boundary implemented by disabled, recording, and future provider brokers.
pub trait NotificationSink: Send + Sync {
    /// Report whether this broker represents a real provider.
    fn capability(&self) -> BackgroundPushCapability;
    /// Register one opaque handle for an authorization grant.
    fn register(&self, grant: PushGrant) -> Result<RegistrationHandle, EnqueueResult>;
    /// Revoke a handle and cancel all queued work for it.
    fn revoke(&self, registration: RegistrationHandle) -> bool;
    /// Queue a generic hint if the registration and bounds permit it.
    fn enqueue(
        &self,
        registration: RegistrationHandle,
        hint: GenericPushHint,
        now_secs: u64,
    ) -> EnqueueResult;
    /// Claim the next non-expired, non-revoked delivery.
    fn next(&self, now_secs: u64) -> Option<PushDelivery>;
    /// Requeue a failed delivery while retry and expiry limits permit.
    fn retry(&self, delivery: PushDelivery, now_secs: u64) -> bool;
}

/// No-provider adapter.  It never pretends that background push is available.
#[derive(Debug, Default, Clone, Copy)]
pub struct DisabledNotificationSink;

impl NotificationSink for DisabledNotificationSink {
    fn capability(&self) -> BackgroundPushCapability {
        BackgroundPushCapability::Unavailable
    }
    fn register(&self, _grant: PushGrant) -> Result<RegistrationHandle, EnqueueResult> {
        Err(EnqueueResult::RejectedUnavailable)
    }
    fn revoke(&self, _registration: RegistrationHandle) -> bool {
        false
    }
    fn enqueue(
        &self,
        _registration: RegistrationHandle,
        _hint: GenericPushHint,
        _now_secs: u64,
    ) -> EnqueueResult {
        EnqueueResult::RejectedUnavailable
    }
    fn next(&self, _now_secs: u64) -> Option<PushDelivery> {
        None
    }
    fn retry(&self, _delivery: PushDelivery, _now_secs: u64) -> bool {
        false
    }
}

#[derive(Debug)]
struct Pending {
    delivery_id: u64,
    registration: RegistrationHandle,
    hint: GenericPushHint,
    attempts: u8,
}

#[derive(Debug, Default)]
struct RecordingState {
    next_nonce: u128,
    next_delivery_id: u64,
    registrations: HashMap<RegistrationHandle, PushGrant>,
    queue: VecDeque<Pending>,
}

/// In-memory broker for tests and diagnostics.  It is intentionally not a
/// production provider: it records generic hints and exercises revocation,
/// expiry, queue, and retry policy without sending anything externally.
#[derive(Debug, Clone, Default)]
pub struct RecordingNotificationSink {
    state: Arc<Mutex<RecordingState>>,
}

impl RecordingNotificationSink {
    /// Return the number of currently queued hints.
    pub fn pending_len(&self) -> usize {
        self.state.lock().expect("push state lock").queue.len()
    }
}

impl NotificationSink for RecordingNotificationSink {
    fn capability(&self) -> BackgroundPushCapability {
        BackgroundPushCapability::RecordingOnly
    }

    fn register(&self, grant: PushGrant) -> Result<RegistrationHandle, EnqueueResult> {
        let mut state = self.state.lock().expect("push state lock");
        state.next_nonce = state.next_nonce.saturating_add(1);
        let handle = RegistrationHandle {
            nonce: state.next_nonce,
            grant: grant.0,
        };
        state.registrations.insert(handle, grant);
        Ok(handle)
    }

    fn revoke(&self, registration: RegistrationHandle) -> bool {
        let mut state = self.state.lock().expect("push state lock");
        let removed = state.registrations.remove(&registration).is_some();
        state.queue.retain(|item| item.registration != registration);
        removed
    }

    fn enqueue(
        &self,
        registration: RegistrationHandle,
        hint: GenericPushHint,
        now_secs: u64,
    ) -> EnqueueResult {
        let mut state = self.state.lock().expect("push state lock");
        if !state.registrations.contains_key(&registration) {
            return EnqueueResult::RejectedRevoked;
        }
        if hint.is_expired(now_secs) {
            return EnqueueResult::RejectedExpired;
        }
        if state.queue.len() >= MAX_PENDING_HINTS {
            return EnqueueResult::RejectedFull;
        }
        state.next_delivery_id = state.next_delivery_id.saturating_add(1);
        let delivery_id = state.next_delivery_id;
        state.queue.push_back(Pending {
            delivery_id,
            registration,
            hint,
            attempts: 0,
        });
        EnqueueResult::Queued
    }

    fn next(&self, now_secs: u64) -> Option<PushDelivery> {
        let mut state = self.state.lock().expect("push state lock");
        while let Some(item) = state.queue.pop_front() {
            if item.hint.is_expired(now_secs)
                || !state.registrations.contains_key(&item.registration)
            {
                continue;
            }
            return Some(PushDelivery {
                delivery_id: item.delivery_id,
                registration: item.registration,
                hint: item.hint,
                attempts: item.attempts.saturating_add(1),
            });
        }
        None
    }

    fn retry(&self, delivery: PushDelivery, now_secs: u64) -> bool {
        let mut state = self.state.lock().expect("push state lock");
        if delivery.attempts >= MAX_DELIVERY_ATTEMPTS
            || delivery.hint.is_expired(now_secs)
            || !state.registrations.contains_key(&delivery.registration)
            || state.queue.len() >= MAX_PENDING_HINTS
        {
            return false;
        }
        state.queue.push_back(Pending {
            delivery_id: delivery.delivery_id,
            registration: delivery.registration,
            hint: delivery.hint,
            attempts: delivery.attempts,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> PushGrant {
        PushGrant([7; 32])
    }

    #[test]
    fn no_provider_is_honest() {
        let sink = DisabledNotificationSink;
        assert_eq!(sink.capability(), BackgroundPushCapability::Unavailable);
        assert_eq!(
            sink.register(grant()),
            Err(EnqueueResult::RejectedUnavailable)
        );
    }

    #[test]
    fn recording_payload_has_no_sensitive_fields() {
        let sink = RecordingNotificationSink::default();
        let handle = sink.register(grant()).unwrap();
        assert_eq!(
            sink.enqueue(
                handle,
                GenericPushHint::new(GenericHintKind::NewActivity, 2, 10),
                10
            ),
            EnqueueResult::Queued
        );
        let delivery = sink.next(10).unwrap();
        assert_eq!(delivery.hint.count, 2);
        assert_eq!(delivery.hint.kind, GenericHintKind::NewActivity);
    }

    #[test]
    fn revocation_cancels_queued_hints_and_retries() {
        let sink = RecordingNotificationSink::default();
        let handle = sink.register(grant()).unwrap();
        sink.enqueue(
            handle,
            GenericPushHint::new(GenericHintKind::NewActivity, 1, 10),
            10,
        );
        let delivery = sink.next(10).unwrap();
        assert!(sink.revoke(handle));
        assert!(!sink.retry(delivery, 10));
        assert!(sink.next(10).is_none());
    }

    #[test]
    fn expired_hints_are_not_delivered() {
        let sink = RecordingNotificationSink::default();
        let handle = sink.register(grant()).unwrap();
        let hint = GenericPushHint {
            kind: GenericHintKind::NewActivity,
            count: 1,
            expires_at_secs: 20,
        };
        assert_eq!(sink.enqueue(handle, hint, 19), EnqueueResult::Queued);
        assert!(sink.next(20).is_none());
    }
}
