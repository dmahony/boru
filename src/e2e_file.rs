//! Deterministic, sandbox-only file actions for the E2E control plane.
//!
//! This adapter owns synthetic fixture bytes and drives the same bounded file
//! metadata/hash checks used by the test pathway. It never accepts arbitrary
//! filesystem paths: every source and destination must canonicalize beneath
//! the run-owned sandbox.

#![allow(missing_docs)]

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::e2e_control::{TransferMarker, TransferSnapshot, TransferState};

/// Deterministic fixture size used by the small-file E2E lane.
pub const SMALL_FIXTURE_SIZE: u64 = 4 * 1024;
/// Deterministic fixture size used by the bounded progress E2E lane.
pub const MEDIUM_FIXTURE_SIZE: u64 = 2 * 1024 * 1024;
const MAX_FIXTURE_SIZE: u64 = 16 * 1024 * 1024;
const COPY_CHUNK_SIZE: usize = 64 * 1024;

/// Structured failures returned by file actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileActionError {
    InvalidPath {
        message: String,
    },
    OutsideSandbox {
        path: String,
    },
    NotFound {
        marker: String,
    },
    AlreadyExists {
        marker: String,
    },
    Declined {
        marker: String,
    },
    PermissionDenied {
        marker: String,
    },
    InvalidState {
        marker: String,
        state: TransferState,
    },
    SizeMismatch {
        expected: u64,
        actual: u64,
    },
    HashMismatch,
    Io {
        operation: &'static str,
        message: String,
    },
}

impl fmt::Display for FileActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath { message } => write!(f, "invalid_path: {message}"),
            Self::OutsideSandbox { path } => write!(f, "outside_sandbox: {path}"),
            Self::NotFound { marker } => write!(f, "not_found: {marker}"),
            Self::AlreadyExists { marker } => write!(f, "already_exists: {marker}"),
            Self::Declined { marker } => write!(f, "declined: {marker}"),
            Self::PermissionDenied { marker } => write!(f, "permission_denied: {marker}"),
            Self::InvalidState { marker, state } => {
                write!(f, "invalid_state: {marker} ({state:?})")
            }
            Self::SizeMismatch { expected, actual } => {
                write!(f, "size_mismatch: expected {expected}, got {actual}")
            }
            Self::HashMismatch => f.write_str("hash_mismatch"),
            Self::Io { operation, message } => write!(f, "io_{operation}: {message}"),
        }
    }
}

impl std::error::Error for FileActionError {}

/// Metadata and path for one generated fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticFixture {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub seed: u64,
    pub sha256: String,
    pub blake3: String,
}

/// A deterministic fixture directory and bounded file-action registry.
#[derive(Debug)]
pub struct SyntheticFileAdapter {
    sandbox: PathBuf,
    offers: HashMap<TransferMarker, Offer>,
}

#[derive(Debug, Clone)]
struct Offer {
    fixture: SyntheticFixture,
    state: TransferState,
    bytes_transferred: u64,
}

impl SyntheticFileAdapter {
    /// Create an adapter rooted at an existing or newly-created sandbox.
    pub fn new(sandbox: impl Into<PathBuf>) -> Result<Self, FileActionError> {
        let sandbox = sandbox.into();
        fs::create_dir_all(&sandbox).map_err(|e| io_error("create_sandbox", e))?;
        let sandbox =
            fs::canonicalize(&sandbox).map_err(|e| io_error("canonicalize_sandbox", e))?;
        Ok(Self {
            sandbox,
            offers: HashMap::new(),
        })
    }

    /// Return the canonical run-owned sandbox path.
    pub fn sandbox(&self) -> &Path {
        &self.sandbox
    }

    /// Generate a deterministic fixture, replacing no existing files.
    pub fn create_fixture(
        &self,
        marker: &TransferMarker,
        size_bytes: u64,
        seed: u64,
    ) -> Result<SyntheticFixture, FileActionError> {
        if size_bytes == 0 || size_bytes > MAX_FIXTURE_SIZE {
            return Err(FileActionError::InvalidPath {
                message: format!("fixture size {size_bytes} is outside bounded range"),
            });
        }
        let path = self
            .sandbox
            .join(format!("fixture-{}.bin", safe_marker(marker)?));
        if path.exists() {
            return Err(FileActionError::AlreadyExists {
                marker: marker.as_ref().to_owned(),
            });
        }
        let mut file = File::create(&path).map_err(|e| io_error("create_fixture", e))?;
        let mut sha = Sha256::new();
        let mut blake = blake3::Hasher::new();
        let mut remaining = size_bytes;
        let mut offset = 0u64;
        let mut buffer = vec![0u8; COPY_CHUNK_SIZE];
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            fill_pattern(&mut buffer[..count], seed, offset);
            file.write_all(&buffer[..count])
                .map_err(|e| io_error("write_fixture", e))?;
            sha.update(&buffer[..count]);
            blake.update(&buffer[..count]);
            offset += count as u64;
            remaining -= count as u64;
        }
        file.sync_all().map_err(|e| io_error("sync_fixture", e))?;
        Ok(SyntheticFixture {
            path: fs::canonicalize(&path).map_err(|e| io_error("canonicalize_fixture", e))?,
            size_bytes,
            seed,
            sha256: hex::encode(sha.finalize()),
            blake3: blake.finalize().to_hex().to_string(),
        })
    }

    /// Offer a fixture through the normal bounded file action path.
    pub fn share(
        &mut self,
        marker: TransferMarker,
        fixture: SyntheticFixture,
    ) -> Result<TransferSnapshot, FileActionError> {
        self.validate_path(&fixture.path)?;
        if self.offers.contains_key(&marker) {
            return Err(FileActionError::AlreadyExists {
                marker: marker.as_ref().to_owned(),
            });
        }
        let actual = fs::metadata(&fixture.path)
            .map_err(|e| io_error("stat_offer", e))?
            .len();
        if actual != fixture.size_bytes {
            return Err(FileActionError::SizeMismatch {
                expected: fixture.size_bytes,
                actual,
            });
        }
        let offer = Offer {
            fixture,
            state: TransferState::Offered,
            bytes_transferred: 0,
        };
        self.offers.insert(marker.clone(), offer);
        Ok(self.snapshot(&marker).expect("inserted offer"))
    }

    /// Accept or explicitly decline a pending offer.
    pub fn accept(
        &mut self,
        marker: &TransferMarker,
        accept: bool,
    ) -> Result<TransferSnapshot, FileActionError> {
        let offer = self
            .offers
            .get_mut(marker)
            .ok_or_else(|| not_found(marker))?;
        if offer.state != TransferState::Offered {
            return Err(FileActionError::InvalidState {
                marker: marker.as_ref().to_owned(),
                state: offer.state,
            });
        }
        if !accept {
            offer.state = TransferState::Declined;
            return Ok(self.snapshot(marker).expect("offer remains registered"));
        }
        offer.state = TransferState::Accepted;
        Ok(self.snapshot(marker).expect("offer remains registered"))
    }

    /// Download an accepted offer into the sandbox and verify both hashes.
    pub fn download(
        &mut self,
        marker: &TransferMarker,
        destination: &Path,
    ) -> Result<TransferSnapshot, FileActionError> {
        self.download_with_progress(marker, destination, |_, _| {})
    }

    /// Copy until measurable progress has been made, then leave a resumable
    /// partial in place to model a sender/receiver restart.
    pub fn interrupt_after(
        &mut self,
        marker: &TransferMarker,
        destination: &Path,
        interrupt_after: u64,
    ) -> Result<TransferSnapshot, FileActionError> {
        let offer = self.offers.get_mut(marker).ok_or_else(|| not_found(marker))?;
        if offer.state != TransferState::Accepted || interrupt_after == 0 || interrupt_after >= offer.fixture.size_bytes {
            return Err(FileActionError::InvalidState { marker: marker.as_ref().to_owned(), state: offer.state });
        }
        let destination = canonical_child(&self.sandbox, destination)?;
        if destination.exists() {
            return Err(FileActionError::PermissionDenied { marker: marker.as_ref().to_owned() });
        }
        offer.state = TransferState::Active;
        let mut source = File::open(&offer.fixture.path).map_err(|e| io_error("open_source", e))?;
        let mut output = File::create(&destination).map_err(|e| io_error("create_destination", e))?;
        let mut buffer = vec![0u8; COPY_CHUNK_SIZE];
        while offer.bytes_transferred < interrupt_after {
            let read_len = (interrupt_after - offer.bytes_transferred).min(buffer.len() as u64) as usize;
            let count = source.read(&mut buffer[..read_len])
                .map_err(|e| io_error("read_source", e))?;
            if count == 0 { break; }
            output.write_all(&buffer[..count]).map_err(|e| io_error("write_destination", e))?;
            offer.bytes_transferred += count as u64;
        }
        output.sync_all().map_err(|e| io_error("sync_destination", e))?;
        Ok(self.snapshot(marker).expect("offer remains registered"))
    }

    /// Verify a retained prefix and append the remaining source bytes.
    pub fn resume_download<F: FnMut(u64, u64)>(
        &mut self,
        marker: &TransferMarker,
        destination: &Path,
        mut progress: F,
    ) -> Result<TransferSnapshot, FileActionError> {
        let offer = self.offers.get_mut(marker).ok_or_else(|| not_found(marker))?;
        if offer.state != TransferState::Active {
            return Err(FileActionError::InvalidState { marker: marker.as_ref().to_owned(), state: offer.state });
        }
        let destination = canonical_child(&self.sandbox, destination)?;
        let partial_size = fs::metadata(&destination).map_err(|e| io_error("stat_partial", e))?.len();
        if partial_size == 0 || partial_size >= offer.fixture.size_bytes || partial_size != offer.bytes_transferred {
            offer.state = TransferState::Failed;
            let _ = fs::remove_file(&destination);
            return Err(FileActionError::HashMismatch);
        }
        let mut source = File::open(&offer.fixture.path).map_err(|e| io_error("open_source", e))?;
        let mut partial = File::open(&destination).map_err(|e| io_error("open_partial", e))?;
        let mut source_buf = vec![0u8; COPY_CHUNK_SIZE];
        let mut partial_buf = vec![0u8; COPY_CHUNK_SIZE];
        let mut remaining = partial_size;
        while remaining > 0 {
            let count = remaining.min(COPY_CHUNK_SIZE as u64) as usize;
            source.read_exact(&mut source_buf[..count]).map_err(|e| io_error("read_source", e))?;
            partial.read_exact(&mut partial_buf[..count]).map_err(|e| io_error("read_partial", e))?;
            if source_buf[..count] != partial_buf[..count] {
                offer.state = TransferState::Failed;
                let _ = fs::remove_file(&destination);
                return Err(FileActionError::HashMismatch);
            }
            remaining -= count as u64;
        }
        drop(partial);
        let mut output = fs::OpenOptions::new().append(true).open(&destination).map_err(|e| io_error("open_destination", e))?;
        let mut total = partial_size;
        let mut buffer = vec![0u8; COPY_CHUNK_SIZE];
        while total < offer.fixture.size_bytes {
            let count = source.read(&mut buffer).map_err(|e| io_error("read_source", e))?;
            if count == 0 { break; }
            output.write_all(&buffer[..count]).map_err(|e| io_error("write_destination", e))?;
            total += count as u64;
            offer.bytes_transferred = total;
            progress(total, offer.fixture.size_bytes);
        }
        output.sync_all().map_err(|e| io_error("sync_destination", e))?;
        if let Err(error) = verify_file(&destination, &offer.fixture) {
            offer.state = TransferState::Failed;
            let _ = fs::remove_file(&destination);
            return Err(error);
        }
        offer.state = TransferState::Completed;
        Ok(self.snapshot(marker).expect("offer remains registered"))
    }

    /// Download while reporting bounded `(bytes_transferred, total_bytes)` progress.
    pub fn download_with_progress<F: FnMut(u64, u64)>(
        &mut self,
        marker: &TransferMarker,
        destination: &Path,
        mut progress: F,
    ) -> Result<TransferSnapshot, FileActionError> {
        let offer = self
            .offers
            .get_mut(marker)
            .ok_or_else(|| not_found(marker))?;
        if offer.state == TransferState::Declined {
            return Err(FileActionError::Declined {
                marker: marker.as_ref().to_owned(),
            });
        }
        if offer.state != TransferState::Accepted {
            return Err(FileActionError::InvalidState {
                marker: marker.as_ref().to_owned(),
                state: offer.state,
            });
        }
        let destination = canonical_child(&self.sandbox, destination)?;
        if destination.exists() {
            return Err(FileActionError::PermissionDenied {
                marker: marker.as_ref().to_owned(),
            });
        }
        offer.state = TransferState::Active;
        let mut source = File::open(&offer.fixture.path).map_err(|e| io_error("open_source", e))?;
        let mut output =
            File::create(&destination).map_err(|e| io_error("create_destination", e))?;
        let mut sha = Sha256::new();
        let mut blake = blake3::Hasher::new();
        let mut buffer = vec![0u8; COPY_CHUNK_SIZE];
        loop {
            let count = source
                .read(&mut buffer)
                .map_err(|e| io_error("read_source", e))?;
            if count == 0 {
                break;
            }
            output
                .write_all(&buffer[..count])
                .map_err(|e| io_error("write_destination", e))?;
            sha.update(&buffer[..count]);
            blake.update(&buffer[..count]);
            offer.bytes_transferred += count as u64;
            progress(offer.bytes_transferred, offer.fixture.size_bytes);
        }
        output
            .sync_all()
            .map_err(|e| io_error("sync_destination", e))?;
        let actual_size = fs::metadata(&destination)
            .map_err(|e| io_error("stat_destination", e))?
            .len();
        if actual_size != offer.fixture.size_bytes {
            offer.state = TransferState::Failed;
            return Err(FileActionError::SizeMismatch {
                expected: offer.fixture.size_bytes,
                actual: actual_size,
            });
        }
        if hex::encode(sha.finalize()) != offer.fixture.sha256
            || blake.finalize().to_hex().as_str() != offer.fixture.blake3
        {
            offer.state = TransferState::Failed;
            return Err(FileActionError::HashMismatch);
        }
        offer.state = TransferState::Completed;
        Ok(self.snapshot(marker).expect("offer remains registered"))
    }

    /// Query the compact control-plane transfer state.
    pub fn query(&self, marker: &TransferMarker) -> Result<TransferSnapshot, FileActionError> {
        self.snapshot(marker).ok_or_else(|| not_found(marker))
    }

    fn snapshot(&self, marker: &TransferMarker) -> Option<TransferSnapshot> {
        self.offers.get(marker).map(|offer| TransferSnapshot {
            schema: Default::default(),
            transfer_marker: marker.clone(),
            state: offer.state,
            size_bytes: offer.fixture.size_bytes,
            bytes_transferred: offer.bytes_transferred,
            content_hash: Some(offer.fixture.blake3.clone()),
        })
    }

    fn validate_path(&self, path: &Path) -> Result<(), FileActionError> {
        canonical_child(&self.sandbox, path).map(|_| ())
    }
}

fn verify_file(path: &Path, fixture: &SyntheticFixture) -> Result<(), FileActionError> {
    let mut file = File::open(path).map_err(|e| io_error("open_destination", e))?;
    let mut sha = Sha256::new();
    let mut blake = blake3::Hasher::new();
    let mut buffer = vec![0u8; COPY_CHUNK_SIZE];
    let mut size = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(|e| io_error("read_destination", e))?;
        if count == 0 { break; }
        size += count as u64;
        sha.update(&buffer[..count]);
        blake.update(&buffer[..count]);
    }
    if size != fixture.size_bytes {
        return Err(FileActionError::SizeMismatch { expected: fixture.size_bytes, actual: size });
    }
    if hex::encode(sha.finalize()) != fixture.sha256
        || blake.finalize().to_hex().as_str() != fixture.blake3
    {
        return Err(FileActionError::HashMismatch);
    }
    Ok(())
}

fn safe_marker(marker: &TransferMarker) -> Result<String, FileActionError> {
    let value = marker.as_ref();
    if value.is_empty()
        || value.len() > 96
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(FileActionError::InvalidPath {
            message: "transfer marker must be a bounded filename-safe token".into(),
        });
    }
    Ok(value.to_owned())
}

fn canonical_child(root: &Path, path: &Path) -> Result<PathBuf, FileActionError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical = if candidate.exists() {
        fs::canonicalize(&candidate)
    } else {
        let parent = candidate
            .parent()
            .ok_or_else(|| FileActionError::InvalidPath {
                message: "missing destination parent".into(),
            })?;
        fs::canonicalize(parent).map(|p| p.join(candidate.file_name().unwrap_or_default()))
    }
    .map_err(|e| io_error("canonicalize_path", e))?;
    if canonical == root || !canonical.starts_with(root) {
        return Err(FileActionError::OutsideSandbox {
            path: path.display().to_string(),
        });
    }
    Ok(canonical)
}

fn fill_pattern(buffer: &mut [u8], seed: u64, offset: u64) {
    for (index, byte) in buffer.iter_mut().enumerate() {
        let x = seed
            .wrapping_add(offset + index as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15);
        *byte = (x ^ (x >> 23) ^ (x >> 41)) as u8;
    }
}

fn not_found(marker: &TransferMarker) -> FileActionError {
    FileActionError::NotFound {
        marker: marker.as_ref().to_owned(),
    }
}
fn io_error(operation: &'static str, error: io::Error) -> FileActionError {
    FileActionError::Io {
        operation,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn small_fixture_round_trips_with_exact_metadata() {
        let dir = tempdir().unwrap();
        let mut adapter = SyntheticFileAdapter::new(dir.path()).unwrap();
        let marker: TransferMarker = "small-1".into();
        let fixture = adapter
            .create_fixture(&marker, SMALL_FIXTURE_SIZE, 7)
            .unwrap();
        let expected = fixture.clone();
        let offered = adapter.share(marker.clone(), fixture).unwrap();
        assert_eq!(offered.state, TransferState::Offered);
        adapter.accept(&marker, true).unwrap();
        let destination = dir.path().join("received.bin");
        let complete = adapter.download(&marker, &destination).unwrap();
        assert_eq!(complete.state, TransferState::Completed);
        assert_eq!(complete.size_bytes, expected.size_bytes);
        assert_eq!(
            complete.content_hash.as_deref(),
            Some(expected.blake3.as_str())
        );
        assert_eq!(
            fs::metadata(destination).unwrap().len(),
            expected.size_bytes
        );
    }

    #[test]
    fn medium_fixture_reports_progress_and_hashes() {
        let dir = tempdir().unwrap();
        let mut adapter = SyntheticFileAdapter::new(dir.path()).unwrap();
        let marker: TransferMarker = "medium-1".into();
        let fixture = adapter
            .create_fixture(&marker, MEDIUM_FIXTURE_SIZE, 99)
            .unwrap();
        adapter.share(marker.clone(), fixture).unwrap();
        adapter.accept(&marker, true).unwrap();
        let mut progress = Vec::new();
        adapter
            .download_with_progress(&marker, &dir.path().join("medium.bin"), |done, total| {
                progress.push((done, total))
            })
            .unwrap();
        assert!(progress.len() > 1);
        assert_eq!(progress.last().unwrap().0, MEDIUM_FIXTURE_SIZE);
    }

    #[test]
    fn interrupted_transfer_resumes_without_duplicate_destination() {
        let dir = tempdir().unwrap();
        let mut adapter = SyntheticFileAdapter::new(dir.path()).unwrap();
        let marker: TransferMarker = "resume-1".into();
        let fixture = adapter.create_fixture(&marker, MEDIUM_FIXTURE_SIZE, 123).unwrap();
        adapter.share(marker.clone(), fixture.clone()).unwrap();
        adapter.accept(&marker, true).unwrap();
        let destination = dir.path().join("resumed.bin");
        let interrupted = adapter.interrupt_after(&marker, &destination, 128 * 1024).unwrap();
        assert_eq!(interrupted.state, TransferState::Active);
        assert!(interrupted.bytes_transferred > 0);
        let completed = adapter.resume_download(&marker, &destination, |_, _| {}).unwrap();
        assert_eq!(completed.state, TransferState::Completed);
        assert_eq!(fs::metadata(&destination).unwrap().len(), fixture.size_bytes);
        assert_eq!(completed.content_hash.as_deref(), Some(fixture.blake3.as_str()));
    }

    #[test]
    fn corrupted_partial_fails_and_cleans_run_owned_artifact() {
        let dir = tempdir().unwrap();
        let mut adapter = SyntheticFileAdapter::new(dir.path()).unwrap();
        let marker: TransferMarker = "resume-2".into();
        let fixture = adapter.create_fixture(&marker, 128 * 1024, 321).unwrap();
        adapter.share(marker.clone(), fixture).unwrap();
        adapter.accept(&marker, true).unwrap();
        let destination = dir.path().join("corrupt.bin");
        adapter.interrupt_after(&marker, &destination, 64 * 1024).unwrap();
        let mut bytes = fs::read(&destination).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&destination, bytes).unwrap();
        assert!(matches!(
            adapter.resume_download(&marker, &destination, |_, _| {}),
            Err(FileActionError::HashMismatch)
        ));
        assert!(!destination.exists());
    }

    #[test]
    fn rejects_outside_paths_and_declined_downloads() {
        let dir = tempdir().unwrap();
        let mut adapter = SyntheticFileAdapter::new(dir.path()).unwrap();
        let marker: TransferMarker = "decline-1".into();
        let fixture = adapter.create_fixture(&marker, 64, 1).unwrap();
        assert!(matches!(
            adapter.share(
                marker.clone(),
                SyntheticFixture {
                    path: dir.path().parent().unwrap().to_path_buf(),
                    ..fixture.clone()
                }
            ),
            Err(FileActionError::OutsideSandbox { .. })
        ));
        adapter.share(marker.clone(), fixture).unwrap();
        adapter.accept(&marker, true).unwrap();
        assert!(matches!(
            adapter.download(&marker, &dir.path().parent().unwrap().join("outside.bin"),),
            Err(FileActionError::OutsideSandbox { .. })
        ));
        // A receiver can explicitly decline before any bytes are copied.
        let declined_marker: TransferMarker = "decline-2".into();
        let declined_fixture = adapter.create_fixture(&declined_marker, 64, 2).unwrap();
        adapter
            .share(declined_marker.clone(), declined_fixture)
            .unwrap();
        adapter.accept(&declined_marker, false).unwrap();
        assert!(matches!(
            adapter.download(&declined_marker, &dir.path().join("nope")),
            Err(FileActionError::Declined { .. })
        ));
        assert!(matches!(
            adapter.query(&"missing".into()),
            Err(FileActionError::NotFound { .. })
        ));
    }
}
