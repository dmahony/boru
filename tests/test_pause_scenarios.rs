//! End-to-end pause scenario tests — DownloadManager-level orchestration.
//!
//! These tests verify the full pause lifecycle:
//!   - pause during resolution, permission request, and active transfer
//!   - repeated pause (pause–resume–pause cycles)
//!   - cancellation flag signalling
//!   - temporary file cleanup
//!   - state persistence and storage integrity

use std::sync::atomic::Ordering;

use boru_chat::download::DownloadState;
use boru_chat::download_manager::DownloadManager;
use boru_chat::storage::Storage;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Create a download directly in the requested state.
fn create_download_in_state(
    storage: &Storage,
    hash: &str,
    total_bytes: u64,
    state: DownloadState,
) -> i64 {
    storage
        .put_file_object(
            hash,
            total_bytes,
            "application/octet-stream",
            "file.bin",
            b"",
        )
        .unwrap();
    let id = storage
        .create_download(hash, "test-peer", total_bytes)
        .unwrap();
    if state != DownloadState::Queued {
        storage
            .update_download_progress(id, 0, state.as_str())
            .unwrap();
    }
    id
}

/// Set up a temp file on disk and register it as the download's temp path.
fn setup_temp_file(dir: &tempfile::TempDir, download_id: i64, storage: &Storage, contents: &[u8]) {
    let temp = dir.path().join(format!("download_{download_id}.part"));
    std::fs::write(&temp, contents).unwrap();
    let dest = dir.path().join(format!("download_{download_id}.bin"));
    storage
        .set_download_paths(download_id, &temp, &dest)
        .unwrap();
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Pause a download that is currently resolving the peer.
#[test]
fn pause_during_resolution() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "pause-resolve-hash",
        4096,
        DownloadState::ResolvingPeer,
    );

    // Simulate an active in-flight worker by registering a cancel flag
    // (the real tick() path does this when a queued download is claimed).
    manager.register_cancel_flag(id);

    // Pause via the service-level orchestrator.
    manager.pause_download(id).unwrap();

    // State is persisted.
    let paused = storage.get_download(id).unwrap().unwrap();
    assert_eq!(paused.state, "paused");
    assert_eq!(paused.content_hash, "pause-resolve-hash");
    assert_eq!(paused.remote_peer, "test-peer");
    assert_eq!(paused.total_bytes, 4096);

    // Cancel flag is set so any in-flight worker stops.
    assert!(
        manager.cancel_flag(id).load(Ordering::Relaxed),
        "cancel flag must be set after pause"
    );
}

/// Pause a download that is waiting for peer permission.
#[test]
fn pause_during_permission_request() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "pause-perm-hash",
        2048,
        DownloadState::RequestingPermission,
    );

    // Advance partial bytes to confirm they survive the pause.
    storage
        .update_download_progress(id, 512, "requesting_permission")
        .unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().bytes_downloaded,
        512
    );

    // Simulate an active in-flight worker.
    manager.register_cancel_flag(id);

    manager.pause_download(id).unwrap();

    let paused = storage.get_download(id).unwrap().unwrap();
    assert_eq!(paused.state, "paused");
    assert_eq!(paused.bytes_downloaded, 512);
    assert_eq!(paused.total_bytes, 2048);

    assert!(
        manager.cancel_flag(id).load(Ordering::Relaxed),
        "cancel flag must be set after pause"
    );
}

/// Pause a download mid-transfer — verify the temp file is cleaned up
/// and the DB path is cleared.
#[test]
fn pause_during_transfer_cleans_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "pause-transfer-hash",
        1000,
        DownloadState::Downloading,
    );

    // Register a temp file on disk, as if transfer has begun.
    setup_temp_file(&dir, id, &storage, b"some partial content");

    let temp_path = storage
        .get_download_temp_path(id)
        .unwrap()
        .expect("temp path should exist");
    assert!(
        std::path::Path::new(&temp_path).is_file(),
        "temp file should exist on disk"
    );

    // Simulate an active in-flight worker.
    manager.register_cancel_flag(id);

    manager.pause_download(id).unwrap();

    // Temp file must be gone after pause.
    assert!(
        !std::path::Path::new(&temp_path).is_file(),
        "temp file must be removed on pause"
    );

    // DB path must be cleared.
    assert!(
        storage.get_download_temp_path(id).unwrap().is_none(),
        "temp path must be cleared from DB on pause"
    );

    // State is paused.
    let paused = storage.get_download(id).unwrap().unwrap();
    assert_eq!(paused.state, "paused");

    assert!(
        manager.cancel_flag(id).load(Ordering::Relaxed),
        "cancel flag must be set after pause"
    );
}

/// Multiple pause–resume cycles should not corrupt state or lose progress.
#[test]
fn repeated_pause_resume_cycles() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "repeated-pause-hash",
        5000,
        DownloadState::ResolvingPeer,
    );

    // Simulate an active in-flight worker.
    manager.register_cancel_flag(id);

    // ── Cycle 1: pause → resume → pause ──────────────────────────────
    manager.pause_download(id).unwrap();
    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");

    storage.resume_download(id).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "resolving_peer"
    );

    manager.pause_download(id).unwrap();
    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");

    // ── Cycle 2: another pause → resume → pause ──────────────────────
    storage.resume_download(id).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "resolving_peer"
    );

    manager.pause_download(id).unwrap();
    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");

    // Bytes should be intact.
    assert_eq!(storage.get_download(id).unwrap().unwrap().total_bytes, 5000);
    // Content hash should be intact.
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().content_hash,
        "repeated-pause-hash"
    );
    // Peer should be intact.
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().remote_peer,
        "test-peer"
    );

    // Cancel flag should be set after final pause.
    assert!(
        manager.cancel_flag(id).load(Ordering::Relaxed),
        "cancel flag must be set after final pause"
    );
}

/// Pausing an already-paused download is idempotent and does not reset
/// any fields.
#[test]
fn pause_already_paused_is_idempotent() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "idempotent-pause-hash",
        3000,
        DownloadState::Downloading,
    );
    storage
        .update_download_progress(id, 1500, "downloading")
        .unwrap();

    // Simulate an active in-flight worker.
    manager.register_cancel_flag(id);

    manager.pause_download(id).unwrap();
    let after_first = storage.get_download(id).unwrap().unwrap();
    assert_eq!(after_first.state, "paused");
    assert_eq!(after_first.bytes_downloaded, 1500);

    // Second pause — should be a no-op.
    manager.pause_download(id).unwrap();
    let after_second = storage.get_download(id).unwrap().unwrap();
    assert_eq!(after_second.state, "paused");
    // Bytes must not have been reset.
    assert_eq!(after_second.bytes_downloaded, 1500);
    // Cancel flag already set, should remain set.
    assert!(
        manager.cancel_flag(id).load(Ordering::Relaxed),
        "cancel flag must remain set after idempotent pause"
    );
}

/// Pause must reject terminal states (complete, failed, cancelled, version_mismatch).
#[test]
fn pause_rejects_terminal_states() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    for terminal in [
        DownloadState::Complete,
        DownloadState::Failed,
        DownloadState::Cancelled,
        DownloadState::VersionMismatch,
    ] {
        let hash = format!("terminal-pause-{terminal:?}");
        let id = create_download_in_state(&storage, &hash, 100, terminal);
        assert!(
            manager.pause_download(id).is_err(),
            "pause must reject terminal state {terminal:?}"
        );
        // State must not have changed.
        let dl = storage.get_download(id).unwrap().unwrap();
        assert_eq!(dl.state, terminal.as_str());
    }
}

/// Pause on a non-existent download must fail.
#[test]
fn pause_rejects_unknown_download() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());
    assert!(
        manager.pause_download(i64::MAX).is_err(),
        "pause must reject non-existent download"
    );
}

/// After pause → resume → accept → downloading, the cancel flag should
/// still be reachable (not accidentally removed by the pause path).
#[test]
fn cancel_flag_survives_resume() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "flag-survive-hash",
        100,
        DownloadState::ResolvingPeer,
    );

    // Register a cancel flag, as an active worker would have.
    manager.register_cancel_flag(id);

    manager.pause_download(id).unwrap();
    assert!(manager.cancel_flag(id).load(Ordering::Relaxed));

    storage.resume_download(id).unwrap();
    // Cancel flag should still exist — DownloadManager::pause_download does
    // NOT remove it (only cancel_download does remove_cancel_flag).
    // After resume the flag is still there, still set to true.
    // The caller or a new worker can reset it with a new register_cancel_flag.
    assert!(
        manager.cancel_flag(id).load(Ordering::Relaxed),
        "cancel flag should survive resume (only removed on cancel, not pause)"
    );
}

/// Concurrent state checks: a paused download cannot be advanced by
/// stale progress updates.
#[test]
fn paused_download_rejects_stale_progress() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "stale-progress-hash",
        500,
        DownloadState::Downloading,
    );
    storage
        .update_download_progress(id, 200, "downloading")
        .unwrap();

    manager.pause_download(id).unwrap();

    // Outside worker trying to report progress after pause.
    let result = storage.update_download_progress(id, 400, "downloading");
    assert!(
        result.is_err(),
        "stale progress after pause must be rejected"
    );

    // Bytes must remain at what was reported before pause.
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().bytes_downloaded,
        200
    );
}

/// Pause does not corrupt other rows in the downloads table.
#[test]
fn pause_does_not_affect_other_downloads() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let a = create_download_in_state(&storage, "sibling-a", 100, DownloadState::Downloading);
    let b = create_download_in_state(&storage, "sibling-b", 200, DownloadState::ResolvingPeer);
    let c = create_download_in_state(&storage, "sibling-c", 300, DownloadState::Complete);

    manager.pause_download(a).unwrap();

    assert_eq!(storage.get_download(a).unwrap().unwrap().state, "paused");
    assert_eq!(
        storage.get_download(b).unwrap().unwrap().state,
        "resolving_peer"
    );
    assert_eq!(storage.get_download(c).unwrap().unwrap().state, "complete");
}

/// Temp file cleanup does not affect downloads without temp files.
#[test]
fn pause_without_temp_file_is_harmless() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(&storage, "no-temp-hash", 500, DownloadState::Downloading);

    // Verify no temp path is registered.
    assert!(storage.get_download_temp_path(id).unwrap().is_none());

    // Pause should succeed even though there's nothing to clean up.
    manager.pause_download(id).unwrap();

    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");
}

/// Verify that a pause–resume–accept complete cycle works correctly.
#[test]
fn pause_resume_accept_completes_download() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let hash = "pause-resume-accept-hash";
    let id = create_download_in_state(&storage, hash, 250, DownloadState::Downloading);

    // Pause.
    manager.pause_download(id).unwrap();
    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");

    // Resume → resolving_peer.
    storage.resume_download(id).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "resolving_peer"
    );

    // Accept the descriptor → downloading.
    storage.accept_resumed_descriptor(id, hash, 250).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        "downloading"
    );

    // Complete the download (simulate a full file being present).
    storage
        .put_file_object(hash, 250, "application/octet-stream", "file.bin", b"")
        .unwrap();
    storage.complete_download(id, 250).unwrap();

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "complete");
    assert_eq!(dl.total_bytes, 250);
}

/// Rapid pause–resume cycles should not leak cancel flags or corrupt state.
#[test]
fn rapid_pause_resume_cycles() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "rapid-cycles-hash",
        1000,
        DownloadState::ResolvingPeer,
    );

    for cycle in 0..10 {
        manager.pause_download(id).unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            "paused",
            "must be paused after cycle {cycle}"
        );

        storage.resume_download(id).unwrap();
        assert_eq!(
            storage.get_download(id).unwrap().unwrap().state,
            "resolving_peer",
            "must be resolving_peer after resume cycle {cycle}"
        );
    }

    // Final pause.
    manager.pause_download(id).unwrap();
    let paused = storage.get_download(id).unwrap().unwrap();
    assert_eq!(paused.state, "paused");
    assert_eq!(paused.content_hash, "rapid-cycles-hash");
    assert_eq!(paused.total_bytes, 1000);
    assert_eq!(paused.remote_peer, "test-peer");
}

/// Pause in the `verifying` state — should work like other active states.
#[test]
fn pause_during_verification() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(&storage, "pause-verify-hash", 800, DownloadState::Verifying);

    // Simulate an active in-flight worker.
    manager.register_cancel_flag(id);

    manager.pause_download(id).unwrap();

    let paused = storage.get_download(id).unwrap().unwrap();
    assert_eq!(paused.state, "paused");
    assert_eq!(paused.content_hash, "pause-verify-hash");
    assert_eq!(paused.remote_peer, "test-peer");
    assert_eq!(paused.total_bytes, 800);

    assert!(
        manager.cancel_flag(id).load(Ordering::Relaxed),
        "cancel flag must be set after pause during verification"
    );
}

/// Pause then cancel should work correctly.
#[test]
fn pause_then_cancel() {
    let storage = Storage::memory().unwrap();
    let manager = DownloadManager::new(storage.clone());

    let id = create_download_in_state(
        &storage,
        "pause-then-cancel-hash",
        500,
        DownloadState::ResolvingPeer,
    );

    manager.pause_download(id).unwrap();
    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");

    manager.cancel_download(id).unwrap();

    let cancelled = storage.get_download(id).unwrap().unwrap();
    assert_eq!(cancelled.state, "cancelled");
    // Cancel flag should be removed after cancel_download.
    // cancel_flag() will create a fresh one (unset) via or_insert_with.
    assert!(
        !manager.cancel_flag(id).load(Ordering::Relaxed),
        "cancel flag must be fresh (unset) after cancel removes and re-creates it"
    );
}
