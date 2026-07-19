//! Integration tests for all resume scenarios.
//!
//! Each test exercises the full resume lifecycle through
//! [`DownloadManager`] and [`Storage`] without real network calls —
//! decisions are made via [`evaluate_resume_response`] and applied
//! via [`execute_resume_decision`].
//!
//! Scenarios covered:
//!   1. Resume same session (identical descriptor)
//!   2. Resume after app restart (state persisted on disk)
//!   3. Expired prior descriptor (triggers fresh permission cycle)
//!   4. Version changed (triggers restart with preserved hash)
//!   5. Access revoked (PermissionDenied -> fail gracefully)
//!
//! Each test verifies that no silent download of changed content occurs.

use std::time::{SystemTime, UNIX_EPOCH};

use boru_chat::{
    download_limits::DownloadLimitsConfig,
    download_manager::{
        evaluate_resume_response, DownloadManager, ResumeDecision, ResumeOutcome,
        ResumeRestartReason,
    },
    file_access_protocol::{sign_download_descriptor, BlobFormat, FileAccessResponse},
    storage::Storage,
};
use iroh::SecretKey;

// ---- Helpers ---------------------------------------------------------------

/// Create a hash string (64 hex chars) from a 2-char repeating pattern.
fn make_hash(pattern: &str) -> String {
    assert_eq!(pattern.len(), 2, "pattern must be exactly 2 chars");
    pattern.repeat(32)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Build a minimal DownloadManager backed by an in-memory DB,
/// returning both the manager and the storage handle.
fn make_manager() -> (DownloadManager, Storage) {
    let storage = Storage::memory().expect("in-memory storage");
    let mgr = DownloadManager::with_limits(storage.clone(), DownloadLimitsConfig::default());
    (mgr, storage)
}

/// Seed a file_object row so FK constraints are satisfied.
fn seed_file_object(storage: &Storage, hash: &str) {
    storage
        .put_file_object(hash, 4096, "application/octet-stream", "test.bin", b"")
        .unwrap();
}

/// Create a paused download with a pre-seeded file_object.
fn create_paused(storage: &Storage, hash: &str, peer: &str, bytes: u64) -> i64 {
    seed_file_object(storage, hash);
    let id = storage.create_download(hash, peer, bytes).unwrap();
    storage.pause_download(id).unwrap();
    id
}

fn hex_to_bytes(hex_str: &str) -> [u8; 32] {
    let raw = hex::decode(hex_str).expect("valid hex");
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&raw);
    arr
}

/// Build a minimal `FileAccessResponse::Granted` with the given
/// content hash and size.
fn granted_response(hash: &str, size: u64) -> FileAccessResponse {
    let sk = SecretKey::generate();
    let pk = sk.public();
    let now = now_ms();
    let desc = sign_download_descriptor(
        &sk,
        pk,
        "test-file-id".into(),
        hex_to_bytes(hash),
        size,
        BlobFormat::Raw,
        now,
        u64::MAX,
    );
    FileAccessResponse::Granted(Box::new(desc))
}

// ---- 1. Resume Same Session -----------------------------------------------

/// Happy path: pause -> resume -> Continue (hash unchanged) -> downloading -> complete.
#[test]
fn resume_same_session_identical_descriptor() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("aa");
    let id = create_paused(&storage, &hash, "peer-same", 4096);

    // Check we start paused.
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "paused");

    // Resume -> resolving_peer.
    storage.resume_download(id).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "resolving_peer");
    assert_eq!(dl.content_hash, hash);

    // Apply Continue decision.
    let outcome = _mgr
        .execute_resume_decision(id, &ResumeDecision::Continue, Some(&hash), Some(4096))
        .unwrap();
    assert_eq!(outcome, ResumeOutcome::Resumed);

    // State -> downloading, hash unchanged.
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "downloading");
    assert_eq!(dl.content_hash, hash, "no silent content change");
    assert_eq!(dl.total_bytes, 4096);

    // Complete the download.
    storage.complete_download(id, 4096).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "complete");
    assert_eq!(dl.content_hash, hash);
}

/// Continue with a changed hash must be rejected at the decision level.
#[test]
fn resume_rejects_changed_hash_in_same_session() {
    let (_mgr, storage) = make_manager();
    let original_hash = make_hash("bb");
    let changed_hash = make_hash("cc");
    let id = create_paused(&storage, &original_hash, "peer-changed", 2048);

    // Resume -> resolving_peer.
    storage.resume_download(id).unwrap();

    // evaluate_resume_response with a mismatched hash -> Restart(HashMismatch).
    let response = granted_response(&changed_hash, 2048);
    let decision = evaluate_resume_response(&response, &original_hash);
    assert!(matches!(
        decision,
        ResumeDecision::Restart {
            reason: ResumeRestartReason::HashMismatch { .. },
        }
    ));

    // Apply the decision -- should restart with the fresh hash.
    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    match outcome {
        ResumeOutcome::Restarted {
            new_download_id,
            content_hash,
        } => {
            assert_eq!(content_hash, changed_hash);
            // Original download must be cancelled (not silently downloaded).
            let old_dl = storage.get_download(id).unwrap().unwrap();
            assert_eq!(old_dl.state, "cancelled");
            // New download starts queued with the changed hash.
            let new_dl = storage.get_download(new_download_id).unwrap().unwrap();
            assert_eq!(new_dl.state, "queued");
            assert_eq!(new_dl.content_hash, changed_hash);
            assert_eq!(new_dl.remote_peer, "peer-changed");
        }
        other => panic!("expected Restarted, got {other:?}"),
    }
}

// ---- 2. Resume After App Restart ------------------------------------------

/// State survives an app restart (DB close->reopen).
#[test]
fn resume_after_app_restart() {
    let dir =
        std::env::temp_dir().join(format!("resume_restart_integration_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let hash = make_hash("dd");

    // ---- "First session" ----
    let storage = Storage::open(&dir).expect("open storage");
    seed_file_object(&storage, &hash);
    let id = storage
        .create_download(&hash, "peer-restart", 8192)
        .unwrap();
    storage.pause_download(id).unwrap();
    drop(storage);

    // ---- "Second session" ----
    let storage2 = Storage::open(&dir).expect("re-open storage");
    let mgr = DownloadManager::with_limits(storage2.clone(), DownloadLimitsConfig::default());

    // After re-open, the paused state must be preserved.
    let dl = storage2.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "paused", "pause state must survive restart");
    assert_eq!(dl.content_hash, hash, "content hash must survive restart");
    assert_eq!(dl.remote_peer, "peer-restart", "peer must survive restart");
    assert_eq!(dl.total_bytes, 8192, "total bytes must survive restart");

    // Resume -> resolving_peer.
    storage2.resume_download(id).unwrap();
    let dl = storage2.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "resolving_peer");

    // Apply Continue decision.
    let outcome = mgr
        .execute_resume_decision(id, &ResumeDecision::Continue, Some(&hash), Some(8192))
        .unwrap();
    assert_eq!(outcome, ResumeOutcome::Resumed);

    // State -> downloading.
    let dl = storage2.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "downloading");
    assert_eq!(dl.content_hash, hash, "no silent content change");

    // Complete the download.
    storage2.complete_download(id, 8192).unwrap();
    let dl = storage2.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "complete");

    let _ = std::fs::remove_dir_all(&dir);
}

/// RecoverFromRestart properly regresses active states to paused/queued
/// and they can be resumed in a new session.
#[test]
fn recover_from_restart_then_resume() {
    let dir = std::env::temp_dir().join(format!(
        "resume_recover_from_restart_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);

    let hash = make_hash("ee");

    // ---- First session: create a download in a non-terminal state ----
    {
        let storage = Storage::open(&dir).expect("open storage");
        seed_file_object(&storage, &hash);
        let id = storage.create_download(&hash, "peer-recover", 512).unwrap();
        // Simulate a crash mid-transfer by leaving state = downloading.
        storage
            .update_download_progress(id, 256, "downloading")
            .unwrap();
    }
    // ---- Second session: recovery ----
    // Note: Storage::open calls recover_downloads_from_restart() automatically,
    // so the downloading state is already regressed to paused at open time.
    let storage = Storage::open(&dir).expect("re-open storage");

    // Find the download by its content hash.
    let all_paused = storage.list_downloads_by_state("paused").unwrap();
    assert!(
        !all_paused.is_empty(),
        "recovery should pause interrupted downloads"
    );
    let id = all_paused
        .iter()
        .find(|d| d.content_hash == hash)
        .expect("download with our hash should exist")
        .id;

    let mgr = DownloadManager::with_limits(storage.clone(), DownloadLimitsConfig::default());

    // Now we can resume normally.
    storage.resume_download(id).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "resolving_peer");

    // Apply Continue -> downloading.
    let outcome = mgr
        .execute_resume_decision(id, &ResumeDecision::Continue, Some(&hash), Some(512))
        .unwrap();
    assert_eq!(outcome, ResumeOutcome::Resumed);

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "downloading");
    assert_eq!(
        dl.content_hash, hash,
        "content hash must not change silently"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- 3. Expired Prior Descriptor ------------------------------------------

/// Expired descriptor triggers a fresh permission cycle --
/// download stays paused until a fresh descriptor arrives.
#[test]
fn expired_prior_descriptor_triggers_fresh_permission() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("ff");
    let id = create_paused(&storage, &hash, "peer-expired", 1024);

    // Resume -> resolving_peer.
    storage.resume_download(id).unwrap();

    // Attempt to accept an expired descriptor.
    let expired_at_ms = 1_000; // way in the past
    let now_ms = 5_000; // well past expiry
    let err = storage
        .accept_resumed_descriptor_at(id, &hash, 1024, expired_at_ms, now_ms)
        .expect_err("expired descriptor must be rejected");

    assert!(
        err.to_string().contains("expired") || err.to_string().contains("stale"),
        "error message should mention expiry: {err}"
    );

    // Download stays paused after expired descriptor rejection.
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(
        dl.state, "paused",
        "download should remain paused after expired descriptor"
    );
    assert_eq!(dl.content_hash, hash, "hash preserved");
    assert!(
        dl.last_error.is_some(),
        "last_error should be set after expired descriptor"
    );

    // ---- Second attempt: fresh descriptor (not expired) ----
    storage.resume_download(id).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "resolving_peer");

    // Accept a fresh descriptor.
    storage.accept_resumed_descriptor(id, &hash, 1024).unwrap();
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(
        dl.state, "downloading",
        "fresh descriptor should advance to downloading"
    );
    assert_eq!(
        dl.content_hash, hash,
        "content hash must not change with fresh descriptor"
    );
}

// ---- 4. Version Changed ---------------------------------------------------

/// Version changed triggers restart with the same content hash.
/// The old download is cancelled, a new queued download is created.
#[test]
fn version_changed_triggers_restart() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("11");
    let id = create_paused(&storage, &hash, "peer-version", 2048);

    // Resume -> resolving_peer.
    storage.resume_download(id).unwrap();

    // Simulate version mismatch from the peer response.
    let response = FileAccessResponse::VersionMismatch {
        current_version: 42,
    };
    let decision = evaluate_resume_response(&response, &hash);

    assert!(matches!(
        decision,
        ResumeDecision::Restart {
            reason: ResumeRestartReason::VersionMismatch { .. },
        }
    ));

    // Apply the decision.
    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    match outcome {
        ResumeOutcome::Restarted {
            new_download_id,
            content_hash,
        } => {
            // Hash is preserved on version mismatch.
            assert_eq!(content_hash, hash);

            // Old download cancelled (not silently downloaded).
            let old_dl = storage.get_download(id).unwrap().unwrap();
            assert_eq!(old_dl.state, "cancelled");

            // New download queued with same hash.
            let new_dl = storage.get_download(new_download_id).unwrap().unwrap();
            assert_eq!(new_dl.state, "queued");
            assert_eq!(
                new_dl.content_hash, hash,
                "hash preserved on version mismatch"
            );
            assert_eq!(new_dl.remote_peer, "peer-version");
            assert_eq!(new_dl.total_bytes, 2048);
        }
        other => panic!("expected Restarted, got {other:?}"),
    }
}

/// Version mismatch evaluation returns correct decision.
#[test]
fn version_mismatch_evaluation_returns_restart() {
    let hash = make_hash("22");
    let response = FileAccessResponse::VersionMismatch {
        current_version: 99,
    };
    let decision = evaluate_resume_response(&response, &hash);

    assert_eq!(
        decision,
        ResumeDecision::Restart {
            reason: ResumeRestartReason::VersionMismatch {
                cached_version: 0,
                fresh_version: 99,
            },
        }
    );
}

// ---- 5. Access Revoked ----------------------------------------------------

/// PermissionDenied at resume -> download fails gracefully.
/// No new download is created, no silent transfer occurs.
#[test]
fn access_revoked_fails_gracefully() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("33");
    let id = create_paused(&storage, &hash, "peer-revoked", 512);

    // Must resume first so the download is in resolving_peer state.
    // fail_download explicitly rejects paused state.
    storage.resume_download(id).unwrap();

    // Apply PermissionDenied decision.
    let decision = ResumeDecision::Restart {
        reason: ResumeRestartReason::PermissionDenied,
    };
    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    assert_eq!(
        outcome,
        ResumeOutcome::Failed {
            error: "permission denied by remote peer on resume".to_string(),
        }
    );

    // State must be failed.
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "failed", "permission denied must fail download");
    assert_eq!(dl.content_hash, hash, "hash preserved even on failure");

    // No new download should have been created.
    let all_queued = storage.list_downloads_by_state("queued").unwrap();
    assert!(
        all_queued.is_empty(),
        "no silent queued download after permission denied"
    );
    let all_downloading = storage.list_downloads_by_state("downloading").unwrap();
    assert!(
        all_downloading.is_empty(),
        "no silent downloading after permission denied"
    );
}

/// PermissionDenied via evaluate_resume_response (from peer).
#[test]
fn access_revoked_via_peer_response() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("44");
    let id = create_paused(&storage, &hash, "peer-revoked-via", 256);

    storage.resume_download(id).unwrap();

    let response = FileAccessResponse::PermissionDenied;
    let decision = evaluate_resume_response(&response, &hash);

    assert_eq!(
        decision,
        ResumeDecision::Restart {
            reason: ResumeRestartReason::PermissionDenied,
        }
    );

    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    assert_eq!(
        outcome,
        ResumeOutcome::Failed {
            error: "permission denied by remote peer on resume".to_string(),
        }
    );

    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "failed");
    assert_eq!(dl.content_hash, hash);

    let all_downloading = storage.list_downloads_by_state("downloading").unwrap();
    assert!(all_downloading.is_empty());
}

/// NotFound from peer also maps to PermissionDenied.
#[test]
fn not_found_on_resume_fails_gracefully() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("55");
    let id = create_paused(&storage, &hash, "peer-notfound", 128);

    storage.resume_download(id).unwrap();

    let response = FileAccessResponse::NotFound;
    let decision = evaluate_resume_response(&response, &hash);

    assert_eq!(
        decision,
        ResumeDecision::Restart {
            reason: ResumeRestartReason::PermissionDenied,
        }
    );

    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    assert!(matches!(outcome, ResumeOutcome::Failed { .. }));
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "failed");
}

/// Disabled from peer also maps to PermissionDenied.
#[test]
fn disabled_on_resume_fails_gracefully() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("66");
    let id = create_paused(&storage, &hash, "peer-disabled", 64);

    storage.resume_download(id).unwrap();

    let response = FileAccessResponse::Disabled;
    let decision = evaluate_resume_response(&response, &hash);

    assert_eq!(
        decision,
        ResumeDecision::Restart {
            reason: ResumeRestartReason::PermissionDenied,
        }
    );

    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    assert!(matches!(outcome, ResumeOutcome::Failed { .. }));
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(dl.state, "failed");
}

// ---- Additional safety tests ----------------------------------------------

/// Changed response (content replaced) must not download silently.
#[test]
fn changed_content_must_not_silently_download() {
    let (_mgr, storage) = make_manager();
    let original_hash = make_hash("77");
    let id = create_paused(&storage, &original_hash, "peer-changed", 4096);

    storage.resume_download(id).unwrap();

    // Peer reports content changed.
    let response = FileAccessResponse::Changed;
    let decision = evaluate_resume_response(&response, &original_hash);

    assert!(matches!(
        decision,
        ResumeDecision::Restart {
            reason: ResumeRestartReason::HashMismatch { .. },
        }
    ));

    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    match outcome {
        ResumeOutcome::Restarted {
            new_download_id,
            content_hash: fresh_hash,
        } => {
            assert_eq!(fresh_hash, "", "Changed response yields empty fresh hash");

            let old = storage.get_download(id).unwrap().unwrap();
            assert_eq!(old.state, "cancelled");

            let new_dl = storage.get_download(new_download_id).unwrap().unwrap();
            assert_eq!(new_dl.state, "queued");
        }
        other => panic!("expected Restarted, got {other:?}"),
    }
}

/// Transient error (PeerUnreachable) leaves download paused for retry.
/// The download stays in `resolving_peer` state (execute_resume_decision
/// does NOT regress it — the caller is expected to handle retry scheduling).
#[test]
fn peer_unreachable_leaves_resolving_for_retry() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("88");
    let id = create_paused(&storage, &hash, "peer-unreachable", 1024);

    storage.resume_download(id).unwrap();

    let decision = ResumeDecision::Restart {
        reason: ResumeRestartReason::PeerUnreachable {
            details: "connection refused".to_string(),
        },
    };

    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    assert!(matches!(outcome, ResumeOutcome::RemainedPaused { .. }));
    // State stays as resolving_peer — the caller is expected to
    // schedule retry scheduling externally.
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(
        dl.state, "resolving_peer",
        "transient failure leaves download in resolving_peer"
    );
    assert_eq!(dl.content_hash, hash);

    // No queued downloads should have been created.
    let all_queued = storage.list_downloads_by_state("queued").unwrap();
    assert!(
        all_queued.is_empty(),
        "no new queued download on transient failure"
    );
}

/// Transient error (DescriptorFetchFailed) leaves download in resolving_peer for retry.
#[test]
fn descriptor_fetch_failed_leaves_resolving_for_retry() {
    let (_mgr, storage) = make_manager();
    let hash = make_hash("99");
    let id = create_paused(&storage, &hash, "peer-fetch-fail", 2048);

    storage.resume_download(id).unwrap();

    let decision = ResumeDecision::Restart {
        reason: ResumeRestartReason::DescriptorFetchFailed {
            details: "server unavailable".to_string(),
        },
    };

    let outcome = _mgr
        .execute_resume_decision(id, &decision, None, None)
        .unwrap();

    assert!(matches!(outcome, ResumeOutcome::RemainedPaused { .. }));
    let dl = storage.get_download(id).unwrap().unwrap();
    assert_eq!(
        dl.state, "resolving_peer",
        "transient failure leaves download in resolving_peer"
    );
    assert_eq!(dl.content_hash, hash);
}
