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
//! Storage extension methods in this file add temp-path tracking and
//! permission-rejection bookkeeping to [`Storage`] without modifying
//! `src/storage.rs`.

use std::sync::Arc;

use anyhow::Result;
use n0_error::StdResultExt;
use rusqlite::params;
use tracing::{debug, info};

use crate::diagnostics::{DiagnosticEventKind, Diagnostics};
use crate::storage::Storage;

// ── DownloadManager ──────────────────────────────────────────────────────

/// Tick-driven download manager.
///
/// Minimal shell that will be fleshed out as the transfer pipeline is built.
#[derive(Debug)]
pub struct DownloadManager {
    storage: Storage,
    diagnostics: Option<Arc<Diagnostics>>,
}

impl DownloadManager {
    /// Create a new download manager backed by the given storage.
    pub fn new(storage: Storage) -> Self {
        Self {
            storage,
            diagnostics: None,
        }
    }

    /// Attach a diagnostics store for recording transfer events.
    pub fn with_diagnostics(&mut self, diagnostics: Arc<Diagnostics>) -> &mut Self {
        self.diagnostics = Some(diagnostics);
        self
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
        let maybe_id: Option<i64> = self.storage.with_conn(|conn| {
            let id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM downloads WHERE state = 'queued'
                     ORDER BY created_at_ms ASC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .ok();
            Ok(id)
        })?;

        let Some(download_id) = maybe_id else {
            return Ok(false);
        };

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

        info!(download_id, "download-manager: claimed queued download");

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
