//! Central size and count limits for catalogue protocol traffic.
//!
//! All hard limits enforced by the server (handler) and client are defined
//! here. Values are chosen to prevent resource exhaustion while supporting
//! legitimate usage patterns. The [`CatalogueLimitsConfig`] type provides a
//! JSON-loadable, validated configuration for deployments that need to tune
//! these limits without rebuilding the application.

//!
//! # Limit catalogue (all documented hard limits)
//!
//! | Limit | Value | Enforced at |
//! |---|---|---|
//! | Max request payload bytes | 256 KiB | Handler (server) — reject oversized requests |
//! | Max response payload bytes | 4 MiB | Client (receive) + Handler (send) — reject oversized responses |
//! | Max files per catalogue | 10,000 | Handler (build) + Client (receive) |
//! | Max collections per catalogue | 1,000 | Handler (build) + Client (receive) |
//! | Max file name (display_name) length | 512 bytes | [`RemoteSharedFile::validate`] |
//! | Max description length | 1,024 UTF-8 bytes; only tab, CR, and LF controls are allowed; Unicode format and line/paragraph-separator characters are rejected | [`RemoteSharedFile::validate`], [`RemoteCollection::validate`] |
//! | Max mime_type length | 128 bytes | [`RemoteSharedFile::validate`] |
//! | Max content_hash length | 128 bytes | [`RemoteSharedFile::validate`] |
//! | Max shared_file_id length | 256 bytes | [`RemoteSharedFile::validate`] |
//! | Max collections per file | 256 | [`RemoteSharedFile::validate`] |
//! | Max collection_id length | 256 bytes | [`RemoteCollection::validate`] |
//! | Max collection name length | 512 bytes | [`RemoteCollection::validate`] |
//! | Max individual file size | 10 TiB | Handler (build) — checked in catalogue construction |
//! | Max file-details response bytes | 256 KiB | Handler (send) + Client (receive) |

use serde::{Deserialize, Serialize};
use std::fmt;

#[cfg(feature = "net")]
use std::{fs, path::Path};

/// Maximum serialized wire-format request payload in bytes.
///
/// Catalogue requests are tiny — a version u16, an enum discriminant, and
/// a few optional/small fields.  256 KiB is generous and allows for future
/// extension without risking large allocations.
pub const MAX_CATALOGUE_REQUEST_BYTES: usize = 256 * 1024; // 256 KiB

/// Maximum serialized wire-format response payload in bytes.
///
/// Catalogue responses carry up to [`MAX_CATALOGUE_FILES`] file entries and
/// [`MAX_COLLECTIONS`] collection entries.  4 MiB allows a sizeable catalogue
/// while bounding memory allocation on the receiving end.
pub const MAX_CATALOGUE_RESPONSE_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

/// Maximum number of serialized bytes in one paginated catalogue response.
pub const MAX_CATALOGUE_PAGE_BYTES: usize = 1024 * 1024; // 1 MiB

/// Maximum number of files in a single catalogue response.
///
/// Beyond this count the server truncates or returns
/// [`CatalogErrorCode::InvalidRequest`].
pub const MAX_CATALOGUE_FILES: usize = 10_000;

/// Maximum number of collections in a single catalogue response.
pub const MAX_COLLECTIONS: usize = 1_000;

/// Maximum number of file entries that may reference one collection.
pub const MAX_ENTRIES_PER_COLLECTION: usize = 10_000;

/// Maximum number of files returned in one page.
pub const MAX_CATALOGUE_PAGE_SIZE: u32 = 500;

/// Maximum invalid catalogue responses tolerated by one fetch operation.
pub const MAX_INVALID_RESPONSE_ATTEMPTS: usize = 3;

/// Maximum size of a single file's `size_bytes` field (in bytes).
///
/// 10 TiB covers any plausible real-world file size while rejecting
/// obviously bogus or overflow values.
pub const MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024 * 1024; // 10 TiB

/// Maximum serialized wire-format [`CatalogResponse::FileDetails`] payload
/// in bytes.  File details are a single [`RemoteSharedFile`] – 256 KiB is
/// far beyond what a single file entry needs.
pub const MAX_FILE_DETAILS_PAYLOAD_BYTES: usize = 256 * 1024; // 256 KiB

/// Tunable catalogue admission and resource limits.
///
/// The JSON representation uses these field names directly. Missing fields
/// take the documented defaults. Use [`CatalogueLimitsConfig::load_from_path`]
/// to read and validate a JSON configuration file before constructing the
/// catalogue handlers.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct CatalogueLimitsConfig {
    /// Maximum files in one catalogue.
    pub max_files_per_catalogue: usize,
    /// Maximum collections in one catalogue.
    pub max_collections: usize,
    /// Maximum entries in one collection.
    pub max_entries_per_collection: usize,
    /// Maximum files returned in one page.
    pub max_page_size: u32,
    /// Maximum serialized bytes in one page response.
    pub max_total_page_bytes: usize,
    /// Maximum requests allowed from one peer during the request window.
    pub max_requests_per_window: u32,
    /// Length of the per-peer request window, in seconds.
    pub request_window_seconds: u64,
    /// Number of invalid responses tolerated before a fetch is aborted.
    pub max_invalid_responses_before_block: usize,
}

impl Default for CatalogueLimitsConfig {
    fn default() -> Self {
        Self {
            max_files_per_catalogue: MAX_CATALOGUE_FILES,
            max_collections: MAX_COLLECTIONS,
            max_entries_per_collection: MAX_ENTRIES_PER_COLLECTION,
            max_page_size: MAX_CATALOGUE_PAGE_SIZE,
            max_total_page_bytes: MAX_CATALOGUE_PAGE_BYTES,
            max_requests_per_window: crate::catalogue_rate_limits::MAX_CATALOGUE_REQUESTS_PER_PEER,
            request_window_seconds: 10,
            max_invalid_responses_before_block: MAX_INVALID_RESPONSE_ATTEMPTS,
        }
    }
}

impl CatalogueLimitsConfig {
    /// Validate all values and their cross-field constraints.
    pub fn validate(&self) -> Result<(), CatalogueLimitsConfigError> {
        let positive = [
            ("max_files_per_catalogue", self.max_files_per_catalogue),
            ("max_collections", self.max_collections),
            (
                "max_entries_per_collection",
                self.max_entries_per_collection,
            ),
            ("max_page_size", self.max_page_size as usize),
            ("max_total_page_bytes", self.max_total_page_bytes),
            (
                "max_requests_per_window",
                self.max_requests_per_window as usize,
            ),
            (
                "request_window_seconds",
                self.request_window_seconds as usize,
            ),
            (
                "max_invalid_responses_before_block",
                self.max_invalid_responses_before_block,
            ),
        ];
        if let Some((field, _)) = positive.into_iter().find(|(_, value)| *value == 0) {
            return Err(CatalogueLimitsConfigError::InvalidValue {
                field,
                reason: "must be greater than zero",
            });
        }
        if self.max_page_size as usize > self.max_files_per_catalogue {
            return Err(CatalogueLimitsConfigError::InvalidValue {
                field: "max_page_size",
                reason: "must not exceed max_files_per_catalogue",
            });
        }
        Ok(())
    }

    /// Parse and validate a JSON configuration string.
    #[cfg(feature = "net")]
    pub fn from_json_str(contents: &str) -> Result<Self, CatalogueLimitsConfigError> {
        let config: Self =
            serde_json::from_str(contents).map_err(CatalogueLimitsConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    /// Load and validate a JSON configuration file.
    #[cfg(feature = "net")]
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, CatalogueLimitsConfigError> {
        let path = path.as_ref();
        let contents =
            fs::read_to_string(path).map_err(|source| CatalogueLimitsConfigError::Io {
                path: path.display().to_string(),
                source,
            })?;
        Self::from_json_str(&contents)
    }
}

/// A clear error returned when catalogue limits cannot be loaded safely.
#[derive(Debug)]
pub enum CatalogueLimitsConfigError {
    /// The file could not be read.
    #[cfg(feature = "net")]
    Io {
        /// Path that could not be read.
        path: String,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The JSON document is malformed.
    #[cfg(feature = "net")]
    Parse(serde_json::Error),
    /// A value is zero or violates a cross-field constraint.
    InvalidValue {
        /// Name of the invalid field.
        field: &'static str,
        /// Reason the value is invalid.
        reason: &'static str,
    },
}

impl std::error::Error for CatalogueLimitsConfigError {}

impl fmt::Display for CatalogueLimitsConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "net")]
            Self::Io { path, source } => {
                write!(f, "cannot read catalogue limits '{path}': {source}")
            }
            #[cfg(feature = "net")]
            Self::Parse(source) => write!(f, "invalid catalogue limits JSON: {source}"),
            Self::InvalidValue { field, reason } => {
                write!(f, "invalid catalogue limit '{field}': {reason}")
            }
        }
    }
}

// ── Validation helpers ────────────────────────────────────────────────────

/// Check whether a catalogue response would exceed the byte-size limit.
///
/// Returns `Err` with the message suitable for the protocol error code
/// when the payload is too large.
pub fn check_response_payload_size(payload_len: usize) -> Result<(), String> {
    if payload_len > MAX_CATALOGUE_RESPONSE_BYTES {
        Err(format!(
            "response payload too large: {payload_len} > {}",
            MAX_CATALOGUE_RESPONSE_BYTES
        ))
    } else {
        Ok(())
    }
}

/// Check whether a file-details response would exceed the byte-size limit.
pub fn check_file_details_payload_size(payload_len: usize) -> Result<(), String> {
    if payload_len > MAX_FILE_DETAILS_PAYLOAD_BYTES {
        Err(format!(
            "file-details payload too large: {payload_len} > {}",
            MAX_FILE_DETAILS_PAYLOAD_BYTES
        ))
    } else {
        Ok(())
    }
}

/// Check whether a paginated response exceeds the per-page byte limit.
pub fn check_page_payload_size(payload_len: usize) -> Result<(), String> {
    if payload_len > MAX_CATALOGUE_PAGE_BYTES {
        Err(format!(
            "page payload too large: {payload_len} > {}",
            MAX_CATALOGUE_PAGE_BYTES
        ))
    } else {
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_payload_within_limit_ok() {
        assert!(check_response_payload_size(1_000).is_ok());
        assert!(check_response_payload_size(MAX_CATALOGUE_RESPONSE_BYTES).is_ok());
    }

    #[test]
    fn test_response_payload_over_limit_rejected() {
        let result = check_response_payload_size(MAX_CATALOGUE_RESPONSE_BYTES + 1);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too large"));
    }

    #[test]
    fn test_file_details_payload_within_limit_ok() {
        assert!(check_file_details_payload_size(1_000).is_ok());
        assert!(check_file_details_payload_size(MAX_FILE_DETAILS_PAYLOAD_BYTES).is_ok());
    }

    #[test]
    fn test_file_details_payload_over_limit_rejected() {
        let result = check_file_details_payload_size(MAX_FILE_DETAILS_PAYLOAD_BYTES + 1);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too large"));
    }

    #[test]
    fn test_check_response_payload_zero_bytes() {
        assert!(check_response_payload_size(0).is_ok());
    }

    #[test]
    fn test_max_catalogue_files_value() {
        assert_eq!(MAX_CATALOGUE_FILES, 10_000);
    }

    #[test]
    fn test_max_collections_value() {
        assert_eq!(MAX_COLLECTIONS, 1_000);
    }

    #[test]
    fn test_max_file_size_bytes_value() {
        assert_eq!(MAX_FILE_SIZE_BYTES, 10 * 1024 * 1024 * 1024 * 1024);
    }

    #[test]
    fn default_configuration_is_valid() {
        assert!(CatalogueLimitsConfig::default().validate().is_ok());
    }

    #[test]
    fn zero_configuration_value_is_rejected() {
        let config = CatalogueLimitsConfig {
            max_page_size: 0,
            ..Default::default()
        };
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("max_page_size"));
        assert!(error.contains("greater than zero"));
    }

    #[test]
    fn page_size_cannot_exceed_catalogue_size() {
        let config = CatalogueLimitsConfig {
            max_files_per_catalogue: 10,
            max_page_size: 11,
            ..Default::default()
        };
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("must not exceed max_files_per_catalogue"));
    }

    #[cfg(feature = "net")]
    #[test]
    fn json_loader_applies_defaults_and_rejects_malformed_input() {
        let source = CatalogueLimitsConfig {
            max_page_size: 100,
            max_files_per_catalogue: 200,
            ..Default::default()
        };
        let contents = serde_json::to_string(&source).unwrap();
        let config = CatalogueLimitsConfig::from_json_str(&contents).unwrap();
        assert_eq!(config.max_page_size, 100);
        assert_eq!(config.max_collections, MAX_COLLECTIONS);

        let error = CatalogueLimitsConfig::from_json_str("{not-json").unwrap_err();
        assert!(error.to_string().contains("invalid catalogue limits JSON"));
    }
}
