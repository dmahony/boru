//! Catalogue protocol types — wire-safe request, response, and error code types
//! for the `/iroh-chat-catalogue/1` protocol.
//!
//! # Design
//!
//! - All types are `Serialize` + `Deserialize` for wire transport via postcard.
//! - Error codes are stable, human-readable strings (snake_case), never raw Rust
//!   error internals.
//! - Unknown error codes received from a remote peer are mapped to
//!   [`CatalogErrorCode::InternalError`] so deserialization never panics on an
//!   unrecognised variant.

use serde::{Deserialize, Serialize};

// ── Stable Wire-Safe Error Codes ──────────────────────────────────────────

/// Stable, wire-safe error codes for the catalogue protocol.
///
/// These codes are sent in [`CatalogResponse::Error`] and are suitable for
/// presentation to remote peers — they contain no internal details, stack
/// traces, or sensitive information.
///
/// # Wire format
///
/// Each variant serializes as a snake_case string (e.g. `"permission_denied"`).
/// Unknown strings received from a remote peer are safely mapped to
/// [`InternalError`] — the deserializer never panics on an unrecognised value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogErrorCode {
    /// The caller lacks permission to access the requested resource.
    PermissionDenied,
    /// The requested resource was not found.
    NotFound,
    /// The request was malformed or contained invalid parameters.
    InvalidRequest,
    /// The protocol version is not supported by this server.
    UnsupportedVersion,
    /// The caller has been rate-limited; back off and retry.
    RateLimited,
    /// The server is busy and cannot process the request right now.
    Busy,
    /// The response exceeded the maximum allowed size.
    ResponseTooLarge,
    /// An internal server error occurred. No details are disclosed.
    InternalError,
}

impl<'de> Deserialize<'de> for CatalogErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Use a Visitor to read the string, then match known values or fall
        // back safely to InternalError.
        struct CatalogErrorCodeVisitor;

        impl<'de> serde::de::Visitor<'de> for CatalogErrorCodeVisitor {
            type Value = CatalogErrorCode;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a snake_case error code string")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<CatalogErrorCode, E> {
                Ok(match v {
                    "permission_denied" => CatalogErrorCode::PermissionDenied,
                    "not_found" => CatalogErrorCode::NotFound,
                    "invalid_request" => CatalogErrorCode::InvalidRequest,
                    "unsupported_version" => CatalogErrorCode::UnsupportedVersion,
                    "rate_limited" => CatalogErrorCode::RateLimited,
                    "busy" => CatalogErrorCode::Busy,
                    "response_too_large" => CatalogErrorCode::ResponseTooLarge,
                    "internal_error" => CatalogErrorCode::InternalError,
                    // Unknown values map safely to InternalError — never
                    // crash on an unrecognised remote error code.
                    _ => CatalogErrorCode::InternalError,
                })
            }
        }

        deserializer.deserialize_str(CatalogErrorCodeVisitor)
    }
}

impl CatalogErrorCode {
    /// Return the canonical snake_case string representation of this code.
    ///
    /// This is the same string that would be produced by serde serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogErrorCode::PermissionDenied => "permission_denied",
            CatalogErrorCode::NotFound => "not_found",
            CatalogErrorCode::InvalidRequest => "invalid_request",
            CatalogErrorCode::UnsupportedVersion => "unsupported_version",
            CatalogErrorCode::RateLimited => "rate_limited",
            CatalogErrorCode::Busy => "busy",
            CatalogErrorCode::ResponseTooLarge => "response_too_large",
            CatalogErrorCode::InternalError => "internal_error",
        }
    }
}

impl std::fmt::Display for CatalogErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Protocol Request / Response Types ─────────────────────────────────────

/// A request sent by the client to the catalogue server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CatalogRequest {
    /// Fetch a page of the remote file catalogue.
    GetCataloguePage {
        /// The revision the client already knows (if any).  When this matches
        /// the server's current revision, the server returns
        /// [`CatalogResponse::RevisionChanged`] with the new revision so the
        /// client can request a full refresh.
        known_revision: Option<u64>,
        /// Opaque pagination cursor.  `None` for the first page.
        cursor: Option<String>,
        /// Maximum number of items to return in this page.
        page_size: u32,
    },
}

/// A page of catalogue data returned in [`CatalogResponse::CataloguePage`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CataloguePage {
    /// The catalogue revision at the time this page was generated.
    pub revision: u64,
    /// The file items in this page.
    pub items: Vec<crate::catalogue_model::RemoteSharedFile>,
    /// Opaque cursor for the next page.  `None` when this is the last page.
    pub next_cursor: Option<String>,
}

impl CataloguePage {
    /// Verify the integrity of this page (placeholder).
    ///
    /// In the current implementation, catalogue pages are served over an
    /// authenticated QUIC connection and signed by the owning profile's
    /// secret key at the catalogue level.  Individual page verification is
    /// deferred to the complete [`SignedFileCatalogue`] verification.
    pub fn verify(&self) -> Result<(), &'static str> {
        // Each item's content_hash is verified against the blake3 hash at
        // download time.  Page-level verification is a no-op for now.
        Ok(())
    }
}

/// A response from the catalogue server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CatalogResponse {
    /// A successful page of catalogue data.
    CataloguePage(CataloguePage),
    /// The server's revision has changed since the client's last request.
    /// The client should re-fetch from the beginning.
    RevisionChanged {
        /// The new current revision number.
        new_revision: u64,
    },
    /// An error occurred.  The [`CatalogErrorCode`] provides a stable,
    /// non-sensitive classification.  The message is a human-readable
    /// explanation suitable for logging or display.
    Error {
        /// Stable error code.
        code: CatalogErrorCode,
        /// Human-readable explanation (may be disclosed to remote peer).
        message: String,
    },
}

// ── Error helpers ─────────────────────────────────────────────────────────

/// Convenience constructor for an error response.
impl CatalogResponse {
    /// Create an [`CatalogResponse::Error`] with the given code and message.
    pub fn error(code: CatalogErrorCode, message: impl Into<String>) -> Self {
        CatalogResponse::Error {
            code,
            message: message.into(),
        }
    }

    /// Create an [`CatalogResponse::Error`] with [`CatalogErrorCode::InternalError`]
    /// and a generic message — no internal details are leaked.
    pub fn internal_error() -> Self {
        CatalogResponse::Error {
            code: CatalogErrorCode::InternalError,
            message: "An internal error occurred".to_string(),
        }
    }
}

// ── Internal-to-public mapping ────────────────────────────────────────────

/// Convert any internal error type into a stable [`CatalogResponse`] error,
/// ensuring no raw Rust error internals reach the wire.
impl From<std::string::FromUtf8Error> for CatalogResponse {
    fn from(_: std::string::FromUtf8Error) -> Self {
        CatalogResponse::internal_error()
    }
}

// ── Versioned Wire Wrappers ────────────────────────────────────────────────

/// Current wire version for catalogue protocol messages.
///
/// Bump this when making a backwards-incompatible change to `CatalogRequest`
/// or `CatalogResponse` wire format.
pub const CATALOGUE_WIRE_VERSION: u16 = 1;

/// All wire versions that the current code understands.
///
/// When a new version is added, append it here and update the rejection
/// test to cover the gap.
pub const SUPPORTED_CATALOGUE_VERSIONS: &[u16] = &[1];

/// Versioned wire wrapper for catalogue requests.
///
/// Embed `version` directly in the message body so that compatibility
/// does **not** depend solely on the ALPN string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogWireRequest {
    /// Wire protocol version.
    pub version: u16,
    /// The inner request payload.
    pub inner: CatalogRequest,
}

impl CatalogWireRequest {
    /// Create a new wire request with the current [`CATALOGUE_WIRE_VERSION`].
    pub fn new(inner: CatalogRequest) -> Self {
        Self {
            version: CATALOGUE_WIRE_VERSION,
            inner,
        }
    }

    /// Validate that `self.version` is in [`SUPPORTED_CATALOGUE_VERSIONS`].
    ///
    /// Returns `Ok(())` or [`CatalogErrorCode::UnsupportedVersion`].
    pub fn validate_version(&self) -> Result<(), CatalogErrorCode> {
        if SUPPORTED_CATALOGUE_VERSIONS.contains(&self.version) {
            Ok(())
        } else {
            Err(CatalogErrorCode::UnsupportedVersion)
        }
    }
}

/// Versioned wire wrapper for catalogue responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogWireResponse {
    /// Wire protocol version.
    pub version: u16,
    /// The inner response payload.
    ///
    /// [`CatalogResponse`] already carries an [`CatalogResponse::Error`]
    /// variant for wire-safe error reporting.
    pub inner: CatalogResponse,
}

impl CatalogWireResponse {
    /// Create a new wire response with the current [`CATALOGUE_WIRE_VERSION`].
    pub fn new(inner: CatalogResponse) -> Self {
        Self {
            version: CATALOGUE_WIRE_VERSION,
            inner,
        }
    }

    /// Validate that `self.version` is in [`SUPPORTED_CATALOGUE_VERSIONS`].
    ///
    /// Returns `Ok(())` or [`CatalogErrorCode::UnsupportedVersion`].
    pub fn validate_version(&self) -> Result<(), CatalogErrorCode> {
        if SUPPORTED_CATALOGUE_VERSIONS.contains(&self.version) {
            Ok(())
        } else {
            Err(CatalogErrorCode::UnsupportedVersion)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    /// All known error codes round-trip through serde_json serialization.
    #[test]
    fn test_catalog_error_code_serde_roundtrip() {
        let codes = vec![
            CatalogErrorCode::PermissionDenied,
            CatalogErrorCode::NotFound,
            CatalogErrorCode::InvalidRequest,
            CatalogErrorCode::UnsupportedVersion,
            CatalogErrorCode::RateLimited,
            CatalogErrorCode::Busy,
            CatalogErrorCode::ResponseTooLarge,
            CatalogErrorCode::InternalError,
        ];

        for code in &codes {
            let json = serde_json::to_string(code).unwrap();
            let deserialized: CatalogErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, code, "roundtrip failed for {code:?}");
        }
    }

    /// Known error codes serialize to the expected snake_case strings.
    #[test]
    fn test_catalog_error_code_serialized_form() {
        assert_eq!(
            serde_json::to_string(&CatalogErrorCode::PermissionDenied).unwrap(),
            "\"permission_denied\""
        );
        assert_eq!(
            serde_json::to_string(&CatalogErrorCode::NotFound).unwrap(),
            "\"not_found\""
        );
        assert_eq!(
            serde_json::to_string(&CatalogErrorCode::InvalidRequest).unwrap(),
            "\"invalid_request\""
        );
        assert_eq!(
            serde_json::to_string(&CatalogErrorCode::UnsupportedVersion).unwrap(),
            "\"unsupported_version\""
        );
        assert_eq!(
            serde_json::to_string(&CatalogErrorCode::RateLimited).unwrap(),
            "\"rate_limited\""
        );
        assert_eq!(
            serde_json::to_string(&CatalogErrorCode::Busy).unwrap(),
            "\"busy\""
        );
        assert_eq!(
            serde_json::to_string(&CatalogErrorCode::ResponseTooLarge).unwrap(),
            "\"response_too_large\""
        );
        assert_eq!(
            serde_json::to_string(&CatalogErrorCode::InternalError).unwrap(),
            "\"internal_error\""
        );
    }

    /// Unknown error codes from the wire are safely mapped to InternalError.
    #[test]
    fn test_catalog_error_code_unknown_fallback() {
        let cases = vec![
            "\"unknown_variant\"",
            "\"\"",
            "\"some_new_error_code\"",
            "\"INTERNAL_ERROR\"",   // wrong case
            "\"PermissionDenied\"", // wrong case (not snake_case)
        ];

        for input in &cases {
            let result: Result<CatalogErrorCode, _> = serde_json::from_str(input);
            assert!(
                result.is_ok(),
                "deserializing {input} should not panic; got error: {:?}",
                result.err()
            );
            let code = result.unwrap();
            assert_eq!(
                code,
                CatalogErrorCode::InternalError,
                "unknown input {input} should map to InternalError, got {code:?}"
            );
        }
    }

    /// `as_str()` returns the canonical snake_case string.
    #[test]
    fn test_catalog_error_code_as_str() {
        assert_eq!(
            CatalogErrorCode::PermissionDenied.as_str(),
            "permission_denied"
        );
        assert_eq!(CatalogErrorCode::NotFound.as_str(), "not_found");
        assert_eq!(CatalogErrorCode::InternalError.as_str(), "internal_error");
    }

    /// `Display` produces the same string as `as_str()`.
    #[test]
    fn test_catalog_error_code_display() {
        assert_eq!(
            format!("{}", CatalogErrorCode::PermissionDenied),
            "permission_denied"
        );
        assert_eq!(
            format!("{}", CatalogErrorCode::InternalError),
            "internal_error"
        );
    }

    /// `CatalogResponse::error()` creates an error response with the given code.
    #[test]
    fn test_catalog_response_error_constructor() {
        let resp = CatalogResponse::error(CatalogErrorCode::PermissionDenied, "not allowed");
        match resp {
            CatalogResponse::Error { code, message } => {
                assert_eq!(code, CatalogErrorCode::PermissionDenied);
                assert_eq!(message, "not allowed");
            }
            other => panic!("expected Error variant, got {other:?}"),
        }
    }

    /// `CatalogResponse::internal_error()` creates a safe generic error.
    #[test]
    fn test_catalog_response_internal_error() {
        let resp = CatalogResponse::internal_error();
        match resp {
            CatalogResponse::Error { code, message } => {
                assert_eq!(code, CatalogErrorCode::InternalError);
                // Message must not leak internal details.
                assert!(!message.is_empty());
                assert_ne!(message, "something went wrong in module xyz");
            }
            other => panic!("expected Error variant, got {other:?}"),
        }
    }

    /// `CatalogResponse::Error` round-trips through postcard (binary wire format).
    #[test]
    fn test_catalog_error_response_postcard_roundtrip() {
        let resp = CatalogResponse::error(CatalogErrorCode::RateLimited, "too many requests");
        let bytes = postcard::to_stdvec(&resp).expect("postcard serialize");
        let deserialized: CatalogResponse =
            postcard::from_bytes(&bytes).expect("postcard deserialize");
        assert_eq!(deserialized, resp);
    }

    /// A successful `CataloguePage` round-trips through postcard.
    #[test]
    fn test_catalogue_page_response_postcard_roundtrip() {
        // Create a minimal CataloguePage with an empty items vec.
        let page = CataloguePage {
            revision: 42,
            items: vec![],
            next_cursor: Some("cursor-abc".to_string()),
        };
        let resp = CatalogResponse::CataloguePage(page);
        let bytes = postcard::to_stdvec(&resp).expect("postcard serialize");
        let deserialized: CatalogResponse =
            postcard::from_bytes(&bytes).expect("postcard deserialize");
        match &deserialized {
            CatalogResponse::CataloguePage(p) => {
                assert_eq!(p.revision, 42);
                assert_eq!(p.next_cursor.as_deref(), Some("cursor-abc"));
                assert!(p.items.is_empty());
            }
            other => panic!("expected CataloguePage, got {other:?}"),
        }
        assert_eq!(deserialized, resp);
    }

    /// Unknown error codes in postcard binary format also fall back safely.
    #[test]
    fn test_catalog_error_code_postcard_unknown_fallback() {
        // Serialize an unknown error code via postcard using the raw string.
        let bytes = postcard::to_stdvec(&"some_new_error").expect("postcard serialize raw");
        // The bytes contain just the string "some_new_error" encoded in postcard format.
        let result: Result<CatalogErrorCode, _> = postcard::from_bytes(&bytes);
        assert!(result.is_ok(), "unknown postcard value should not panic");
        assert_eq!(
            result.unwrap(),
            CatalogErrorCode::InternalError,
            "unknown value should map to InternalError"
        );
    }

    /// `RevisionChanged` response round-trips.
    #[test]
    fn test_revision_changed_roundtrip() {
        let resp = CatalogResponse::RevisionChanged { new_revision: 99 };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: CatalogResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, resp);
    }

    /// `CatalogRequest` round-trips through postcard.
    #[test]
    fn test_catalog_request_roundtrip() {
        let req = CatalogRequest::GetCataloguePage {
            known_revision: Some(42),
            cursor: Some("abc".to_string()),
            page_size: 10,
        };
        let bytes = postcard::to_stdvec(&req).expect("postcard serialize");
        let deserialized: CatalogRequest =
            postcard::from_bytes(&bytes).expect("postcard deserialize");
        assert_eq!(deserialized, req);
    }

    // ── Wire wrapper tests ────────────────────────────────────────────────

    #[test]
    fn test_catalogue_wire_request_round_trip() {
        let inner = CatalogRequest::GetCataloguePage {
            known_revision: Some(42),
            cursor: Some("abc".into()),
            page_size: 50,
        };
        let original = CatalogWireRequest::new(inner);
        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let decoded: CatalogWireRequest = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
        assert_eq!(decoded.version, CATALOGUE_WIRE_VERSION);
    }

    #[test]
    fn test_catalogue_wire_response_round_trip() {
        let page = CataloguePage {
            revision: 1,
            items: vec![],
            next_cursor: None,
        };
        let original = CatalogWireResponse::new(CatalogResponse::CataloguePage(page));
        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let decoded: CatalogWireResponse = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
        assert_eq!(decoded.version, CATALOGUE_WIRE_VERSION);
    }

    #[test]
    fn test_catalogue_wire_request_rejects_unsupported_version() {
        let inner = CatalogRequest::GetCataloguePage {
            known_revision: None,
            cursor: None,
            page_size: 10,
        };
        let msg = CatalogWireRequest {
            version: 999,
            inner,
        };
        assert_eq!(
            msg.validate_version(),
            Err(CatalogErrorCode::UnsupportedVersion)
        );
    }

    #[test]
    fn test_catalogue_wire_response_rejects_unsupported_version() {
        let msg = CatalogWireResponse {
            version: 0,
            inner: CatalogResponse::internal_error(),
        };
        assert_eq!(
            msg.validate_version(),
            Err(CatalogErrorCode::UnsupportedVersion)
        );
    }

    #[test]
    fn test_catalogue_wire_request_version_zero_rejected() {
        let inner = CatalogRequest::GetCataloguePage {
            known_revision: None,
            cursor: None,
            page_size: 10,
        };
        let msg = CatalogWireRequest { version: 0, inner };
        assert_eq!(
            msg.validate_version(),
            Err(CatalogErrorCode::UnsupportedVersion)
        );
    }

    #[test]
    fn test_catalogue_wire_current_version_is_valid() {
        let msg = CatalogWireRequest::new(CatalogRequest::GetCataloguePage {
            known_revision: None,
            cursor: None,
            page_size: 10,
        });
        assert!(msg.validate_version().is_ok());
    }

    #[test]
    fn test_catalogue_wire_request_truncated_fails() {
        let inner = CatalogRequest::GetCataloguePage {
            known_revision: None,
            cursor: None,
            page_size: 10,
        };
        let original = CatalogWireRequest::new(inner);
        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let truncated = &bytes[..bytes.len().saturating_sub(4)];
        let result: Result<CatalogWireRequest, _> = postcard::from_bytes(truncated);
        assert!(
            result.is_err(),
            "truncated message should fail to deserialize"
        );
    }

    #[test]
    fn test_catalogue_wire_response_truncated_fails() {
        let page = CataloguePage {
            revision: 1,
            items: vec![],
            next_cursor: None,
        };
        let original = CatalogWireResponse::new(CatalogResponse::CataloguePage(page));
        let bytes = postcard::to_stdvec(&original).expect("serialize");
        let truncated = &bytes[..bytes.len().saturating_sub(1)];
        let result: Result<CatalogWireResponse, _> = postcard::from_bytes(truncated);
        assert!(
            result.is_err(),
            "truncated message should fail to deserialize"
        );
    }

    #[test]
    fn test_catalogue_wire_empty_fails() {
        let result: Result<CatalogWireRequest, _> = postcard::from_bytes(&[]);
        assert!(result.is_err(), "empty message should fail to deserialize");
    }

    #[test]
    fn test_catalogue_wire_request_trailing_data_rejected() {
        let inner = CatalogRequest::GetCataloguePage {
            known_revision: None,
            cursor: None,
            page_size: 10,
        };
        let original = CatalogWireRequest::new(inner);
        let mut bytes = postcard::to_stdvec(&original).expect("serialize");
        bytes.extend_from_slice(b"TRAILING_GARBAGE");
        // Use take_from_bytes to detect trailing data.
        let result: Result<(CatalogWireRequest, &[u8]), _> = postcard::take_from_bytes(&bytes);
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
    fn test_catalogue_wire_response_trailing_data_rejected() {
        let original = CatalogWireResponse::new(CatalogResponse::internal_error());
        let mut bytes = postcard::to_stdvec(&original).expect("serialize");
        bytes.extend_from_slice(b"\xDE\xAD");
        let result: Result<(CatalogWireResponse, &[u8]), _> = postcard::take_from_bytes(&bytes);
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
    fn test_supported_versions_contains_current() {
        assert!(
            SUPPORTED_CATALOGUE_VERSIONS.contains(&CATALOGUE_WIRE_VERSION),
            "SUPPORTED_CATALOGUE_VERSIONS must include CATALOGUE_WIRE_VERSION"
        );
    }

    #[test]
    fn test_supported_versions_is_sorted_and_unique() {
        let mut sorted = SUPPORTED_CATALOGUE_VERSIONS.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            SUPPORTED_CATALOGUE_VERSIONS,
            sorted.as_slice(),
            "SUPPORTED_CATALOGUE_VERSIONS must be sorted and unique"
        );
    }
}
