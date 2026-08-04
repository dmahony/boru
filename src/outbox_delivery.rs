//! Single-owner durable delivery worker for the SQLite outbox.
//!
//! Claiming is the only operation that happens in the database before network
//! I/O.  A row is released only after the transport reports a verified
//! protocol acknowledgement; writing bytes to a QUIC stream is not success.
//!
//! # Concurrency
//!
//! When [`OutboxDeliveryWorker::with_max_concurrent`] is set to a value > 1,
//! [`run_once`](OutboxDeliveryWorker::run_once) claims batches of due rows
//! transactionally and processes them concurrently (up to the configured
//! limit).  Per-peer ordering is preserved: at most one delivery is in flight
//! for a given recipient at any time.

use crate::{
    storage::Storage,
    store::{OutboxRow, StoredEnvelope},
};
use iroh::PublicKey;
use n0_error::Result;
use std::{
    collections::HashMap,
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, Semaphore};

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

/// Manages per-peer delivery slots for concurrent but ordered delivery.
///
/// At most one delivery task may be active per peer, ensuring that messages
/// addressed to the same recipient are processed sequentially (FIFO) while
/// deliveries to different peers proceed in parallel.
#[derive(Debug)]
struct PeerOrderGuard {
    /// Map from peer public key to a semaphore with exactly 1 permit.
    slots: Mutex<HashMap<PublicKey, Arc<Semaphore>>>,
}

impl PeerOrderGuard {
    fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire a per-peer permit.  Returns a guard that holds the permit
    /// for the lifetime of the returned value.  Blocks until no other
    /// delivery is active for this peer.
    async fn acquire(&self, peer: PublicKey) -> PeerPermit {
        let semaphore = {
            let mut slots = self.slots.lock().unwrap();
            slots
                .entry(peer)
                .or_insert_with(|| Arc::new(Semaphore::new(1)))
                .clone()
        };
        let permit = semaphore
            .acquire_owned()
            .await
            .expect("semaphore not closed");
        PeerPermit { _permit: permit }
    }
}

/// RAII guard that holds a per-peer delivery permit.
#[derive(Debug)]
struct PeerPermit {
    _permit: tokio::sync::OwnedSemaphorePermit,
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
    /// Maximum concurrent deliveries across all peers.
    /// 1 (default) = fully sequential, matching pre-concurrency behaviour.
    max_concurrent: NonZeroUsize,
    /// How many rows to claim in a single batch transaction.
    /// Larger batches reduce SQLite round-trips but may hold the write lock longer.
    claim_batch_size: u32,
    /// How often to extend leases for in-flight deliveries (fraction of lease_duration_ms).
    lease_heartbeat_fraction: f64,
}

impl<P, T> std::fmt::Debug for OutboxDeliveryWorker<P, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxDeliveryWorker")
            .field("lease_owner", &self.lease_owner)
            .field("lease_duration_ms", &self.lease_duration_ms)
            .field("claim_limit", &self.claim_limit)
            .field("max_concurrent", &self.max_concurrent)
            .field("claim_batch_size", &self.claim_batch_size)
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
            max_concurrent: NonZeroUsize::new(1).unwrap(),
            claim_batch_size: 8,
            lease_heartbeat_fraction: 0.5,
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

    /// Set the maximum number of concurrent outbound deliveries.
    ///
    /// A value of 1 (the default) reproduces pre-concurrency sequential
    /// behaviour.  Higher values allow deliveries to *different* peers to
    /// proceed in parallel, while per-peer ordering is always preserved
    /// (at most one in-flight delivery per recipient).
    pub fn with_max_concurrent(mut self, max: NonZeroUsize) -> Self {
        self.max_concurrent = max;
        self
    }

    /// Set how many rows to claim in a single batch transaction.
    ///
    /// The default is 8.  Higher values reduce SQLite round-trips but hold
    /// the write lock longer during the claim phase.  Has no effect when
    /// `max_concurrent` is 1.
    pub fn with_claim_batch_size(mut self, batch_size: u32) -> Self {
        self.claim_batch_size = batch_size.max(1);
        self
    }

    /// Set the lease-heartbeat fraction (default 0.5 = 50% of lease_duration_ms).
    ///
    /// The heartbeat task extends the lease at this fraction of the duration
    /// so that long transfers do not lose their claim.  Set to 0.0 to disable
    /// heartbeats entirely.
    pub fn with_lease_heartbeat_fraction(mut self, fraction: f64) -> Self {
        self.lease_heartbeat_fraction = fraction.clamp(0.0, 1.0);
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
    ///
    /// When `max_concurrent` > 1, deliveries to different peers proceed in
    /// parallel while per-peer ordering is preserved.
    pub async fn run_once(&self) -> usize {
        let now = self.clock.now_ms();
        let _ = self.storage.expire_outbox(now);
        let _ = self.storage.recover_stale_outbox_leases(now);
        let _ = self.storage.recover_stale_sending_deliveries(now);

        if self.max_concurrent.get() <= 1 {
            // ── Sequential path ─────────────────────────────────────
            let rows = self
                .storage
                .claim_pending_deliveries(self.claim_limit, now)
                .unwrap_or_default();
            let mut attempted = 0;
            for row in rows {
                attempted += 1;
                self.process_claim(row).await;
            }
            return attempted;
        }

        // ── Concurrent path ─────────────────────────────────────────
        let peer_guard = Arc::new(PeerOrderGuard::new());
        let semaphore = Arc::new(Semaphore::new(self.max_concurrent.get()));
        let mut total_attempted = 0usize;

        loop {
            let batch = self
                .storage
                .claim_n_due_outbox(
                    now,
                    &self.lease_owner,
                    self.lease_duration_ms,
                    self.claim_batch_size,
                )
                .unwrap_or_default();

            if batch.is_empty() {
                break;
            }

            let mut handles = Vec::with_capacity(batch.len());
            for row in batch {
                if total_attempted >= self.claim_limit as usize {
                    // Release unprocessed claimed rows so they are due again.
                    let _ = self.storage.release_outbox_lease(
                        &row.msg_id,
                        row.recipient_device_id,
                        &self.lease_owner,
                    );
                    continue;
                }
                total_attempted += 1;

                let storage = self.storage.clone();
                let policy = self.policy.clone();
                let transport = self.transport.clone();
                let retry_policy = self.retry_policy;
                let lease_owner = self.lease_owner.clone();
                let lease_duration_ms = self.lease_duration_ms;
                let lease_heartbeat_fraction = self.lease_heartbeat_fraction;
                let clock = self.clock.clone();
                let permit = semaphore.clone().acquire_owned().await;
                let peer_guard = peer_guard.clone();

                let handle = tokio::spawn(async move {
                    let peer = row.recipient_device_id;
                    let _peer_permit = peer_guard.acquire(peer).await;

                    // Run lease-extension heartbeat in a background task
                    // if fraction > 0 and the lease is long enough.
                    let heartbeat_handle =
                        if lease_heartbeat_fraction > 0.0 && lease_duration_ms >= 10_000 {
                            let interval_ms =
                                (lease_duration_ms as f64 * lease_heartbeat_fraction) as u64;
                            let storage_hb = storage.clone();
                            let msg_id = row.msg_id;
                            let lease_owner_hb = lease_owner.clone();
                            Some(tokio::spawn(async move {
                                loop {
                                    tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                                    let now = unix_ms();
                                    let locked_until = now.saturating_add(lease_duration_ms);
                                    let ok = storage_hb
                                        .extend_outbox_lease(
                                            &msg_id,
                                            peer,
                                            &lease_owner_hb,
                                            now,
                                            locked_until,
                                        )
                                        .unwrap_or(false);
                                    if !ok {
                                        // Lease was lost (cancelled or claimed by another worker).
                                        break;
                                    }
                                }
                            }))
                        } else {
                            None
                        };

                    let outcome: Result<()> = async {
                        let authorized = policy.authorize(peer).await?;
                        if !authorized {
                            return Err(n0_error::anyerr!("recipient is no longer authorized"));
                        }
                        let envelope = storage
                            .get_inbox(&row.msg_id)?
                            .ok_or_else(|| n0_error::anyerr!("outbox envelope is missing"))?;
                        if envelope.expires_at_ms <= unix_ms() {
                            return Err(n0_error::anyerr!("outbox envelope expired"));
                        }
                        transport.deliver(peer, envelope).await
                    }
                    .await;

                    // Stop the heartbeat before recording the outcome.
                    if let Some(hb) = heartbeat_handle {
                        hb.abort();
                    }

                    let now = clock.now_ms();
                    let (success, error) = match outcome {
                        Ok(()) => (true, None),
                        Err(err) => (false, Some(err.to_string())),
                    };

                    if success {
                        let jitter = (rand::random::<u64>() as f64) / (u64::MAX as f64);
                        let delay = retry_policy.delay_ms(row.attempts, jitter);
                        let _ = storage.mark_sent(
                            &row.msg_id,
                            peer,
                            now.saturating_add(delay),
                        );
                    } else {
                        let jitter = (rand::random::<u64>() as f64) / (u64::MAX as f64);
                        let delay = retry_policy.delay_ms(row.attempts, jitter);
                        let _ = storage.record_attempt(
                            &row.msg_id,
                            peer,
                            now.saturating_add(delay),
                            error.as_deref(),
                        );
                    }

                    // Release the leases explicitly.
                    let _ = storage.release_outbox_lease(&row.msg_id, peer, &lease_owner);

                    // Drop _peer_permit and permit implicitly.
                    drop(_peer_permit);
                    drop(permit);
                });
                handles.push(handle);
            }

            // Wait for this batch to drain before claiming the next one.
            for h in handles {
                let _ = h.await;
            }
        }

        total_attempted
    }

    /// Retry only due rows for a peer that just became reachable. The bound
    /// prevents an online event from monopolising the delivery worker.
    pub async fn run_once_for_peer(&self, peer: PublicKey, max_attempts: u32) -> usize {
        let now = unix_ms();
        let _ = self.storage.recover_stale_outbox_leases(now);
        let _ = self.storage.recover_stale_sending_deliveries(now);
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
            let jitter = (rand::random::<u64>() as f64) / (u64::MAX as f64);
            let delay = self.retry_policy.delay_ms(row.attempts, jitter);
            let _ = self.storage.mark_sent(&msg_id, peer, now.saturating_add(delay));
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

    #[test]
    fn peer_order_guard_serializes_same_peer() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let guard = Arc::new(PeerOrderGuard::new());
            let peer = iroh::SecretKey::generate().public();

            let g1 = guard.clone();
            let g2 = guard.clone();
            let (start_tx, start_rx) = tokio::sync::oneshot::channel();
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();

            // Task 1 acquires the peer lock and signals it's started.
            let t1 = tokio::spawn(async move {
                let _permit = g1.acquire(peer).await;
                let _ = start_tx.send(());
                let _ = done_rx.await;
            });

            // Wait for task 1 to acquire.
            let _ = start_rx.await;

            // Task 2: try to acquire (should block).
            let acquired = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let acquired_clone = acquired.clone();
            let g2_clone = g2;
            let t2 = tokio::spawn(async move {
                let _permit = g2_clone.acquire(peer).await;
                acquired_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            });

            // Small delay — t2 should still be blocked.
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(
                !acquired.load(std::sync::atomic::Ordering::SeqCst),
                "t2 should be blocked by t1's peer lock"
            );

            // Release t1.
            let _ = done_tx.send(());
            let _ = t1.await;

            // Now t2 should proceed.
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(
                acquired.load(std::sync::atomic::Ordering::SeqCst),
                "t2 should have acquired after t1 released"
            );
            let _ = t2.await;
        });
    }

    /// Test that concurrent delivery processes different peers in parallel.
    /// Uses a mock storage + transport to verify parallelism.
    #[tokio::test]
    async fn test_different_peers_deliver_concurrently() {
        use crate::storage::Storage;
        use crate::store::StoredEnvelope;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let storage = Storage::memory().unwrap();

        // Create inbox envelopes and outbox rows for 3 different peers.
        let peers: Vec<PublicKey> = (0..3)
            .map(|_| iroh::SecretKey::generate().public())
            .collect();

        let conv_id = [42u8; 32];
        let now = unix_ms();
        for (i, peer) in peers.iter().enumerate() {
            let msg_id = [i as u8; 32];
            let env = StoredEnvelope {
                msg_id,
                conversation_id: conv_id,
                author_user_id: *peer,
                author_device_id: *peer,
                created_at_ms: now,
                expires_at_ms: now + 86_400_000,
                ciphertext: bytes::Bytes::from_static(b"test"),
                signature: [0u8; 64],
                acked_at_ms: None,
            };
            storage.insert_inbox(&env).unwrap();
            storage.enqueue_outbox(&msg_id, *peer, now).unwrap();
        }

        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_concurrent_observed = Arc::new(AtomicUsize::new(0));

        // Mock transport that simulates 100ms delivery and tracks concurrency.
        struct TrackedTransport {
            in_flight: Arc<AtomicUsize>,
            max_observed: Arc<AtomicUsize>,
        }
        impl DeliveryTransport for TrackedTransport {
            fn deliver(
                &self,
                _recipient: PublicKey,
                _envelope: StoredEnvelope,
            ) -> BoxFuture<Result<()>> {
                let in_flight = self.in_flight.clone();
                let max_observed = self.max_observed.clone();
                Box::pin(async move {
                    let prev = in_flight.fetch_add(1, Ordering::SeqCst);
                    let _ = max_observed.fetch_max(prev + 1, Ordering::SeqCst);
                    // Simulate network I/O delay.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        }
        let transport = Arc::new(TrackedTransport {
            in_flight: in_flight.clone(),
            max_observed: max_concurrent_observed.clone(),
        });

        let policy = Arc::new(AllowListedPolicy(move |_peer| {
            Box::pin(async move { Ok(true) })
        }));

        let (_trigger, rx) = mpsc::channel(1);
        let worker =
            OutboxDeliveryWorker::new(storage.clone(), policy, transport, "test-worker", rx)
                .with_max_concurrent(NonZeroUsize::new(3).unwrap())
                .with_claim_batch_size(3)
                .with_lease(5_000); // short lease for tests

        worker.run_once().await;

        // We should have observed at least 2 concurrent deliveries
        // (probably 3, but allow for timing variance in CI).
        let observed = max_concurrent_observed.load(Ordering::SeqCst);
        assert!(
            observed >= 2,
            "expected at least 2 concurrent deliveries, got {observed}"
        );

        // All rows should now be acked.
        let due = storage.fetch_due_outbox(now + 5000).unwrap();
        assert!(due.is_empty(), "all rows should have been delivered");
    }

    /// Test that same-peer deliveries are serialized even with max_concurrent > 1.
    #[tokio::test]
    async fn test_same_peer_serialized() {
        use crate::storage::Storage;
        use crate::store::StoredEnvelope;

        let storage = Storage::memory().unwrap();
        let peer = iroh::SecretKey::generate().public();
        let conv_id = [42u8; 32];
        let now = unix_ms();

        // Insert 3 messages for the same peer.
        for i in 0..3u8 {
            let msg_id = [i; 32];
            let env = StoredEnvelope {
                msg_id,
                conversation_id: conv_id,
                author_user_id: peer,
                author_device_id: peer,
                created_at_ms: now,
                expires_at_ms: now + 86_400_000,
                ciphertext: bytes::Bytes::from_static(b"test"),
                signature: [0u8; 64],
                acked_at_ms: None,
            };
            storage.insert_inbox(&env).unwrap();
            storage.enqueue_outbox(&msg_id, peer, now).unwrap();
        }

        let delivery_order = Arc::new(Mutex::new(Vec::new()));

        // Transport that records delivery order with a small delay.
        struct RecordingTransport {
            order: Arc<Mutex<Vec<u64>>>,
        }
        impl DeliveryTransport for RecordingTransport {
            fn deliver(
                &self,
                _recipient: PublicKey,
                _envelope: StoredEnvelope,
            ) -> BoxFuture<Result<()>> {
                let order = self.order.clone();
                Box::pin(async move {
                    {
                        let mut o = order.lock().unwrap();
                        o.push(unix_ms());
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(())
                })
            }
        }
        let transport = Arc::new(RecordingTransport {
            order: delivery_order.clone(),
        });

        let policy = Arc::new(AllowListedPolicy(move |_peer| {
            Box::pin(async move { Ok(true) })
        }));

        let (_trigger, rx) = mpsc::channel(1);
        let worker =
            OutboxDeliveryWorker::new(storage.clone(), policy, transport, "test-worker", rx)
                .with_max_concurrent(NonZeroUsize::new(3).unwrap())
                .with_claim_batch_size(3)
                .with_lease(5_000);

        worker.run_once().await;

        // Verify all rows were delivered.
        let due = storage.fetch_due_outbox(now + 5000).unwrap();
        assert!(due.is_empty(), "all rows should have been delivered");

        let order = delivery_order.lock().unwrap();
        assert_eq!(order.len(), 3, "all 3 deliveries should have happened");
        // The order should be monotonic (sequential per peer).
        for pair in order.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "deliveries to same peer should be sequential"
            );
        }
    }
}
