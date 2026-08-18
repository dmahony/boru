//! Unit + integration tests for blob transfer (blob_transfer).
//!
//! Covers the public transfer helpers driving the real installed iroh-blobs
//! API over loopback endpoints (RelayMode::Disabled, so no prod relay).

use super::*;
use crate::download_limits::DownloadLimiter;
use crate::file_access_protocol::{sign_download_descriptor, BlobFormat};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn test_config() -> BlobTransferConfig {
    BlobTransferConfig {
        transfer_timeout: Duration::from_secs(30),
        chunk_timeout: Duration::from_secs(5),
        progress_persist_interval: Duration::from_millis(100),
    }
}

/// Verify that transfer_blob_to_temp with in-memory iroh-blobs store
/// successfully downloads and writes a small blob to a temp file.
#[tokio::test]
async fn transfer_small_blob_success() {
    let tmp = TempDir::new().unwrap();
    let temp_path = tmp.path().join("download.part");
    let storage = Storage::memory().unwrap();

    // Create a blob store with a known blob.
    let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
    let data = b"hello blob transfer";
    let expected_hash = blake3::hash(data);
    let expected_hash_bytes: [u8; 32] = *expected_hash.as_bytes();
    let _blob_hash = iroh_blobs::Hash::from(expected_hash_bytes);

    // Import the blob into the store.
    blob_store
        .blobs()
        .add_bytes(data.to_vec())
        .await
        .expect("add bytes");

    // Create a descriptor that matches this blob.
    let sk = iroh::SecretKey::generate();
    let pk = sk.public();
    let now = now_ms();
    let descriptor = sign_download_descriptor(
        &sk,
        pk,
        "test-file".into(),
        expected_hash_bytes,
        data.len() as u64,
        BlobFormat::Raw,
        now,
        now + 60_000,
    );

    let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig {
        max_concurrent_downloads: 5,
        max_startup_downloads: 3,
        max_downloads_per_peer: 2,
        max_active_hash_verifications: 2,
        max_queued_downloads: 16,
        progress_update_interval: Duration::from_millis(100),
    });

    // Create a download row to track progress.
    // We need the storage to have a download in the RequestingPermission
    // state.  Use the pattern from file_access_client tests.
    storage
        .put_file_object(
            &hex::encode(expected_hash_bytes),
            data.len() as u64,
            "text/plain",
            "test.txt",
            data,
        )
        .expect("put file object");

    let download_id = storage
        .create_download(
            &hex::encode(expected_hash_bytes),
            &pk.to_string(),
            data.len() as u64,
        )
        .expect("create download");

    // Transition queued → requesting_permission via direct SQL
    // (the begin_download/mark_resume_peer_resolved pipeline was removed).
    storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
    storage
        .accept_resumed_descriptor(
            download_id,
            &hex::encode(expected_hash_bytes),
            data.len() as u64,
        )
        .expect("accept descriptor → downloading");

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let mut events = Vec::new();

    let result = transfer_blob_to_temp(
        &blob_store,
        // We need an endpoint for the downloader.  Since the blob is
        // already local, the downloader shouldn't need network I/O,
        // but it still requires an Endpoint handle.  Use a minimal
        // in-memory endpoint.
        &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::generate())
            .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
            .bind()
            .await
            .expect("bind endpoint"),
        &descriptor,
        vec![pk],
        temp_path.clone(),
        &storage,
        download_id,
        &limiter,
        cancel_flag,
        test_config(),
        |ev| events.push(ev),
    )
    .await;

    assert!(
        result.is_ok(),
        "transfer should succeed: {:?}",
        result.err()
    );

    // Verify the temp file exists and has the right content.
    assert!(temp_path.exists(), "temp file should exist");
    let actual = std::fs::read(&temp_path).expect("read temp file");
    assert_eq!(actual, data, "content should match");

    // Verify progress events: Started, at least one Progress, Completed.
    let started = events
        .iter()
        .any(|e| matches!(e, BlobTransferProgress::Started { .. }));
    let completed = events
        .iter()
        .any(|e| matches!(e, BlobTransferProgress::Completed { .. }));
    assert!(started, "should have Started event");
    assert!(completed, "should have Completed event");
}

/// Verify that cancellation stops the transfer and cleans up the temp file.
#[tokio::test]
async fn transfer_cancellation_cleans_up() {
    let tmp = TempDir::new().unwrap();
    let temp_path = tmp.path().join("cancel.part");
    let storage = Storage::memory().unwrap();

    let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
    let data = vec![0xABu8; 65_536]; // 64 KiB blob
    let expected_hash = blake3::hash(&data);
    let expected_hash_bytes: [u8; 32] = *expected_hash.as_bytes();

    blob_store
        .blobs()
        .add_bytes(data.clone())
        .await
        .expect("add bytes");

    let sk = iroh::SecretKey::generate();
    let pk = sk.public();
    let now = now_ms();
    let descriptor = sign_download_descriptor(
        &sk,
        pk,
        "cancel-test".into(),
        expected_hash_bytes,
        data.len() as u64,
        BlobFormat::Raw,
        now,
        now + 60_000,
    );

    let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig::default());

    storage
        .put_file_object(
            &hex::encode(expected_hash_bytes),
            data.len() as u64,
            "application/octet-stream",
            "cancel.bin",
            &data,
        )
        .expect("put file object");

    let download_id = storage
        .create_download(
            &hex::encode(expected_hash_bytes),
            &pk.to_string(),
            data.len() as u64,
        )
        .expect("create download");

    storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
    storage
        .accept_resumed_descriptor(
            download_id,
            &hex::encode(expected_hash_bytes),
            data.len() as u64,
        )
        .expect("accept");

    // Cancel immediately.
    let cancel_flag = Arc::new(AtomicBool::new(true));

    let result = transfer_blob_to_temp(
        &blob_store,
        &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::generate())
            .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
            .bind()
            .await
            .expect("bind endpoint"),
        &descriptor,
        vec![pk],
        temp_path.clone(),
        &storage,
        download_id,
        &limiter,
        cancel_flag,
        test_config(),
        |_ev| {},
    )
    .await;

    assert!(result.is_err(), "cancelled transfer should fail");
    // Temp file should be cleaned up.
    assert!(
        !temp_path.exists(),
        "temp file should be removed on cancellation"
    );
}

/// Verify that timeout aborts the transfer.
#[tokio::test]
async fn transfer_timeout_aborts() {
    let tmp = TempDir::new().unwrap();
    let temp_path = tmp.path().join("timeout.part");
    let storage = Storage::memory().unwrap();

    let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
    let data = vec![0xCDu8; 4096];
    let expected_hash = blake3::hash(&data);
    let expected_hash_bytes: [u8; 32] = *expected_hash.as_bytes();

    blob_store
        .blobs()
        .add_bytes(data.clone())
        .await
        .expect("add bytes");

    let sk = iroh::SecretKey::generate();
    let pk = sk.public();
    let now = now_ms();
    let descriptor = sign_download_descriptor(
        &sk,
        pk,
        "timeout-test".into(),
        expected_hash_bytes,
        data.len() as u64,
        BlobFormat::Raw,
        now,
        now + 60_000,
    );

    let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig::default());

    storage
        .put_file_object(
            &hex::encode(expected_hash_bytes),
            data.len() as u64,
            "application/octet-stream",
            "timeout.bin",
            &data,
        )
        .expect("put file object");

    let download_id = storage
        .create_download(
            &hex::encode(expected_hash_bytes),
            &pk.to_string(),
            data.len() as u64,
        )
        .expect("create download");

    storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
    storage
        .accept_resumed_descriptor(
            download_id,
            &hex::encode(expected_hash_bytes),
            data.len() as u64,
        )
        .expect("accept");

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let mut config = test_config();
    config.transfer_timeout = Duration::from_millis(1); // unrealistically short

    let result = transfer_blob_to_temp(
        &blob_store,
        &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::generate())
            .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
            .bind()
            .await
            .expect("bind endpoint"),
        &descriptor,
        vec![pk],
        temp_path.clone(),
        &storage,
        download_id,
        &limiter,
        cancel_flag,
        config,
        |_ev| {},
    )
    .await;

    assert!(result.is_err(), "timed-out transfer should fail");
    // Temp file should be cleaned up.
    assert!(
        !temp_path.exists(),
        "temp file should be removed on timeout"
    );
}

/// Verify that a wrong-size descriptor causes failure.
#[tokio::test]
async fn size_mismatch_rejected() {
    let tmp = TempDir::new().unwrap();
    let temp_path = tmp.path().join("size_mismatch.part");
    let storage = Storage::memory().unwrap();

    let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
    let data = b"actual content";
    let expected_hash = blake3::hash(data);
    let expected_hash_bytes: [u8; 32] = *expected_hash.as_bytes();

    blob_store
        .blobs()
        .add_bytes(data.to_vec())
        .await
        .expect("add bytes");

    let sk = iroh::SecretKey::generate();
    let pk = sk.public();
    let now = now_ms();
    // Deliberately wrong size:
    let descriptor = sign_download_descriptor(
        &sk,
        pk,
        "size-mismatch".into(),
        expected_hash_bytes,
        9999, // wrong size
        BlobFormat::Raw,
        now,
        now + 60_000,
    );

    let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig::default());

    storage
        .put_file_object(
            &hex::encode(expected_hash_bytes),
            9999,
            "application/octet-stream",
            "size_mismatch.bin",
            data,
        )
        .expect("put file object");

    let download_id = storage
        .create_download(&hex::encode(expected_hash_bytes), &pk.to_string(), 9999)
        .expect("create download");

    storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
    storage
        .accept_resumed_descriptor(download_id, &hex::encode(expected_hash_bytes), 9999)
        .expect("accept");

    let cancel_flag = Arc::new(AtomicBool::new(false));

    let result = transfer_blob_to_temp(
        &blob_store,
        &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::generate())
            .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
            .bind()
            .await
            .expect("bind endpoint"),
        &descriptor,
        vec![pk],
        temp_path.clone(),
        &storage,
        download_id,
        &limiter,
        cancel_flag,
        test_config(),
        |_ev| {},
    )
    .await;

    assert!(result.is_err(), "size mismatch should fail");
    // Verify the download state in storage is 'failed'.
    let download = storage.get_download(download_id).unwrap().unwrap();
    assert_eq!(download.state, "failed", "should be marked as failed");
}

/// Verify that a wrong-content-hash descriptor causes failure.
#[tokio::test]
async fn hash_mismatch_rejected() {
    let tmp = TempDir::new().unwrap();
    let temp_path = tmp.path().join("hash_mismatch.part");
    let storage = Storage::memory().unwrap();

    let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
    let data = b"actual content";
    let data_hash = blake3::hash(data);
    let _data_hash_bytes: [u8; 32] = *data_hash.as_bytes();

    blob_store
        .blobs()
        .add_bytes(data.to_vec())
        .await
        .expect("add bytes");

    let sk = iroh::SecretKey::generate();
    let pk = sk.public();
    let now = now_ms();
    // Wrong content hash in descriptor:
    let wrong_hash = [0xBBu8; 32];
    let descriptor = sign_download_descriptor(
        &sk,
        pk,
        "hash-mismatch".into(),
        wrong_hash,
        data.len() as u64,
        BlobFormat::Raw,
        now,
        now + 60_000,
    );

    let limiter = DownloadLimiter::new(crate::download_limits::DownloadLimitsConfig::default());

    storage
        .put_file_object(
            &hex::encode(wrong_hash),
            data.len() as u64,
            "application/octet-stream",
            "hash_mismatch.bin",
            data,
        )
        .expect("put file object");

    let download_id = storage
        .create_download(&hex::encode(wrong_hash), &pk.to_string(), data.len() as u64)
        .expect("create download");

    storage
            .with_conn(|conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                conn.execute(
                    "UPDATE downloads SET state = 'requesting_permission', updated_at_ms = ?1 WHERE id = ?2",
                    rusqlite::params![now, download_id],
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(())
            })
            .expect("set download to requesting_permission");
    storage
        .accept_resumed_descriptor(download_id, &hex::encode(wrong_hash), data.len() as u64)
        .expect("accept");

    let cancel_flag = Arc::new(AtomicBool::new(false));

    let result = transfer_blob_to_temp(
        &blob_store,
        &iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(iroh::SecretKey::generate())
            .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
            .bind()
            .await
            .expect("bind endpoint"),
        &descriptor,
        vec![pk],
        temp_path.clone(),
        &storage,
        download_id,
        &limiter,
        cancel_flag,
        test_config(),
        |_ev| {},
    )
    .await;

    assert!(result.is_err(), "hash mismatch should fail");
}
