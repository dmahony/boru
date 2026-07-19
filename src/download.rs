//! Download state and post-transfer verification.
//!
//! A transfer is not complete merely because bytes arrived.  The temporary
//! file must match both the advertised size and the advertised BLAKE3 content
//! hash before it is installed at its destination.

use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::storage::Storage;

/// Durable states used by the download worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// Waiting for a worker slot.
    Queued,
    /// Resolving the remote peer.
    ResolvingPeer,
    /// Requesting a fresh access descriptor.
    RequestingPermission,
    /// Receiving bytes into a temporary file.
    Downloading,
    /// Checking size and content hash.
    Verifying,
    /// Installed and durably recorded as verified.
    Complete,
    /// Paused by the user or during restart recovery.
    Paused,
    /// Failed and eligible for retry.
    Failed,
    /// Cancelled by the user.
    Cancelled,
    /// The catalogue no longer matches the requested content.
    VersionMismatch,
}

impl DownloadState {
    /// Whether this state will not be advanced by the worker.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Complete | Self::Failed | Self::Cancelled | Self::VersionMismatch
        )
    }

    /// Database spelling for this state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::ResolvingPeer => "resolving_peer",
            Self::RequestingPermission => "requesting_permission",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Complete => "complete",
            Self::Paused => "paused",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::VersionMismatch => "version_mismatch",
        }
    }
}

/// Result of validating a downloaded temporary file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDownload {
    /// Number of bytes read from the temporary file.
    pub bytes: u64,
    /// Lower-case hexadecimal BLAKE3 digest.
    pub content_hash: String,
}

/// Validate a temporary download without modifying either file or database.
///
/// Size is checked before hashing so an over-sized or truncated transfer is
/// rejected deterministically.  Hashing streams through a fixed-size buffer;
/// the entire file is never loaded into memory.
pub fn verify_download_file(
    temp_path: impl AsRef<Path>,
    expected_size: u64,
    expected_hash: &str,
) -> Result<VerifiedDownload> {
    let temp_path = temp_path.as_ref();
    let metadata = std::fs::metadata(temp_path)
        .with_context(|| format!("stat downloaded temporary file {}", temp_path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!("download temporary path is not a regular file"));
    }
    if metadata.len() != expected_size {
        return Err(anyhow!(
            "download size mismatch: expected {expected_size} bytes, got {}",
            metadata.len()
        ));
    }

    let mut file = std::fs::File::open(temp_path)
        .with_context(|| format!("open downloaded temporary file {}", temp_path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)
            .with_context(|| format!("hash downloaded temporary file {}", temp_path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes += read as u64;
    }
    let actual_hash = hasher.finalize().to_hex().to_string();
    if actual_hash != expected_hash.to_ascii_lowercase() {
        return Err(anyhow!(
            "download content hash mismatch: expected {expected_hash}, got {actual_hash}"
        ));
    }

    Ok(VerifiedDownload {
        bytes,
        content_hash: actual_hash,
    })
}

/// Verify a temporary file, atomically install it, then mark the download
/// complete.  If the database update fails after the rename, the destination
/// is moved back to the temporary path so an unrecorded file is not left
/// installed.  The temp file must be in the destination directory for the
/// rename to be atomic.
pub fn verify_install_and_complete(
    storage: &Storage,
    download_id: i64,
    temp_path: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    expected_size: u64,
    expected_hash: &str,
) -> Result<VerifiedDownload> {
    let temp_path = temp_path.as_ref();
    let destination = destination.as_ref();
    let verified = verify_download_file(temp_path, expected_size, expected_hash)?;

    if temp_path.parent() != destination.parent() {
        return Err(anyhow!(
            "temporary file and destination must share a directory for atomic rename"
        ));
    }

    std::fs::rename(temp_path, destination).with_context(|| {
        format!(
            "atomically install verified download {} -> {}",
            temp_path.display(),
            destination.display()
        )
    })?;

    if let Err(error) = storage.complete_download(download_id, verified.bytes) {
        // Best-effort rollback keeps the invariant that an installed file has
        // a durable Complete row. Preserve the original database error.
        let rollback = std::fs::rename(destination, temp_path);
        let message = match rollback {
            Ok(()) => "database completion failed; installation rolled back".to_string(),
            Err(rollback_error) => {
                format!("database completion failed and rollback also failed: {rollback_error}")
            }
        };
        return Err(error.context(message).into());
    }

    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fixture(bytes: &[u8]) -> (TempDir, PathBuf, String) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("download.part");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        let hash = blake3::hash(bytes).to_hex().to_string();
        (dir, path, hash)
    }

    #[test]
    fn valid_transfer_is_verified() {
        let (_dir, path, hash) = fixture(b"verified bytes");
        let result = verify_download_file(&path, 14, &hash).unwrap();
        assert_eq!(result.bytes, 14);
        assert_eq!(result.content_hash, hash);
    }

    #[test]
    fn wrong_size_is_rejected_before_install() {
        let (_dir, path, hash) = fixture(b"verified bytes");
        let error = verify_download_file(&path, 13, &hash).unwrap_err();
        assert!(error.to_string().contains("size mismatch"));
        assert!(path.exists());
    }

    #[test]
    fn corrupted_bytes_are_rejected() {
        let (_dir, path, _hash) = fixture(b"verified bytes");
        let error =
            verify_download_file(&path, 14, blake3::hash(b"other").to_hex().as_ref()).unwrap_err();
        assert!(error.to_string().contains("content hash mismatch"));
        assert!(path.exists());
    }

    #[test]
    fn rename_failure_does_not_remove_verified_temp_file() {
        let (_dir, path, hash) = fixture(b"verified bytes");
        let destination = path.parent().unwrap().join("final.bin");
        std::fs::create_dir(&destination).unwrap();
        let error = verify_install_and_complete(
            &Storage::memory().unwrap(),
            1,
            &path,
            &destination,
            14,
            &hash,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("atomically install verified download"));
        assert!(path.exists());
    }

    #[test]
    fn cross_directory_install_is_rejected_before_rename() {
        let (_dir, path, hash) = fixture(b"verified bytes");
        let other = TempDir::new().unwrap();
        let destination = other.path().join("final.bin");
        let error = verify_install_and_complete(
            &Storage::memory().unwrap(),
            1,
            &path,
            &destination,
            14,
            &hash,
        )
        .unwrap_err();
        assert!(error.to_string().contains("share a directory"));
        assert!(path.exists());
    }

    #[test]
    fn database_failure_after_rename_rolls_back_installation() {
        let (_dir, path, hash) = fixture(b"verified bytes");
        let destination = path.parent().unwrap().join("final.bin");
        let storage = Storage::memory().unwrap();

        // No row with id 999 exists, so completion must fail after the file
        // has been renamed. The helper must restore the temp file.
        let error =
            verify_install_and_complete(&storage, 999, &path, &destination, 14, &hash).unwrap_err();
        assert!(error.to_string().contains("completion failed"));
        assert!(path.exists(), "rollback must restore the temporary file");
        assert!(
            !destination.exists(),
            "failed completion must not install output"
        );
    }
}
