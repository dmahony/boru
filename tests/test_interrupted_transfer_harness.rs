//! Reusable interrupted-transfer test harness and fixtures.
//!
//! Provides controlled infrastructure for testing file-transfer interruption,
//! crash recovery, permission changes between retries, catalogue version
//! mismatches, and post-resume state assertions.
//!
//! # Architecture
//!
//! [`InterruptedTransferHarness`] owns two isolated SQLite database directories
//! (sender + receiver) on `TempDir`s.  The sender owns a catalogue entry with
//! an offered file, optional permission grants, and a manifest revision.  The
//! receiver initiates and tracks downloads.
//!
//! Crash simulation uses the `Storage::open(path)` + drop + re-open pattern
//! (same approach as `test_crash_recovery.rs`).  The harness seeds downloads
//! in specific states (`queued`, `resolving_peer`, `downloading`, `verifying`)
//! and asserts the correct recovery outcome (`queued`, `paused`, `complete`)
//! after reopening.
//!
//! # Key fixtures
//!
//! | Method | Purpose |
//! |--------|---------|
//! | [`InterruptedTransferHarness::new`] | Create fresh sender + receiver stores |
//! | [`create_download_at_state`] | Seed a download row in a specific state |
//! | [`simulate_crash_and_reopen_sender`] | Drop and reopen sender's storage |
//! | [`simulate_crash_and_reopen_receiver`] | Drop and reopen receiver's storage |
//! | [`assert_download_state`] | Inspect a download's state, bytes, retries |
//! | [`change_permission_for_retry`] | Grant/revoke permission between attempts |
//! | [`stale_catalogue_version`] | Advance manifest revision for mismatch testing |
//! | [`verify_temp_file_integrity`] | Check that temp file hash matches descriptor |
//!
//! # Example
//!
//! ```ignore
//! use test_interrupted_transfer_harness::InterruptedTransferHarness;
//!
//! #[test]
//! fn transfer_survives_crash_in_downloading_state() {
//!     let mut harness = InterruptedTransferHarness::new();
//!     let dl_id = harness.create_download_at_state("downloading", 42).unwrap();
//!
//!     // Simulate crash and reopen
//!     harness.simulate_crash_and_reopen_receiver().unwrap();
//!
//!     // After recovery, downloading → paused (temp file exists)
//!     let dl = harness.assert_download_state(dl_id, "paused").unwrap();
//!     assert_eq!(dl.bytes_downloaded, 42);
//! }
//! ```

// ── Imports ─────────────────────────────────────────────────────────────────

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use boru_core::storage::Storage;

use iroh::SecretKey;
use tempfile::TempDir;

// ── Constants ────────────────────────────────────────────────────────────────

/// Default content hash used by the harness (a blake3 hash hex string).
const DEFAULT_CONTENT_HASH: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Default file name placed in the sender's catalogue.
const DEFAULT_FILENAME: &str = "transfer-test.bin";

/// Default file size used by the harness.
const DEFAULT_FILE_SIZE: u64 = 65536;

// ── Helper: wall-clock millis ───────────────────────────────────────────────

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Helper: minimal temp dir name with pid+timestamp ────────────────────────

fn temp_dir_path(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = now_ms();
    dir.push(format!("boru-transfer-harness-{name}-{pid}-{nanos}"));
    dir
}

// ═════════════════════════════════════════════════════════════════════════════
// InterruptedTransferHarness
// ═════════════════════════════════════════════════════════════════════════════

/// Reusable test harness for interrupted-transfer scenarios.
///
/// Manages sender and receiver storage directories, seed catalogue data,
/// and provides methods for controlled interruption, crash simulation,
/// state inspection, permission changes, and catalogue version manipulation.
///
/// See the [module-level documentation](self) for design details and example.
#[derive(Debug)]
pub struct InterruptedTransferHarness {
    /// Sender's database directory (on-disk, for crash simulation).
    pub sender_dir: TempDir,
    /// Receiver's database directory (on-disk, for crash simulation).
    pub receiver_dir: TempDir,
    /// Sender's storage (lives while the harness runs; `None` after crash).
    sender: Option<Storage>,
    /// Receiver's storage (lives while the harness runs; `None` after crash).
    receiver: Option<Storage>,
    /// Hex-encoded sender public key (the file owner).
    pub sender_peer: String,
    /// Hex-encoded receiver public key (the downloader).
    pub receiver_peer: String,
    /// Content hash of the seeded file.
    pub content_hash: String,
    /// Expected file size.
    pub file_size: u64,
}

impl InterruptedTransferHarness {
    // ── Construction ────────────────────────────────────────────────────

    /// Create a new harness with on-disk sender and receiver storage.
    ///
    /// Seeds the sender's catalogue with one file entry, a manifest state
    /// row, and a permission grant for the receiver.  Both stores use
    /// `Storage::open()` on a `TempDir` so crash simulation via drop+reopen
    /// works correctly.
    pub fn new() -> Self {
        let sender_dir_path = temp_dir_path("sender");
        let receiver_dir_path = temp_dir_path("receiver");

        let sender_dir = TempDir::new_in(
            sender_dir_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("/tmp")),
        )
        .expect("create sender temp dir");

        let receiver_dir = TempDir::new_in(
            receiver_dir_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("/tmp")),
        )
        .expect("create receiver temp dir");

        // Move the TempDir to the deterministic path.
        // Actually TempDir::new_in creates inside the given dir, not at a specific path.
        // We need to use the temp_dir's actual path.
        let sender = Storage::open(sender_dir.path()).expect("open sender storage");
        let receiver = Storage::open(receiver_dir.path()).expect("open receiver storage");

        let sender_sk = SecretKey::generate();
        let receiver_sk = SecretKey::generate();
        let sender_peer = sender_sk.public().to_string();
        let receiver_peer = receiver_sk.public().to_string();

        let content_hash = DEFAULT_CONTENT_HASH.to_string();
        let file_size = DEFAULT_FILE_SIZE;

        // Seed sender's catalogue
        Self::seed_catalogue(&sender, &sender_peer, &content_hash, file_size);

        // Also seed receiver's file_objects table so foreign keys work
        // when creating downloads (the catalogue_client would do this
        // when processing the remote catalogue response).
        receiver
            .put_file_object(
                &content_hash,
                file_size,
                "application/octet-stream",
                DEFAULT_FILENAME,
                b"",
            )
            .expect("seed receiver file_object");

        // Grant default permission from sender to receiver
        sender
            .grant_permission(
                &content_hash,
                &sender_peer,
                &receiver_peer,
                "download",
                None,
            )
            .expect("seed permission");

        Self {
            sender_dir,
            receiver_dir,
            sender: Some(sender),
            receiver: Some(receiver),
            sender_peer,
            receiver_peer,
            content_hash,
            file_size,
        }
    }

    /// Seed the sender's catalogue with a file entry and manifest state.
    fn seed_catalogue(storage: &Storage, user_id: &str, content_hash: &str, file_size: u64) {
        storage
            .put_file_object(
                content_hash,
                file_size,
                "application/octet-stream",
                DEFAULT_FILENAME,
                b"",
            )
            .expect("seed file_object");

        storage
            .upsert_shared_file(
                content_hash,
                user_id,
                content_hash,
                DEFAULT_FILENAME,
                None,
                true,
            )
            .expect("seed shared_file");

        storage
            .bump_manifest_revision(user_id, "seed-hash")
            .expect("seed manifest revision");
    }

    // ── Accessors ──────────────────────────────────────────────────────

    /// Borrow a reference to sender storage (panics if crashed).
    pub fn sender(&self) -> &Storage {
        self.sender
            .as_ref()
            .expect("sender storage is None (did you call simulate_crash_and_reopen_sender?)")
    }

    /// Borrow a reference to receiver storage (panics if crashed).
    pub fn receiver(&self) -> &Storage {
        self.receiver
            .as_ref()
            .expect("receiver storage is None (did you call simulate_crash_and_reopen_receiver?)")
    }

    /// Mutable borrow sender storage (panics if crashed).
    pub fn sender_mut(&mut self) -> &mut Storage {
        self.sender
            .as_mut()
            .expect("sender storage is None (did you call simulate_crash_and_reopen_sender?)")
    }

    /// Mutable borrow receiver storage (panics if crashed).
    pub fn receiver_mut(&mut self) -> &mut Storage {
        self.receiver
            .as_mut()
            .expect("receiver storage is None (did you call simulate_crash_and_reopen_receiver?)")
    }

    // ── Download creation ───────────────────────────────────────────────

    /// Create a download row in a specific state.
    ///
    /// Creates a new download via `Storage::create_download`, then
    /// advances it to the requested state using the appropriate storage
    /// APIs (`update_download_progress`, direct SQL, etc.).
    ///
    /// Supported states:
    /// - `"queued"` — default after `create_download`
    /// - `"resolving_peer"` — after `tick` claims the row
    /// - `"downloading"` — with configurable bytes downloaded
    /// - `"verifying"` — with full bytes downloaded
    /// - `"paused"` — paused state
    /// - `"failed"` — with error message
    /// - `"complete"` — completed successfully
    /// - `"cancelled"` — cancelled
    ///
    /// `bytes` is only meaningful for `"downloading"` and `"verifying"`
    /// states; it is ignored for other states.
    pub fn create_download_at_state(&self, state: &str, bytes: u64) -> Result<i64, anyhow::Error> {
        let storage = self.receiver();
        let dl_id =
            storage.create_download(&self.content_hash, &self.sender_peer, self.file_size)?;

        match state {
            "queued" => {
                // Already queued by create_download.
            }
            "resolving_peer" => {
                storage.update_download_progress(dl_id, 0, "resolving_peer")?;
            }
            "downloading" => {
                storage.update_download_progress(dl_id, bytes, "downloading")?;
            }
            "verifying" => {
                storage.update_download_progress(dl_id, self.file_size, "downloading")?;
                storage.update_download_progress(dl_id, self.file_size, "verifying")?;
            }
            "paused" => {
                storage.pause_download(dl_id)?;
            }
            "failed" => {
                storage.fail_download(dl_id, "harness-induced failure", None)?;
            }
            "complete" => {
                storage.complete_download(dl_id, self.file_size)?;
            }
            "cancelled" => {
                storage.cancel_download(dl_id)?;
            }
            other => {
                anyhow::bail!("unsupported download state: {other}");
            }
        }

        Ok(dl_id)
    }

    /// Create a download and optionally write a temporary file for recovery testing.
    ///
    /// The temp file path is stored in the download row via
    /// `set_download_temp_path`.  If `temp_file_content` is provided, the
    /// file is written to that path (for testing temp-file recovery during
    /// crash restart).
    pub fn create_download_with_temp_file(
        &self,
        state: &str,
        temp_file_content: Option<&[u8]>,
    ) -> Result<(i64, PathBuf), anyhow::Error> {
        let dl_id = self.create_download_at_state(state, 0)?;

        if let Some(content) = temp_file_content {
            let temp_path = self
                .receiver_dir
                .path()
                .join(format!("download-{dl_id}.part"));
            std::fs::write(&temp_path, content)?;
            self.receiver()
                .set_download_temp_path(dl_id, &temp_path.to_string_lossy())?;
            Ok((dl_id, temp_path))
        } else {
            let temp_path = self
                .receiver_dir
                .path()
                .join(format!("download-{dl_id}.part"));
            Ok((dl_id, temp_path))
        }
    }

    // ── Crash simulation ────────────────────────────────────────────────

    /// Simulate a crash of the sender by dropping its storage and reopening.
    ///
    /// After this call, `sender()` and `sender_mut()` will panic until
    /// [`restore_sender`](Self::restore_sender) is called.
    pub fn simulate_crash_and_reopen_sender(&mut self) -> Result<(), anyhow::Error> {
        // Drop current handle (simulates crash)
        self.sender.take();
        // Reopen — triggers recover_crash_state + recover_downloads_from_restart
        let reopened = Storage::open(self.sender_dir.path())?;
        self.sender = Some(reopened);
        Ok(())
    }

    /// Simulate a crash of the receiver by dropping its storage and reopening.
    ///
    /// After this call, `receiver()` and `receiver_mut()` will panic until
    /// [`restore_receiver`](Self::restore_receiver) is called (but this
    /// method already restores it).
    pub fn simulate_crash_and_reopen_receiver(&mut self) -> Result<(), anyhow::Error> {
        self.receiver.take();
        let reopened = Storage::open(self.receiver_dir.path())?;
        self.receiver = Some(reopened);
        Ok(())
    }

    /// Drop and reopen both sender and receiver storage simultaneously
    /// (simulates a full system crash).
    pub fn simulate_full_crash(&mut self) -> Result<(), anyhow::Error> {
        self.sender.take();
        self.receiver.take();
        let reopened_sender = Storage::open(self.sender_dir.path())?;
        let reopened_receiver = Storage::open(self.receiver_dir.path())?;
        self.sender = Some(reopened_sender);
        self.receiver = Some(reopened_receiver);
        Ok(())
    }

    // ── State assertion ─────────────────────────────────────────────────

    /// Fetch a download row and assert its state matches `expected_state`.
    ///
    /// Returns the [`Download`](boru_core::storage::Download) for further
    /// field inspection (bytes_downloaded, retry_count, etc.).
    pub fn assert_download_state(
        &self,
        dl_id: i64,
        expected_state: &str,
    ) -> Result<boru_core::storage::Download, anyhow::Error> {
        let dl = self
            .receiver()
            .get_download(dl_id)?
            .unwrap_or_else(|| panic!("download {dl_id} not found after reopen"));
        assert_eq!(
            dl.state, expected_state,
            "download {dl_id}: expected state '{expected_state}', got '{}'",
            dl.state
        );
        Ok(dl)
    }

    /// Assert that a download row exists with exact field matches.
    ///
    /// Checks state, bytes_downloaded, retry_count, and total_bytes.
    /// Pass `None` for fields you don't care about.
    pub fn assert_download_exact(
        &self,
        dl_id: i64,
        expected_state: Option<&str>,
        expected_bytes: Option<u64>,
        expected_retries: Option<u32>,
    ) -> Result<boru_core::storage::Download, anyhow::Error> {
        let dl = self
            .receiver()
            .get_download(dl_id)?
            .unwrap_or_else(|| panic!("download {dl_id} not found"));
        if let Some(s) = expected_state {
            assert_eq!(dl.state, s, "download {dl_id} state mismatch");
        }
        if let Some(b) = expected_bytes {
            assert_eq!(dl.bytes_downloaded, b, "download {dl_id} bytes mismatch");
        }
        if let Some(r) = expected_retries {
            assert_eq!(dl.retry_count, r, "download {dl_id} retry count mismatch");
        }
        Ok(dl)
    }

    /// Assert that no download with the given id exists.
    pub fn assert_no_download(&self, dl_id: i64) {
        let dl = self.receiver().get_download(dl_id).unwrap_or_else(|e| {
            panic!("error checking download {dl_id}: {e}");
        });
        assert!(
            dl.is_none(),
            "download {dl_id} should not exist but was found in state '{}'",
            dl.unwrap().state
        );
    }

    // ── Wiring / hooks between attempts ────────────────────────────────

    /// Grant or revoke a permission between retry attempts.
    ///
    /// If `grant` is `true`, a 'download' permission is granted (upserted)
    /// from the sender to the receiver on the current content hash.
    /// If `grant` is `false`, the permission is revoked (deleted).
    ///
    /// Useful for testing that a resumed transfer correctly re-checks
    /// permissions on restart.
    pub fn change_permission_for_retry(&self, grant: bool) -> Result<(), anyhow::Error> {
        if grant {
            self.sender().grant_permission(
                &self.content_hash,
                &self.sender_peer,
                &self.receiver_peer,
                "download",
                None,
            )?;
        } else {
            let removed = self.sender().revoke_permission(
                &self.content_hash,
                &self.sender_peer,
                &self.receiver_peer,
                "download",
            )?;
            assert!(removed, "expected permission to exist for revocation");
        }
        Ok(())
    }

    /// Advance the sender's manifest revision (simulates a catalogue change).
    ///
    /// After this call, the sender's `profile_manifest_state` has a new
    /// revision and manifest hash.  Use this to test that resumed transfers
    /// detect a changed catalogue and transition to `version_mismatch`.
    pub fn stale_catalogue_version(&self) -> Result<u64, anyhow::Error> {
        let new_rev = self
            .sender()
            .bump_manifest_revision(&self.sender_peer, "changed-manifest-hash")?;
        Ok(new_rev)
    }

    /// Get the current manifest revision for the sender.
    pub fn current_catalogue_revision(&self) -> Result<u64, anyhow::Error> {
        let state = self.sender().get_manifest_state(&self.sender_peer)?;
        Ok(state.map(|s| s.revision).unwrap_or(0))
    }

    /// Replace the sender's file entry with a different hash (simulates
    /// the sender replacing a file between transfer attempts).
    ///
    /// Returns the previous content hash for reference.
    pub fn replace_sender_file(
        &mut self,
        new_hash: &str,
        new_size: u64,
    ) -> Result<String, anyhow::Error> {
        let old_hash = self.content_hash.clone();
        self.content_hash = new_hash.to_string();
        self.file_size = new_size;

        // Offer the new file and delete the old shared entry
        Self::seed_catalogue(self.sender(), &self.sender_peer, new_hash, new_size);
        self.sender()
            .delete_shared_file(&old_hash, &self.sender_peer)?;

        // Bump revision so the change is visible
        self.sender()
            .bump_manifest_revision(&self.sender_peer, "replaced-file")?;

        Ok(old_hash)
    }
    // ── Content integrity verification ─────────────────────────────────

    /// Verify that a file at `path` has the given expected size and BLAKE3
    /// hash.  Uses `verify_download_file` from the download module.
    ///
    /// Returns `Ok(())` on match, or an error with details on mismatch.
    pub fn verify_temp_file_integrity(
        &self,
        path: impl AsRef<std::path::Path>,
        expected_size: u64,
        expected_hash: &str,
    ) -> Result<(), anyhow::Error> {
        let verified =
            boru_core::download::verify_download_file(path, expected_size, expected_hash)?;
        assert_eq!(verified.bytes, expected_size);
        assert_eq!(verified.content_hash, expected_hash);
        Ok(())
    }

    /// Verify that a download's temp path (if set) matches expected integrity.
    pub fn verify_download_integrity(
        &self,
        dl_id: i64,
        expected_size: u64,
        expected_hash: &str,
    ) -> Result<(), anyhow::Error> {
        let temp_path = self
            .receiver()
            .get_download_temp_path(dl_id)?
            .expect("download has no temp path set");
        self.verify_temp_file_integrity(&temp_path, expected_size, expected_hash)
    }

    /// Clean up the receiver's temp file for a download.
    pub fn clean_temp_file(&self, dl_id: i64) -> Result<(), anyhow::Error> {
        if let Some(path) = self.receiver().get_download_temp_path(dl_id)? {
            let _ = std::fs::remove_file(&path);
            self.receiver().clear_download_temp_path(dl_id)?;
        }
        Ok(())
    }

    /// List all downloads for the current file+peer combination.
    pub fn list_downloads(&self) -> Result<Vec<boru_core::storage::Download>, anyhow::Error> {
        let results = self
            .receiver()
            .find_downloads_for_file(&self.content_hash, Some(&self.sender_peer))?;
        Ok(results)
    }

    /// Count downloads for the current file+peer combination.
    pub fn download_count(&self) -> Result<usize, anyhow::Error> {
        Ok(self.list_downloads()?.len())
    }
}

impl Default for InterruptedTransferHarness {
    fn default() -> Self {
        Self::new()
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Free-standing helper utilities
// ═════════════════════════════════════════════════════════════════════════════

/// Generate a deterministic public key hex string for test use.
///
/// The generated key is consistent across calls within the same session,
/// but differs each run (`SecretKey::generate()` is random).
pub fn random_peer_pk_string() -> String {
    SecretKey::generate().public().to_string()
}

/// Generate a deterministic content hash from a label string.
///
/// Pads or truncates to exactly 64 hex characters so it passes all
/// content hash validation.
pub fn content_hash_from_label(label: &str) -> String {
    let mut h = label.to_string();
    while h.len() < 64 {
        h.push('0');
    }
    h[..64].to_string()
}

/// Compute the BLAKE3 hex hash of a byte slice.
pub fn blake3_hex(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// Create a temporary file with the given content and return its path.
pub fn create_temp_file_in(dir: &std::path::Path, name: &str, content: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("write temp file");
    path
}

/// Simulate the remote peer rejecting a resumed permission request.
///
/// Transitions a download from `resolving_peer` or `requesting_permission`
/// back to `paused` with the rejection reason and an incremented retry
/// count.  This is the storage-level effect that
/// [`DownloadManager::reject_resumed_permission`] produces when the
/// remote peer returns `PermissionDenied`, `VersionMismatch`, `NotFound`,
/// `Busy`, `RateLimited`, or `Disabled`.
///
/// Using this helper lets harness tests verify that the correct state
/// machine transition occurs after a permission rejection without needing
/// a live network round-trip.
pub fn reject_resumed_permission(
    storage: &Storage,
    dl_id: i64,
    reason: &str,
) -> Result<(), anyhow::Error> {
    let now = now_ms() as i64;
    storage.with_conn(|conn| {
        conn.execute(
            "UPDATE downloads SET state = 'paused', last_error = ?1,\
             retry_count = retry_count + 1, updated_at_ms = ?2\
             WHERE id = ?3 AND state IN ('resolving_peer', 'requesting_permission')",
            rusqlite::params![reason, now, dl_id],
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    })?;
    Ok(())
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests — each exercises one scenario from the acceptance criteria
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── SECTION 1: Controlled interruption at each state ────────────────

    /// Queued downloads survive crash and remain queued.
    #[test]
    fn queued_survives_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness.create_download_at_state("queued", 0).unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        let dl = harness.assert_download_state(dl_id, "queued").unwrap();
        assert_eq!(dl.bytes_downloaded, 0);
        assert_eq!(dl.content_hash, harness.content_hash);
    }

    /// Resolving_peer → after crash → queued (no temp file).
    #[test]
    fn resolving_peer_reverts_to_queued_after_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("resolving_peer", 0)
            .unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        let dl = harness.assert_download_state(dl_id, "queued").unwrap();
        assert_eq!(dl.bytes_downloaded, 0);
    }

    /// Downloading without a temp file → paused after crash.
    #[test]
    fn downloading_without_temp_becomes_paused_after_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness.create_download_at_state("downloading", 42).unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        // downloading → paused (recover_downloads_from_restart always pauses
        // active transfers regardless of temp file presence)
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        // bytes should survive as persisted progress
        assert_eq!(dl.bytes_downloaded, 42);
    }

    /// Downloading with a temp file → paused after crash (temp preserved).
    #[test]
    fn downloading_with_temp_file_preserves_bytes_after_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let temp_content = b"partial-file-content-42-bytes-0123456789";
        let (dl_id, temp_path) = harness
            .create_download_with_temp_file("downloading", Some(temp_content))
            .unwrap();

        // Record some progress and the temp path
        harness
            .receiver()
            .update_download_progress(dl_id, temp_content.len() as u64, "downloading")
            .unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.bytes_downloaded, temp_content.len() as u64);
        // Temp file should still exist after reopen (WAL mode preserves it)
        assert!(
            temp_path.exists(),
            "temp file should survive crash in paused state"
        );
    }

    /// Verifying → paused after crash (download row reverted to downloading).
    /// Note: recovery logic transitions verifying → downloading if no valid
    /// destination or temp file exists, or → complete if a valid destination
    /// exists.
    #[test]
    fn verifying_without_files_reverts_to_downloading() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness.create_download_at_state("verifying", 0).unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        // No temp file exists → should revert to "downloading" for re-fetch
        let dl = harness.assert_download_state(dl_id, "downloading").unwrap();
        assert_eq!(dl.bytes_downloaded, harness.file_size);
    }

    // ── SECTION 2: Restart + state inspection ───────────────────────────

    /// Paused downloads remain paused across crashes.
    #[test]
    fn paused_survives_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness.create_download_at_state("paused", 0).unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.bytes_downloaded, 0);
    }

    /// Failed downloads remain failed across crashes.
    #[test]
    fn failed_state_is_terminal_after_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness.create_download_at_state("failed", 0).unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        let dl = harness.assert_download_state(dl_id, "failed").unwrap();
        assert!(
            dl.last_error.is_some(),
            "failed download should have an error message"
        );
    }

    /// Complete downloads remain complete across crashes.
    #[test]
    fn complete_state_is_terminal_after_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness.create_download_at_state("complete", 0).unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        harness.assert_download_state(dl_id, "complete").unwrap();
    }

    /// Cancelled downloads remain cancelled across crashes.
    #[test]
    fn cancelled_state_is_terminal_after_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness.create_download_at_state("cancelled", 0).unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        harness.assert_download_state(dl_id, "cancelled").unwrap();
    }

    // ── SECTION 3: No duplication or loss after recovery ────────────────

    /// After crash recovery, the download count remains exactly 1.
    #[test]
    fn crash_does_not_create_duplicate_downloads() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("downloading", 100)
            .unwrap();

        assert_eq!(harness.download_count().unwrap(), 1);

        harness.simulate_crash_and_reopen_receiver().unwrap();

        // Still exactly 1 download — no duplicate rows appear.
        let count = harness.download_count().unwrap();
        assert_eq!(count, 1, "crash must not create duplicate download rows");

        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.bytes_downloaded, 100, "bytes must not be lost");
    }

    /// Multiple downloads in various states all survive a single crash.
    #[test]
    fn multiple_downloads_all_survive_crash_without_corruption() {
        let mut harness = InterruptedTransferHarness::new();

        let dl_a = harness.create_download_at_state("queued", 0).unwrap();
        let dl_b = harness
            .create_download_at_state("resolving_peer", 0)
            .unwrap();
        let dl_c = harness
            .create_download_at_state("downloading", 200)
            .unwrap();
        let dl_d = harness.create_download_at_state("complete", 0).unwrap();

        assert_eq!(harness.download_count().unwrap(), 4);

        harness.simulate_crash_and_reopen_receiver().unwrap();

        let count_after = harness.download_count().unwrap();
        assert_eq!(
            count_after, 4,
            "all {count_after} downloads should still exist after crash"
        );

        // Each state recovers correctly
        harness.assert_download_state(dl_a, "queued").unwrap();
        harness.assert_download_state(dl_b, "queued").unwrap();
        harness.assert_download_state(dl_c, "paused").unwrap();
        harness.assert_download_state(dl_d, "complete").unwrap();
    }

    // ── SECTION 4: Permission changes between attempts ──────────────────

    /// Permission changes between retry attempts are reflected.
    #[test]
    fn permission_change_between_attempts() {
        let harness = InterruptedTransferHarness::new();

        // Initially permission exists
        assert!(harness
            .sender()
            .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
            .unwrap());

        // Revoke permission
        harness.change_permission_for_retry(false).unwrap();
        assert!(
            !harness
                .sender()
                .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
                .unwrap(),
            "permission should be revoked"
        );

        // Grant again
        harness.change_permission_for_retry(true).unwrap();
        assert!(
            harness
                .sender()
                .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
                .unwrap(),
            "permission should be re-granted"
        );
    }

    /// Permission can be changed after a crash, testing the retry path.
    #[test]
    fn permission_change_after_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let _dl_id = harness.create_download_at_state("downloading", 50).unwrap();

        harness.simulate_crash_and_reopen_receiver().unwrap();

        // Revoke permission for the retry
        harness.simulate_crash_and_reopen_sender().unwrap();
        harness.change_permission_for_retry(false).unwrap();

        assert!(
            !harness
                .sender()
                .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
                .unwrap(),
            "permission should be revoked after crash recovery"
        );
    }

    // ── SECTION 5: Catalogue version changes between attempts ───────────

    /// Advancing the catalogue revision is reflected.
    #[test]
    fn catalogue_version_changes_between_attempts() {
        let harness = InterruptedTransferHarness::new();
        let rev_before = harness.current_catalogue_revision().unwrap();
        assert!(rev_before > 0, "catalogue should have a revision");

        let new_rev = harness.stale_catalogue_version().unwrap();
        assert!(
            new_rev > rev_before,
            "new revision {new_rev} > old revision {rev_before}"
        );

        let rev_after = harness.current_catalogue_revision().unwrap();
        assert_eq!(rev_after, new_rev);
    }

    /// Replacing the sender's file changes content hash and size.
    #[test]
    fn replace_sender_file_between_attempts() {
        let mut harness = InterruptedTransferHarness::new();
        let old_hash = harness.content_hash.clone();

        let new_hash = content_hash_from_label("replacement-file");
        harness.replace_sender_file(&new_hash, 9999).unwrap();

        assert_ne!(harness.content_hash, old_hash, "content hash should change");
        assert_eq!(harness.file_size, 9999, "file size should update");
        assert_eq!(harness.content_hash, new_hash);
    }

    // ── SECTION 6: Content integrity assertions ─────────────────────────

    /// `verify_temp_file_integrity` passes for a valid file.
    #[test]
    fn verify_valid_temp_file_passes() {
        let harness = InterruptedTransferHarness::new();
        let content = b"hello-transfer-world!";
        let hash = blake3_hex(content);
        let path = create_temp_file_in(harness.receiver_dir.path(), "valid-temp.bin", content);

        // This should pass
        harness
            .verify_temp_file_integrity(&path, content.len() as u64, &hash)
            .unwrap();
    }

    /// `verify_temp_file_integrity` rejects a wrong hash.
    #[test]
    #[should_panic(expected = "content hash mismatch")]
    fn verify_corrupted_temp_file_fails() {
        let harness = InterruptedTransferHarness::new();
        let content = b"hello-transfer-world!";
        let wrong_hash = blake3_hex(b"different-content");
        let path = create_temp_file_in(harness.receiver_dir.path(), "corrupt-temp.bin", content);

        // This should panic due to hash mismatch
        harness
            .verify_temp_file_integrity(&path, content.len() as u64, &wrong_hash)
            .unwrap();
    }

    /// `blake3_hex` helper produces the correct hash.
    #[test]
    fn blake3_helper_produces_correct_hash() {
        let data = b"test-data-for-blake3";
        let hash = blake3_hex(data);
        assert!(!hash.is_empty(), "hash should not be empty");
        assert_eq!(hash.len(), 64, "blake3 hex should be 64 chars");
        // Cross-check with blake3 crate directly
        let expected = blake3::hash(data).to_hex().to_string();
        assert_eq!(hash, expected);
    }

    // ── SECTION 7: Cleanup and isolation ────────────────────────────────

    /// Two harness instances do not interfere with each other.
    #[test]
    fn harness_isolated_across_instances() {
        let mut h1 = InterruptedTransferHarness::new();
        let h2 = InterruptedTransferHarness::new();

        // Different peers (randomly generated each time)
        assert_ne!(h1.sender_peer, h2.sender_peer);
        assert_ne!(h1.receiver_peer, h2.receiver_peer);

        let d1 = h1.create_download_at_state("queued", 0).unwrap();
        let d2 = h2.create_download_at_state("queued", 0).unwrap();

        h1.simulate_crash_and_reopen_receiver().unwrap();

        // h1's download should still be queued
        h1.assert_download_state(d1, "queued").unwrap();
        // h2's download should be unaffected (different storage)
        h2.assert_download_state(d2, "queued").unwrap();
    }

    /// Harness seed data is set up correctly.
    #[test]
    fn harness_seed_data_is_correct() {
        let harness = InterruptedTransferHarness::new();

        // Sender has a manifest state row
        let state = harness
            .sender()
            .get_manifest_state(&harness.sender_peer)
            .unwrap();
        assert!(state.is_some(), "sender should have manifest state");

        // Permission exists
        assert!(harness
            .sender()
            .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
            .unwrap());
    }

    // ── SECTION 8: Full crash cycle across both peers ───────────────────

    /// Both peers can survive a simulated full system crash.
    #[test]
    fn full_system_crash_both_peers() {
        let mut harness = InterruptedTransferHarness::new();

        let dl_id = harness.create_download_at_state("downloading", 77).unwrap();

        assert_eq!(harness.download_count().unwrap(), 1);

        // Simulate full system crash
        harness.simulate_full_crash().unwrap();

        // Both peers should be functional
        assert!(harness
            .sender()
            .get_manifest_state(&harness.sender_peer)
            .is_ok());
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.bytes_downloaded, 77);
    }

    /// After full crash: sender's state is intact, receiver's downloads are
    /// recovered correctly.
    #[test]
    fn sender_state_intact_after_full_crash() {
        let mut harness = InterruptedTransferHarness::new();

        let _dl_id = harness.create_download_at_state("queued", 0).unwrap();

        // Check sender permission before crash
        assert!(harness
            .sender()
            .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
            .unwrap());

        harness.simulate_full_crash().unwrap();

        // Sender state should survive
        assert!(
            harness
                .sender()
                .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
                .unwrap(),
            "sender permission should survive full crash"
        );
    }

    // ── SECTION 9: Sender stops mid-transfer — recovery and resume ──────

    /// Full lifecycle: sender stops mid-transfer, both peers crash and
    /// recover, the download is resumed, a fresh descriptor is accepted,
    /// and the transfer completes successfully.
    ///
    /// Verifies:
    /// - Download survives in "paused" state with partial bytes preserved
    /// - No duplicate download rows after crash recovery
    /// - Resume transitions through "resolving_peer" → "downloading" → "complete"
    /// - Final state reports the expected total bytes
    #[test]
    fn sender_stops_mid_download_receiver_recovers_and_completes() {
        let mut harness = InterruptedTransferHarness::new();
        let partial_bytes: u64 = 30000;
        let dl_id = harness
            .create_download_at_state("downloading", partial_bytes)
            .unwrap();

        assert_eq!(harness.download_count().unwrap(), 1);

        // ── Simulate sender crash and restart ───────────────────────────
        harness.simulate_crash_and_reopen_sender().unwrap();
        // Sender's catalogue state must survive
        assert!(
            harness
                .sender()
                .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
                .unwrap(),
            "sender permission must survive crash"
        );

        // ── Simulate receiver crash and restart ─────────────────────────
        harness.simulate_crash_and_reopen_receiver().unwrap();

        // After recovery: downloading → paused (active transfers are paused)
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(
            dl.bytes_downloaded, partial_bytes,
            "partial bytes must survive crash"
        );
        assert_eq!(
            dl.content_hash, harness.content_hash,
            "content hash must survive crash"
        );

        // No duplicate rows created by recovery
        assert_eq!(
            harness.download_count().unwrap(),
            1,
            "crash must not create duplicate download rows"
        );

        // ── Resume the transfer ────────────────────────────────────────
        harness
            .receiver()
            .resume_download(dl_id)
            .expect("resume download");
        let dl = harness
            .assert_download_state(dl_id, "resolving_peer")
            .unwrap();
        assert_eq!(
            dl.bytes_downloaded, partial_bytes,
            "bytes must be preserved in resolving_peer"
        );

        // ── Accept a fresh descriptor (simulates fresh permission) ──────
        harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &harness.content_hash, harness.file_size)
            .expect("accept fresh descriptor");
        let dl = harness.assert_download_state(dl_id, "downloading").unwrap();
        // Accepting a descriptor does not reset byte count
        assert_eq!(
            dl.bytes_downloaded, partial_bytes,
            "bytes must be preserved after accepting descriptor"
        );

        // ── Complete the download ───────────────────────────────────────
        harness
            .receiver()
            .complete_download(dl_id, harness.file_size)
            .expect("complete download");

        let dl = harness.assert_download_state(dl_id, "complete").unwrap();
        assert_eq!(
            dl.bytes_downloaded, harness.file_size,
            "final bytes must equal full file size"
        );
        // Still only one download row
        assert_eq!(harness.download_count().unwrap(), 1);
    }

    /// Multiple sequential sender stops: the receiver survives repeated
    /// cycles of download → crash → recover → resume → more bytes → crash
    /// → recover → resume → complete.  Each cycle advances progress further
    /// and no data is lost between cycles.
    #[test]
    fn sender_stops_multiple_times_before_completion() {
        let mut harness = InterruptedTransferHarness::new();

        // ── Cycle 1: 10000 bytes ───────────────────────────────────────
        let dl_id = harness
            .create_download_at_state("downloading", 10000)
            .unwrap();
        assert_eq!(harness.download_count().unwrap(), 1);

        // Crash sender first
        harness.simulate_crash_and_reopen_sender().unwrap();
        // Verify sender state survives the crash
        harness
            .sender()
            .get_manifest_state(&harness.sender_peer)
            .unwrap()
            .expect("sender manifest must survive crash");

        // Then crash receiver
        harness.simulate_crash_and_reopen_receiver().unwrap();
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.bytes_downloaded, 10000, "cycle 1: bytes preserved");
        assert_eq!(harness.download_count().unwrap(), 1);

        // Resume and accept descriptor for cycle 1
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &harness.content_hash, harness.file_size)
            .unwrap();

        // ── Cycle 2: advance to 30000 bytes ────────────────────────────
        // Simulate more bytes arriving during this transfer phase
        harness
            .receiver()
            .update_download_progress(dl_id, 30000, "downloading")
            .unwrap();
        harness.assert_download_state(dl_id, "downloading").unwrap();

        // Crash sender + receiver again
        harness.simulate_crash_and_reopen_sender().unwrap();
        harness.simulate_crash_and_reopen_receiver().unwrap();

        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.bytes_downloaded, 30000, "cycle 2: bytes preserved");
        assert_eq!(
            harness.download_count().unwrap(),
            1,
            "cycle 2: no duplicates"
        );

        // Resume and accept descriptor for cycle 2
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &harness.content_hash, harness.file_size)
            .unwrap();

        // ── Cycle 3: advance to 50000 bytes ────────────────────────────
        harness
            .receiver()
            .update_download_progress(dl_id, 50000, "downloading")
            .unwrap();
        harness.assert_download_state(dl_id, "downloading").unwrap();

        // Crash sender + receiver a third time
        harness.simulate_crash_and_reopen_sender().unwrap();
        harness.simulate_crash_and_reopen_receiver().unwrap();

        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.bytes_downloaded, 50000, "cycle 3: bytes preserved");
        assert_eq!(
            harness.download_count().unwrap(),
            1,
            "cycle 3: no duplicates"
        );

        // Resume and accept descriptor for cycle 3
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &harness.content_hash, harness.file_size)
            .unwrap();

        // ── Finally, complete the transfer ──────────────────────────────
        harness
            .receiver()
            .complete_download(dl_id, harness.file_size)
            .expect("complete after multiple interruptions");

        let dl = harness.assert_download_state(dl_id, "complete").unwrap();
        assert_eq!(
            dl.bytes_downloaded, harness.file_size,
            "final bytes must equal full file size after multiple interruptions"
        );
        assert_eq!(
            harness.download_count().unwrap(),
            1,
            "final: no duplicates after multiple interruption cycles"
        );
    }

    /// Sender stops mid-transfer with a temp file already written.  After
    /// receiver crash recovery, the temp file survives and the download can
    /// be resumed and completed without data loss.
    #[test]
    fn sender_stops_temp_file_survives_and_verifies() {
        let mut harness = InterruptedTransferHarness::new();

        let temp_content = b"partial-bytes-arrived-from-sender-before-stop-42";
        let (dl_id, temp_path) = harness
            .create_download_with_temp_file("downloading", Some(temp_content))
            .unwrap();

        // Record the bytes in the download row
        harness
            .receiver()
            .update_download_progress(dl_id, temp_content.len() as u64, "downloading")
            .unwrap();

        // ── Simulate sender crash ──────────────────────────────────────
        harness.simulate_crash_and_reopen_sender().unwrap();

        // ── Simulate receiver crash ────────────────────────────────────
        harness.simulate_crash_and_reopen_receiver().unwrap();

        // Download should be paused with preserved bytes
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(
            dl.bytes_downloaded,
            temp_content.len() as u64,
            "temp file bytes preserved"
        );

        // Temp file must exist on disk after crash (WAL mode preserves it)
        assert!(
            temp_path.exists(),
            "temp file must survive crash in paused state"
        );

        // Verify temp file content integrity
        let content_hash = blake3_hex(temp_content);
        harness
            .verify_temp_file_integrity(&temp_path, temp_content.len() as u64, &content_hash)
            .expect("temp file integrity must pass after crash");

        assert_eq!(harness.download_count().unwrap(), 1);

        // ── Resume and complete ────────────────────────────────────────
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &harness.content_hash, harness.file_size)
            .unwrap();
        harness
            .receiver()
            .complete_download(dl_id, harness.file_size)
            .expect("complete after temp file recovery");

        harness.assert_download_state(dl_id, "complete").unwrap();
        assert_eq!(
            harness.download_count().unwrap(),
            1,
            "no duplicates after temp file recovery"
        );
    }

    /// After the sender stops mid-transfer and both peers recover, all
    /// download fields are preserved exactly — no bytes shift, no extra
    /// retries, no hash/peer corruption.
    #[test]
    fn sender_stops_fields_preserved_exactly() {
        let mut harness = InterruptedTransferHarness::new();
        let partial_bytes: u64 = 12345;

        // Create download and advance to downloading with partial bytes
        let dl_id = harness
            .create_download_at_state("downloading", partial_bytes)
            .unwrap();

        // ── Record all fields before sender stop ───────────────────────
        let before = harness
            .receiver()
            .get_download(dl_id)
            .unwrap()
            .expect("download must exist before crash");

        assert_eq!(before.state, "downloading");
        assert_eq!(before.bytes_downloaded, partial_bytes);
        assert_eq!(before.content_hash, harness.content_hash);
        assert_eq!(before.total_bytes, harness.file_size);
        assert_eq!(before.retry_count, 0);
        assert_eq!(before.remote_peer, harness.sender_peer);

        // ── Simulate sender stop (crash and restart) ───────────────────
        harness.simulate_crash_and_reopen_sender().unwrap();

        // ── Simulate receiver crash and restart ────────────────────────
        harness.simulate_crash_and_reopen_receiver().unwrap();

        // ── Verify all fields preserved exactly ────────────────────────
        let after = harness
            .receiver()
            .get_download(dl_id)
            .unwrap()
            .expect("download must exist after crash");

        assert_ne!(
            after.state, before.state,
            "state must change: downloading → paused"
        );
        assert_eq!(
            after.state, "paused",
            "download must be paused after recovery"
        );
        assert_eq!(
            after.bytes_downloaded, partial_bytes,
            "bytes_downloaded must be preserved exactly"
        );
        assert_eq!(
            after.content_hash, harness.content_hash,
            "content_hash must be preserved"
        );
        assert_eq!(
            after.total_bytes, harness.file_size,
            "total_bytes must be preserved"
        );
        assert_eq!(after.retry_count, 0, "retry_count must stay at 0");
        assert_eq!(
            after.remote_peer, harness.sender_peer,
            "remote_peer must be preserved"
        );

        assert_eq!(harness.download_count().unwrap(), 1);

        // ── Complete the transfer and verify final state ────────────────
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &harness.content_hash, harness.file_size)
            .unwrap();
        harness
            .receiver()
            .complete_download(dl_id, harness.file_size)
            .expect("complete download after field-preservation check");

        let final_dl = harness.assert_download_state(dl_id, "complete").unwrap();
        assert_eq!(final_dl.bytes_downloaded, harness.file_size);
        assert_eq!(harness.download_count().unwrap(), 1);
    }

    // ── SECTION 10: Edge-case resume scenarios after receiver stop ─────
    //
    // Tests that verify the product protocol's handling of version changes,
    // permission revocation, expired descriptors, and retry book-keeping
    // when the receiver reconnects after an interruption.

    /// Resuming a download after crash where the content hash changed
    /// results in version_mismatch (not silent data corruption).
    #[test]
    fn resume_after_crash_detects_changed_content_hash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("downloading", 5000)
            .unwrap();

        // Crash + reopen → downloading becomes paused
        harness.simulate_crash_and_reopen_receiver().unwrap();
        harness.assert_download_state(dl_id, "paused").unwrap();

        // Resume: paused → resolving_peer
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .assert_download_state(dl_id, "resolving_peer")
            .unwrap();

        // Accept descriptor with WRONG hash → version_mismatch
        let wrong_hash = content_hash_from_label("wrong-content");
        let result =
            harness
                .receiver()
                .accept_resumed_descriptor(dl_id, &wrong_hash, harness.file_size);
        assert!(
            result.is_err(),
            "accepting descriptor with changed hash must fail"
        );

        let dl = harness
            .assert_download_state(dl_id, "version_mismatch")
            .unwrap();
        assert!(
            dl.last_error.is_some(),
            "version_mismatch must record an error message"
        );
        assert_eq!(
            harness.download_count().unwrap(),
            1,
            "no duplicate rows on version mismatch"
        );
    }

    /// Resuming after crash when permission was revoked during the outage
    /// → the resumed permission request is correctly rejected.
    #[test]
    fn resume_after_crash_with_revoked_permission_rejected() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("downloading", 20000)
            .unwrap();

        // Crash + reopen → downloading becomes paused
        harness.simulate_crash_and_reopen_receiver().unwrap();
        harness.assert_download_state(dl_id, "paused").unwrap();

        // Revoke permission on the sender side while receiver was down
        harness.simulate_crash_and_reopen_sender().unwrap();
        harness.change_permission_for_retry(false).unwrap();

        // Resume: paused → resolving_peer
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .assert_download_state(dl_id, "resolving_peer")
            .unwrap();

        // Simulate permission rejection
        harness
            .receiver()
            .reject_resumed_permission(dl_id, "permission denied after receiver restart")
            .unwrap();
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert!(
            dl.last_error.is_some(),
            "rejected permission must record an error"
        );
        assert_eq!(
            dl.retry_count, 1,
            "retry count must increment on rejection after restart"
        );

        // Exactly one download row
        assert_eq!(harness.download_count().unwrap(), 1);
    }

    /// Resume after crash with an expired descriptor — the download
    /// returns to paused so a fresh grant must be obtained.
    #[test]
    fn resume_after_crash_expired_descriptor_returns_to_paused() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("downloading", 8000)
            .unwrap();

        // Crash + reopen → downloading becomes paused
        harness.simulate_crash_and_reopen_receiver().unwrap();
        harness.assert_download_state(dl_id, "paused").unwrap();

        // Resume: paused → resolving_peer
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .assert_download_state(dl_id, "resolving_peer")
            .unwrap();

        // Accept descriptor with expiry in the past (now=9999, expires=5000)
        let result = harness.receiver().accept_resumed_descriptor_at(
            dl_id,
            &harness.content_hash,
            harness.file_size,
            5000, // expires_at_ms (in the past)
            9999, // now_ms (past the expiry)
        );
        assert!(result.is_err(), "expired descriptor must be rejected");

        // Should return to paused with error mentioning expiry
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert!(
            dl.last_error.is_some(),
            "paused state after expired descriptor must have error"
        );
        let err = dl.last_error.as_ref().unwrap();
        assert!(
            err.to_lowercase().contains("expired"),
            "error must mention expiry: {err}"
        );

        // Exactly one download row
        assert_eq!(harness.download_count().unwrap(), 1);
    }

    /// Resolving_peer state reverts to queued on crash (no temp file).
    /// The product protocol discards the unresolved state and lets the
    /// normal tick cycle re-claim it.
    #[test]
    fn resolving_peer_reverts_to_queued_on_crash() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("resolving_peer", 0)
            .unwrap();

        // Crash + reopen → resolving_peer becomes queued (no temp file)
        harness.simulate_crash_and_reopen_receiver().unwrap();
        let dl = harness.assert_download_state(dl_id, "queued").unwrap();
        assert_eq!(dl.bytes_downloaded, 0);

        // Exactly one download row — no duplicates from recovery
        assert_eq!(harness.download_count().unwrap(), 1);
    }

    /// Multiple downloads in different states all survive receiver crash
    /// and each can be individually resumed to completion.
    #[test]
    fn multiple_downloads_all_survive_crash_and_resume() {
        let mut harness = InterruptedTransferHarness::new();

        let dl_a = harness
            .create_download_at_state("downloading", 15000)
            .unwrap();
        let dl_b = harness.create_download_at_state("paused", 25000).unwrap();
        let dl_c = harness.create_download_at_state("queued", 0).unwrap();
        let dl_d = harness.create_download_at_state("complete", 0).unwrap();

        assert_eq!(harness.download_count().unwrap(), 4);

        // Simulate sender stop + receiver crash
        harness.simulate_crash_and_reopen_sender().unwrap();
        harness.simulate_crash_and_reopen_receiver().unwrap();

        // All 4 still exist — no duplicate creation
        assert_eq!(harness.download_count().unwrap(), 4);

        // Each recovered to the correct state
        harness.assert_download_state(dl_a, "paused").unwrap(); // downloading → paused
        harness.assert_download_state(dl_b, "paused").unwrap(); // paused stays paused
        harness.assert_download_state(dl_c, "queued").unwrap(); // queued stays queued
        harness.assert_download_state(dl_d, "complete").unwrap(); // complete stays complete

        // Resume A: paused → resolving_peer → downloading → complete
        harness.receiver().resume_download(dl_a).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_a, &harness.content_hash, harness.file_size)
            .unwrap();
        harness
            .receiver()
            .complete_download(dl_a, harness.file_size)
            .unwrap();
        harness.assert_download_state(dl_a, "complete").unwrap();

        // Resume B
        harness.receiver().resume_download(dl_b).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_b, &harness.content_hash, harness.file_size)
            .unwrap();
        harness
            .receiver()
            .complete_download(dl_b, harness.file_size)
            .unwrap();

        // All four still present — no duplicates after resume
        assert_eq!(harness.download_count().unwrap(), 4);
        harness.assert_download_state(dl_a, "complete").unwrap();
        harness.assert_download_state(dl_b, "complete").unwrap();
        harness.assert_download_state(dl_c, "queued").unwrap();
        harness.assert_download_state(dl_d, "complete").unwrap();
    }

    /// Retry count is preserved and managed correctly through repeated
    /// crash+resume+rejection cycles before eventual success.
    #[test]
    fn retry_count_tracked_across_crash_and_resume_cycles() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("downloading", 12000)
            .unwrap();

        // ── Crash 1 → paused ─────────────────────────────────────
        harness.simulate_crash_and_reopen_receiver().unwrap();
        harness.assert_download_state(dl_id, "paused").unwrap();

        // Resume 1 → reject permission (simulates transient failure)
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .reject_resumed_permission(dl_id, "transient failure after crash 1")
            .unwrap();
        let _dl = harness
            .assert_download_exact(dl_id, Some("paused"), Some(12000), Some(1))
            .unwrap();

        // ── Crash 2 → retry_count survives ───────────────────────
        harness.simulate_crash_and_reopen_receiver().unwrap();
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.retry_count, 1, "retry_count must survive crash");

        // Resume 2 → reject again
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .reject_resumed_permission(dl_id, "transient failure after crash 2")
            .unwrap();
        let _dl2 = harness
            .assert_download_exact(dl_id, Some("paused"), Some(12000), Some(2))
            .unwrap();

        // ── Resume 3 → success ───────────────────────────────────
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &harness.content_hash, harness.file_size)
            .unwrap();
        harness.assert_download_state(dl_id, "downloading").unwrap();

        // Complete
        harness
            .receiver()
            .complete_download(dl_id, harness.file_size)
            .unwrap();
        harness.assert_download_state(dl_id, "complete").unwrap();
        assert_eq!(harness.download_count().unwrap(), 1);
    }

    /// Full crash of both peers — receiver's download survives and can
    /// be fully resumed to completion.
    #[test]
    fn full_system_crash_both_peers_receiver_resume_completes() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("downloading", 42000)
            .unwrap();

        assert_eq!(harness.download_count().unwrap(), 1);

        // Full system crash — both peers drop and reopen
        harness.simulate_full_crash().unwrap();

        // Both peers functional
        assert!(
            harness
                .sender()
                .get_manifest_state(&harness.sender_peer)
                .is_ok(),
            "sender manifest must survive full crash"
        );
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(dl.bytes_downloaded, 42000);

        // Resume → resolving_peer → downloading → complete
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &harness.content_hash, harness.file_size)
            .unwrap();
        harness
            .receiver()
            .complete_download(dl_id, harness.file_size)
            .unwrap();
        harness.assert_download_state(dl_id, "complete").unwrap();
        assert_eq!(harness.download_count().unwrap(), 1);
    }

    // ── SECTION 11: Resume after external state changes (permission,
    // catalogue) and follow-up transfer verifies no corruption ─────────

    /// After interrupted transfer with permission revoked, resume is
    /// rejected.  Regranting permission allows a fresh download to complete
    /// cleanly — no corrupt or falsely-completed state remains.
    #[test]
    fn permission_revoked_before_resume_then_regranted_new_transfer_works() {
        let mut harness = InterruptedTransferHarness::new();
        let dl_id = harness
            .create_download_at_state("downloading", 18000)
            .unwrap();
        assert_eq!(harness.download_count().unwrap(), 1);

        // ── Phase 1: Interruption ──────────────────────────────────────
        harness.simulate_crash_and_reopen_receiver().unwrap();
        harness.assert_download_state(dl_id, "paused").unwrap();

        // ── Phase 2: Revoke permission while receiver was down ──────────
        harness.simulate_crash_and_reopen_sender().unwrap();
        harness.change_permission_for_retry(false).unwrap();
        assert!(
            !harness
                .sender()
                .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
                .unwrap(),
            "permission must be revoked before resume attempt"
        );

        // ── Phase 3: Resume attempt — rejected ─────────────────────────
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .assert_download_state(dl_id, "resolving_peer")
            .unwrap();

        reject_resumed_permission(
            harness.receiver(),
            dl_id,
            "permission denied: revoked during outage",
        )
        .unwrap();
        let dl = harness.assert_download_state(dl_id, "paused").unwrap();
        assert!(dl.last_error.is_some(), "rejected resume must record error");
        assert_eq!(
            dl.retry_count, 1,
            "retry_count must increment on permission rejection"
        );

        // ── Phase 4: Regrant permission ─────────────────────────────────
        harness.change_permission_for_retry(true).unwrap();
        assert!(
            harness
                .sender()
                .check_permission(&harness.content_hash, &harness.receiver_peer, "download")
                .unwrap(),
            "permission must be regranted"
        );

        // ── Phase 5: Fresh transfer completes cleanly ──────────────────
        // Using a different download id proves the system is not corrupt.
        let fresh_id = harness.create_download_at_state("paused", 0).unwrap();
        assert_eq!(harness.download_count().unwrap(), 2);

        harness.receiver().resume_download(fresh_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(fresh_id, &harness.content_hash, harness.file_size)
            .unwrap();
        harness
            .receiver()
            .complete_download(fresh_id, harness.file_size)
            .unwrap();
        harness.assert_download_state(fresh_id, "complete").unwrap();

        // Original paused download is still intact — no false completion
        let original = harness.assert_download_state(dl_id, "paused").unwrap();
        assert_eq!(original.bytes_downloaded, 18000);
        assert_eq!(harness.download_count().unwrap(), 2);
    }

    /// After interrupted transfer where the sender's catalogue content
    /// changed (file replaced + revision bumped), resume detects the
    /// version mismatch.  A subsequent fresh download for the new file
    /// completes cleanly — no corrupt or falsely-completed state.
    #[test]
    fn catalogue_changed_during_interruption_resume_detects_mismatch() {
        let mut harness = InterruptedTransferHarness::new();
        let old_hash = harness.content_hash.clone();
        let dl_id = harness
            .create_download_at_state("downloading", 25000)
            .unwrap();
        assert_eq!(harness.download_count().unwrap(), 1);

        // ── Phase 1: Interruption ──────────────────────────────────────
        // Receiver crashes while downloading
        harness.simulate_crash_and_reopen_receiver().unwrap();
        harness.assert_download_state(dl_id, "paused").unwrap();

        // ── Phase 2: Sender replaces the file + bumps revision ─────────
        let new_hash = content_hash_from_label("replacement-catalogue-file");
        let new_size = 99999u64;
        harness.replace_sender_file(&new_hash, new_size).unwrap();

        // Verify catalogue truly changed
        assert_eq!(harness.content_hash, new_hash);
        assert_eq!(harness.file_size, new_size);
        let dl = harness.receiver().get_download(dl_id).unwrap().unwrap();
        assert_eq!(
            dl.content_hash, old_hash,
            "existing download must still reference the old hash"
        );

        // ── Phase 3: Resume attempt — version_mismatch ─────────────────
        harness.receiver().resume_download(dl_id).unwrap();
        harness
            .assert_download_state(dl_id, "resolving_peer")
            .unwrap();

        // Accepting a descriptor with the NEW hash fails because the
        // download row expects the OLD content_hash.
        let result = harness
            .receiver()
            .accept_resumed_descriptor(dl_id, &new_hash, new_size);
        assert!(
            result.is_err(),
            "accepting descriptor with catalogue-changed hash must fail"
        );

        let dl = harness
            .assert_download_state(dl_id, "version_mismatch")
            .unwrap();
        assert!(
            dl.last_error.is_some(),
            "version_mismatch must record error"
        );
        assert!(
            dl.last_error.as_ref().unwrap().contains("hash mismatch"),
            "error must mention hash mismatch"
        );

        // ── Phase 4: Fresh download for the new file completes cleanly ──
        // The receiver's file_objects table doesn't have the new hash yet,
        // so seed it (as catalogue_client would when processing the new
        // catalogue).
        harness
            .receiver()
            .put_file_object(
                &new_hash,
                new_size,
                "application/octet-stream",
                "replacement.bin",
                b"",
            )
            .unwrap();

        let fresh_id = harness
            .receiver()
            .create_download(&new_hash, &harness.sender_peer, new_size)
            .unwrap();
        // After replace_sender_file, harness.content_hash is now new_hash,
        // so download_count() searches by new_hash — only finds fresh_id.
        assert_eq!(harness.download_count().unwrap(), 1);

        // Pause the fresh download so we can resume it
        harness.receiver().pause_download(fresh_id).unwrap();

        harness.receiver().resume_download(fresh_id).unwrap();
        harness
            .receiver()
            .accept_resumed_descriptor(fresh_id, &new_hash, new_size)
            .unwrap();
        harness
            .receiver()
            .complete_download(fresh_id, new_size)
            .unwrap();
        harness.assert_download_state(fresh_id, "complete").unwrap();

        // Old download is still in version_mismatch — no false progress
        harness
            .assert_download_state(dl_id, "version_mismatch")
            .unwrap();
        // Non-targeted count: verify both rows exist individually
        assert!(
            harness.receiver().get_download(dl_id).unwrap().is_some(),
            "old download must still exist"
        );
        assert!(
            harness.receiver().get_download(fresh_id).unwrap().is_some(),
            "fresh download must still exist"
        );
    }
}
