//! File access protocol types — wire-safe error codes and request/response
//! types for the `/iroh-chat-transfer-auth/1` protocol.
//!
//! # Design
//!
//! - All types are `Serialize` + `Deserialize` for wire transport via postcard.
//! - Error codes mirror [`crate::catalogue_protocol::CatalogErrorCode`] for
//!   consistency across protocols.
//! - Unknown error codes received from a remote peer are safely mapped to
//!   [`FileAccessErrorCode::InternalError`].

use serde::{Deserialize, Serialize};

// ── Stable Wire-Safe Error Codes ──────────────────────────────────────────

/// Stable, wire-safe error codes for the file access protocol.
///
/// These codes are returned in access-denied or error responses for
/// `/iroh-chat-transfer-auth/1`.  They contain no internal details and are
/// safe to send to remote peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum FileAccessErrorCode {
    /// The caller lacks permission to download the requested file.
    PermissionDenied,
    /// The requested file or blob was not found.
    NotFound,
    /// The request was malformed.
    InvalidRequest,
    /// The protocol version is not supported.
    UnsupportedVersion,
    /// The caller has been rate-limited.
    RateLimited,
    /// The server is busy.
    Busy,
    /// The requested content is too large to transfer.
    ResponseTooLarge,
    /// An internal server error occurred.
    InternalError,
}

const UNKNOWN_FILE_ACCESS_ERROR: &str = "internal_error";

impl<'de> Deserialize<'de> for FileAccessErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FileAccessErrorCodeVisitor;

        impl<'de> serde::de::Visitor<'de> for FileAccessErrorCodeVisitor {
            type Value = FileAccessErrorCode;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a snake_case file access error code string")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<FileAccessErrorCode, E> {
                Ok(match v {
                    "permission_denied" => FileAccessErrorCode::PermissionDenied,
                    "not_found" => FileAccessErrorCode::NotFound,
                    "invalid_request" => FileAccessErrorCode::InvalidRequest,
                    "unsupported_version" => FileAccessErrorCode::UnsupportedVersion,
                    "rate_limited" => FileAccessErrorCode::RateLimited,
                    "busy" => FileAccessErrorCode::Busy,
                    "response_too_large" => FileAccessErrorCode::ResponseTooLarge,
                    "internal_error" => FileAccessErrorCode::InternalError,
                    _ => FileAccessErrorCode::InternalError,
                })
            }
        }

        deserializer.deserialize_str(FileAccessErrorCodeVisitor)
    }
}

impl FileAccessErrorCode {
    /// Return the canonical snake_case string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            FileAccessErrorCode::PermissionDenied => "permission_denied",
            FileAccessErrorCode::NotFound => "not_found",
            FileAccessErrorCode::InvalidRequest => "invalid_request",
            FileAccessErrorCode::UnsupportedVersion => "unsupported_version",
            FileAccessErrorCode::RateLimited => "rate_limited",
            FileAccessErrorCode::Busy => "busy",
            FileAccessErrorCode::ResponseTooLarge => "response_too_large",
            FileAccessErrorCode::InternalError => "internal_error",
        }
    }
}

impl std::fmt::Display for FileAccessErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Protocol Request / Response Types ─────────────────────────────────────

/// A request for authorisation to download a file.
///
/// Sent by the downloader to the file owner over `/iroh-chat-transfer-auth/1`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileAccessRequest {
    /// The BLAKE3 content hash of the requested file (hex-encoded).
    pub content_hash: String,
    /// The catalogue revision at which the file was offered.
    pub catalogue_revision: u64,
}

/// Outcome of a file access authorisation request.
///
/// On success, the response carries a signed download descriptor.  On failure,
/// it carries a stable error code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileAccessResponse {
    /// The request was approved.  The payload is a short-lived, requester-bound,
    /// Ed25519-signed download ticket.
    Approved(DownloadTicket),
    /// The request was denied or an error occurred.
    Denied {
        /// Stable error code.
        code: FileAccessErrorCode,
        /// Human-readable explanation.
        message: String,
    },
}

/// A short-lived, requester-bound download ticket.
///
/// Authorises the named requester to download a specific blob via iroh-blobs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadTicket {
    /// The BLAKE3 content hash of the file (hex-encoded).
    pub content_hash: String,
    /// The authorised requester's public key (hex-encoded).
    pub requester: String,
    /// Unix timestamp (seconds) when this ticket expires.
    pub expires_at: u64,
    /// Ed25519 signature from the file owner covering the above fields.
    pub signature: Vec<u8>,
}

// ── Error helpers ─────────────────────────────────────────────────────────

impl FileAccessResponse {
    /// Create a denied response with the given error code and message.
    pub fn denied(code: FileAccessErrorCode, message: impl Into<String>) -> Self {
        FileAccessResponse::Denied {
            code,
            message: message.into(),
        }
    }

    /// Create a denied response with [`FileAccessErrorCode::InternalError`]
    /// and a generic message — no internal details are leaked.
    pub fn internal_error() -> Self {
        FileAccessResponse::Denied {
            code: FileAccessErrorCode::InternalError,
            message: "An internal error occurred".to_string(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    /// All known error codes round-trip through serde_json.
    #[test]
    fn test_file_access_error_code_serde_roundtrip() {
        let codes = vec![
            FileAccessErrorCode::PermissionDenied,
            FileAccessErrorCode::NotFound,
            FileAccessErrorCode::InvalidRequest,
            FileAccessErrorCode::UnsupportedVersion,
            FileAccessErrorCode::RateLimited,
            FileAccessErrorCode::Busy,
            FileAccessErrorCode::ResponseTooLarge,
            FileAccessErrorCode::InternalError,
        ];

        for code in &codes {
            let json = serde_json::to_string(code).unwrap();
            let deserialized: FileAccessErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, code, "roundtrip failed for {code:?}");
        }
    }

    /// Known error codes serialize to the expected snake_case strings.
    #[test]
    fn test_file_access_error_code_serialized_form() {
        assert_eq!(
            serde_json::to_string(&FileAccessErrorCode::PermissionDenied).unwrap(),
            "\"permission_denied\""
        );
        assert_eq!(
            serde_json::to_string(&FileAccessErrorCode::NotFound).unwrap(),
            "\"not_found\""
        );
        assert_eq!(
            serde_json::to_string(&FileAccessErrorCode::InvalidRequest).unwrap(),
            "\"invalid_request\""
        );
        assert_eq!(
            serde_json::to_string(&FileAccessErrorCode::UnsupportedVersion).unwrap(),
            "\"unsupported_version\""
        );
        assert_eq!(
            serde_json::to_string(&FileAccessErrorCode::RateLimited).unwrap(),
            "\"rate_limited\""
        );
        assert_eq!(
            serde_json::to_string(&FileAccessErrorCode::Busy).unwrap(),
            "\"busy\""
        );
        assert_eq!(
            serde_json::to_string(&FileAccessErrorCode::ResponseTooLarge).unwrap(),
            "\"response_too_large\""
        );
        assert_eq!(
            serde_json::to_string(&FileAccessErrorCode::InternalError).unwrap(),
            "\"internal_error\""
        );
    }

    /// Unknown error codes from the wire are safely mapped to InternalError.
    #[test]
    fn test_file_access_error_code_unknown_fallback() {
        let cases = vec![
            "\"unknown_variant\"",
            "\"\"",
            "\"some_new_error\"",
            "\"PERMISSION_DENIED\"",
        ];

        for input in &cases {
            let result: Result<FileAccessErrorCode, _> = serde_json::from_str(input);
            assert!(
                result.is_ok(),
                "deserializing {input} should not panic; got error: {:?}",
                result.err()
            );
            let code = result.unwrap();
            assert_eq!(
                code,
                FileAccessErrorCode::InternalError,
                "unknown input {input} should map to InternalError, got {code:?}"
            );
        }
    }

    /// `as_str()` returns the canonical snake_case string.
    #[test]
    fn test_file_access_error_code_as_str() {
        assert_eq!(
            FileAccessErrorCode::PermissionDenied.as_str(),
            "permission_denied"
        );
        assert_eq!(FileAccessErrorCode::NotFound.as_str(), "not_found");
        assert_eq!(
            FileAccessErrorCode::InternalError.as_str(),
            "internal_error"
        );
    }

    /// `Display` produces the same string as `as_str()`.
    #[test]
    fn test_file_access_error_code_display() {
        assert_eq!(
            format!("{}", FileAccessErrorCode::PermissionDenied),
            "permission_denied"
        );
        assert_eq!(
            format!("{}", FileAccessErrorCode::InternalError),
            "internal_error"
        );
    }

    /// `FileAccessResponse::denied()` creates a denied response with the given code.
    #[test]
    fn test_file_access_response_denied_constructor() {
        let resp = FileAccessResponse::denied(FileAccessErrorCode::NotFound, "file not found");
        match resp {
            FileAccessResponse::Denied { code, message } => {
                assert_eq!(code, FileAccessErrorCode::NotFound);
                assert_eq!(message, "file not found");
            }
            other => panic!("expected Denied variant, got {other:?}"),
        }
    }

    /// `FileAccessResponse::internal_error()` creates a safe generic error.
    #[test]
    fn test_file_access_response_internal_error() {
        let resp = FileAccessResponse::internal_error();
        match resp {
            FileAccessResponse::Denied { code, message } => {
                assert_eq!(code, FileAccessErrorCode::InternalError);
                // Message must not leak internal details.
                assert!(!message.is_empty());
                assert!(!message.contains("panic") && !message.contains("unwrap"));
            }
            other => panic!("expected Denied variant, got {other:?}"),
        }
    }

    /// `FileAccessResponse` round-trips through postcard.
    #[test]
    fn test_file_access_response_postcard_roundtrip() {
        let resp = FileAccessResponse::denied(FileAccessErrorCode::RateLimited, "back off");
        let bytes = postcard::to_stdvec(&resp).expect("postcard serialize");
        let deserialized: FileAccessResponse =
            postcard::from_bytes(&bytes).expect("postcard deserialize");
        assert_eq!(deserialized, resp);
    }

    /// `FileAccessRequest` round-trips through postcard.
    #[test]
    fn test_file_access_request_roundtrip() {
        let req = FileAccessRequest {
            content_hash: "abcdef0123456789".to_string(),
            catalogue_revision: 42,
        };
        let bytes = postcard::to_stdvec(&req).expect("postcard serialize");
        let deserialized: FileAccessRequest =
            postcard::from_bytes(&bytes).expect("postcard deserialize");
        assert_eq!(deserialized, req);
    }

    /// `DownloadTicket` round-trips through postcard.
    #[test]
    fn test_download_ticket_roundtrip() {
        let ticket = DownloadTicket {
            content_hash: "deadbeef".to_string(),
            requester: "peer_pk_hex".to_string(),
            expires_at: 1000000,
            signature: vec![1, 2, 3, 4],
        };
        let bytes = postcard::to_stdvec(&ticket).expect("postcard serialize");
        let deserialized: DownloadTicket =
            postcard::from_bytes(&bytes).expect("postcard deserialize");
        assert_eq!(deserialized, ticket);
    }
}
