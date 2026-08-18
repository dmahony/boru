//! The public `transfer_blob_to_temp` orchestrator.
//!
//! Coordinates the two transfer stages ([`super::stage_a`], [`super::stage_b`])
//! and the size/hash verification, reporting progress along the way.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::Result;
use iroh::Endpoint;
use iroh_base::PublicKey;
use tracing::{debug, error, info};

use super::{
    fail_download_blocking, stage_a::stage_network_download, stage_b::stage_copy_to_temp,
    BlobTransferConfig, BlobTransferProgress,
};
use crate::chat_core::TRANSFER_TELEMETRY;
use crate::diagnostics::ErrorCategory;
use crate::download_limits::{BatchedProgressWriter, DownloadLimiter};
use crate::file_access_protocol::SignedDownloadDescriptor;
use crate::storage::Storage;

// ── Public API ────────────────────────────────────────────────────────────

/// Download a blob from the iroh network to a temporary file.
///
/// # Arguments
///
/// * `blob_store` — The shared iroh-blobs store (the blob is downloaded into
///   this store and then copied out to the temp file).
/// * `endpoint` — The local iroh endpoint used for peer-to-peer connections.
/// * `descriptor` — The [`SignedDownloadDescriptor`] authorising the transfer
///   (carries the content hash, expected size, and blob format).
/// * `providers` — The peers expected to have this blob (the file owner
///   first, then fallback peers from the gossip mesh).
/// * `temp_path` — Where to write the temporary file during transfer.
/// * `storage` — The relational storage layer (for persisting progress).
/// * `download_id` — The durable download row id for progress updates.
/// * `limiter` — The [`DownloadLimiter`] for admission control and progress
///   gating.
/// * `cancel_flag` — Shared `Arc<AtomicBool>`; set to `true` to cancel.
/// * `config` — Transfer timeout and persistence tuning.
/// * `on_progress` — Callback invoked on every progress event.
///
/// # Returns
///
/// * `Ok(temp_path)` — the verified temporary file path (caller should
///   verify, install, and complete via [`verify_install_and_complete`]).
/// * `Err` — if the transfer fails, is cancelled, or times out.
///
/// [`verify_install_and_complete`]: crate::download::verify_install_and_complete
#[allow(clippy::too_many_arguments)]
pub async fn transfer_blob_to_temp(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    descriptor: &SignedDownloadDescriptor,
    providers: Vec<PublicKey>,
    temp_path: PathBuf,
    storage: &Storage,
    download_id: i64,
    limiter: &DownloadLimiter,
    cancel_flag: Arc<AtomicBool>,
    config: BlobTransferConfig,
    mut on_progress: impl FnMut(BlobTransferProgress),
) -> Result<PathBuf> {
    let total_bytes = descriptor.size_bytes;
    // The blob lookup uses the descriptor's canonical raw hash.  This is the
    // same value that was signed by the owner and checked by the client, so a
    // transfer can never authorize one hash and download another
    // (BORU-AUDIT-06).  iroh-blobs Hash from raw bytes cannot fail.
    let blob_hash: iroh_blobs::Hash = descriptor.blob_hash.into();

    // ── 1. Record temp path in storage (crash recovery) ─────────────────
    let temp_str = temp_path.to_string_lossy().to_string();
    storage
        .run_blocking("blob_transfer.set_temp_path", {
            let temp = temp_str.clone();
            move |s| s.set_download_temp_path(download_id, &temp)
        })
        .await?;
    debug!(download_id, path = %temp_str, "blob-transfer: recorded temp path");

    on_progress(BlobTransferProgress::Started { total_bytes });
    TRANSFER_TELEMETRY.transfer_started(download_id, total_bytes, None);

    // ── 2. Acquire download slot from the limiter ──────────────────────
    let peer_str = descriptor.owner_id.to_string();
    let queued = limiter
        .try_enqueue(&peer_str)
        .map_err(|e| anyhow::anyhow!("blob-transfer: admission failed: {e:?}"))?;
    let _active = queued
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("blob-transfer: start failed: {e:?}"))?;

    let batched_writer = BatchedProgressWriter::new(config.progress_persist_interval);

    // ── 3. Stage A: Network download into blob store ──────────────────
    let result = stage_network_download(
        blob_store,
        endpoint,
        blob_hash,
        &providers,
        &cancel_flag,
        &config,
        &batched_writer,
        storage,
        download_id,
        total_bytes,
        &mut on_progress,
    )
    .await;

    let network_bytes = match result {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(e);
        }
    };

    // ── 3a. Acquire hash verification budget (CPU-bound hashing) ──────
    // Hold the permit through step 4 (hash computation) and steps 5-6
    // (size + hash verification) so the concurrent hash limit is respected
    // for the entire verification lifecycle.
    let _hash_permit = limiter.acquire_hash_verification().await;

    // ── 4. Stage B: Copy from blob store to temp file + hash ───────────
    let result = stage_copy_to_temp(
        blob_store,
        blob_hash,
        &temp_path,
        total_bytes,
        &cancel_flag,
        &config,
        &batched_writer,
        storage,
        download_id,
        network_bytes,
        &mut on_progress,
    )
    .await;

    let (bytes_written, computed_hash) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(e);
        }
    };

    // ── 5. Verify size ─────────────────────────────────────────────────
    if bytes_written != total_bytes {
        let _ = tokio::fs::remove_file(&temp_path).await;
        let msg = format!(
            "blob-transfer: size mismatch after copy: wrote {bytes_written}, expected {total_bytes}"
        );
        error!(download_id, "{msg}");
        TRANSFER_TELEMETRY.verification(
            download_id,
            "failed",
            Some(bytes_written),
            Some(total_bytes),
        );
        TRANSFER_TELEMETRY.failure(
            download_id,
            ErrorCategory::SizeMismatch,
            false,
            Some(bytes_written),
            None,
            None,
        );
        fail_download_blocking(storage, download_id, &msg).await?;
        on_progress(BlobTransferProgress::Failed { error: msg.clone() });
        return Err(anyhow::anyhow!("{msg}"));
    }

    // ── 6. Verify BLAKE3 hash ──────────────────────────────────────────
    // The computed hash of the downloaded content is compared byte-for-byte
    // against the descriptor's canonical `blob_hash` — the same value that
    // was signed by the owner, used for the blob lookup, and checked by the
    // client's authorization step.  No transfer can authorize one hash and
    // download another (BORU-AUDIT-06).
    let expected_hash_bytes = descriptor.blob_hash;
    if computed_hash != expected_hash_bytes {
        let hash_hex = hex::encode(computed_hash);
        let expected_hex = hex::encode(expected_hash_bytes);
        let _ = tokio::fs::remove_file(&temp_path).await;
        let msg = format!(
            "blob-transfer: content hash mismatch: computed {hash_hex}, expected {expected_hex}"
        );
        error!(download_id, "{msg}");
        TRANSFER_TELEMETRY.verification(
            download_id,
            "failed",
            Some(bytes_written),
            Some(total_bytes),
        );
        TRANSFER_TELEMETRY.failure(
            download_id,
            ErrorCategory::IntegrityMismatch,
            false,
            Some(bytes_written),
            None,
            None,
        );
        fail_download_blocking(storage, download_id, &msg).await?;
        on_progress(BlobTransferProgress::Failed { error: msg.clone() });
        return Err(anyhow::anyhow!("{msg}"));
    }

    TRANSFER_TELEMETRY.verification(
        download_id,
        "passed",
        Some(bytes_written),
        Some(total_bytes),
    );
    let hash_hex = hex::encode(computed_hash);
    info!(
        download_id,
        bytes = bytes_written,
        hash = %hash_hex,
        "blob-transfer: completed successfully"
    );
    on_progress(BlobTransferProgress::Completed {
        total_bytes: bytes_written,
        content_hash: hash_hex,
    });

    Ok(temp_path)
}
