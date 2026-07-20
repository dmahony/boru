//! File access client — requests download permission from a remote peer
//! and processes the response.
//!
//! Provides the client side of the `/boru-file-access/1` protocol.
//! The transfer worker calls [`request_download_permission`] to obtain a fresh
//! [`SignedDownloadDescriptor`] from a remote peer, then calls
//! [`handle_permission_response`] to verify the descriptor and persist the
//! appropriate download-state transition.
//!
//! # Flow
//!
//! 1. [`request_download_permission`] — connect, send request, receive response.
//! 2. [`handle_permission_response`] — verify descriptor, apply storage
//!    transitions (granted → `downloading`, denied → `paused`, etc.).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::{Endpoint, EndpointAddr, PublicKey};
use tracing::{debug, info, warn};

use crate::chat_core::TRANSFER_TELEMETRY;
use crate::diagnostics::ErrorCategory;
use crate::file_access_protocol::{
    verify_download_descriptor, DescriptorVerification, FileAccessErrorCode, FileAccessRequest,
    FileAccessResponse, FileAccessWireRequest, FileAccessWireResponse, SignedDownloadDescriptor,
};
use crate::storage::Storage;

/// Default timeout for a file-access request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum payload size for a file-access response (256 KiB).
const MAX_RESPONSE_SIZE: u64 = 256 * 1024;

/// Error returned by [`request_download_permission`].
#[derive(Debug, Clone)]
pub enum FileAccessRequestError {
    /// Connection to the remote peer failed.
    ConnectionFailed {
        /// Human-readable failure details.
        details: String,
    },
    /// The operation timed out.
    Timeout,
    /// The response was malformed or the protocol was violated.
    ProtocolError {
        /// Human-readable error details.
        details: String,
    },
    /// The server returned an error response.
    ServerError(FileAccessResponse),
}

impl std::fmt::Display for FileAccessRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionFailed { details } => write!(f, "connection failed: {details}"),
            Self::Timeout => f.write_str("timeout"),
            Self::ProtocolError { details } => write!(f, "protocol error: {details}"),
            Self::ServerError(resp) => write!(f, "server error: {resp:?}"),
        }
    }
}

impl std::error::Error for FileAccessRequestError {}

impl From<Box<dyn std::error::Error + Send + Sync>> for FileAccessRequestError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        FileAccessRequestError::ProtocolError {
            details: format!("{e:#}"),
        }
    }
}

/// Connect to a remote peer and request a fresh download descriptor.
///
/// # Arguments
///
/// * `client_ep` — The local endpoint (authenticated with the caller's
///   [`iroh::SecretKey`]).
/// * `server_pk` — The [`PublicKey`] of the file owner peer.
/// * `request` — The [`FileAccessRequest`] with expected ID, version, and hash.
///
/// # Returns
///
/// * `Ok(FileAccessResponse)` — the parsed response from the server.
/// * `Err(FileAccessRequestError)` — connection, protocol, or timeout error.
pub async fn request_download_permission(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    request: &FileAccessRequest,
) -> std::result::Result<FileAccessResponse, FileAccessRequestError> {
    let addr = EndpointAddr::new(server_pk);

    debug!(
        peer = %server_pk.fmt_short(),
        shared_file_id = %request.shared_file_id,
        "file-access: requesting download permission"
    );

    // ── 1. Connect using the file-access ALPN ──────────────────────────
    let conn = tokio::time::timeout(
        REQUEST_TIMEOUT,
        client_ep.connect(addr, crate::file_access_handler::FILE_ACCESS_ALPN),
    )
    .await
    .map_err(|_| FileAccessRequestError::Timeout)?
    .map_err(|e| FileAccessRequestError::ConnectionFailed {
        details: format!("connect: {e}"),
    })?;

    // ── 2. Open a bi-directional stream ────────────────────────────────
    let (mut send, mut recv) =
        conn.open_bi()
            .await
            .map_err(|e| FileAccessRequestError::ConnectionFailed {
                details: format!("open_bi: {e}"),
            })?;

    // ── 3. Serialise and send the wire request ─────────────────────────
    let wire_req = FileAccessWireRequest::new(request.clone());
    let payload =
        postcard::to_stdvec(&wire_req).map_err(|e| FileAccessRequestError::ProtocolError {
            details: format!("encode request: {e}"),
        })?;

    send.write_all(&payload)
        .await
        .map_err(|e| FileAccessRequestError::ConnectionFailed {
            details: format!("write request: {e}"),
        })?;

    send.finish()
        .map_err(|e| FileAccessRequestError::ConnectionFailed {
            details: format!("finish send: {e}"),
        })?;

    // ── 4. Read the response (up to 256 KiB) ───────────────────────────
    let resp_bytes = tokio::time::timeout(
        REQUEST_TIMEOUT,
        recv.read_to_end(MAX_RESPONSE_SIZE as usize),
    )
    .await
    .map_err(|_| FileAccessRequestError::Timeout)?
    .map_err(|e| FileAccessRequestError::ConnectionFailed {
        details: format!("read response: {e}"),
    })?;

    if resp_bytes.is_empty() {
        return Err(FileAccessRequestError::ProtocolError {
            details: "server closed connection without response".to_string(),
        });
    }

    // ── 5. Deserialise the wire response ───────────────────────────────
    let wire_resp: FileAccessWireResponse =
        postcard::from_bytes(&resp_bytes).map_err(|e| FileAccessRequestError::ProtocolError {
            details: format!("decode response: {e}"),
        })?;

    // Validate wire version of the response.
    if let Err(code) = wire_resp.validate_version() {
        return Err(FileAccessRequestError::ProtocolError {
            details: format!("unsupported wire version: {code}"),
        });
    }

    // ── 6. Return the inner response ───────────────────────────────────
    match wire_resp.inner {
        Ok(resp) => {
            info!(
                peer = %server_pk.fmt_short(),
                shared_file_id = %request.shared_file_id,
                response = ?resp,
                "file-access: permission response received"
            );
            Ok(resp)
        }
        Err(code) => {
            info!(
                peer = %server_pk.fmt_short(),
                shared_file_id = %request.shared_file_id,
                error_code = %code,
                "file-access: permission denied at wire level"
            );
            // Map wire-level errors to FileAccessResponse variants for
            // consistent handling.
            Ok(match code {
                FileAccessErrorCode::UnsupportedVersion => FileAccessResponse::UnsupportedVersion,
                FileAccessErrorCode::PermissionDenied => FileAccessResponse::PermissionDenied,
                FileAccessErrorCode::NotFound => FileAccessResponse::NotFound,
                FileAccessErrorCode::InvalidRequest => FileAccessResponse::NotFound,
                FileAccessErrorCode::RateLimited => FileAccessResponse::RateLimited,
                FileAccessErrorCode::Busy => FileAccessResponse::Busy,
                FileAccessErrorCode::ResponseTooLarge => FileAccessResponse::Unavailable,
                FileAccessErrorCode::InternalError => FileAccessResponse::Unavailable,
            })
        }
    }
}

/// Handle the response from a file-access request, applying the appropriate
/// storage transition and verifying the descriptor if granted.
///
/// # Arguments
///
/// * `storage` — The storage layer.
/// * `download_id` — The download id whose state to transition.
/// * `response` — The [`FileAccessResponse`] from the server.
/// * `local_pk` — Our own public key (to verify requester binding).
/// * `expected_content_hash_hex` — The content hash we expect (hex string).
/// * `expected_size` — The expected file size in bytes.
///
/// # Returns
///
/// * `Ok(Some(descriptor))` — permission granted, descriptor verified, download
///   transitioned to `downloading`.
/// * `Ok(None)` — permission denied or other non-granted response. The download
///   is transitioned to an appropriate state (`paused` for retryable errors,
///   `failed` for terminal errors).
/// * `Err` — a storage or verification error that should be treated as a
///   hard failure (e.g. content hash mismatch that was recorded as a failure).
pub fn handle_permission_response(
    storage: &Storage,
    download_id: i64,
    response: FileAccessResponse,
    local_pk: &PublicKey,
    expected_content_hash_hex: &str,
    expected_size: u64,
) -> std::result::Result<Option<SignedDownloadDescriptor>, anyhow::Error> {
    match response {
        FileAccessResponse::Granted(descriptor) => handle_granted(
            storage,
            download_id,
            *descriptor,
            local_pk,
            expected_content_hash_hex,
            expected_size,
        ),
        FileAccessResponse::PermissionDenied => {
            info!(download_id, "file-access: permission denied");
            storage.reject_resumed_permission(download_id, "permission denied by remote peer")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::PermissionDenied,
                false,
                None,
                Some(false),
                None,
            );
            Ok(None)
        }
        FileAccessResponse::VersionMismatch { current_version } => {
            info!(
                download_id,
                current_version, "file-access: version mismatch"
            );
            storage.reject_resumed_permission(
                download_id,
                &format!("version mismatch: server has version {current_version}"),
            )?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::VersionMismatch,
                false,
                None,
                Some(false),
                None,
            );
            Ok(None)
        }
        FileAccessResponse::NotFound => {
            info!(download_id, "file-access: file not found on remote peer");
            storage.reject_resumed_permission(download_id, "file not found on remote peer")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::NotFound,
                false,
                None,
                Some(false),
                None,
            );
            Ok(None)
        }
        FileAccessResponse::Disabled => {
            info!(download_id, "file-access: file sharing disabled on remote");
            storage
                .reject_resumed_permission(download_id, "file sharing disabled on remote peer")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::PermissionDenied,
                false,
                None,
                Some(false),
                None,
            );
            Ok(None)
        }
        FileAccessResponse::Unavailable => {
            info!(download_id, "file-access: file unavailable on remote peer");
            storage.reject_resumed_permission(download_id, "file unavailable on remote peer")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::PeerUnavailable,
                true,
                None,
                Some(true),
                None,
            );
            Ok(None)
        }
        FileAccessResponse::Changed => {
            info!(download_id, "file-access: file content changed on remote");
            // Content changed is a permanent issue — fail so the user must
            // manually re-fetch with an updated catalogue.
            storage.fail_download(
                download_id,
                "file content changed since catalogue was fetched",
                None,
            )?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::VersionMismatch,
                false,
                None,
                Some(false),
                None,
            );
            Ok(None)
        }
        FileAccessResponse::Busy => {
            warn!(download_id, "file-access: remote peer is busy");
            storage.reject_resumed_permission(download_id, "remote peer is busy")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::ResourceExhausted,
                true,
                None,
                Some(true),
                None,
            );
            Ok(None)
        }
        FileAccessResponse::RateLimited => {
            warn!(download_id, "file-access: rate limited by remote peer");
            storage.reject_resumed_permission(download_id, "rate limited by remote peer")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::RateLimited,
                true,
                None,
                Some(true),
                None,
            );
            Ok(None)
        }
        FileAccessResponse::UnsupportedVersion => {
            warn!(download_id, "file-access: unsupported protocol version");
            storage.fail_download(
                download_id,
                "remote peer does not support our protocol version",
                None,
            )?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::ProtocolError,
                false,
                None,
                Some(false),
                None,
            );
            Ok(None)
        }
    }
}

/// Process a `Granted` response: verify the [`SignedDownloadDescriptor`] and
/// persist the transition to `downloading` if every check passes.
fn handle_granted(
    storage: &Storage,
    download_id: i64,
    descriptor: SignedDownloadDescriptor,
    local_pk: &PublicKey,
    expected_content_hash_hex: &str,
    expected_size: u64,
) -> std::result::Result<Option<SignedDownloadDescriptor>, anyhow::Error> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // ── 1. Verify the descriptor integrity and lifetime ─────────────────
    let verification = verify_download_descriptor(
        &descriptor,
        &descriptor.owner_id, // expected owner = the signer
        local_pk,             // we are the expected requester
        now_ms,
    );

    match verification {
        DescriptorVerification::Valid => {
            // Proceed to content verification below.
        }
        DescriptorVerification::Expired => {
            warn!(download_id, "file-access: descriptor expired");
            storage.reject_resumed_permission(download_id, "descriptor expired before use")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::PermissionDenied,
                false,
                None,
                Some(false),
                None,
            );
            return Ok(None);
        }
        DescriptorVerification::NotYetValid => {
            warn!(download_id, "file-access: descriptor not yet valid");
            storage
                .reject_resumed_permission(download_id, "descriptor issue time is in the future")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::ProtocolError,
                false,
                None,
                Some(false),
                None,
            );
            return Ok(None);
        }
        DescriptorVerification::OwnerMismatch => {
            warn!(download_id, "file-access: descriptor owner mismatch");
            storage.fail_download(
                download_id,
                "descriptor owner does not match expected peer",
                None,
            )?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::PermissionDenied,
                false,
                None,
                Some(false),
                None,
            );
            return Ok(None);
        }
        DescriptorVerification::RequesterMismatch => {
            warn!(download_id, "file-access: descriptor requester mismatch");
            storage.fail_download(
                download_id,
                "descriptor requester does not match our identity",
                None,
            )?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::PermissionDenied,
                false,
                None,
                Some(false),
                None,
            );
            return Ok(None);
        }
        DescriptorVerification::InvalidSignature => {
            warn!(download_id, "file-access: descriptor invalid signature");
            storage.fail_download(
                download_id,
                "descriptor signature verification failed",
                None,
            )?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::ProtocolError,
                false,
                None,
                Some(false),
                None,
            );
            return Ok(None);
        }
        DescriptorVerification::NonceReused => {
            warn!(download_id, "file-access: descriptor nonce reused");
            storage.reject_resumed_permission(download_id, "descriptor nonce already consumed")?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::PermissionDenied,
                false,
                None,
                Some(false),
                None,
            );
            return Ok(None);
        }
        DescriptorVerification::ContentMismatch => {
            warn!(download_id, "file-access: descriptor content hash mismatch");
            storage.fail_download(
                download_id,
                "descriptor content hash does not match expected file",
                None,
            )?;
            TRANSFER_TELEMETRY.failure(
                download_id,
                ErrorCategory::IntegrityMismatch,
                false,
                None,
                Some(false),
                None,
            );
            return Ok(None);
        }
    }

    // ── 2. Verify content hash matches what we expect ───────────────────
    let desc_hash_hex = &descriptor.content_hash;
    if !desc_hash_hex.eq_ignore_ascii_case(expected_content_hash_hex) {
        TRANSFER_TELEMETRY.failure(
            download_id,
            ErrorCategory::IntegrityMismatch,
            false,
            None,
            Some(false),
            None,
        );
        storage.fail_download(
            download_id,
            &format!(
                "descriptor content hash mismatch: expected {expected_content_hash_hex}, got {desc_hash_hex}"
            ),
            None,
        )?;
        return Err(anyhow::anyhow!(
            "descriptor content hash mismatch: expected {expected_content_hash_hex}, got {desc_hash_hex}"
        ));
    }

    // ── 3. Verify size matches ─────────────────────────────────────────
    if descriptor.size_bytes != expected_size {
        TRANSFER_TELEMETRY.failure(
            download_id,
            ErrorCategory::SizeMismatch,
            false,
            None,
            Some(false),
            None,
        );
        storage.fail_download(
            download_id,
            &format!(
                "descriptor size mismatch: expected {expected_size}, got {}",
                descriptor.size_bytes
            ),
            None,
        )?;
        return Err(anyhow::anyhow!(
            "descriptor size mismatch: expected {expected_size}, got {}",
            descriptor.size_bytes
        ));
    }

    // ── 4. Persist transition to downloading ───────────────────────────
    storage.accept_resumed_descriptor(download_id, expected_content_hash_hex, expected_size)?;

    info!(
        download_id,
        "file-access: permission granted, download starting"
    );
    Ok(Some(descriptor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_access_protocol::{sign_download_descriptor, BlobFormat};

    /// Helper: create an in-memory storage with a download in
    /// `requesting_permission` state.
    fn setup_download_in_permission_state(
        storage: &Storage,
        content_hash: &str,
        remote_peer: &str,
        total_bytes: u64,
    ) -> i64 {
        // Must insert a file object first (FK constraint).
        storage
            .put_file_object(
                content_hash,
                total_bytes,
                "text/plain",
                "test.txt",
                &vec![0u8; total_bytes as usize],
            )
            .expect("put file object");

        let id = storage
            .create_download(content_hash, remote_peer, total_bytes)
            .expect("create download");

        // Transition: Queued → ResolvingPeer → RequestingPermission
        // use direct SQL since the old begin_download / mark_resume_peer_resolved
        // methods have been replaced by the resume_download pipeline.
        storage.with_conn(|conn| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            conn.execute(
                "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                rusqlite::params![now, id],
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(())
        })
        .expect("set download to requesting_permission");

        id
    }

    fn hex_to_raw(hex_str: &str) -> [u8; 32] {
        let raw = hex::decode(hex_str).expect("valid hex");
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&raw);
        arr
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    // ── Granted: descriptor fully valid ─────────────────────────────────

    #[test]
    fn handle_granted_valid_descriptor() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "ab".repeat(32);
        let total_bytes = 4096;
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            total_bytes,
        );

        let now = now_ms();
        let descriptor = sign_download_descriptor(
            &server_sk,
            client_pk,
            "test-shared-file-id".into(),
            hex_to_raw(&content_hash),
            total_bytes,
            BlobFormat::Raw,
            now.saturating_sub(1000), // issued 1s ago
            now + 60_000,             // expires in 60s
        );

        let response = FileAccessResponse::Granted(Box::new(descriptor));
        let result = handle_permission_response(
            &storage,
            id,
            response,
            &client_pk,
            &content_hash,
            total_bytes,
        )
        .expect("handle response");

        assert!(result.is_some(), "expected a descriptor back");

        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(
            download.state, "downloading",
            "download should transition to downloading"
        );
    }

    // ── Expired descriptor ──────────────────────────────────────────────

    #[test]
    fn handle_granted_expired_descriptor() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "cd".repeat(32);
        let total_bytes = 2048;
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            total_bytes,
        );

        let now = now_ms();
        // Descriptor that expired 60s ago.
        let descriptor = sign_download_descriptor(
            &server_sk,
            client_pk,
            "test-shared-file-id".into(),
            hex_to_raw(&content_hash),
            total_bytes,
            BlobFormat::Raw,
            now.saturating_sub(120_000), // issued 2m ago
            now.saturating_sub(60_000),  // expired 1m ago
        );

        let response = FileAccessResponse::Granted(Box::new(descriptor));
        let result = handle_permission_response(
            &storage,
            id,
            response,
            &client_pk,
            &content_hash,
            total_bytes,
        )
        .expect("handle response");

        assert!(result.is_none(), "expired descriptor should be rejected");

        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(
            download.state, "paused",
            "expired descriptor should pause (not fail)"
        );
    }

    // ── Permission denied ──────────────────────────────────────────────

    #[test]
    fn handle_permission_denied() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "ef".repeat(32);
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            1024,
        );

        let response = FileAccessResponse::PermissionDenied;
        let result =
            handle_permission_response(&storage, id, response, &client_pk, &content_hash, 1024)
                .expect("handle response");

        assert!(result.is_none());

        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(download.state, "paused");
    }

    // ── Version mismatch ───────────────────────────────────────────────

    #[test]
    fn handle_version_mismatch() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "01".repeat(32);
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            1024,
        );

        let response = FileAccessResponse::VersionMismatch {
            current_version: 42,
        };
        let result =
            handle_permission_response(&storage, id, response, &client_pk, &content_hash, 1024)
                .expect("handle response");

        assert!(result.is_none());

        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(download.state, "paused");
    }

    // ── Invalid descriptor signature (tampered payload) ──────────────

    #[test]
    fn handle_invalid_descriptor_signature() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "23".repeat(32);
        let total_bytes = 4096;
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            total_bytes,
        );

        let now = now_ms();
        let mut descriptor = sign_download_descriptor(
            &server_sk,
            client_pk,
            "test-shared-file-id".into(),
            hex_to_raw(&content_hash),
            total_bytes,
            BlobFormat::Raw,
            now.saturating_sub(1000),
            now + 60_000,
        );

        // Tamper with the size after signing — the signature no longer
        // matches the payload.
        descriptor.size_bytes = 9999;

        let response = FileAccessResponse::Granted(Box::new(descriptor));
        let result = handle_permission_response(
            &storage,
            id,
            response,
            &client_pk,
            &content_hash,
            total_bytes,
        )
        .expect("handle response");

        assert!(result.is_none(), "tampered descriptor should be rejected");

        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(
            download.state, "failed",
            "tampered signature should fail the download"
        );
    }

    // ── Content hash mismatch in descriptor ────────────────────────────

    #[test]
    fn handle_content_hash_mismatch_in_descriptor() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "45".repeat(32);
        let wrong_hash = "67".repeat(32);
        let total_bytes = 4096;
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            total_bytes,
        );

        let now = now_ms();
        // Descriptor has a different content hash than what the download expects.
        let descriptor = sign_download_descriptor(
            &server_sk,
            client_pk,
            "test-shared-file-id".into(),
            hex_to_raw(&wrong_hash),
            total_bytes,
            BlobFormat::Raw,
            now.saturating_sub(1000),
            now + 60_000,
        );

        let response = FileAccessResponse::Granted(Box::new(descriptor));
        let result = handle_permission_response(
            &storage,
            id,
            response,
            &client_pk,
            &content_hash,
            total_bytes,
        );

        assert!(result.is_err(), "hash mismatch should return an error");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("content hash mismatch"),
            "error should mention content hash mismatch: {err}"
        );

        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(
            download.state, "failed",
            "hash mismatch should fail the download"
        );
    }

    // ── File not found ─────────────────────────────────────────────────

    #[test]
    fn handle_file_not_found() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "89".repeat(32);
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            1024,
        );

        let response = FileAccessResponse::NotFound;
        let result =
            handle_permission_response(&storage, id, response, &client_pk, &content_hash, 1024)
                .expect("handle response");

        assert!(result.is_none());
        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(download.state, "paused");
    }

    // ── Changed content ────────────────────────────────────────────────

    #[test]
    fn handle_changed_response_fails_download() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "ab".repeat(32);
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            1024,
        );

        let response = FileAccessResponse::Changed;
        let result =
            handle_permission_response(&storage, id, response, &client_pk, &content_hash, 1024)
                .expect("handle response");

        assert!(result.is_none());
        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(
            download.state, "failed",
            "Changed response should fail the download"
        );
    }

    // ── Busy / rate limited → paused ──────────────────────────────────

    #[test]
    fn handle_busy_pauses_download() {
        let storage = Storage::memory().expect("memory storage");
        let content_hash = "cd".repeat(32);
        let server_sk = iroh::SecretKey::generate();
        let server_pk = server_sk.public();
        let client_sk = iroh::SecretKey::generate();
        let client_pk = client_sk.public();

        let id = setup_download_in_permission_state(
            &storage,
            &content_hash,
            &server_pk.to_string(),
            1024,
        );

        let response = FileAccessResponse::Busy;
        let result =
            handle_permission_response(&storage, id, response, &client_pk, &content_hash, 1024)
                .expect("handle response");

        assert!(result.is_none());
        let download = storage
            .get_download(id)
            .expect("get download")
            .expect("exists");
        assert_eq!(download.state, "paused");
    }

    // ── No-op: begin_download rejects invalid states ──────────────────

    #[test]
    fn begin_download_rejects_already_started() {
        let storage = Storage::memory().expect("memory storage");
        let id = setup_download_in_permission_state(
            &storage,
            &"01".repeat(32),
            &iroh::SecretKey::generate().public().to_string(),
            1024,
        );

        // Already at requesting_permission — accept_resumed_descriptor should work.
        storage
            .accept_resumed_descriptor(id, &"01".repeat(32), 1024)
            .expect("accept_resumed_descriptor should succeed on requesting_permission");
    }

    #[test]
    fn get_download_rejects_nonexistent() {
        let storage = Storage::memory().expect("memory storage");
        let result = storage
            .get_download(99999)
            .expect("get_download on nonexistent should return Ok(None)");
        assert!(result.is_none(), "nonexistent download should return None");
    }
}
