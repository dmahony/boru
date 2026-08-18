//! Stage B — copy from the blob store to a temp file, hashing incrementally.
//!
//! Streams the completed blob out of the local store to a temp file in
//! bounded chunks, computing the BLAKE3 hash as it goes, and persists
//! progress periodically.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::warn;

use super::{
    fail_download_blocking, flush_progress_blocking, BlobTransferConfig, BlobTransferProgress,
    COPY_BUF_SIZE, READ_TIMEOUT_SECS,
};
use crate::chat_core::TRANSFER_TELEMETRY;
use crate::download_limits::BatchedProgressWriter;
use crate::storage::Storage;

// ── Stage B: Copy from blob store to temp file ───────────────────────────

/// Stream the blob from the local iroh-blobs store to a temporary file,
/// computing the BLAKE3 hash as we go.
///
/// Returns the number of bytes written and the raw 32-byte BLAKE3 hash of the
/// copied content — the same representation as the descriptor's canonical
/// `blob_hash`, so the final integrity check compares bytes directly.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stage_copy_to_temp(
    blob_store: &iroh_blobs::api::Store,
    blob_hash: iroh_blobs::Hash,
    temp_path: &Path,
    total_bytes: u64,
    cancel_flag: &AtomicBool,
    config: &BlobTransferConfig,
    batcher: &BatchedProgressWriter,
    storage: &Storage,
    download_id: i64,
    initial_bytes: u64,
    on_progress: &mut impl FnMut(BlobTransferProgress),
) -> Result<(u64, [u8; 32])> {
    let deadline = Instant::now() + config.transfer_timeout;

    // Ensure the parent directory exists.
    if let Some(parent) = temp_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("blob-transfer: create temp dir")?;
    }

    let mut file = tokio::fs::File::create(temp_path)
        .await
        .context("blob-transfer: create temp file")?;

    let mut reader = blob_store.blobs().reader(blob_hash);
    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut hasher = blake3::Hasher::new();
    let mut bytes_written: u64 = 0;
    let mut last_checkpoint = Instant::now();
    let checkpoint_interval = config.progress_persist_interval;

    loop {
        // ── Cancellation check ──────────────────────────────────────
        if cancel_flag.load(Ordering::Relaxed) {
            on_progress(BlobTransferProgress::Cancelled);
            return Err(anyhow::anyhow!(
                "blob-transfer: cancelled during copy to temp"
            ));
        }

        // ── Timeout check ──────────────────────────────────────────
        if Instant::now() > deadline {
            let msg = format!(
                "blob-transfer: copy to temp timed out after {:?}",
                config.transfer_timeout
            );
            warn!(download_id, "{msg}");
            on_progress(BlobTransferProgress::Failed { error: msg.clone() });
            fail_download_blocking(storage, download_id, &msg).await?;
            return Err(anyhow::anyhow!("{msg}"));
        }

        // ── Read a chunk (bounded buffer) ──────────────────────────
        let n = tokio::time::timeout(
            Duration::from_secs(READ_TIMEOUT_SECS),
            reader.read(&mut buf),
        )
        .await
        .context("blob-transfer: read chunk timed out")?
        .context("blob-transfer: read from blob store failed")?;

        if n == 0 {
            break; // EOF
        }

        // ── Write to temp file ─────────────────────────────────────
        file.write_all(&buf[..n])
            .await
            .context("blob-transfer: write to temp file failed")?;

        // ── Hash incrementally ────────────────────────────────────
        hasher.update(&buf[..n]);

        bytes_written += n as u64;

        // ── Persist progress periodically via batched writer ──────
        let total_received = initial_bytes + bytes_written;
        if batcher.submit(download_id, total_received, "downloading") {
            if let Err(e) = flush_progress_blocking(batcher, storage).await {
                warn!(
                    download_id,
                    bytes = total_received,
                    "blob-transfer: progress persist failed: {e:#}"
                );
            }
        }

        on_progress(BlobTransferProgress::Progress {
            bytes_received: initial_bytes + bytes_written,
            total_bytes,
        });

        // Emit telemetry checkpoint (rate-limited by progress interval).
        if last_checkpoint.elapsed() >= checkpoint_interval {
            let total_received = initial_bytes + bytes_written;
            TRANSFER_TELEMETRY.progress_checkpoint(
                download_id,
                total_received,
                total_bytes,
                None,
                Some(checkpoint_interval.as_millis() as u64),
                None,
            );
            last_checkpoint = Instant::now();
        }
    }

    // ── Finalise the file ────────────────────────────────────────────
    file.flush()
        .await
        .context("blob-transfer: flush temp file failed")?;
    file.shutdown()
        .await
        .context("blob-transfer: shutdown temp file failed")?;
    drop(file);

    // Force a final progress persist.
    let total_received = initial_bytes + bytes_written;
    if batcher.submit(download_id, total_received, "downloading") || batcher.has_pending() {
        if let Err(e) = flush_progress_blocking(batcher, storage).await {
            warn!(
                download_id,
                bytes = total_received,
                "blob-transfer: final progress persist failed: {e:#}"
            );
        }
    }

    let hash_bytes = *hasher.finalize().as_bytes();
    Ok((bytes_written, hash_bytes))
}
