//! Single-owner durable delivery worker for the SQLite outbox.
//!
//! Claiming is the only operation that happens in the database before network
//! I/O.  A row is released only after the transport reports a verified
//! protocol acknowledgement; writing bytes to a QUIC stream is not success.

use crate::{storage::Storage, store::{OutboxRow, StoredEnvelope}};
use iroh::PublicKey;
use n0_error::Result;
use std::{future::Future, pin::Pin, sync::Arc, time::{SystemTime, UNIX_EPOCH}};
use tokio::sync::mpsc;

/// Boxed, sendable future used by the worker extension points.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Resolves the current authorization and recipient addressing policy.
/// Returning an error prevents delivery and schedules a retry.
pub trait RecipientPolicy: Send + Sync {
    /// Check contact authorization and resolve current recipient state.
    fn authorize<'a>(&'a self, recipient: PublicKey) -> BoxFuture<Result<bool>>;
}

/// Sends one stored envelope and returns only after the remote protocol has
/// acknowledged and authenticated the envelope.
pub trait DeliveryTransport: Send + Sync {
    /// Deliver an envelope and await a verified protocol acknowledgement.
    fn deliver<'a>(&'a self, recipient: PublicKey, envelope: StoredEnvelope) -> BoxFuture<Result<()>>;
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
        Self { storage, policy, transport, lease_owner: lease_owner.into(), lease_duration_ms: 60_000, claim_limit: 32, trigger }
    }

    /// Set the lease duration used while network I/O is in progress.
    pub fn with_lease(mut self, duration_ms: u64) -> Self { self.lease_duration_ms = duration_ms.max(1_000); self }
    /// Set the maximum number of rows claimed per pass.
    pub fn with_claim_limit(mut self, limit: u32) -> Self { self.claim_limit = limit.max(1); self }

    /// Process all currently claimable rows. The returned count is the number
    /// of attempts made, not the number of bytes written.
    pub async fn run_once(&self) -> usize {
        let now = unix_ms();
        let _ = self.storage.recover_stale_outbox_leases(now);
        let _ = self.storage.expire_outbox(now);
        let mut attempted = 0;
        loop {
            let row = match self.storage.claim_due_outbox(now, &self.lease_owner, self.lease_duration_ms, self.claim_limit) {
                Ok(Some(row)) => row,
                Ok(None) | Err(_) => break,
            };
            attempted += 1;
            self.process_claim(row).await;
        }
        attempted
    }

    async fn process_claim(&self, row: OutboxRow) {
        let msg_id = row.msg_id;
        let peer = row.recipient_device_id;
        let outcome: Result<()> = async {
            let authorized = self.policy.authorize(peer).await?;
            if !authorized { return Err(n0_error::anyerr!("recipient is no longer authorized")); }
            let envelope = self.storage.get_inbox(&msg_id)?.ok_or_else(|| n0_error::anyerr!("outbox envelope is missing"))?;
            if envelope.expires_at_ms <= unix_ms() { return Err(n0_error::anyerr!("outbox envelope expired")); }
            self.transport.deliver(peer, envelope).await
        }.await;
        let now = unix_ms();
        let (success, error) = match outcome {
            Ok(()) => (true, None),
            Err(err) => (false, Some(err.to_string())),
        };
        // The lease owner check makes a late completion harmless if another
        // worker has reclaimed an expired lease.
        let _ = self.storage.finish_outbox_attempt(
            &msg_id, peer, &self.lease_owner, success,
            now.saturating_add(backoff_ms(row.attempts)), error.as_deref(),
        );
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
}

fn unix_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }
fn backoff_ms(attempts: u32) -> u64 { match attempts { 0 => 5_000, 1 => 30_000, 2 => 120_000, 3 => 600_000, 4 => 1_800_000, 5 => 7_200_000, _ => 21_600_000 } }

/// Convenience policy for applications whose contact store is already
/// authoritative and whose transport performs address resolution itself.
pub struct AllowListedPolicy<F>(pub F);
impl<F> std::fmt::Debug for AllowListedPolicy<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AllowListedPolicy").finish_non_exhaustive()
    }
}
impl<F, Fut> RecipientPolicy for AllowListedPolicy<F>
where F: Fn(PublicKey) -> Fut + Send + Sync, Fut: Future<Output = Result<bool>> + Send + 'static {
    fn authorize<'a>(&'a self, recipient: PublicKey) -> BoxFuture<Result<bool>> { Box::pin((self.0)(recipient)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn backoff_is_bounded() { assert_eq!(backoff_ms(0), 5_000); assert_eq!(backoff_ms(99), 21_600_000); }
}
