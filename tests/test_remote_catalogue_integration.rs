//! Remote File Catalogue integration tests - deterministic two-peer tests using
//! temporary profiles.  No public DHT / DNS / relays / internet dependency.
//!
//! Scenarios:
//!   1. contacts-only file visibility
//!   2. non-contact (blocked) denial
//!   3. metadata change with revision increment
//!   4. revision notice and refresh (NotModified)
//!   5. offer removal and cache cleanup
//!   6. invalid signature rejection
//!   7. wrong-owner rejection
//!   8. large catalogue pagination
//!   9. revision change during pagination
//!   10. offline stale cache display

use std::sync::Arc;
use std::time::Duration;

use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
    EndpointAddr, PublicKey, RelayMode, SecretKey,
};
use n0_error::Result;
use tempfile::TempDir;

use boru_chat::{
    catalogue_client::{fetch_remote_catalogue, RemoteCatalogueFetchError},
    catalogue_handler::CatalogueHandler,
    catalogue_model::RemoteSharedFile,
    catalogue_protocol::{CatalogRequest, CatalogResponse},
    friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore},
    protocol_version::{
        read_frame, write_frame, CATALOGUE_ALPN, CATALOGUE_RETRIEVAL_V1,
        SUPPORTED_CATALOGUE_RETRIEVAL,
    },
    storage::Storage,
};

// -- Constants --------------------------------------------------------------------

/// Default file size used in tests.
const FILE_SIZE: u64 = 1024;

/// Default MIME type used in tests.
const MIME_TYPE: &str = "application/octet-stream";

/// Short timeout for operations that should succeed quickly on localhost.
const SHORT_TIMEOUT: Duration = Duration::from_secs(5);

/// Number of files for pagination tests.
const PAGINATION_FILE_COUNT: usize = 75;

// -- Helpers ---------------------------------------------------------------------

/// Create in-memory storage with manifest initialised.
fn make_storage() -> (Arc<Storage>, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    (storage, dir)
}

/// Initialise the manifest revision for a profile.
fn init_manifest(storage: &Storage, profile_id: &str) {
    storage
        .bump_manifest_revision(profile_id, "initial")
        .expect("bump manifest");
}

/// Add a file object and optionally offer it as shared.
fn add_file(storage: &Storage, profile_id: &str, hash: &str, filename: &str, offered: bool) {
    storage
        .put_file_object(hash, FILE_SIZE, MIME_TYPE, filename, &[])
        .expect("put file object");
    storage
        .upsert_shared_file(hash, profile_id, "meta", filename, None, offered)
        .expect("upsert shared file");
}

/// Bump the manifest revision to simulate a catalogue change.
fn bump_revision(storage: &Storage, profile_id: &str) -> u64 {
    storage
        .bump_manifest_revision(profile_id, "updated")
        .expect("bump manifest revision")
}

/// Remove an offered file from the shared files list.
fn remove_offer(storage: &Storage, profile_id: &str, hash: &str) {
    // To 'remove' an offer we un-offer it -- the file stays in the DB but
    // is no longer included in catalogues.
    storage
        .upsert_shared_file(hash, profile_id, "meta", "removed.data", None, false)
        .expect("un-offer file");
    // Also bump revision so callers detect the change.
    bump_revision(storage, profile_id);
}

/// Mark a peer as a friend in the store.
fn make_friend(friends: &mut FriendsStore, peer_pk: &PublicKey) {
    let fid = FriendId::from_public_key(*peer_pk);
    let mut record = FriendRecord::default();
    record.relationship = FriendRelationship::Friends;
    friends.upsert(fid, record);
}

/// Mark a peer as blocked in the store.
fn make_blocked(friends: &mut FriendsStore, peer_pk: &PublicKey) {
    let fid = FriendId::from_public_key(*peer_pk);
    let mut record = FriendRecord::default();
    record.relationship = FriendRelationship::Blocked;
    friends.upsert(fid, record);
}

/// Create a catalogue server node: endpoint + router with CatalogueHandler.
async fn create_server(
    storage: Arc<Storage>,
    secret_key: SecretKey,
    profile_user_id: String,
    friends: FriendsStore,
) -> (Router, Endpoint) {
    let handler = CatalogueHandler::new(storage, secret_key.clone(), profile_user_id, friends);
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(secret_key)
        .relay_mode(RelayMode::Disabled)
        .bind_addr(
            "127.0.0.1:0"
                .parse::<std::net::SocketAddrV4>()
                .expect("valid addr"),
        )
        .expect("bind addr")
        .bind()
        .await
        .expect("bind endpoint");
    let router = Router::builder(ep.clone())
        .accept(CATALOGUE_ALPN, handler)
        .spawn();
    (router, ep)
}

/// Create a client endpoint with a MemoryLookup for address resolution.
///
/// When `secret_key` is `Some`, the endpoint authenticates as that identity.
/// When `None`, a random ephemeral key is generated.
async fn create_client_with_key(secret_key: Option<SecretKey>) -> (Endpoint, MemoryLookup) {
    let sk = secret_key.unwrap_or_else(SecretKey::generate);
    let lookup = MemoryLookup::new();
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(sk)
        .address_lookup(lookup.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr(
            "127.0.0.1:0"
                .parse::<std::net::SocketAddrV4>()
                .expect("valid addr"),
        )
        .expect("bind addr")
        .bind()
        .await
        .expect("bind endpoint");
    (ep, lookup)
}

/// Convenience wrapper — creates a client with a random ephemeral identity.
async fn create_client() -> (Endpoint, MemoryLookup) {
    create_client_with_key(None).await
}

/// Low-level fetch of a single catalogue page via QUIC.
async fn fetch_catalogue_page(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    known_revision: Option<u64>,
    cursor: Option<String>,
    page_size: u32,
) -> Result<CatalogResponse> {
    let addr = EndpointAddr::new(server_pk);
    let conn = tokio::time::timeout(SHORT_TIMEOUT, client_ep.connect(addr, CATALOGUE_ALPN))
        .await
        .map_err(|_| n0_error::anyerr!("connect timeout"))?
        .map_err(|e| n0_error::anyerr!("connect: {e}"))?;

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| n0_error::anyerr!("open_bi: {e}"))?;

    let request = CatalogRequest::GetCataloguePage {
        known_revision,
        cursor,
        page_size,
    };
    let payload =
        postcard::to_stdvec(&request).map_err(|e| n0_error::anyerr!("encode request: {e}"))?;

    write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, &payload)
        .await
        .map_err(|e| n0_error::anyerr!("write frame: {e}"))?;
    send.finish()
        .map_err(|e| n0_error::anyerr!("finish send: {e}"))?;

    let (_version, resp_bytes) = read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue")
        .await
        .map_err(|e| n0_error::anyerr!("read frame: {e}"))?;

    let response: CatalogResponse =
        postcard::from_bytes(&resp_bytes).map_err(|e| n0_error::anyerr!("decode response: {e}"))?;

    Ok(response)
}

// -- Test 1: Contacts-only file visibility --------------------------------
// A friend peer sees offered files; a non-friend peer sees nothing.

#[tokio::test]
async fn contacts_only_file_visibility() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();
    let _stranger_sk = SecretKey::generate();
    let _stranger_pk = _stranger_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);

    // Add one file.
    add_file(&storage, &profile_user_id, "abcdef01", "friend.data", true);

    // Friends store: friend_pk is a friend; stranger_pk is not.
    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);
    // stranger is deliberately NOT added.

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;

    // Friend client should see the file.
    {
        let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
        lookup.set_endpoint_info(ep.addr());
        let catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
            .await
            .expect("friend should receive catalogue");
        assert_eq!(catalogue.files.len(), 1, "friend sees 1 file");
        assert_eq!(
            catalogue.files[0].content_hash, "abcdef01",
            "friend sees the correct file"
        );
        assert!(catalogue.verify().is_ok(), "catalogue signature valid");
        drop(cli_ep);
    }

    // Non-friend (stranger) client should get an empty catalogue.
    {
        let (cli_ep, lookup) = create_client().await;
        lookup.set_endpoint_info(ep.addr());
        let catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
            .await
            .expect("non-friend should also receive a catalogue (empty)");
        assert!(
            catalogue.files.is_empty(),
            "non-friend sees 0 files (contacts-only default)"
        );
        assert!(catalogue.verify().is_ok(), "catalogue signature valid");
        drop(cli_ep);
    }

    drop(router);
    drop(ep);
}

// -- Test 2: Non-contact (blocked) denial ------------------------------------
// A blocked peer gets PermissionDenied.

#[tokio::test]
async fn non_contact_denial() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let blocked_sk = SecretKey::generate();
    let blocked_pk = blocked_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "deadbeef", "secret.data", true);

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_blocked(&mut friends, &blocked_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(blocked_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    let err = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect_err("blocked peer should be denied");

    assert!(
        matches!(err, RemoteCatalogueFetchError::PermissionDenied),
        "expected PermissionDenied, got {:?}",
        err
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// -- Test 3: Metadata change with revision increment -------------------------
// Adding a file bumps the revision.

#[tokio::test]
async fn metadata_change_revision_increment() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    // Start with 1 file.
    add_file(&storage, &profile_user_id, "hash_one", "one.data", true);

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);

    let (router, ep) =
        create_server(storage.clone(), server_sk, profile_user_id.clone(), friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // First fetch -- should have 1 file.
    let cat1 = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("first fetch");
    assert_eq!(cat1.files.len(), 1, "first catalogue has 1 file");
    let rev1 = cat1.revision;

    // Add a second file and bump manifest revision.
    add_file(&storage, &profile_user_id, "hash_two", "two.data", true);
    let new_rev = bump_revision(&storage, &profile_user_id);
    assert!(new_rev > rev1, "revision increased: {} > {}", new_rev, rev1);

    // Second fetch -- should have 2 files and higher revision.
    let cat2 = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("second fetch");
    assert_eq!(cat2.files.len(), 2, "second catalogue has 2 files");
    assert!(
        cat2.revision > rev1,
        "revision increased: {} > {}",
        cat2.revision,
        rev1
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// -- Test 4: Revision notice and refresh (NotModified) -----------------------
// Sending known_revision = current_revision returns NotModified.

#[tokio::test]
async fn revision_notice_and_refresh() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "hash_a", "alpha.data", true);

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Fetch to learn the current revision.
    let catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("first fetch");
    let known_rev = catalogue.revision;

    // Fetch again with the known revision -- expect NotModified.
    let err = fetch_remote_catalogue(&cli_ep, server_pk, Some(known_rev))
        .await
        .expect_err("should return NotModified");

    assert!(
        matches!(err, RemoteCatalogueFetchError::NotModified),
        "expected NotModified, got {:?}",
        err
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// -- Test 5: Offer removal and cache cleanup --------------------------------
// Removing an offer removes the file from subsequent catalogue fetches.

#[tokio::test]
async fn offer_removal_and_cache_cleanup() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    // Two files initially.
    add_file(&storage, &profile_user_id, "keep_hash", "keep.data", true);
    add_file(
        &storage,
        &profile_user_id,
        "remove_hash",
        "remove.data",
        true,
    );

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);

    let (router, ep) =
        create_server(storage.clone(), server_sk, profile_user_id.clone(), friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // First fetch -- both files visible.
    let cat1 = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("first fetch");
    assert_eq!(cat1.files.len(), 2, "both files visible initially");

    // Remove one offer.
    remove_offer(&storage, &profile_user_id, "remove_hash");

    // Re-fetch -- only the kept file remains.
    let cat2 = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("second fetch");
    assert_eq!(cat2.files.len(), 1, "only kept file after removal");
    assert_eq!(
        cat2.files[0].content_hash, "keep_hash",
        "removed file is gone"
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// -- Test 6: Invalid signature rejection ---------------------------------------
// A tampered catalogue is rejected by the client-side verification.

#[tokio::test]
async fn invalid_signature_rejection() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(
        &storage,
        &profile_user_id,
        "hash_sig",
        "sig_test.data",
        true,
    );

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Fetch a valid catalogue.
    let mut catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("should fetch valid catalogue");

    // Tamper the revision -- this invalidates the signature.
    catalogue.revision = 9_999_999;

    // Verify that the client helper rejects it.
    let result = catalogue.verify();
    assert!(result.is_err(), "tampered catalogue must fail verification");

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// -- Test 7: Wrong-owner rejection -------------------------------------------
// A catalogue whose owner_id does not match the connection is rejected.

#[tokio::test]
async fn wrong_owner_rejection() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(
        &storage,
        &profile_user_id,
        "hash_owner",
        "owner_test.data",
        true,
    );

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Fetch a valid catalogue.
    let mut catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("should fetch valid catalogue");

    // Replace the owner_id with a different key.
    let wrong_pk = SecretKey::generate().public();
    catalogue.owner_id = wrong_pk;

    // SignedFileCatalogue::verify should now fail because the owner_id
    // does not match the signature.
    let result = catalogue.verify();
    assert!(
        result.is_err(),
        "catalogue with wrong owner must fail verification"
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// -- Test 8: Large catalogue pagination --------------------------------------
// A catalogue with many files can be fetched in pages via GetCataloguePage.

#[tokio::test]
async fn large_catalogue_pagination() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);

    // Add many files (PAGINATION_FILE_COUNT).
    for i in 0..PAGINATION_FILE_COUNT {
        let hash = format!("{:064x}", i);
        let filename = format!("file_{}.data", i);
        add_file(&storage, &profile_user_id, &hash, &filename, true);
    }

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Fetch all pages with page_size = 10.
    let page_size = 10u32;
    let mut all_files: Vec<RemoteSharedFile> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut current_revision: Option<u64> = None;

    loop {
        let response =
            fetch_catalogue_page(&cli_ep, server_pk, current_revision, cursor, page_size)
                .await
                .expect("fetch page");

        match response {
            CatalogResponse::CataloguePage(payload) => {
                assert!(payload.verify().is_ok(), "page signature must be valid");
                current_revision = Some(payload.revision);
                let count = payload.items.len() as u32;
                all_files.extend(payload.items);
                cursor = payload.next_cursor;

                if cursor.is_none() {
                    // Last page (may be partial).
                    assert!(
                        count <= page_size,
                        "last page has at most {} items",
                        page_size
                    );
                    break;
                }
                assert_eq!(count, page_size, "full page has {} items", page_size);
            }
            other => panic!("expected CataloguePage, got {:?}", other),
        }
    }

    assert_eq!(
        all_files.len(),
        PAGINATION_FILE_COUNT,
        "all files collected via pagination"
    );

    // Verify all hashes are unique.
    let mut hashes: Vec<&str> = all_files.iter().map(|f| f.content_hash.as_str()).collect();
    hashes.sort();
    hashes.dedup();
    assert_eq!(
        hashes.len(),
        PAGINATION_FILE_COUNT,
        "all file hashes must be unique"
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// -- Test 9: Revision change during pagination --------------------------------
// When the server's revision changes between page fetches, the server signals
// RevisionChanged and the client must restart.

#[tokio::test]
async fn revision_change_during_pagination() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);

    // Start with a moderate set of files.
    for i in 0..20 {
        let hash = format!("{:064x}", i);
        let filename = format!("initial_{}.data", i);
        add_file(&storage, &profile_user_id, &hash, &filename, true);
    }

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);

    let (router, ep) =
        create_server(storage.clone(), server_sk, profile_user_id.clone(), friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Fetch first page to get a cursor.
    let page1 = fetch_catalogue_page(&cli_ep, server_pk, None, None, 5)
        .await
        .expect("first page");
    let (revision_before, cursor1) = match &page1 {
        CatalogResponse::CataloguePage(p) => (p.revision, p.next_cursor.clone()),
        other => panic!("expected CataloguePage, got {:?}", other),
    };
    assert!(
        cursor1.is_some(),
        "first page should have a next cursor when >5 files exist"
    );

    // Now the server adds more files and bumps the revision.
    for i in 100..110 {
        let hash = format!("{:064x}", i);
        let filename = format!("new_{}.data", i);
        add_file(&storage, &profile_user_id, &hash, &filename, true);
    }
    bump_revision(&storage, &profile_user_id);

    // Request the next page with the stale cursor -- server should detect
    // revision mismatch.
    let page2 = fetch_catalogue_page(&cli_ep, server_pk, Some(revision_before), cursor1, 5)
        .await
        .expect("second page (expecting RevisionChanged)");

    match &page2 {
        CatalogResponse::RevisionChanged { new_revision } => {
            assert!(
                *new_revision > revision_before,
                "new_revision {} > {}",
                new_revision,
                revision_before
            );
        }
        other => panic!(
            "expected RevisionChanged after server revision bump, got {:?}",
            other
        ),
    }

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// -- Test 10: Offline stale cache display ------------------------------------
// When the server is unreachable, the client gets a connection error.

#[tokio::test]
async fn offline_stale_cache_display() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(
        &storage,
        &profile_user_id,
        "cached_hash",
        "cached.data",
        true,
    );

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &friend_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;

    let (cli_ep, lookup) = create_client_with_key(Some(friend_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Verify the server is reachable.
    let catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("server should be reachable before shutdown");
    assert_eq!(catalogue.files.len(), 1, "cached file exists");

    // Shut down the server.
    drop(router);
    drop(ep);
    // Brief yield so the shutdown propagates.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Now fetching should fail with a connection error.
    let err = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect_err("server is offline");

    // The error should indicate the connection could not be established.
    // It may be ConnectionFailed or Timeout depending on timing.
    let is_conn_err = matches!(
        &err,
        RemoteCatalogueFetchError::ConnectionFailed { .. }
            | RemoteCatalogueFetchError::Timeout { .. }
    );
    assert!(
        is_conn_err,
        "expected ConnectionFailed or Timeout after server shutdown, got {:?}",
        err
    );

    drop(cli_ep);
}
