//! Temporary database / storage setup (BORU-TEST-010).

use std::path::PathBuf;

use tempfile::TempDir;

/// Create a temporary directory for a test run.
///
/// The `TempDir` is deleted on `Drop`, so tests that need a storage path for
/// the lifetime of a peer should keep the handle alive alongside the peer.
pub fn temp_dir(prefix: &str) -> TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("create temp dir for test")
}

/// Create a temporary directory and return its path immediately.
///
/// Prefer [`temp_dir`] when the borrow checker allows keeping the `TempDir`
/// alive; use this only when a `&Path` must outlive the block that creates it.
pub fn temp_path(prefix: &str) -> PathBuf {
    temp_dir(prefix).keep()
}
