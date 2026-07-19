//! Download integration tests — create, get, fail, complete, pause, resume,
//! and list downloads via the Storage API directly.
//!
//! No public DHT / DNS / relays / internet dependency.  Uses `Storage::memory()`.
//! There is no `DownloadManager` / state-machine `tick()` — every operation goes
//! through `Storage` methods directly and is synchronous.
//!
//! Scenarios covered (10):
//!   1. Create and complete a small download
//!   2. Large total_bytes boundary
//!   3. Fail a download with retry scheduling
//!   4. Create, pause, resume, then complete
//!   5. Resume and accept descriptor → downloading
//!   6. List downloads by state (queued, failed, complete)
//!   7. Multiple downloads at different states
//!   8. Re-create a download for the same content_hash
//!   9. Pause an already-paused download is idempotent
//!  10. Fail a download that has progress, verify error & retry_count

use boru_chat::{download::DownloadState, storage::Storage};

const FILE_SIZE: u64 = 1024;

fn make_storage(hash: &str) -> Storage {
    let storage = Storage::memory().expect("in-memory storage");
    storage
        .put_file_object(hash, FILE_SIZE, "application/octet-stream", hash, &[])
        .expect("seed");
    storage
}

fn make_storage_multi(hashes: &[&str]) -> Storage {
    let storage = Storage::memory().expect("in-memory storage");
    for h in hashes {
        storage
            .put_file_object(h, FILE_SIZE, "application/octet-stream", h, &[])
            .expect("seed");
    }
    storage
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 1: Create and complete a small download
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn create_and_complete_small_download() {
    let storage = make_storage("small-hash");

    let id = storage.create_download("small-hash", "alice", 64).unwrap();

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.content_hash, "small-hash");
    assert_eq!(dl.remote_peer, "alice");
    assert_eq!(dl.total_bytes, 64);
    assert_eq!(dl.state, "queued");
    assert_eq!(dl.bytes_downloaded, 0);
    assert_eq!(dl.retry_count, 0);
    assert!(dl.last_error.is_none());

    storage.complete_download(id, 64).unwrap();

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "complete");
    assert_eq!(dl.bytes_downloaded, 64);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 2: Large file size boundary
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn large_file_size_boundary() {
    let storage = make_storage("large-hash");
    let huge = u64::MAX / 2;

    let id = storage.create_download("large-hash", "bob", huge).unwrap();

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.content_hash, "large-hash");
    assert_eq!(dl.total_bytes, huge);
    assert_eq!(dl.state, "queued");

    storage.complete_download(id, huge).unwrap();

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "complete");
    assert_eq!(dl.total_bytes, huge);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 3: Fail a download with error and retry scheduling
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn fail_download_with_retry_schedule() {
    let storage = make_storage("fail-hash");

    let id = storage.create_download("fail-hash", "carol", 512).unwrap();

    // Fail with a retry-at time in the future.
    let retry_at = 9999999999999u64;
    storage
        .fail_download(id, "network timeout", Some(retry_at))
        .unwrap();

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "failed");
    assert!(dl.last_error.as_deref().unwrap_or("").contains("timeout"));
    assert_eq!(dl.retry_count, 1);
    assert_eq!(dl.next_retry_at_ms, Some(retry_at));

    // Verify is_terminal() matches DownloadState semantics.
    assert!(DownloadState::Failed.is_terminal());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 4: Create, pause, resume, accept descriptor → downloading
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn create_pause_resume_and_accept_descriptor() {
    let storage = make_storage("pause-resume-hash");

    let id = storage
        .create_download("pause-resume-hash", "dave", 200)
        .unwrap();

    // Pause before any progress.
    storage.pause_download(id).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "paused");

    // Resume goes to resolving_peer.
    storage.resume_download(id).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "resolving_peer");

    // Accept a fresh descriptor → downloading.
    storage
        .accept_resumed_descriptor(id, "pause-resume-hash", 200)
        .unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "downloading");

    // Complete the download.
    storage.complete_download(id, 200).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "complete");
    assert_eq!(dl.bytes_downloaded, 200);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 5: Resume rejects a changed content hash → version_mismatch
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn resume_rejects_changed_content_hash() {
    let storage = make_storage("original-hash");

    let id = storage
        .create_download("original-hash", "eve", FILE_SIZE)
        .unwrap();

    storage.pause_download(id).unwrap();
    storage.resume_download(id).unwrap();

    // Accepting a descriptor with a different hash should fail
    // and transition to version_mismatch.
    let err = storage
        .accept_resumed_descriptor(id, "changed-hash", FILE_SIZE * 2)
        .unwrap_err();
    assert!(err.to_string().contains("hash mismatch"));

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "version_mismatch");
    assert!(DownloadState::VersionMismatch.is_terminal());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 6: List downloads by state (queued, failed, complete)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn list_downloads_by_state() {
    let storage = make_storage_multi(&["list-a", "list-b", "list-c", "list-d"]);

    let id_a = storage.create_download("list-a", "frank", 100).unwrap();
    let id_b = storage.create_download("list-b", "grace", 200).unwrap();
    let id_c = storage.create_download("list-c", "heidi", 300).unwrap();
    let id_d = storage.create_download("list-d", "ivan", 400).unwrap();

    // All 4 start as queued.
    let all = storage.list_downloads_by_state("queued").unwrap();
    assert_eq!(all.len(), 4);

    // Complete two, fail one, leave one queued.
    storage.complete_download(id_a, 100).unwrap();
    storage.complete_download(id_b, 200).unwrap();
    storage
        .fail_download(id_c, "permission denied", None)
        .unwrap();

    assert_eq!(
        storage.list_downloads_by_state("complete").unwrap().len(),
        2
    );
    assert_eq!(storage.list_downloads_by_state("failed").unwrap().len(), 1);
    assert_eq!(storage.list_downloads_by_state("queued").unwrap().len(), 1);

    // Verify the queued one is still id_d.
    let queued = storage.list_downloads_by_state("queued").unwrap();
    assert_eq!(queued[0].id, id_d);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 7: Multiple downloads at different states coexist
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn multiple_downloads_different_states() {
    let storage = make_storage_multi(&["multi-q", "multi-c", "multi-f", "multi-p", "multi-vm"]);

    let q = storage.create_download("multi-q", "judy", 100).unwrap();
    let c = storage.create_download("multi-c", "karen", 200).unwrap();
    let f = storage.create_download("multi-f", "leo", 300).unwrap();
    let p = storage.create_download("multi-p", "mallory", 400).unwrap();
    let vm = storage.create_download("multi-vm", "nancy", 500).unwrap();

    // Push each to a different state.
    storage.complete_download(c, 200).unwrap();
    storage.fail_download(f, "disk full", None).unwrap();
    storage.pause_download(p).unwrap();

    // Version mismatch via accept_resumed_descriptor with wrong hash.
    storage.pause_download(vm).unwrap();
    storage.resume_download(vm).unwrap();
    let _ = storage.accept_resumed_descriptor(vm, "wrong-hash", 600);

    // q stays queued.
    assert_eq!(storage.get_download(q).unwrap().unwrap().state, "queued");
    assert_eq!(storage.get_download(c).unwrap().unwrap().state, "complete");
    assert_eq!(storage.get_download(f).unwrap().unwrap().state, "failed");
    assert_eq!(storage.get_download(p).unwrap().unwrap().state, "paused");
    assert_eq!(
        storage.get_download(vm).unwrap().unwrap().state,
        "version_mismatch"
    );

    // Each state list returns exactly one.
    assert_eq!(storage.list_downloads_by_state("queued").unwrap().len(), 1);
    assert_eq!(
        storage.list_downloads_by_state("complete").unwrap().len(),
        1
    );
    assert_eq!(storage.list_downloads_by_state("failed").unwrap().len(), 1);
    assert_eq!(storage.list_downloads_by_state("paused").unwrap().len(), 1);
    assert_eq!(
        storage
            .list_downloads_by_state("version_mismatch")
            .unwrap()
            .len(),
        1
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 8: Re-create a download for the same content_hash (distinct ids)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn duplicate_content_hash_gets_separate_ids() {
    let storage = make_storage("dedup-hash");

    let id1 = storage.create_download("dedup-hash", "oscar", 600).unwrap();
    let id2 = storage.create_download("dedup-hash", "peggy", 600).unwrap();

    assert_ne!(id1, id2, "each download must have a unique id");

    // Both complete independently.
    storage.complete_download(id1, 600).unwrap();
    storage.complete_download(id2, 600).unwrap();

    assert_eq!(
        storage.get_download(id1).unwrap().unwrap().state,
        "complete"
    );
    assert_eq!(
        storage.get_download(id2).unwrap().unwrap().state,
        "complete"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 9: Pause an already-paused download is idempotent
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn pause_already_paused_is_idempotent() {
    let storage = make_storage("idempotent-hash");

    let id = storage
        .create_download("idempotent-hash", "quentin", 128)
        .unwrap();

    storage.pause_download(id).unwrap();
    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");

    // Pause again — should be a no-op.
    storage.pause_download(id).unwrap();
    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");

    // Resume → resolving_peer.
    storage.resume_download(id).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "resolving_peer"
    );

    // Resume again — should be idempotent (stays resolving_peer).
    storage.resume_download(id).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "resolving_peer"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 10: Fail a download that has progress, verify error and retry_count
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn fail_with_progress_tracks_retry_count() {
    let storage = make_storage("progress-hash");

    let id = storage
        .create_download("progress-hash", "rupert", 1000)
        .unwrap();

    // Simulate some progress before failing.
    storage
        .update_download_progress(id, 400, "downloading")
        .unwrap();

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.bytes_downloaded, 400);

    // First failure.
    storage
        .fail_download(id, "connection reset by peer", Some(1000))
        .unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "failed");
    assert_eq!(dl.retry_count, 1);

    // Fail again — retry_count increments.
    // (fail_download is guarded against paused state but not failed,
    // so we can transition failed→failed by calling it again.)
    storage
        .fail_download(id, "retry also failed", Some(2000))
        .unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "failed");
    assert_eq!(dl.retry_count, 2);
    assert!(dl
        .last_error
        .as_deref()
        .unwrap_or("")
        .contains("also failed"));
    assert_eq!(dl.next_retry_at_ms, Some(2000));

    assert!(DownloadState::Failed.is_terminal());
}
