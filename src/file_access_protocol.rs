//! File access protocol — request, response, and versioned wire wrappers.
//!
//! # Wire format
//!
//! Every message on the wire is postcard-encoded inside a versioned wrapper:
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │ FileAccessWireRequest {          │
//! │   version: u16 = 1,              │
//! │   inner: FileAccessRequest,       │
//! │ }                                │
//! └──────────────────────────────────┘
//! ```
//!
//! The version field lets us evolve the file-access protocol without
//! changing the ALPN string.  Unknown versions MUST be rejected with
//! [`FileAccessErrorCode::UnsupportedVersion`].
//!
//! # Feature flag
//!
//! Always available (no feature gate).  Only uses `serde` and `postcard`.

use iroh_base::PublicKey;

use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;

/// Ed25519 signature length in bytes.
const SIGNATURE_LEN: usize = 64;

// ── Wire version ─────────────────────────────────────────────────────────────

/// Current wire version for file-access protocol messages.
pub const FILE_ACCESS_WIRE_VERSION: u16 = 1;

/// All wire versions that the current code understands.
pub const SUPPORTED_FILE_ACCESS_VERSIONS: &[u16] = &[1];

/// Maximum length (in bytes) for a filename received in a file-access request.
const MAX_FILENAME_BYTES: usize = 512;

// ── Error codes (wire-safe) ──────────────────────────────────────────────────

/// Stable, wire-safe error codes for file-access operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FileAccessErrorCode {
    /// The requested wire version is not supported.
    UnsupportedVersion = 1,
    /// The requesting peer is not authorised to access this file.
    PermissionDenied = 2,
    /// The requested file was not found on this peer.
    NotFound = 3,
    /// The request payload was malformed or contained invalid fields.
    InvalidRequest = 4,
    /// The peer has been rate-limited; try again later.
    RateLimited = 5,
    /// The server is busy and cannot process the request right now.
    Busy = 6,
    /// The response exceeded the maximum allowed size.
    ResponseTooLarge = 7,
    /// An unexpected internal error occurred.
    InternalError = 8,
}

impl Serialize for FileAccessErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FileAccessErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = FileAccessErrorCode;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a snake_case file access error code string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(match value {
                    "unsupported_version" => FileAccessErrorCode::UnsupportedVersion,
                    "permission_denied" => FileAccessErrorCode::PermissionDenied,
                    "not_found" => FileAccessErrorCode::NotFound,
                    "invalid_request" => FileAccessErrorCode::InvalidRequest,
                    "rate_limited" => FileAccessErrorCode::RateLimited,
                    "busy" => FileAccessErrorCode::Busy,
                    "response_too_large" => FileAccessErrorCode::ResponseTooLarge,
                    "internal_error" => FileAccessErrorCode::InternalError,
                    _ => FileAccessErrorCode::InternalError,
                })
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl FileAccessErrorCode {
    /// Return the canonical wire-safe snake_case representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "unsupported_version",
            Self::PermissionDenied => "permission_denied",
            Self::NotFound => "not_found",
            Self::InvalidRequest => "invalid_request",
            Self::RateLimited => "rate_limited",
            Self::Busy => "busy",
            Self::ResponseTooLarge => "response_too_large",
            Self::InternalError => "internal_error",
        }
    }
}

impl std::fmt::Display for FileAccessErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ── Additional types used by file_access_handler ─────────────────────────

/// Whether a blob is expected to already exist locally or needs to be
/// downloaded from the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobFormat {
    /// The blob must already be present in the local store.
    Raw,
    /// The blob is a hash-seq (iroh concept for large files).
    HashSeq,
}

/// Safe wire-friendly metadata for a prepared file ready to serve.
#[derive(Debug, Clone)]
pub struct PreparedFile {
    /// The content hash of the prepared blob.
    pub content_hash: String,
    /// Expected file size in bytes.
    pub size_bytes: u64,
    /// How the blob is stored (Raw / HashSeq).
    pub blob_format: BlobFormat,
    /// MIME type of the file.
    pub mime_type: String,
    /// Safe display filename for wire transfer.
    pub filename: String,
}

/// Outcome of verifying a SignedDownloadDescriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorVerification {
    /// The descriptor is valid and has not been used before.
    Valid,
    /// The descriptor's nonce was already consumed.
    NonceReused,
    /// The descriptor's signature is invalid.
    InvalidSignature,
    /// The descriptor has expired.
    Expired,
    /// The descriptor is not yet valid (issue time is in the future).
    NotYetValid,
    /// The descriptor's content hash does not match the expected file.
    ContentMismatch,
    /// The descriptor's owner does not match the expected peer.
    OwnerMismatch,
    /// The descriptor's requester does not match our identity.
    RequesterMismatch,
}

/// Sign a [`SignedDownloadDescriptor`] with the owner's secret key.
///
/// Generates a random nonce and an empty blob ticket internally.
/// The caller supplies the blob hash, size, and lifetime bounds.
#[allow(clippy::too_many_arguments)]
pub fn sign_download_descriptor(
    owner: &iroh::SecretKey,
    requester: iroh::PublicKey,
    shared_file_id: String,
    blob_hash: [u8; 32],
    size_bytes: u64,
    _blob_format: BlobFormat,
    now_ms: u64,
    expires_at_ms: u64,
) -> SignedDownloadDescriptor {
    let content_hash = hex::encode(blob_hash);
    let nonce = rand::random::<[u8; 32]>();
    let blob_ticket = Vec::new(); // populated by the blob-transfer layer

    let mut payload = Vec::new();
    payload.extend_from_slice(owner.public().as_bytes());
    payload.extend_from_slice(requester.as_bytes());
    payload.extend_from_slice(shared_file_id.as_bytes());
    payload.extend_from_slice(&blob_hash);
    payload.extend_from_slice(content_hash.as_bytes());
    payload.extend_from_slice(&size_bytes.to_le_bytes());
    payload.extend_from_slice(&blob_ticket);
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&now_ms.to_le_bytes());
    payload.extend_from_slice(&expires_at_ms.to_le_bytes());
    let signature = owner.sign(&payload);
    SignedDownloadDescriptor {
        owner_id: owner.public(),
        requester,
        shared_file_id,
        blob_hash,
        content_hash,
        size_bytes,
        blob_ticket,
        nonce,
        issued_at_ms: now_ms,
        expires_at_ms,
        signature: ByteArray::from(signature.to_bytes()),
    }
}

/// Verify a [`SignedDownloadDescriptor`]'s owner, requester, signature, and
/// expiry.
///
/// Returns [`DescriptorVerification::Valid`] on success, or a reason.
pub fn verify_download_descriptor(
    descriptor: &SignedDownloadDescriptor,
    expected_owner: &iroh::PublicKey,
    expected_requester: &iroh::PublicKey,
    now_ms: u64,
) -> DescriptorVerification {
    // ── 1. Check expiry (fast path) ──────────────────────────────────────
    if now_ms > descriptor.expires_at_ms {
        return DescriptorVerification::Expired;
    }

    // ── 2. Check not-yet-valid ───────────────────────────────────────────
    if now_ms < descriptor.issued_at_ms {
        return DescriptorVerification::NotYetValid;
    }

    // ── 3. Check that the owner matches what we expect ───────────────────
    if &descriptor.owner_id != expected_owner {
        return DescriptorVerification::OwnerMismatch;
    }

    // ── 4. Check that the requester matches what we expect ───────────────
    if &descriptor.requester != expected_requester {
        return DescriptorVerification::RequesterMismatch;
    }

    // ── 5. Reconstruct the signing payload ──────────────────────────────
    let mut payload = Vec::new();
    payload.extend_from_slice(descriptor.owner_id.as_bytes());
    payload.extend_from_slice(descriptor.requester.as_bytes());
    payload.extend_from_slice(descriptor.shared_file_id.as_bytes());
    payload.extend_from_slice(&descriptor.blob_hash);
    payload.extend_from_slice(descriptor.content_hash.as_bytes());
    payload.extend_from_slice(&descriptor.size_bytes.to_le_bytes());
    payload.extend_from_slice(&descriptor.blob_ticket);
    payload.extend_from_slice(&descriptor.nonce);
    payload.extend_from_slice(&descriptor.issued_at_ms.to_le_bytes());
    payload.extend_from_slice(&descriptor.expires_at_ms.to_le_bytes());

    // ── 6. Verify the signature ──────────────────────────────────────────
    let sig_bytes = *descriptor.signature.as_ref();
    let sig = iroh::Signature::from_bytes(&sig_bytes);
    if descriptor.owner_id.verify(&payload, &sig).is_ok() {
        DescriptorVerification::Valid
    } else {
        DescriptorVerification::InvalidSignature
    }
}

// ── Inner protocol types ─────────────────────────────────────────────────────

/// A request to access (download) a file from a remote peer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileAccessRequest {
    /// Blake3 content hash of the requested file (hex-encoded).
    pub content_hash: String,
    /// Suggested filename (from the catalogue).
    pub filename: String,
    /// Expected file size in bytes (from the catalogue).
    pub expected_size: u64,
    /// Stable shared-file identifier from the catalogue.
    #[serde(default)]
    pub shared_file_id: String,
    /// Expected content hash (raw 32 bytes).
    #[serde(default)]
    pub expected_content_hash: [u8; 32],
    /// Expected version number (ms timestamp from catalogue).
    #[serde(default)]
    pub expected_version: u64,
}

impl FileAccessRequest {
    /// Create a new request with the given parameters and sensible defaults.
    pub fn new(
        shared_file_id: &str,
        expected_content_hash: [u8; 32],
        expected_version: u64,
    ) -> Self {
        Self {
            content_hash: hex::encode(expected_content_hash),
            filename: "unknown".to_string(),
            expected_size: 0,
            shared_file_id: shared_file_id.to_string(),
            expected_content_hash,
            expected_version,
        }
    }

    /// Validate the request fields.
    pub fn validate(&self) -> std::result::Result<(), (FileAccessErrorCode, &'static str)> {
        if self.shared_file_id.is_empty() {
            return Err((
                FileAccessErrorCode::InvalidRequest,
                "shared_file_id is empty",
            ));
        }
        if self.content_hash.is_empty() && self.expected_content_hash == [0; 32] {
            return Err((FileAccessErrorCode::InvalidRequest, "content hash is empty"));
        }
        // Validate filename: must not contain path separators or control chars.
        if self.filename.contains('/') || self.filename.contains('\\') {
            return Err((
                FileAccessErrorCode::InvalidRequest,
                "filename contains path separators",
            ));
        }
        if self.filename.len() > MAX_FILENAME_BYTES {
            return Err((
                FileAccessErrorCode::InvalidRequest,
                "filename exceeds maximum length",
            ));
        }
        if !self.filename.is_empty() && self.filename.chars().any(|ch| ch.is_control()) {
            return Err((
                FileAccessErrorCode::InvalidRequest,
                "filename contains control characters",
            ));
        }
        Ok(())
    }

    /// Validate the wire version (delegates to the wire wrapper; this is a
    /// convenience method for backward compatibility).
    pub fn validate_request_version(&self) -> std::result::Result<(), FileAccessErrorCode> {
        // Version validation is done by the wire wrapper; this is kept
        // for backward compatibility with code that calls it on the inner request.
        Ok(())
    }
}

/// A signed descriptor that authorises the requester to download a file.
///
/// Contains a short-lived blob ticket that the requester validates before
/// starting the actual blob transfer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignedDownloadDescriptor {
    /// Public key of the file owner (signer).
    pub owner_id: PublicKey,
    /// Public key of the authorised requester.
    pub requester: PublicKey,
    /// Stable shared-file identifier from the catalogue.
    pub shared_file_id: String,
    /// Blake3 content hash of the file (raw 32 bytes).
    pub blob_hash: [u8; 32],
    /// Hex-encoded blake3 content hash (for display/lookup).
    pub content_hash: String,
    /// Expected file size in bytes.
    pub size_bytes: u64,
    /// Opaque blob ticket (iroh blob ticket bytes).
    pub blob_ticket: Vec<u8>,
    /// Unique nonce for replay protection.
    pub nonce: [u8; 32],
    /// Timestamp when the descriptor was issued (ms since UNIX epoch).
    pub issued_at_ms: u64,
    /// Expiration timestamp (milliseconds since UNIX epoch).
    pub expires_at_ms: u64,
    /// Ed25519 signature over the payload by `owner_id`.
    pub signature: ByteArray<SIGNATURE_LEN>,
}

/// Response to a [`FileAccessRequest`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FileAccessResponse {
    /// Access granted — contains the download descriptor.
    Granted(Box<SignedDownloadDescriptor>),
    /// The requested wire version is not supported.
    UnsupportedVersion,
    /// The requesting peer is not permitted to download this file.
    PermissionDenied,
    /// The file was not found on this peer.
    NotFound,
    /// File sharing has been disabled by the owner.
    Disabled,
    /// The file content has changed since the catalogue was issued.
    Changed,
    /// The remote peer is temporarily unavailable.
    Unavailable,
    /// The remote peer is busy — try again later.
    Busy,
    /// Rate-limited — the requester has exceeded the per-peer limit.
    RateLimited,
    /// The requested version of the file is not available (mismatch).
    VersionMismatch {
        /// The current version on the server.
        current_version: u64,
    },
}

// ── Versioned wire wrappers ──────────────────────────────────────────────────

/// Versioned wire wrapper for file-access requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileAccessWireRequest {
    /// Wire protocol version.
    pub version: u16,
    /// The inner request payload.
    pub inner: FileAccessRequest,
}

impl FileAccessWireRequest {
    /// Create a new wire request with the current [`FILE_ACCESS_WIRE_VERSION`].
    pub fn new(inner: FileAccessRequest) -> Self {
        Self {
            version: FILE_ACCESS_WIRE_VERSION,
            inner,
        }
    }

    /// Validate that `self.version` is in [`SUPPORTED_FILE_ACCESS_VERSIONS`].
    pub fn validate_version(&self) -> Result<(), FileAccessErrorCode> {
        if SUPPORTED_FILE_ACCESS_VERSIONS.contains(&self.version) {
            Ok(())
        } else {
            Err(FileAccessErrorCode::UnsupportedVersion)
        }
    }
}

/// Versioned wire wrapper for file-access responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileAccessWireResponse {
    /// Wire protocol version.
    pub version: u16,
    /// The inner response payload, or a wire-safe error code.
    pub inner: Result<FileAccessResponse, FileAccessErrorCode>,
}

impl FileAccessWireResponse {
    /// Create a new success response with the current [`FILE_ACCESS_WIRE_VERSION`].
    pub fn success(inner: FileAccessResponse) -> Self {
        Self {
            version: FILE_ACCESS_WIRE_VERSION,
            inner: Ok(inner),
        }
    }

    /// Create a new error response with the current [`FILE_ACCESS_WIRE_VERSION`].
    pub fn error(code: FileAccessErrorCode) -> Self {
        Self {
            version: FILE_ACCESS_WIRE_VERSION,
            inner: Err(code),
        }
    }

    /// Validate that `self.version` is in [`SUPPORTED_FILE_ACCESS_VERSIONS`].
    pub fn validate_version(&self) -> Result<(), FileAccessErrorCode> {
        if SUPPORTED_FILE_ACCESS_VERSIONS.contains(&self.version) {
            Ok(())
        } else {
            Err(FileAccessErrorCode::UnsupportedVersion)
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── FileAccessRequest validation ─────────────────────────────────────

    #[test]
    fn file_access_request_valid_succeeds() {
        let req = FileAccessRequest {
            content_hash: "abc123".into(),
            filename: "photo.png".into(),
            expected_size: 1024,
            shared_file_id: "file-001".into(),
            expected_content_hash: [1u8; 32],
            expected_version: 1,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn file_access_request_empty_shared_file_id_rejected() {
        let req = FileAccessRequest {
            shared_file_id: String::new(),
            ..FileAccessRequest::new("x", [1u8; 32], 1)
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn file_access_request_filename_with_path_separators_rejected() {
        for sep in &["/", "\\"] {
            let req = FileAccessRequest {
                filename: format!("sub{}dir{}name.txt", sep, sep),
                ..FileAccessRequest::new("id", [1u8; 32], 1)
            };
            assert!(
                req.validate().is_err(),
                "filename with '{sep}' must be rejected"
            );
        }
    }

    #[test]
    fn file_access_request_filename_with_control_chars_rejected() {
        let req = FileAccessRequest {
            filename: "file\u{0000}.txt".into(),
            ..FileAccessRequest::new("id", [1u8; 32], 1)
        };
        assert!(
            req.validate().is_err(),
            "filename with null byte must be rejected"
        );

        let req = FileAccessRequest {
            filename: "file\u{001B}.txt".into(),
            ..FileAccessRequest::new("id", [1u8; 32], 1)
        };
        assert!(
            req.validate().is_err(),
            "filename with ESC must be rejected"
        );
    }

    #[test]
    fn file_access_request_oversized_filename_rejected() {
        let req = FileAccessRequest {
            filename: "x".repeat(MAX_FILENAME_BYTES + 1),
            ..FileAccessRequest::new("id", [1u8; 32], 1)
        };
        assert!(
            req.validate().is_err(),
            "oversized filename must be rejected"
        );
    }

    // ── Serialization round-trip ───────────────────────────────────────────

    #[test]
    fn file_access_wire_request_round_trip() {
        let inner = FileAccessRequest {
            content_hash: "deadbeef".into(),
            filename: "photo.png".into(),
            expected_size: 65536,
            shared_file_id: String::new(),
            expected_content_hash: [0u8; 32],
            expected_version: 0,
        };
        let original = FileAccessWireRequest::new(inner);

        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let decoded: FileAccessWireRequest = postcard::from_bytes(&bytes).expect("deserialize");

        assert_eq!(original, decoded);
        assert_eq!(decoded.version, FILE_ACCESS_WIRE_VERSION);
    }

    #[test]
    fn file_access_wire_response_round_trip_success() {
        let desc = SignedDownloadDescriptor {
            owner_id: PublicKey::from_bytes(&[0u8; 32]).expect("valid key"),
            requester: PublicKey::from_bytes(&[1u8; 32]).expect("valid key"),
            shared_file_id: "test-file".into(),
            blob_hash: [0u8; 32],
            content_hash: "deadbeef".into(),
            size_bytes: 1024,
            blob_ticket: vec![1, 2, 3, 4],
            nonce: [0u8; 32],
            issued_at_ms: 1000,
            expires_at_ms: 1234567890000,
            signature: ByteArray::from([0u8; 64]),
        };
        let original = FileAccessWireResponse::success(FileAccessResponse::Granted(Box::new(desc)));

        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let decoded: FileAccessWireResponse = postcard::from_bytes(&bytes).expect("deserialize");

        assert_eq!(original, decoded);
        assert_eq!(decoded.version, FILE_ACCESS_WIRE_VERSION);
    }

    #[test]
    fn file_access_wire_response_round_trip_error() {
        let original = FileAccessWireResponse::error(FileAccessErrorCode::PermissionDenied);

        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let decoded: FileAccessWireResponse = postcard::from_bytes(&bytes).expect("deserialize");

        assert_eq!(original, decoded);
        assert_eq!(decoded.inner, Err(FileAccessErrorCode::PermissionDenied));
    }

    // ── Unsupported version rejection ──────────────────────────────────────

    #[test]
    fn file_access_wire_request_rejects_unsupported_version() {
        let inner = FileAccessRequest {
            content_hash: "abc".into(),
            filename: "f".into(),
            expected_size: 0,
            shared_file_id: String::new(),
            expected_content_hash: [0u8; 32],
            expected_version: 0,
        };
        let msg = FileAccessWireRequest {
            version: 999,
            inner,
        };
        assert_eq!(
            msg.validate_version(),
            Err(FileAccessErrorCode::UnsupportedVersion)
        );
    }

    #[test]
    fn file_access_wire_response_rejects_unsupported_version() {
        let msg = FileAccessWireResponse {
            version: 0,
            inner: Err(FileAccessErrorCode::InternalError),
        };
        assert_eq!(
            msg.validate_version(),
            Err(FileAccessErrorCode::UnsupportedVersion)
        );
    }

    #[test]
    fn file_access_wire_request_current_version_is_valid() {
        let inner = FileAccessRequest {
            content_hash: "abc".into(),
            filename: "f".into(),
            expected_size: 0,
            shared_file_id: String::new(),
            expected_content_hash: [0u8; 32],
            expected_version: 0,
        };
        let msg = FileAccessWireRequest::new(inner);
        assert!(msg.validate_version().is_ok());
    }

    // ── Truncated message ──────────────────────────────────────────────────

    #[test]
    fn file_access_wire_request_truncated_fails() {
        let inner = FileAccessRequest {
            content_hash: "abc".into(),
            filename: "f".into(),
            expected_size: 100,
            shared_file_id: String::new(),
            expected_content_hash: [0u8; 32],
            expected_version: 0,
        };
        let original = FileAccessWireRequest::new(inner);
        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let truncated = &bytes[..bytes.len().saturating_sub(4)];
        let result: Result<FileAccessWireRequest, _> = postcard::from_bytes(truncated);
        assert!(
            result.is_err(),
            "truncated message should fail to deserialize"
        );
    }

    #[test]
    fn file_access_wire_response_truncated_fails() {
        let original = FileAccessWireResponse::error(FileAccessErrorCode::NotFound);
        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let truncated = &bytes[..bytes.len().saturating_sub(1)];
        let result: Result<FileAccessWireResponse, _> = postcard::from_bytes(truncated);
        assert!(
            result.is_err(),
            "truncated message should fail to deserialize"
        );
    }

    #[test]
    fn file_access_wire_empty_fails() {
        let result: Result<FileAccessWireRequest, _> = postcard::from_bytes(&[]);
        assert!(result.is_err(), "empty message should fail to deserialize");
    }

    // ── Trailing unexpected data ───────────────────────────────────────────

    #[test]
    fn file_access_wire_request_trailing_data_rejected() {
        let inner = FileAccessRequest {
            content_hash: "abc".into(),
            filename: "f".into(),
            expected_size: 100,
            shared_file_id: String::new(),
            expected_content_hash: [0u8; 32],
            expected_version: 0,
        };
        let original = FileAccessWireRequest::new(inner);
        let mut bytes = postcard::to_stdvec(&original).expect("serialize");
        bytes.extend_from_slice(b"TRAILING");
        let result: Result<(FileAccessWireRequest, &[u8]), _> = postcard::take_from_bytes(&bytes);
        match result {
            Ok((_, remaining)) => {
                assert!(!remaining.is_empty(), "trailing data should be detected");
            }
            Err(_) => {
                // Deserialization error is also acceptable.
            }
        }
    }

    #[test]
    fn file_access_wire_response_trailing_data_rejected() {
        let original = FileAccessWireResponse::error(FileAccessErrorCode::Busy);
        let mut bytes = postcard::to_stdvec(&original).expect("serialize");
        bytes.extend_from_slice(b"\xDE\xAD\xBE\xEF");
        let result: Result<(FileAccessWireResponse, &[u8]), _> = postcard::take_from_bytes(&bytes);
        match result {
            Ok((_, remaining)) => {
                assert!(!remaining.is_empty(), "trailing data should be detected");
            }
            Err(_) => {
                // Deserialization error is also acceptable.
            }
        }
    }

    // ── Error code serialization ───────────────────────────────────────────

    #[test]
    fn file_access_error_code_serialization_round_trip() {
        let codes = [
            FileAccessErrorCode::UnsupportedVersion,
            FileAccessErrorCode::PermissionDenied,
            FileAccessErrorCode::NotFound,
            FileAccessErrorCode::InvalidRequest,
            FileAccessErrorCode::RateLimited,
            FileAccessErrorCode::Busy,
            FileAccessErrorCode::ResponseTooLarge,
            FileAccessErrorCode::InternalError,
        ];
        for &code in &codes {
            let bytes = postcard::to_stdvec(&code).expect("serialize");
            let decoded: FileAccessErrorCode = postcard::from_bytes(&bytes).expect("deserialize");
            assert_eq!(code, decoded, "round-trip for {:?}", code);
        }
    }

    #[test]
    fn file_access_unknown_error_code_fails() {
        let unknown_bytes = postcard::to_stdvec(&"future_error").expect("serialize");
        let result: Result<FileAccessErrorCode, _> = postcard::from_bytes(&unknown_bytes);
        assert_eq!(
            result.expect("unknown values should be safe"),
            FileAccessErrorCode::InternalError
        );
    }

    #[test]
    fn file_access_error_code_includes_response_too_large() {
        let code = FileAccessErrorCode::ResponseTooLarge;
        let bytes = postcard::to_stdvec(&code).expect("serialize");
        let decoded: FileAccessErrorCode = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(decoded, code);
    }

    // ── Wire wrapper constructors ──────────────────────────────────────────

    #[test]
    fn file_access_wire_request_new_sets_current_version() {
        let inner = FileAccessRequest {
            content_hash: "abc".into(),
            filename: "f".into(),
            expected_size: 0,
            shared_file_id: String::new(),
            expected_content_hash: [0u8; 32],
            expected_version: 0,
        };
        let msg = FileAccessWireRequest::new(inner);
        assert_eq!(msg.version, FILE_ACCESS_WIRE_VERSION);
    }

    #[test]
    fn file_access_wire_response_success_sets_current_version() {
        let desc = SignedDownloadDescriptor {
            owner_id: PublicKey::from_bytes(&[0u8; 32]).expect("valid key"),
            requester: PublicKey::from_bytes(&[1u8; 32]).expect("valid key"),
            shared_file_id: "test".into(),
            blob_hash: [0u8; 32],
            content_hash: "abc".into(),
            size_bytes: 512,
            blob_ticket: vec![],
            nonce: [0u8; 32],
            issued_at_ms: 500,
            expires_at_ms: 0,
            signature: ByteArray::from([0u8; 64]),
        };
        let msg = FileAccessWireResponse::success(FileAccessResponse::Granted(Box::new(desc)));
        assert_eq!(msg.version, FILE_ACCESS_WIRE_VERSION);
        assert!(msg.inner.is_ok());
    }

    #[test]
    fn file_access_wire_response_error_sets_current_version() {
        let msg = FileAccessWireResponse::error(FileAccessErrorCode::RateLimited);
        assert_eq!(msg.version, FILE_ACCESS_WIRE_VERSION);
        assert_eq!(msg.inner, Err(FileAccessErrorCode::RateLimited));
    }

    // ── Version constants consistency ──────────────────────────────────────

    #[test]
    fn supported_file_access_versions_contains_current() {
        assert!(
            SUPPORTED_FILE_ACCESS_VERSIONS.contains(&FILE_ACCESS_WIRE_VERSION),
            "SUPPORTED_FILE_ACCESS_VERSIONS must include FILE_ACCESS_WIRE_VERSION"
        );
    }

    #[test]
    fn supported_file_access_versions_is_sorted_and_unique() {
        let mut sorted = SUPPORTED_FILE_ACCESS_VERSIONS.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            SUPPORTED_FILE_ACCESS_VERSIONS,
            sorted.as_slice(),
            "SUPPORTED_FILE_ACCESS_VERSIONS must be sorted and unique"
        );
    }
}
