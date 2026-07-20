//! Download initiation — validates preconditions before queuing a new
//! durable download from a remote peer's catalogue.
//!
//! # Checks performed
//!
//! 1. **Catalogue verified** — the remote peer's catalogue must have been
//!    fetched and stored locally.
//! 2. **File metadata valid** — the requested file must exist in the peer's
//!    stored catalogue entry with reasonable metadata (non-empty fields).
//! 3. **No conflicting download** — no completed or in-progress download
//!    already exists for the same content hash and remote peer.
//!
//! If all checks pass, [`initiate_download`] creates a new download row via
//! [`Storage::create_download`] and returns the download ID.

use n0_error::StdResultExt;
use tracing::info;

use crate::chat_core::TRANSFER_TELEMETRY;
use crate::storage::Storage;

// ── Error type ──────────────────────────────────────────────────────────────

/// Errors that can prevent a download from being initiated.
#[derive(Debug, Clone)]
pub enum InitiateDownloadError {
    /// The remote peer's catalogue has not been fetched and stored.
    CatalogueNotFetched {
        /// The remote peer's hex-encoded public key.
        peer: String,
    },

    /// The requested file was not found in the remote peer's stored catalogue.
    FileNotFoundInCatalogue {
        /// The content hash that was looked up.
        content_hash: String,
        /// The remote peer.
        peer: String,
    },

    /// The file entry exists in the peer's catalogue but its metadata is
    /// invalid or incomplete.
    FileMetadataInvalid {
        /// The content hash of the file.
        content_hash: String,
        /// Human-readable reason.
        reason: String,
    },

    /// A download for this file from this peer already exists in a state
    /// that conflicts with initiating a new one (completed, in-progress,
    /// or queued).
    DownloadAlreadyExists {
        /// The content hash of the file.
        content_hash: String,
        /// The remote peer.
        peer: String,
        /// Current state of the existing download.
        existing_state: String,
        /// The database id of the existing download.
        existing_download_id: i64,
    },
}

impl std::fmt::Display for InitiateDownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CatalogueNotFetched { peer } => {
                write!(
                    f,
                    "catalogue for peer {peer} has not been fetched and verified"
                )
            }
            Self::FileNotFoundInCatalogue { content_hash, peer } => {
                write!(f, "file {content_hash} not found in {peer}'s catalogue")
            }
            Self::FileMetadataInvalid {
                content_hash,
                reason,
            } => {
                write!(f, "file {content_hash} has invalid metadata: {reason}")
            }
            Self::DownloadAlreadyExists {
                content_hash,
                peer,
                existing_state,
                existing_download_id,
            } => {
                write!(
                    f,
                    "a download for {content_hash} from {peer} already exists \
                     (id={existing_download_id}, state={existing_state})"
                )
            }
        }
    }
}

impl std::error::Error for InitiateDownloadError {}

// ── Result type ─────────────────────────────────────────────────────────────

/// The outcome of a successful download initiation.
#[derive(Debug, Clone)]
pub struct InitiateDownloadResult {
    /// The database id of the newly created download row.
    pub download_id: i64,
    /// The content hash of the file being downloaded.
    pub content_hash: String,
    /// The remote peer the download targets.
    pub remote_peer: String,
    /// Expected total bytes (0 if unknown at initiation time).
    pub total_bytes: u64,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Initiate a new durable download from a remote peer.
///
/// Performs the following precondition checks (in order):
///
/// 1. **Catalogue verified** — the remote peer's catalogue must have been
///    fetched and stored locally.  Checks `profile_manifest_state` for a
///    matching row.
/// 2. **File metadata valid** — the file entry (identified by `content_hash`)
///    must exist in the stored catalogue for this peer, and its fields must
///    be non-empty and reasonable.
/// 3. **No conflicting download** — no terminal (complete, completed, failed,
///    version_mismatch) or active (queued, resolving_peer, requesting_permission,
///    downloading, verifying) download exists for the same content hash and
///    peer.  An existing paused or cancelled download is not considered a
///    conflict (the caller should resume or ignore those).
///
/// If all checks pass, a new download row is created in `queued` state and
/// the result is returned with the new download's id.
///
/// # Errors
///
/// Returns [`InitiateDownloadError`] describing the first failed check.
pub fn initiate_download(
    storage: &Storage,
    content_hash: &str,
    remote_peer: &str,
    known_size: Option<u64>,
) -> std::result::Result<InitiateDownloadResult, InitiateDownloadError> {
    // ── Check 1: Catalogue is verified ─────────────────────────────────
    let peer_pk = remote_peer.parse::<iroh::PublicKey>().map_err(|_e| {
        InitiateDownloadError::CatalogueNotFetched {
            peer: remote_peer.to_string(),
        }
    })?;

    // If get_remote_catalogue_meta returns None, the catalogue hasn't been
    // fetched and stored (the fetch process stores the manifest state).
    let meta = storage
        .get_remote_catalogue_meta(&peer_pk)
        .std_context("look up remote catalogue meta")
        .map_err(|_e| InitiateDownloadError::CatalogueNotFetched {
            peer: remote_peer.to_string(),
        })?;

    if meta.is_none() {
        return Err(InitiateDownloadError::CatalogueNotFetched {
            peer: remote_peer.to_string(),
        });
    }

    // ── Check 2: File exists in catalogue with valid metadata ──────────
    let shared_files = storage
        .get_remote_shared_files(&peer_pk)
        .std_context("look up remote shared files")
        .map_err(|_e| InitiateDownloadError::FileNotFoundInCatalogue {
            content_hash: content_hash.to_string(),
            peer: remote_peer.to_string(),
        })?;

    let file_entry = shared_files
        .iter()
        .find(|f| f.content_hash.eq_ignore_ascii_case(content_hash))
        .ok_or_else(|| InitiateDownloadError::FileNotFoundInCatalogue {
            content_hash: content_hash.to_string(),
            peer: remote_peer.to_string(),
        })?;

    // Validate the file entry's metadata fields.
    if file_entry.display_filename.is_empty() {
        return Err(InitiateDownloadError::FileMetadataInvalid {
            content_hash: content_hash.to_string(),
            reason: "display filename is empty".to_string(),
        });
    }
    if file_entry.mime_type.is_empty() {
        return Err(InitiateDownloadError::FileMetadataInvalid {
            content_hash: content_hash.to_string(),
            reason: "MIME type is empty".to_string(),
        });
    }
    if file_entry.size_bytes == 0 {
        return Err(InitiateDownloadError::FileMetadataInvalid {
            content_hash: content_hash.to_string(),
            reason: "file size is zero".to_string(),
        });
    }

    // ── Check 3: No conflicting download exists ────────────────────────
    // Conflicting means: the download is in a terminal or active state
    // (complete, completed, failed, version_mismatch, queued, resolving_peer,
    // requesting_permission, downloading, verifying).  Paused and cancelled
    // are not conflicts.
    let existing = storage
        .find_downloads_for_file(content_hash, Some(remote_peer))
        .std_context("check existing downloads")
        .map_err(|_e| InitiateDownloadError::DownloadAlreadyExists {
            content_hash: content_hash.to_string(),
            peer: remote_peer.to_string(),
            existing_state: "unknown (query error)".to_string(),
            existing_download_id: 0,
        })?;

    let conflicting_states: &[&str] = &[
        "complete",
        "completed",
        "failed",
        "version_mismatch",
        "queued",
        "resolving_peer",
        "requesting_permission",
        "downloading",
        "verifying",
    ];

    if let Some(conflict) = existing
        .iter()
        .find(|dl| conflicting_states.contains(&dl.state.as_str()))
    {
        return Err(InitiateDownloadError::DownloadAlreadyExists {
            content_hash: content_hash.to_string(),
            peer: remote_peer.to_string(),
            existing_state: conflict.state.clone(),
            existing_download_id: conflict.id,
        });
    }

    // ── All checks passed — create the download ────────────────────────
    let size = known_size.unwrap_or(file_entry.size_bytes);
    let download_id = storage
        .create_download(content_hash, remote_peer, size)
        .expect("create_download should succeed after all prechecks passed");

    info!(
        download_id,
        content_hash, remote_peer, size, "download-initiation: new download created"
    );

    TRANSFER_TELEMETRY.download_queued(download_id, size, None);

    Ok(InitiateDownloadResult {
        download_id,
        content_hash: content_hash.to_string(),
        remote_peer: remote_peer.to_string(),
        total_bytes: size,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;

    /// Helper: set up a storage with a fake peer and file entry so that
    /// the catalogue-verified and file-found checks pass.
    fn seed_peer_catalogue(storage: &Storage, peer_hex: &str, content_hash: &str) {
        // Insert a file_object row (required foreign key for shared_files).
        storage
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO file_objects (content_hash, size, mime_type, filename, created_at_ms)
                     VALUES (?1, 4096, 'application/octet-stream', 'test.bin', ?2)",
                    rusqlite::params![content_hash, 1000000i64],
                )
                .std_context("seed file_object")?;
                Ok(())
            })
            .expect("seed file_object failed");

        // Insert a shared_files row so the file appears in the peer's catalogue.
        storage
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO shared_files (content_hash, profile_user_id, metadata_id,
                            display_filename, description, offered, created_at_ms, updated_at_ms)
                     VALUES (?1, ?2, 'meta-1', 'test.bin', NULL, 1, ?3, ?3)",
                    rusqlite::params![content_hash, peer_hex, 1000000i64],
                )
                .std_context("seed shared_file")?;
                Ok(())
            })
            .expect("seed shared_file failed");

        // Insert a profile_manifest_state row so catalogue-meta check passes.
        storage
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO profile_manifest_state (user_id, revision, manifest_hash, created_at_ms)
                     VALUES (?1, 1, 'abc', ?2)",
                    rusqlite::params![peer_hex, 1000000i64],
                )
                .std_context("seed manifest state")?;
                Ok(())
            })
            .expect("seed manifest state failed");
    }

    fn test_peer() -> String {
        iroh::SecretKey::generate().public().to_string()
    }

    fn test_content_hash() -> String {
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".into()
    }

    // ── Happy path ─────────────────────────────────────────────────────

    #[test]
    fn happy_path_creates_download() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        let result = initiate_download(&storage, &hash, &peer, None).unwrap();
        assert!(result.download_id > 0);
        assert_eq!(result.content_hash, hash);
        assert_eq!(result.remote_peer, peer);
        assert_eq!(result.total_bytes, 4096);

        // Verify the download row is in 'queued' state.
        let dl = storage.get_download(result.download_id).unwrap().unwrap();
        assert_eq!(dl.state, "queued");
    }

    #[test]
    fn happy_path_with_explicit_size() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        // known_size overrides the catalogue's size_bytes.
        let result = initiate_download(&storage, &hash, &peer, Some(999_999)).unwrap();
        assert!(result.download_id > 0);
        assert_eq!(result.total_bytes, 999_999);
    }

    // ── Check 1: catalogue not fetched ─────────────────────────────────

    #[test]
    fn error_when_catalogue_not_fetched() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();

        // No catalogue data seeded — the meta check must fail.
        let err = initiate_download(&storage, &hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::CatalogueNotFetched { peer: p } if p == &peer),
            "expected CatalogueNotFetched, got {err}"
        );
    }

    #[test]
    fn error_when_peer_key_is_invalid() {
        let storage = Storage::memory().unwrap();
        let err = initiate_download(&storage, "hash", "not-a-valid-public-key", None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::CatalogueNotFetched { .. }),
            "expected CatalogueNotFetched, got {err}"
        );
    }

    // ── Check 2: file not found in catalogue ───────────────────────────

    #[test]
    fn error_when_file_not_in_catalogue() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        let other_hash = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        seed_peer_catalogue(&storage, &peer, &hash);

        // Request a different content_hash that isn't in the peer's catalogue.
        let err = initiate_download(&storage, other_hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::FileNotFoundInCatalogue { content_hash: c, .. } if c == other_hash),
            "expected FileNotFoundInCatalogue, got {err}"
        );
    }

    // ── Check 2: file metadata validation ──────────────────────────────

    #[test]
    fn error_when_file_metadata_has_empty_mime_type() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        // Corrupt the mime_type to empty.
        storage
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE file_objects SET mime_type = '' WHERE content_hash = ?1",
                    rusqlite::params![hash],
                )
                .std_context("corrupt mime_type")?;
                Ok(())
            })
            .expect("corrupt mime_type failed");

        let err = initiate_download(&storage, &hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::FileMetadataInvalid { content_hash: c, reason: r }
                if c == &hash && r.contains("MIME type")),
            "expected FileMetadataInvalid about MIME type, got {err}"
        );
    }

    #[test]
    fn error_when_file_metadata_has_empty_display_filename() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        // Corrupt the display_filename to empty.
        storage
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE shared_files SET display_filename = '' WHERE content_hash = ?1 AND profile_user_id = ?2",
                    rusqlite::params![hash, peer],
                )
                .std_context("corrupt display_filename")?;
                Ok(())
            })
            .expect("corrupt display_filename failed");

        let err = initiate_download(&storage, &hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::FileMetadataInvalid { content_hash: c, reason: r }
                if c == &hash && r.contains("filename")),
            "expected FileMetadataInvalid about filename, got {err}"
        );
    }

    #[test]
    fn error_when_file_size_is_zero() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        // Corrupt the size to 0.
        storage
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE file_objects SET size = 0 WHERE content_hash = ?1",
                    rusqlite::params![hash],
                )
                .std_context("corrupt size")?;
                Ok(())
            })
            .expect("corrupt size failed");

        let err = initiate_download(&storage, &hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::FileMetadataInvalid { content_hash: c, reason: r }
                if c == &hash && r.contains("size")),
            "expected FileMetadataInvalid about size, got {err}"
        );
    }

    // ── Check 3: conflicting download exists ───────────────────────────

    #[test]
    fn error_when_completed_download_exists() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        // Create a completed download for the same file+peer.
        let dl_id = storage.create_download(&hash, &peer, 4096).unwrap();
        storage.complete_download(dl_id, 4096).unwrap();

        let err = initiate_download(&storage, &hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::DownloadAlreadyExists {
                existing_state, existing_download_id, ..
            } if existing_state == "complete" && *existing_download_id == dl_id),
            "expected DownloadAlreadyExists, got {err}"
        );
    }

    #[test]
    fn error_when_failed_download_exists() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        let dl_id = storage.create_download(&hash, &peer, 4096).unwrap();
        storage.fail_download(dl_id, "test error", None).unwrap();

        let err = initiate_download(&storage, &hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::DownloadAlreadyExists { existing_state, .. }
                if existing_state == "failed"),
            "expected DownloadAlreadyExists for failed state, got {err}"
        );
    }

    #[test]
    fn error_when_queued_download_exists() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        // A queued download exists (the default state).
        let dl_id = storage.create_download(&hash, &peer, 4096).unwrap();

        let err = initiate_download(&storage, &hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::DownloadAlreadyExists { existing_state, .. }
                if existing_state == "queued"),
            "expected DownloadAlreadyExists for queued, got {err}"
        );
        // Verify the first download remains.
        let dl = storage.get_download(dl_id).unwrap().unwrap();
        assert_eq!(dl.state, "queued");
    }

    #[test]
    fn error_when_downloading_exists() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        let dl_id = storage.create_download(&hash, &peer, 4096).unwrap();
        storage
            .update_download_progress(dl_id, 2048, "downloading")
            .unwrap();

        let err = initiate_download(&storage, &hash, &peer, None).unwrap_err();
        assert!(
            matches!(&err, InitiateDownloadError::DownloadAlreadyExists { existing_state, .. }
                if existing_state == "downloading"),
            "expected DownloadAlreadyExists for downloading, got {err}"
        );
    }

    // ── Non-conflicting states: paused, cancelled ──────────────────────

    #[test]
    fn paused_download_does_not_block_new_initiation() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        // Create a paused download for the same file+peer.
        let _dl_id = storage.create_download(&hash, &peer, 4096).unwrap();
        storage.pause_download(1).unwrap();

        // A new download should be allowed.
        let result = initiate_download(&storage, &hash, &peer, None).unwrap();
        assert!(result.download_id > 0);
        // The new download should have a different id.
        assert_ne!(result.download_id, 1);
        let dl = storage.get_download(result.download_id).unwrap().unwrap();
        assert_eq!(dl.state, "queued");
    }

    #[test]
    fn cancelled_download_does_not_block_new_initiation() {
        let storage = Storage::memory().unwrap();
        let peer = test_peer();
        let hash = test_content_hash();
        seed_peer_catalogue(&storage, &peer, &hash);

        // Create a cancelled download for the same file+peer.
        let _dl_id = storage.create_download(&hash, &peer, 4096).unwrap();
        storage.cancel_download(1).unwrap();

        // A new download should be allowed.
        let result = initiate_download(&storage, &hash, &peer, None).unwrap();
        assert!(result.download_id > 0);
        let dl = storage.get_download(result.download_id).unwrap().unwrap();
        assert_eq!(dl.state, "queued");
    }

    // ── Error display ─────────────────────────────────────────────────

    #[test]
    fn error_display_messages_are_descriptive() {
        let err1 = InitiateDownloadError::CatalogueNotFetched {
            peer: "peer123".into(),
        };
        let msg1 = err1.to_string();
        assert!(msg1.contains("catalogue"), "msg: {msg1}");
        assert!(msg1.contains("peer123"), "msg: {msg1}");

        let err2 = InitiateDownloadError::FileNotFoundInCatalogue {
            content_hash: "hash1".into(),
            peer: "peer123".into(),
        };
        let msg2 = err2.to_string();
        assert!(msg2.contains("hash1"), "msg: {msg2}");
        assert!(msg2.contains("peer123"), "msg: {msg2}");

        let err3 = InitiateDownloadError::FileMetadataInvalid {
            content_hash: "hash1".into(),
            reason: "bad size".into(),
        };
        let msg3 = err3.to_string();
        assert!(msg3.contains("hash1"));
        assert!(msg3.contains("bad size"));

        let err4 = InitiateDownloadError::DownloadAlreadyExists {
            content_hash: "hash1".into(),
            peer: "peer123".into(),
            existing_state: "complete".into(),
            existing_download_id: 42,
        };
        let msg4 = err4.to_string();
        assert!(msg4.contains("hash1"));
        assert!(msg4.contains("peer123"));
        assert!(msg4.contains("42"));
        assert!(msg4.contains("complete"));
    }
}
