//! Transfer-request admission & preparation bounds.
//!
//! Owns [`PrepareLimiter`] (concurrency / size / timeout bounds on file
//! preparation) and [`UploadLimiter`] (queue / per-peer / active /
//! verification admission for file-access upload requests).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

// ── Preparation bounds ─────────────────────────────────────────────────────

/// Configuration for bounding expensive file-preparation work.
///
/// These limits prevent a burst of file-access requests from exhausting
/// CPU, disk I/O, or memory by launching unbounded rehash / re-import jobs.
///
/// Defaults:
/// - `max_concurrent_preparations`: 4
/// - `max_file_size_bytes`: 1 GiB
/// - `prepare_timeout`: 60 seconds
#[derive(Debug, Clone)]
pub struct PrepareConfig {
    /// Maximum number of file-preparation operations running concurrently.
    pub max_concurrent_preparations: usize,
    /// Files larger than this (in bytes) are rejected without attempting
    /// preparation.  Set to `u64::MAX` to disable the size guard.
    pub max_file_size_bytes: u64,
    /// Per-preparation timeout.  If a prepare call does not complete
    /// within this duration, the operation is cancelled.
    pub prepare_timeout: Duration,
}

impl Default for PrepareConfig {
    fn default() -> Self {
        Self {
            max_concurrent_preparations: 4,
            max_file_size_bytes: 1024 * 1024 * 1024, // 1 GiB
            prepare_timeout: Duration::from_secs(60),
        }
    }
}

/// Structured errors for bounded file-preparation operations.
#[derive(Debug, Clone)]
pub enum PrepareError {
    /// The preparation concurrency limit was reached — try again later.
    Busy,
    /// The file exceeds the configured maximum size.
    TooLarge {
        /// Actual file size in bytes.
        size_bytes: u64,
        /// Maximum allowed file size in bytes.
        max_bytes: u64,
    },
    /// The preparation operation timed out.
    Timeout {
        /// Duration of the timeout that was exceeded.
        timeout: Duration,
    },
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => write!(f, "preparation concurrency limit reached"),
            Self::TooLarge {
                size_bytes,
                max_bytes,
            } => {
                write!(
                    f,
                    "file too large for preparation: {size_bytes} bytes exceeds limit of {max_bytes} bytes"
                )
            }
            Self::Timeout { timeout } => {
                write!(f, "preparation timed out after {timeout:?}")
            }
        }
    }
}

impl std::error::Error for PrepareError {}

/// Bounds expensive file-preparation work with concurrency limiting,
/// file-size gating, and per-call timeouts.
///
/// Designed to be shared via `Arc` across protocol handler instances.
#[derive(Debug, Clone)]
pub struct PrepareLimiter {
    /// Configuration (cloned on construction, immutable thereafter).
    config: PrepareConfig,
    /// Semaphore that caps concurrent preparation operations.
    semaphore: Arc<Semaphore>,
}

impl PrepareLimiter {
    /// Create a new limiter from the given configuration.
    ///
    /// `max_concurrent_preparations` is clamped to at least 1.
    pub fn new(config: PrepareConfig) -> Self {
        let clamped = config.max_concurrent_preparations.max(1);
        Self {
            config,
            semaphore: Arc::new(Semaphore::new(clamped)),
        }
    }

    /// Return the configuration snapshot (immutable after construction).
    pub fn config(&self) -> &PrepareConfig {
        &self.config
    }

    /// Try to acquire the right to begin preparation work, subject to
    /// all bounds.
    ///
    /// Returns `Ok(permit)` when the file is within size limits and a
    /// concurrency slot is available.  Returns `Err(PrepareError)` if
    /// the file is too large or the server is busy.
    pub fn try_begin(&self, file_size_bytes: u64) -> Result<PreparePermit, PrepareError> {
        // ── 1. Size gate (cheap check before semaphore) ─────────────
        if file_size_bytes > self.config.max_file_size_bytes {
            return Err(PrepareError::TooLarge {
                size_bytes: file_size_bytes,
                max_bytes: self.config.max_file_size_bytes,
            });
        }

        // ── 2. Concurrency gate ─────────────────────────────────────
        let permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| PrepareError::Busy)?;

        Ok(PreparePermit {
            _permit: permit,
            timeout: self.config.prepare_timeout,
        })
    }
}

/// A permit that reserves one concurrent preparation slot.
///
/// Drops the slot automatically when the permit is dropped, and carries
/// the per-call timeout so the caller can apply it.
#[derive(Debug)]
pub struct PreparePermit {
    /// The semaphore permit — held for the duration of preparation.
    _permit: OwnedSemaphorePermit,
    /// Per-call timeout extracted from the limit config at admission time.
    pub timeout: Duration,
}

impl PreparePermit {
    /// Return the per-call timeout for this preparation operation.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

// ── Upload concurrency limits ──────────────────────────────────────────────

/// Configuration for bounding concurrent upload (file-access) request handling.
///
/// These limits prevent a burst of incoming file-access requests from
/// exhausting server resources by bounding how many are queued, active,
/// verifying permissions, and from the same peer.
///
/// Defaults:
/// - `max_active_uploads`: 8
/// - `max_uploads_per_peer`: 2
/// - `max_queued_uploads`: 32
/// - `max_concurrent_verifications`: 4
/// - `request_timeout`: 60 seconds
#[derive(Debug, Clone)]
pub struct UploadLimitsConfig {
    /// Maximum number of file-access requests being actively processed.
    pub max_active_uploads: usize,
    /// Maximum concurrent file-access requests from a single peer
    /// (queued + active combined).
    pub max_uploads_per_peer: usize,
    /// Maximum number of queued file-access requests waiting to start.
    pub max_queued_uploads: usize,
    /// Maximum number of concurrent permission-verification operations.
    pub max_concurrent_verifications: usize,
    /// Per-request timeout for the entire file-access handler cycle.
    pub request_timeout: Duration,
}

impl Default for UploadLimitsConfig {
    fn default() -> Self {
        Self {
            max_active_uploads: 8,
            max_uploads_per_peer: 2,
            max_queued_uploads: 32,
            max_concurrent_verifications: 4,
            request_timeout: Duration::from_secs(60),
        }
    }
}

/// Why an upload operation could not be admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadError {
    /// The upload queue is full — too many requests waiting.
    QueueFull,
    /// This peer already has the maximum number of requests queued or active.
    PeerLimitReached,
    /// The server is at capacity for permission-verification operations.
    VerificationBusy,
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => write!(f, "upload queue is full"),
            Self::PeerLimitReached => write!(f, "per-peer upload limit reached"),
            Self::VerificationBusy => write!(f, "verification concurrency limit reached"),
        }
    }
}

impl std::error::Error for UploadError {}

/// In-memory state tracked by the [`UploadLimiter`].
#[derive(Debug)]
struct UploadState {
    /// Number of requests currently in the queue (not yet active).
    queued: usize,
    /// Per-peer count of queued requests.
    queued_by_peer: HashMap<String, usize>,
    /// Per-peer count of active requests.
    active_by_peer: HashMap<String, usize>,
}

/// Admission controller for bounded upload (file-access) request handling.
///
/// Protects the server from resource exhaustion by bounding:
/// - Active requests (global semaphore)
/// - Per-peer requests (queued + active)
/// - Queued requests waiting to start
/// - Permission-verification concurrency (independent semaphore)
///
/// Designed to be shared via `Arc` across protocol handler instances.
#[derive(Debug, Clone)]
pub struct UploadLimiter {
    /// Cloned configuration snapshot.
    config: UploadLimitsConfig,
    /// Global semaphore capping concurrent active uploads.
    active: Arc<Semaphore>,
    /// Semaphore for the permission-verification phase.
    verifications: Arc<Semaphore>,
    /// Per-peer semaphores (created on demand).
    peer_slots: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    /// Tracked queue and per-peer state.
    state: Arc<Mutex<UploadState>>,
}

impl UploadLimiter {
    /// Create a new limiter from the given configuration.
    ///
    /// All limits are clamped to at least 1.
    pub fn new(config: UploadLimitsConfig) -> Self {
        let clamped = UploadLimitsConfig {
            max_active_uploads: config.max_active_uploads.max(1),
            max_uploads_per_peer: config.max_uploads_per_peer.max(1),
            max_queued_uploads: config.max_queued_uploads.max(1),
            max_concurrent_verifications: config.max_concurrent_verifications.max(1),
            request_timeout: config.request_timeout.max(Duration::from_secs(1)),
        };
        Self {
            active: Arc::new(Semaphore::new(clamped.max_active_uploads)),
            verifications: Arc::new(Semaphore::new(clamped.max_concurrent_verifications)),
            peer_slots: Arc::new(Mutex::new(HashMap::new())),
            state: Arc::new(Mutex::new(UploadState {
                queued: 0,
                queued_by_peer: HashMap::new(),
                active_by_peer: HashMap::new(),
            })),
            config: clamped,
        }
    }

    /// Return the configuration snapshot.
    pub fn config(&self) -> &UploadLimitsConfig {
        &self.config
    }

    /// Try to enqueue an upload request from the given peer.
    ///
    /// Returns `Ok(UploadPermit)` when a queue slot is available and the
    /// peer has not exceeded its per-peer limit.  The permit holds the
    /// queue slot until [`start`](UploadPermit::start) is called or the
    /// permit is dropped.
    pub fn try_enqueue(&self, peer: impl Into<String>) -> Result<UploadPermit, UploadError> {
        let peer = peer.into();
        let mut state = self.state.lock().expect("UploadLimiter state poisoned");

        // ── Global queue depth check ──────────────────────────────────
        if state.queued >= self.config.max_queued_uploads {
            return Err(UploadError::QueueFull);
        }

        // ── Per-peer check (queued + active combined) ─────────────────
        let peer_count = state.queued_by_peer.get(&peer).copied().unwrap_or(0)
            + state.active_by_peer.get(&peer).copied().unwrap_or(0);
        if peer_count >= self.config.max_uploads_per_peer {
            return Err(UploadError::PeerLimitReached);
        }

        state.queued += 1;
        *state.queued_by_peer.entry(peer.clone()).or_default() += 1;

        Ok(UploadPermit {
            limiter: self.clone(),
            peer,
            started: false,
        })
    }

    /// Try to acquire a permission-verification slot.
    ///
    /// This is an independent budget — separate from the active upload
    /// limit — for the CPU-bound permission-check phase.
    pub fn try_acquire_verification(&self) -> Result<VerificationPermit, UploadError> {
        self.verifications
            .clone()
            .try_acquire_owned()
            .map(|permit| VerificationPermit { _permit: permit })
            .map_err(|_| UploadError::VerificationBusy)
    }

    fn release_queued(&self, peer: &str) {
        let mut state = self.state.lock().expect("UploadLimiter state poisoned");
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
            .peer_slots
            .lock()
            .expect("UploadLimiter peer slots poisoned");
        slots
            .entry(peer.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(self.config.max_uploads_per_peer)))
            .clone()
    }

    fn mark_active(&self, peer: &str) {
        let mut state = self.state.lock().expect("UploadLimiter state poisoned");
        *state.active_by_peer.entry(peer.to_owned()).or_default() += 1;
    }

    fn release_active(&self, peer: &str) {
        let mut state = self.state.lock().expect("UploadLimiter state poisoned");
        if let Some(count) = state.active_by_peer.get_mut(peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.active_by_peer.remove(peer);
            }
        }
    }
}

/// A queued upload reservation that holds one slot in the queue.
///
/// Call [`start`](UploadPermit::start) to promote it to an active upload,
/// or drop it to release the queue slot.
#[derive(Debug)]
pub struct UploadPermit {
    limiter: UploadLimiter,
    peer: String,
    started: bool,
}

impl UploadPermit {
    /// Promote this queued permit to an active upload.
    ///
    /// Acquires a global active slot and a per-peer slot (awaiting if
    /// necessary), releases the queue slot, and returns an [`ActiveUpload`]
    /// guard that releases both semaphore slots on drop.
    pub async fn start(mut self) -> Result<ActiveUpload, UploadError> {
        let global = self
            .limiter
            .active
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| UploadError::QueueFull)?;
        let peer_slot = self
            .limiter
            .peer_semaphore(&self.peer)
            .acquire_owned()
            .await
            .map_err(|_| UploadError::PeerLimitReached)?;

        self.limiter.release_queued(&self.peer);
        self.limiter.mark_active(&self.peer);
        self.started = true;

        Ok(ActiveUpload {
            limiter: self.limiter.clone(),
            peer: self.peer.clone(),
            _global_permit: global,
            _peer_permit: peer_slot,
        })
    }

    /// Return the peer identifier.
    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// Return the configured request timeout.
    pub fn timeout(&self) -> Duration {
        self.limiter.config.request_timeout
    }
}

impl Drop for UploadPermit {
    fn drop(&mut self) {
        if !self.started {
            self.limiter.release_queued(&self.peer);
        }
    }
}

/// An active upload — holds global and per-peer semaphore permits.
///
/// Dropping this releases both permits and updates accounting.
#[derive(Debug)]
pub struct ActiveUpload {
    limiter: UploadLimiter,
    peer: String,
    _global_permit: OwnedSemaphorePermit,
    _peer_permit: OwnedSemaphorePermit,
}

impl Drop for ActiveUpload {
    fn drop(&mut self) {
        self.limiter.release_active(&self.peer);
    }
}

/// A permission-verification permit — released on drop.
#[derive(Debug)]
pub struct VerificationPermit {
    _permit: OwnedSemaphorePermit,
}
