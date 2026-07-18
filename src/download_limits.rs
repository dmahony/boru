//! Bounded admission and resource controls for file downloads.
//!
//! The limiter keeps large bursts bounded before they reach the transfer layer:
//! queued work is admitted atomically, active transfers are capped globally and
//! per peer, hash verification has its own cap, and progress writes can be
//! coalesced to avoid turning the database into a hot path.

#![allow(missing_docs)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Resource limits applied before starting a download.
#[derive(Clone, Debug)]
pub struct DownloadLimitsConfig {
    pub max_active_downloads: usize,
    pub max_downloads_per_peer: usize,
    pub max_active_hash_verifications: usize,
    pub max_queued_downloads: usize,
    pub progress_update_interval: Duration,
}

impl Default for DownloadLimitsConfig {
    fn default() -> Self {
        Self {
            max_active_downloads: 4,
            max_downloads_per_peer: 1,
            max_active_hash_verifications: 2,
            max_queued_downloads: 32,
            progress_update_interval: Duration::from_millis(250),
        }
    }
}

/// Why an operation could not be admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadLimitError {
    QueueFull,
    PeerQueueFull,
    HashVerificationBusy,
}

#[derive(Debug)]
struct State {
    queued: usize,
    queued_by_peer: HashMap<String, usize>,
    active_by_peer: HashMap<String, usize>,
}

#[derive(Debug)]
struct Inner {
    config: DownloadLimitsConfig,
    state: Mutex<State>,
    active: Arc<Semaphore>,
    hash_verification: Arc<Semaphore>,
    peer_slots: Mutex<HashMap<String, Arc<Semaphore>>>,
}

/// Admission controller shared by all download tasks in a process.
#[derive(Clone, Debug)]
pub struct DownloadLimiter(Arc<Inner>);

impl DownloadLimiter {
    pub fn new(config: DownloadLimitsConfig) -> Self {
        let config = DownloadLimitsConfig {
            max_active_downloads: config.max_active_downloads.max(1),
            max_downloads_per_peer: config.max_downloads_per_peer.max(1),
            max_active_hash_verifications: config.max_active_hash_verifications.max(1),
            max_queued_downloads: config.max_queued_downloads.max(1),
            ..config
        };
        Self(Arc::new(Inner {
            active: Arc::new(Semaphore::new(config.max_active_downloads)),
            hash_verification: Arc::new(Semaphore::new(config.max_active_hash_verifications)),
            state: Mutex::new(State {
                queued: 0,
                queued_by_peer: HashMap::new(),
                active_by_peer: HashMap::new(),
            }),
            peer_slots: Mutex::new(HashMap::new()),
            config,
        }))
    }

    /// Reserve one bounded queue slot. The reservation releases itself on drop.
    pub fn try_enqueue(
        &self,
        peer: impl Into<String>,
    ) -> Result<QueuedDownload, DownloadLimitError> {
        let peer = peer.into();
        let mut state = self
            .0
            .state
            .lock()
            .expect("download limiter state poisoned");
        if state.queued >= self.0.config.max_queued_downloads {
            return Err(DownloadLimitError::QueueFull);
        }
        let peer_count = state.queued_by_peer.get(&peer).copied().unwrap_or(0)
            + state.active_by_peer.get(&peer).copied().unwrap_or(0);
        if peer_count >= self.0.config.max_downloads_per_peer {
            return Err(DownloadLimitError::PeerQueueFull);
        }
        state.queued += 1;
        *state.queued_by_peer.entry(peer.clone()).or_default() += 1;
        Ok(QueuedDownload {
            limiter: self.clone(),
            peer,
            started: false,
        })
    }

    /// Try to acquire the independent CPU budget used for post-transfer hashing.
    ///
    /// Hash verification is deliberately non-queued: callers can leave the
    /// download in its bounded queue and retry this admission later instead of
    /// creating an unbounded task backlog.
    pub fn try_acquire_hash_verification(
        &self,
    ) -> Result<HashVerificationPermit, DownloadLimitError> {
        self.0
            .hash_verification
            .clone()
            .try_acquire_owned()
            .map(|permit| HashVerificationPermit { _permit: permit })
            .map_err(|_| DownloadLimitError::HashVerificationBusy)
    }

    fn release_queue(&self, peer: &str) {
        let mut state = self
            .0
            .state
            .lock()
            .expect("download limiter state poisoned");
        state.queued = state.queued.saturating_sub(1);
        if let Some(count) = state.queued_by_peer.get_mut(peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.queued_by_peer.remove(peer);
            }
        }
    }

    fn peer_semaphore(&self, peer: &str) -> Arc<Semaphore> {
        let mut slots = self
            .0
            .peer_slots
            .lock()
            .expect("download limiter peer slots poisoned");
        slots
            .entry(peer.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(self.0.config.max_downloads_per_peer)))
            .clone()
    }

    fn mark_active(&self, peer: &str) {
        let mut state = self
            .0
            .state
            .lock()
            .expect("download limiter state poisoned");
        *state.active_by_peer.entry(peer.to_owned()).or_default() += 1;
    }

    fn release_active(&self, peer: &str) {
        let mut state = self
            .0
            .state
            .lock()
            .expect("download limiter state poisoned");
        if let Some(count) = state.active_by_peer.get_mut(peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.active_by_peer.remove(peer);
            }
        }
    }

    /// Create a progress-write gate using this limiter's configured interval.
    pub fn progress_gate(&self) -> ProgressUpdateGate {
        ProgressUpdateGate::new(self.0.config.progress_update_interval)
    }
}

/// A bounded queued download reservation.
#[derive(Debug)]
pub struct QueuedDownload {
    limiter: DownloadLimiter,
    peer: String,
    started: bool,
}

impl QueuedDownload {
    /// Wait for both the global and per-peer active-transfer budgets.
    pub async fn start(mut self) -> Result<ActiveDownload, DownloadLimitError> {
        let global = self
            .limiter
            .0
            .active
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DownloadLimitError::QueueFull)?;
        let peer = self
            .limiter
            .peer_semaphore(&self.peer)
            .acquire_owned()
            .await
            .map_err(|_| DownloadLimitError::PeerQueueFull)?;
        self.limiter.release_queue(&self.peer);
        self.limiter.mark_active(&self.peer);
        self.started = true;
        Ok(ActiveDownload {
            limiter: self.limiter.clone(),
            peer_name: self.peer.clone(),
            _global: global,
            _peer: peer,
        })
    }
}

impl Drop for QueuedDownload {
    fn drop(&mut self) {
        if !self.started {
            self.limiter.release_queue(&self.peer);
        }
    }
}

/// Active transfer budget. Dropping it releases global and per-peer slots.
#[derive(Debug)]
pub struct ActiveDownload {
    limiter: DownloadLimiter,
    peer_name: String,
    _global: OwnedSemaphorePermit,
    _peer: OwnedSemaphorePermit,
}

impl Drop for ActiveDownload {
    fn drop(&mut self) {
        self.limiter.release_active(&self.peer_name);
    }
}

impl ActiveDownload {
    pub fn peer(&self) -> &str {
        &self.peer_name
    }
    pub fn limiter(&self) -> &DownloadLimiter {
        &self.limiter
    }
}

/// Independent permit for CPU-bound hash verification.
#[derive(Debug)]
pub struct HashVerificationPermit {
    _permit: OwnedSemaphorePermit,
}

/// Coalesces high-frequency progress events into database writes.
#[derive(Debug)]
pub struct ProgressUpdateGate {
    interval: Duration,
    last_persisted: Mutex<Option<Instant>>,
}

impl ProgressUpdateGate {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_persisted: Mutex::new(None),
        }
    }

    pub fn should_persist(&self, now: Instant) -> bool {
        let mut last = self.last_persisted.lock().expect("progress gate poisoned");
        if last
            .map(|at| now.duration_since(at) < self.interval)
            .unwrap_or(false)
        {
            return false;
        }
        *last = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limiter() -> DownloadLimiter {
        DownloadLimiter::new(DownloadLimitsConfig {
            max_active_downloads: 2,
            max_downloads_per_peer: 1,
            max_active_hash_verifications: 1,
            max_queued_downloads: 2,
            progress_update_interval: Duration::from_millis(100),
        })
    }
    #[test]
    fn admission_bounds_queue_and_peer_budget() {
        let limiter = limiter();
        let first = limiter.try_enqueue("alice").unwrap();
        assert!(limiter.try_enqueue("alice").is_err());
        let second = limiter.try_enqueue("bob").unwrap();
        assert!(limiter.try_enqueue("carol").is_err());
        drop(first);
        assert!(limiter.try_enqueue("alice").is_ok());
        drop(second);
    }
    #[tokio::test]
    async fn active_downloads_are_bounded_globally() {
        let limiter = limiter();
        let a = limiter.try_enqueue("a").unwrap().start().await.unwrap();
        assert!(limiter.try_enqueue("a").is_err());
        let b = limiter.try_enqueue("b").unwrap().start().await.unwrap();
        let queued = limiter.try_enqueue("c").unwrap();
        let waiter = tokio::spawn(async move { queued.start().await.unwrap() });
        assert!(!waiter.is_finished());
        drop(a);
        let _c = waiter.await.unwrap();
        drop(b);
    }
    #[test]
    fn hash_verification_has_independent_limit() {
        let limiter = limiter();
        let first = limiter.try_acquire_hash_verification().unwrap();
        assert!(limiter.try_acquire_hash_verification().is_err());
        drop(first);
        assert!(limiter.try_acquire_hash_verification().is_ok());
    }
    #[test]
    fn progress_gate_coalesces_database_updates() {
        let gate = ProgressUpdateGate::new(Duration::from_secs(1));
        let now = Instant::now();
        assert!(gate.should_persist(now));
        assert!(!gate.should_persist(now + Duration::from_millis(999)));
        assert!(gate.should_persist(now + Duration::from_secs(1)));
    }
}
