//! Catalogue lifecycle diagnostic event tests.
//!
//! Verifies that the seven catalogue lifecycle events are emitted at the
//! correct transitions, with the expected identifiers and metadata, and
//! that no sensitive catalogue contents appear in the event payloads.
//!
//! Scenarios:
//!   1. Successful fetch flow                     — FetchStarted → FetchCompleted
//!   2. Fetch + persistent store                  — FetchStarted → FetchCompleted → RevisionInstalled
//!   3. handle_catalogue_notice full orchestration — NoticeReceived → FetchStarted → FetchCompleted → RevisionInstalled
//!   4. Signature rejection                       — FetchStarted → SignatureRejected
//!   5. Fetch failure (offline server)            — FetchStarted → FetchFailed
//!   6. Cached data used (server-side NotModified) — CatalogueCachedDataUsed
//!   7. Sensitive contents not leaked in event payloads

use std::sync::Arc;
use std::time::Duration;

use boru_chat::{
    catalogue_client::{
        fetch_remote_catalogue, handle_catalogue_notice, process_and_store_remote_catalogue,
        validate_complete_catalogue, RemoteCatalogueFetchError,
    },
    catalogue_handler::CatalogueHandler,
    chat_core::DIAGNOSTICS,
    diagnostics::{DiagnosticEvent, DiagnosticEventKind},
    friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore},
    protocol_version::CATALOGUE_ALPN,
    storage::Storage,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMode, SecretKey,
};
use tempfile::TempDir;

const FILE_SIZE: u64 = 1024;
const MIME_TYPE: &str = "application/octet-stream";

// ── Helpers ──────────────────────────────────────────────────────────────────

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

/// Add a file object and offer it as shared.
fn add_file(storage: &Storage, profile_id: &str, hash: &str, filename: &str) {
    storage
        .put_file_object(hash, FILE_SIZE, MIME_TYPE, filename, &[])
        .expect("put file object");
    storage
        .upsert_shared_file(hash, profile_id, "meta", filename, None, true)
        .expect("upsert shared file");
}

/// Mark a peer as a friend in the store.
fn make_friend(friends: &mut FriendsStore, peer_pk: &PublicKey) {
    let fid = FriendId::from_public_key(*peer_pk);
    let record = FriendRecord {
        relationship: FriendRelationship::Friends,
        ..Default::default()
    };
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

/// Create a client endpoint with a random ephemeral identity.
async fn create_client() -> (Endpoint, MemoryLookup) {
    create_client_with_key(None).await
}

/// Stable event name string for a diagnostic event kind.
fn event_name(kind: &DiagnosticEventKind) -> &'static str {
    match kind {
        DiagnosticEventKind::CatalogueNoticeReceived { .. } => "CatalogueNoticeReceived",
        DiagnosticEventKind::CatalogueFetchStarted { .. } => "CatalogueFetchStarted",
        DiagnosticEventKind::CatalogueFetchCompleted { .. } => "CatalogueFetchCompleted",
        DiagnosticEventKind::CatalogueFetchFailed { .. } => "CatalogueFetchFailed",
        DiagnosticEventKind::CatalogueSignatureRejected { .. } => "CatalogueSignatureRejected",
        DiagnosticEventKind::CatalogueRevisionInstalled { .. } => "CatalogueRevisionInstalled",
        DiagnosticEventKind::CatalogueCachedDataUsed { .. } => "CatalogueCachedDataUsed",
        _ => "other",
    }
}

/// Get the event names from a slice of events for comparison.
fn event_names(events: &[DiagnosticEvent]) -> Vec<&'static str> {
    events.iter().map(|e| event_name(&e.kind)).collect()
}

/// Assert that events match the expected names in order.
fn assert_event_sequence(events: &[DiagnosticEvent], expected: &[&'static str]) {
    let names = event_names(events);
    assert_eq!(
        names, expected,
        "event sequence mismatch.\n  got:      {:?}\n  expected: {:?}\n\nFull events:\n{:#?}",
        names, expected, events
    );
}

/// Record the current DIAGNOSTICS sequence, run an async operation, return
/// the events recorded during the operation filtered by peer_id.
///
/// Uses the raw next-event counter so the capture correctly spans from the
/// first event recorded during `op` through the last (using `>=` semantics
/// via `saturating_sub(1)` for `events_since_filtered`'s strict `>`).
///
/// NOTE: `events_since` uses `>` (strictly greater) comparison, which
/// excludes seq 0. When no events have been recorded yet (fresh process),
/// this function emits a throwaway dummy event at seq 0 so real events
/// always start at seq ≥ 1 and are correctly captured.
async fn capture_events<F, T>(
    peer_id_filter: Option<&PublicKey>,
    op: F,
) -> (T, Vec<DiagnosticEvent>)
where
    F: std::future::Future<Output = T>,
{
    // Seed the sequence counter if no events have been recorded yet.
    // `events_since()` uses `>` comparison, so seq 0 is always excluded.
    // A throwaway event at seq 0 pushes the first real event to seq ≥ 1.
    if DIAGNOSTICS.next_event_sequence() == 0 {
        DIAGNOSTICS.record(None, DiagnosticEventKind::PeerDiscovered);
    }

    let before_next = DIAGNOSTICS.next_event_sequence();
    let result = op.await;

    // events_since_filtered uses `sequence > since_sequence`, so passing
    // (before_next - 1) gives us events with sequence >= before_next.
    let since = before_next.saturating_sub(1);
    let events = if let Some(pk) = peer_id_filter {
        DIAGNOSTICS.events_since_filtered(since, 100, None, Some(&pk.to_string()))
    } else {
        DIAGNOSTICS.events_since(since, 100, None)
    };

    (result, events)
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 1: Successful fetch — FetchStarted → FetchCompleted
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn successful_fetch_emits_started_and_completed() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "abcdef01", "test.data");

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Act: fetch the catalogue while capturing events (filter by server_pk).
    let (result, events) = capture_events(
        Some(&server_pk),
        fetch_remote_catalogue(&cli_ep, server_pk, None),
    )
    .await;

    // Assert: fetch succeeded and exactly two events were emitted.
    let catalogue = result.expect("fetch should succeed");
    assert_eq!(catalogue.files.len(), 1, "friend client should see 1 file");
    assert_event_sequence(
        &events,
        &["CatalogueFetchStarted", "CatalogueFetchCompleted"],
    );

    // Assert payload fields on FetchStarted.
    let fetch_started = &events[0];
    assert!(matches!(
        &fetch_started.kind,
        DiagnosticEventKind::CatalogueFetchStarted {
            known_revision: None
        }
    ));

    // Assert payload fields on FetchCompleted.
    let fetch_completed = &events[1];
    match &fetch_completed.kind {
        DiagnosticEventKind::CatalogueFetchCompleted {
            revision,
            file_count,
            collection_count,
        } => {
            assert_eq!(*revision, catalogue.revision);
            assert_eq!(*file_count, 1);
            assert_eq!(*collection_count, 0);
        }
        other => panic!("expected FetchCompleted, got {:?}", other),
    }

    // Assert peer_id is set on both events.
    let server_pk_str = server_pk.to_string();
    assert_eq!(
        fetch_started.peer_id.as_deref(),
        Some(server_pk_str.as_str()),
        "peer_id should be the server's public key"
    );
    assert_eq!(
        fetch_completed.peer_id.as_deref(),
        Some(server_pk_str.as_str()),
        "peer_id should be the server's public key"
    );

    // Assert room_id is None (no room context).
    assert!(fetch_started.room_id.is_none());
    assert!(fetch_completed.room_id.is_none());

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 2: Fetch + store — FetchStarted → FetchCompleted → RevisionInstalled
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fetch_and_store_emits_all_three_events() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "hash1234", "data.bin");
    add_file(&storage, &profile_user_id, "hash5678", "more.bin");

    // Separate storage for the receiving side.
    let (client_storage, _client_dir) = make_storage();

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Step 1: Fetch the catalogue.
    let catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("fetch should succeed");

    // Step 2: Store it, capturing events.
    let before = DIAGNOSTICS.latest_sequence();
    let store_result = process_and_store_remote_catalogue(&client_storage, &catalogue);
    let events = DIAGNOSTICS.events_since_filtered(before, 100, None, Some(&server_pk.to_string()));

    store_result.expect("store should succeed");

    // We should see only the RevisionInstalled event.
    assert_event_sequence(&events, &["CatalogueRevisionInstalled"]);

    // Assert payload fields.
    let installed = &events[0];
    match &installed.kind {
        DiagnosticEventKind::CatalogueRevisionInstalled {
            revision,
            file_count,
            collection_count,
        } => {
            assert_eq!(*revision, catalogue.revision);
            assert_eq!(*file_count, 2);
            assert_eq!(*collection_count, 0);
        }
        other => panic!("expected RevisionInstalled, got {:?}", other),
    }

    // peer_id is the catalogue's owner_id (server_pk).
    assert_eq!(
        installed.peer_id.as_deref(),
        Some(server_pk.to_string().as_str()),
        "peer_id should match catalogue owner"
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 3: handle_catalogue_notice — NoticeReceived → FetchStarted →
//         FetchCompleted → RevisionInstalled
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn handle_catalogue_notice_emits_four_events() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (server_storage, _dir) = make_storage();
    init_manifest(&server_storage, &profile_user_id);
    add_file(&server_storage, &profile_user_id, "noticed", "notice.bin");

    let (client_storage, _client_dir) = make_storage();

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(server_storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Act: handle_catalogue_notice orchestrates the full lifecycle.
    let (result, events) = capture_events(
        Some(&server_pk),
        handle_catalogue_notice(&cli_ep, server_pk, None, &client_storage),
    )
    .await;

    let catalogue = result.expect("handle_catalogue_notice should succeed");
    assert!(!catalogue.files.is_empty());

    // Assert the exact event sequence.
    assert_event_sequence(
        &events,
        &[
            "CatalogueNoticeReceived",
            "CatalogueFetchStarted",
            "CatalogueFetchCompleted",
            "CatalogueRevisionInstalled",
        ],
    );

    // Verify NoticeReceived payload.
    match &events[0].kind {
        DiagnosticEventKind::CatalogueNoticeReceived { known_revision } => {
            assert_eq!(*known_revision, None);
        }
        other => panic!("expected NoticeReceived, got {:?}", other),
    }

    // Verify FetchStarted payload.
    match &events[1].kind {
        DiagnosticEventKind::CatalogueFetchStarted { known_revision } => {
            assert_eq!(*known_revision, None);
        }
        other => panic!("expected FetchStarted, got {:?}", other),
    }

    // Verify FetchCompleted payload.
    match &events[2].kind {
        DiagnosticEventKind::CatalogueFetchCompleted {
            revision,
            file_count,
            ..
        } => {
            assert_eq!(*revision, catalogue.revision);
            assert!(*file_count >= 1);
        }
        other => panic!("expected FetchCompleted, got {:?}", other),
    }

    // Verify RevisionInstalled payload.
    match &events[3].kind {
        DiagnosticEventKind::CatalogueRevisionInstalled {
            revision,
            file_count,
            ..
        } => {
            assert_eq!(*revision, catalogue.revision);
            assert!(*file_count >= 1);
        }
        other => panic!("expected RevisionInstalled, got {:?}", other),
    }

    // All events should carry the server's peer_id.
    let server_pk_str = server_pk.to_string();
    for event in &events {
        assert_eq!(
            event.peer_id.as_deref(),
            Some(server_pk_str.as_str()),
            "every event should carry server_pk as peer_id, got {:?} for {:?}",
            event.peer_id,
            event_name(&event.kind)
        );
    }

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 4: Signature rejection — FetchStarted → SignatureRejected
// ═════════════════════════════════════════════════════════════════════════════
//
// Validates that validate_complete_catalogue emits CatalogueSignatureRejected
// when a fetched catalogue has been tampered (signature no longer valid).
// The CatalogueFetchStarted event is emitted by fetch_remote_catalogue and
// recorded before the fetch completes; here we test the signature event path
// directly by calling validate_complete_catalogue on a tampered catalogue.

#[tokio::test]
async fn signature_rejection_emits_fetch_started_and_signature_rejected() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "hash_a", "file_a.bin");
    add_file(&storage, &profile_user_id, "hash_b", "file_b.bin");

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Fetch a valid catalogue to tamper with.
    let mut catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("initial valid fetch");

    // Tamper the revision — this invalidates the signature.
    catalogue.revision = 9_999_999;

    // Act: validate the tampered catalogue and capture events.
    let before = DIAGNOSTICS.latest_sequence();
    let err = validate_complete_catalogue(&catalogue, server_pk)
        .expect_err("tampered catalogue should fail validation");
    let events = DIAGNOSTICS.events_since_filtered(before, 50, None, Some(&server_pk.to_string()));

    // Assert: SignatureInvalid error.
    assert!(
        matches!(&err, RemoteCatalogueFetchError::SignatureInvalid { .. }),
        "expected SignatureInvalid, got {:?}",
        err
    );

    // Assert: exactly one event — CatalogueSignatureRejected.
    assert_eq!(
        events.len(),
        1,
        "expected exactly 1 CatalogueSignatureRejected event, got {}\n{:#?}",
        events.len(),
        events
    );
    let sig_rejected = &events[0];
    assert!(
        matches!(
            sig_rejected.kind,
            DiagnosticEventKind::CatalogueSignatureRejected { .. }
        ),
        "expected CatalogueSignatureRejected, got {:?}",
        sig_rejected.kind
    );

    // Verify the error message is non-empty and contains no sensitive content.
    match &sig_rejected.kind {
        DiagnosticEventKind::CatalogueSignatureRejected { error } => {
            assert!(!error.is_empty(), "error message must not be empty");
            assert!(
                !error.contains("file_a.bin"),
                "error must not leak file names: {error}"
            );
            assert!(
                !error.contains("hash_a"),
                "error must not leak file hashes: {error}"
            );
        }
        _ => unreachable!(),
    }

    // Verify peer_id is set to the server's public key.
    assert_eq!(
        sig_rejected.peer_id.as_deref(),
        Some(server_pk.to_string().as_str()),
        "peer_id should be server's public key"
    );

    // Verify room_id is None (no room context).
    assert!(sig_rejected.room_id.is_none());

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 5: Fetch failure (offline server) — FetchStarted → FetchFailed
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fetch_failure_emits_started_and_failed() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();

    // Create a server, get its address, shut it down immediately.
    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "gone", "gone.bin");

    let friends = FriendsStore::empty_at(TempDir::new().expect("temp").path());
    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;
    let addr = ep.addr();
    // Shut down the server so the client can't connect.
    drop(router);
    drop(ep);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (cli_ep, lookup) = create_client().await;
    lookup.set_endpoint_info(addr);

    // Act: fetch from the dead server.
    let (result, events) = capture_events(
        Some(&server_pk),
        fetch_remote_catalogue(&cli_ep, server_pk, None),
    )
    .await;

    // Assert: fetch should fail.
    assert!(result.is_err(), "fetch must fail against offline server");
    let err = result.unwrap_err();
    let is_conn_err = matches!(
        &err,
        RemoteCatalogueFetchError::ConnectionFailed { .. } | RemoteCatalogueFetchError::Timeout
    );
    assert!(
        is_conn_err,
        "expected ConnectionFailed or Timeout, got {:?}",
        err
    );

    // Assert events: FetchStarted → FetchFailed.
    assert_eq!(events.len(), 2, "expected exactly 2 events");
    assert_event_sequence(&events, &["CatalogueFetchStarted", "CatalogueFetchFailed"]);

    // Verify FetchFailed payload.
    match &events[1].kind {
        DiagnosticEventKind::CatalogueFetchFailed { error } => {
            assert!(!error.is_empty(), "error message must not be empty");
            // Verify no sensitive internal details in the error.
            assert!(
                !error.contains("gone.bin") && !error.contains("gone"),
                "FetchFailed error must not leak catalogue file details: {error}"
            );
        }
        other => panic!("expected FetchFailed, got {:?}", other),
    }

    // Verify peer_id is set.
    let server_pk_str = server_pk.to_string();
    assert_eq!(
        events[0].peer_id.as_deref(),
        Some(server_pk_str.as_str()),
        "FetchStarted should carry server peer_id"
    );
    assert_eq!(
        events[1].peer_id.as_deref(),
        Some(server_pk_str.as_str()),
        "FetchFailed should carry server peer_id"
    );

    drop(cli_ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 6: Cached data used (server-side NotModified path)
// ═════════════════════════════════════════════════════════════════════════════
//
// When the client sends known_revision matching the current server revision,
// the server emits CatalogueCachedDataUsed and returns NotModified.  The
// server-side event is recorded in the global DIAGNOSTICS and visible in
// the test process.

#[tokio::test]
async fn cached_data_used_when_revision_matches() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "cached", "cached.bin");

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // First fetch: get the catalogue and its revision.
    let catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("first fetch ok");
    let current_revision = catalogue.revision;

    // Second fetch with known_revision matching the server's current revision.
    // The server should detect NotModified and emit CatalogueCachedDataUsed.
    //
    // Capture events referencing the CLIENT's public key (because the
    // server-side CatalogueCachedDataUsed event records the requester —
    // i.e. the client — as peer_id).  We DON'T filter by peer_id so we
    // can also see the client-side events.
    let (result, events) = capture_events(
        None,
        fetch_remote_catalogue(&cli_ep, server_pk, Some(current_revision)),
    )
    .await;

    // Assert: the client should get NotModified.
    assert!(
        matches!(&result, Err(RemoteCatalogueFetchError::NotModified)),
        "expected NotModified, got {:?}",
        result
    );

    // Find CatalogueCachedDataUsed events whose peer_id matches the client.
    let client_pk_str = client_pk.to_string();
    let cached_events: Vec<&DiagnosticEvent> = events
        .iter()
        .filter(|e| {
            matches!(e.kind, DiagnosticEventKind::CatalogueCachedDataUsed { .. })
                && e.peer_id.as_deref() == Some(client_pk_str.as_str())
        })
        .collect();

    assert!(
        !cached_events.is_empty(),
        "must have at least one CatalogueCachedDataUsed event for client {client_pk_str}\n\
         all events from this test:\n{:#?}",
        events
    );

    // Verify the cached_revision in the event.
    match &cached_events[0].kind {
        DiagnosticEventKind::CatalogueCachedDataUsed { cached_revision } => {
            assert_eq!(
                *cached_revision, current_revision,
                "cached_revision must match the known revision"
            );
        }
        _ => unreachable!(),
    }

    // Verify client-side events also exist.
    let client_names = event_names(&events);
    assert!(
        client_names.contains(&"CatalogueFetchStarted"),
        "should include FetchStarted"
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 7: Sensitive catalogue contents not logged in event payloads
// ═════════════════════════════════════════════════════════════════════════════
//
// Verify that event payloads don't contain full catalogue file names,
// display names, or raw file hashes.

#[tokio::test]
async fn no_sensitive_contents_in_event_payloads() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);

    // Add files with distinctive names and hashes that should NOT appear
    // in any event payload.
    let sensitive_hash = "DEADBEEFCAFEBABE0102030405060708090a0b0c0d0e0f001122334455667788";
    let sensitive_name = "secret-project-plan.pdf";
    add_file(&storage, &profile_user_id, sensitive_hash, sensitive_name);

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Use capture_events to get events from this test only — filter by
    // server_pk to avoid cross-test contamination.
    let (result, events) = capture_events(
        Some(&server_pk),
        fetch_remote_catalogue(&cli_ep, server_pk, None),
    )
    .await;

    let catalogue = result.expect("fetch should succeed");
    assert_eq!(catalogue.files.len(), 1);
    assert_eq!(catalogue.files[0].content_hash, sensitive_hash);
    assert_eq!(catalogue.files[0].display_name, sensitive_name);
    assert!(catalogue.verify().is_ok());

    // Serialise all event payloads to strings and check for sensitive data.
    let event_debugs: Vec<String> = events.iter().map(|e| format!("{:#?}", e.kind)).collect();

    for debug_str in &event_debugs {
        // The file name should never appear in any event.
        assert!(
            !debug_str.contains(sensitive_name),
            "event payload must not contain file name '{sensitive_name}':\n{debug_str}"
        );
        // The full content hash should never appear in any event.
        assert!(
            !debug_str.contains(sensitive_hash),
            "event payload must not contain full content hash:\n{debug_str}"
        );
    }

    // The event should carry controlled metadata only.
    if let Some(fetch_completed) = events
        .iter()
        .find(|e| matches!(e.kind, DiagnosticEventKind::CatalogueFetchCompleted { .. }))
    {
        match &fetch_completed.kind {
            DiagnosticEventKind::CatalogueFetchCompleted {
                revision,
                file_count,
                collection_count,
            } => {
                assert!(*revision > 0);
                assert_eq!(*file_count, 1);
                assert_eq!(*collection_count, 0);
            }
            _ => unreachable!(),
        }
    }

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 8: known_revision forwarded into NoticeReceived and FetchStarted
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn known_revision_carried_into_notice_and_fetch_events() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (server_storage, _dir) = make_storage();
    init_manifest(&server_storage, &profile_user_id);
    add_file(&server_storage, &profile_user_id, "rev_data", "rev.bin");

    let (client_storage, _client_dir) = make_storage();

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(server_storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // First, fetch to learn the revision.
    let catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("first fetch");
    let known = Some(catalogue.revision);

    // Second call with known_revision — should propagate into events.
    let (result, events) = capture_events(
        Some(&server_pk),
        handle_catalogue_notice(&cli_ep, server_pk, known, &client_storage),
    )
    .await;

    match &result {
        Ok(_) => {
            // If the server returned a new catalogue, check events.
            assert_event_sequence(
                &events,
                &[
                    "CatalogueNoticeReceived",
                    "CatalogueFetchStarted",
                    "CatalogueFetchCompleted",
                    "CatalogueRevisionInstalled",
                ],
            );

            // NoticeReceived should carry the known revision.
            match &events[0].kind {
                DiagnosticEventKind::CatalogueNoticeReceived { known_revision: kr } => {
                    assert_eq!(*kr, known);
                }
                _ => unreachable!(),
            }

            // FetchStarted should carry the known revision.
            match &events[1].kind {
                DiagnosticEventKind::CatalogueFetchStarted { known_revision: kr } => {
                    assert_eq!(*kr, known);
                }
                _ => unreachable!(),
            }
        }
        Err(RemoteCatalogueFetchError::NotModified) => {
            // Server returned NotModified because known_revision matches.
            // In this case the orchestration emits NoticeReceived → FetchStarted
            // (FetchFailed is for transport/protocol errors only).
            assert_event_sequence(
                &events,
                &["CatalogueNoticeReceived", "CatalogueFetchStarted"],
            );

            // NoticeReceived should still carry the known revision.
            match &events[0].kind {
                DiagnosticEventKind::CatalogueNoticeReceived { known_revision: kr } => {
                    assert_eq!(*kr, known);
                }
                _ => unreachable!(),
            }
        }
        Err(other) => panic!("unexpected error: {:?}", other),
    }

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 9: Duplicate fetch does not emit duplicate events for the same
//         operation (no double Recording)
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fetch_emits_exactly_one_event_per_lifecycle_stage() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "unique", "unique.bin");

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    let (result, events) = capture_events(
        Some(&server_pk),
        fetch_remote_catalogue(&cli_ep, server_pk, None),
    )
    .await;

    assert!(result.is_ok(), "fetch should succeed");

    // Exactly one FetchStarted and one FetchCompleted — no duplicates.
    let started_count = events
        .iter()
        .filter(|e| matches!(e.kind, DiagnosticEventKind::CatalogueFetchStarted { .. }))
        .count();
    let completed_count = events
        .iter()
        .filter(|e| matches!(e.kind, DiagnosticEventKind::CatalogueFetchCompleted { .. }))
        .count();

    assert_eq!(
        started_count, 1,
        "exactly one FetchStarted expected, got {started_count}"
    );
    assert_eq!(
        completed_count, 1,
        "exactly one FetchCompleted expected, got {completed_count}"
    );

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 10: handle_catalogue_notice with fetch failure (offline server)
// ═════════════════════════════════════════════════════════════════════════════
//
// When the server is unreachable, handle_catalogue_notice must still emit
// CatalogueNoticeReceived, then the fetch path emits FetchStarted → FetchFailed.
// This verifies the orchestration boundary: the notice event is always emitted
// before trying the fetch, even when the fetch will fail.

#[tokio::test]
async fn handle_catalogue_notice_with_fetch_failure() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();

    // Set up a server, learn its address, then shut it down.
    let profile_user_id = server_pk.to_string();
    let (storage, _dir) = make_storage();
    init_manifest(&storage, &profile_user_id);
    add_file(&storage, &profile_user_id, "offline", "offline.bin");

    let friends = FriendsStore::empty_at(TempDir::new().expect("temp").path());
    let (router, ep) = create_server(storage, server_sk, profile_user_id, friends).await;
    let addr = ep.addr();
    drop(router);
    drop(ep);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Client storage (won't be reached since fetch fails first).
    let (client_storage, _client_dir) = make_storage();
    let (cli_ep, lookup) = create_client().await;
    lookup.set_endpoint_info(addr);

    // Act: call handle_catalogue_notice against the dead server.
    let (result, events) = capture_events(
        Some(&server_pk),
        handle_catalogue_notice(&cli_ep, server_pk, None, &client_storage),
    )
    .await;

    // Assert: the orchestration propagates the fetch error.
    let err = result.expect_err("handle_catalogue_notice must fail against offline server");
    let is_conn_err = matches!(
        &err,
        RemoteCatalogueFetchError::ConnectionFailed { .. } | RemoteCatalogueFetchError::Timeout
    );
    assert!(
        is_conn_err,
        "expected ConnectionFailed or Timeout, got {:?}",
        err
    );

    // Assert exact event sequence: NoticeReceived → FetchStarted → FetchFailed.
    // Note: FetchCompleted and RevisionInstalled are NOT emitted since the
    // fetch never completed successfully.
    assert_event_sequence(
        &events,
        &[
            "CatalogueNoticeReceived",
            "CatalogueFetchStarted",
            "CatalogueFetchFailed",
        ],
    );

    // Verify NoticeReceived carries known_revision (None).
    match &events[0].kind {
        DiagnosticEventKind::CatalogueNoticeReceived { known_revision } => {
            assert_eq!(*known_revision, None);
        }
        other => panic!("expected NoticeReceived, got {:?}", other),
    }

    // Verify FetchFailed carries a non-empty sanitized error.
    match &events[2].kind {
        DiagnosticEventKind::CatalogueFetchFailed { error } => {
            assert!(!error.is_empty(), "error must not be empty");
            assert!(
                !error.contains("offline.bin") && !error.contains("offline"),
                "FetchFailed must not leak file details: {error}"
            );
        }
        other => panic!("expected FetchFailed, got {:?}", other),
    }

    // All events carry the server's peer_id.
    let server_pk_str = server_pk.to_string();
    for (i, event) in events.iter().enumerate() {
        assert_eq!(
            event.peer_id.as_deref(),
            Some(server_pk_str.as_str()),
            "event[{i}] {:?} should carry server_pk as peer_id",
            event_name(&event.kind)
        );
    }

    drop(cli_ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 11: handle_catalogue_notice with NotModified when known_revision
//          matches the server's current revision
// ═════════════════════════════════════════════════════════════════════════════
//
// When the caller already has the latest revision and passes it as
// known_revision, the server returns NotModified.  The client emits
// CatalogueNoticeReceived and CatalogueFetchStarted but NOT FetchFailed
// (since NotModified is a protocol-level signal, not a transport error).
// The server side emits CatalogueCachedDataUsed.

#[tokio::test]
async fn handle_catalogue_notice_with_not_modified() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (server_storage, _dir) = make_storage();
    init_manifest(&server_storage, &profile_user_id);
    add_file(&server_storage, &profile_user_id, "nm_data", "nm.bin");

    let (client_storage, _client_dir) = make_storage();

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(server_storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // First fetch to learn the current revision.
    let catalogue = fetch_remote_catalogue(&cli_ep, server_pk, None)
        .await
        .expect("first fetch");
    let current_revision = catalogue.revision;

    // Second call with known_revision matching the server — expect NotModified.
    let (result, events) = capture_events(
        Some(&server_pk),
        handle_catalogue_notice(&cli_ep, server_pk, Some(current_revision), &client_storage),
    )
    .await;

    // Assert NotModified error.
    assert!(
        matches!(&result, Err(RemoteCatalogueFetchError::NotModified)),
        "expected NotModified, got {:?}",
        result
    );

    // Client-side events: only NoticeReceived and FetchStarted.
    // No FetchCompleted, no FetchFailed, no RevisionInstalled.
    // NotModified is not a transport error so record_fetch_result does NOT
    // emit FetchFailed.
    assert_event_sequence(
        &events,
        &["CatalogueNoticeReceived", "CatalogueFetchStarted"],
    );

    // Both events carry the known_revision.
    match &events[0].kind {
        DiagnosticEventKind::CatalogueNoticeReceived { known_revision: kr } => {
            assert_eq!(*kr, Some(current_revision));
        }
        _ => unreachable!(),
    }
    match &events[1].kind {
        DiagnosticEventKind::CatalogueFetchStarted { known_revision: kr } => {
            assert_eq!(*kr, Some(current_revision));
        }
        _ => unreachable!(),
    }

    // Verify the server-side CatalogueCachedDataUsed event exists with the
    // correct revision and carries the client as peer_id (the requester).
    let client_pk_str = client_pk.to_string();
    let all_events = DIAGNOSTICS.events_since(0, 200, None);
    let cached_events: Vec<&DiagnosticEvent> = all_events
        .iter()
        .filter(|e| {
            matches!(e.kind, DiagnosticEventKind::CatalogueCachedDataUsed { .. })
                && e.peer_id.as_deref() == Some(client_pk_str.as_str())
        })
        .collect();
    assert!(
        !cached_events.is_empty(),
        "server-side CatalogueCachedDataUsed event not found for client {client_pk_str}"
    );
    match &cached_events[0].kind {
        DiagnosticEventKind::CatalogueCachedDataUsed { cached_revision } => {
            assert_eq!(*cached_revision, current_revision);
        }
        _ => unreachable!(),
    }

    // Verify peer_id on client events.
    let server_pk_str = server_pk.to_string();
    for (i, event) in events.iter().enumerate() {
        assert_eq!(
            event.peer_id.as_deref(),
            Some(server_pk_str.as_str()),
            "event[{i}] {:?} should carry server_pk as peer_id",
            event_name(&event.kind)
        );
    }

    // Assert no duplicate or spurious events on the client side.
    assert_eq!(events.len(), 2, "exactly 2 client-side events expected");

    drop(cli_ep);
    drop(router);
    drop(ep);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 12: Repeated handle_catalogue_notice calls each emit their own
//          CatalogueNoticeReceived (no cross-call deduplication)
// ═════════════════════════════════════════════════════════════════════════════
//
// Each notice/fetch cycle is independent.  Two calls to handle_catalogue_notice
// must produce two distinct NoticeReceived events with the same payload.
// This validates that the event emission is per-invocation, not deduplicated.

#[tokio::test]
async fn repeated_notice_calls_emit_separate_notice_received_events() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let client_sk = SecretKey::generate();
    let client_pk = client_sk.public();

    let profile_user_id = server_pk.to_string();
    let (server_storage, _dir) = make_storage();
    init_manifest(&server_storage, &profile_user_id);
    add_file(&server_storage, &profile_user_id, "repeat1", "r1.bin");
    add_file(&server_storage, &profile_user_id, "repeat2", "r2.bin");

    let (client_storage, _client_dir) = make_storage();

    let friends_dir = TempDir::new().expect("friends temp dir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    make_friend(&mut friends, &client_pk);

    let (router, ep) = create_server(server_storage, server_sk, profile_user_id, friends).await;
    let (cli_ep, lookup) = create_client_with_key(Some(client_sk)).await;
    lookup.set_endpoint_info(ep.addr());

    // Capture events across two calls.
    let before = DIAGNOSTICS.next_event_sequence();
    let result1 = handle_catalogue_notice(&cli_ep, server_pk, None, &client_storage).await;
    let result2 = handle_catalogue_notice(&cli_ep, server_pk, None, &client_storage).await;

    let all_events = DIAGNOSTICS.events_since(before.saturating_sub(1), 200, None);

    // Both calls must succeed (we add 2 files so there's content).
    let catalogue1 = result1.expect("first handle_catalogue_notice should succeed");
    let catalogue2 = result2.expect("second handle_catalogue_notice should succeed");

    // Both catalogues should be identical (same server state).
    assert_eq!(catalogue1.revision, catalogue2.revision);
    assert_eq!(catalogue1.files.len(), catalogue2.files.len());

    // Filter for NoticeReceived events with the server's peer_id.
    let server_pk_str = server_pk.to_string();
    let notice_events: Vec<&DiagnosticEvent> = all_events
        .iter()
        .filter(|e| {
            matches!(e.kind, DiagnosticEventKind::CatalogueNoticeReceived { .. })
                && e.peer_id.as_deref() == Some(server_pk_str.as_str())
        })
        .collect();

    // Must see exactly 2 NoticeReceived events — one per call.
    assert_eq!(
        notice_events.len(),
        2,
        "expected exactly 2 NoticeReceived events (one per call), got {}: {:#?}",
        notice_events.len(),
        notice_events
    );

    // Verify the event sequence overall: each call produces its own
    // NoticeReceived → FetchStarted → FetchCompleted → RevisionInstalled.
    let filtered_events: Vec<&DiagnosticEvent> = all_events
        .iter()
        .filter(|e| e.peer_id.as_deref() == Some(server_pk_str.as_str()))
        .collect();
    let names: Vec<&str> = filtered_events
        .iter()
        .map(|e| event_name(&e.kind))
        .collect();

    // Two complete cycles = 8 events.
    assert_eq!(
        names.len(),
        8,
        "expected 8 total events (2×4 lifecycle), got {}: {:?}",
        names.len(),
        names
    );

    // Each cycle: NoticeReceived, FetchStarted, FetchCompleted, RevisionInstalled.
    assert_eq!(names[0], "CatalogueNoticeReceived");
    assert_eq!(names[1], "CatalogueFetchStarted");
    assert_eq!(names[2], "CatalogueFetchCompleted");
    assert_eq!(names[3], "CatalogueRevisionInstalled");
    assert_eq!(names[4], "CatalogueNoticeReceived");
    assert_eq!(names[5], "CatalogueFetchStarted");
    assert_eq!(names[6], "CatalogueFetchCompleted");
    assert_eq!(names[7], "CatalogueRevisionInstalled");

    // Verify both NoticeReceived events carry known_revision == None.
    let notice_kinds: Vec<&DiagnosticEventKind> = filtered_events
        .iter()
        .filter(|e| matches!(e.kind, DiagnosticEventKind::CatalogueNoticeReceived { .. }))
        .map(|e| &e.kind)
        .collect();
    assert_eq!(notice_kinds.len(), 2);
    for kind in &notice_kinds {
        match kind {
            DiagnosticEventKind::CatalogueNoticeReceived { known_revision } => {
                assert_eq!(*known_revision, None);
            }
            _ => unreachable!(),
        }
    }

    drop(cli_ep);
    drop(router);
    drop(ep);
}
