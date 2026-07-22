//! Corrupted-download safety coverage.
//!
//! The transfer target is deliberately written with bytes that have the right
//! length but the wrong BLAKE3 digest.  Verification must reject it before the
//! atomic install, and a fresh download must start without trusting the failed
//! partial.

use std::io::Write;

use boru_core::download::verify_install_and_complete;
use boru_core::storage::Storage;
use tempfile::TempDir;

/// Seed a file-object entry, create a download, then advance it to the
/// given active state via `update_download_progress`.
fn create_download_in_state(
    storage: &Storage,
    hash: &str,
    size: u64,
    peer: &str,
    state: &str,
) -> i64 {
    storage
        .put_file_object(hash, size, "application/octet-stream", "corrupt.bin", &[])
        .expect("seed file object");
    let id = storage
        .create_download(hash, peer, size)
        .expect("create download");
    storage
        .update_download_progress(id, 0, state)
        .expect("set initial state");
    id
}

#[test]
fn corrupted_content_fails_before_install_and_retry_can_complete() {
    let storage = Storage::memory().expect("storage");
    let dir = TempDir::new().expect("temp directory");
    let destination = dir.path().join("corrupt.bin");
    let temp_path = dir.path().join("corrupt.bin.part");
    let trusted = b"trusted content";
    let corrupted = b"trusxed content"; // same length, different digest
    assert_eq!(trusted.len(), corrupted.len());
    let expected_hash = blake3::hash(trusted).to_hex().to_string();

    let id = create_download_in_state(
        &storage,
        &expected_hash,
        trusted.len() as u64,
        "corrupt-peer",
        "verifying",
    );

    // Write corrupted bytes to the temp file.
    std::fs::File::create(&temp_path)
        .expect("create partial")
        .write_all(corrupted)
        .expect("write corrupted bytes");

    // Verification must reject the corrupted file.
    let error = verify_install_and_complete(
        &storage,
        id,
        &temp_path,
        &destination,
        trusted.len() as u64,
        &expected_hash,
    )
    .expect_err("corrupted bytes must be rejected");
    assert!(
        error.to_string().contains("content hash mismatch"),
        "error: {error}"
    );
    assert!(
        temp_path.exists(),
        "verification must not install or trust partial"
    );
    assert!(
        !destination.exists(),
        "destination must not be created on mismatch"
    );
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "verifying"
    );

    // The worker records failure and removes the rejected partial.
    std::fs::remove_file(&temp_path).expect("remove rejected partial");
    storage
        .fail_download(id, &error.to_string(), None)
        .expect("record failed download");
    let failed = storage.get_download(id).unwrap().unwrap();
    assert_eq!(failed.state, "failed");
    assert!(
        !destination.exists(),
        "destination must not exist after failure"
    );

    // Create a new download for the same content — a fresh attempt with the
    // trusted bytes may now be installed normally.  (The old `retry_download`
    // API has been replaced by a new download cycle.)
    let retry_id = create_download_in_state(
        &storage,
        &expected_hash,
        trusted.len() as u64,
        "corrupt-peer",
        "verifying",
    );
    let retry_temp = dir.path().join("corrupt.bin.retry.part");
    std::fs::write(&retry_temp, trusted).expect("write trusted retry");

    verify_install_and_complete(
        &storage,
        retry_id,
        &retry_temp,
        &destination,
        trusted.len() as u64,
        &expected_hash,
    )
    .expect("trusted retry should install");

    assert_eq!(
        storage.get_download(retry_id).unwrap().unwrap().state,
        "complete"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), trusted);
    assert!(!retry_temp.exists());
}

#[test]
fn incorrect_expected_hash_is_failed_and_retry_does_not_reuse_partial() {
    let storage = Storage::memory().expect("storage");
    let dir = TempDir::new().expect("temp directory");
    let destination = dir.path().join("wrong-hash.bin");
    let temp_path = dir.path().join("wrong-hash.bin.part");
    let trusted = b"content whose hash is checked";
    let expected_hash = blake3::hash(trusted).to_hex().to_string();
    let incorrect_hash = blake3::hash(b"different content").to_hex().to_string();
    assert_ne!(expected_hash, incorrect_hash);

    let id = create_download_in_state(
        &storage,
        &expected_hash,
        trusted.len() as u64,
        "wrong-hash-peer",
        "verifying",
    );
    std::fs::write(&temp_path, trusted).expect("write downloaded bytes");

    // Supplying a deliberately incorrect expected hash must fail before the
    // atomic rename. The partial remains available only for the worker to
    // quarantine/remove; it is never exposed as the destination.
    let error = verify_install_and_complete(
        &storage,
        id,
        &temp_path,
        &destination,
        trusted.len() as u64,
        &incorrect_hash,
    )
    .expect_err("incorrect expected hash must be rejected");
    assert!(error.to_string().contains("content hash mismatch"));
    assert!(temp_path.exists());
    assert!(!destination.exists());
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "verifying"
    );

    // Model the transfer worker's failure path: discard the rejected partial
    // and durably mark the attempt failed and retryable.
    std::fs::remove_file(&temp_path).expect("remove rejected partial");
    storage
        .fail_download(id, &error.to_string(), Some(1234))
        .expect("record failed download");
    let failed = storage.get_download(id).unwrap().unwrap();
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.next_retry_at_ms, Some(1234));
    assert!(!temp_path.exists());
    assert!(!destination.exists());

    // A retry uses a fresh temporary path and trusted bytes. It must not be
    // able to complete by reusing the rejected partial.
    let retry_id = create_download_in_state(
        &storage,
        &expected_hash,
        trusted.len() as u64,
        "wrong-hash-peer",
        "verifying",
    );
    let retry_temp = dir.path().join("wrong-hash.bin.retry.part");
    std::fs::write(&retry_temp, trusted).expect("write trusted retry");
    verify_install_and_complete(
        &storage,
        retry_id,
        &retry_temp,
        &destination,
        trusted.len() as u64,
        &expected_hash,
    )
    .expect("trusted retry should install");

    assert_eq!(
        storage.get_download(retry_id).unwrap().unwrap().state,
        "complete"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), trusted);
    assert!(!retry_temp.exists());
}
