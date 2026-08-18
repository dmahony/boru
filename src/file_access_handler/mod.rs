//! File access (download-authorisation) protocol handler — server side.
//!
//! Implements [`ProtocolHandler`](iroh::protocol::ProtocolHandler) for the `/boru-file-access/1` ALPN.
//! On each incoming connection:
//!
//! 1. Authenticate the requester via [`Connection::remote_id()`](iroh::endpoint::Connection::remote_id).
//! 2. Deserialise and validate the [`FileAccessWireRequest`](crate::file_access_protocol::FileAccessWireRequest).
//! 3. Perform a **request-time** permission, availability, and integrity
//!    check against the current database state.
//! 4. Issue a `SignedDownloadDescriptor` (short-lived) or return the
//!    appropriate refusal variant.
//!
//! The handler never relies on cached catalogue state — every request is
//! checked against live database state so stale catalogues cannot grant
//! stale access.
//!
//! This is the I/O-orchestration facade (BORU-CORE-003): it owns the
//! [`FileAccessHandler`] struct, its construction, and the QUIC
//! accept/serve loop.  Architectural concerns live in focused submodules:
//!
//! - [`nonce`] — single-use descriptor nonce / replay protection (authorization)
//! - [`limits`] — preparation + upload admission bounds (transfer-request handling)
//! - [`policy`] — request-time permission & integrity policy
//! - [`prepare`] — path resolution/containment + I/O preparation

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use tracing::{debug, warn};

use crate::file_access_protocol::{
    FileAccessErrorCode, FileAccessWireRequest, FileAccessWireResponse,
};
use crate::friends::FriendsStore;
use crate::storage::Storage;

// ── Submodules ─────────────────────────────────────────────
mod limits;
mod nonce;
mod policy;
mod prepare;
#[cfg(test)]
mod tests;

// ── Re-exports (public facade, BORU-CORE-003) ─────────────
pub use limits::*;
pub use nonce::*;
pub use prepare::*;

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
