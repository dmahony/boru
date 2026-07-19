//! Central size and count limits for catalogue protocol traffic.
//!
//! All hard limits enforced by the server (handler) and client are defined
//! here.  Values are chosen to prevent resource exhaustion while supporting
//! legitimate usage patterns.  These are the final, documented hard limits
//! for catalogue traffic.
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
//! | Max description length | 1,024 bytes | [`RemoteSharedFile::validate`], [`RemoteCollection::validate`] |
//! | Max mime_type length | 128 bytes | [`RemoteSharedFile::validate`] |
//! | Max content_hash length | 128 bytes | [`RemoteSharedFile::validate`] |
//! | Max shared_file_id length | 256 bytes | [`RemoteSharedFile::validate`] |
//! | Max collections per file | 256 | [`RemoteSharedFile::validate`] |
//! | Max collection_id length | 256 bytes | [`RemoteCollection::validate`] |
//! | Max collection name length | 512 bytes | [`RemoteCollection::validate`] |
//! | Max individual file size | 10 TiB | Handler (build) — checked in catalogue construction |
//! | Max file-details response bytes | 256 KiB | Handler (send) + Client (receive) |

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
}
