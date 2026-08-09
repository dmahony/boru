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
//! # Descriptor signing (BORU-AUDIT-05)
//!
//! A [`SignedDownloadDescriptor`] is authenticated by an Ed25519 signature
//! over the canonical serialization of a [`DescriptorSignedPayloadV2`]:
//!
//! ```text
//! postcard::to_stdvec(DescriptorSignedPayloadV2 {
//!   protocol: "boru/file-descriptor",  // domain separation
//!   version:  2,                       // signed payload version
//!   owner_id, requester, shared_file_id, blob_hash, size_bytes,
//!   blob_ticket, nonce, issued_at_ms, expires_at_ms,
//! })
//! ```
//!
//! The struct field order IS the wire order: postcard serializes struct
//! fields in declaration order and length-prefixes every variable-length
//! field, so no two descriptors can encode the same byte string with
//! different meanings.  [`sign_download_descriptor`] and
//! [`verify_download_descriptor`] both call
//! [`DescriptorSignedPayloadV2::canonical_bytes`], so the signed bytes can
//! never drift between signing and verification.  Unknown signed-payload
//! versions are rejected as [`DescriptorVerification::UnsupportedVersion`].
//! The hex display string `content_hash` is deliberately NOT signed — the
//! strongly typed `blob_hash` is the single canonical hash representation
//! (BORU-AUDIT-06 removes the duplicate string entirely).
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

/// Protocol domain separator for signed download descriptors.
///
/// Domain separation prevents cross-protocol and cross-version signature
/// confusion: a signature made over `boru/file-descriptor` bytes can never be
/// replayed as a signature over any other Boru object.
pub const DESCRIPTOR_SIGNED_PROTOCOL: &str = "boru/file-descriptor";

/// Version of the canonical signed descriptor payload.
///
/// The descriptor signature always covers a payload with this version inside
/// it.  Unknown versions are rejected by [`verify_download_descriptor`]
/// instead of being guessed (BORU-AUDIT-05).
pub const DESCRIPTOR_SIGNED_PAYLOAD_VERSION: u16 = 2;

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
    /// The descriptor's signed-payload version is not supported.
    UnsupportedVersion,
}

/// Canonical signed payload for a download descriptor (version 2).
///
/// This is the ONLY byte representation a descriptor signature covers.  All
/// signed fields live here in a fixed semantic order, so sign and verify can
/// never drift.  Variable-length fields (`shared_file_id`, `blob_ticket`) are
/// length-prefixed by the deterministic serializer, so adjacent variable-length
/// values can never be confused the way bare `extend_from_slice` concatenation
/// allowed (BORU-AUDIT-05).
///
/// # Signed-field invariant
/// The fields of this struct — and only these fields — are authenticated by
/// `SignedDownloadDescriptor.signature`.  Adding a field here changes the
/// canonical bytes and MUST bump [`DESCRIPTOR_SIGNED_PAYLOAD_VERSION`]; old
/// descriptors are then rejected as [`DescriptorVerification::UnsupportedVersion`],
/// which is fail-closed and never guesses the old layout.  Do NOT add
/// display-only fields (e.g. the hex `content_hash`) here: the strongly typed
/// `blob_hash` is the single canonical hash representation.  See
/// `docs/file-access-descriptor-signing.md` for the full protocol note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescriptorSignedPayloadV2 {
    /// Protocol domain separator — always [`DESCRIPTOR_SIGNED_PROTOCOL`].
    pub protocol: String,
    /// Signed payload version — always [`DESCRIPTOR_SIGNED_PAYLOAD_VERSION`].
    pub version: u16,
    /// Public key of the file owner (signer).
    pub owner_id: PublicKey,
    /// Public key of the authorised requester.
    pub requester: PublicKey,
    /// Stable shared-file identifier from the catalogue.
    pub shared_file_id: String,
    /// Blake3 content hash of the file (raw 32 bytes) — canonical hash.
    pub blob_hash: [u8; 32],
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
}

impl DescriptorSignedPayloadV2 {
    /// Build the canonical payload for a descriptor.
    pub fn from_descriptor(descriptor: &SignedDownloadDescriptor) -> Self {
        Self {
            protocol: DESCRIPTOR_SIGNED_PROTOCOL.to_string(),
            version: DESCRIPTOR_SIGNED_PAYLOAD_VERSION,
            owner_id: descriptor.owner_id,
            requester: descriptor.requester,
            shared_file_id: descriptor.shared_file_id.clone(),
            blob_hash: descriptor.blob_hash,
            size_bytes: descriptor.size_bytes,
            blob_ticket: descriptor.blob_ticket.clone(),
            nonce: descriptor.nonce,
            issued_at_ms: descriptor.issued_at_ms,
            expires_at_ms: descriptor.expires_at_ms,
        }
    }

    /// Serialize this payload to its canonical signed bytes.
    ///
    /// Uses the project's deterministic serializer (postcard) with the
    /// field order fixed by the struct declaration.  Both
    /// [`sign_download_descriptor`] and [`verify_download_descriptor`] call
    /// this exact function, so signing and verification always agree on the
    /// bytes being signed.
    pub fn canonical_bytes(&self) -> std::result::Result<Vec<u8>, postcard::Error> {
        postcard::to_stdvec(self)
    }
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

    // Sign the canonical payload — the ONLY bytes the signature covers.
    let payload = DescriptorSignedPayloadV2 {
        protocol: DESCRIPTOR_SIGNED_PROTOCOL.to_string(),
        version: DESCRIPTOR_SIGNED_PAYLOAD_VERSION,
        owner_id: owner.public(),
        requester,
        shared_file_id: shared_file_id.clone(),
        blob_hash,
        size_bytes,
        blob_ticket: blob_ticket.clone(),
        nonce,
        issued_at_ms: now_ms,
        expires_at_ms,
    };
    let canonical = payload
        .canonical_bytes()
        .expect("postcard serialization of a fixed-size descriptor payload cannot fail");
    let signature = owner.sign(&canonical);
    SignedDownloadDescriptor {
        signed_version: DESCRIPTOR_SIGNED_PAYLOAD_VERSION,
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
/// Unknown signed-payload versions are rejected
/// ([`DescriptorVerification::UnsupportedVersion`]) instead of being guessed.
pub fn verify_download_descriptor(
    descriptor: &SignedDownloadDescriptor,
    expected_owner: &iroh::PublicKey,
    expected_requester: &iroh::PublicKey,
    now_ms: u64,
) -> DescriptorVerification {
    // ── 0. Reject unknown signed-payload versions ──────────────────────
    // Never guess a field layout for a version we do not understand.
    if descriptor.signed_version != DESCRIPTOR_SIGNED_PAYLOAD_VERSION {
        return DescriptorVerification::UnsupportedVersion;
    }

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

    // ── 5. Reconstruct the canonical signing payload ────────────────────
    // The descriptor carries the same fields that were signed; serialize them
    // through the exact same helper as sign_download_descriptor so the bytes
    // are guaranteed identical.  `content_hash` (a hex display string derived
    // from `blob_hash`) is intentionally NOT part of the signed payload — the
    // strongly typed `blob_hash` is the canonical hash representation.
    let payload = DescriptorSignedPayloadV2::from_descriptor(descriptor);
    let canonical = match payload.canonical_bytes() {
        Ok(bytes) => bytes,
        Err(_) => return DescriptorVerification::InvalidSignature,
    };

    // ── 6. Verify the signature with the expected owner's key ─────────
    // The verification key MUST come from the caller (the peer we
    // selected/connected to), not from `descriptor.owner_id`.  At this
    // point the owner check above guarantees they are equal, but verifying
    // against `expected_owner` keeps the trusted identity anchored outside
    // the response so a substituted descriptor can never self-validate.
    let sig_bytes = *descriptor.signature.as_ref();
    let sig = iroh::Signature::from_bytes(&sig_bytes);
    if expected_owner.verify(&canonical, &sig).is_ok() {
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
    /// Signed-payload version of this descriptor.  Must equal
    /// [`DESCRIPTOR_SIGNED_PAYLOAD_VERSION`]; [`verify_download_descriptor`]
    /// rejects any other value without guessing a field layout.
    pub signed_version: u16,
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
            signed_version: DESCRIPTOR_SIGNED_PAYLOAD_VERSION,
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
            signed_version: DESCRIPTOR_SIGNED_PAYLOAD_VERSION,
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

    // ── BORU-AUDIT-05: canonical descriptor signing ────────────────────────

    /// Fixed descriptor fields must produce the exact canonical bytes that the
    /// signature covers.  This golden vector pins the encoding: postcard with
    /// struct declaration order, domain separator first, then the version, then
    /// every signed field.  If this test fails, someone changed the signed
    /// payload layout without bumping [`DESCRIPTOR_SIGNED_PAYLOAD_VERSION`].
    #[test]
    fn descriptor_canonical_bytes_golden_vector() {
        let payload = DescriptorSignedPayloadV2 {
            protocol: DESCRIPTOR_SIGNED_PROTOCOL.to_string(),
            version: DESCRIPTOR_SIGNED_PAYLOAD_VERSION,
            owner_id: PublicKey::from_bytes(&[0u8; 32]).expect("valid key"),
            requester: PublicKey::from_bytes(&[1u8; 32]).expect("valid key"),
            shared_file_id: "file-001".into(),
            blob_hash: [0xABu8; 32],
            size_bytes: 1024,
            blob_ticket: vec![1, 2, 3, 4],
            nonce: [0xCDu8; 32],
            issued_at_ms: 1000,
            expires_at_ms: 2000,
        };
        let bytes = payload.canonical_bytes().expect("canonical bytes");
        let expected = "\
            14 62 6f 72 75 2f 66 69 6c 65 2d 64 65 73 63 72 69 70 74 6f 72 \
            02 \
            00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
            01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 01 \
            08 66 69 6c 65 2d 30 30 31 \
            ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab \
            80 08 \
            04 01 02 03 04 \
            cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd cd \
            e8 07 \
            d0 0f";
        let expected_bytes: Vec<u8> = expected
            .split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
            .collect();
        assert_eq!(
            bytes, expected_bytes,
            "canonical descriptor payload bytes must be stable (BORU-AUDIT-05)"
        );
    }

    /// A signed descriptor round-trips: sign with the owner key, verify with
    /// the same owner key and requester.
    #[test]
    fn descriptor_sign_verify_round_trip() {
        let owner = iroh::SecretKey::generate();
        let requester = iroh::SecretKey::generate().public();
        let now = 1_700_000_000_000u64;
        let descriptor = sign_download_descriptor(
            &owner,
            requester,
            "shared-file-1".into(),
            [7u8; 32],
            4096,
            BlobFormat::Raw,
            now,
            now + 60_000,
        );
        let outcome = verify_download_descriptor(&descriptor, &owner.public(), &requester, now);
        assert_eq!(outcome, DescriptorVerification::Valid);
    }

    /// Changing any single signed field must invalidate the signature.
    #[test]
    fn descriptor_field_mutation_invalidates_signature() {
        let owner = iroh::SecretKey::generate();
        let owner_pk = owner.public();
        let requester = iroh::SecretKey::generate().public();
        let now = 1_700_000_000_000u64;
        let base = sign_download_descriptor(
            &owner,
            requester,
            "shared-file-1".into(),
            [7u8; 32],
            4096,
            BlobFormat::Raw,
            now,
            now + 60_000,
        );
        assert_eq!(
            verify_download_descriptor(&base, &owner_pk, &requester, now),
            DescriptorVerification::Valid
        );

        // Mutate each signed field one at a time; every one must break
        // verification (either signature or a specific identity/version check).
        let mut cases: Vec<(&str, SignedDownloadDescriptor)> = Vec::new();

        let mut d = base.clone();
        d.shared_file_id = "shared-file-2".into();
        cases.push(("shared_file_id", d));

        let mut d = base.clone();
        d.blob_hash = [8u8; 32];
        cases.push(("blob_hash", d));

        let mut d = base.clone();
        d.size_bytes = 9999;
        cases.push(("size_bytes", d));

        let mut d = base.clone();
        d.blob_ticket = vec![9, 9, 9];
        cases.push(("blob_ticket", d));

        let mut d = base.clone();
        d.nonce = [0xEEu8; 32];
        cases.push(("nonce", d));

        let mut d = base.clone();
        d.issued_at_ms = now + 1;
        cases.push(("issued_at_ms", d));

        let mut d = base.clone();
        d.expires_at_ms = now - 1;
        cases.push(("expires_at_ms", d));

        let mut d = base.clone();
        d.requester = iroh::SecretKey::generate().public();
        cases.push(("requester", d));

        let mut d = base.clone();
        d.owner_id = iroh::SecretKey::generate().public();
        cases.push(("owner_id", d));

        let mut d = base.clone();
        d.signed_version = 999;
        cases.push(("signed_version", d));

        for (field, mutated) in cases {
            let outcome = verify_download_descriptor(&mutated, &owner_pk, &requester, now);
            assert_ne!(
                outcome,
                DescriptorVerification::Valid,
                "mutating {field} must invalidate the descriptor"
            );
        }
    }

    /// Reordering JSON/map fields or changing display serialization must not
    /// change the canonical signed bytes: the signature covers the postcard
    /// struct bytes, not any JSON or human-readable representation.
    #[test]
    fn descriptor_json_reorder_does_not_affect_canonical_bytes() {
        let payload = DescriptorSignedPayloadV2 {
            protocol: DESCRIPTOR_SIGNED_PROTOCOL.to_string(),
            version: DESCRIPTOR_SIGNED_PAYLOAD_VERSION,
            owner_id: PublicKey::from_bytes(&[0u8; 32]).expect("valid key"),
            requester: PublicKey::from_bytes(&[1u8; 32]).expect("valid key"),
            shared_file_id: "file-001".into(),
            blob_hash: [0xABu8; 32],
            size_bytes: 1024,
            blob_ticket: vec![1, 2, 3, 4],
            nonce: [0xCDu8; 32],
            issued_at_ms: 1000,
            expires_at_ms: 2000,
        };
        let canonical = payload.canonical_bytes().expect("canonical bytes");

        // Serialize to JSON (human-readable: PublicKey becomes a string) and
        // back.  Even a JSON round-trip with keys in a different order must
        // produce the same canonical signed bytes.
        let json = serde_json::to_string(&payload).expect("json");
        let decoded: DescriptorSignedPayloadV2 = serde_json::from_str(&json).expect("from json");
        let canonical_again = decoded.canonical_bytes().expect("canonical bytes");
        assert_eq!(
            canonical, canonical_again,
            "JSON round-trip must not alter signed bytes"
        );

        // Manually reorder the JSON object keys and re-decode.
        let reordered_json = reorder_json_keys(&json);
        let re_decoded: DescriptorSignedPayloadV2 =
            serde_json::from_str(&reordered_json).expect("from reordered json");
        let canonical_third = re_decoded.canonical_bytes().expect("canonical bytes");
        assert_eq!(
            canonical, canonical_third,
            "JSON key order must not alter canonical signed bytes"
        );
    }

    /// Unknown signed-payload versions are rejected instead of guessed.
    #[test]
    fn descriptor_unknown_version_rejected() {
        let owner = iroh::SecretKey::generate();
        let owner_pk = owner.public();
        let requester = iroh::SecretKey::generate().public();
        let now = 1_700_000_000_000u64;
        let mut descriptor = sign_download_descriptor(
            &owner,
            requester,
            "shared-file-1".into(),
            [7u8; 32],
            4096,
            BlobFormat::Raw,
            now,
            now + 60_000,
        );
        descriptor.signed_version = DESCRIPTOR_SIGNED_PAYLOAD_VERSION + 1;
        assert_eq!(
            verify_download_descriptor(&descriptor, &owner_pk, &requester, now),
            DescriptorVerification::UnsupportedVersion,
            "unknown signed payload version must be rejected, not guessed"
        );
    }

    /// The display `content_hash` is NOT part of the signed payload: only the
    /// strongly typed `blob_hash` is authenticated.  (BORU-AUDIT-06 removes the
    /// duplicate string entirely.)
    #[test]
    fn descriptor_display_content_hash_not_signed() {
        let owner = iroh::SecretKey::generate();
        let owner_pk = owner.public();
        let requester = iroh::SecretKey::generate().public();
        let now = 1_700_000_000_000u64;
        let mut descriptor = sign_download_descriptor(
            &owner,
            requester,
            "shared-file-1".into(),
            [7u8; 32],
            4096,
            BlobFormat::Raw,
            now,
            now + 60_000,
        );
        // Change only the display string; the signature must still verify
        // because blob_hash (the canonical hash) is unchanged.
        descriptor.content_hash = "deadbeef".into();
        assert_eq!(
            verify_download_descriptor(&descriptor, &owner_pk, &requester, now),
            DescriptorVerification::Valid
        );
    }

    /// Helper: emit a JSON object with the top-level keys in reverse order.
    ///
    /// `serde_json::Map` is sorted by key, so rebuilding via `Map` cannot
    /// produce a different textual key order; this hand-rolled writer emits the
    /// same values with the keys genuinely reversed so the test proves key
    /// order cannot affect the canonical signed bytes.  The payload's JSON is
    /// flat (strings, numbers, arrays of numbers), which keeps the writer tiny.
    fn reorder_json_keys(json: &str) -> String {
        use serde_json::Value;
        let value: Value = serde_json::from_str(json).expect("parse json");
        let obj = value.as_object().expect("object");
        let mut out = String::from("{");
        let keys: Vec<&String> = obj.keys().collect();
        for (i, key) in keys.iter().rev().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("\"{key}\":"));
            let v = &obj[*key];
            match v {
                Value::Number(n) => out.push_str(&n.to_string()),
                Value::String(s) => out.push_str(&format!("\"{s}\"")),
                Value::Array(items) => {
                    out.push('[');
                    for (j, item) in items.iter().enumerate() {
                        if j > 0 {
                            out.push(',');
                        }
                        out.push_str(&item.to_string());
                    }
                    out.push(']');
                }
                other => panic!("unexpected JSON value in payload: {other}"),
            }
        }
        out.push('}');
        out
    }
}
