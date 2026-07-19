//! File access (download-authorisation) protocol handler — server side.
//!
//! Implements [`ProtocolHandler`] for the `/boru-file-access/1` ALPN.
//! On each incoming connection:
//!
//! 1. Authenticate the requester via [`Connection::remote_id()`].
//! 2. Deserialise and validate the [`FileAccessWireRequest`].
//! 3. Perform a **request-time** permission, availability, and integrity
//!    check against the current database state.
//! 4. Issue a [`SignedDownloadDescriptor`] (short-lived) or return the
//!    appropriate refusal variant.
//!
//! The handler never relies on cached catalogue state — every request is
//! checked against live database state so stale catalogues cannot grant
//! stale access.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    PublicKey,
};
use n0_error::Result;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{debug, error, info, warn};

use crate::file_access_protocol::{
    sign_download_descriptor, BlobFormat, FileAccessErrorCode, FileAccessRequest,
    FileAccessResponse, FileAccessWireRequest, FileAccessWireResponse, PreparedFile,
};
use crate::friends::{FriendId, FriendRelationship, FriendsStore};
use crate::storage::Storage;

use rusqlite::params;

/// Outcome of checking a nonce against the [`NonceStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceCheck {
    /// The nonce is new — no replay detected.
    Accepted,
    /// The nonce was already consumed — replay attempt.
    Replayed,
}

/// In-memory store for tracking used download-descriptor nonces.
///
/// Each nonce is stored with an expiry timestamp (`expires_at_ms`).  A
/// nonce that appears in the store is considered *consumed* and will not
/// be accepted again, even if the descriptor's TTL has not yet elapsed.
///
/// # Replay-prevention policy
///
/// - Descriptors are **single-use**: a nonce is marked consumed upon
///   first presentation to the download protocol.
/// - Replayed descriptors (same nonce) are rejected with
///   [`DescriptorVerification::NonceReused`].
/// - Expired entries are cleaned up lazily on every [`check`](Self::check) /
///   [`check_and_mark`](Self::check_and_mark) call, keeping the store
///   bounded to at most one TTL window's worth of entries.
///
/// # Concurrency
///
/// `NonceStore` is `Send + Sync`, suitable for sharing via `Arc` between
/// the issuance handler and the download transfer handler.
#[derive(Debug)]
pub struct NonceStore {
    /// Map from nonce bytes to the descriptor's `expires_at_ms`.
    seen: Mutex<HashMap<[u8; 32], u64>>,
}

impl NonceStore {
    /// Create a new empty nonce store.
    pub fn new() -> Self {
        Self {
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Check whether `nonce` has already been consumed.
    ///
    /// Returns [`NonceCheck::Accepted`] if the nonce is new (or its
    /// prior entry has expired).  Returns [`NonceCheck::Replayed`] if
    /// the nonce is already in the store and its expiry has not passed.
    ///
    /// A side-effect-free check — the nonce is NOT marked.
    pub fn check(&self, nonce: &[u8; 32], now_ms: u64) -> NonceCheck {
        let mut map = self.seen.lock().expect("NonceStore lock poisoned");
        self.evict_expired(&mut map, now_ms);

        if map.contains_key(nonce) {
            NonceCheck::Replayed
        } else {
            NonceCheck::Accepted
        }
    }

    /// Atomically check and mark a nonce as consumed.
    ///
    /// If the nonce is new (or its prior entry has expired), it is
    /// inserted with the given `expires_at_ms` and `Accepted` is
    /// returned.  If it is already tracked and unexpired, `Replayed`
    /// is returned and the map is unchanged.
    pub fn check_and_mark(&self, nonce: [u8; 32], expires_at_ms: u64, now_ms: u64) -> NonceCheck {
        let mut map = self.seen.lock().expect("NonceStore lock poisoned");
        self.evict_expired(&mut map, now_ms);

        if map.contains_key(&nonce) {
            return NonceCheck::Replayed;
        }

        map.insert(nonce, expires_at_ms);
        NonceCheck::Accepted
    }

    /// Remove all nonces whose expiry has passed.
    fn evict_expired(&self, map: &mut HashMap<[u8; 32], u64>, now_ms: u64) {
        map.retain(|_, expires_at| *expires_at > now_ms);
    }

    /// Return the number of tracked (unexpired) nonces.
    ///
    /// Useful for testing and metrics.
    pub fn len(&self) -> usize {
        self.seen.lock().expect("NonceStore lock poisoned").len()
    }

    /// Return true if the store holds no unexpired nonces.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for NonceStore {
    fn default() -> Self {
        Self::new()
    }
}

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

/// Lifetime for issued [`SignedDownloadDescriptor`] values (60 seconds).
const DOWNLOAD_DESCRIPTOR_TTL: Duration = Duration::from_secs(60);

/// ALPN for the file access (download authorisation) protocol.
///
/// Matches `net::FILE_ACCESS_ALPN` (`/boru-file-access/1`).
pub const FILE_ACCESS_ALPN: &[u8] = b"/boru-file-access/1";

/// File-access protocol handler — re-checks permissions at request time.
///
/// Every incoming `FileAccessRequest` is validated against the current
/// storage and friends state, so that a stale cached catalogue cannot
/// grant access to a file that has since been revoked, disabled, or
/// changed.
///
/// The handler shares a [`NonceStore`] with the download transfer handler
/// to enforce single-use replay protection on issued descriptors.
#[derive(Debug, Clone)]
pub struct FileAccessHandler {
    /// Shared storage backend.
    storage: Arc<Storage>,
    /// The secret key of the owning profile.
    secret_key: iroh::SecretKey,
    /// The owning profile's user id (the PublicKey string form).
    profile_user_id: String,
    /// Friends store — relationship and permission lookups.
    friends: FriendsStore,
    /// Shared nonce store for single-use descriptor enforcement.
    nonce_store: Arc<NonceStore>,
    /// iroh-blobs store — used to verify imported file availability.
    blob_store: Arc<iroh_blobs::api::Store>,
    /// Preparation bounds — concurrency, size, and timeout limits.
    prepare_limiter: Arc<PrepareLimiter>,
    /// Upload (file-access request) admission limits.
    upload_limiter: Arc<UploadLimiter>,
}

impl FileAccessHandler {
    /// Create a new [`FileAccessHandler`].
    ///
    /// Uses default [`PrepareConfig`] and [`UploadLimitsConfig`] for bounds. Call
    /// [`with_limiters`](Self::with_limiters) to override.
    pub fn new(
        storage: Arc<Storage>,
        secret_key: iroh::SecretKey,
        profile_user_id: String,
        friends: FriendsStore,
        nonce_store: Arc<NonceStore>,
        blob_store: Arc<iroh_blobs::api::Store>,
    ) -> Self {
        Self {
            storage,
            secret_key,
            profile_user_id,
            friends,
            nonce_store,
            blob_store,
            prepare_limiter: Arc::new(PrepareLimiter::new(PrepareConfig::default())),
            upload_limiter: Arc::new(UploadLimiter::new(UploadLimitsConfig::default())),
        }
    }

    /// Create a new [`FileAccessHandler`] with custom [`PrepareLimiter`] and
    /// [`UploadLimiter`].
    #[allow(clippy::too_many_arguments)]
    pub fn with_limiters(
        storage: Arc<Storage>,
        secret_key: iroh::SecretKey,
        profile_user_id: String,
        friends: FriendsStore,
        nonce_store: Arc<NonceStore>,
        blob_store: Arc<iroh_blobs::api::Store>,
        prepare_limiter: Arc<PrepareLimiter>,
        upload_limiter: Arc<UploadLimiter>,
    ) -> Self {
        Self {
            storage,
            secret_key,
            profile_user_id,
            friends,
            nonce_store,
            blob_store,
            prepare_limiter,
            upload_limiter,
        }
    }

    /// Return a reference to the shared [`NonceStore`].
    pub fn nonce_store(&self) -> &Arc<NonceStore> {
        &self.nonce_store
    }

    /// Return a reference to the [`PrepareLimiter`].
    pub fn prepare_limiter(&self) -> &Arc<PrepareLimiter> {
        &self.prepare_limiter
    }

    /// Return a reference to the [`UploadLimiter`].
    pub fn upload_limiter(&self) -> &Arc<UploadLimiter> {
        &self.upload_limiter
    }

    /// Emit a structured diagnostic [`info!`] event for an access request.
    ///
    /// Fields follow a fixed schema so log aggregators can index them:
    /// - `peer` — requesting peer (short form)
    /// - `shared_file_id` — first 16 characters of the file id (privacy-safe prefix)
    /// - `result` — the overall outcome (`"Granted"`, `"PermissionDenied"`, etc.)
    /// - `error_category` — high-level grouping (`"none"`, `"permission"`, `"availability"`, etc.)
    /// - `version_ok` — `true`/`false` when the version check was reached, absent otherwise
    /// - `prep_ok` — `true`/`false` when preparation ran, absent otherwise
    /// - `descriptor_issued` — `true` only when a [`SignedDownloadDescriptor`] was created
    ///
    /// No descriptor secrets or local filesystem paths are ever included.
    fn access_diag(
        peer: &iroh::PublicKey,
        shared_file_id: &str,
        result: &'static str,
        error_category: &'static str,
        version_ok: Option<bool>,
        prep_ok: Option<bool>,
        descriptor_issued: bool,
    ) {
        let prefix = &shared_file_id[..shared_file_id.len().min(16)];
        info!(
            peer = %peer.fmt_short(),
            shared_file_id = %prefix,
            result,
            error_category,
            version_ok,
            prep_ok,
            descriptor_issued,
            "file-access: access request",
        );
    }

    /// Perform a request-time permission, availability, and integrity check.
    ///
    /// This is the core access-control function.  It checks **everything**
    /// against current database state — no cached catalogue data is trusted.
    ///
    /// Returns the appropriate [`FileAccessResponse`] variant:
    /// - `Granted(...)` — all checks pass, a short-lived download descriptor
    ///   is returned.
    /// - `UnsupportedVersion` — wire or inner-request version not supported.
    /// - `InvalidRequest` / `NotFound` — structural problems.
    /// - `PermissionDenied` — requester is blocked or lacks explicit grant.
    /// - `Disabled` — the file offer is disabled.
    /// - `Unavailable` — the file object is no longer available locally.
    /// - `Changed` — the content_hash has changed since the requester's
    ///   catalogue was fetched.
    async fn check_permission(
        &self,
        requester: &PublicKey,
        request: &FileAccessRequest,
    ) -> FileAccessResponse {
        let requester_id = FriendId::from_public_key(*requester);

        // ── 1. Structural validation ──────────────────────────────────
        if let Err((code, _msg)) = request.validate() {
            Self::access_diag(
                requester,
                &request.shared_file_id,
                "InvalidRequest",
                "invalid",
                None,
                None,
                false,
            );
            return FileAccessResponse::from(code);
        }

        // ── 2. Look up the shared file by metadata_id ────────────────
        let row = match self
            .storage
            .get_shared_file_by_metadata_id(&self.profile_user_id, &request.shared_file_id)
        {
            Ok(Some(r)) => r,
            Ok(None) => {
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "NotFound",
                    "not_found",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::NotFound;
            }
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "get_shared_file_by_metadata_id: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "NotFound",
                    "internal",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        // ── 3. Offer enabled check ──────────────────────────────────
        if !row.offered {
            Self::access_diag(
                requester,
                &request.shared_file_id,
                "Disabled",
                "disabled",
                None,
                None,
                false,
            );
            return FileAccessResponse::Disabled;
        }

        // ── 4. Blocked check ─────────────────────────────────────────
        if let Some(record) = self.friends.get(&requester_id) {
            if record.relationship == FriendRelationship::Blocked {
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "permission",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::PermissionDenied;
            }
        }

        // Authorization and catalogue-integrity checks must precede local
        // preparation.  A stale catalogue must produce the precise denial or
        // mismatch response even when the local object is unavailable.
        let permissions = match self
            .storage
            .list_permissions_for_grantee(requester_id.as_str())
        {
            Ok(p) => p,
            Err(_) => return FileAccessResponse::from(FileAccessErrorCode::InternalError),
        };
        let mut explicitly_granted = false;
        for perm in &permissions {
            if perm.grantor_user_id == self.profile_user_id && perm.content_hash == row.content_hash
            {
                match perm.permission.as_str() {
                    "deny" => return FileAccessResponse::PermissionDenied,
                    "read" => explicitly_granted = true,
                    _ => {}
                }
            }
        }
        let has_any_read_grants = match self
            .storage
            .count_read_grants_for_file(&row.content_hash, &self.profile_user_id)
        {
            Ok(n) => n > 0,
            Err(_) => return FileAccessResponse::from(FileAccessErrorCode::InternalError),
        };
        if (has_any_read_grants && !explicitly_granted)
            || (!has_any_read_grants
                && !self
                    .friends
                    .get(&requester_id)
                    .is_some_and(|r| r.relationship == FriendRelationship::Friends))
        {
            return FileAccessResponse::PermissionDenied;
        }

        let expected_hex = hex::encode(request.expected_content_hash);
        if expected_hex != row.content_hash {
            return FileAccessResponse::Changed;
        }
        if request.expected_version != 0 && request.expected_version != row.updated_at_ms {
            return FileAccessResponse::VersionMismatch {
                current_version: row.updated_at_ms,
            };
        }

        // ── 5. Availability check ─────────────────────────────────────
        // Determine whether this is a referenced file (has source_path)
        // and call the appropriate preparation function, bounded by
        // the prepare limiter (concurrency, size, timeout).
        let file_obj = match self.storage.get_file_object(&row.content_hash) {
            Ok(Some(fo)) => fo,
            Ok(None) => {
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "availability",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::Unavailable;
            }
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "get_file_object: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "internal",
                    None,
                    None,
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        // ── 5a. Apply preparation bounds ──────────────────────────
        let permit = match self.prepare_limiter.try_begin(file_obj.size) {
            Ok(p) => p,
            Err(PrepareError::Busy) => {
                warn!(
                    peer = %requester.fmt_short(),
                    "file preparation busy — max_concurrent_preparations reached (size={})",
                    file_obj.size,
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Busy",
                    "busy",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::Busy;
            }
            Err(PrepareError::TooLarge {
                size_bytes,
                max_bytes,
            }) => {
                warn!(
                    peer = %requester.fmt_short(),
                    "file too large for preparation: {size_bytes} > {max_bytes}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "availability",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::Unavailable;
            }
            Err(PrepareError::Timeout { .. }) => {
                // Should not happen from try_begin (timeout is applied
                // during the actual async call), but handle defensively.
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "availability",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::Unavailable;
            }
        };

        // ── 5b. Run bounded preparation with timeout ──────────────
        let storage = self.storage.clone();
        let blob_store = self.blob_store.clone();
        let content_hash = row.content_hash.clone();
        let is_referenced = file_obj.source_path.is_some();

        let bounded_prepare = async move {
            if is_referenced {
                prepare_referenced_file(
                    &storage,
                    &blob_store,
                    &content_hash,
                    None, // verify_hash — deferred to descriptor verification
                    None, // verify_size — deferred to descriptor verification
                )
                .await
            } else {
                prepare_imported_file(
                    &storage,
                    &blob_store,
                    &content_hash,
                    None, // verify_hash — deferred to descriptor verification
                    None, // verify_size — deferred to descriptor verification
                )
                .await
            }
        };

        // The permit is moved into the timeout future so it stays
        // alive (holding its semaphore slot) for the duration.
        let prepare_result = tokio::time::timeout(permit.timeout(), bounded_prepare).await;

        let prepare_result = match prepare_result {
            Ok(res) => res,
            Err(_elapsed) => {
                warn!(
                    peer = %requester.fmt_short(),
                    "file preparation timed out (size={})",
                    file_obj.size,
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "availability",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::Unavailable;
            }
        };

        match prepare_result {
            Ok(_prepared) => {}
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("not found") || msg.contains("missing") {
                    Self::access_diag(
                        requester,
                        &request.shared_file_id,
                        "Unavailable",
                        "availability",
                        None,
                        Some(false),
                        false,
                    );
                    return FileAccessResponse::Unavailable;
                }
                error!(
                    peer = %requester.fmt_short(),
                    "file preparation: {msg}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "Unavailable",
                    "internal",
                    None,
                    Some(false),
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        }

        // ── 6. Explicit denial check ──────────────────────────────────
        let permissions = match self
            .storage
            .list_permissions_for_grantee(requester_id.as_str())
        {
            Ok(p) => p,
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "list_permissions_for_grantee: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "internal",
                    None,
                    Some(true),
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        let mut explicitly_granted = false;
        for perm in &permissions {
            if perm.grantor_user_id == self.profile_user_id && perm.content_hash == row.content_hash
            {
                match perm.permission.as_str() {
                    "deny" => {
                        Self::access_diag(
                            requester,
                            &request.shared_file_id,
                            "PermissionDenied",
                            "permission",
                            None,
                            Some(true),
                            false,
                        );
                        return FileAccessResponse::PermissionDenied;
                    }
                    "read" => explicitly_granted = true,
                    _ => {}
                }
            }
        }

        // ── 7. Visibility / permission mode check ─────────────────────
        let has_any_read_grants = match self
            .storage
            .count_read_grants_for_file(&row.content_hash, &self.profile_user_id)
        {
            Ok(n) => n > 0,
            Err(e) => {
                error!(
                    peer = %requester.fmt_short(),
                    "count_read_grants_for_file: {e:#}"
                );
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "internal",
                    None,
                    Some(true),
                    false,
                );
                return FileAccessResponse::from(FileAccessErrorCode::InternalError);
            }
        };

        if has_any_read_grants {
            // Selected-peers mode: requester must have an explicit read grant.
            if !explicitly_granted {
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "permission",
                    None,
                    Some(true),
                    false,
                );
                return FileAccessResponse::PermissionDenied;
            }
        } else {
            // Contacts-only mode: requester must be a friend.
            let is_friend = self
                .friends
                .get(&requester_id)
                .is_some_and(|r| r.relationship == FriendRelationship::Friends);
            if !is_friend {
                Self::access_diag(
                    requester,
                    &request.shared_file_id,
                    "PermissionDenied",
                    "permission",
                    None,
                    Some(true),
                    false,
                );
                return FileAccessResponse::PermissionDenied;
            }
        }

        // ── 8. Content hash integrity check ──────────────────────────
        // Convert the expected (raw [u8; 32]) to hex for comparison with
        // the stored hex string.  A mismatch means the file content
        // changed since the requester fetched their catalogue.
        let expected_hex = hex::encode(request.expected_content_hash);
        if expected_hex != row.content_hash {
            Self::access_diag(
                requester,
                &request.shared_file_id,
                "Changed",
                "integrity",
                None,
                Some(true),
                false,
            );
            return FileAccessResponse::Changed;
        }

        // ── 9. Version check ──────────────────────────────────────────
        // FIXME: The `shared_files` table does not currently track a
        // monotonically-increasing version number.  Until a `version`
        // column is added and bumped on every file metadata change, the
        // version check uses `updated_at_ms` as a heuristic proxy.  The
        // content-hash check above (step 8) already catches the important
        // case of file content changing — this version check is an
        // additional guard against metadata-only changes (description,
        // display name, collection membership).
        //
        // Once `shared_files.version` is added, replace this with a direct
        // comparison against the database version column.
        if request.expected_version != row.updated_at_ms {
            Self::access_diag(
                requester,
                &request.shared_file_id,
                "VersionMismatch",
                "version",
                Some(true),
                Some(true),
                false,
            );
            return FileAccessResponse::VersionMismatch {
                current_version: row.updated_at_ms,
            };
        }

        // ── 10. All checks pass — issue download descriptor ──────────
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let expires_at_ms = now_ms + DOWNLOAD_DESCRIPTOR_TTL.as_millis() as u64;

        // Look up file size from the file_objects table.
        let size_bytes = match self.storage.get_file_object(&row.content_hash) {
            Ok(Some(fo)) => fo.size,
            _ => {
                // Should not happen since we checked existence above, but
                // fall back to 0 rather than blocking the download.
                0
            }
        };

        // Convert the hex content_hash to raw 32 bytes for the descriptor.
        let mut raw_hash = [0u8; 32];
        let hash_bytes = hex::decode(&row.content_hash).unwrap_or_default();
        let copy_len = hash_bytes.len().min(32);
        raw_hash[..copy_len].copy_from_slice(&hash_bytes[..copy_len]);

        let descriptor = sign_download_descriptor(
            &self.secret_key,
            *requester,
            request.shared_file_id.clone(),
            raw_hash,
            size_bytes,
            BlobFormat::Raw,
            now_ms,
            expires_at_ms,
        );

        Self::access_diag(
            requester,
            &request.shared_file_id,
            "Granted",
            "none",
            Some(true),
            Some(true),
            true,
        );
        FileAccessResponse::Granted(Box::new(descriptor))
    }
}

impl ProtocolHandler for FileAccessHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();
        debug!(
            peer = %remote_id.fmt_short(),
            "file-access: incoming connection"
        );

        let timeout = self.upload_limiter.config().request_timeout;
        match tokio::time::timeout(timeout, serve_file_access(&connection, self)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(
                    peer = %remote_id.fmt_short(),
                    "file-access: serve error: {e:#}"
                );
            }
            Err(_elapsed) => {
                warn!(
                    peer = %remote_id.fmt_short(),
                    "file-access: handler timeout after {timeout:?}"
                );
            }
        }

        // Keep the connection alive until the client finishes reading.
        let _ = connection.closed().await;
        Ok(())
    }
}

/// Serve a single file-access request on an already-accepted connection.
async fn serve_file_access(
    connection: &Connection,
    handler: &FileAccessHandler,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let remote_id = connection.remote_id();
    let peer_str = remote_id.fmt_short().to_string();

    // Accept the bi-directional stream opened by the client.
    let (mut send, mut recv) = connection.accept_bi().await?;

    // ── 1. Apply queue-admission limit ───────────────────────────────
    let upload_permit = match handler.upload_limiter.try_enqueue(&peer_str) {
        Ok(p) => p,
        Err(UploadError::QueueFull) => {
            warn!(
                peer = %remote_id.fmt_short(),
                "file-access: upload queue full"
            );
            let resp = FileAccessWireResponse::error(FileAccessErrorCode::Busy);
            let bytes = postcard::to_stdvec(&resp)?;
            send.write_all(&bytes).await?;
            send.finish()?;
            return Ok(());
        }
        Err(UploadError::PeerLimitReached) => {
            warn!(
                peer = %remote_id.fmt_short(),
                "file-access: per-peer upload limit reached"
            );
            let resp = FileAccessWireResponse::error(FileAccessErrorCode::RateLimited);
            let bytes = postcard::to_stdvec(&resp)?;
            send.write_all(&bytes).await?;
            send.finish()?;
            return Ok(());
        }
        Err(UploadError::VerificationBusy) => {
            // Not expected from try_enqueue, but handle defensively.
            let resp = FileAccessWireResponse::error(FileAccessErrorCode::Busy);
            let bytes = postcard::to_stdvec(&resp)?;
            send.write_all(&bytes).await?;
            send.finish()?;
            return Ok(());
        }
    };

    // Read the full request payload (max 256 KiB).
    let payload = recv.read_to_end(256 * 1024).await?;

    if payload.is_empty() {
        // Clean end-of-stream — nothing to do.
        return Ok(());
    }

    // Deserialise the versioned wire request.
    let wire_req: FileAccessWireRequest = match postcard::from_bytes(&payload) {
        Ok(req) => req,
        Err(e) => {
            warn!(
                peer = %remote_id.fmt_short(),
                "file-access: deserialisation failed: {e:#}"
            );
            let resp = FileAccessWireResponse::error(FileAccessErrorCode::InvalidRequest);
            let bytes = postcard::to_stdvec(&resp)?;
            send.write_all(&bytes).await?;
            send.finish()?;
            return Ok(());
        }
    };

    // Validate wire version.
    if let Err(code) = wire_req.validate_version() {
        let resp = FileAccessWireResponse::error(code);
        let bytes = postcard::to_stdvec(&resp)?;
        send.write_all(&bytes).await?;
        send.finish()?;
        return Ok(());
    }

    // Validate inner request version.
    if let Err(code) = wire_req.inner.validate_request_version() {
        let resp = FileAccessWireResponse::error(code);
        let bytes = postcard::to_stdvec(&resp)?;
        send.write_all(&bytes).await?;
        send.finish()?;
        return Ok(());
    }

    // ── 2. Promote from queue to active (acquire global + per-peer slots) ─
    let _active = match upload_permit.start().await {
        Ok(a) => a,
        Err(_) => {
            // Queue slot was already released if start() fails; tell
            // the client the server is busy.
            let resp = FileAccessWireResponse::error(FileAccessErrorCode::Busy);
            let bytes = postcard::to_stdvec(&resp)?;
            send.write_all(&bytes).await?;
            send.finish()?;
            return Ok(());
        }
    };

    // ── 3. Acquire verification budget ───────────────────────────────
    let _verification = match handler.upload_limiter.try_acquire_verification() {
        Ok(permit) => Some(permit),
        Err(UploadError::VerificationBusy) => {
            warn!(
                peer = %remote_id.fmt_short(),
                "file-access: verification concurrency limit reached"
            );
            let resp = FileAccessWireResponse::error(FileAccessErrorCode::RateLimited);
            let bytes = postcard::to_stdvec(&resp)?;
            send.write_all(&bytes).await?;
            send.finish()?;
            return Ok(());
        }
        _ => {
            // Unexpected error from try_acquire — treat as busy.
            let resp = FileAccessWireResponse::error(FileAccessErrorCode::Busy);
            let bytes = postcard::to_stdvec(&resp)?;
            send.write_all(&bytes).await?;
            send.finish()?;
            return Ok(());
        }
    };

    // Perform the request-time permission check.
    let response = handler.check_permission(&remote_id, &wire_req.inner).await;

    // Serialise and send the response.
    let wire_resp = FileAccessWireResponse::success(response);
    let resp_bytes = postcard::to_stdvec(&wire_resp)?;
    send.write_all(&resp_bytes).await?;
    send.finish()?;

    Ok(())
}

// ── Helper: map FileAccessErrorCode to FileAccessResponse ─────────────────

impl From<FileAccessErrorCode> for FileAccessResponse {
    fn from(code: FileAccessErrorCode) -> Self {
        match code {
            FileAccessErrorCode::UnsupportedVersion => FileAccessResponse::UnsupportedVersion,
            FileAccessErrorCode::PermissionDenied => FileAccessResponse::PermissionDenied,
            FileAccessErrorCode::NotFound => FileAccessResponse::NotFound,
            FileAccessErrorCode::InvalidRequest => FileAccessResponse::NotFound,
            FileAccessErrorCode::RateLimited => FileAccessResponse::RateLimited,
            FileAccessErrorCode::Busy => FileAccessResponse::Busy,
            FileAccessErrorCode::ResponseTooLarge => FileAccessResponse::Unavailable,
            FileAccessErrorCode::InternalError => FileAccessResponse::Unavailable,
        }
    }
}

/// Prepare an imported file for transfer — confirm managed bytes exist,
/// optionally verify content integrity, and return safe transfer metadata.
///
/// # Steps
///
/// 1. **Look up** the file object by `content_hash` in local storage.
/// 2. **Import check** — if the file has a `blob_hash` (imported from a
///    remote peer), confirm the blob exists in the `blob_store`.  If the
///    file has inline `data`, import those bytes into the blob store so
///    the file can be served by hash.
/// 3. **Optional verification** — if `verify_hash` and/or `verify_size`
///    are provided, verify that the stored metadata matches.
/// 4. **Return** a [`PreparedFile`] with safe metadata (no local paths).
///
/// # Errors
///
/// Returns an error if the file object is not found in the database, if
/// the blob store does not contain the expected blob, or if optional
/// hash/size verification fails.
pub async fn prepare_imported_file(
    storage: &Storage,
    blob_store: &iroh_blobs::api::Store,
    content_hash: &str,
    verify_hash: Option<&str>,
    verify_size: Option<u64>,
) -> Result<PreparedFile, anyhow::Error> {
    // ── 1. Look up the file object ───────────────────────────────────
    let file_obj = storage
        .get_file_object(content_hash)
        .map_err(|e| anyhow::anyhow!("db lookup failed: {e:#}"))?
        .ok_or_else(|| anyhow::anyhow!("file not found: {content_hash}"))?;

    // ── 2. Check / import blob availability ─────────────────────────
    // Check if the file has a blob_hash (imported from a remote peer).
    let blob_hash_str: Option<String> = storage
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT blob_hash FROM file_objects \
                     WHERE content_hash = ?1 AND blob_hash IS NOT NULL",
                )
                .map_err(|e| anyhow::anyhow!("prepare blob_hash query: {e}"))?;
            let result: Option<String> =
                stmt.query_row(params![content_hash], |row| row.get(0)).ok();
            Ok(result)
        })
        .unwrap_or(None);

    if let Some(ref hash_str) = blob_hash_str {
        // Imported file — confirm the blob exists in the iroh-blobs store.
        let blob_hash: iroh_blobs::Hash = hash_str
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid blob hash {hash_str}: {e}"))?;

        let blob_present = blob_store
            .blobs()
            .has(blob_hash)
            .await
            .map_err(|e| anyhow::anyhow!("blob_store.has failed: {e:#}"))?;

        if !blob_present {
            return Err(anyhow::anyhow!(
                "imported blob missing from store: {hash_str}"
            ));
        }
    } else if let Some(ref data) = file_obj.data {
        // Inline file — import into blob store if not already present.
        let hash_for_inline = blake3::hash(data);
        let blob_hash = iroh_blobs::Hash::from(hash_for_inline);
        let already_present = blob_store
            .blobs()
            .has(blob_hash)
            .await
            .map_err(|e| anyhow::anyhow!("blob_store.has failed: {e:#}"))?;
        if !already_present {
            let progress = blob_store.blobs().add_slice(data);
            let _tag = progress
                .await
                .map_err(|e| anyhow::anyhow!("add_slice failed: {e:#}"))?;
        }
    } else {
        // File exists in DB but has neither blob_hash nor inline data.
        return Err(anyhow::anyhow!(
            "file object {content_hash} has no data and no blob hash"
        ));
    }

    // ── 3. Optional verification ─────────────────────────────────────
    if let Some(expected_size) = verify_size {
        if file_obj.size != expected_size {
            return Err(anyhow::anyhow!(
                "size mismatch: expected {expected_size}, got {}",
                file_obj.size
            ));
        }
    }

    if let Some(expected_hash) = verify_hash {
        let expected = expected_hash.to_ascii_lowercase();
        if file_obj.content_hash != expected {
            return Err(anyhow::anyhow!(
                "hash mismatch: expected {expected}, got {}",
                file_obj.content_hash
            ));
        }
    }

    // ── 4. Return safe transfer metadata ────────────────────────────
    // Determine blob format — imported files are always Raw; inline
    // files are also Raw by default.
    let blob_format = BlobFormat::Raw;

    Ok(PreparedFile {
        content_hash: file_obj.content_hash,
        size_bytes: file_obj.size,
        blob_format,
        mime_type: file_obj.mime_type,
        filename: file_obj.filename,
    })
}

/// Prepare a referenced file for transfer — confirm the source still
/// exists on the local filesystem, verify integrity, and import it into
/// the iroh-blobs store so it can be served by hash.
///
/// # Steps
///
/// 1. **Look up** the file object by `content_hash` in local storage.
/// 2. **Path validation** — confirm the `source_path` exists, is a
///    regular file (not a directory, not a symlink), and has the
///    expected size.
/// 3. **Content verification** — read the file from disk and verify
///    its blake3 content hash matches the expected value.
/// 4. **Import** — add the file data to the iroh-blobs store so it can
///    be served by content hash.
/// 5. **Return** a [`PreparedFile`] with safe metadata (no local paths).
///
/// # Errors
///
/// Returns an error if the file object does not exist in the database,
/// has no `source_path` field, the source file is missing or has been
/// replaced (by a directory, symlink, or different content), or the
/// file cannot be read.
pub async fn prepare_referenced_file(
    storage: &Storage,
    blob_store: &iroh_blobs::api::Store,
    content_hash: &str,
    verify_hash: Option<&str>,
    verify_size: Option<u64>,
) -> Result<PreparedFile, anyhow::Error> {
    use std::fs;
    use std::io;
    use std::path::Path;

    // ── 1. Look up the file object ───────────────────────────────────
    let file_obj = storage
        .get_file_object(content_hash)
        .map_err(|e| anyhow::anyhow!("db lookup failed: {e:#}"))?
        .ok_or_else(|| anyhow::anyhow!("file not found: {content_hash}"))?;

    // ── 2. Get and validate the source path ──────────────────────────
    let src = file_obj
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("file object {content_hash} has no source path"))?
        .to_owned();

    let path = Path::new(&src);

    // Confirm the path exists and is a regular file (not a directory,
    // not a symlink).
    let metadata = fs::symlink_metadata(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            anyhow::anyhow!("referenced source file not found: {src}")
        } else {
            anyhow::anyhow!("failed to stat referenced source {src}: {e:#}")
        }
    })?;

    if metadata.is_dir() {
        return Err(anyhow::anyhow!(
            "referenced source is a directory, not a regular file: {src}"
        ));
    }

    if metadata.file_type().is_symlink() {
        return Err(anyhow::anyhow!(
            "referenced source is a symlink, not a regular file: {src}"
        ));
    }

    // ── 3. Size verification ─────────────────────────────────────────
    if let Some(expected_size) = verify_size {
        let actual_size = metadata.len();
        if actual_size != expected_size {
            return Err(anyhow::anyhow!(
                "size mismatch: expected {expected_size}, got {actual_size}"
            ));
        }
    }

    // ── 4. Read and hash the file content ────────────────────────────
    let file_data = fs::read(path).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            anyhow::anyhow!("permission denied reading referenced source: {src}")
        } else {
            anyhow::anyhow!("failed to read referenced source {src}: {e:#}")
        }
    })?;

    // Optional hash verification.
    if let Some(expected_hash) = verify_hash {
        let actual_hash = blake3::hash(&file_data);
        let actual_hex = hex::encode(actual_hash.as_bytes());
        let expected = expected_hash.to_ascii_lowercase();
        if actual_hex != expected {
            return Err(anyhow::anyhow!(
                "hash mismatch: expected {expected}, got {actual_hex}"
            ));
        }
    }

    // ── 5. Import into iroh-blobs store ─────────────────────────────
    // Check if already present to avoid re-import.
    let file_blake3 = blake3::hash(&file_data);
    let blob_hash = iroh_blobs::Hash::from(file_blake3);
    let already_present = blob_store
        .blobs()
        .has(blob_hash)
        .await
        .map_err(|e| anyhow::anyhow!("blob_store.has failed: {e:#}"))?;

    if !already_present {
        let progress = blob_store.blobs().add_slice(&file_data);
        let _tag = progress
            .await
            .map_err(|e| anyhow::anyhow!("add_slice failed: {e:#}"))?;
    }

    // ── 6. Return safe transfer metadata ─────────────────────────────
    Ok(PreparedFile {
        content_hash: file_obj.content_hash,
        size_bytes: file_data.len() as u64,
        blob_format: BlobFormat::Raw,
        mime_type: file_obj.mime_type,
        filename: file_obj.filename,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use crate::friends::FriendRecord;

    /// Helper to build a minimal in-memory storage with a shared file.
    fn setup_storage_with_file(
        metadata_id: &str,
        content_hash_hex: &str,
        offered: bool,
    ) -> (Arc<Storage>, FriendsStore) {
        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        let profile_user_id = "owner-profile-id";

        // Insert a file object (put_file_object stores inline data).
        storage
            .put_file_object(
                content_hash_hex,
                1024,
                "text/plain",
                "test.txt",
                b"hello world",
            )
            .expect("put file object");

        // Insert the shared file offer.
        storage
            .upsert_shared_file(
                content_hash_hex,
                profile_user_id,
                metadata_id,
                "Test File",
                None,
                offered,
            )
            .expect("upsert shared file");

        // Friends store (empty = default contacts-only mode).
        let friends = FriendsStore::default();

        (storage, friends)
    }

    /// Helper to build a [`FileAccessHandler`] for testing.
    fn test_handler(
        storage: Arc<Storage>,
        friends: FriendsStore,
    ) -> (FileAccessHandler, Arc<iroh_blobs::api::Store>) {
        let secret_key = iroh::SecretKey::generate();
        let profile_user_id = "owner-profile-id".to_string();
        let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());
        let handler = FileAccessHandler::new(
            storage,
            secret_key,
            profile_user_id,
            friends,
            Arc::new(NonceStore::new()),
            Arc::clone(&blob_store),
        );
        (handler, blob_store)
    }

    /// Helper to build a [`FileAccessHandler`] with a custom [`PrepareLimiter`].
    fn test_handler_with_limiter(
        storage: Arc<Storage>,
        friends: FriendsStore,
        prepare_limiter: Arc<PrepareLimiter>,
    ) -> (FileAccessHandler, Arc<iroh_blobs::api::Store>) {
        let secret_key = iroh::SecretKey::generate();
        let profile_user_id = "owner-profile-id".to_string();
        let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());
        let handler = FileAccessHandler::with_limiters(
            storage,
            secret_key,
            profile_user_id,
            friends,
            Arc::new(NonceStore::new()),
            Arc::clone(&blob_store),
            prepare_limiter,
            Arc::new(UploadLimiter::new(UploadLimitsConfig::default())),
        );
        (handler, blob_store)
    }

    /// Create a test requester `PublicKey`.
    fn requester_pk() -> PublicKey {
        iroh::SecretKey::generate().public()
    }

    /// Helper to create a valid `FileAccessRequest` with the given parameters.
    fn make_request(
        shared_file_id: &str,
        content_hash_hex: &str,
        expected_version: u64,
    ) -> FileAccessRequest {
        let raw_hash = hex::decode(content_hash_hex).expect("valid hex");
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&raw_hash);
        FileAccessRequest::new(shared_file_id, arr, expected_version)
    }

    /// Shortcut: make the requester a friend of the profile.
    fn add_friend(handler: &mut FileAccessHandler, pk: PublicKey) {
        handler.friends.upsert(
            FriendId::from_public_key(pk),
            FriendRecord {
                relationship: FriendRelationship::Friends,
                ..FriendRecord::default()
            },
        );
    }

    /// Helper to compute the blake3 hex hash of a byte slice, zero-padded to 64 chars.
    fn hex_hash(data: &[u8]) -> String {
        let hash = blake3::hash(data);
        hex::encode(hash.as_bytes())
    }

    // ── Basic happy path ──────────────────────────────────────────────

    #[tokio::test]
    async fn happy_path_granted() {
        let metadata_id = "file-1";
        let content_hash = "ab".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
        let (mut handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        add_friend(&mut handler, requester);

        // Use the actual updated_at_ms from the DB as the expected version.
        let row = handler
            .storage
            .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
            .expect("get shared file")
            .expect("shared file exists");

        let request = make_request(metadata_id, &content_hash, row.updated_at_ms);
        let response = handler.check_permission(&requester, &request).await;

        assert!(
            matches!(response, FileAccessResponse::Granted(_)),
            "expected Granted, got {response:?}"
        );
    }

    // ── Not found ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn file_not_found() {
        let content_hash = "cd".repeat(32);
        let (storage, friends) = setup_storage_with_file("file-1", &content_hash, true);
        let (handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        // Request a metadata_id that doesn't exist.
        let request = make_request("nonexistent-file", &content_hash, 0);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(response, FileAccessResponse::NotFound);
    }

    // ── Disabled (offer removed) ─────────────────────────────────────

    #[tokio::test]
    async fn file_disabled_after_catalogue_fetch() {
        let metadata_id = "file-1";
        let content_hash = "ef".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, false);
        let (mut handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        add_friend(&mut handler, requester);

        let request = make_request(metadata_id, &content_hash, 0);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(response, FileAccessResponse::Disabled);
    }

    // ── Blocked after catalogue fetch ─────────────────────────────────

    #[tokio::test]
    async fn blocked_after_catalogue_fetch() {
        let metadata_id = "file-1";
        let content_hash = "01".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
        let (mut handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        // Requester is blocked.
        handler.friends.upsert(
            FriendId::from_public_key(requester),
            FriendRecord {
                relationship: FriendRelationship::Blocked,
                ..FriendRecord::default()
            },
        );

        let request = make_request(metadata_id, &content_hash, 0);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(response, FileAccessResponse::PermissionDenied);
    }

    // ── Version changed ───────────────────────────────────────────────

    #[tokio::test]
    async fn version_changed_after_catalogue_fetch() {
        let metadata_id = "file-1";
        let content_hash = "23".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
        let (mut handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        add_friend(&mut handler, requester);

        // Expected version doesn't match updated_at_ms.
        let request = make_request(metadata_id, &content_hash, 999_999_999);
        let response = handler.check_permission(&requester, &request).await;

        assert!(
            matches!(response, FileAccessResponse::VersionMismatch { .. }),
            "expected VersionMismatch, got {response:?}"
        );
    }

    // ── Content hash changed ─────────────────────────────────────────

    #[tokio::test]
    async fn content_hash_changed_after_catalogue_fetch() {
        let metadata_id = "file-1";
        let old_hash = "45".repeat(32);
        let new_hash = "67".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &new_hash, true);
        let (mut handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        add_friend(&mut handler, requester);

        // Requester has old hash, but file has new hash.
        let request = make_request(metadata_id, &old_hash, 0);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(response, FileAccessResponse::Changed);
    }

    // ── Explicit denial ───────────────────────────────────────────────

    #[tokio::test]
    async fn explicit_denial_at_request_time() {
        let metadata_id = "file-1";
        let content_hash = "89".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
        let storage_clone = Arc::clone(&storage);
        let (mut handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        add_friend(&mut handler, requester);

        // Add an explicit denial for this requester.
        storage_clone
            .grant_permission(
                &content_hash,
                "owner-profile-id",
                &requester.to_string(),
                "deny",
                None,
            )
            .expect("add denial");

        let request = make_request(metadata_id, &content_hash, 0);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(response, FileAccessResponse::PermissionDenied);
    }

    // ── Permission revoked after catalogue fetch (selected-peers mode) ─

    #[tokio::test]
    async fn permission_revoked_after_catalogue_fetch() {
        let metadata_id = "file-1";
        let content_hash = "ab".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
        let storage_clone = Arc::clone(&storage);
        let (handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        // File has an explicit read grant for another peer → selected-peers mode.
        storage_clone
            .grant_permission(
                &content_hash,
                "owner-profile-id",
                "other-peer",
                "read",
                None,
            )
            .expect("add grant for other peer");

        // Requester is NOT in the read grants → should be denied.
        let request = make_request(metadata_id, &content_hash, 0);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(response, FileAccessResponse::PermissionDenied);
    }

    // ── Not friend in contacts-only mode ──────────────────────────────

    #[tokio::test]
    async fn not_friend_in_contacts_only_mode() {
        let metadata_id = "file-1";
        let content_hash = "cd".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
        let (handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        // No relationship established — requester is not a friend.
        let request = make_request(metadata_id, &content_hash, 0);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(response, FileAccessResponse::PermissionDenied);
    }

    // ── Source changed / new file at same metadata_id ────────────────

    #[tokio::test]
    async fn source_changed_after_catalogue_fetch() {
        let metadata_id = "file-1";
        let old_hash = "01".repeat(32);
        let new_hash = "23".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &new_hash, true);
        let (mut handler, _blob_store) = test_handler(storage, friends);
        let requester = requester_pk();

        add_friend(&mut handler, requester);

        // Requester has the old content hash in their catalogue.
        let request = make_request(metadata_id, &old_hash, 0);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(response, FileAccessResponse::Changed);
    }

    // ── NonceStore tests (unchanged — still sync) ────────────────────

    #[test]
    fn nonce_store_accepts_new_nonce() {
        let store = NonceStore::new();
        let nonce = [0x01; 32];
        let result = store.check_and_mark(nonce, 2_000_000, 1_000_000);
        assert_eq!(result, NonceCheck::Accepted);
    }

    #[test]
    fn nonce_store_rejects_replayed_nonce() {
        let store = NonceStore::new();
        let nonce = [0xAA; 32];

        // First use — accepted.
        assert_eq!(
            store.check_and_mark(nonce, 2_000_000, 1_000_000),
            NonceCheck::Accepted,
        );

        // Second use with the same nonce — replayed.
        assert_eq!(
            store.check_and_mark(nonce, 2_000_000, 1_000_000),
            NonceCheck::Replayed,
        );
    }

    #[test]
    fn nonce_store_accepts_different_nonces() {
        let store = NonceStore::new();

        assert_eq!(
            store.check_and_mark([0x01; 32], 2_000_000, 1_000_000),
            NonceCheck::Accepted,
        );
        assert_eq!(
            store.check_and_mark([0x02; 32], 2_000_000, 1_000_000),
            NonceCheck::Accepted,
        );
        assert_eq!(
            store.check_and_mark([0x03; 32], 2_000_000, 1_000_000),
            NonceCheck::Accepted,
        );
    }

    #[test]
    fn nonce_store_accepts_nonce_after_expiry() {
        let store = NonceStore::new();
        let nonce = [0xBB; 32];

        // Mark at T=1000, expires at 2000.
        assert_eq!(
            store.check_and_mark(nonce, 2_000_000, 1_000_000),
            NonceCheck::Accepted,
        );

        // After expiry (T=3000), the same nonce can be used again.
        assert_eq!(
            store.check_and_mark(nonce, 4_000_000, 3_000_000),
            NonceCheck::Accepted,
        );
    }

    #[test]
    fn nonce_store_len_and_is_empty() {
        let store = NonceStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store.check_and_mark([0xCC; 32], 2_000_000, 1_000_000);
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
    }

    #[test]
    fn nonce_store_evicts_expired_on_access() {
        let store = NonceStore::new();

        // Insert a nonce that expires at T=2000.
        store.check_and_mark([0xDD; 32], 2_000_000, 1_000_000);
        assert_eq!(store.len(), 1);

        // Reading at T=3000 triggers lazy eviction.
        let result = store.check(&[0xDD; 32], 3_000_000);
        assert_eq!(result, NonceCheck::Accepted); // expired → treated as new
        assert_eq!(store.len(), 0); // evicted
    }

    #[test]
    fn nonce_store_check_does_not_mark() {
        let store = NonceStore::new();
        let nonce = [0xEE; 32];

        // A read-only check returns Accepted for an unseen nonce.
        assert_eq!(store.check(&nonce, 1_000_000), NonceCheck::Accepted);
        // The nonce is NOT marked — a subsequent check_and_mark should
        // still see it as new.
        assert_eq!(
            store.check_and_mark(nonce, 2_000_000, 1_000_000),
            NonceCheck::Accepted,
        );
    }

    // ── prepare_imported_file tests ──────────────────────────────────

    #[tokio::test]
    async fn prepare_valid_imported_object() {
        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        let data = b"hello world";
        let hex_hash = hex_hash(data);

        // Insert an imported file object with a blob_hash.
        // First add the data to the blob store so the blob exists.
        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());
        let progress = blob_store.blobs().add_slice(data);
        let blob_hash_str = progress.await.expect("add_slice").hash.to_string();

        storage
            .put_imported_file_object(
                &hex_hash,
                11,
                "text/plain",
                "hello.txt",
                &blob_hash_str,
                "sourc3r",
            )
            .expect("put imported file object");

        let prepared =
            prepare_imported_file(&storage, &blob_store, &hex_hash, Some(&hex_hash), Some(11))
                .await
                .expect("prepare imported file");

        assert_eq!(prepared.content_hash, hex_hash);
        assert_eq!(prepared.size_bytes, 11);
        assert_eq!(prepared.mime_type, "text/plain");
        assert_eq!(prepared.filename, "hello.txt");
        assert_eq!(prepared.blob_format, BlobFormat::Raw);
    }

    #[tokio::test]
    async fn prepare_missing_imported_object() {
        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        // A hash string that parses but does not exist in the store.
        let fake_hash = iroh_blobs::Hash::from([0xAAu8; 32]);
        let hash_str = fake_hash.to_string();

        // Insert an imported file object with a blob_hash that does NOT
        // exist in the blob store.
        storage
            .put_imported_file_object(
                "aa".repeat(32).as_str(),
                100,
                "text/plain",
                "missing.txt",
                &hash_str,
                "peer1",
            )
            .expect("put imported file object");

        let result =
            prepare_imported_file(&storage, &blob_store, &"aa".repeat(32), None, None).await;

        assert!(result.is_err(), "expected error for missing blob");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing"),
            "error should mention missing blob, got: {err}"
        );
    }

    #[tokio::test]
    async fn prepare_inline_file_imports_into_blob_store() {
        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        let data = b"inline file data";
        let hex_hash = hex_hash(data);

        // Insert an inline file object (no blob_hash).
        storage
            .put_file_object(
                &hex_hash,
                data.len() as u64,
                "text/plain",
                "inline.txt",
                data,
            )
            .expect("put inline file object");

        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        // Before prepare_imported_file, the blob store should NOT have
        // this blob yet. Use the blake3 hash from BLAKE3 (not the content
        // hex, the actual blob hash).
        let raw_blake3 = blake3::hash(data);
        let blob_hash = iroh_blobs::Hash::from(raw_blake3);
        let exists_before = blob_store.blobs().has(blob_hash).await.unwrap();
        assert!(!exists_before, "blob should not exist yet");

        // Prepare should import the inline data into the blob store.
        let prepared = prepare_imported_file(&storage, &blob_store, &hex_hash, None, None)
            .await
            .expect("prepare inline file");

        assert_eq!(prepared.content_hash, hex_hash);
        assert_eq!(prepared.size_bytes, data.len() as u64);

        // After prepare, the blob should exist in the store.
        let exists_after = blob_store.blobs().has(blob_hash).await.unwrap();
        assert!(exists_after, "blob should exist after prepare");
    }

    #[tokio::test]
    async fn prepare_wrong_size_is_rejected() {
        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        let data = b"some content";
        let hex_hash = hex_hash(data);

        // Inline file.
        storage
            .put_file_object(&hex_hash, data.len() as u64, "text/plain", "f.txt", data)
            .expect("put file object");

        let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        let result = prepare_imported_file(
            &storage,
            &blob_store,
            &hex_hash,
            None,
            Some(9999), // wrong size
        )
        .await;

        assert!(result.is_err(), "expected error for wrong size");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("size mismatch"), "error: {err}");
    }

    #[tokio::test]
    async fn prepare_wrong_hash_is_rejected() {
        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        let data = b"content for hash check";
        let hex_hash = hex_hash(data); // actual hash

        storage
            .put_file_object(&hex_hash, data.len() as u64, "text/plain", "f.txt", data)
            .expect("put file object");

        let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        // Provide a wrong expected hash.
        let wrong_hash = "ff".repeat(32);
        let result =
            prepare_imported_file(&storage, &blob_store, &hex_hash, Some(&wrong_hash), None).await;

        assert!(result.is_err(), "expected error for wrong hash");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("hash mismatch"), "error: {err}");
    }

    #[tokio::test]
    async fn prepare_nonexistent_file_returns_error() {
        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        let result =
            prepare_imported_file(&storage, &blob_store, "nonexistent-hash", None, None).await;

        assert!(result.is_err(), "expected error for nonexistent file");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "error: {err}");
    }

    // ── Bounded preparation tests ───────────────────────────────────

    #[tokio::test]
    async fn prepare_limiter_rejects_too_large() {
        let config = PrepareConfig {
            max_file_size_bytes: 100,
            ..Default::default()
        };
        let limiter = PrepareLimiter::new(config);

        // File under limit: accepts.
        assert!(limiter.try_begin(50).is_ok());

        // File at limit: accepts.
        assert!(limiter.try_begin(100).is_ok());

        // File over limit: rejects.
        match limiter.try_begin(101).unwrap_err() {
            PrepareError::TooLarge {
                size_bytes,
                max_bytes,
            } => {
                assert_eq!(size_bytes, 101);
                assert_eq!(max_bytes, 100);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prepare_limiter_rejects_when_busy() {
        let config = PrepareConfig {
            max_concurrent_preparations: 2,
            ..Default::default()
        };
        let limiter = PrepareLimiter::new(config);

        let _p1 = limiter.try_begin(100).expect("first permit");
        let _p2 = limiter.try_begin(100).expect("second permit");

        // Third should be Busy.
        match limiter.try_begin(100).unwrap_err() {
            PrepareError::Busy => {}
            other => panic!("expected Busy, got {other:?}"),
        }

        // Drop one permit — now re-usable.
        drop(_p1);
        assert!(limiter.try_begin(100).is_ok());
    }

    #[tokio::test]
    async fn check_permission_returns_busy_when_limiter_exhausted() {
        let config = PrepareConfig {
            max_concurrent_preparations: 1,
            ..Default::default()
        };
        let limiter = Arc::new(PrepareLimiter::new(config));

        // Acquire the single slot before the request arrives.
        let _slot = limiter.try_begin(10).expect("reserve prep slot");

        let metadata_id = "file-1";
        let content_hash = "ab".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
        let (mut handler, _blob_store) = test_handler_with_limiter(storage, friends, limiter);
        let requester = requester_pk();

        add_friend(&mut handler, requester);

        let row = handler
            .storage
            .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
            .expect("get shared file")
            .expect("shared file exists");

        let request = make_request(metadata_id, &content_hash, row.updated_at_ms);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(
            response,
            FileAccessResponse::Busy,
            "expected Busy when prepare limiter exhausted, got {response:?}"
        );
    }

    #[tokio::test]
    async fn check_permission_rejects_file_too_large() {
        let config = PrepareConfig {
            max_file_size_bytes: 1, // only 1 byte allowed
            ..Default::default()
        };
        let limiter = Arc::new(PrepareLimiter::new(config));

        let metadata_id = "file-1";
        let content_hash = "ab".repeat(32);
        let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
        let (mut handler, _blob_store) = test_handler_with_limiter(storage, friends, limiter);
        let requester = requester_pk();

        add_friend(&mut handler, requester);

        let row = handler
            .storage
            .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
            .expect("get shared file")
            .expect("shared file exists");

        let request = make_request(metadata_id, &content_hash, row.updated_at_ms);
        let response = handler.check_permission(&requester, &request).await;

        assert_eq!(
            response,
            FileAccessResponse::Unavailable,
            "expected Unavailable for oversized file, got {response:?}"
        );
    }

    // ── prepare_referenced_file tests ────────────────────────────────

    /// Helper: create a temp file with the given data and return its path
    /// and the hex-encoded blake3 hash.
    fn write_temp_file(
        dir: &std::path::Path,
        name: &str,
        data: &[u8],
    ) -> (std::path::PathBuf, String) {
        let path = dir.join(name);
        std::fs::write(&path, data).expect("write temp file");
        let hash = blake3::hash(data);
        let hex_hash = hex::encode(hash.as_bytes());
        (path, hex_hash)
    }

    #[allow(dead_code)]
    fn hex_hash_for(data: &[u8]) -> String {
        let hash = blake3::hash(data);
        hex::encode(hash.as_bytes())
    }

    #[tokio::test]
    async fn prepare_referenced_unchanged_source() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let data = b"hello referenced file";
        let (_file_path, hex_hash) = write_temp_file(tmp.path(), "source.txt", data);

        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        storage
            .put_file_object(
                &hex_hash,
                data.len() as u64,
                "text/plain",
                "source.txt",
                data,
            )
            .expect("put file object");

        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        let prepared = prepare_referenced_file(
            &storage,
            &blob_store,
            &hex_hash,
            Some(&hex_hash),
            Some(data.len() as u64),
        )
        .await
        .expect("prepare referenced file");

        assert_eq!(prepared.content_hash, hex_hash);
        assert_eq!(prepared.size_bytes, data.len() as u64);
        assert_eq!(prepared.mime_type, "text/plain");
        assert_eq!(prepared.filename, "source.txt");
        assert_eq!(prepared.blob_format, BlobFormat::Raw);

        // Verify the blob was imported into the store.
        let raw_blake3 = blake3::hash(data);
        let blob_hash = iroh_blobs::Hash::from(raw_blake3);
        let exists = blob_store.blobs().has(blob_hash).await.unwrap();
        assert!(exists, "blob should exist after preparation");
    }

    #[tokio::test]
    async fn prepare_referenced_missing_source() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let data = b"will be deleted";
        let (file_path, hex_hash) = write_temp_file(tmp.path(), "delete_me.txt", data);

        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        storage
            .put_file_object(
                &hex_hash,
                data.len() as u64,
                "text/plain",
                "delete_me.txt",
                data,
            )
            .expect("put file object");

        // Delete the source file.
        std::fs::remove_file(&file_path).expect("remove file");

        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        let result = prepare_referenced_file(&storage, &blob_store, &hex_hash, None, None).await;

        assert!(result.is_err(), "expected error for missing source");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "error should mention 'not found', got: {err}"
        );
    }

    #[tokio::test]
    async fn prepare_referenced_changed_content() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let original = b"original content";
        let (file_path, hex_hash) = write_temp_file(tmp.path(), "changed.txt", original);

        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        storage
            .put_file_object(
                &hex_hash,
                original.len() as u64,
                "text/plain",
                "changed.txt",
                original,
            )
            .expect("put file object");

        // Replace the file with different content (same length to avoid
        // catching a size mismatch — we want the hash check to fail).
        let modified = b"MODIFIED CONTENT"; // same length as "original content"
        assert_eq!(modified.len(), original.len(), "same length");
        std::fs::write(&file_path, modified).expect("write modified file");

        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        let result = prepare_referenced_file(
            &storage,
            &blob_store,
            &hex_hash,
            Some(&hex_hash), // expects original hash
            None,
        )
        .await;

        assert!(result.is_err(), "expected error for changed content");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("hash mismatch"),
            "error should mention 'hash mismatch', got: {err}"
        );
    }

    #[tokio::test]
    async fn prepare_referenced_replaced_by_directory() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let data = b"will be replaced by dir";
        let (file_path, hex_hash) = write_temp_file(tmp.path(), "dir_replacement.txt", data);

        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        storage
            .put_file_object(
                &hex_hash,
                data.len() as u64,
                "text/plain",
                "dir_replacement.txt",
                data,
            )
            .expect("put file object");

        // Remove the file and create a directory in its place.
        std::fs::remove_file(&file_path).expect("remove file");
        std::fs::create_dir(&file_path).expect("create dir at same path");

        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        let result = prepare_referenced_file(&storage, &blob_store, &hex_hash, None, None).await;

        assert!(result.is_err(), "expected error for directory replacement");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("directory"),
            "error should mention 'directory', got: {err}"
        );
    }

    #[tokio::test]
    async fn prepare_referenced_replaced_by_symlink() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let data = b"will be replaced by symlink";
        let (file_path, hex_hash) = write_temp_file(tmp.path(), "symlink_target.txt", data);

        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        storage
            .put_file_object(
                &hex_hash,
                data.len() as u64,
                "text/plain",
                "symlink_target.txt",
                data,
            )
            .expect("put file object");

        // Remove the file and create a symlink pointing elsewhere.
        std::fs::remove_file(&file_path).expect("remove file");
        let target = tmp.path().join("other_file.txt");
        std::fs::write(&target, b"other content").expect("write other file");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &file_path).expect("create symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &file_path).expect("create symlink");

        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        let result = prepare_referenced_file(&storage, &blob_store, &hex_hash, None, None).await;

        assert!(result.is_err(), "expected error for symlink replacement");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("symlink"),
            "error should mention 'symlink', got: {err}"
        );
    }

    #[tokio::test]
    async fn prepare_referenced_read_failure() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let data = b"unreadable file";
        let (file_path, hex_hash) = write_temp_file(tmp.path(), "unreadable.txt", data);

        let storage = Arc::new(Storage::memory().expect("in-memory storage"));
        storage
            .put_file_object(
                &hex_hash,
                data.len() as u64,
                "text/plain",
                "unreadable.txt",
                data,
            )
            .expect("put file object");

        let blob_store: Arc<iroh_blobs::api::Store> =
            Arc::new(iroh_blobs::store::mem::MemStore::new().into());

        // Remove read permission from the file.
        let mut perms = std::fs::metadata(&file_path)
            .expect("get metadata")
            .permissions();
        // Clear existing permissions and set to no permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o000); // no permissions at all
        }
        #[cfg(not(unix))]
        {
            perms.set_readonly(false);
        }
        std::fs::set_permissions(&file_path, perms).expect("set permissions");
        // On Linux, the owner can always change permissions back, but the
        // *other* categories are removed.  The test verifies that the
        // function returns an error — the exact error message depends on
        // platform but should relate to reading/permission.

        let result = prepare_referenced_file(&storage, &blob_store, &hex_hash, None, None).await;

        assert!(
            result.is_err(),
            "expected error for unreadable file, got {:?}",
            result
        );
    }

    // ── UploadLimiter unit tests ──────────────────────────────────────

    /// Create a test [`UploadLimiter`] with small, deterministic limits.
    fn upload_limiter() -> UploadLimiter {
        UploadLimiter::new(UploadLimitsConfig {
            max_active_uploads: 2,
            max_uploads_per_peer: 1,
            max_queued_uploads: 2,
            max_concurrent_verifications: 1,
            request_timeout: Duration::from_secs(10),
        })
    }

    #[test]
    fn upload_limiter_admits_and_rejects_on_queue_depth() {
        let limiter = upload_limiter();

        // First permit from peer-a — should succeed.
        let a1 = limiter.try_enqueue("peer-a").expect("first enqueue");
        assert_eq!(a1.peer(), "peer-a");

        // Second permit from peer-b — should succeed (depth is 2).
        let b1 = limiter.try_enqueue("peer-b").expect("second enqueue");

        // Third permit from peer-c — should be QueueFull (depth reached).
        match limiter.try_enqueue("peer-c").unwrap_err() {
            UploadError::QueueFull => {}
            other => panic!("expected QueueFull, got {other:?}"),
        }

        // Dropping a1 frees a queue slot.
        drop(a1);

        // Now peer-c can enqueue.
        let _c1 = limiter.try_enqueue("peer-c").expect("after drop");
        drop(b1);
        drop(_c1);
    }

    #[test]
    fn upload_limiter_per_peer_limit() {
        let limiter = upload_limiter(); // max_uploads_per_peer: 1

        let first = limiter.try_enqueue("alice").expect("first alice");

        // Second from same peer should hit the per-peer limit.
        match limiter.try_enqueue("alice").unwrap_err() {
            UploadError::PeerLimitReached => {}
            other => panic!("expected PeerLimitReached, got {other:?}"),
        }

        // Different peer should succeed.
        let _bob = limiter.try_enqueue("bob").expect("bob enqueues");

        // After dropping alice's first, alice can enqueue again.
        drop(first);
        let _alice2 = limiter.try_enqueue("alice").expect("alice after drop");
        drop(_bob);
        drop(_alice2);
    }

    #[tokio::test]
    async fn upload_limiter_global_active_cap() {
        let limiter = upload_limiter(); // max_active_uploads: 2, per-peer: 1

        // Enqueue and start two from different peers.
        let a = limiter.try_enqueue("a").unwrap().start().await.unwrap();
        let b = limiter.try_enqueue("b").unwrap().start().await.unwrap();

        // Third peer enqueues but start must wait (global cap at 2).
        let c_queued = limiter.try_enqueue("c").unwrap();
        let waiter = tokio::spawn(async move { c_queued.start().await.unwrap() });
        assert!(!waiter.is_finished(), "should be waiting for global slot");

        // Drop one — waiter should proceed.
        drop(a);
        let _c = waiter.await.unwrap();
        drop(b);
        drop(_c);
    }

    #[tokio::test]
    async fn upload_limiter_per_peer_active_cap() {
        // Use per-peer=1, global=3: the bottleneck is the per-peer limit.
        // With per-peer=1, Alice can have at most 1 request (queued + active
        // combined).  We test that when Alice has 1 active, a second from
        // Alice enqueues into the queue but blocks on the per-peer semaphore
        // at start() time.
        let limiter = UploadLimiter::new(UploadLimitsConfig {
            max_active_uploads: 3,
            max_uploads_per_peer: 2,
            max_queued_uploads: 3,
            max_concurrent_verifications: 1,
            request_timeout: Duration::from_secs(10),
        });

        // Alice's first request starts (queued → active).
        let alice1 = limiter.try_enqueue("alice").unwrap().start().await.unwrap();

        // Alice's second request enqueues (fits in per-peer=2) but start
        // acquires a per-peer semaphore with 2 permits.  Since Alice already
        // holds 1, the second must wait until the first releases.
        let alice2 = limiter.try_enqueue("alice").unwrap();
        let waiter = tokio::spawn(async move { alice2.start().await.unwrap() });
        assert!(!waiter.is_finished(), "should be waiting for per-peer slot");

        // Drop alice1 — waiter proceeds.
        drop(alice1);
        let _alice2 = waiter.await.unwrap();
        drop(_alice2);
    }

    #[test]
    fn upload_limiter_verification_budget() {
        let limiter = upload_limiter(); // max_concurrent_verifications: 1

        let v1 = limiter
            .try_acquire_verification()
            .expect("first verification");
        assert!(
            limiter.try_acquire_verification().is_err(),
            "second should be busy"
        );
        drop(v1);
        assert!(
            limiter.try_acquire_verification().is_ok(),
            "should succeed after drop"
        );
    }

    #[test]
    fn upload_limiter_release_on_drop() {
        let limiter = upload_limiter(); // max_queued_uploads: 2

        let a = limiter.try_enqueue("a").unwrap();
        let b = limiter.try_enqueue("b").unwrap();

        // Queue is full.
        assert!(limiter.try_enqueue("c").is_err());

        // Dropping 'a' without starting releases its queue slot.
        drop(a);
        let _c = limiter.try_enqueue("c").expect("queue slot freed");

        // Dropping all should leave everything clean.
        drop(b);
        drop(_c);

        // Sanity: can enqueue again.
        let _d = limiter.try_enqueue("d").expect("clean state");
        drop(_d);
    }

    #[test]
    fn upload_limiter_config_accessors() {
        let config = UploadLimitsConfig {
            max_active_uploads: 10,
            max_uploads_per_peer: 3,
            max_queued_uploads: 50,
            max_concurrent_verifications: 5,
            request_timeout: Duration::from_secs(120),
        };
        let limiter = UploadLimiter::new(config.clone());
        let got = limiter.config();
        assert_eq!(got.max_active_uploads, 10);
        assert_eq!(got.max_uploads_per_peer, 3);
        assert_eq!(got.max_queued_uploads, 50);
        assert_eq!(got.max_concurrent_verifications, 5);
        assert_eq!(got.request_timeout, Duration::from_secs(120));
    }

    #[test]
    fn upload_error_display() {
        assert_eq!(UploadError::QueueFull.to_string(), "upload queue is full");
        assert_eq!(
            UploadError::PeerLimitReached.to_string(),
            "per-peer upload limit reached"
        );
        assert_eq!(
            UploadError::VerificationBusy.to_string(),
            "verification concurrency limit reached"
        );
    }
}
