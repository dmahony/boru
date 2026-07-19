//! Single-owner durable delivery worker for the SQLite outbox.
//!
//! Claiming is the only operation that happens in the database before network
//! I/O.  A row is released only after the transport reports a verified
//! protocol acknowledgement; writing bytes to a QUIC stream is not success.

use crate::{
    storage::Storage,
    store::{OutboxRow, StoredEnvelope},
};
use iroh::PublicKey;
use n0_error::Result;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

/// Source of a peer-online notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReachabilitySource {
    /// Peer was discovered through multicast DNS.
    Mdns,
    /// Peer address was resolved through a relay.
    Relay,
    /// A friend ping confirmed the peer is online.
    FriendPing,
    /// A direct connection was established.
    DirectConnection,
    /// A known peer was restored during application startup.
    Startup,
}

/// A coalesced peer-online event consumed by the delivery worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerReachable {
    /// The peer whose pending messages should be retried.
    pub peer: PublicKey,
    /// Addresses observed for the peer, retained for endpoint-cache updates.
    pub addresses: Vec<String>,
    /// The subsystem that established reachability.
    pub source: ReachabilitySource,
}

/// Non-blocking, debounced reconnect notification sender.
#[derive(Clone, Debug)]
pub struct ReconnectDeliveryTrigger {
    tx: mpsc::Sender<PeerReachable>,
    state: Arc<Mutex<HashMap<PublicKey, (Instant, PeerReachable)>>>,
    debounce: Duration,
}

impl ReconnectDeliveryTrigger {
    /// Create a trigger and bounded receiver pair.
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<PeerReachable>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (
            Self {
                tx,
                state: Arc::new(Mutex::new(HashMap::new())),
                debounce: Duration::from_secs(2),
            },
            rx,
        )
    }
    /// Configure the duplicate-notification debounce interval.
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }
    /// Submit a peer-online event without blocking the network event loop.
    /// Returns false when it was debounced or the bounded queue is full.
    pub fn notify(&self, event: PeerReachable) -> bool {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        if let Some((last, previous)) = state.get_mut(&event.peer) {
            let changed = previous.addresses != event.addresses || previous.source != event.source;
            *previous = event.clone();
            if !changed && now.duration_since(*last) < self.debounce {
                return false;
            }
            *last = now;
        } else {
            state.insert(event.peer, (now, event.clone()));
        }
        self.tx.try_send(event).is_ok()
    }
    /// Return the latest address/source snapshot for a peer.
    pub fn latest(&self, peer: PublicKey) -> Option<PeerReachable> {
        self.state
            .lock()
            .unwrap()
            .get(&peer)
            .map(|(_, e)| e.clone())
    }
}

/// Retry schedule with exponential growth, bounded jitter, and a hard cap.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Initial delay before the first retry.
    pub initial_delay_ms: u64,
    /// Maximum retry delay.
    pub max_delay_ms: u64,
    /// Maximum positive jitter as a fraction of the base delay.
    pub jitter_fraction: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            initial_delay_ms: 5_000,
            max_delay_ms: 180_000,
            jitter_fraction: 0.5,
        }
    }
}

impl RetryPolicy {
    /// Compute a delay for a zero-based attempt and a deterministic jitter in [0, 1].
    pub fn delay_ms(&self, attempt: u32, jitter: f64) -> u64 {
        let base = self
            .initial_delay_ms
            .saturating_mul(1u64 << attempt.min(31));
        let capped = base.min(self.max_delay_ms);
        let factor = 1.0 + self.jitter_fraction * jitter.clamp(0.0, 1.0);
        ((capped as f64 * factor) as u64).min(self.max_delay_ms)
    }
}

/// Injectable clock for deterministic scheduling tests.
pub trait Clock: Send + Sync {
    /// Return current Unix time in milliseconds.
    fn now_ms(&self) -> u64;
}
/// Production wall clock implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        unix_ms()
    }
}

/// Whether a delivery failure can be retried automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// The condition may clear without changing local state or user intent.
    Transient,
    /// Retrying cannot succeed for this envelope or protocol operation.
    Permanent,
    /// Retrying is meaningful only after the user changes authorization or data.
    RetryableOnlyAfterUserAction,
}

/// Stable, machine-readable reasons for a failed delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryFailure {
    /// Recipient is currently not reachable but may return later.
    PeerOffline,
    /// No usable address was available for the recipient.
    AddressUnavailable,
    /// A connection attempt failed before protocol exchange.
    ConnectionFailed,
    /// Delivery did not complete before its deadline.
    Timeout,
    /// The configured relay could not be reached or used.
    RelayUnavailable,
    /// The remote rejected the envelope or protocol request.
    ProtocolRejected,
    /// Local authorization does not permit delivery.
    Unauthorised,
    /// The recipient cannot currently accept this message state.
    InvalidRecipientState,
    /// The message lifetime elapsed before delivery.
    MessageExpired,
    /// The contact authorization was revoked.
    ContactRevoked,
    /// The envelope exceeds a protocol or policy size limit.
    PayloadTooLarge,
    /// Reading or writing the local durable store failed.
    LocalStorageFailure,
    /// An otherwise-unclassified internal failure occurred.
    InternalError,
}

impl DeliveryFailure {
    /// Stable wire/UI/storage code. Do not change these strings once published.
    pub const fn code(self) -> &'static str {
        match self {
            Self::PeerOffline => "peer_offline",
            Self::AddressUnavailable => "address_unavailable",
            Self::ConnectionFailed => "connection_failed",
            Self::Timeout => "timeout",
            Self::RelayUnavailable => "relay_unavailable",
            Self::ProtocolRejected => "protocol_rejected",
            Self::Unauthorised => "unauthorised",
            Self::InvalidRecipientState => "invalid_recipient_state",
            Self::MessageExpired => "message_expired",
            Self::ContactRevoked => "contact_revoked",
            Self::PayloadTooLarge => "payload_too_large",
            Self::LocalStorageFailure => "local_storage_failure",
            Self::InternalError => "internal_error",
        }
    }

    /// Return the retry policy for this failure.
    pub const fn class(self) -> FailureClass {
        match self {
            Self::PeerOffline
            | Self::AddressUnavailable
            | Self::ConnectionFailed
            | Self::Timeout
            | Self::RelayUnavailable
            | Self::LocalStorageFailure
            | Self::InternalError => FailureClass::Transient,
            Self::ProtocolRejected | Self::MessageExpired | Self::PayloadTooLarge => {
                FailureClass::Permanent
            }
            Self::Unauthorised | Self::InvalidRecipientState | Self::ContactRevoked => {
                FailureClass::RetryableOnlyAfterUserAction
            }
        }
    }
}

impl std::fmt::Display for DeliveryFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// A classified delivery failure with optional bounded diagnostic context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryError {
    /// The stable failure category.
    pub failure: DeliveryFailure,
    detail: Option<String>,
}

impl DeliveryError {
    /// Construct an error without diagnostic detail.
    pub fn new(failure: DeliveryFailure) -> Self {
        Self {
            failure,
            detail: None,
        }
    }

    /// Attach diagnostic text after removing control characters and bounding size.
    /// Callers must not pass secrets; this sanitisation is not secret detection.
    pub fn with_detail(failure: DeliveryFailure, detail: impl AsRef<str>) -> Self {
        let cleaned: String = detail
            .as_ref()
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let detail = cleaned.trim().chars().take(512).collect::<String>();
        Self {
            failure,
            detail: (!detail.is_empty()).then_some(detail),
        }
    }

    /// Return the stable machine-readable code.
    pub fn code(&self) -> &'static str {
        self.failure.code()
    }
    /// Return the retry policy for the underlying failure.
    pub fn class(&self) -> FailureClass {
        self.failure.class()
    }
    /// Return optional sanitized diagnostic detail.
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.detail() {
            Some(detail) => write!(f, "{}: {detail}", self.code()),
            None => f.write_str(self.code()),
        }
    }
}

impl std::error::Error for DeliveryError {}

/// Boxed, sendable future used by the worker extension points.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Resolves the current authorization and recipient addressing policy.
/// Returning an error prevents delivery and schedules a retry.
pub trait RecipientPolicy: Send + Sync {
    /// Check contact authorization and resolve current recipient state.
    fn authorize(&self, recipient: PublicKey) -> BoxFuture<Result<bool>>;
}

/// Sends one stored envelope and returns only after the remote protocol has
/// acknowledged and authenticated the envelope.
pub trait DeliveryTransport: Send + Sync {
    /// Deliver an envelope and await a verified protocol acknowledgement.
    fn deliver(&self, recipient: PublicKey, envelope: StoredEnvelope) -> BoxFuture<Result<()>>;
}

/// Durable, single-owner outbox worker.  Do not create another retry loop for
/// the same `Storage`; all callers should signal this worker through `trigger`.
pub struct OutboxDeliveryWorker<P, T> {
    storage: Storage,
    policy: Arc<P>,
    transport: Arc<T>,
    lease_owner: String,
    lease_duration_ms: u64,
    claim_limit: u32,
    retry_policy: RetryPolicy,
    clock: Arc<dyn Clock>,
    trigger: mpsc::Receiver<()>,
}

impl<P, T> std::fmt::Debug for OutboxDeliveryWorker<P, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxDeliveryWorker")
            .field("lease_owner", &self.lease_owner)
            .field("lease_duration_ms", &self.lease_duration_ms)
            .field("claim_limit", &self.claim_limit)
            .finish_non_exhaustive()
    }
}

impl<P: RecipientPolicy + 'static, T: DeliveryTransport + 'static> OutboxDeliveryWorker<P, T> {
    /// Construct a worker. The caller owns the trigger sender and should use
    /// it to coalesce wakeups after enqueueing an outbox row.
    pub fn new(
        storage: Storage,
        policy: Arc<P>,
        transport: Arc<T>,
        lease_owner: impl Into<String>,
        trigger: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            storage,
            policy,
            transport,
            lease_owner: lease_owner.into(),
            lease_duration_ms: 60_000,
            claim_limit: 32,
            retry_policy: RetryPolicy::default(),
            clock: Arc::new(SystemClock),
            trigger,
        }
    }

    /// Set the lease duration used while network I/O is in progress.
    pub fn with_lease(mut self, duration_ms: u64) -> Self {
        self.lease_duration_ms = duration_ms.max(1_000);
        self
    }
    /// Set the maximum number of rows claimed per pass.
    pub fn with_claim_limit(mut self, limit: u32) -> Self {
        self.claim_limit = limit.max(1);
        self
    }

    /// Replace the production clock with an injectable clock.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Ask the durable store to retry a recipient immediately.
    pub fn retry_now(&self, msg_id: &crate::store::MessageId, peer: PublicKey) -> Result<usize> {
        self.storage
            .retry_outbox_now(msg_id, peer, self.clock.now_ms())
    }

    /// Accelerate all pending messages when a peer is newly discovered.
    pub fn peer_discovered(&self, peer: PublicKey) -> Result<usize> {
        self.storage.wake_outbox_for_peer(peer, self.clock.now_ms())
    }

    /// Process all currently claimable rows. The returned count is the number
    /// of attempts made, not the number of bytes written.
    pub async fn run_once(&self) -> usize {
        let now = self.clock.now_ms();
        let _ = self.storage.expire_outbox(now);
        let rows = self.storage.fetch_due_outbox(now).unwrap_or_default();
        let mut attempted = 0;
        for row in rows.into_iter().take(self.claim_limit as usize) {
            attempted += 1;
            self.process_claim(row).await;
        }
        attempted
    }

    /// Retry only due rows for a peer that just became reachable. The bound
    /// prevents an online event from monopolising the delivery worker.
    pub async fn run_once_for_peer(&self, peer: PublicKey, max_attempts: u32) -> usize {
        let now = unix_ms();
        let _ = self.storage.recover_stale_outbox_leases(now);
        let _ = self.storage.expire_outbox(now);
        let mut attempted = 0;
        while attempted < max_attempts.max(1) {
            let row = match self.storage.claim_due_outbox_for_peer(
                now,
                peer,
                &self.lease_owner,
                self.lease_duration_ms,
            ) {
                Ok(Some(row)) => row,
                Ok(None) | Err(_) => break,
            };
            attempted += 1;
            self.process_claim(row).await;
        }
        attempted as usize
    }

    async fn process_claim(&self, row: OutboxRow) {
        let msg_id = row.msg_id;
        let peer = row.recipient_device_id;
        let outcome: Result<()> = async {
            let authorized = self.policy.authorize(peer).await?;
            if !authorized {
                return Err(n0_error::anyerr!("recipient is no longer authorized"));
            }
            let envelope = self
                .storage
                .get_inbox(&msg_id)?
                .ok_or_else(|| n0_error::anyerr!("outbox envelope is missing"))?;
            if envelope.expires_at_ms <= unix_ms() {
                return Err(n0_error::anyerr!("outbox envelope expired"));
            }
            self.transport.deliver(peer, envelope).await
        }
        .await;
        let now = self.clock.now_ms();
        let (success, error) = match outcome {
            Ok(()) => (true, None),
            Err(err) => (false, Some(err.to_string())),
        };
        if success {
            let _ = self.storage.mark_acked(&msg_id, peer);
        } else {
            let jitter = (rand::random::<u64>() as f64) / (u64::MAX as f64);
            let delay = self.retry_policy.delay_ms(row.attempts, jitter);
            let _ = self.storage.record_attempt(
                &msg_id,
                peer,
                now.saturating_add(delay),
                error.as_deref(),
            );
        }
    }

    /// Run until the trigger channel closes, with a periodic recovery tick.
    pub async fn run(mut self) {
        loop {
            tokio::select! {
                Some(_) = self.trigger.recv() => { self.run_once().await; }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => { self.run_once().await; }
                else => break,
            }
        }
    }

    /// Run normal retries plus bounded retries triggered by peer reachability.
    pub async fn run_with_reconnects(
        mut self,
        mut reconnects: mpsc::Receiver<PeerReachable>,
        max_attempts: u32,
    ) {
        loop {
            tokio::select! {
                Some(event) = reconnects.recv() => {
                    self.run_once_for_peer(event.peer, max_attempts).await;
                }
                Some(_) = self.trigger.recv() => { self.run_once().await; }
                _ = tokio::time::sleep(Duration::from_secs(30)) => { self.run_once().await; }
                else => break,
            }
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Convenience policy for applications whose contact store is already
/// authoritative and whose transport performs address resolution itself.
pub struct AllowListedPolicy<F>(pub F);
impl<F> std::fmt::Debug for AllowListedPolicy<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AllowListedPolicy").finish_non_exhaustive()
    }
}
impl<F, Fut> RecipientPolicy for AllowListedPolicy<F>
where
    F: Fn(PublicKey) -> Fut + Send + Sync,
    Fut: Future<Output = Result<bool>> + Send + 'static,
{
    fn authorize(&self, recipient: PublicKey) -> BoxFuture<Result<bool>> {
        Box::pin((self.0)(recipient))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_is_bounded() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_ms(0, 0.5), 6_250);
        assert_eq!(policy.delay_ms(99, 0.5), 180_000);
    }

    #[test]
    fn backoff_grows_exponentially_and_jitter_is_bounded() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_ms(1, 0.0), 10_000);
        assert_eq!(policy.delay_ms(1, 1.0), 15_000);
        assert_eq!(policy.delay_ms(2, 0.0), 20_000);
        assert_eq!(policy.delay_ms(2, 1.0), 30_000);
        assert!(policy.delay_ms(20, 1.0) <= policy.max_delay_ms);
    }

    #[test]
    fn reconnect_trigger_debounces_duplicate_online_events_and_keeps_latest_addresses() {
        let peer = iroh::SecretKey::generate().public();
        let (trigger, mut rx) = ReconnectDeliveryTrigger::channel(4);
        let first = PeerReachable {
            peer,
            addresses: vec!["127.0.0.1:1".into()],
            source: ReachabilitySource::Mdns,
        };
        assert!(trigger.notify(first.clone()));
        assert!(!trigger.notify(first.clone()));
        assert!(rx.try_recv().is_ok());

        let updated = PeerReachable {
            addresses: vec!["127.0.0.1:2".into()],
            source: ReachabilitySource::DirectConnection,
            ..first
        };
        assert!(trigger.notify(updated.clone()));
        assert_eq!(trigger.latest(peer), Some(updated));
    }
}
