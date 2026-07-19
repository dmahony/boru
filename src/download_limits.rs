//! Bounded admission and resource controls for file downloads.
//!
//! The limiter keeps large bursts bounded before they reach the transfer layer:
//! queued work is admitted atomically, active transfers are capped globally and
//! per peer, hash verification has its own cap, and progress writes can be
//! coalesced to avoid turning the database into a hot path.

#![allow(missing_docs)]

use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Resource limits applied before starting a download.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct DownloadLimitsConfig {
    pub max_concurrent_downloads: usize,
    pub max_startup_downloads: usize,
    pub max_downloads_per_peer: usize,
    pub max_active_hash_verifications: usize,
    pub max_queued_downloads: usize,
    pub progress_update_interval: Duration,
}

impl Default for DownloadLimitsConfig {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 5,
            max_startup_downloads: 3,
            max_downloads_per_peer: 2,
            max_active_hash_verifications: 2,
            max_queued_downloads: 32,
            progress_update_interval: Duration::from_millis(250),
        }
    }
}

impl DownloadLimitsConfig {
    /// Validate values loaded from an external configuration source.
    pub fn validate(&self) -> Result<(), DownloadLimitsConfigError> {
        for (field, value) in [
            ("max_concurrent_downloads", self.max_concurrent_downloads),
            ("max_startup_downloads", self.max_startup_downloads),
            ("max_downloads_per_peer", self.max_downloads_per_peer),
            (
                "max_active_hash_verifications",
                self.max_active_hash_verifications,
            ),
            ("max_queued_downloads", self.max_queued_downloads),
        ] {
            if value == 0 {
                return Err(DownloadLimitsConfigError::InvalidValue {
                    field,
                    reason: "must be greater than zero",
                });
            }
        }
        Ok(())
    }

    /// Parse and validate a JSON configuration document.
    pub fn from_json_str(contents: &str) -> Result<Self, DownloadLimitsConfigError> {
        let config: Self =
            serde_json::from_str(contents).map_err(DownloadLimitsConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    /// Load and validate a JSON configuration file.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, DownloadLimitsConfigError> {
        let path = path.as_ref();
        let contents =
            fs::read_to_string(path).map_err(|source| DownloadLimitsConfigError::Io {
                path: path.display().to_string(),
                source,
            })?;
        Self::from_json_str(&contents)
    }

    /// Load overrides from environment variables, falling back to defaults.
    pub fn from_env() -> Result<Self, DownloadLimitsConfigError> {
        let mut config = Self::default();
        for (name, target) in [
            (
                "BORU_CHAT_MAX_CONCURRENT_DOWNLOADS",
                &mut config.max_concurrent_downloads,
            ),
            (
                "BORU_CHAT_MAX_STARTUP_DOWNLOADS",
                &mut config.max_startup_downloads,
            ),
            (
                "BORU_CHAT_MAX_DOWNLOADS_PER_PEER",
                &mut config.max_downloads_per_peer,
            ),
            (
                "BORU_CHAT_MAX_QUEUED_DOWNLOADS",
                &mut config.max_queued_downloads,
            ),
        ] {
            if let Ok(value) = std::env::var(name) {
                *target = value
                    .parse()
                    .map_err(|_| DownloadLimitsConfigError::InvalidEnv {
                        variable: name,
                        value,
                    })?;
            }
        }
        config.validate()?;

        // Load progress DB update interval from env (overrides progress_update_interval).
        if let Ok(value) = std::env::var("BORU_CHAT_PROGRESS_DB_UPDATE_INTERVAL_MS") {
            let millis: u64 = value
                .parse()
                .map_err(|_| DownloadLimitsConfigError::InvalidEnv {
                    variable: "BORU_CHAT_PROGRESS_DB_UPDATE_INTERVAL_MS",
                    value,
                })?;
            config.progress_update_interval = Duration::from_millis(millis);
        }

        Ok(config)
    }
}

/// Error returned when download limits cannot be loaded safely.
#[derive(Debug)]
pub enum DownloadLimitsConfigError {
    Io {
        path: String,
        source: std::io::Error,
    },
    Parse(serde_json::Error),
    InvalidValue {
        field: &'static str,
        reason: &'static str,
    },
    InvalidEnv {
        variable: &'static str,
        value: String,
    },
}

impl std::fmt::Display for DownloadLimitsConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "cannot read download limits '{path}': {source}")
            }
            Self::Parse(source) => write!(f, "invalid download limits JSON: {source}"),
            Self::InvalidValue { field, reason } => {
                write!(f, "invalid download limit '{field}': {reason}")
            }
            Self::InvalidEnv { variable, value } => write!(
                f,
                "invalid {variable} value '{value}': expected a positive integer"
            ),
        }
    }
}

impl std::error::Error for DownloadLimitsConfigError {}

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
    startup: Arc<Semaphore>,
    hash_verification: Arc<Semaphore>,
    peer_slots: Mutex<HashMap<String, Arc<Semaphore>>>,
}

/// Admission controller shared by all download tasks in a process.
#[derive(Clone, Debug)]
pub struct DownloadLimiter(Arc<Inner>);

impl DownloadLimiter {
    pub fn new(config: DownloadLimitsConfig) -> Self {
        let config = DownloadLimitsConfig {
            max_concurrent_downloads: config.max_concurrent_downloads.max(1),
            max_downloads_per_peer: config.max_downloads_per_peer.max(1),
            max_active_hash_verifications: config.max_active_hash_verifications.max(1),
            max_queued_downloads: config.max_queued_downloads.max(1),
            ..config
        };
        Self(Arc::new(Inner {
            active: Arc::new(Semaphore::new(config.max_concurrent_downloads)),
            startup: Arc::new(Semaphore::new(config.max_startup_downloads)),
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
            startup: None,
        })
    }

    /// Reserve a slot for a download restored during application startup.
    pub fn try_enqueue_startup(
        &self,
        peer: impl Into<String>,
    ) -> Result<QueuedDownload, DownloadLimitError> {
        let startup = self
            .0
            .startup
            .clone()
            .try_acquire_owned()
            .map_err(|_| DownloadLimitError::QueueFull)?;
        match self.try_enqueue(peer) {
            Ok(mut queued) => {
                queued.startup = Some(startup);
                Ok(queued)
            }
            Err(error) => Err(error),
        }
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

    /// Acquire the CPU budget for post-transfer hashing, blocking until a
    /// permit is available.
    ///
    /// Unlike [`try_acquire_hash_verification`] this waits instead of
    /// returning [`DownloadLimitError::HashVerificationBusy`].  Use this when
    /// the caller already holds an active download slot (and therefore the
    /// download is guaranteed to proceed eventually).
    pub async fn acquire_hash_verification(&self) -> HashVerificationPermit {
        let permit = self
            .0
            .hash_verification
            .clone()
            .acquire_owned()
            .await
            .expect("hash verification semaphore closed");
        HashVerificationPermit { _permit: permit }
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
    startup: Option<OwnedSemaphorePermit>,
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
            _startup: self.startup.take(),
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
    _startup: Option<OwnedSemaphorePermit>,
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

/// Batches pending progress updates from concurrent downloads and writes
/// them in a single SQLite transaction when flushed.
///
/// # Design
///
/// Each call to [`submit`](Self::submit) queues the update in memory.
/// When enough time has elapsed since the last flush, the method returns
/// `true` to signal that the caller should call [`flush`](Self::flush)
/// with a function that writes the batch.  Between flushes, pending
/// updates are accumulated and the most recent value per download id
/// replaces any older entry, so the database write always reflects the
/// latest byte count.
///
/// The `flush` closure receives a `&mut HashMap<i64, (u64, &str)>` of
/// `(download_id, (bytes_downloaded, state))` tuples.  The batch is
/// drained from the writer, so a second flush without intervening
/// submits is a no-op.
#[derive(Debug)]
pub struct BatchedProgressWriter {
    pending: Mutex<HashMap<i64, (u64, String)>>,
    interval: Duration,
    last_flush: Mutex<Instant>,
}

impl BatchedProgressWriter {
    pub fn new(interval: Duration) -> Self {
        // Start with a stale timer so the first submit always signals flush.
        let stale = Instant::now()
            .checked_sub(interval)
            .unwrap_or(Instant::now());
        Self {
            pending: Mutex::new(HashMap::new()),
            interval,
            last_flush: Mutex::new(stale),
        }
    }

    /// Queue a progress update.
    ///
    /// Returns `true` when the interval has elapsed since the last flush,
    /// signalling that the caller should call [`flush`](Self::flush).
    pub fn submit(&self, download_id: i64, bytes_downloaded: u64, state: &str) -> bool {
        let mut pending = self.pending.lock().expect("batched writer poisoned");
        pending.insert(download_id, (bytes_downloaded, state.to_owned()));

        let mut last_flush = self.last_flush.lock().expect("batched writer poisoned");
        if last_flush.elapsed() >= self.interval {
            *last_flush = Instant::now();
            true
        } else {
            false
        }
    }

    /// Drain all pending updates and pass them to `write_fn`.
    ///
    /// The closure receives a `&[(i64, u64, &str)]` — a snapshot of every
    /// queued `(download_id, bytes_downloaded, state)` tuple at the
    /// time of the call.  After `write_fn` returns (even on error), the
    /// pending queue is cleared so that stale entries are not written
    /// twice.
    pub fn flush<F, E>(&self, write_fn: F) -> Result<(), E>
    where
        F: FnOnce(&[(i64, u64, &str)]) -> Result<(), E>,
    {
        let batch: Vec<(i64, u64, String)> = {
            let mut pending = self.pending.lock().expect("batched writer poisoned");
            if pending.is_empty() {
                return Ok(());
            }
            pending
                .drain()
                .map(|(id, (bytes, state))| (id, bytes, state))
                .collect()
        };

        let refs: Vec<(i64, u64, &str)> = batch
            .iter()
            .map(|(id, bytes, state)| (*id, *bytes, state.as_str()))
            .collect();

        write_fn(&refs)?;
        Ok(())
    }

    /// Returns `true` when there are queued updates waiting to be flushed.
    pub fn has_pending(&self) -> bool {
        !self
            .pending
            .lock()
            .expect("batched writer poisoned")
            .is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_json_overrides() {
        let defaults = DownloadLimitsConfig::default();
        assert_eq!(defaults.max_concurrent_downloads, 5);
        assert_eq!(defaults.max_startup_downloads, 3);
        let config = DownloadLimitsConfig::from_json_str(
            r#"{"max_concurrent_downloads": 8, "max_startup_downloads": 2}"#,
        )
        .unwrap();
        assert_eq!(config.max_concurrent_downloads, 8);
        assert_eq!(config.max_startup_downloads, 2);
        assert_eq!(config.max_queued_downloads, defaults.max_queued_downloads);
    }

    #[test]
    fn config_rejects_zero_values() {
        let error =
            DownloadLimitsConfig::from_json_str(r#"{"max_concurrent_downloads": 0}"#).unwrap_err();
        assert!(error.to_string().contains("max_concurrent_downloads"));
    }

    #[test]
    fn startup_admission_is_bounded() {
        let limiter = DownloadLimiter::new(DownloadLimitsConfig {
            max_concurrent_downloads: 5,
            max_startup_downloads: 1,
            max_downloads_per_peer: 5,
            max_active_hash_verifications: 1,
            max_queued_downloads: 5,
            progress_update_interval: Duration::from_millis(1),
        });
        let first = limiter.try_enqueue_startup("a").unwrap();
        assert!(limiter.try_enqueue_startup("b").is_err());
        drop(first);
        assert!(limiter.try_enqueue_startup("b").is_ok());
    }

    fn limiter() -> DownloadLimiter {
        DownloadLimiter::new(DownloadLimitsConfig {
            max_concurrent_downloads: 2,
            max_startup_downloads: 1,
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
    #[tokio::test]
    async fn acquire_hash_verification_waits_for_permit() {
        let limiter = limiter(); // max_active_hash_verifications: 1
        let first = limiter.try_acquire_hash_verification().unwrap();
        // Second attempt would overflow the semaphore; try_acquire rejects.
        assert!(limiter.try_acquire_hash_verification().is_err());
        // Spawn a task that waits for the permit via the async method.
        let limiter2 = limiter.clone();
        let waiter = tokio::spawn(async move { limiter2.acquire_hash_verification().await });
        assert!(!waiter.is_finished(), "waiter should be blocked");
        drop(first);
        let _permit = waiter.await.unwrap();
        assert!(
            limiter.try_acquire_hash_verification().is_err(),
            "should hold permit"
        );
    }
    #[test]
    fn progress_gate_coalesces_database_updates() {
        let gate = ProgressUpdateGate::new(Duration::from_secs(1));
        let now = Instant::now();
        assert!(gate.should_persist(now));
        assert!(!gate.should_persist(now + Duration::from_millis(999)));
        assert!(gate.should_persist(now + Duration::from_secs(1)));
    }

    // ── BatchedProgressWriter tests ─────────────────────────────────────

    #[test]
    fn batched_writer_accumulates_and_flushes() {
        let writer = BatchedProgressWriter::new(Duration::from_millis(10));
        // Submit returns true on first call (interval starts now).
        assert!(
            writer.submit(1, 100, "downloading"),
            "first submit should signal flush"
        );
        assert!(
            writer.has_pending(),
            "writer should have pending after submit"
        );

        // Submit again with same id — replaces old value.
        assert!(
            !writer.submit(1, 200, "downloading"),
            "second submit within interval should NOT signal flush"
        );

        // Submit a second download.
        assert!(
            !writer.submit(2, 50, "downloading"),
            "different id still within interval"
        );

        // Flush should drain both updates.
        let mut flushed: Vec<(i64, u64, String)> = Vec::new();
        writer
            .flush(|batch| {
                for &(id, bytes, state) in batch {
                    flushed.push((id, bytes, state.to_owned()));
                }
                Ok::<_, ()>(())
            })
            .unwrap();

        // Should have 2 entries — id=2 first (HashMap ordering), id=1 with 200 (latest value).
        assert_eq!(flushed.len(), 2, "should flush 2 pending updates");
        assert!(!writer.has_pending(), "writer should be empty after flush");

        // Verify the replacement worked: id=1 has 200, not 100.
        let entry1 = flushed.iter().find(|(id, _, _)| *id == 1).unwrap();
        assert_eq!(entry1.1, 200, "id=1 should use latest bytes");
    }

    #[test]
    fn batched_writer_empty_flush_is_noop() {
        let writer = BatchedProgressWriter::new(Duration::from_millis(10));
        assert!(!writer.has_pending());

        let mut called = false;
        writer
            .flush(|_| {
                called = true;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert!(!called, "flush should not call write_fn when empty");
    }

    #[test]
    fn batched_writer_interval_respects_submit_timing() {
        // Use a long interval so multiple submits stay within it.
        let writer = BatchedProgressWriter::new(Duration::from_secs(60));
        assert!(
            writer.submit(1, 10, "downloading"),
            "first submit signals flush"
        );
        assert!(
            !writer.submit(2, 20, "downloading"),
            "immediate second submit does not signal"
        );

        // Manually wait for interval to elapse.
        {
            let mut last_flush = writer.last_flush.lock().unwrap();
            *last_flush = Instant::now() - Duration::from_secs(61);
        }

        assert!(
            writer.submit(3, 30, "downloading"),
            "after interval elapsed, submit should signal flush again"
        );
    }

    #[test]
    fn batched_writer_flush_error_propagates() {
        let writer = BatchedProgressWriter::new(Duration::from_millis(10));
        writer.submit(1, 100, "downloading");
        let result: Result<(), &str> = writer.flush(|_| Err("boom"));
        assert_eq!(result.unwrap_err(), "boom", "flush errors should propagate");
        // After error, pending is still drained.
        assert!(
            !writer.has_pending(),
            "pending cleared even after flush error"
        );
    }

    #[test]
    fn batched_writer_submit_replaces_stale_entry() {
        let writer = BatchedProgressWriter::new(Duration::from_secs(60));
        // Submit three updates for the same download.
        assert!(writer.submit(1, 10, "downloading"), "first submit");
        writer.submit(1, 50, "downloading");
        writer.submit(1, 100, "downloading");

        let mut flushed: Vec<(i64, u64, String)> = Vec::new();
        writer
            .flush(|batch| {
                for &(id, bytes, state) in batch {
                    flushed.push((id, bytes, state.to_owned()));
                }
                Ok::<_, ()>(())
            })
            .unwrap();

        assert_eq!(flushed.len(), 1, "only one entry for same download id");
        assert_eq!(flushed[0].1, 100, "should retain the latest byte count");
    }

    fn per_peer_limiter(max_per_peer: usize) -> DownloadLimiter {
        DownloadLimiter::new(DownloadLimitsConfig {
            max_concurrent_downloads: 5,
            max_startup_downloads: 3,
            max_downloads_per_peer: max_per_peer,
            max_active_hash_verifications: 2,
            max_queued_downloads: 10,
            progress_update_interval: Duration::from_millis(1),
        })
    }

    #[tokio::test]
    async fn per_peer_allows_up_to_limit_active_simultaneously() {
        let limiter = per_peer_limiter(2);
        let d1 = limiter.try_enqueue("peer").unwrap().start().await.unwrap();
        let d2 = limiter.try_enqueue("peer").unwrap().start().await.unwrap();
        assert_eq!(
            limiter.try_enqueue("peer").unwrap_err(),
            DownloadLimitError::PeerQueueFull,
            "third enqueue from same peer should be rejected"
        );
        drop(d1);
        drop(d2);
    }

    #[tokio::test]
    async fn per_peer_completion_frees_slot_for_same_peer() {
        let limiter = per_peer_limiter(2);
        let d1 = limiter.try_enqueue("peer").unwrap().start().await.unwrap();
        let d2 = limiter.try_enqueue("peer").unwrap().start().await.unwrap();

        // Complete one — drop releases its per-peer slot.
        drop(d1);

        // Now another queued item from the same peer can start.
        let d3 = limiter.try_enqueue("peer").unwrap().start().await.unwrap();
        assert!(limiter.try_enqueue("peer").is_err());
        drop(d2);
        drop(d3);
    }

    #[tokio::test]
    async fn two_peers_have_independent_per_peer_budgets() {
        let limiter = per_peer_limiter(2);
        let a1 = limiter.try_enqueue("alice").unwrap().start().await.unwrap();
        let a2 = limiter.try_enqueue("alice").unwrap().start().await.unwrap();
        // alice has 2 active — 3rd should be rejected.
        assert_eq!(
            limiter.try_enqueue("alice").unwrap_err(),
            DownloadLimitError::PeerQueueFull,
        );
        // bob should still be able to enqueue+start (independent budget).
        let b1 = limiter.try_enqueue("bob").unwrap().start().await.unwrap();
        let b2 = limiter.try_enqueue("bob").unwrap().start().await.unwrap();
        // Both at capacity — 4 active total, all within global limit (5).
        assert_eq!(
            limiter.try_enqueue("bob").unwrap_err(),
            DownloadLimitError::PeerQueueFull,
        );
        drop(a1);
        drop(a2);
        drop(b1);
        drop(b2);
    }

    #[tokio::test]
    async fn per_peer_deactivates_on_queued_drop_before_start() {
        let limiter = per_peer_limiter(2);
        let _d1 = limiter.try_enqueue("peer").unwrap();
        // Drop the QueuedDownload before start — should release its peer queue slot.
        drop(_d1);
        // Now can enqueue again (count back to 0).
        let d2 = limiter.try_enqueue("peer").unwrap().start().await.unwrap();
        let d3 = limiter.try_enqueue("peer").unwrap().start().await.unwrap();
        drop(d2);
        drop(d3);
    }

    #[tokio::test]
    async fn per_peer_one_slot_only() {
        let limiter = per_peer_limiter(1);
        let d1 = limiter.try_enqueue("peer").unwrap().start().await.unwrap();
        assert_eq!(
            limiter.try_enqueue("peer").unwrap_err(),
            DownloadLimitError::PeerQueueFull,
        );
        // Different peer unaffected.
        let _d2 = limiter.try_enqueue("other").unwrap().start().await.unwrap();
        drop(d1);
    }
}
