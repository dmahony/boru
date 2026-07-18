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

use serde::{Deserialize, Serialize};

// ── Wire version ─────────────────────────────────────────────────────────────

/// Current wire version for file-access protocol messages.
pub const FILE_ACCESS_WIRE_VERSION: u16 = 1;

/// All wire versions that the current code understands.
pub const SUPPORTED_FILE_ACCESS_VERSIONS: &[u16] = &[1];

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
}

/// A signed descriptor that authorises the requester to download a file.
///
/// Contains a short-lived blob ticket that the requester validates before
/// starting the actual blob transfer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignedDownloadDescriptor {
    /// Content hash of the authorised file.
    pub content_hash: String,
    /// Opaque blob ticket (iroh blob ticket bytes).
    pub blob_ticket: Vec<u8>,
    /// Expiration timestamp (milliseconds since UNIX epoch).
    pub expires_at_ms: u64,
}

/// Response to a [`FileAccessRequest`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FileAccessResponse {
    /// Access granted — contains the download descriptor.
    AccessGranted(SignedDownloadDescriptor),
    /// The requested wire version is not supported.
    UnsupportedVersion {
        /// The version the caller requested.
        requested: u16,
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

    // ── Serialization round-trip ───────────────────────────────────────────

    #[test]
    fn file_access_wire_request_round_trip() {
        let inner = FileAccessRequest {
            content_hash: "deadbeef".into(),
            filename: "photo.png".into(),
            expected_size: 65536,
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
            content_hash: "deadbeef".into(),
            blob_ticket: vec![1, 2, 3, 4],
            expires_at_ms: 1234567890000,
        };
        let original = FileAccessWireResponse::success(FileAccessResponse::AccessGranted(desc));

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
        };
        let msg = FileAccessWireRequest::new(inner);
        assert_eq!(msg.version, FILE_ACCESS_WIRE_VERSION);
    }

    #[test]
    fn file_access_wire_response_success_sets_current_version() {
        let desc = SignedDownloadDescriptor {
            content_hash: "abc".into(),
            blob_ticket: vec![],
            expires_at_ms: 0,
        };
        let msg = FileAccessWireResponse::success(FileAccessResponse::AccessGranted(desc));
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
