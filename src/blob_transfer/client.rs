//! Public `request_and_transfer_blob` — an end-to-end download that first
//! requests permission, verifies/accepts the response, then transfers the
//! blob to a temp file.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::Result;

use super::{transfer_blob_to_temp, BlobTransferConfig, BlobTransferProgress};
use crate::chat_core::TRANSFER_TELEMETRY;
use crate::download_limits::DownloadLimiter;
use crate::storage::Storage;

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
    TRANSFER_TELEMETRY.access_requested(download_id, "initial");
    let response =
        crate::file_access_client::request_download_permission(client_ep, server_pk, request)
            .await
            .map_err(|e| {
                // Map request-level errors to failure telemetry before propagating.
                let (category, retryable) = match &e {
                    crate::file_access_client::FileAccessRequestError::ConnectionFailed {
                        ..
                    } => (crate::diagnostics::ErrorCategory::PeerUnavailable, true),
                    crate::file_access_client::FileAccessRequestError::Timeout => {
                        (crate::diagnostics::ErrorCategory::Timeout, true)
                    }
                    crate::file_access_client::FileAccessRequestError::ProtocolError { .. } => {
                        (crate::diagnostics::ErrorCategory::ProtocolError, false)
                    }
                    crate::file_access_client::FileAccessRequestError::ServerError(_) => {
                        (crate::diagnostics::ErrorCategory::Unknown, false)
                    }
                };
                TRANSFER_TELEMETRY.failure(
                    download_id,
                    category,
                    retryable,
                    None,
                    Some(retryable),
                    None,
                );
                anyhow::anyhow!("permission request failed: {e}")
            })?;

    // ── 2. Verify and accept the response ───────────────────────────
    // The expected owner is the peer we selected for this request
    // (`server_pk`, passed down from the connection/request state), never a
    // key reconstructed from the response itself.  The expected content hash
    // is the request's canonical raw bytes — the same value the server must
    // have signed into the descriptor's `blob_hash`.
    //
    // `handle_permission_response` transitions the download row in SQLite;
    // run it on the blocking pool so the async worker is never stalled
    // (BORU-AUDIT-18).
    let server_pk_owned = server_pk;
    let local_pk_owned = *local_pk;
    let response_owned = response;
    let expected_hash_owned = request.expected_content_hash;
    let descriptor = storage
        .run_blocking("blob_transfer.handle_permission_response", move |s| {
            crate::file_access_client::handle_permission_response(
                s,
                download_id,
                response_owned,
                &server_pk_owned,
                &local_pk_owned,
                expected_hash_owned,
                expected_size,
            )
            .map_err(|e| n0_error::anyerr!("handle permission response: {e:#}"))
        })
        .await?
        .ok_or_else(|| anyhow::anyhow!("permission denied or retryable error"))?;

    // Grant TTL: not_after - current time, or None if unknown.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let expires_at_ms = descriptor.expires_at_ms;
    let grant_ttl_ms = if expires_at_ms > now_ms {
        Some(expires_at_ms - now_ms)
    } else {
        None
    };
    TRANSFER_TELEMETRY.access_granted(download_id, grant_ttl_ms);

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
