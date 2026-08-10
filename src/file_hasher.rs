//! Bounded blocking worker for file-content hashing.
//!
//! File hashing (blake3) is CPU-bound and involves synchronous file I/O.
//! Running it directly on the async runtime would block the worker threads
//! for the duration of the read + hash.  This module provides a bounded
//! [`FileHasher`](crate::file_hasher::FileHasher) that delegates to [`tokio::task::spawn_blocking`] with
//! configurable concurrency limits.
//!
//! # Mtime/Size verification
//!
//! When verifying, the caller supplies the expected mtime and/or size.
//! The hasher captures the current metadata *after* hashing and fails the
//! operation if the file changed during the read.  This is the
//! verify-path-before-caching pattern: read the file, then check if it
//! was still the same file.
//!
//! # Semantics
//!
//! - [`hash_file`](crate::file_hasher::FileHasher::hash_file) returns `Ok(Some(hash))` on success.
//! - Returns `Ok(None)` when the file has changed (mismatched mtime or
//!   size) — the caller should retry or discard.
//! - Returns `Err` on genuine I/O errors (permission denied, not found,
//!   etc.).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use n0_error::Result;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::warn;

/// Bounded blocking hasher for file-content operations.
///
/// Shared via `Arc` across all hashing call sites.  Concurrency is
/// bounded by an internal [`Semaphore`]; calls beyond the limit will
/// wait for a slot (fair queueing via the standard tokio semaphore).
#[derive(Debug, Clone)]
pub struct FileHasher {
    /// Maximum concurrent hashing operations.
    semaphore: Arc<Semaphore>,
}

impl FileHasher {
    /// Create a new [`FileHasher`] limiting concurrent hashing to
    /// `max_concurrent` operations.
    ///
    /// `max_concurrent` is clamped to at least 1.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
        }
    }

    /// Return a cloned [`FileHasher`] sharing the same semaphore.
    ///
    /// All clones share the global concurrency limit.
    pub fn shared(&self) -> Self {
        Self {
            semaphore: Arc::clone(&self.semaphore),
        }
    }

    /// Hash a file on a blocking worker thread, with optional mtime/size
    /// verification.
    ///
    /// # Arguments
    ///
    /// * `path` — Absolute path to the file to hash.
    /// * `expected_size` — If set, verify the file still has this size
    ///   *after* reading.  Returns `Ok(None)` on mismatch.
    /// * `expected_mtime` — If set, verify the modification time still
    ///   matches *after* reading.  Returns `Ok(None)` on mismatch.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(hash))` — hash computed, file unchanged.
    /// * `Ok(None)` — file changed during read (mtime/size mismatch).
    ///   The caller should retry or discard.
    /// * `Err(...)` — genuine I/O error.
    pub async fn hash_file(
        &self,
        path: PathBuf,
        expected_size: Option<u64>,
        expected_mtime: Option<SystemTime>,
    ) -> Result<Option<[u8; 32]>> {
        let _permit: OwnedSemaphorePermit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("FileHasher semaphore closed");

        spawn_blocking_hash(path, expected_size, expected_mtime).await
    }

    /// Same as [`hash_file`](crate::file_hasher::FileHasher::hash_file) but tries once without waiting for a
    /// concurrency slot.  Returns `None` immediately if busy.
    pub async fn try_hash_file(
        &self,
        path: PathBuf,
        expected_size: Option<u64>,
        expected_mtime: Option<SystemTime>,
    ) -> Result<Option<Option<[u8; 32]>>> {
        let permit = self.semaphore.clone().try_acquire_owned();
        match permit {
            Ok(p) => {
                let _permit = p;
                let result = spawn_blocking_hash(path, expected_size, expected_mtime).await?;
                Ok(Some(result))
            }
            Err(_) => Ok(None),
        }
    }
}

/// Internal helper: run the actual hashing on a blocking thread.
async fn spawn_blocking_hash(
    path: PathBuf,
    expected_size: Option<u64>,
    expected_mtime: Option<SystemTime>,
) -> Result<Option<[u8; 32]>> {
    let path_str = path.to_string_lossy().to_string();
    let result = tokio::task::spawn_blocking(move || do_hash(&path, expected_size, expected_mtime))
        .await
        .map_err(|join_err| {
            anyhow::anyhow!("hashing task panicked for {path_str}: {join_err}")
        })??;
    Ok(result)
}

/// Synchronous hash + post-read verification.  Runs on a blocking thread.
fn do_hash(
    path: &std::path::Path,
    expected_size: Option<u64>,
    expected_mtime: Option<SystemTime>,
) -> anyhow::Result<Option<[u8; 32]>> {
    use std::io;

    // ── 1. Pre-hash metadata snapshot for post-read comparison ──────
    let pre_meta = std::fs::metadata(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            anyhow::anyhow!("file vanished before hashing: {}", path.display())
        } else {
            anyhow::anyhow!("failed to stat {} before hashing: {e:#}", path.display())
        }
    })?;

    if let Some(exp_size) = expected_size {
        if pre_meta.len() != exp_size {
            // Size already mismatched — caller may have a stale entry.
            // Don't waste time reading the file.
            return Ok(None);
        }
    }
    if let Some(exp_mtime) = expected_mtime {
        let pre_mtime = pre_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if pre_mtime != exp_mtime {
            return Ok(None);
        }
    }

    // ── 2. Stream the file through a blake3 hasher ──────────────────
    // Streaming avoids loading the entire file into memory.
    let mut file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("failed to open {} for hashing: {e:#}", path.display()))?;

    let mut hasher = blake3::Hasher::new();
    if let Err(e) = io::copy(&mut file, &mut hasher) {
        warn!("I/O error while hashing {}: {e:#}", path.display());
        return Err(anyhow::anyhow!(
            "I/O error hashing {}: {e:#}",
            path.display()
        ));
    }
    let hash = *hasher.finalize().as_bytes();

    // ── 3. Post-read verification — did the file change while we read? ─
    let post_meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            warn!("failed to stat {} after hashing: {e:#}", path.display());
            // Return the hash anyway — metadata check is best-effort.
            return Ok(Some(hash));
        }
    };

    if let Some(exp_mtime) = expected_mtime {
        let post_mtime = post_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if post_mtime != exp_mtime {
            return Ok(None);
        }
    }
    if let Some(exp_size) = expected_size {
        if post_meta.len() != exp_size {
            return Ok(None);
        }
    }

    Ok(Some(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn hash_returns_blake3_for_small_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, b"hello world").unwrap();

        let hasher = FileHasher::new(4);
        let result = hasher.hash_file(path, None, None).await.unwrap();
        assert!(result.is_some());
        let hash = result.unwrap();
        let expected = *blake3::hash(b"hello world").as_bytes();
        assert_eq!(hash, expected);
    }

    #[tokio::test]
    async fn hash_discards_when_size_mismatches() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, b"hello world").unwrap();

        let hasher = FileHasher::new(4);
        let result = hasher.hash_file(path, Some(99), None).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn hash_discards_when_mtime_mismatches() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, b"data").unwrap();

        let wrong_mtime = SystemTime::UNIX_EPOCH;
        let hasher = FileHasher::new(4);
        let result = hasher
            .hash_file(path, None, Some(wrong_mtime))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn try_hash_returns_none_when_busy() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, b"data").unwrap();

        let hasher = FileHasher::new(1);
        // Hold the one slot.
        let _permit = hasher.semaphore.clone().try_acquire_owned().unwrap();

        let result = hasher.try_hash_file(path, None, None).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn hash_reports_file_not_found() {
        let hasher = FileHasher::new(4);
        let path = PathBuf::from("/tmp/nonexistent-file-for-hash-test-12345");
        let result = hasher.hash_file(path, None, None).await;
        assert!(result.is_err());
    }
}
