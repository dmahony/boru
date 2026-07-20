//! Download state-machine manager — tick-driven worker that processes
//! queued downloads through the full lifecycle.
//!
//! # Architecture
//!
//! [`DownloadManager`] owns the download lifecycle loop.  Each call to
//! [`tick`](DownloadManager::tick) claims one queued download row from
//! the database and advances it into the `resolving_peer` state.  Downstream
//! protocol handlers (file-access client, blob transfer) drive the row
//! through the remaining states via [`Storage`] methods.
//!
//! On startup, [`recover_from_restart`](DownloadManager::recover_from_restart)
//! collects interrupted downloads, recovers them to `queued`, and pushes them
//! into a [`BoundedStartupScheduler`] which starts up to `max_startup_downloads`
//! immediately via [`kickstart`] and holds the rest in a pending queue.
//! Remaining items are started by [`notify_startup_completed`] as active
//! downloads finish.
//!
//! Storage extension methods in this file add temp-path tracking and
//! permission-rejection bookkeeping to [`Storage`] without modifying
//! `src/storage.rs`.
//!
//! [`kickstart`]: BoundedStartupScheduler::kickstart
//! [`notify_startup_completed`]: DownloadManager::notify_startup_completed

use std::collections::HashMap;
use std::sync::{atomic::AtomicBool, Arc, Mutex};

use anyhow::Result;
use n0_error::StdResultExt;
use rusqlite::{params, OptionalExtension};
use tracing::{debug, info};

use crate::bounded_startup_scheduler::BoundedStartupScheduler;
use crate::chat_core::TRANSFER_TELEMETRY;
use crate::diagnostics::{DiagnosticEventKind, Diagnostics};
use crate::download_limits::{
    ActiveDownload, DownloadLimiter, DownloadLimitsConfig, QueuedDownload,
};
use crate::storage::Storage;

// ── DownloadManager ──────────────────────────────────────────────────────

/// Tick-driven download manager with bounded startup recovery.
///
/// On restart, call [`recover_from_restart`] which uses the intrinsic
/// [`BoundedStartupScheduler`] to limit how many downloads start at once.
/// After the burst, [`notify_startup_completed`] advances the scheduler to
/// start the next pending item.
///
/// [`recover_from_restart`]: Self::recover_from_restart
/// [`notify_startup_completed`]: Self::notify_startup_completed
#[derive(Debug)]
pub struct DownloadManager {
    storage: Storage,
    diagnostics: Option<Arc<Diagnostics>>,
    limiter: DownloadLimiter,
    /// Bounded startup scheduler — limits how many restored downloads start
    /// at once after a restart.  Created from the same [`DownloadLimitsConfig`]
    /// as the limiter so the burst cap stays consistent.
    startup_scheduler: Mutex<BoundedStartupScheduler>,
    /// ActiveDownload handles for items started by `kickstart`.  Dropping one
    /// releases its semaphore permits, and the caller should then call
    /// [`notify_startup_completed`] so the scheduler starts the next pending
    /// item.
    ///
    /// [`notify_startup_completed`]: Self::notify_startup_completed
    startup_active: Mutex<Vec<ActiveDownload>>,
    /// Per-download cancellation flags.  Set to `true` to signal active
    /// workers (blob transfer, permission request) to stop gracefully.
    cancel_flags: Mutex<HashMap<i64, Arc<AtomicBool>>>,
}

impl DownloadManager {
    /// Create a new download manager backed by the given storage.
    pub fn new(storage: Storage) -> Self {
        Self::with_limits(
            storage,
            DownloadLimitsConfig::from_env().unwrap_or_default(),
        )
    }

    /// Create a manager with explicit download admission limits.
    pub fn with_limits(storage: Storage, limits: DownloadLimitsConfig) -> Self {
        let scheduler_config = limits.clone();
        Self {
            storage,
            diagnostics: None,
            limiter: DownloadLimiter::new(limits),
            startup_scheduler: Mutex::new(BoundedStartupScheduler::new(scheduler_config)),
            startup_active: Mutex::new(Vec::new()),
            cancel_flags: Mutex::new(HashMap::new()),
        }
    }

    /// Admission controller used by transfer workers.
    pub fn limiter(&self) -> &DownloadLimiter {
        &self.limiter
    }

    /// Reference to the startup scheduler (read-only access for diagnostics).
    pub fn startup_scheduler(&self) -> &Mutex<BoundedStartupScheduler> {
        &self.startup_scheduler
    }

    /// Attach a diagnostics store for recording transfer events.
    pub fn with_diagnostics(&mut self, diagnostics: Arc<Diagnostics>) -> &mut Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// Recover interrupted downloads and bound the startup burst.
    ///
    /// Collects downloads that were in a non-terminal, non-queued state at
    /// the time of the restart, recovers them to `queued` (or `paused` if a
    /// temp file exists), then pushes the newly-queued admissions into the
    /// [`BoundedStartupScheduler`] and runs [`kickstart`] to start up to
    /// `max_startup_downloads` immediately.  Remaining items stay in the
    /// scheduler's pending queue.
    ///
    /// [`kickstart`]: BoundedStartupScheduler::kickstart
    pub async fn recover_from_restart(&self) -> Result<()> {
        let mut interrupted = Vec::new();
        for state in [
            "resolving_peer",
            "requesting_permission",
            "downloading",
            "verifying",
        ] {
            interrupted.extend(self.storage.list_downloads_by_state(state)?);
        }
        self.storage.recover_downloads_from_restart()?;
        // Recovery is ordered by the durable creation timestamp, not by the
        // order in which SQLite returned each state query.
        interrupted.sort_by_key(|download| (download.created_at_ms, download.id));

        // Create startup admissions and push them into the scheduler.
        let mut items: Vec<(i64, QueuedDownload)> = Vec::new();
        for download in &interrupted {
            let Some(restored) = self.storage.get_download(download.id)? else {
                continue;
            };

            // Emit a resume event for every item recovered by restart.
            // Items that went to 'queued' will be auto-scheduled below;
            // items that went to 'paused' await user action but are still
            // part of the restart recovery cycle.
            if restored.state == "queued" || restored.state == "paused" {
                let bytes = if restored.bytes_downloaded > 0 {
                    Some(restored.bytes_downloaded)
                } else {
                    None
                };
                TRANSFER_TELEMETRY.resume(download.id, "restart_recovery", bytes);
            }

            if restored.state != "queued" {
                continue;
            }
            if let Ok(queued) = self.limiter.try_enqueue(download.remote_peer.clone()) {
                items.push((download.id, queued));
            }
        }

        if items.is_empty() {
            return Ok(());
        }

        // ── Phase 1: push items and pop the burst budget (sync) ───
        let burst = {
            let mut scheduler = self
                .startup_scheduler
                .lock()
                .expect("startup scheduler poisoned");
            scheduler.push(items);
            scheduler.pop_burst()
        }; // MutexGuard dropped here

        // ── Phase 2: start items without holding the lock (async) ──
        let started = Self::start_burst_items(burst).await;

        // ── Phase 3: record started count (sync) ──────────────────
        {
            let mut scheduler = self
                .startup_scheduler
                .lock()
                .expect("startup scheduler poisoned");
            scheduler.record_started(started.len());
        }
        let mut active = self.startup_active.lock().expect("startup active poisoned");
        active.extend(started);
        Ok(())
    }

    /// Start a batch of popped queued downloads, stopping on the first
    /// failure.  Mirrors the original [`kickstart`] loop logic.
    ///
    /// [`kickstart`]: BoundedStartupScheduler::kickstart
    async fn start_burst_items(items: Vec<(i64, QueuedDownload)>) -> Vec<ActiveDownload> {
        let mut started = Vec::with_capacity(items.len());
        for (_id, queued) in items {
            match queued.start().await {
                Ok(active) => {
                    info!("bounded-startup-scheduler: started download (burst)");
                    started.push(active);
                }
                Err(e) => {
                    info!(
                        "bounded-startup-scheduler: burst start failed: {e:?}, \
                         ending burst early"
                    );
                    break;
                }
            }
        }
        started
    }

    /// Clean up scheduler tracking for a download that completed locally
    /// without needing a network transfer.
    ///
    /// Removes the download from the scheduler's pending queue and ID tracker
    /// if it was a scheduler-managed startup item.
    #[cfg(test)]
    fn cleanup_local_completion(&self, download_id: i64) {
        let mut scheduler = self
            .startup_scheduler
            .lock()
            .expect("startup scheduler poisoned");
        scheduler.remove_from_pending(download_id);
        // Also remove from the ID tracker — the item may have already been
        // started by kickstart (popped from pending queue) and only needs
        // the tracker entry cleared.
        scheduler.remove_id(download_id);
    }

    /// Notify the startup scheduler that a download managed by it has
    /// completed.  The scheduler decrements its active count and starts
    /// the next pending item if under the concurrent cap.
    ///
    /// Call this after a scheduler-started download finishes (completed,
    /// failed, cancelled).
    ///
    /// Returns the new [`ActiveDownload`] handle if one was started.
    pub async fn notify_startup_completed(&self) -> Option<ActiveDownload> {
        // ── Phase 1: sync decrement ─────────────────────────────────
        {
            let mut scheduler = self
                .startup_scheduler
                .lock()
                .expect("startup scheduler poisoned");
            scheduler.notify_completed_sync();
        }

        // ── Phase 2: retry loop — pop items under lock, start without ─
        loop {
            let maybe_item = {
                let mut scheduler = self
                    .startup_scheduler
                    .lock()
                    .expect("startup scheduler poisoned");
                scheduler.pop_next_to_start()
            };
            let (_id, queued) = maybe_item?;
            match queued.start().await {
                Ok(active) => {
                    let mut scheduler = self
                        .startup_scheduler
                        .lock()
                        .expect("startup scheduler poisoned");
                    scheduler.record_started(1);
                    return Some(active);
                }
                Err(e) => {
                    info!(
                        "bounded-startup-scheduler: start failed: {e:?}, \
                         skipping item"
                    );
                    continue;
                }
            }
        }
    }

    /// Advance the download state machine by one tick.
    ///
    /// Returns `true` if a download was claimed (work was done), `false` if
    /// the queue is idle.
    #[allow(clippy::unused_async)]
    pub async fn tick(&self) -> Result<bool> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // ── 1. Claim the oldest queued download ────────────────────────
        //
        // Read the id first, then UPDATE by id, so that the subquery does
        // not fight SQLite's restriction on mutating the same table
        // referenced in a sub-SELECT.
        //
        // Skip downloads that are already managed by the startup scheduler.
        let maybe_id: Option<(i64, String)> = self.storage.with_conn(|conn| {
            let id_and_peer: Option<(i64, String)> = conn
                .query_row(
                    "SELECT id, remote_peer FROM downloads WHERE state = 'queued'
                     ORDER BY created_at_ms ASC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();
            Ok(id_and_peer)
        })?;

        let Some((download_id, _remote_peer)) = maybe_id else {
            return Ok(false);
        };

        // Skip if the startup scheduler owns this download.
        {
            let scheduler = self
                .startup_scheduler
                .lock()
                .expect("startup scheduler poisoned");
            if scheduler.contains(download_id) {
                return Ok(false);
            }
        }

        let changed = self.storage.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE downloads SET state = 'resolving_peer', updated_at_ms = ?1
                     WHERE id = ?2 AND state = 'queued'",
                    params![now, download_id],
                )
                .std_context("claim queued download")?;
            Ok(n)
        })?;

        if changed == 0 {
            // Another worker claimed it first — nothing to do this tick.
            return Ok(false);
        }

        info!(
            download_id = download_id,
            "download-manager: claimed queued download"
        );

        if let Some(diag) = &self.diagnostics {
            diag.record(
                None,
                DiagnosticEventKind::TransferStarted {
                    transfer_id: download_id.to_string(),
                    total_bytes: 0,
                },
            );
        }

        Ok(true)
    }

    // ── Cancellation infrastructure ───────────────────────────────────

    /// Register a cancellation flag for a download — creates a new `false`
    /// flag and stores it.  The flag is returned so external workers can
    /// poll it during network operations.
    pub fn register_cancel_flag(&self, download_id: i64) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.cancel_flags
            .lock()
            .expect("cancel_flags poisoned")
            .insert(download_id, flag.clone());
        flag
    }

    /// Get the cancellation flag for a download, or create one if none exists.
    /// The returned flag can be passed to [`transfer_blob_to_temp`] or similar
    /// network tasks so they check for cancellation mid-transfer.
    ///
    /// [`transfer_blob_to_temp`]: crate::blob_transfer::transfer_blob_to_temp
    pub fn cancel_flag(&self, download_id: i64) -> Arc<AtomicBool> {
        let mut map = self.cancel_flags.lock().expect("cancel_flags poisoned");
        map.entry(download_id)
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    }

    /// Remove the cancellation flag for a download (after completion, failure,
    /// or the flag is no longer needed).
    pub fn remove_cancel_flag(&self, download_id: i64) {
        self.cancel_flags
            .lock()
            .expect("cancel_flags poisoned")
            .remove(&download_id);
    }

    /// Pause an active download.
    ///
    /// Sets the cancel flag (signalling any in-flight worker), cleans up the
    /// temporary file if one exists, and transitions the download state to
    /// `paused` in storage.
    ///
    /// Idempotent on already-paused downloads.  Rejects terminal states.
    pub fn pause_download(&self, download_id: i64) -> Result<()> {
        // 1. Signal any in-flight worker to stop.
        self.cancel_flag(download_id)
            .store(true, std::sync::atomic::Ordering::Relaxed);

        // 2. Clean up temporary file if present.
        let temp_path = self
            .storage
            .get_download_temp_path(download_id)
            .ok()
            .flatten();
        if let Some(ref path) = temp_path {
            let _ = std::fs::remove_file(path);
        }
        let _ = self.storage.clear_download_temp_path(download_id);

        // 3. Transition state in storage.
        self.storage.pause_download(download_id)?;

        TRANSFER_TELEMETRY.pause(download_id, "user", None);
        Ok(())
    }

    /// Resume a paused download — transitions to `resolving_peer` and emits a
    /// `resume` telemetry event with reason `user`.
    ///
    /// Idempotent on already-active downloads (those in `resolving_peer`,
    /// `requesting_permission`, `downloading`, or `verifying`).  Rejects
    /// terminal and unknown states.
    pub fn resume_download(&self, download_id: i64) -> Result<()> {
        // Read the current bytes_received before resume (used for telemetry).
        let bytes = self
            .storage
            .get_download(download_id)
            .ok()
            .flatten()
            .map(|dl| dl.bytes_downloaded);
        self.storage.resume_download(download_id)?;
        TRANSFER_TELEMETRY.resume(download_id, "user", bytes);
        Ok(())
    }

    /// Cancel a download permanently.
    ///
    /// Transitions the download state to `cancelled` in storage and removes
    /// the cancellation flag so a fresh, unset flag is created on the next
    /// lookup.
    pub fn cancel_download(&self, download_id: i64) -> Result<()> {
        self.storage.cancel_download(download_id)?;
        self.remove_cancel_flag(download_id);
        TRANSFER_TELEMETRY.cancellation(download_id, "user", None);
        Ok(())
    }
}

// ── Storage extension methods ────────────────────────────────────────────
//
// These methods are used by the blob-transfer and file-access layers and are
// placed here (rather than in src/storage.rs) to keep the storage module
// focused on schema management and repository-style accessors.

impl Storage {
    /// Persist the temporary file path for a download so it can be
    /// recovered after a process restart or crash.
    ///
    /// If the `downloads` table was created by an older schema version and
    /// does not yet have a `temp_path` column, the column is added
    /// automatically (idempotent — repeated calls are harmless).
    pub fn set_download_temp_path(&self, download_id: i64, path: &str) -> Result<()> {
        self.with_conn(|conn| {
            // Ensure the column exists (ignore "duplicate column" errors
            // so the method works with both old and new schema versions).
            let _ = conn.execute_batch("ALTER TABLE downloads ADD COLUMN temp_path TEXT;");

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            conn.execute(
                "UPDATE downloads SET temp_path = ?1, updated_at_ms = ?2 WHERE id = ?3",
                params![path, now, download_id],
            )
            .std_context("set download temp path")?;
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Read the persisted temporary file path for a download.
    ///
    /// Returns `None` if no temp path has been recorded or the column
    /// does not exist (graceful handling for old schema versions).
    pub fn get_download_temp_path(&self, download_id: i64) -> Result<Option<String>> {
        self.with_conn(|conn| {
            let _ = conn.execute_batch("ALTER TABLE downloads ADD COLUMN temp_path TEXT;");
            let result: Option<Option<String>> = conn
                .query_row(
                    "SELECT temp_path FROM downloads WHERE id = ?1",
                    params![download_id],
                    |row| row.get(0),
                )
                .optional()
                .std_context("get download temp path")?;
            Ok(result.flatten())
        })
        .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Clear the persisted temporary file path for a download.
    ///
    /// Used after the temp file has been removed so the database does not
    /// reference a non-existent path.
    pub fn clear_download_temp_path(&self, download_id: i64) -> Result<()> {
        self.with_conn(|conn| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            conn.execute(
                "UPDATE downloads SET temp_path = NULL, updated_at_ms = ?1 WHERE id = ?2",
                params![now, download_id],
            )
            .std_context("clear download temp path")?;
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Transition a download from `resolving_peer` or `requesting_permission`
    /// to `failed` with the given reason.
    ///
    /// Used when the remote peer rejects the permission request (denied,
    /// not found, expired descriptor, rate-limited, etc.).  The retry count
    /// is incremented so the download can be re-scheduled if the rejection
    /// is transient.
    pub fn reject_resumed_permission(&self, download_id: i64, reason: &str) -> Result<()> {
        self.with_conn(|conn| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            let changed = conn
                .execute(
                    "UPDATE downloads SET state = 'paused', last_error = ?1,
                            retry_count = retry_count + 1, updated_at_ms = ?2
                     WHERE id = ?3 AND state IN ('resolving_peer', 'requesting_permission')",
                    params![reason, now, download_id],
                )
                .std_context("reject resumed permission")?;
            if changed == 0 {
                debug!(
                    download_id,
                    "reject_resumed_permission: no matching download in \
                     resolving/requesting state"
                );
            }
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("{e}"))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download_limits::DownloadLimitsConfig;
    use std::time::Duration;

    fn test_config() -> DownloadLimitsConfig {
        DownloadLimitsConfig {
            max_concurrent_downloads: 5,
            max_startup_downloads: 2,
            max_downloads_per_peer: 5,
            max_active_hash_verifications: 1,
            max_queued_downloads: 10,
            progress_update_interval: Duration::from_millis(100),
        }
    }

    #[tokio::test]
    async fn recover_from_restart_recovers_interrupted_to_queued() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash-a", 4, "app/bin", "a.bin", b"data")
            .unwrap();
        let id = storage.create_download("hash-a", "peer1", 4).unwrap();
        storage
            .update_download_progress(id, 0, "resolving_peer")
            .unwrap();

        let manager = DownloadManager::with_limits(storage.clone(), test_config());
        manager.recover_from_restart().await.unwrap();

        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(
            dl.state, "queued",
            "interrupted download recovers to queued"
        );
    }

    #[tokio::test]
    async fn recover_from_restart_binds_startup_burst() {
        let storage = Storage::memory().unwrap();
        let mut ids = Vec::new();
        for i in 0..5 {
            let hash = format!("hash-{i}");
            storage
                .put_file_object(&hash, 4, "app/bin", &format!("{i}.bin"), b"data")
                .unwrap();
            let id = storage.create_download(&hash, "peer2", 4).unwrap();
            storage
                .update_download_progress(id, 0, "resolving_peer")
                .unwrap();
            ids.push(id);
        }

        let manager = DownloadManager::with_limits(storage.clone(), test_config());
        manager.recover_from_restart().await.unwrap();

        // All downloads are queued (state recovery).
        for &id in &ids {
            let dl = storage.get_download(id).unwrap().unwrap();
            assert_eq!(dl.state, "queued", "download {id} should be queued");
        }

        // The scheduler should manage all 5 items:
        // max_startup_downloads=2 started by kickstart, 3 remain pending.
        let scheduler = manager.startup_scheduler.lock().unwrap();
        assert_eq!(
            scheduler.active_count(),
            2,
            "kickstart starts up to max_startup_downloads"
        );
        assert_eq!(scheduler.pending_count(), 3, "remaining items stay pending");
        assert!(scheduler.contains(ids[0]));
        assert!(scheduler.contains(ids[1]));
        assert!(scheduler.contains(ids[2]));
        drop(scheduler);
    }

    #[tokio::test]
    async fn tick_skips_scheduler_managed_downloads() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash-sched", 4, "app/bin", "sched.bin", b"data")
            .unwrap();

        // One download in resolving_peer state (recovered by scheduler).
        let id_sched = storage.create_download("hash-sched", "peer3", 4).unwrap();
        storage
            .update_download_progress(id_sched, 0, "resolving_peer")
            .unwrap();

        let manager = DownloadManager::with_limits(storage.clone(), test_config());
        manager.recover_from_restart().await.unwrap();
        {
            let s = manager.startup_scheduler.lock().unwrap();
            assert_eq!(
                s.pending_count() + s.active_count(),
                1,
                "one item in scheduler"
            );
        }

        // tick should return false because the only queued item is managed by the scheduler.
        let worked = manager.tick().await.unwrap();
        assert!(
            !worked,
            "tick should not claim a scheduler-managed download"
        );
    }

    #[tokio::test]
    async fn notify_startup_completed_advances_scheduler() {
        let storage = Storage::memory().unwrap();
        for i in 0..4 {
            let hash = format!("hash-notify-{i}");
            storage
                .put_file_object(&hash, 4, "app/bin", &format!("{i}.bin"), b"data")
                .unwrap();
            let id = storage.create_download(&hash, "peer4", 4).unwrap();
            storage
                .update_download_progress(id, 0, "resolving_peer")
                .unwrap();
        }

        let manager = DownloadManager::with_limits(storage.clone(), test_config());
        manager.recover_from_restart().await.unwrap();

        // active=2, pending=2
        {
            let s = manager.startup_scheduler.lock().unwrap();
            assert_eq!(s.active_count(), 2);
            assert_eq!(s.pending_count(), 2);
        }

        // Drop one active handle and notify — scheduler starts next pending.
        {
            let mut active = manager.startup_active.lock().unwrap();
            assert!(!active.is_empty(), "should have active handles");
            active.pop(); // release one permit
        }
        let next = manager.notify_startup_completed().await;
        assert!(next.is_some(), "should start next pending item");

        // active still 2 (one freed, one started), pending drops to 1.
        {
            let s = manager.startup_scheduler.lock().unwrap();
            assert_eq!(s.active_count(), 2);
            assert_eq!(s.pending_count(), 1);
        }
    }

    #[tokio::test]
    async fn cleanup_local_completion_removes_from_scheduler() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash-clean", 4, "app/bin", "clean.bin", b"data")
            .unwrap();
        let id = storage.create_download("hash-clean", "peer5", 4).unwrap();
        storage
            .update_download_progress(id, 0, "resolving_peer")
            .unwrap();

        let manager = DownloadManager::with_limits(storage.clone(), test_config());
        manager.recover_from_restart().await.unwrap();

        // The item is still pending (no concurrent cap issues since max_startup=2).
        assert!(manager.startup_scheduler.lock().unwrap().contains(id));

        manager.cleanup_local_completion(id);
        assert!(
            !manager.startup_scheduler.lock().unwrap().contains(id),
            "should be removed from scheduler"
        );
    }

    #[tokio::test]
    async fn resume_download_transitions_state_and_emits_event() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash-resume", 4, "app/bin", "resume.bin", b"data")
            .unwrap();
        let id = storage.create_download("hash-resume", "peer6", 4).unwrap();

        // Set download to paused state.
        storage.pause_download(id).unwrap();

        let manager = DownloadManager::with_limits(storage.clone(), test_config());
        manager.resume_download(id).unwrap();

        // Verify state transition.
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(
            dl.state, "resolving_peer",
            "resumed download should be in resolving_peer state"
        );
    }

    #[tokio::test]
    async fn resume_download_is_idempotent_on_active_downloads() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash-resume-idem", 4, "app/bin", "resume-idem.bin", b"data")
            .unwrap();
        let id = storage
            .create_download("hash-resume-idem", "peer7", 4)
            .unwrap();

        // Set to resolving_peer to test idempotency.
        storage
            .update_download_progress(id, 0, "resolving_peer")
            .unwrap();

        let manager = DownloadManager::with_limits(storage.clone(), test_config());
        // Should not error — idempotent on already-active downloads.
        manager.resume_download(id).unwrap();
    }

    #[tokio::test]
    async fn resume_download_rejects_terminal_states() {
        let storage = Storage::memory().unwrap();
        storage
            .put_file_object("hash-resume-term", 4, "app/bin", "resume-term.bin", b"data")
            .unwrap();
        let id = storage
            .create_download("hash-resume-term", "peer8", 4)
            .unwrap();

        // Complete the download.
        storage.complete_download(id, 4).unwrap();

        let manager = DownloadManager::with_limits(storage.clone(), test_config());
        let result = manager.resume_download(id);
        assert!(result.is_err(), "resume should reject completed downloads");
    }
}
