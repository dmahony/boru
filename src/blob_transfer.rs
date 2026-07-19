//! Blob transfer — downloads a blob from the iroh network to a temporary file
//! using the actual installed iroh-blobs API.
//!
//! # Design
//!
//! - Streams bytes directly to disk — never loads the full file into memory.
//! - Reports progress via a callback.
//! - Supports cancellation via a shared `AtomicBool` flag.
//! - Enforces a per-transfer timeout and a per-chunk network timeout.
//! - Persists progress periodically to the database via a [`ProgressUpdateGate`].
//! - Temporary file is cleaned up on error or cancellation.
//!
//! # Flow
//!
//! 1. Download the blob into the iroh-blobs local store (network I/O, streamed
//!    by iroh-blobs directly).
//! 2. Stream the completed blob from the local store to the temporary file
//!    using a bounded buffer (128 KiB chunks), computing the BLAKE3 hash
//!    incrementally.
//! 3. Verify size + hash match the [`SignedDownloadDescriptor`].
//! 4. Record the temp path in the database for crash recovery.
//! 5. Report progress every chunk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::Endpoint;
use iroh_base::PublicKey;
use n0_future::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info, warn};

use crate::download_limits::{DownloadLimiter, ProgressUpdateGate};
use crate::file_access_protocol::SignedDownloadDescriptor;
use crate::storage::Storage;

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
const COPY_BUF_SIZE: usize = 128 * 1024; // 128 KiB

// ── Transfer timeout seconds (chunk-level) ─────────────────────────────────

/// Seconds without progress on a blob-read stream before aborting the copy.
const READ_TIMEOUT_SECS: u64 = 30;

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
    let blob_hash: iroh_blobs::Hash = descriptor
        .content_hash
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid blob hash in descriptor: {e}"))?;

    // ── 1. Record temp path in storage (crash recovery) ─────────────────
    let temp_str = temp_path.to_string_lossy().to_string();
    storage.set_download_temp_path(download_id, &temp_str)?;
    debug!(download_id, path = %temp_str, "blob-transfer: recorded temp path");

    on_progress(BlobTransferProgress::Started { total_bytes });
    crate::chat_core::DIAGNOSTICS.record(
        None,
        crate::diagnostics::DiagnosticEventKind::TransferStarted {
            transfer_id: crate::diagnostics::short_transfer_id(download_id),
            total_bytes,
        },
    );

    // ── 2. Acquire download slot from the limiter ──────────────────────
    let peer_str = descriptor.owner_id.to_string();
    let queued = limiter
        .try_enqueue(&peer_str)
        .map_err(|e| anyhow::anyhow!("blob-transfer: admission failed: {e:?}"))?;
    let _active = queued
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("blob-transfer: start failed: {e:?}"))?;

    let progress_gate = ProgressUpdateGate::new(config.progress_persist_interval);

    // ── 3. Stage A: Network download into blob store ──────────────────
    let result = stage_network_download(
        blob_store,
        endpoint,
        blob_hash,
        &providers,
        &cancel_flag,
        &config,
        &progress_gate,
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

    // ── 4. Stage B: Copy from blob store to temp file + hash ───────────
    let result = stage_copy_to_temp(
        blob_store,
        blob_hash,
        &temp_path,
        total_bytes,
        &cancel_flag,
        &config,
        &progress_gate,
        storage,
        download_id,
        network_bytes,
        &mut on_progress,
    )
    .await;

    let (bytes_written, hash_hex) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(e);
        }
    };

    // ── 5. Verify size ─────────────────────────────────────────────────
    crate::chat_core::DIAGNOSTICS.record(
        None,
        crate::diagnostics::DiagnosticEventKind::TransferVerification {
            transfer_id: crate::diagnostics::short_transfer_id(download_id),
            bytes: bytes_written,
            total_bytes,
            success: false,
        },
    );
    if bytes_written != total_bytes {
        let _ = tokio::fs::remove_file(&temp_path).await;
        let msg = format!(
            "blob-transfer: size mismatch after copy: wrote {bytes_written}, expected {total_bytes}"
        );
        error!(download_id, "{msg}");
        storage.fail_download(download_id, &msg, None)?;
        on_progress(BlobTransferProgress::Failed { error: msg.clone() });
        return Err(anyhow::anyhow!("{msg}"));
    }

    // ── 6. Verify BLAKE3 hash ──────────────────────────────────────────
    let expected_hex = hex::encode(descriptor.content_hash.clone());
    if hash_hex != expected_hex {
        let _ = tokio::fs::remove_file(&temp_path).await;
        let msg = format!(
            "blob-transfer: content hash mismatch: computed {hash_hex}, expected {expected_hex}"
        );
        error!(download_id, "{msg}");
        storage.fail_download(download_id, &msg, None)?;
        on_progress(BlobTransferProgress::Failed { error: msg.clone() });
        return Err(anyhow::anyhow!("{msg}"));
    }

    crate::chat_core::DIAGNOSTICS.record(
        None,
        crate::diagnostics::DiagnosticEventKind::TransferVerification {
            transfer_id: crate::diagnostics::short_transfer_id(download_id),
            bytes: bytes_written,
            total_bytes,
            success: true,
        },
    );
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

// ── Stage A: Network download ────────────────────────────────────────────

/// Download the blob into the iroh-blobs local store and emit progress
/// events as bytes arrive.
#[allow(clippy::too_many_arguments)]
async fn stage_network_download(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    blob_hash: iroh_blobs::Hash,
    providers: &[PublicKey],
    cancel_flag: &AtomicBool,
    config: &BlobTransferConfig,
    progress_gate: &ProgressUpdateGate,
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
            storage.fail_download(download_id, &msg, None)?;
            return Err(anyhow::anyhow!("{msg}"));
        }

        // ── Read next progress item (with per-chunk timeout) ────────
        let item = tokio::time::timeout(config.chunk_timeout, stream.next())
            .await
            .context("blob-transfer: chunk read timed out")?;

        let Some(item) = item else {
            // Stream ended — network download completed successfully.
            return Ok(network_bytes);
        };

        match item {
            iroh_blobs::api::downloader::DownloadProgressItem::Progress(n) => {
                network_bytes = n;

                // Persist progress periodically.
                if progress_gate.should_persist(Instant::now()) {
                    if let Err(e) =
                        storage.update_download_progress(download_id, network_bytes, "downloading")
                    {
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
            }
            iroh_blobs::api::downloader::DownloadProgressItem::Error(e) => {
                let msg = format!("blob-transfer: download error: {e}");
                error!(download_id, "{msg}");
                on_progress(BlobTransferProgress::Failed { error: msg.clone() });
                storage.fail_download(download_id, &msg, None)?;
                return Err(anyhow::anyhow!("{msg}"));
            }
            iroh_blobs::api::downloader::DownloadProgressItem::DownloadError => {
                let msg = "blob-transfer: download error".to_string();
                error!(download_id, "{msg}");
                on_progress(BlobTransferProgress::Failed { error: msg.clone() });
                storage.fail_download(download_id, &msg, None)?;
                return Err(anyhow::anyhow!("{msg}"));
            }
            // Ignore TryProvider, ProviderFailed, PartComplete
            _ => {}
        }
    }
}

// ── Stage B: Copy from blob store to temp file ───────────────────────────

/// Stream the blob from the local iroh-blobs store to a temporary file,
/// computing the BLAKE3 hash as we go.
#[allow(clippy::too_many_arguments)]
async fn stage_copy_to_temp(
    blob_store: &iroh_blobs::api::Store,
    blob_hash: iroh_blobs::Hash,
    temp_path: &Path,
    total_bytes: u64,
    cancel_flag: &AtomicBool,
    config: &BlobTransferConfig,
    progress_gate: &ProgressUpdateGate,
    storage: &Storage,
    download_id: i64,
    initial_bytes: u64,
    on_progress: &mut impl FnMut(BlobTransferProgress),
) -> Result<(u64, String)> {
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
            storage.fail_download(download_id, &msg, None)?;
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

        // ── Persist progress periodically ─────────────────────────
        if progress_gate.should_persist(Instant::now()) {
            let total_received = initial_bytes + bytes_written;
            if let Err(e) =
                storage.update_download_progress(download_id, total_received, "downloading")
            {
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
    let _ = progress_gate.should_persist(std::time::Instant::now());
    let total_received = initial_bytes + bytes_written;
    if let Err(e) = storage.update_download_progress(download_id, total_received, "downloading") {
        warn!(
            download_id,
            bytes = total_received,
            "blob-transfer: final progress persist failed: {e:#}"
        );
    }

    let hash_hex = hasher.finalize().to_hex().to_string();
    Ok((bytes_written, hash_hex))
}

// ── Cancellable download with acceptance ───────────────────────────────────

/// Convenience wrapper: request permission, verify response, then transfer
/// the blob to a temp file.
///
/// This is the top-level entry point for a single-file download.  It
/// combines the three-step flow:
///
/// 1. Request permission via [`request_download_permission`].
/// 2. Verify and accept the response via [`handle_permission_response`].
/// 3. Stream the blob to a temp file via [`transfer_blob_to_temp`].
///
/// [`request_download_permission`]: crate::file_access_client::request_download_permission
/// [`handle_permission_response`]: crate::file_access_client::handle_permission_response
#[allow(clippy::too_many_arguments)]
pub async fn request_and_transfer_blob(
    client_ep: &iroh::Endpoint,
    server_pk: iroh::PublicKey,
    providers: Vec<iroh::PublicKey>,
    blob_store: &iroh_blobs::api::Store,
    request: &crate::file_access_protocol::FileAccessRequest,
    temp_path: PathBuf,
    storage: &Storage,
    download_id: i64,
    local_pk: &iroh::PublicKey,
    expected_size: u64,
    limiter: &DownloadLimiter,
    cancel_flag: Arc<AtomicBool>,
    config: BlobTransferConfig,
    on_progress: impl FnMut(BlobTransferProgress),
) -> Result<PathBuf> {
    // ── 1. Request permission ───────────────────────────────────────
    let response =
        crate::file_access_client::request_download_permission(client_ep, server_pk, request)
            .await
            .map_err(|e| anyhow::anyhow!("permission request failed: {e}"))?;

    // ── 2. Verify and accept the response ───────────────────────────
    let expected_content_hash_hex = hex::encode(request.expected_content_hash);
    let descriptor = crate::file_access_client::handle_permission_response(
        storage,
        download_id,
        response,
        local_pk,
        &expected_content_hash_hex,
        expected_size,
    )?
    .ok_or_else(|| anyhow::anyhow!("permission denied or retryable error"))?;

    // ── 3. Transfer the blob ────────────────────────────────────────
    transfer_blob_to_temp(
        blob_store,
        client_ep,
        &descriptor,
        providers,
        temp_path,
        storage,
        download_id,
        limiter,
        cancel_flag,
        config,
        on_progress,
    )
    .await
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_access_protocol::{sign_download_descriptor, BlobFormat};
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn test_config() -> BlobTransferConfig {
        BlobTransferConfig {
            transfer_timeout: Duration::from_secs(30),
            chunk_timeout: Duration::from_secs(5),
            progress_persist_interval: Duration::from_millis(100),
        }
    }

    /// Verify that transfer_blob_to_temp with in-memory iroh-blobs store
    /// successfully downloads and writes a small blob to a temp file.
    #[tokio::test]
    async fn transfer_small_blob_success() {
        let tmp = TempDir::new().unwrap();
        let temp_path = tmp.path().join("download.part");
        let storage = Storage::memory().unwrap();

        // Create a blob store with a known blob.
        let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
        let data = b"hello blob transfer";
        let expected_hash = blake3::hash(data);
        let expected_hash_bytes: [u8; 32] = *expected_hash.as_bytes();
        let _blob_hash = iroh_blobs::Hash::from(expected_hash_bytes);

        // Import the blob into the store.
        blob_store
            .blobs()
            .add_bytes(data.to_vec())
            .await
            .expect("add bytes");

        // Create a descriptor that matches this blob.
        let sk = iroh::SecretKey::generate();
        let pk = sk.public();
        let now = now_ms();
        let descriptor = sign_download_descriptor(
            &sk,
            pk,
            "test-file".into(),
            expected_hash_bytes,
            data.len() as u64,
            BlobFormat::Raw,
            now,
            now + 60_000,
        );

        let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig {
            max_active_downloads: 4,
            max_downloads_per_peer: 2,
            max_active_hash_verifications: 2,
            max_queued_downloads: 16,
            progress_update_interval: Duration::from_millis(100),
        });

        // Create a download row to track progress.
        // We need the storage to have a download in the RequestingPermission
        // state.  Use the pattern from file_access_client tests.
        storage
            .put_file_object(
                &hex::encode(expected_hash_bytes),
                data.len() as u64,
                "text/plain",
                "test.txt",
                data,
            )
            .expect("put file object");

        let download_id = storage
            .create_download(
                &hex::encode(expected_hash_bytes),
                &pk.to_string(),
                data.len() as u64,
            )
            .expect("create download");

        // Transition queued → requesting_permission via direct SQL
        // (the begin_download/mark_resume_peer_resolved pipeline was removed).
        storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
        storage
            .accept_resumed_descriptor(
                download_id,
                &hex::encode(expected_hash_bytes),
                data.len() as u64,
            )
            .expect("accept descriptor → downloading");

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let mut events = Vec::new();

        let result = transfer_blob_to_temp(
            &blob_store,
            // We need an endpoint for the downloader.  Since the blob is
            // already local, the downloader shouldn't need network I/O,
            // but it still requires an Endpoint handle.  Use a minimal
            // in-memory endpoint.
            &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(iroh::SecretKey::generate())
                .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
                .bind()
                .await
                .expect("bind endpoint"),
            &descriptor,
            vec![pk],
            temp_path.clone(),
            &storage,
            download_id,
            &limiter,
            cancel_flag,
            test_config(),
            |ev| events.push(ev),
        )
        .await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.err()
        );

        // Verify the temp file exists and has the right content.
        assert!(temp_path.exists(), "temp file should exist");
        let actual = std::fs::read(&temp_path).expect("read temp file");
        assert_eq!(actual, data, "content should match");

        // Verify progress events: Started, at least one Progress, Completed.
        let started = events
            .iter()
            .any(|e| matches!(e, BlobTransferProgress::Started { .. }));
        let completed = events
            .iter()
            .any(|e| matches!(e, BlobTransferProgress::Completed { .. }));
        assert!(started, "should have Started event");
        assert!(completed, "should have Completed event");
    }

    /// Verify that cancellation stops the transfer and cleans up the temp file.
    #[tokio::test]
    async fn transfer_cancellation_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let temp_path = tmp.path().join("cancel.part");
        let storage = Storage::memory().unwrap();

        let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
        let data = vec![0xABu8; 65_536]; // 64 KiB blob
        let expected_hash = blake3::hash(&data);
        let expected_hash_bytes: [u8; 32] = *expected_hash.as_bytes();

        blob_store
            .blobs()
            .add_bytes(data.clone())
            .await
            .expect("add bytes");

        let sk = iroh::SecretKey::generate();
        let pk = sk.public();
        let now = now_ms();
        let descriptor = sign_download_descriptor(
            &sk,
            pk,
            "cancel-test".into(),
            expected_hash_bytes,
            data.len() as u64,
            BlobFormat::Raw,
            now,
            now + 60_000,
        );

        let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig::default());

        storage
            .put_file_object(
                &hex::encode(expected_hash_bytes),
                data.len() as u64,
                "application/octet-stream",
                "cancel.bin",
                &data,
            )
            .expect("put file object");

        let download_id = storage
            .create_download(
                &hex::encode(expected_hash_bytes),
                &pk.to_string(),
                data.len() as u64,
            )
            .expect("create download");

        storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
        storage
            .accept_resumed_descriptor(
                download_id,
                &hex::encode(expected_hash_bytes),
                data.len() as u64,
            )
            .expect("accept");

        // Cancel immediately.
        let cancel_flag = Arc::new(AtomicBool::new(true));

        let result = transfer_blob_to_temp(
            &blob_store,
            &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(iroh::SecretKey::generate())
                .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
                .bind()
                .await
                .expect("bind endpoint"),
            &descriptor,
            vec![pk],
            temp_path.clone(),
            &storage,
            download_id,
            &limiter,
            cancel_flag,
            test_config(),
            |_ev| {},
        )
        .await;

        assert!(result.is_err(), "cancelled transfer should fail");
        // Temp file should be cleaned up.
        assert!(
            !temp_path.exists(),
            "temp file should be removed on cancellation"
        );
    }

    /// Verify that timeout aborts the transfer.
    #[tokio::test]
    async fn transfer_timeout_aborts() {
        let tmp = TempDir::new().unwrap();
        let temp_path = tmp.path().join("timeout.part");
        let storage = Storage::memory().unwrap();

        let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
        let data = vec![0xCDu8; 4096];
        let expected_hash = blake3::hash(&data);
        let expected_hash_bytes: [u8; 32] = *expected_hash.as_bytes();

        blob_store
            .blobs()
            .add_bytes(data.clone())
            .await
            .expect("add bytes");

        let sk = iroh::SecretKey::generate();
        let pk = sk.public();
        let now = now_ms();
        let descriptor = sign_download_descriptor(
            &sk,
            pk,
            "timeout-test".into(),
            expected_hash_bytes,
            data.len() as u64,
            BlobFormat::Raw,
            now,
            now + 60_000,
        );

        let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig::default());

        storage
            .put_file_object(
                &hex::encode(expected_hash_bytes),
                data.len() as u64,
                "application/octet-stream",
                "timeout.bin",
                &data,
            )
            .expect("put file object");

        let download_id = storage
            .create_download(
                &hex::encode(expected_hash_bytes),
                &pk.to_string(),
                data.len() as u64,
            )
            .expect("create download");

        storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
        storage
            .accept_resumed_descriptor(
                download_id,
                &hex::encode(expected_hash_bytes),
                data.len() as u64,
            )
            .expect("accept");

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let mut config = test_config();
        config.transfer_timeout = Duration::from_millis(1); // unrealistically short

        let result = transfer_blob_to_temp(
            &blob_store,
            &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(iroh::SecretKey::generate())
                .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
                .bind()
                .await
                .expect("bind endpoint"),
            &descriptor,
            vec![pk],
            temp_path.clone(),
            &storage,
            download_id,
            &limiter,
            cancel_flag,
            config,
            |_ev| {},
        )
        .await;

        assert!(result.is_err(), "timed-out transfer should fail");
        // Temp file should be cleaned up.
        assert!(
            !temp_path.exists(),
            "temp file should be removed on timeout"
        );
    }

    /// Verify that a wrong-size descriptor causes failure.
    #[tokio::test]
    async fn size_mismatch_rejected() {
        let tmp = TempDir::new().unwrap();
        let temp_path = tmp.path().join("size_mismatch.part");
        let storage = Storage::memory().unwrap();

        let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
        let data = b"actual content";
        let expected_hash = blake3::hash(data);
        let expected_hash_bytes: [u8; 32] = *expected_hash.as_bytes();

        blob_store
            .blobs()
            .add_bytes(data.to_vec())
            .await
            .expect("add bytes");

        let sk = iroh::SecretKey::generate();
        let pk = sk.public();
        let now = now_ms();
        // Deliberately wrong size:
        let descriptor = sign_download_descriptor(
            &sk,
            pk,
            "size-mismatch".into(),
            expected_hash_bytes,
            9999, // wrong size
            BlobFormat::Raw,
            now,
            now + 60_000,
        );

        let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig::default());

        storage
            .put_file_object(
                &hex::encode(expected_hash_bytes),
                9999,
                "application/octet-stream",
                "size_mismatch.bin",
                data,
            )
            .expect("put file object");

        let download_id = storage
            .create_download(&hex::encode(expected_hash_bytes), &pk.to_string(), 9999)
            .expect("create download");

        storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
        storage
            .accept_resumed_descriptor(download_id, &hex::encode(expected_hash_bytes), 9999)
            .expect("accept");

        let cancel_flag = Arc::new(AtomicBool::new(false));

        let result = transfer_blob_to_temp(
            &blob_store,
            &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(iroh::SecretKey::generate())
                .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
                .bind()
                .await
                .expect("bind endpoint"),
            &descriptor,
            vec![pk],
            temp_path.clone(),
            &storage,
            download_id,
            &limiter,
            cancel_flag,
            test_config(),
            |_ev| {},
        )
        .await;

        assert!(result.is_err(), "size mismatch should fail");
        // Verify the download state in storage is 'failed'.
        let download = storage.get_download(download_id).unwrap().unwrap();
        assert_eq!(download.state, "failed", "should be marked as failed");
    }

    /// Verify that a wrong-content-hash descriptor causes failure.
    #[tokio::test]
    async fn hash_mismatch_rejected() {
        let tmp = TempDir::new().unwrap();
        let temp_path = tmp.path().join("hash_mismatch.part");
        let storage = Storage::memory().unwrap();

        let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
        let data = b"actual content";
        let data_hash = blake3::hash(data);
        let _data_hash_bytes: [u8; 32] = *data_hash.as_bytes();

        blob_store
            .blobs()
            .add_bytes(data.to_vec())
            .await
            .expect("add bytes");

        let sk = iroh::SecretKey::generate();
        let pk = sk.public();
        let now = now_ms();
        // Wrong content hash in descriptor:
        let wrong_hash = [0xBBu8; 32];
        let descriptor = sign_download_descriptor(
            &sk,
            pk,
            "hash-mismatch".into(),
            wrong_hash,
            data.len() as u64,
            BlobFormat::Raw,
            now,
            now + 60_000,
        );

        let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig::default());

        storage
            .put_file_object(
                &hex::encode(wrong_hash),
                data.len() as u64,
                "application/octet-stream",
                "hash_mismatch.bin",
                data,
            )
            .expect("put file object");

        let download_id = storage
            .create_download(&hex::encode(wrong_hash), &pk.to_string(), data.len() as u64)
            .expect("create download");

        storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
        storage
            .accept_resumed_descriptor(download_id, &hex::encode(wrong_hash), data.len() as u64)
            .expect("accept");

        let cancel_flag = Arc::new(AtomicBool::new(false));

        let result = transfer_blob_to_temp(
            &blob_store,
            &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(iroh::SecretKey::generate())
                .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
                .bind()
                .await
                .expect("bind endpoint"),
            &descriptor,
            vec![pk],
            temp_path.clone(),
            &storage,
            download_id,
            &limiter,
            cancel_flag,
            test_config(),
            |_ev| {},
        )
        .await;

        assert!(result.is_err(), "hash mismatch should fail");
    }
}
