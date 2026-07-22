//! Durable interruption/restart scenarios for download transfers.
//!
//! These tests model recovery through the Application-layer pause/resume cycle.
//! The worker must always re-resolve the peer and obtain a fresh descriptor
//! before transferring bytes.  No assumptions are made about auto-pausing on
//! reopen — the Storage layer preserves state but does not rewrite active rows
//! at startup.

use boru_core::storage::Storage;

fn make_storage(hash: &str) -> Storage {
    let storage = Storage::memory().expect("in-memory storage");
    storage
        .put_file_object(hash, 1024, "application/octet-stream", "transfer.bin", &[])
        .expect("seed file object");
    storage
}

fn enter_downloading(storage: &Storage, hash: &str, size: u64) -> i64 {
    let id = storage
        .create_download(hash, "sender-peer", size)
        .expect("create download");
    storage
        .update_download_progress(id, 0, "downloading")
        .expect("enter downloading via progress update");
    id
}

#[test]
fn pause_preserves_progress_during_active_transfer() {
    let storage = make_storage("transfer-hash");

    let id = enter_downloading(&storage, "transfer-hash", 1024);
    storage
        .update_download_progress(id, 384, "downloading")
        .expect("update progress");
    storage.pause_download(id).expect("pause active download");

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "paused");
    assert_eq!(dl.bytes_downloaded, 384);
    assert_eq!(dl.content_hash, "transfer-hash");
}

#[test]
fn resume_goes_through_resolving_peer_phase() {
    let storage = make_storage("queued-hash");

    let id = enter_downloading(&storage, "queued-hash", 1024);
    storage.pause_download(id).expect("pause");
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "paused");
    assert_eq!(dl.bytes_downloaded, 0);

    // Resume always transitions to resolving_peer — no shortcut to bytes.
    storage.resume_download(id).expect("resume");
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "resolving_peer");
    assert_eq!(dl.bytes_downloaded, 0);
}

#[test]
fn fresh_permission_required_after_resume() {
    let storage = make_storage("transfer-hash");

    // Create a download, make partial progress, pause it.
    let id = enter_downloading(&storage, "transfer-hash", 1024);
    storage
        .update_download_progress(id, 512, "downloading")
        .expect("partial progress");
    storage.pause_download(id).expect("pause");

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "paused");
    assert_eq!(dl.bytes_downloaded, 512);

    // Resume → resolving_peer (must re-resolve, not jump back to download).
    storage.resume_download(id).expect("resume");
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "resolving_peer"
    );

    // Reject: expired descriptor → stayed paused with error.
    let err = storage
        .accept_resumed_descriptor_at(id, "transfer-hash", 1024, 1_000, 2_000)
        .expect_err("expired descriptor must be rejected");
    assert!(err.to_string().contains("expired"), "error: {err}");
    let denied = storage.get_download(id).unwrap().unwrap();
    assert_eq!(denied.state, "paused");
    assert_eq!(denied.bytes_downloaded, 512);
    assert!(denied.last_error.unwrap().contains("expired"));

    // A later fresh descriptor for the unchanged target can proceed.
    storage.resume_download(id).expect("second resume");
    storage
        .accept_resumed_descriptor(id, "transfer-hash", 1024)
        .expect("accept fresh descriptor");
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "downloading"
    );
}

#[test]
fn changed_descriptor_hash_goes_to_version_mismatch() {
    let storage = make_storage("original-hash");

    let id = enter_downloading(&storage, "original-hash", 1024);
    storage.pause_download(id).expect("pause");
    storage.resume_download(id).expect("resume");

    // Accepting a descriptor with a different content hash must be rejected
    // and recorded as a terminal version mismatch.
    let err = storage
        .accept_resumed_descriptor(id, "changed-hash", 2048)
        .expect_err("mismatched hash must be rejected");
    assert!(err.to_string().contains("hash mismatch"), "error: {err}");

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "version_mismatch");
    assert_eq!(dl.content_hash, "original-hash");
    assert_eq!(dl.bytes_downloaded, 0);
}

#[test]
fn expired_resume_descriptor_keeps_partial_transfer_paused() {
    let storage = make_storage("partial-hash");

    let id = enter_downloading(&storage, "partial-hash", 1024);
    storage
        .update_download_progress(id, 256, "downloading")
        .expect("partial progress");
    storage.pause_download(id).expect("pause");
    storage.resume_download(id).expect("resume");

    // Descriptor expired before it could be used → stays paused.
    let err = storage
        .accept_resumed_descriptor_at(id, "partial-hash", 1024, 500, 1_000)
        .expect_err("expired descriptor must be rejected");
    assert!(err.to_string().contains("expired"), "error: {err}");

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "paused");
    assert_eq!(dl.bytes_downloaded, 256);
    assert!(dl.last_error.unwrap().contains("expired"));
}
