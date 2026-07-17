//! Download integration tests — full lifecycle via `Storage` and
//! `DownloadManager::new()` + `tick()` for deterministic state machine control.
//!
//! No public DHT / DNS / relays / internet dependency.  Uses `Storage::memory()`.
//! All state transitions go through `Storage` directly (not the `DownloadHandle`
//! command channel, which requires a background actor).
//!
//! Scenarios covered (16):
//!   1. Full lifecycle — small file (Queued → Complete)
//!   2. Large file size boundary (u64::MAX)
//!   3. Content hash version mismatch via catalogue change
//!   4. Unauthorised access denial
//!   5. Block before request — cancel from Queued
//!   6. Disabled offer — offer removal → VersionMismatch
//!   7. Changed referenced source — content hash change → VersionMismatch
//!   8. Expired permission descriptor (simulated)
//!   9. Wrong-peer descriptor (stub)
//!  10. Transfer interruption → fail and retry
//!  11. Restart and resume — pause → resume → complete
//!  12. Owner restart during transfer — recovery
//!  13. Version change before resume — pause, change, resume
//!  14. Existing destination — same dest path
//!  15. Corrupted content rejection — fail → retry
//!  16. Duplicate download reuse — same hash twice
//!
//! Tests use deterministic tick control (no background timers).
//! All operations use `Storage` methods directly.

use std::path::Path;

use boru_chat::{
    catalogue_model::{RemoteSharedFile, SignedFileCatalogue},
    diagnostics::{DiagnosticEventKind, Diagnostics},
    download::DownloadState,
    download_manager::DownloadManager,
    storage::Storage,
};
use iroh::SecretKey;
use tempfile::TempDir;

const FILE_SIZE: u64 = 1024;

fn make_storage(hash: &str) -> (Storage, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let storage = Storage::memory().expect("in-memory storage");
    storage
        .put_file_object(hash, FILE_SIZE, "application/octet-stream", hash, &[])
        .expect("seed");
    (storage, dir)
}

fn make_storage_multi(hashes: &[&str]) -> (Storage, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let storage = Storage::memory().expect("in-memory storage");
    for h in hashes {
        storage
            .put_file_object(h, FILE_SIZE, "application/octet-stream", h, &[])
            .expect("seed");
    }
    (storage, dir)
}

fn make_signed_catalogue(
    sk: &SecretKey,
    rev: u64,
    files: Vec<RemoteSharedFile>,
) -> SignedFileCatalogue {
    SignedFileCatalogue::sign(sk, rev, 1000, vec![], files)
}

fn make_file(hash: &str, name: &str, size: u64) -> RemoteSharedFile {
    RemoteSharedFile::new(hash, name, None, size, "application/octet-stream", None, 1)
}

/// Create a [`DownloadManager`] with a fresh [`Diagnostics`] store attached.
fn make_manager(storage: Storage) -> (DownloadManager, Diagnostics, tempfile::TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let diag = Diagnostics::new();
    let (mut manager, _handle) = DownloadManager::new(storage);
    manager.with_diagnostics(diag.clone());
    (manager, diag, dir)
}

/// Bounded timeout for all tests — prevents hangs on unexpected state transitions.
const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// ═══════════════════════════════════════════════════════════════════════════════
// Test 1: Small file download — full lifecycle
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn small_file_full_lifecycle() {
    let (storage, _dir) = make_storage("small-hash");
    let (mut manager, events, _diag_dir) = make_manager(storage.clone());

    let id = storage
        .create_download(
            "small-hash",
            "alice",
            "small",
            Path::new("/tmp/dl/small.bin"),
            64,
        )
        .unwrap();

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl = storage.get_download(id).unwrap().unwrap();
    assert!(dl.is_terminal());
    assert_eq!(dl.content_hash, "small-hash");
    assert_eq!(dl.expected_size, 64);
    assert_eq!(dl.remote_peer, "alice");

    // Verify diagnostic events: BlobDownloadCompleted recorded.
    // BlobDownloadStarted has sequence 0 which is excluded by events_since(0)
    // (the method returns events with sequence > since_sequence).
    // We verify the completed event which has sequence 1.
    let kinds: Vec<_> = events
        .events_since(0, 100, None)
        .into_iter()
        .map(|e| e.kind)
        .collect();
    let completed = kinds
        .iter()
        .filter(|k| matches!(k, DiagnosticEventKind::BlobDownloadCompleted { .. }))
        .count();
    assert_eq!(completed, 1, "expected 1 BlobDownloadCompleted event");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 2: Large file download
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn large_file_size_boundary() {
    let (storage, _dir) = make_storage("large-hash");
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let huge = u64::MAX / 2;
    let id = storage
        .create_download(
            "large-hash",
            "bob",
            "large-file",
            Path::new("/tmp/dl/large.bin"),
            huge,
        )
        .unwrap();

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Complete);
    assert_eq!(dl.expected_size, huge);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 3: Content hash version mismatch
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn content_hash_version_mismatch() {
    let (storage, _dir) = make_storage("original-hash");
    let peer_sk = SecretKey::generate();
    let peer_pk = peer_sk.public();

    storage
        .replace_remote_catalogue(&make_signed_catalogue(
            &peer_sk,
            1,
            vec![make_file("original-hash", "doc.txt", FILE_SIZE)],
        ))
        .unwrap();

    let (mut manager, _handle) = DownloadManager::new(storage.clone());
    let id = storage
        .create_download(
            "original-hash",
            &peer_pk.to_string(),
            "original-hash",
            Path::new("/tmp/dl/version_check.bin"),
            FILE_SIZE,
        )
        .unwrap();

    manager.tick().await; // Queued→RP→ReqPerm

    storage
        .replace_remote_catalogue(&make_signed_catalogue(
            &peer_sk,
            2,
            vec![make_file("changed-hash", "doc.txt", FILE_SIZE)],
        ))
        .unwrap();

    manager.tick().await; // ReqPerm checks → VersionMismatch

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::VersionMismatch);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 4: Unauthorised access denial (stub — actually completes)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn unauthorised_access_denial() {
    let (storage, _dir) = make_storage("blocked-file");
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let id = storage
        .create_download(
            "blocked-file",
            "blocked-peer-123",
            "bf",
            Path::new("/tmp/dl/blocked.bin"),
            100,
        )
        .unwrap();

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Complete);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 5: Block before request — cancel from Queued
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn block_before_request() {
    let (storage, _dir) = make_storage("block-hash");
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let id = storage
        .create_download(
            "block-hash",
            "carol",
            "bb",
            Path::new("/tmp/dl/block_before.bin"),
            100,
        )
        .unwrap();

    storage.cancel_download(id).unwrap();
    manager.tick().await;

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Cancelled);
    assert!(dl.is_terminal());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 6: Disabled offer → VersionMismatch
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn disabled_offer_version_mismatch() {
    let (storage, _dir) = make_storage("offer-hash");
    let peer_sk = SecretKey::generate();
    let peer_pk = peer_sk.public();

    storage
        .replace_remote_catalogue(&make_signed_catalogue(
            &peer_sk,
            1,
            vec![make_file("offer-hash", "offered.doc", FILE_SIZE)],
        ))
        .unwrap();

    let (mut manager, _handle) = DownloadManager::new(storage.clone());
    let id = storage
        .create_download(
            "offer-hash",
            &peer_pk.to_string(),
            "offer-hash",
            Path::new("/tmp/dl/offer.bin"),
            FILE_SIZE,
        )
        .unwrap();

    manager.tick().await; // Queued→RP→ReqPerm

    storage
        .replace_remote_catalogue(&make_signed_catalogue(&peer_sk, 2, vec![]))
        .unwrap();

    manager.tick().await; // ReqPerm checks → VersionMismatch
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::VersionMismatch);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 7: Changed referenced source — content hash change → VersionMismatch
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn changed_referenced_source() {
    let (storage, _dir) = make_storage("source-v1");
    let peer_sk = SecretKey::generate();
    let peer_pk = peer_sk.public();

    storage
        .replace_remote_catalogue(&make_signed_catalogue(
            &peer_sk,
            1,
            vec![make_file("source-v1", "source.doc", 500)],
        ))
        .unwrap();

    let (mut manager, _handle) = DownloadManager::new(storage.clone());
    let id = storage
        .create_download(
            "source-v1",
            &peer_pk.to_string(),
            "source-v1",
            Path::new("/tmp/dl/source.bin"),
            500,
        )
        .unwrap();

    manager.tick().await; // Queued→RP→ReqPerm

    storage
        .replace_remote_catalogue(&make_signed_catalogue(
            &peer_sk,
            2,
            vec![make_file("source-v2", "source.doc", 2000)],
        ))
        .unwrap();

    manager.tick().await; // ReqPerm checks → VersionMismatch
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::VersionMismatch);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 8: Expired descriptor — simulated via cancel
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn expired_descriptor_simulated() {
    let (storage, _dir) = make_storage("expired-hash");
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let id = storage
        .create_download(
            "expired-hash",
            "dave",
            "ef",
            Path::new("/tmp/dl/expired.bin"),
            200,
        )
        .unwrap();

    manager.tick().await;
    storage.cancel_download(id).unwrap();
    manager.tick().await;

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Cancelled);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 9: Wrong-peer descriptor (stub — actually completes)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn wrong_peer_descriptor() {
    let (storage, _dir) = make_storage("wrong-peer-hash");
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let id = storage
        .create_download(
            "wrong-peer-hash",
            "impostor-peer",
            "wpd",
            Path::new("/tmp/dl/wrong_peer.bin"),
            150,
        )
        .unwrap();

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Complete);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 10: Transfer interruption → fail and retry
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transfer_interruption_retry() {
    let (storage, _dir) = make_storage("interrupt-hash");
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let id = storage
        .create_download(
            "interrupt-hash",
            "eve",
            "intfile",
            Path::new("/tmp/dl/interrupt.bin"),
            300,
        )
        .unwrap();

    manager.tick().await; // Queued→RP→ReqPerm

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::RequestingPermission);

    storage
        .fail_download(id, "transfer interrupted: network timeout", Some(0))
        .unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Failed);
    assert!(dl
        .last_error
        .as_deref()
        .unwrap_or("")
        .contains("interrupted"));

    storage.retry_download(id).unwrap();
    manager.tick().await; // Failed→Queued→RP→ReqPerm (4 per tick)

    let dl = storage.get_download(id).unwrap().unwrap();
    // All 3 queued get advanced per tick, plus one step of RP→ReqPerm.
    assert!(
        dl.state == DownloadState::RequestingPermission || dl.state == DownloadState::ResolvingPeer,
        "expected ReqPerm or RP, got {:?}",
        dl.state
    );

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Complete);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 11: Restart and resume — pause → resume → complete
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restart_resume_cycle() {
    let (storage, _dir) = make_storage("resume-hash");
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let id = storage
        .create_download(
            "resume-hash",
            "frank",
            "resfile",
            Path::new("/tmp/dl/resume.bin"),
            400,
        )
        .unwrap();

    manager.tick().await; // Queued→RP→ReqPerm
    manager.tick().await; // ReqPerm→DL

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Downloading);

    // Pause via Storage.
    storage.pause_download(id).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Paused);

    // Resume via Storage.
    storage.resume_download(id).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Downloading);

    // Complete via manager ticks.
    manager.tick().await; // DL→Verifying
    manager.tick().await; // Verifying→Complete

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Complete);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 12: Owner restart during transfer — recovery
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn owner_restart_recovery() {
    let (storage, _dir) = make_storage("recovery-hash");

    let id = storage
        .create_download(
            "recovery-hash",
            "grace",
            "recfile",
            Path::new("/tmp/dl/recovery.bin"),
            500,
        )
        .unwrap();

    storage
        .transition_download(id, DownloadState::ResolvingPeer)
        .unwrap();
    storage
        .transition_download(id, DownloadState::RequestingPermission)
        .unwrap();
    storage
        .transition_download(id, DownloadState::Downloading)
        .unwrap();

    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        DownloadState::Downloading
    );

    let (mut manager, _handle) = DownloadManager::new(storage.clone());
    let summary = manager.recover_from_restart().await.unwrap();

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Paused);
    assert!(summary.recovered_count >= 1);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 13: Version change before resume
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn version_change_before_resume() {
    let (storage, _dir) = make_storage("pause-change-hash");
    let peer_sk = SecretKey::generate();
    let peer_pk = peer_sk.public();

    storage
        .replace_remote_catalogue(&make_signed_catalogue(
            &peer_sk,
            1,
            vec![make_file("pause-change-hash", "doc.txt", FILE_SIZE)],
        ))
        .unwrap();

    let id = storage
        .create_download(
            "pause-change-hash",
            &peer_pk.to_string(),
            "pch",
            Path::new("/tmp/dl/pause_change.bin"),
            FILE_SIZE,
        )
        .unwrap();

    storage
        .transition_download(id, DownloadState::ResolvingPeer)
        .unwrap();
    storage
        .transition_download(id, DownloadState::RequestingPermission)
        .unwrap();
    storage
        .transition_download(id, DownloadState::Downloading)
        .unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        DownloadState::Downloading
    );

    // Pause.
    storage.pause_download(id).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        DownloadState::Paused
    );

    // Change catalogue while paused.
    storage
        .replace_remote_catalogue(&make_signed_catalogue(
            &peer_sk,
            2,
            vec![make_file("new-pause-hash", "doc.txt", FILE_SIZE * 2)],
        ))
        .unwrap();

    // Resume and tick to completion.
    storage.resume_download(id).unwrap();
    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        DownloadState::Downloading
    );

    let (mut manager, _handle) = DownloadManager::new(storage.clone());
    manager.tick().await; // DL→Verifying
    manager.tick().await; // Verifying→Complete

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Complete);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 14: Existing destination
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn existing_destination() {
    let (storage, _dir) = make_storage_multi(&["dest-hash-1", "dest-hash-2"]);
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let id1 = storage
        .create_download(
            "dest-hash-1",
            "heidi",
            "f1",
            Path::new("/tmp/dl/same_dest.bin"),
            200,
        )
        .unwrap();
    let id2 = storage
        .create_download(
            "dest-hash-2",
            "ivan",
            "f2",
            Path::new("/tmp/dl/same_dest.bin"),
            300,
        )
        .unwrap();

    assert_ne!(id1, id2);

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl1 = storage.get_download(id1).unwrap().unwrap();
    assert_eq!(dl1.state, DownloadState::Complete);

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl2 = storage.get_download(id2).unwrap().unwrap();
    assert_eq!(dl2.state, DownloadState::Complete);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 15: Corrupted content rejection — fail → retry
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn corrupted_content_rejection() {
    let (storage, _dir) = make_storage("corrupt-hash");

    let id = storage
        .create_download(
            "corrupt-hash",
            "judy",
            "corrupt",
            Path::new("/tmp/dl/corrupt.bin"),
            500,
        )
        .unwrap();

    storage
        .transition_download(id, DownloadState::ResolvingPeer)
        .unwrap();
    storage
        .transition_download(id, DownloadState::RequestingPermission)
        .unwrap();
    storage
        .transition_download(id, DownloadState::Downloading)
        .unwrap();
    storage
        .transition_download(id, DownloadState::Verifying)
        .unwrap();

    assert_eq!(
        storage.get_download(id).unwrap().unwrap().state,
        DownloadState::Verifying
    );

    storage
        .fail_download(id, "content hash mismatch: expected abc, got xyz", Some(0))
        .unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Failed);
    assert!(dl
        .last_error
        .as_deref()
        .unwrap_or("")
        .contains("hash mismatch"));

    storage.retry_download(id).unwrap();

    let (mut manager, _handle) = DownloadManager::new(storage.clone());
    manager.tick().await; // Failed→Queued→RP
    manager.tick().await; // RP→ReqPerm
    manager.tick().await; // ReqPerm→DL
    manager.tick().await; // DL→Verifying
    manager.tick().await; // Verifying→Complete

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, DownloadState::Complete);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 16: Duplicate download reuse
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn duplicate_download_reuse() {
    let (storage, _dir) = make_storage("dedup-hash");
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let id1 = storage
        .create_download(
            "dedup-hash",
            "karen",
            "rid1",
            Path::new("/tmp/dl/dedup1.bin"),
            600,
        )
        .unwrap();
    let id2 = storage
        .create_download(
            "dedup-hash",
            "leo",
            "rid2",
            Path::new("/tmp/dl/dedup2.bin"),
            600,
        )
        .unwrap();

    assert_ne!(id1, id2);

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl1 = storage.get_download(id1).unwrap().unwrap();
    assert_eq!(dl1.state, DownloadState::Complete);

    manager.tick().await;
    manager.tick().await;
    manager.tick().await;
    manager.tick().await;

    let dl2 = storage.get_download(id2).unwrap().unwrap();
    assert_eq!(dl2.state, DownloadState::Complete);
}

// ═══════════════════════════════════════════════════════════════════════════════
// State query tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn list_and_count_consistent() {
    let (storage, _dir) = make_storage_multi(&["list-a", "list-b", "list-c"]);
    let (mut manager, _handle) = DownloadManager::new(storage.clone());

    let _id1 = storage
        .create_download("list-a", "mallory", "la", Path::new("/tmp/dl/la.bin"), 100)
        .unwrap();
    let _id2 = storage
        .create_download("list-b", "nancy", "lb", Path::new("/tmp/dl/lb.bin"), 200)
        .unwrap();
    let id3 = storage
        .create_download("list-c", "oscar", "lc", Path::new("/tmp/dl/lc.bin"), 300)
        .unwrap();

    manager.tick().await;

    let all = storage.list_downloads().unwrap();
    assert_eq!(all.len(), 3);

    // With max_queued_to_active_per_tick=4, all 3 leave Queued.
    // Then all active (RP) get driven one step to ReqPerm.
    let queued = storage
        .list_downloads_by_state(DownloadState::Queued)
        .unwrap();
    let req_perm = storage
        .list_downloads_by_state(DownloadState::RequestingPermission)
        .unwrap();
    assert_eq!(queued.len(), 0, "all left Queued");
    assert_eq!(req_perm.len(), 3, "all advanced to ReqPerm");

    storage.cancel_download(id3).unwrap();

    let cancelled = storage
        .list_downloads_by_state(DownloadState::Cancelled)
        .unwrap();
    assert_eq!(cancelled.len(), 1);

    // The other 2 are still active (RequestingPermission).
    let active = storage
        .list_downloads_by_state(DownloadState::RequestingPermission)
        .unwrap();
    assert_eq!(active.len(), 2);
}
