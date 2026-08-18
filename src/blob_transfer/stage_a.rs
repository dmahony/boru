//! Stage A — network download into the local iroh-blobs store.
//!
//! Streams the blob from providers into the local blob store, emitting
//! progress events and honouring cancellation / per-chunk timeouts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use iroh::Endpoint;
use iroh_base::PublicKey;
use n0_future::StreamExt;
use tracing::{error, warn};

use super::{
    fail_download_blocking, flush_progress_blocking, BlobTransferConfig, BlobTransferProgress,
};
use crate::chat_core::TRANSFER_TELEMETRY;
use crate::download_limits::BatchedProgressWriter;
use crate::storage::Storage;

// ── Stage A: Network download ────────────────────────────────────────────

/// Download the blob into the iroh-blobs local store and emit progress
/// events as bytes arrive.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stage_network_download(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    blob_hash: iroh_blobs::Hash,
    providers: &[PublicKey],
    cancel_flag: &AtomicBool,
    config: &BlobTransferConfig,
    batcher: &BatchedProgressWriter,
    storage: &Storage,
    download_id: i64,
    total_bytes: u64,
    on_progress: &mut impl FnMut(BlobTransferProgress),
) -> Result<u64> {
    let deadline = Instant::now() + config.transfer_timeout;

    let downloader = blob_store.downloader(endpoint);
    let progress = downloader.download(blob_hash, providers.to_vec());
    let mut stream = progress
        .stream()
        .await
        .context("blob-transfer: open download stream")?;

    let mut network_bytes: u64 = 0;
    let mut last_checkpoint = Instant::now();
    let checkpoint_interval = config.progress_persist_interval;

    loop {
        // ── Cancellation check ──────────────────────────────────────
        if cancel_flag.load(Ordering::Relaxed) {
            on_progress(BlobTransferProgress::Cancelled);
            return Err(anyhow::anyhow!(
                "blob-transfer: cancelled during network download"
            ));
        }

        // ── Timeout check ──────────────────────────────────────────
        if Instant::now() > deadline {
            let msg = format!(
                "blob-transfer: network download timed out after {:?}",
                config.transfer_timeout
            );
            warn!(download_id, "{msg}");
            on_progress(BlobTransferProgress::Failed { error: msg.clone() });
            fail_download_blocking(storage, download_id, &msg).await?;
            return Err(anyhow::anyhow!("{msg}"));
        }

        // ── Read next progress item (with per-chunk timeout) ────────
        let item = tokio::time::timeout(config.chunk_timeout, stream.next())
            .await
            .context("blob-transfer: chunk read timed out")?;

        let Some(item) = item else {
            // Stream ended — network download completed successfully.
            // Force a final flush of any queued progress.
            if batcher.has_pending() {
                if let Err(e) = flush_progress_blocking(batcher, storage).await {
                    warn!(
                        download_id,
                        "blob-transfer: final progress persist failed: {e:#}"
                    );
                }
            }
            return Ok(network_bytes);
        };

        match item {
            iroh_blobs::api::downloader::DownloadProgressItem::Progress(n) => {
                network_bytes = n;

                // Queue the progress update; flush if the interval has elapsed.
                if batcher.submit(download_id, network_bytes, "downloading") {
                    if let Err(e) = flush_progress_blocking(batcher, storage).await {
                        warn!(
                            download_id,
                            bytes = network_bytes,
                            "blob-transfer: progress persist failed: {e:#}"
                        );
                    }
                }

                on_progress(BlobTransferProgress::Progress {
                    bytes_received: network_bytes,
                    total_bytes,
                });

                // Emit telemetry checkpoint (rate-limited by progress interval).
                if last_checkpoint.elapsed() >= checkpoint_interval {
                    TRANSFER_TELEMETRY.progress_checkpoint(
                        download_id,
                        network_bytes,
                        total_bytes,
                        None,
                        Some(checkpoint_interval.as_millis() as u64),
                        None,
                    );
                    last_checkpoint = Instant::now();
                }
            }
            iroh_blobs::api::downloader::DownloadProgressItem::Error(e) => {
                let msg = format!("blob-transfer: download error: {e}");
                error!(download_id, "{msg}");
                on_progress(BlobTransferProgress::Failed { error: msg.clone() });
                fail_download_blocking(storage, download_id, &msg).await?;
                return Err(anyhow::anyhow!("{msg}"));
            }
            iroh_blobs::api::downloader::DownloadProgressItem::DownloadError => {
                let msg = "blob-transfer: download error".to_string();
                error!(download_id, "{msg}");
                on_progress(BlobTransferProgress::Failed { error: msg.clone() });
                fail_download_blocking(storage, download_id, &msg).await?;
                return Err(anyhow::anyhow!("{msg}"));
            }
            // Ignore TryProvider, ProviderFailed, PartComplete
            _ => {}
        }
    }
}
