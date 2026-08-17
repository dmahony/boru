//! Blob transfer execution — downloading blobs from the iroh network with
//! progress reporting, safety admission control, and atomic destination
//! writes.  Extracted from `chat_core` so transfer behaviour can be tested
//! independently of protocol dispatch and storage.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use iroh::{Endpoint, EndpointAddr, PublicKey};
use n0_error::Result;
use n0_future::StreamExt;

use crate::chat_callbacks::{TransferId, TransferKind, TransferProgress};
use crate::public_room_safety::PublicRoomSafety;

/// Download an announced file offer into a reserved destination while
/// emitting the same progress lifecycle used by blob downloads.
#[allow(clippy::too_many_arguments)]
pub async fn download_file_offer_to_file(
    endpoint: &Endpoint,
    owner: PublicKey,
    offer_id: crate::chat_core::protocol::FileOfferId,
    name: String,
    kind: TransferKind,
    destination: &mut crate::safe_destination::ReservedDestination,
    on_progress: impl FnMut(TransferProgress) + Send + 'static,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let id = TransferId::next();
    let shared_cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(on_progress))));
    let emit = |event: TransferProgress| {
        if let Ok(mut guard) = shared_cb.lock() {
            if let Some(callback) = guard.as_mut() {
                callback(event);
            }
        }
    };
    emit(TransferProgress::Started { id, kind, name: name.clone(), total: None });
    let cancel_guard = CancelGuard::new(id, kind, name.clone(), shared_cb.clone());
    let mut transfer = match crate::file_offer_protocol::open_file_offer(
        endpoint, EndpointAddr::new(owner), offer_id,
    ).await {
        Ok(transfer) => transfer,
        Err(error) => {
            cancel_guard.disarm();
            tracing::error!(
                target: "boru::file_offer",
                ?offer_id,
                %error,
                "direct file offer open failed (receiver)"
            );
            emit(TransferProgress::Failed { id, name, error: error.to_string() });
            return Err(n0_error::anyerr!("direct file offer failed: {error}"));
        }
    };
    if transfer.header.offer_id != offer_id || transfer.header.name != name {
        cancel_guard.disarm();
        tracing::error!(
            target: "boru::file_offer",
            ?offer_id,
            header_offer = ?transfer.header.offer_id,
            header_name = %transfer.header.name,
            expected_name = %name,
            "direct transfer header mismatch (receiver)"
        );
        emit(TransferProgress::Failed { id, name, error: "direct transfer header does not match the requested offer".into() });
        return Err(n0_error::anyerr!("direct transfer header mismatch"));
    }
    let expected_size = transfer.header.size;
    let std_file = match destination.take_file() {
        Some(file) => file,
        None => {
            cancel_guard.disarm();
            tracing::error!(
                target: "boru::file_offer",
                ?offer_id,
                "reserved destination already consumed (receiver)"
            );
            emit(TransferProgress::Failed { id, name, error: "reserved destination already consumed".into() });
            return Err(n0_error::anyerr!("reserved destination already consumed"));
        }
    };
    let mut file = tokio::fs::File::from_std(std_file);
    let mut hasher = blake3::Hasher::new();
    let result: Result<()> = async {
        let mut buffer = vec![0u8; 256 * 1024];
        let mut received = 0u64;
        while received < expected_size {
            let read_len = (expected_size - received).min(buffer.len() as u64) as usize;
            let count = match transfer.read(&mut buffer[..read_len]).await {
                Ok(Some(count)) if count > 0 => count,
                Ok(_) => {
                    tracing::warn!(
                        target: "boru::file_offer",
                        ?offer_id,
                        received,
                        expected_size,
                        "direct transfer ended before advertised size (receiver)"
                    );
                    return Err(n0_error::anyerr!(
                        "direct transfer ended before advertised size"
                    ));
                }
                Err(error) => {
                    tracing::error!(
                        target: "boru::file_offer",
                        ?offer_id,
                        received,
                        expected_size,
                        %error,
                        "direct transfer read failed (receiver)"
                    );
                    return Err(n0_error::anyerr!("direct transfer read failed: {error}"));
                }
            };
            file.write_all(&buffer[..count]).await
                .map_err(|error| n0_error::anyerr!("write download destination: {error}"))?;
            hasher.update(&buffer[..count]);
            received += count as u64;
            emit(TransferProgress::Progress { id, kind, name: name.clone(), bytes: received, total: Some(expected_size) });
        }
        file.sync_all().await
            .map_err(|error| n0_error::anyerr!("sync download destination: {error}"))?;
        destination.restore_file(file.into_std().await);
        let _content_hash = hasher.finalize();
        Ok(())
    }.await;
    match result {
        Ok(()) => {
            cancel_guard.disarm();
            emit(TransferProgress::Completed { id, kind, name });
            Ok(())
        }
        Err(error) => {
            cancel_guard.disarm();
            tracing::error!(
                target: "boru::file_offer",
                ?offer_id,
                expected_size,
                %error,
                "direct offer download failed (receiver)"
            );
            emit(TransferProgress::Failed { id, name, error: error.to_string() });
            Err(error)
        }
    }
}

/// Shared progress callback used by blob download functions with progress tracking.
pub(crate) type TransferProgressCallback =
    Arc<Mutex<Option<Box<dyn FnMut(TransferProgress) + Send>>>>;

/// Build a list of blob download candidates: the original sender first, then
/// any online gossip neighbors (deduplicated).
///
/// Pass the result as the `providers` argument to
/// [`Downloader::download`][iroh_blobs::api::downloader::Downloader::download]
/// so the download can fall back to other peers that may have the blob
/// if the original sender is offline.
///
/// The original sender is always placed first so the primary peer is tried
/// before fallback candidates.
pub fn download_candidates(original: PublicKey, neighbors: &HashSet<PublicKey>) -> Vec<PublicKey> {
    let mut candidates: Vec<PublicKey> = Vec::with_capacity(neighbors.len() + 1);
    candidates.push(original);
    for n in neighbors {
        if *n != original {
            candidates.push(*n);
        }
    }
    candidates
}

// ── CancelGuard ──────────────────────────────────────────────────────────
//
// A RAII guard that emits TransferProgress::Cancelled via the shared callback
// wrapper when dropped without being disarmed first.
pub(crate) struct CancelGuard {
    callback: TransferProgressCallback,
    id: TransferId,
    kind: TransferKind,
    name: String,
    armed: bool,
}

impl CancelGuard {
    pub(crate) fn new(
        id: TransferId,
        kind: TransferKind,
        name: String,
        callback: TransferProgressCallback,
    ) -> Self {
        Self {
            callback,
            id,
            kind,
            name,
            armed: true,
        }
    }

    /// Disarm the guard so Drop does not emit Cancelled.
    pub(crate) fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Ok(mut guard) = self.callback.lock() {
                if let Some(cb) = guard.as_mut() {
                    cb(TransferProgress::Cancelled {
                        id: self.id,
                        kind: self.kind,
                        name: self.name.clone(),
                    });
                }
            }
        }
    }
}

/// Download a blob from the iroh network with observable progress events.
///
/// Wraps the iroh-blobs streaming download API to emit [`TransferProgress`]
/// events — `Started`, `Progress`, `Completed`, `Failed`, or `Cancelled` —
/// through the provided `on_progress` callback.  No events are emitted after
/// a terminal state (completed, failed, or cancelled).
///
/// `total` on `Progress` events is `None` because iroh-blobs does not expose
/// the total blob size before the download is complete.
///
/// If the future is dropped before the download finishes (e.g. the caller
/// cancels via a timeout, `select!`, or component unmount), a `Cancelled`
/// event is emitted automatically via the shared callback wrapper.
///
/// On success the blob bytes are returned.  The caller must ensure the blob
/// was actually stored (the stream only confirms the download completed).
///
/// When `max_bytes` is `Some(limit)` the download is aborted as soon as the
/// cumulative progress exceeds the limit — the stream is abandoned and the
/// partially-stored blob is never loaded into memory.  When `None` (the
/// private-room path) no size enforcement is applied.
#[expect(clippy::too_many_arguments)]
pub async fn download_blob_with_progress(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    hash: iroh_blobs::Hash,
    candidates: Vec<PublicKey>,
    name: String,
    kind: TransferKind,
    on_progress: impl FnMut(TransferProgress) + Send + 'static,
    max_bytes: Option<u64>,
) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let id = TransferId::next();

    // Wrap the callback in a shared Mutex so the CancelGuard can reach it.
    let shared_cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(on_progress))));

    // Helper: lock and call the callback.
    let emit = |ev: TransferProgress| {
        if let Ok(mut guard) = shared_cb.lock() {
            if let Some(cb) = guard.as_mut() {
                cb(ev);
            }
        }
    };

    emit(TransferProgress::Started {
        id,
        kind,
        name: name.clone(),
        total: None,
    });

    // Drop-guard: emit Cancelled if the future is dropped before
    // completion or failure.
    let cancel_guard = CancelGuard::new(id, kind, name.clone(), shared_cb.clone());

    let downloader = blob_store.downloader(endpoint);
    let progress = downloader.download(hash, candidates);

    // Stream the download progress items.
    let mut stream = progress.stream().await?;

    loop {
        use iroh_blobs::api::downloader::DownloadProgressItem;
        match stream.next().await {
            Some(DownloadProgressItem::Progress(n)) => {
                // Enforce streaming size cap before forwarding the event.
                if let Some(max) = max_bytes {
                    if n > max {
                        cancel_guard.disarm();
                        emit(TransferProgress::Failed {
                            id,
                            name,
                            error: format!("blob too large ({} bytes, limit {max} bytes)", n,),
                        });
                        return Err(n0_error::anyerr!(
                            "blob too large: {n} bytes, limit {max} bytes",
                        ));
                    }
                }
                emit(TransferProgress::Progress {
                    id,
                    kind,
                    name: name.clone(),
                    bytes: n,
                    total: None,
                });
            }
            Some(DownloadProgressItem::Error(e)) => {
                cancel_guard.disarm();
                emit(TransferProgress::Failed {
                    id,
                    name,
                    error: format!("{e}"),
                });
                return Err(e);
            }
            Some(DownloadProgressItem::DownloadError) => {
                cancel_guard.disarm();
                emit(TransferProgress::Failed {
                    id,
                    name,
                    error: "Download error".into(),
                });
                return Err(n0_error::anyerr!("Download error"));
            }
            Some(_) => {
                // Ignore TryProvider, ProviderFailed, PartComplete
            }
            None => {
                // Stream ended → download completed successfully.
                break;
            }
        }
    }

    cancel_guard.disarm();
    emit(TransferProgress::Completed {
        id,
        kind,
        name: name.clone(),
    });

    // Read back the blob.
    let mut reader = blob_store.blobs().reader(hash);
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Download a blob with progress reporting, streaming directly into an
/// already-reserved destination file.
///
/// The blob is downloaded to the local store, then streamed from the store
/// into the reserved destination handle in fixed-size chunks.  All writes go
/// through the handle that
/// [`reserve_download_destination`](crate::safe_destination::reserve_download_destination)
/// created — the destination path is never reopened, so the
/// time-of-check/time-of-use gap (BORU-AUDIT-21) is closed: a path that was
/// checked cannot be swapped for a symlink or an existing file before the
/// download writes to it.  The BLAKE3 hash of the written bytes is computed
/// as the copy proceeds and compared against `expected_content_hash` when
/// supplied, so a corrupted transfer fails before the destination is
/// published (the caller drops the reservation and the created file is
/// removed).
///
/// Progress events (`TransferProgress`) are emitted via `on_progress`.
#[allow(clippy::too_many_arguments)]
pub async fn download_blob_to_file(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    hash: iroh_blobs::Hash,
    candidates: Vec<PublicKey>,
    name: String,
    kind: TransferKind,
    destination: &mut crate::safe_destination::ReservedDestination,
    expected_content_hash: Option<&str>,
    on_progress: impl FnMut(TransferProgress) + Send + 'static,
    max_bytes: Option<u64>,
) -> Result<()> {
    let id = TransferId::next();
    let shared_cb: TransferProgressCallback = Arc::new(Mutex::new(Some(Box::new(on_progress))));
    let mut emit = |ev: TransferProgress| {
        if let Ok(mut guard) = shared_cb.lock() {
            if let Some(cb) = guard.as_mut() {
                cb(ev);
            }
        }
    };
    emit(TransferProgress::Started {
        id,
        kind,
        name: name.clone(),
        total: None,
    });
    let cancel_guard = CancelGuard::new(id, kind, name.clone(), shared_cb.clone());

    // Phase 1: download to the local blob store
    let downloader = blob_store.downloader(endpoint);
    let progress = downloader.download(hash, candidates);
    let mut stream = progress.stream().await?;
    use iroh_blobs::api::downloader::DownloadProgressItem;
    loop {
        match stream.next().await {
            Some(DownloadProgressItem::Progress(n)) => {
                if let Some(max) = max_bytes {
                    if n > max {
                        emit(TransferProgress::Failed {
                            id,
                            name: name.clone(),
                            error: format!("blob too large ({} bytes, limit {} bytes)", n, max),
                        });
                        return Err(n0_error::anyerr!("blob too large"));
                    }
                }
                emit(TransferProgress::Progress {
                    id,
                    kind,
                    name: name.clone(),
                    bytes: n,
                    total: None,
                });
            }
            Some(DownloadProgressItem::Error(e)) => {
                cancel_guard.disarm();
                emit(TransferProgress::Failed {
                    id,
                    name: name.clone(),
                    error: format!("{e}"),
                });
                return Err(e);
            }
            Some(DownloadProgressItem::DownloadError) => {
                cancel_guard.disarm();
                emit(TransferProgress::Failed {
                    id,
                    name: name.clone(),
                    error: "Download error".into(),
                });
                return Err(n0_error::anyerr!("Download error"));
            }
            None => break,
            _ => {}
        }
    }
    cancel_guard.disarm();

    // Phase 2: stream from the local store into the reserved destination
    // handle.  Never reopens the path (BORU-AUDIT-21); verifies the content
    // hash before the caller publishes the destination.
    write_blob_to_reserved_file(
        blob_store,
        hash,
        destination,
        expected_content_hash,
        max_bytes,
        &mut emit,
        id,
        kind,
        name.clone(),
    )
    .await?;

    emit(TransferProgress::Completed { id, kind, name });
    Ok(())
}

/// Stream a blob from the local store into a reserved destination handle,
/// computing the BLAKE3 hash of the written bytes as the copy proceeds.
///
/// The destination was created atomically by
/// [`reserve_download_destination`](crate::safe_destination::reserve_download_destination);
/// this function writes exclusively through the returned handle (wrapped in
/// `tokio::fs::File` so blocking I/O is offloaded) and never reopens the
/// path.  On success the file is synced to disk and the handle is restored
/// into the reservation so its drop-cleanup can still run.  On hash mismatch
/// or size overflow the function returns an error; the caller drops the
/// reservation and the created file is removed.
pub(crate) async fn write_blob_to_reserved_file(
    blob_store: &iroh_blobs::api::Store,
    hash: iroh_blobs::Hash,
    destination: &mut crate::safe_destination::ReservedDestination,
    expected_content_hash: Option<&str>,
    max_bytes: Option<u64>,
    on_progress: &mut impl FnMut(TransferProgress),
    id: TransferId,
    kind: TransferKind,
    name: String,
) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let std_file = destination
        .take_file()
        .ok_or_else(|| n0_error::anyerr!("reserved destination already consumed"))?;
    let mut file = tokio::fs::File::from_std(std_file);

    let mut reader = blob_store.blobs().reader(hash);
    let mut buf = vec![0u8; 256 * 1024];
    let mut hasher = blake3::Hasher::new();
    let mut total: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .await
            .map_err(|e| n0_error::anyerr!("read blob from store: {e}"))?;
        if n == 0 {
            break;
        }
        if let Some(max) = max_bytes {
            let written = total + n as u64;
            if written > max {
                return Err(n0_error::anyerr!(
                    "blob too large: {written} bytes, limit {max} bytes"
                ));
            }
        }
        file.write_all(&buf[..n])
            .await
            .map_err(|e| n0_error::anyerr!("write download destination: {e}"))?;
        hasher.update(&buf[..n]);
        total += n as u64;
        on_progress(TransferProgress::Progress {
            id,
            kind,
            name: name.clone(),
            bytes: total,
            total: Some(total),
        });
    }

    // Durability: flush + fsync before the file is published.
    file.sync_all()
        .await
        .map_err(|e| n0_error::anyerr!("sync download destination: {e}"))?;
    // tokio's `into_std` flushes the file and returns the underlying handle
    // directly (no Result wrapper in this version).
    let std_file = file.into_std().await;
    destination.restore_file(std_file);

    let actual_hash = hasher.finalize().to_hex().to_string();
    if let Some(expected) = expected_content_hash {
        if actual_hash != expected.to_ascii_lowercase() {
            return Err(n0_error::anyerr!(
                "download content hash mismatch: expected {expected}, got {actual_hash}"
            ));
        }
    }

    Ok(())
}

/// Download a blob with public-room safety admission control and
/// blob-size enforcement.
///
/// When `safety` is `Some(...)`, this function first calls
/// [`PublicRoomSafety::try_acquire_download`] for the `original_sender`.
/// If the per-peer download queue is full, the function returns an error
/// without starting the download.  On completion the downloaded blob is
/// read back with a [`max_blob_size_bytes`] cap so objects exceeding the
/// configured limit are rejected without allocating beyond the cap.
/// On success or failure, [`PublicRoomSafety::release_download`] is called
/// to free the slot.
///
/// When `safety` is `None`, this is equivalent to
/// [`download_blob_with_progress`] (no size enforcement).
///
/// [`max_blob_size_bytes`]: crate::public_room_config::PublicRoomConfig::max_blob_size_bytes
#[expect(clippy::too_many_arguments)]
pub async fn download_blob_with_safety(
    blob_store: &iroh_blobs::api::Store,
    endpoint: &Endpoint,
    hash: iroh_blobs::Hash,
    candidates: Vec<PublicKey>,
    name: String,
    kind: TransferKind,
    on_progress: impl FnMut(TransferProgress) + Send + 'static,
    safety: Option<&PublicRoomSafety>,
    original_sender: PublicKey,
) -> Result<Vec<u8>> {
    // ── Admission control for public rooms ───────────────────────
    if let Some(s) = safety {
        if !s.try_acquire_download(&original_sender) {
            tracing::debug!(
                "safety: download from {} rejected (queue full)",
                original_sender.fmt_short(),
            );
            return Err(n0_error::anyerr!(
                "download queue full for peer {}",
                original_sender.fmt_short()
            ));
        }
    }

    let max_size = safety.map(|s| s.config().max_blob_size_bytes as u64);
    let result = download_blob_with_progress(
        blob_store,
        endpoint,
        hash,
        candidates,
        name,
        kind,
        on_progress,
        max_size,
    )
    .await;

    // ── Release the download slot ───────────────────────────────
    if let Some(s) = safety {
        s.release_download(&original_sender);
    }

    // ── Blob size enforcement ──────────────────────────────
    if let Some(s) = safety {
        if let Ok(ref bytes) = result {
            if !s.check_blob_size(bytes.len()) {
                let max = s.config().max_blob_size_bytes;
                tracing::debug!(
                    "safety: blob from {} exceeds size limit ({} > {})",
                    original_sender.fmt_short(),
                    bytes.len(),
                    max,
                );
                return Err(n0_error::anyerr!(
                    "blob from {} exceeds size limit ({} > {} bytes)",
                    original_sender.fmt_short(),
                    bytes.len(),
                    max,
                ));
            }
        }
    }

    result
}

