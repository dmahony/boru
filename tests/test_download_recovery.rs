use boru_chat::{
    download::DownloadState, download_limits::DownloadLimitsConfig,
    download_manager::DownloadManager, storage::Storage,
};
use std::fs;
use tempfile::tempdir;

fn create_download(storage: &Storage, hash: &str, state: DownloadState) -> i64 {
    storage
        .put_file_object(hash, 4, "application/octet-stream", "file.bin", b"data")
        .unwrap();
    let id = storage.create_download(hash, "peer", 4).unwrap();
    storage
        .update_download_progress(id, 0, state.as_str())
        .unwrap();
    id
}

#[tokio::test]
async fn resolving_and_permission_recover_to_queue_without_temp_file() {
    let storage = Storage::memory().unwrap();
    let resolving = create_download(&storage, "recovery-resolving", DownloadState::ResolvingPeer);
    let permission = create_download(
        &storage,
        "recovery-permission",
        DownloadState::RequestingPermission,
    );

    DownloadManager::new(storage.clone())
        .recover_from_restart()
        .await
        .unwrap();

    assert_eq!(
        storage.get_download(resolving).unwrap().unwrap().state,
        "queued"
    );
    assert_eq!(
        storage.get_download(permission).unwrap().unwrap().state,
        "queued"
    );
}

#[tokio::test]
async fn resolving_with_partial_temp_recovers_to_paused() {
    let dir = tempdir().unwrap();
    let temp = dir.path().join("download.part");
    fs::write(&temp, b"par").unwrap();
    let storage = Storage::memory().unwrap();
    let id = create_download(&storage, "recovery-partial", DownloadState::ResolvingPeer);
    storage
        .set_download_paths(id, &temp, dir.path().join("file.bin"))
        .unwrap();

    DownloadManager::new(storage.clone())
        .recover_from_restart()
        .await
        .unwrap();

    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "paused");
}

#[test]
fn reopening_database_recovers_each_interrupted_state() {
    let dir = tempdir().unwrap();
    let ids;
    {
        let storage = Storage::open(dir.path()).unwrap();
        ids = [
            create_download(&storage, "reopen-resolving", DownloadState::ResolvingPeer),
            create_download(
                &storage,
                "reopen-permission",
                DownloadState::RequestingPermission,
            ),
            create_download(&storage, "reopen-downloading", DownloadState::Downloading),
            create_download(&storage, "reopen-verifying", DownloadState::Verifying),
            create_download(&storage, "reopen-complete", DownloadState::Complete),
        ];
    }
    let storage = Storage::open(dir.path()).unwrap();
    assert_eq!(
        storage.get_download(ids[0]).unwrap().unwrap().state,
        "queued"
    );
    assert_eq!(
        storage.get_download(ids[1]).unwrap().unwrap().state,
        "queued"
    );
    assert_eq!(
        storage.get_download(ids[2]).unwrap().unwrap().state,
        "paused"
    );
    assert_eq!(
        storage.get_download(ids[3]).unwrap().unwrap().state,
        "downloading"
    );
    assert_eq!(
        storage.get_download(ids[4]).unwrap().unwrap().state,
        "complete"
    );
}

#[tokio::test]
async fn downloading_recovers_to_paused_and_complete_stays_terminal() {
    let dir = tempdir().unwrap();
    let temp = dir.path().join("download.part");
    fs::write(&temp, b"data").unwrap();
    let storage = Storage::memory().unwrap();
    let downloading = create_download(&storage, "recovery-downloading", DownloadState::Downloading);
    storage
        .set_download_paths(downloading, &temp, dir.path().join("file.bin"))
        .unwrap();
    let complete = create_download(&storage, "recovery-complete", DownloadState::Complete);

    DownloadManager::new(storage.clone())
        .recover_from_restart()
        .await
        .unwrap();

    assert_eq!(
        storage.get_download(downloading).unwrap().unwrap().state,
        "paused"
    );
    assert_eq!(
        storage.get_download(complete).unwrap().unwrap().state,
        "complete"
    );
}

#[tokio::test]
async fn verifying_valid_temp_is_installed_and_completed() {
    let dir = tempdir().unwrap();
    let temp = dir.path().join("download.part");
    let destination = dir.path().join("file.bin");
    fs::write(&temp, b"data").unwrap();
    let hash = blake3::hash(b"data").to_hex().to_string();
    let storage = Storage::memory().unwrap();
    storage
        .put_file_object(&hash, 4, "application/octet-stream", "file.bin", b"data")
        .unwrap();
    let id = storage.create_download(&hash, "peer", 4).unwrap();
    storage
        .update_download_progress(id, 4, "verifying")
        .unwrap();
    storage.set_download_paths(id, &temp, &destination).unwrap();

    DownloadManager::new(storage.clone())
        .recover_from_restart()
        .await
        .unwrap();

    assert_eq!(storage.get_download(id).unwrap().unwrap().state, "complete");
    assert_eq!(fs::read(&destination).unwrap(), b"data");
    assert!(!temp.exists());
}

#[tokio::test]
async fn restored_downloads_are_admitted_in_deterministic_bounded_burst() {
    let storage = Storage::memory().unwrap();
    let mut ids = Vec::new();
    for index in 0..5 {
        ids.push(create_download(
            &storage,
            &format!("recovery-burst-{index}"),
            DownloadState::ResolvingPeer,
        ));
    }

    let manager = DownloadManager::with_limits(
        storage.clone(),
        DownloadLimitsConfig {
            max_startup_downloads: 2,
            max_concurrent_downloads: 5,
            max_downloads_per_peer: 5,
            max_active_hash_verifications: 1,
            max_queued_downloads: 10,
            ..DownloadLimitsConfig::default()
        },
    );
    manager.recover_from_restart().await.unwrap();

    let states: Vec<_> = ids
        .iter()
        .map(|id| storage.get_download(*id).unwrap().unwrap().state)
        .collect();
    assert_eq!(
        states,
        vec!["queued", "queued", "queued", "queued", "queued",]
    );

    // With max_startup_downloads=2, burst started 2 and 3 remain pending.
    let scheduler = manager.startup_scheduler().lock().unwrap();
    assert_eq!(scheduler.active_count(), 2, "burst should start 2");
    assert_eq!(scheduler.pending_count(), 3, "3 should remain pending");
    // All 5 IDs are managed by the scheduler (started + pending).
    for id in &ids {
        assert!(scheduler.contains(*id), "id {id} should be in scheduler");
    }
}
