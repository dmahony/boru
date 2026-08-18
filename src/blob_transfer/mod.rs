//! Blob transfer — downloads a blob from the iroh network to a temporary file
//! using the actual installed iroh-blobs API.
//!
//! # Design
//!
//! - Streams bytes directly to disk — never loads the full file into memory.
//! - Reports progress via a callback.
//! - Supports cancellation via a shared `AtomicBool` flag.
//! - Enforces a per-transfer timeout and a per-chunk network timeout.
//! - Persists progress periodically to the database via a [`ProgressUpdateGate`](crate::download_limits::ProgressUpdateGate).
//! - Temporary file is cleaned up on error or cancellation.
//!
//! # Flow
//!
//! 1. Download the blob into the iroh-blobs local store (network I/O, streamed
//!    by iroh-blobs directly).
//! 2. Stream the completed blob from the local store to the temporary file
//!    using a bounded buffer (128 KiB chunks), computing the BLAKE3 hash
//!    incrementally.
//! 3. Verify size + hash match the `SignedDownloadDescriptor`.
//! 4. Record the temp path in the database for crash recovery.
//! 5. Report progress every chunk.
//!
//! # Layout
//!
//! This module is a facade over the blob-transfer engine:
//! - [`transfer`] – the public `transfer_blob_to_temp` orchestrator
//! - [`client`]   – the public `request_and_transfer_blob` (acceptance + transfer)
//! - [`stage_a`]  – network download into the local blob store
//! - [`stage_b`]  – blob-store → temp-file copy + incremental hash
//! - [`tests`]    – unit tests
//!
//! The public surface is [`BlobTransferConfig`], [`BlobTransferProgress`],
//! [`transfer_blob_to_temp`], and [`request_and_transfer_blob`].

use std::time::Duration;

use anyhow::Result;

use crate::download_limits::BatchedProgressWriter;
use crate::storage::Storage;

mod client;
mod stage_a;
mod stage_b;
mod transfer;

#[cfg(test)]
pub(crate) mod tests;

pub use client::request_and_transfer_blob;
pub use transfer::transfer_blob_to_temp;

/// Persist a download failure on the Tokio blocking pool.
///
/// Direct `storage.fail_download` calls from async transfer tasks block a
/// worker thread (BORU-AUDIT-18); route them through the async facade so
/// the SQLite write runs on the blocking pool instead.
pub(crate) async fn fail_download_blocking(
    storage: &Storage,
    download_id: i64,
    msg: &str,
) -> Result<()> {
    let msg_owned = msg.to_owned();
    storage
        .run_blocking("blob_transfer.fail_download", move |s| {
            s.fail_download(download_id, &msg_owned, None)
        })
        .await
}

/// Persist a progress batch on the Tokio blocking pool.
///
/// Drains the batcher locally (a cheap in-memory lock) and runs the SQLite
/// write on the blocking pool so async transfer tasks never block a worker
/// thread (BORU-AUDIT-18).
pub(crate) async fn flush_progress_blocking(
    batcher: &BatchedProgressWriter,
    storage: &Storage,
) -> Result<()> {
    let batch = batcher.drain();
    if batch.is_empty() {
        return Ok(());
    }
    storage
        .run_blocking("blob_transfer.flush_progress", move |s| {
            let refs: Vec<(i64, u64, &str)> = batch
                .iter()
                .map(|(id, bytes, state)| (*id, *bytes, state.as_str()))
                .collect();
            s.flush_progress_batch(&refs)
        })
        .await
}

// ── Configuration ──────────────────────────────────────────────────────────

/// Configuration for a blob transfer operation.
#[derive(Debug, Clone)]
pub struct BlobTransferConfig {
    /// Maximum wall-clock time for the entire transfer (download + copy to
    /// temp file).  Default: 5 minutes.
    pub transfer_timeout: Duration,
    /// Per-chunk network-read timeout.  If no progress item arrives within
    /// this window the transfer is aborted.  Default: 30 seconds.
    pub chunk_timeout: Duration,
    /// How often to persist byte-count progress to the database.
    /// Default: 250 ms.
    pub progress_persist_interval: Duration,
}

impl Default for BlobTransferConfig {
    fn default() -> Self {
        Self {
            transfer_timeout: Duration::from_secs(300), // 5 minutes
            chunk_timeout: Duration::from_secs(30),     // 30 seconds
            progress_persist_interval: Duration::from_millis(250),
        }
    }
}

// ── Progress events ────────────────────────────────────────────────────────

/// Events emitted during a blob transfer.
#[derive(Debug, Clone)]
pub enum BlobTransferProgress {
    /// The download has started.
    Started {
        /// Total expected bytes from the download descriptor.
        total_bytes: u64,
    },
    /// Cumulative progress since the transfer started.
    Progress {
        /// Cumulative bytes received and written to the temporary file so far.
        bytes_received: u64,
        /// Total expected bytes.
        total_bytes: u64,
    },
    /// The transfer completed successfully.
    Completed {
        /// Total bytes received.
        total_bytes: u64,
        /// BLAKE3 content hash of the transferred data (hex).
        content_hash: String,
    },
    /// The transfer failed.
    Failed {
        /// Human-readable error.
        error: String,
    },
    /// The transfer was cancelled.
    Cancelled,
}

// ── Download chunk buffer size ────────────────────────────────────────────

/// Size of the bounded read buffer used when copying from the blob store
/// to the temporary file.  Keeps memory bounded regardless of file size.
pub(crate) const COPY_BUF_SIZE: usize = 128 * 1024; // 128 KiB

// ── Transfer timeout seconds (chunk-level) ─────────────────────────────────

/// Seconds without progress on a blob-read stream before aborting the copy.
pub(crate) const READ_TIMEOUT_SECS: u64 = 30;
