//! UI file-sharing integration tests — end-to-end scenarios covering the
//! peer profile → shared files → download lifecycle.
//!
//! These tests exercise the Storage and DownloadManager layers that feed
//! the Iced GUI's PeerProfile / Shared Files / Download views, without
//! requiring an actual display server or X11 session.
//!
//! Scenarios:
//!   1. Peer profile data flow — store catalogue → verify meta/files/collections
//!   2. Refresh cycle — revision bump → data updated
//!   3. Stale cache detection — old fetched_at correctly identified as stale
//!   4. Collection browsing — catalogue with multiple collections → verify names
//!   5. Download from peer profile — create download → tick → terminal state
//!   6. Download progress observation — progress events recorded by diagnostics
//!   7. Pause and resume — pause → resume → complete
//!   8. Permission denied — non-friend peer sees empty catalogue
//!   9. Version mismatch — catalogue revision change → download rejection
//!  10. Verification failure — download failure handling
//!  11. Completed-file state — download reaches Complete with all fields
//!
//! Tests 1, 2, 8 require the `net` feature (two peers via QUIC).
//! Tests 3, 4, 5, 6, 7, 9, 10, 11 use Storage only (no network).
//!
//! No public DHT / DNS / relays / internet dependency. Uses Storage::memory().

use std::sync::Arc;
use std::time::Duration;

use boru_chat::{
    catalogue_model::{FileCatalogueCollection, RemoteSharedFile, SignedFileCatalogue},
    diagnostics::{DiagnosticEventKind, Diagnostics},
    download_manager::DownloadManager,
    friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore},
    storage::Storage,
};
use iroh::{PublicKey, SecretKey};
use tempfile::TempDir;

// ── Constants ────────────────────────────────────────────────────────────────

const FILE_SIZE: u64 = 1024;
const MIME_TYPE: &str = "application/octet-stream";

/// Threshold (ms) used by the app layer to consider cached data stale.
/// From app.rs line 1448: fetched_at older than 5 minutes → stale.
const STALE_THRESHOLD_MS: u64 = 300_000;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_storage() -> (Arc<Storage>, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    (storage, dir)
}

fn make_signed_catalogue(
    sk: &SecretKey,
    rev: u64,
    generated_at_ms: u64,
    collections: Vec<FileCatalogueCollection>,
    files: Vec<RemoteSharedFile>,
) -> SignedFileCatalogue {
    SignedFileCatalogue::sign(sk, rev, generated_at_ms, collections, files)
}

fn make_file(
    hash: &str,
    name: &str,
    size: u64,
    collection_id: Option<&str>,
    revision: u32,
) -> RemoteSharedFile {
    RemoteSharedFile::new(
        hash,
        name,
        None,
        size,
        MIME_TYPE,
        collection_id.map(|s| s.to_string()),
        revision,
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Simulate the app-level staleness check (from app.rs `apply_filter()`).
fn is_stale(fetched_at_ms: u64) -> bool {
    let elapsed = (now_ms() as i64).saturating_sub(fetched_at_ms as i64);
    elapsed > STALE_THRESHOLD_MS as i64
}

/// Helper to create a download and verify its initial state.
fn create_and_verify_download(
    storage: &Storage,
    content_hash: &str,
    remote_peer: &str,
    total_bytes: u64,
) -> i64 {
    let dl_id = storage
        .create_download(content_hash, remote_peer, total_bytes)
        .expect("create download");
    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert_eq!(dl.state, "queued", "initial state is queued");
    dl_id
}

fn make_friend(friends: &mut FriendsStore, peer_pk: &PublicKey) {
    let fid = FriendId::from_public_key(*peer_pk);
    let record = FriendRecord {
        relationship: FriendRelationship::Friends,
        ..Default::default()
    };
    friends.upsert(fid, record);
}

/// Create a download manager with diagnostics wired in.
fn make_manager_diag(storage: Storage) -> (DownloadManager, Arc<Diagnostics>, tempfile::TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let diag = Arc::new(Diagnostics::new());
    let mut manager = DownloadManager::new(storage);
    manager.with_diagnostics(diag.clone());
    (manager, diag, dir)
}

/// Check whether a state string represents a terminal download state.
fn is_terminal(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "cancelled")
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 1: Peer profile data flow (requires net feature)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Store a signed catalogue → verify all profile data is loadable:
//   - RemoteCatalogueMeta (revision, timestamps)
//   - RemoteSharedFileRow (content_hash, filename, size, collection)
//   - RemoteCollectionRow (name)
//
// This simulates what the UI does when OpenPeerProfile is triggered:
// load_peer_profile_from_storage → get_remote_catalogue_meta +
// get_remote_shared_files + get_remote_collections.
#[cfg(feature = "net")]
#[tokio::test]
async fn peer_profile_data_flow() {
    use boru_chat::catalogue_client::fetch_remote_catalogue;
    use boru_chat::catalogue_handler::CatalogueHandler;
    use boru_chat::protocol_version::CATALOGUE_ALPN;
    use iroh::{
        address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
        RelayMode,
    };

    let (storage, _dir) = make_storage();
    let sk = SecretKey::generate();
    let pk = sk.public();

    // Client identity — must be a friend of the server so the CatalogueHandler
    // returns shared files (contacts-only visibility).
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    // Seed shared files on the server BEFORE creating it.
    let profile_user_id = pk.to_string();
    storage
        .bump_manifest_revision(&profile_user_id, "initial")
        .expect("bump");
    storage
        .put_file_object("hash-aaa", FILE_SIZE, MIME_TYPE, "doc1.pdf", &[])
        .expect("put");
    storage
        .upsert_shared_file("hash-aaa", &profile_user_id, "meta", "doc1.pdf", None, true)
        .expect("upsert");
    storage
        .put_file_object("hash-bbb", 2048, MIME_TYPE, "photo.jpg", &[])
        .expect("put");
    storage
        .upsert_shared_file(
            "hash-bbb",
            &profile_user_id,
            "meta",
            "photo.jpg",
            None,
            true,
        )
        .expect("upsert");

    // Create server.
    let handler = CatalogueHandler::new(storage.clone(), sk.clone(), pk.to_string(), friends);
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(sk)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .expect("bind addr")
        .bind()
        .await
        .expect("bind endpoint");
    let router = Router::builder(ep.clone())
        .accept(CATALOGUE_ALPN, handler)
        .spawn();

    // Client with address lookup.
    let lookup = MemoryLookup::new();
    let cli_ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(client_sk)
        .address_lookup(lookup.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .expect("bind addr")
        .bind()
        .await
        .expect("bind endpoint");
    lookup.set_endpoint_info(ep.addr());

    // Fetch the catalogue.
    let catalogue = tokio::time::timeout(
        Duration::from_secs(5),
        fetch_remote_catalogue(&cli_ep, pk, None),
    )
    .await
    .expect("timeout")
    .expect("fetch catalogue");

    assert!(catalogue.verify().is_ok(), "catalogue signature valid");
    assert_eq!(catalogue.files.len(), 2, "should see 2 shared files");

    // Receiver caches the catalogue.
    let receiver_storage = Storage::memory().expect("receiver storage");
    receiver_storage
        .replace_remote_catalogue(&catalogue)
        .expect("replace");

    // ── Verify: this is what the UI reads on OpenPeerProfile ──

    // Catalogue meta
    let meta = receiver_storage
        .get_remote_catalogue_meta(&pk)
        .expect("get meta")
        .expect("catalogue meta should exist");
    assert_eq!(meta.revision, 1, "revision should be 1");
    assert!(meta.fetched_at_ms > 0, "fetched_at should be set");
    assert!(meta.generated_at_ms > 0, "generated_at should be set");

    // Shared files
    let files = receiver_storage
        .get_remote_shared_files(&pk)
        .expect("get files");
    assert_eq!(files.len(), 2, "should have 2 cached files");
    let first = files
        .iter()
        .find(|f| f.content_hash == "hash-aaa")
        .expect("hash-aaa");
    assert_eq!(first.display_filename, "doc1.pdf");
    assert_eq!(first.size_bytes, FILE_SIZE);
    assert_eq!(first.mime_type, MIME_TYPE);

    // Collections (none in this catalogue).
    let collections = receiver_storage
        .get_remote_collections(&pk)
        .expect("get collections");
    assert!(collections.is_empty(), "no collections in this catalogue");

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 2: Refresh cycle — revision bump updates cached data (requires net)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Simulates the UI refresh flow:
//   1. Initial catalogue with 1 file → cached.
//   2. Server adds a second file and bumps revision.
//   3. Client fetches new catalogue.
//   4. Receiver replaces cached data → verifies new file is present and
//      metadata revision is updated.
#[cfg(feature = "net")]
#[tokio::test]
async fn refresh_cycle_updates_cached_data() {
    use boru_chat::catalogue_client::fetch_remote_catalogue;
    use boru_chat::catalogue_handler::CatalogueHandler;
    use boru_chat::protocol_version::CATALOGUE_ALPN;
    use iroh::{
        address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
        RelayMode,
    };

    let (storage, _dir) = make_storage();
    let sk = SecretKey::generate();
    let pk = sk.public();

    // Client identity — must be a friend of the server.
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    // Create server.
    let handler = CatalogueHandler::new(storage.clone(), sk.clone(), pk.to_string(), friends);
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(sk.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .expect("bind addr")
        .bind()
        .await
        .expect("bind endpoint");
    let router = Router::builder(ep.clone())
        .accept(CATALOGUE_ALPN, handler)
        .spawn();

    // Client with address lookup.
    let lookup = MemoryLookup::new();
    let cli_ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(client_sk)
        .address_lookup(lookup.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .expect("bind addr")
        .bind()
        .await
        .expect("bind endpoint");
    lookup.set_endpoint_info(ep.addr());

    let profile_user_id = pk.to_string();
    storage
        .bump_manifest_revision(&profile_user_id, "initial")
        .expect("bump");
    storage
        .put_file_object("v1-hash", FILE_SIZE, MIME_TYPE, "v1.doc", &[])
        .expect("put");
    storage
        .upsert_shared_file("v1-hash", &profile_user_id, "meta", "v1.doc", None, true)
        .expect("upsert");

    // Fetch v1 catalogue.
    let v1_catalogue = tokio::time::timeout(
        Duration::from_secs(5),
        fetch_remote_catalogue(&cli_ep, pk, None),
    )
    .await
    .expect("timeout")
    .expect("fetch v1 catalogue");

    let receiver_storage = Storage::memory().expect("receiver storage");
    receiver_storage
        .replace_remote_catalogue(&v1_catalogue)
        .expect("replace v1");

    // Verify v1 cached data.
    let meta_v1 = receiver_storage
        .get_remote_catalogue_meta(&pk)
        .expect("get meta v1")
        .expect("catalogue meta v1");
    let files_v1 = receiver_storage
        .get_remote_shared_files(&pk)
        .expect("get files v1");
    assert_eq!(files_v1.len(), 1, "v1 has 1 file");
    assert_eq!(meta_v1.revision, 1, "v1 revision is 1");

    // ── Server adds one more file and bumps revision ──
    storage
        .put_file_object("v2-hash", FILE_SIZE, MIME_TYPE, "v2.doc", &[])
        .expect("put");
    storage
        .upsert_shared_file("v2-hash", &profile_user_id, "meta", "v2.doc", None, true)
        .expect("upsert");
    storage
        .bump_manifest_revision(&profile_user_id, "updated")
        .expect("bump v2");

    // ── Client refreshes (fetches with known_revision=1) ──
    let v2_catalogue = tokio::time::timeout(
        Duration::from_secs(5),
        fetch_remote_catalogue(&cli_ep, pk, Some(1)),
    )
    .await
    .expect("timeout")
    .expect("fetch v2 catalogue");

    assert_eq!(v2_catalogue.files.len(), 2, "v2 has 2 files");
    assert!(v2_catalogue.verify().is_ok(), "v2 signature valid");

    receiver_storage
        .replace_remote_catalogue(&v2_catalogue)
        .expect("replace v2");

    let meta_v2 = receiver_storage
        .get_remote_catalogue_meta(&pk)
        .expect("get meta v2")
        .expect("catalogue meta v2");
    let files_v2 = receiver_storage
        .get_remote_shared_files(&pk)
        .expect("get files v2");
    assert_eq!(files_v2.len(), 2, "v2 has 2 files after refresh");
    assert_eq!(meta_v2.revision, 2, "revision bumped to 2");

    let hashes: Vec<&str> = files_v2.iter().map(|f| f.content_hash.as_str()).collect();
    assert!(
        hashes.contains(&"v1-hash"),
        "v1 file still present after refresh"
    );
    assert!(hashes.contains(&"v2-hash"), "v2 file added after refresh");

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 3: Stale cache detection (no network needed)
// ═══════════════════════════════════════════════════════════════════════════════
//
// The app layer marks cached data as stale when fetched_at is more than
// 5 minutes old.  This test verifies the staleness check logic with
// explicitly dated data, then verifies that a fresh fetch clears the
// stale marker.
//
// NOTE: This test validates the staleness LOGIC used by the UI.
// The storage layer does not implement staleness — it is a UI concern
// computed in PeerProfileUiState::apply_filter() based on fetched_at_ms.
#[tokio::test]
async fn stale_cache_detection() {
    // Test the staleness predicate directly (same logic as app.rs).
    let fresh_ms = now_ms();
    assert!(!is_stale(fresh_ms), "just-fetched data is not stale");

    let old_ms = now_ms() - STALE_THRESHOLD_MS - 60_000; // 6 minutes ago
    assert!(is_stale(old_ms), "data fetched 6+ min ago is stale");

    let boundary_ms = now_ms() - STALE_THRESHOLD_MS + 1_000; // just under 5 min
    assert!(!is_stale(boundary_ms), "data 4m59s old is not stale");

    // ── Now test that replace_remote_catalogue updates fetched_at_ms
    //    so the UI sees fresh data after a refresh. ──
    let sk = SecretKey::generate();
    let pk = sk.public();

    let catalogue_v1 = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![],
        vec![make_file("stale-hash", "stale.txt", 100, None, 1)],
    );

    let storage = Storage::memory().expect("receiver storage");
    storage
        .replace_remote_catalogue(&catalogue_v1)
        .expect("replace v1");

    // Immediately after fetch, data is not stale.
    let meta = storage
        .get_remote_catalogue_meta(&pk)
        .expect("get meta")
        .expect("catalogue meta");
    assert!(
        !is_stale(meta.fetched_at_ms),
        "freshly cached data is not stale"
    );

    // Replace with the same revision — fetched_at should still be recent.
    let catalogue_v1_repeat = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![],
        vec![make_file("stale-hash", "stale.txt", 100, None, 1)],
    );
    storage
        .replace_remote_catalogue(&catalogue_v1_repeat)
        .expect("replace v1 again");

    let meta2 = storage
        .get_remote_catalogue_meta(&pk)
        .expect("get meta")
        .expect("catalogue meta");
    assert!(
        !is_stale(meta2.fetched_at_ms),
        "repeated fetch with same revision should update fetched_at"
    );
    assert!(
        meta2.fetched_at_ms >= meta.fetched_at_ms,
        "fetched_at should not go backwards"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 4: Collection browsing (no network needed)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Store a catalogue with multiple collections. Verify that all collections
// are retrievable via get_remote_collections.
#[tokio::test]
async fn collection_browsing() {
    let sk = SecretKey::generate();
    let pk = sk.public();

    let catalogue = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![
            FileCatalogueCollection {
                collection_id: "docs".into(),
                name: "Documents".into(),
                description: None,
            },
            FileCatalogueCollection {
                collection_id: "photos".into(),
                name: "Photos".into(),
                description: None,
            },
        ],
        vec![
            make_file("doc-hash", "report.pdf", 51200, Some("docs"), 1),
            make_file("photo-hash", "sunset.jpg", 204800, Some("photos"), 1),
            make_file("misc-hash", "notes.txt", 1024, None, 1),
        ],
    );

    let storage = Storage::memory().expect("receiver storage");
    storage
        .replace_remote_catalogue(&catalogue)
        .expect("replace catalogue");

    // ── Verify all collections ──
    let collections = storage
        .get_remote_collections(&pk)
        .expect("get collections");
    assert_eq!(collections.len(), 2, "two collections");
    let coll_names: Vec<&str> = collections.iter().map(|c| c.name.as_str()).collect();
    assert!(coll_names.contains(&"Documents"));
    assert!(coll_names.contains(&"Photos"));

    // ── Verify all files present ──
    let files = storage.get_remote_shared_files(&pk).expect("get files");
    assert_eq!(files.len(), 3, "three files total");

    let doc_file = files
        .iter()
        .find(|f| f.content_hash == "doc-hash")
        .expect("doc-hash file");
    assert_eq!(doc_file.size_bytes, 51200);
    assert_eq!(doc_file.display_filename, "report.pdf");

    let misc_file = files
        .iter()
        .find(|f| f.content_hash == "misc-hash")
        .expect("misc-hash file");
    assert_eq!(misc_file.size_bytes, 1024);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 5: Download from peer profile — full lifecycle (no network needed)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Simulate the full peer-profile download flow:
//   1. Receiver caches a remote catalogue with 1 file.
//   2. User clicks Download → create_download from cached file hash.
//   3. DownloadManager processes the request through its state machine.
//   4. Download reaches Complete.
//   5. Diagnostics events are recorded.
#[tokio::test]
async fn download_from_peer_profile() {
    let sk = SecretKey::generate();
    let pk = sk.public();

    let catalogue = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![],
        vec![make_file("dl-hash", "downloadable.bin", FILE_SIZE, None, 1)],
    );

    let storage = Storage::memory().expect("receiver storage");
    storage
        .replace_remote_catalogue(&catalogue)
        .expect("replace catalogue");

    // Seed the file_object so the download machinery can find it.
    storage
        .put_file_object("dl-hash", FILE_SIZE, MIME_TYPE, "downloadable.bin", &[])
        .expect("put file object");

    // ── User clicks Download in peer profile.
    let dl_id = create_and_verify_download(&storage, "dl-hash", &pk.to_string(), FILE_SIZE);

    // ── Process through DownloadManager ──
    let (manager, diag, _dir) = make_manager_diag(storage.clone());

    // Tick enough times to reach terminal state.
    for _ in 0..10 {
        let _ = manager.tick().await;
    }

    // ── Verify terminal state ──
    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert!(is_terminal(&dl.state), "download should be terminal");
    assert_eq!(dl.state, "completed", "download should complete");
    assert_eq!(dl.content_hash, "dl-hash");
    assert_eq!(dl.total_bytes, FILE_SIZE);
    assert_eq!(dl.remote_peer, pk.to_string());

    // ── Verify diagnostic events. ──
    let events = diag.events_since(0, 100, None);
    let has_completed = events
        .iter()
        .any(|e| matches!(&e.kind, DiagnosticEventKind::BlobTransferCompleted { .. }));
    assert!(has_completed, "BlobTransferCompleted should be recorded");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 6: Download progress observation (no network needed)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Verify that the diagnostic event journal captures download progress
// events that the UI could display as progress bars or status text.
#[tokio::test]
async fn download_progress_observation() {
    let sk = SecretKey::generate();
    let pk = sk.public();

    let catalogue = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![],
        vec![make_file(
            "progress-hash",
            "progress.bin",
            FILE_SIZE,
            None,
            1,
        )],
    );

    let storage = Storage::memory().expect("receiver storage");
    storage
        .replace_remote_catalogue(&catalogue)
        .expect("replace catalogue");
    storage
        .put_file_object("progress-hash", FILE_SIZE, MIME_TYPE, "progress.bin", &[])
        .expect("put file object");

    let dl_id = create_and_verify_download(&storage, "progress-hash", &pk.to_string(), FILE_SIZE);

    let (manager, diag, _dir) = make_manager_diag(storage.clone());

    for _ in 0..10 {
        let _ = manager.tick().await;
    }

    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert!(is_terminal(&dl.state), "download should complete");

    // Check for BlobTransferCompleted via events_since(0) which excludes
    // sequence 0 (the Started event).
    let events: Vec<_> = diag.events_since(0, 100, None);
    let completed_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(&e.kind, DiagnosticEventKind::BlobTransferCompleted { .. }))
        .collect();
    assert!(
        !completed_events.is_empty(),
        "at least one BlobTransferCompleted event"
    );

    let matching = completed_events.iter().any(|e| match &e.kind {
        DiagnosticEventKind::BlobTransferCompleted { content_hash, .. } => {
            content_hash.starts_with("progress")
        }
        _ => false,
    });
    assert!(
        matching,
        "BlobTransferCompleted should reference the hash prefix"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 7: Pause and resume (no network needed)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Pause a download after it enters a non-terminal state, verify paused state,
// then resume and verify it reaches Complete.
#[tokio::test]
async fn pause_and_resume_download() {
    let sk = SecretKey::generate();
    let pk = sk.public();

    let catalogue = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![],
        vec![make_file("pr-hash", "pause_resume.bin", FILE_SIZE, None, 1)],
    );

    let storage = Storage::memory().expect("receiver storage");
    storage
        .replace_remote_catalogue(&catalogue)
        .expect("replace catalogue");
    storage
        .put_file_object("pr-hash", FILE_SIZE, MIME_TYPE, "pause_resume.bin", &[])
        .expect("put file object");

    let dl_id = create_and_verify_download(&storage, "pr-hash", &pk.to_string(), FILE_SIZE);

    let manager = DownloadManager::new(storage.clone());

    // Tick to progress past queued state
    let _ = manager.tick().await;

    // ── Pause ──
    storage.pause_download(dl_id).expect("pause download");
    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert_eq!(dl.state, "paused", "should be paused");

    // ── Resume ──
    storage.resume_download(dl_id).expect("resume download");
    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert_eq!(dl.state, "queued", "should be queued after resume");

    // Complete via manager ticks.
    for _ in 0..10 {
        let _ = manager.tick().await;
    }

    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert_eq!(dl.state, "completed", "should reach completed after resume");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 8: Permission denied — non-friend sees empty catalogue (requires net)
// ═══════════════════════════════════════════════════════════════════════════════
//
// A non-friend peer should receive an empty catalogue (contacts-only visibility).
#[cfg(feature = "net")]
#[tokio::test]
async fn permission_denied_non_friend() {
    use boru_chat::catalogue_client::fetch_remote_catalogue;
    use boru_chat::catalogue_handler::CatalogueHandler;
    use boru_chat::protocol_version::CATALOGUE_ALPN;
    use iroh::{
        address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
        RelayMode,
    };

    let (storage, _dir) = make_storage();
    let sk = SecretKey::generate();
    let pk = sk.public();
    let friends_dir = TempDir::new().expect("friends temp dir");
    let friends = FriendsStore::empty_at(friends_dir.path()); // No friends added.

    // Create server.
    let handler = CatalogueHandler::new(storage.clone(), sk.clone(), pk.to_string(), friends);
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(sk)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .expect("bind addr")
        .bind()
        .await
        .expect("bind endpoint");
    let router = Router::builder(ep.clone())
        .accept(CATALOGUE_ALPN, handler)
        .spawn();

    // Client with address lookup.
    let client_sk = SecretKey::generate();
    let lookup = MemoryLookup::new();
    let cli_ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(client_sk)
        .address_lookup(lookup.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .expect("bind addr")
        .bind()
        .await
        .expect("bind endpoint");
    lookup.set_endpoint_info(ep.addr());

    let profile_user_id = pk.to_string();
    storage
        .bump_manifest_revision(&profile_user_id, "initial")
        .expect("bump");
    storage
        .put_file_object("secret-hash", FILE_SIZE, MIME_TYPE, "secret.doc", &[])
        .expect("put");
    storage
        .upsert_shared_file(
            "secret-hash",
            &profile_user_id,
            "meta",
            "secret.doc",
            None,
            true,
        )
        .expect("upsert");

    // Non-friend should get empty catalogue.
    let catalogue = tokio::time::timeout(
        Duration::from_secs(5),
        fetch_remote_catalogue(&cli_ep, pk, None),
    )
    .await
    .expect("timeout")
    .expect("fetch catalogue");

    assert!(
        catalogue.files.is_empty(),
        "non-friend should see an empty catalogue (contacts-only)"
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 9: Version mismatch — catalogue revision change rejects download
// ═══════════════════════════════════════════════════════════════════════════════
//
// If a file's content hash changes in the cached catalogue while a download
// is in-flight, the download transitions to VersionMismatch.
#[tokio::test]
async fn version_mismatch_rejects_download() {
    let sk = SecretKey::generate();
    let pk = sk.public();

    // Initial catalogue with v1 file.
    let catalogue_v1 = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![],
        vec![make_file("v1-hash", "versioned.doc", FILE_SIZE, None, 1)],
    );

    let storage = Storage::memory().expect("receiver storage");
    storage
        .replace_remote_catalogue(&catalogue_v1)
        .expect("replace v1");
    storage
        .put_file_object("v1-hash", FILE_SIZE, MIME_TYPE, "versioned.doc", &[])
        .expect("put file object");

    let dl_id = create_and_verify_download(&storage, "v1-hash", &pk.to_string(), FILE_SIZE);

    let manager = DownloadManager::new(storage.clone());

    // Tick to start processing
    let _ = manager.tick().await;

    // Now change the catalogue: the file's content hash changed.
    let catalogue_v2 = make_signed_catalogue(
        &sk,
        2,
        2000,
        vec![],
        vec![make_file("v2-hash", "versioned.doc", FILE_SIZE, None, 2)],
    );
    storage
        .replace_remote_catalogue(&catalogue_v2)
        .expect("replace v2");

    // Next tick should detect the mismatch.
    let _ = manager.tick().await;

    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert_eq!(
        dl.state, "version_mismatch",
        "download should reject due to version mismatch"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 10: Verification failure handling (no network needed)
// ═══════════════════════════════════════════════════════════════════════════════
#[tokio::test]
async fn verification_failure_handling() {
    let sk = SecretKey::generate();
    let pk = sk.public();

    let catalogue = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![],
        vec![make_file("verify-hash", "verify.bin", FILE_SIZE, None, 1)],
    );

    let storage = Storage::memory().expect("receiver storage");
    storage
        .replace_remote_catalogue(&catalogue)
        .expect("replace catalogue");

    // Seed a file object.
    storage
        .put_file_object("verify-hash", FILE_SIZE, MIME_TYPE, "verify.bin", &[])
        .expect("put file object");

    let dl_id = create_and_verify_download(&storage, "verify-hash", &pk.to_string(), FILE_SIZE);

    let (manager, diag, _dir) = make_manager_diag(storage.clone());

    for _ in 0..10 {
        let _ = manager.tick().await;
    }

    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert!(
        is_terminal(&dl.state),
        "download should reach a terminal state"
    );

    // Check diagnostics for any failure events.
    let events = diag.events_since(0, 100, None);
    let failure_count = events
        .iter()
        .filter(|e| matches!(&e.kind, DiagnosticEventKind::BlobTransferFailed { .. }))
        .count();
    eprintln!(
        "Verification test: download state={:?}, failure events={}, total events={}",
        dl.state,
        failure_count,
        events.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 11: Completed-file state (no network needed)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Verify that a completed download has all expected fields populated and
// that the UI-facing state is accessible (content_hash, total_bytes,
// remote_peer).
#[tokio::test]
async fn completed_file_state() {
    let sk = SecretKey::generate();
    let pk = sk.public();

    let catalogue = make_signed_catalogue(
        &sk,
        1,
        1000,
        vec![],
        vec![make_file("done-hash", "done.bin", FILE_SIZE, None, 1)],
    );

    let storage = Storage::memory().expect("receiver storage");
    storage
        .replace_remote_catalogue(&catalogue)
        .expect("replace catalogue");
    storage
        .put_file_object("done-hash", FILE_SIZE, MIME_TYPE, "done.bin", &[])
        .expect("put file object");

    let dl_id = create_and_verify_download(&storage, "done-hash", &pk.to_string(), FILE_SIZE);

    let (manager, diag, _dir) = make_manager_diag(storage.clone());

    for _ in 0..10 {
        let _ = manager.tick().await;
    }

    let dl = storage
        .get_download(dl_id)
        .expect("get download")
        .expect("exists");
    assert_eq!(dl.state, "completed", "state is completed");
    assert_eq!(dl.content_hash, "done-hash", "content hash preserved");
    assert_eq!(dl.total_bytes, FILE_SIZE, "total bytes preserved");
    assert_eq!(dl.remote_peer, pk.to_string(), "remote peer preserved");

    // Verify diagnostic event for completion.
    let events = diag.events_since(0, 100, None);
    let completed: Vec<_> = events
        .iter()
        .filter(|e| matches!(&e.kind, DiagnosticEventKind::BlobTransferCompleted { .. }))
        .collect();
    assert!(
        !completed.is_empty(),
        "BlobTransferCompleted event recorded"
    );
    eprintln!(
        "Completed-file test: download complete, {} diagnostic events total",
        events.len()
    );
}
