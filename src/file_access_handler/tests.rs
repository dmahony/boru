// ── Tests ─────────────────────────────────────────────────────────

use super::*;

use crate::file_access_protocol::{BlobFormat, FileAccessRequest, FileAccessResponse};
use crate::friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore};
use crate::rings::RingPermission;
use crate::storage::Storage;
use iroh::PublicKey;
use rusqlite::params;
use std::sync::Arc;
use std::time::Duration;

/// Helper to build a minimal in-memory storage with a shared file.
fn setup_storage_with_file(
    metadata_id: &str,
    content_hash_hex: &str,
    offered: bool,
) -> (Arc<Storage>, FriendsStore) {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let profile_user_id = "owner-profile-id";

    // Insert a file object (put_file_object stores inline data).
    storage
        .put_file_object(
            content_hash_hex,
            1024,
            "text/plain",
            "test.txt",
            b"hello world",
        )
        .expect("put file object");

    // Insert the shared file offer.
    storage
        .upsert_shared_file(
            content_hash_hex,
            profile_user_id,
            metadata_id,
            "Test File",
            None,
            offered,
        )
        .expect("upsert shared file");

    // Friends store (empty = default contacts-only mode).
    let friends = FriendsStore::default();

    (storage, friends)
}

/// Helper to build a [`FileAccessHandler`] for testing.
fn test_handler(
    storage: Arc<Storage>,
    friends: FriendsStore,
) -> (FileAccessHandler, Arc<iroh_blobs::api::Store>) {
    let secret_key = iroh::SecretKey::generate();
    let profile_user_id = "owner-profile-id".to_string();
    let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());
    let handler = FileAccessHandler::new(
        storage,
        secret_key,
        profile_user_id,
        friends,
        Arc::new(NonceStore::new()),
        Arc::clone(&blob_store),
    );
    (handler, blob_store)
}

/// Helper to build a [`FileAccessHandler`] with a custom [`PrepareLimiter`].
fn test_handler_with_limiter(
    storage: Arc<Storage>,
    friends: FriendsStore,
    prepare_limiter: Arc<PrepareLimiter>,
) -> (FileAccessHandler, Arc<iroh_blobs::api::Store>) {
    let secret_key = iroh::SecretKey::generate();
    let profile_user_id = "owner-profile-id".to_string();
    let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());
    let handler = FileAccessHandler::with_limiters(
        storage,
        secret_key,
        profile_user_id,
        friends,
        Arc::new(NonceStore::new()),
        Arc::clone(&blob_store),
        prepare_limiter,
        Arc::new(UploadLimiter::new(UploadLimitsConfig::default())),
    );
    (handler, blob_store)
}

/// Create a test requester `PublicKey`.
fn requester_pk() -> PublicKey {
    iroh::SecretKey::generate().public()
}

/// Helper to create a valid `FileAccessRequest` with the given parameters.
fn make_request(
    shared_file_id: &str,
    content_hash_hex: &str,
    expected_version: u64,
) -> FileAccessRequest {
    let raw_hash = hex::decode(content_hash_hex).expect("valid hex");
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&raw_hash);
    FileAccessRequest::new(shared_file_id, arr, expected_version)
}

/// Shortcut: make the requester a friend of the profile.
fn add_friend(handler: &mut FileAccessHandler, pk: PublicKey) {
    handler.friends.upsert(
        FriendId::from_public_key(pk),
        FriendRecord {
            relationship: FriendRelationship::Friends,
            ..FriendRecord::default()
        },
    );
}

/// Helper to compute the blake3 hex hash of a byte slice, zero-padded to 64 chars.
fn hex_hash(data: &[u8]) -> String {
    let hash = blake3::hash(data);
    hex::encode(hash.as_bytes())
}

// ── Basic happy path ──────────────────────────────────────────────

#[tokio::test]
async fn happy_path_granted() {
    let metadata_id = "file-1";
    let content_hash = "ab".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    // Use the actual version from the DB as the expected version.
    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");

    let request = make_request(metadata_id, &content_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert!(
        matches!(response, FileAccessResponse::Granted(_)),
        "expected Granted, got {response:?}"
    );
}

/// The descriptor the server issues must carry EXACTLY the canonical blob
/// hash the requester asked for (`request.expected_content_hash`).  There
/// is no second representation on the descriptor that could drift: the
/// signed `blob_hash` IS the authorization target (BORU-AUDIT-06).
#[tokio::test]
async fn granted_descriptor_carries_requested_blob_hash() {
    let metadata_id = "file-1";
    let content_hash = "cd".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");

    let request = make_request(metadata_id, &content_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    let FileAccessResponse::Granted(descriptor) = response else {
        panic!("expected Granted, got {response:?}");
    };
    let mut expected_raw = [0u8; 32];
    expected_raw.copy_from_slice(&hex::decode(&content_hash).expect("valid hex"));
    assert_eq!(
        descriptor.blob_hash, expected_raw,
        "descriptor blob_hash must equal the requested content hash"
    );
    assert_eq!(
        hex::encode(descriptor.blob_hash),
        content_hash,
        "descriptor blob_hash must hex-encode to the stored content hash"
    );
}

// ── BORU-AUDIT-07: no guessed/fallback metadata in signed descriptors ──

/// A granted descriptor's size must exactly match the authoritative
/// store (the `file_objects` record that preparation verified), never a
/// guessed 0 or a stale re-query fallback.
#[tokio::test]
async fn descriptor_size_matches_authoritative_store() {
    let metadata_id = "file-1";
    let content_hash = "ef".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");
    let stored = handler
        .storage
        .get_file_object(&content_hash)
        .expect("get file object")
        .expect("file object exists");

    let request = make_request(metadata_id, &content_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    let FileAccessResponse::Granted(descriptor) = response else {
        panic!("expected Granted, got {response:?}");
    };
    assert_eq!(
        descriptor.size_bytes, stored.size,
        "descriptor size must exactly match the verified file object record"
    );
    assert_ne!(
        descriptor.size_bytes, 0,
        "descriptor must never carry a guessed zero size"
    );
}

/// A shared-file row whose content record (file_objects) is missing is
/// corrupt/stale: no descriptor may be issued.  The availability check
/// fails closed with Unavailable instead of signing a size-0 descriptor.
/// The orphan state is simulated by dropping the content record after
/// the offer is created (foreign keys off), matching what a crash or
/// legacy schema can leave behind.
#[tokio::test]
async fn missing_file_object_issues_no_descriptor() {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let metadata_id = "file-orphan";
    let content_hash = "01".repeat(32);
    // Create the offer normally (FK satisfied)…
    storage
        .put_file_object(
            &content_hash,
            10,
            "text/plain",
            "orphan.txt",
            b"orphan data",
        )
        .expect("put file object");
    storage
        .upsert_shared_file(
            &content_hash,
            "owner-profile-id",
            metadata_id,
            "orphan.txt",
            None,
            true,
        )
        .expect("upsert shared file");
    // …then lose the content record (corruption / legacy data).
    storage
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .map_err(|e| anyhow::anyhow!("disable fk: {e}"))?;
            conn.execute(
                "DELETE FROM file_objects WHERE content_hash = ?1",
                params![content_hash],
            )
            .map_err(|e| anyhow::anyhow!("delete file object: {e}"))?;
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .map_err(|e| anyhow::anyhow!("re-enable fk: {e}"))?;
            Ok(())
        })
        .expect("simulate corruption");

    let friends = FriendsStore::default();
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");

    let request = make_request(metadata_id, &content_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(
        response,
        FileAccessResponse::Unavailable,
        "missing file object must fail closed with Unavailable, got {response:?}"
    );
}

/// A persisted content hash that is not canonical (not 32 raw bytes of
/// hex) must never produce a signed descriptor.
#[tokio::test]
async fn invalid_persisted_hash_issues_no_descriptor() {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let metadata_id = "file-corrupt-hash";
    // 64 chars but NOT valid hex ('z' is not a hex digit) — a corrupt
    // catalogue record that must not be signed.
    let corrupt_hash = "z".repeat(64);
    storage
        .put_file_object(&corrupt_hash, 10, "text/plain", "corrupt.txt", b"corrupt")
        .expect("put file object");
    storage
        .upsert_shared_file(
            &corrupt_hash,
            "owner-profile-id",
            metadata_id,
            "corrupt.txt",
            None,
            true,
        )
        .expect("upsert shared file");

    let friends = FriendsStore::default();
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");

    // The requester's expected hash is a valid canonical hash that no
    // longer matches the corrupted record.
    let valid_hash = "ab".repeat(32);
    let request = make_request(metadata_id, &valid_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert!(
        !matches!(response, FileAccessResponse::Granted(_)),
        "corrupt persisted hash must never yield a signed descriptor, got {response:?}"
    );
}

/// A referenced file whose on-disk size no longer matches the DB record
/// must fail closed (Unavailable) instead of signing a descriptor with
/// stale size metadata (BORU-AUDIT-07).  Regression: the old code passed
/// `verify_size=None` and granted a descriptor with the old size.
#[tokio::test]
async fn referenced_file_resized_on_disk_issues_no_descriptor() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let original = b"original content";
    let (file_path, hex_hash) = write_temp_file(tmp.path(), "resized.txt", original);

    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    storage
        .put_file_object(
            &hex_hash,
            original.len() as u64,
            "text/plain",
            "resized.txt",
            original,
        )
        .expect("put file object");
    storage
        .set_file_object_source_path(&hex_hash, Some(file_path.to_str().expect("utf-8 path")))
        .expect("set source_path");
    storage
        .upsert_shared_file(
            &hex_hash,
            "owner-profile-id",
            "file-resized",
            "resized.txt",
            None,
            true,
        )
        .expect("upsert shared file");

    // Replace the file with DIFFERENT-length content: the on-disk size
    // now disagrees with the DB record.
    let modified = b"original content, now much longer on disk";
    assert_ne!(modified.len(), original.len(), "sizes must differ");
    std::fs::write(&file_path, modified).expect("write modified file");

    let friends = FriendsStore::default();
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", "file-resized")
        .expect("get shared file")
        .expect("shared file exists");

    let request = make_request("file-resized", &hex_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(
        response,
        FileAccessResponse::Unavailable,
        "on-disk size mismatch must fail closed, got {response:?}"
    );
}

/// An imported file whose blob-store size disagrees with the DB record
/// must fail closed: the content-addressed store is authoritative for
/// imported content (BORU-AUDIT-07).
#[tokio::test]
async fn imported_blob_size_mismatch_issues_no_descriptor() {
    let data = b"imported blob content";
    let blob_hash = blake3::hash(data);
    let hash_hex = hex::encode(blob_hash.as_bytes());

    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    // DB record claims a WRONG size for this content.
    storage
        .put_imported_file_object(
            &hash_hex,
            data.len() as u64 + 1000,
            "application/octet-stream",
            "imported.bin",
            &hash_hex,
            "some-peer",
        )
        .expect("put imported file object");
    storage
        .upsert_shared_file(
            &hash_hex,
            "owner-profile-id",
            "file-imported",
            "imported.bin",
            None,
            true,
        )
        .expect("upsert shared file");

    // Blob store contains the real content with the real size.
    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());
    let progress = blob_store.blobs().add_slice(data);
    progress.await.expect("add blob");

    let friends = FriendsStore::default();
    let mut handler = FileAccessHandler::new(
        storage,
        iroh::SecretKey::generate(),
        "owner-profile-id".to_string(),
        friends,
        Arc::new(NonceStore::new()),
        Arc::clone(&blob_store),
    );
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", "file-imported")
        .expect("get shared file")
        .expect("shared file exists");

    let request = make_request("file-imported", &hash_hex, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(
        response,
        FileAccessResponse::Unavailable,
        "blob-store size mismatch must fail closed, got {response:?}"
    );
}

// ── Not found ─────────────────────────────────────────────────────

#[tokio::test]
async fn file_not_found() {
    let content_hash = "cd".repeat(32);
    let (storage, friends) = setup_storage_with_file("file-1", &content_hash, true);
    let (handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    // Request a metadata_id that doesn't exist.
    let request = make_request("nonexistent-file", &content_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(response, FileAccessResponse::NotFound);
}

// ── Disabled (offer removed) ─────────────────────────────────────

#[tokio::test]
async fn file_disabled_after_catalogue_fetch() {
    let metadata_id = "file-1";
    let content_hash = "ef".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, false);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let request = make_request(metadata_id, &content_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(response, FileAccessResponse::Disabled);
}

// ── Blocked after catalogue fetch ─────────────────────────────────

#[tokio::test]
async fn blocked_after_catalogue_fetch() {
    let metadata_id = "file-1";
    let content_hash = "01".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    // Requester is blocked.
    handler.friends.upsert(
        FriendId::from_public_key(requester),
        FriendRecord {
            relationship: FriendRelationship::Blocked,
            ..FriendRecord::default()
        },
    );

    let request = make_request(metadata_id, &content_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(response, FileAccessResponse::PermissionDenied);
}

// ── Version changed ───────────────────────────────────────────────

#[tokio::test]
async fn version_changed_after_catalogue_fetch() {
    let metadata_id = "file-1";
    let content_hash = "23".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    // Expected version doesn't match updated_at_ms.
    let request = make_request(metadata_id, &content_hash, 999_999_999);
    let response = handler.check_permission(&requester, &request).await;

    assert!(
        matches!(response, FileAccessResponse::VersionMismatch { .. }),
        "expected VersionMismatch, got {response:?}"
    );
}

// ── Content hash changed ─────────────────────────────────────────

#[tokio::test]
async fn content_hash_changed_after_catalogue_fetch() {
    let metadata_id = "file-1";
    let old_hash = "45".repeat(32);
    let new_hash = "67".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &new_hash, true);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    // Requester has old hash, but file has new hash.
    let request = make_request(metadata_id, &old_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(response, FileAccessResponse::Changed);
}

// ── Explicit denial ───────────────────────────────────────────────

#[tokio::test]
async fn explicit_denial_at_request_time() {
    let metadata_id = "file-1";
    let content_hash = "89".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let storage_clone = Arc::clone(&storage);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    // Add an explicit denial for this requester.
    storage_clone
        .grant_permission(
            &content_hash,
            "owner-profile-id",
            &requester.to_string(),
            "deny",
            None,
        )
        .expect("add denial");

    let request = make_request(metadata_id, &content_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(response, FileAccessResponse::PermissionDenied);
}

// ── Permission revoked after catalogue fetch (selected-peers mode) ─

#[tokio::test]
async fn permission_revoked_after_catalogue_fetch() {
    let metadata_id = "file-1";
    let content_hash = "ab".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let storage_clone = Arc::clone(&storage);
    let (handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    // File has an explicit read grant for another peer → selected-peers mode.
    storage_clone
        .grant_permission(
            &content_hash,
            "owner-profile-id",
            "other-peer",
            "read",
            None,
        )
        .expect("add grant for other peer");

    // Requester is NOT in the read grants → should be denied.
    let request = make_request(metadata_id, &content_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(response, FileAccessResponse::PermissionDenied);
}

// ── Expired read grant does not authorize (selected-peers mode) ────
//
// Regression test for the FS-20 expiry bypass: `list_permissions_for_grantee`
// returns grants regardless of `expires_at_ms`, and the authorization loop
// previously marked ANY `read` grant as `explicitly_granted` — including an
// expired one. When the file also had an active grant to another peer
// (`has_any_read_grants == true`, selected-peers mode), the expired grant
// resurrected access. Expired grants must be inert.

#[tokio::test]
async fn expired_read_grant_does_not_authorize() {
    let metadata_id = "file-1";
    let content_hash = "12".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let storage_clone = Arc::clone(&storage);
    let (handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    // Active grant to another peer → file is in selected-peers mode.
    storage_clone
        .grant_permission(
            &content_hash,
            "owner-profile-id",
            "other-peer",
            "read",
            None,
        )
        .expect("add grant for other peer");

    // Requester's own read grant is EXPIRED.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    storage_clone
        .grant_permission(
            &content_hash,
            "owner-profile-id",
            &requester.to_string(),
            "read",
            Some(now - 1), // already expired
        )
        .expect("add expired grant for requester");

    let request = make_request(metadata_id, &content_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(
        response,
        FileAccessResponse::PermissionDenied,
        "expired read grant must not authorize in selected-peers mode"
    );
}

// ── Active read grant authorizes (selected-peers mode) ─────────────
//
// Counterpart to the expiry regression: a non-expired grant must still
// authorize, so the fix does not regress the happy path.

#[tokio::test]
async fn active_read_grant_authorizes() {
    let metadata_id = "file-1";
    let content_hash = "34".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let storage_clone = Arc::clone(&storage);
    let (handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    // Active grant to another peer → selected-peers mode.
    storage_clone
        .grant_permission(
            &content_hash,
            "owner-profile-id",
            "other-peer",
            "read",
            None,
        )
        .expect("add grant for other peer");

    // Requester's own read grant is still valid (far future expiry).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    storage_clone
        .grant_permission(
            &content_hash,
            "owner-profile-id",
            &requester.to_string(),
            "read",
            Some(now + 60_000),
        )
        .expect("add active grant for requester");

    // Use the actual DB version (stage 9 requires an exact match).
    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");
    let request = make_request(metadata_id, &content_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert!(
        matches!(response, FileAccessResponse::Granted(_)),
        "active read grant should authorize, got {response:?}"
    );
}

// ── Expired deny grant does not deny (contacts-only mode) ──────────
//
// An expired deny is inert: it must not keep a friend out after the
// denial period lapses. (The grantor removes a deny with
// `revoke_permission`; expiry is the schema-level way to bound it.)

#[tokio::test]
async fn expired_deny_grant_does_not_deny_friend() {
    let metadata_id = "file-1";
    let content_hash = "56".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let storage_clone = Arc::clone(&storage);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();
    add_friend(&mut handler, requester);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    storage_clone
        .grant_permission(
            &content_hash,
            "owner-profile-id",
            &requester.to_string(),
            "deny",
            Some(now - 1), // already expired
        )
        .expect("add expired deny for requester");

    // Use the actual DB version (stage 9 requires an exact match).
    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");
    let request = make_request(metadata_id, &content_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert!(
        matches!(response, FileAccessResponse::Granted(_)),
        "expired deny must not block a friend, got {response:?}"
    );
}

// ── Not friend in contacts-only mode ──────────────────────────────

#[tokio::test]
async fn not_friend_in_contacts_only_mode() {
    let metadata_id = "file-1";
    let content_hash = "cd".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let (handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    // No relationship established — requester is not a friend.
    let request = make_request(metadata_id, &content_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(response, FileAccessResponse::PermissionDenied);
}

// ── Source changed / new file at same metadata_id ────────────────

#[tokio::test]
async fn source_changed_after_catalogue_fetch() {
    let metadata_id = "file-1";
    let old_hash = "01".repeat(32);
    let new_hash = "23".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &new_hash, true);
    let (mut handler, _blob_store) = test_handler(storage, friends);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    // Requester has the old content hash in their catalogue.
    let request = make_request(metadata_id, &old_hash, 0);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(response, FileAccessResponse::Changed);
}

// ── NonceStore tests (unchanged — still sync) ────────────────────

#[test]
fn nonce_store_accepts_new_nonce() {
    let store = NonceStore::new();
    let nonce = [0x01; 32];
    let result = store.check_and_mark(nonce, 2_000_000, 1_000_000);
    assert_eq!(result, NonceCheck::Accepted);
}

#[test]
fn nonce_store_rejects_replayed_nonce() {
    let store = NonceStore::new();
    let nonce = [0xAA; 32];

    // First use — accepted.
    assert_eq!(
        store.check_and_mark(nonce, 2_000_000, 1_000_000),
        NonceCheck::Accepted,
    );

    // Second use with the same nonce — replayed.
    assert_eq!(
        store.check_and_mark(nonce, 2_000_000, 1_000_000),
        NonceCheck::Replayed,
    );
}

#[test]
fn nonce_store_accepts_different_nonces() {
    let store = NonceStore::new();

    assert_eq!(
        store.check_and_mark([0x01; 32], 2_000_000, 1_000_000),
        NonceCheck::Accepted,
    );
    assert_eq!(
        store.check_and_mark([0x02; 32], 2_000_000, 1_000_000),
        NonceCheck::Accepted,
    );
    assert_eq!(
        store.check_and_mark([0x03; 32], 2_000_000, 1_000_000),
        NonceCheck::Accepted,
    );
}

#[test]
fn nonce_store_accepts_nonce_after_expiry() {
    let store = NonceStore::new();
    let nonce = [0xBB; 32];

    // Mark at T=1000, expires at 2000.
    assert_eq!(
        store.check_and_mark(nonce, 2_000_000, 1_000_000),
        NonceCheck::Accepted,
    );

    // After expiry (T=3000), the same nonce can be used again.
    assert_eq!(
        store.check_and_mark(nonce, 4_000_000, 3_000_000),
        NonceCheck::Accepted,
    );
}

#[test]
fn nonce_store_len_and_is_empty() {
    let store = NonceStore::new();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);

    store.check_and_mark([0xCC; 32], 2_000_000, 1_000_000);
    assert_eq!(store.len(), 1);
    assert!(!store.is_empty());
}

#[test]
fn nonce_store_evicts_expired_on_access() {
    let store = NonceStore::new();

    // Insert a nonce that expires at T=2000.
    store.check_and_mark([0xDD; 32], 2_000_000, 1_000_000);
    assert_eq!(store.len(), 1);

    // Reading at T=3000 triggers lazy eviction.
    let result = store.check(&[0xDD; 32], 3_000_000);
    assert_eq!(result, NonceCheck::Accepted); // expired → treated as new
    assert_eq!(store.len(), 0); // evicted
}

#[test]
fn nonce_store_check_does_not_mark() {
    let store = NonceStore::new();
    let nonce = [0xEE; 32];

    // A read-only check returns Accepted for an unseen nonce.
    assert_eq!(store.check(&nonce, 1_000_000), NonceCheck::Accepted);
    // The nonce is NOT marked — a subsequent check_and_mark should
    // still see it as new.
    assert_eq!(
        store.check_and_mark(nonce, 2_000_000, 1_000_000),
        NonceCheck::Accepted,
    );
}

// ── prepare_imported_file tests ──────────────────────────────────

#[tokio::test]
async fn prepare_valid_imported_object() {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let data = b"hello world";
    let hex_hash = hex_hash(data);

    // Insert an imported file object with a blob_hash.
    // First add the data to the blob store so the blob exists.
    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());
    let progress = blob_store.blobs().add_slice(data);
    let blob_hash_str = progress.await.expect("add_slice").hash.to_string();

    storage
        .put_imported_file_object(
            &hex_hash,
            11,
            "text/plain",
            "hello.txt",
            &blob_hash_str,
            "sourc3r",
        )
        .expect("put imported file object");

    let prepared =
        prepare_imported_file(&storage, &blob_store, &hex_hash, Some(&hex_hash), Some(11))
            .await
            .expect("prepare imported file");

    assert_eq!(prepared.content_hash, hex_hash);
    assert_eq!(prepared.size_bytes, 11);
    assert_eq!(prepared.mime_type, "text/plain");
    assert_eq!(prepared.filename, "hello.txt");
    assert_eq!(prepared.blob_format, BlobFormat::Raw);
}

#[tokio::test]
async fn prepare_missing_imported_object() {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    // A hash string that parses but does not exist in the store.
    let fake_hash = iroh_blobs::Hash::from([0xAAu8; 32]);
    let hash_str = fake_hash.to_string();

    // Insert an imported file object with a blob_hash that does NOT
    // exist in the blob store.
    storage
        .put_imported_file_object(
            "aa".repeat(32).as_str(),
            100,
            "text/plain",
            "missing.txt",
            &hash_str,
            "peer1",
        )
        .expect("put imported file object");

    let result = prepare_imported_file(&storage, &blob_store, &"aa".repeat(32), None, None).await;

    assert!(result.is_err(), "expected error for missing blob");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("missing"),
        "error should mention missing blob, got: {err}"
    );
}

#[tokio::test]
async fn prepare_inline_file_imports_into_blob_store() {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let data = b"inline file data";
    let hex_hash = hex_hash(data);

    // Insert an inline file object (no blob_hash).
    storage
        .put_file_object(
            &hex_hash,
            data.len() as u64,
            "text/plain",
            "inline.txt",
            data,
        )
        .expect("put inline file object");

    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    // Before prepare_imported_file, the blob store should NOT have
    // this blob yet. Use the blake3 hash from BLAKE3 (not the content
    // hex, the actual blob hash).
    let raw_blake3 = blake3::hash(data);
    let blob_hash = iroh_blobs::Hash::from(raw_blake3);
    let exists_before = blob_store.blobs().has(blob_hash).await.unwrap();
    assert!(!exists_before, "blob should not exist yet");

    // Prepare should import the inline data into the blob store.
    let prepared = prepare_imported_file(&storage, &blob_store, &hex_hash, None, None)
        .await
        .expect("prepare inline file");

    assert_eq!(prepared.content_hash, hex_hash);
    assert_eq!(prepared.size_bytes, data.len() as u64);

    // After prepare, the blob should exist in the store.
    let exists_after = blob_store.blobs().has(blob_hash).await.unwrap();
    assert!(exists_after, "blob should exist after prepare");
}

#[tokio::test]
async fn prepare_wrong_size_is_rejected() {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let data = b"some content";
    let hex_hash = hex_hash(data);

    // Inline file.
    storage
        .put_file_object(&hex_hash, data.len() as u64, "text/plain", "f.txt", data)
        .expect("put file object");

    let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    let result = prepare_imported_file(
        &storage,
        &blob_store,
        &hex_hash,
        None,
        Some(9999), // wrong size
    )
    .await;

    assert!(result.is_err(), "expected error for wrong size");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("size mismatch"), "error: {err}");
}

#[tokio::test]
async fn prepare_wrong_hash_is_rejected() {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let data = b"content for hash check";
    let hex_hash = hex_hash(data); // actual hash

    storage
        .put_file_object(&hex_hash, data.len() as u64, "text/plain", "f.txt", data)
        .expect("put file object");

    let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    // Provide a wrong expected hash.
    let wrong_hash = "ff".repeat(32);
    let result =
        prepare_imported_file(&storage, &blob_store, &hex_hash, Some(&wrong_hash), None).await;

    assert!(result.is_err(), "expected error for wrong hash");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("hash mismatch"), "error: {err}");
}

#[tokio::test]
async fn prepare_nonexistent_file_returns_error() {
    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    let blob_store = Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    let result = prepare_imported_file(&storage, &blob_store, "nonexistent-hash", None, None).await;

    assert!(result.is_err(), "expected error for nonexistent file");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not found"), "error: {err}");
}

// ── Bounded preparation tests ───────────────────────────────────

#[tokio::test]
async fn prepare_limiter_rejects_too_large() {
    let config = PrepareConfig {
        max_file_size_bytes: 100,
        ..Default::default()
    };
    let limiter = PrepareLimiter::new(config);

    // File under limit: accepts.
    assert!(limiter.try_begin(50).is_ok());

    // File at limit: accepts.
    assert!(limiter.try_begin(100).is_ok());

    // File over limit: rejects.
    match limiter.try_begin(101).unwrap_err() {
        PrepareError::TooLarge {
            size_bytes,
            max_bytes,
        } => {
            assert_eq!(size_bytes, 101);
            assert_eq!(max_bytes, 100);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[tokio::test]
async fn prepare_limiter_rejects_when_busy() {
    let config = PrepareConfig {
        max_concurrent_preparations: 2,
        ..Default::default()
    };
    let limiter = PrepareLimiter::new(config);

    let _p1 = limiter.try_begin(100).expect("first permit");
    let _p2 = limiter.try_begin(100).expect("second permit");

    // Third should be Busy.
    match limiter.try_begin(100).unwrap_err() {
        PrepareError::Busy => {}
        other => panic!("expected Busy, got {other:?}"),
    }

    // Drop one permit — now re-usable.
    drop(_p1);
    assert!(limiter.try_begin(100).is_ok());
}

#[tokio::test]
async fn check_permission_returns_busy_when_limiter_exhausted() {
    let config = PrepareConfig {
        max_concurrent_preparations: 1,
        ..Default::default()
    };
    let limiter = Arc::new(PrepareLimiter::new(config));

    // Acquire the single slot before the request arrives.
    let _slot = limiter.try_begin(10).expect("reserve prep slot");

    let metadata_id = "file-1";
    let content_hash = "ab".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let (mut handler, _blob_store) = test_handler_with_limiter(storage, friends, limiter);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");

    let request = make_request(metadata_id, &content_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(
        response,
        FileAccessResponse::Busy,
        "expected Busy when prepare limiter exhausted, got {response:?}"
    );
}

#[tokio::test]
async fn check_permission_rejects_file_too_large() {
    let config = PrepareConfig {
        max_file_size_bytes: 1, // only 1 byte allowed
        ..Default::default()
    };
    let limiter = Arc::new(PrepareLimiter::new(config));

    let metadata_id = "file-1";
    let content_hash = "ab".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let (mut handler, _blob_store) = test_handler_with_limiter(storage, friends, limiter);
    let requester = requester_pk();

    add_friend(&mut handler, requester);

    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");

    let request = make_request(metadata_id, &content_hash, row.version);
    let response = handler.check_permission(&requester, &request).await;

    assert_eq!(
        response,
        FileAccessResponse::Unavailable,
        "expected Unavailable for oversized file, got {response:?}"
    );
}

// ── prepare_referenced_file tests ────────────────────────────────

/// Helper: create a temp file with the given data and return its path
/// and the hex-encoded blake3 hash.
fn write_temp_file(dir: &std::path::Path, name: &str, data: &[u8]) -> (std::path::PathBuf, String) {
    let path = dir.join(name);
    std::fs::write(&path, data).expect("write temp file");
    let hash = blake3::hash(data);
    let hex_hash = hex::encode(hash.as_bytes());
    (path, hex_hash)
}

#[allow(dead_code)]
fn hex_hash_for(data: &[u8]) -> String {
    let hash = blake3::hash(data);
    hex::encode(hash.as_bytes())
}

#[tokio::test]
async fn prepare_referenced_unchanged_source() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data = b"hello referenced file";
    let (file_path, hex_hash) = write_temp_file(tmp.path(), "source.txt", data);

    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    storage
        .put_file_object(
            &hex_hash,
            data.len() as u64,
            "text/plain",
            "source.txt",
            data,
        )
        .expect("put file object");
    storage
        .set_file_object_source_path(&hex_hash, Some(file_path.to_str().expect("utf-8 path")))
        .expect("set source_path");

    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    let prepared = prepare_referenced_file(
        &storage,
        &blob_store,
        &hex_hash,
        Some(&hex_hash),
        Some(data.len() as u64),
    )
    .await
    .expect("prepare referenced file");

    assert_eq!(prepared.content_hash, hex_hash);
    assert_eq!(prepared.size_bytes, data.len() as u64);
    assert_eq!(prepared.mime_type, "text/plain");
    assert_eq!(prepared.filename, "source.txt");
    assert_eq!(prepared.blob_format, BlobFormat::Raw);

    // Verify the blob was imported into the store.
    let raw_blake3 = blake3::hash(data);
    let blob_hash = iroh_blobs::Hash::from(raw_blake3);
    let exists = blob_store.blobs().has(blob_hash).await.unwrap();
    assert!(exists, "blob should exist after preparation");
}

#[tokio::test]
async fn prepare_referenced_missing_source() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data = b"will be deleted";
    let (file_path, hex_hash) = write_temp_file(tmp.path(), "delete_me.txt", data);

    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    storage
        .put_file_object(
            &hex_hash,
            data.len() as u64,
            "text/plain",
            "delete_me.txt",
            data,
        )
        .expect("put file object");
    storage
        .set_file_object_source_path(&hex_hash, Some(file_path.to_str().expect("utf-8 path")))
        .expect("set source_path");

    // Delete the source file.
    std::fs::remove_file(&file_path).expect("remove file");

    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    let result = prepare_referenced_file(&storage, &blob_store, &hex_hash, None, None).await;

    assert!(result.is_err(), "expected error for missing source");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not found"),
        "error should mention 'not found', got: {err}"
    );
}

#[tokio::test]
async fn prepare_referenced_changed_content() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let original = b"original content";
    let (file_path, hex_hash) = write_temp_file(tmp.path(), "changed.txt", original);

    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    storage
        .put_file_object(
            &hex_hash,
            original.len() as u64,
            "text/plain",
            "changed.txt",
            original,
        )
        .expect("put file object");
    storage
        .set_file_object_source_path(&hex_hash, Some(file_path.to_str().expect("utf-8 path")))
        .expect("set source_path");

    // Replace the file with different content (same length to avoid
    // catching a size mismatch — we want the hash check to fail).
    let modified = b"MODIFIED CONTENT"; // same length as "original content"
    assert_eq!(modified.len(), original.len(), "same length");
    std::fs::write(&file_path, modified).expect("write modified file");

    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    let result = prepare_referenced_file(
        &storage,
        &blob_store,
        &hex_hash,
        Some(&hex_hash), // expects original hash
        None,
    )
    .await;

    assert!(result.is_err(), "expected error for changed content");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("hash mismatch"),
        "error should mention 'hash mismatch', got: {err}"
    );
}

#[tokio::test]
async fn prepare_referenced_replaced_by_directory() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data = b"will be replaced by dir";
    let (file_path, hex_hash) = write_temp_file(tmp.path(), "dir_replacement.txt", data);

    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    storage
        .put_file_object(
            &hex_hash,
            data.len() as u64,
            "text/plain",
            "dir_replacement.txt",
            data,
        )
        .expect("put file object");
    storage
        .set_file_object_source_path(&hex_hash, Some(file_path.to_str().expect("utf-8 path")))
        .expect("set source_path");

    // Remove the file and create a directory in its place.
    std::fs::remove_file(&file_path).expect("remove file");
    std::fs::create_dir(&file_path).expect("create dir at same path");

    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    let result = prepare_referenced_file(&storage, &blob_store, &hex_hash, None, None).await;

    assert!(result.is_err(), "expected error for directory replacement");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("directory"),
        "error should mention 'directory', got: {err}"
    );
}

#[tokio::test]
async fn prepare_referenced_replaced_by_symlink() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data = b"will be replaced by symlink";
    let (file_path, hex_hash) = write_temp_file(tmp.path(), "symlink_target.txt", data);

    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    storage
        .put_file_object(
            &hex_hash,
            data.len() as u64,
            "text/plain",
            "symlink_target.txt",
            data,
        )
        .expect("put file object");
    storage
        .set_file_object_source_path(&hex_hash, Some(file_path.to_str().expect("utf-8 path")))
        .expect("set source_path");

    // Remove the file and create a symlink pointing elsewhere.
    std::fs::remove_file(&file_path).expect("remove file");
    let target = tmp.path().join("other_file.txt");
    std::fs::write(&target, b"other content").expect("write other file");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &file_path).expect("create symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &file_path).expect("create symlink");

    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    let result = prepare_referenced_file(&storage, &blob_store, &hex_hash, None, None).await;

    assert!(result.is_err(), "expected error for symlink replacement");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("symlink"),
        "error should mention 'symlink', got: {err}"
    );
}

#[tokio::test]
async fn prepare_referenced_read_failure() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let data = b"unreadable file";
    let (file_path, hex_hash) = write_temp_file(tmp.path(), "unreadable.txt", data);

    let storage = Arc::new(Storage::memory().expect("in-memory storage"));
    storage
        .put_file_object(
            &hex_hash,
            data.len() as u64,
            "text/plain",
            "unreadable.txt",
            data,
        )
        .expect("put file object");

    let blob_store: Arc<iroh_blobs::api::Store> =
        Arc::new(iroh_blobs::store::mem::MemStore::new().into());

    // Remove read permission from the file.
    let mut perms = std::fs::metadata(&file_path)
        .expect("get metadata")
        .permissions();
    // Clear existing permissions and set to no permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o000); // no permissions at all
    }
    #[cfg(not(unix))]
    {
        perms.set_readonly(false);
    }
    std::fs::set_permissions(&file_path, perms).expect("set permissions");
    // On Linux, the owner can always change permissions back, but the
    // *other* categories are removed.  The test verifies that the
    // function returns an error — the exact error message depends on
    // platform but should relate to reading/permission.

    let result = prepare_referenced_file(&storage, &blob_store, &hex_hash, None, None).await;

    assert!(
        result.is_err(),
        "expected error for unreadable file, got {:?}",
        result
    );
}

// ── UploadLimiter unit tests ──────────────────────────────────────

/// Create a test [`UploadLimiter`] with small, deterministic limits.
fn upload_limiter() -> UploadLimiter {
    UploadLimiter::new(UploadLimitsConfig {
        max_active_uploads: 2,
        max_uploads_per_peer: 1,
        max_queued_uploads: 2,
        max_concurrent_verifications: 1,
        request_timeout: Duration::from_secs(10),
    })
}

#[test]
fn upload_limiter_admits_and_rejects_on_queue_depth() {
    let limiter = upload_limiter();

    // First permit from peer-a — should succeed.
    let a1 = limiter.try_enqueue("peer-a").expect("first enqueue");
    assert_eq!(a1.peer(), "peer-a");

    // Second permit from peer-b — should succeed (depth is 2).
    let b1 = limiter.try_enqueue("peer-b").expect("second enqueue");

    // Third permit from peer-c — should be QueueFull (depth reached).
    match limiter.try_enqueue("peer-c").unwrap_err() {
        UploadError::QueueFull => {}
        other => panic!("expected QueueFull, got {other:?}"),
    }

    // Dropping a1 frees a queue slot.
    drop(a1);

    // Now peer-c can enqueue.
    let _c1 = limiter.try_enqueue("peer-c").expect("after drop");
    drop(b1);
    drop(_c1);
}

#[test]
fn upload_limiter_per_peer_limit() {
    let limiter = upload_limiter(); // max_uploads_per_peer: 1

    let first = limiter.try_enqueue("alice").expect("first alice");

    // Second from same peer should hit the per-peer limit.
    match limiter.try_enqueue("alice").unwrap_err() {
        UploadError::PeerLimitReached => {}
        other => panic!("expected PeerLimitReached, got {other:?}"),
    }

    // Different peer should succeed.
    let _bob = limiter.try_enqueue("bob").expect("bob enqueues");

    // After dropping alice's first, alice can enqueue again.
    drop(first);
    let _alice2 = limiter.try_enqueue("alice").expect("alice after drop");
    drop(_bob);
    drop(_alice2);
}

#[tokio::test]
async fn upload_limiter_global_active_cap() {
    let limiter = upload_limiter(); // max_active_uploads: 2, per-peer: 1

    // Enqueue and start two from different peers.
    let a = limiter.try_enqueue("a").unwrap().start().await.unwrap();
    let b = limiter.try_enqueue("b").unwrap().start().await.unwrap();

    // Third peer enqueues but start must wait (global cap at 2).
    let c_queued = limiter.try_enqueue("c").unwrap();
    let waiter = tokio::spawn(async move { c_queued.start().await.unwrap() });
    assert!(!waiter.is_finished(), "should be waiting for global slot");

    // Drop one — waiter should proceed.
    drop(a);
    let _c = waiter.await.unwrap();
    drop(b);
    drop(_c);
}

#[tokio::test]
async fn upload_limiter_per_peer_active_cap() {
    // Use per-peer=1, global=3: the bottleneck is the per-peer limit.
    // With per-peer=1, Alice can have at most 1 request (queued + active
    // combined).  We test that when Alice has 1 active, a second from
    // Alice enqueues into the queue but blocks on the per-peer semaphore
    // at start() time.
    let limiter = UploadLimiter::new(UploadLimitsConfig {
        max_active_uploads: 3,
        max_uploads_per_peer: 2,
        max_queued_uploads: 3,
        max_concurrent_verifications: 1,
        request_timeout: Duration::from_secs(10),
    });

    // Alice's first request starts (queued → active).
    let alice1 = limiter.try_enqueue("alice").unwrap().start().await.unwrap();

    // Alice's second request enqueues (fits in per-peer=2) but start
    // acquires a per-peer semaphore with 2 permits.  Since Alice already
    // holds 1, the second must wait until the first releases.
    let alice2 = limiter.try_enqueue("alice").unwrap();
    let waiter = tokio::spawn(async move { alice2.start().await.unwrap() });
    assert!(!waiter.is_finished(), "should be waiting for per-peer slot");

    // Drop alice1 — waiter proceeds.
    drop(alice1);
    let _alice2 = waiter.await.unwrap();
    drop(_alice2);
}

#[test]
fn upload_limiter_verification_budget() {
    let limiter = upload_limiter(); // max_concurrent_verifications: 1

    let v1 = limiter
        .try_acquire_verification()
        .expect("first verification");
    assert!(
        limiter.try_acquire_verification().is_err(),
        "second should be busy"
    );
    drop(v1);
    assert!(
        limiter.try_acquire_verification().is_ok(),
        "should succeed after drop"
    );
}

#[test]
fn upload_limiter_release_on_drop() {
    let limiter = upload_limiter(); // max_queued_uploads: 2

    let a = limiter.try_enqueue("a").unwrap();
    let b = limiter.try_enqueue("b").unwrap();

    // Queue is full.
    assert!(limiter.try_enqueue("c").is_err());

    // Dropping 'a' without starting releases its queue slot.
    drop(a);
    let _c = limiter.try_enqueue("c").expect("queue slot freed");

    // Dropping all should leave everything clean.
    drop(b);
    drop(_c);

    // Sanity: can enqueue again.
    let _d = limiter.try_enqueue("d").expect("clean state");
    drop(_d);
}

#[test]
fn upload_limiter_config_accessors() {
    let config = UploadLimitsConfig {
        max_active_uploads: 10,
        max_uploads_per_peer: 3,
        max_queued_uploads: 50,
        max_concurrent_verifications: 5,
        request_timeout: Duration::from_secs(120),
    };
    let limiter = UploadLimiter::new(config.clone());
    let got = limiter.config();
    assert_eq!(got.max_active_uploads, 10);
    assert_eq!(got.max_uploads_per_peer, 3);
    assert_eq!(got.max_queued_uploads, 50);
    assert_eq!(got.max_concurrent_verifications, 5);
    assert_eq!(got.request_timeout, Duration::from_secs(120));
}

#[test]
fn upload_error_display() {
    assert_eq!(UploadError::QueueFull.to_string(), "upload queue is full");
    assert_eq!(
        UploadError::PeerLimitReached.to_string(),
        "per-peer upload limit reached"
    );
    assert_eq!(
        UploadError::VerificationBusy.to_string(),
        "verification concurrency limit reached"
    );
}

// ── Ring-based authorization (FILE-01, requires `net` feature) ──

/// Helper: create a named ring for the profile, add the requester, and
/// grant a Read association on the given content hash.
fn grant_ring_read(storage: &Storage, requester_id: &FriendId, content_hash: &str) -> i64 {
    let ring_id = storage
        .create_ring("owner-profile-id", "friends-ring", false)
        .expect("create ring");
    storage
        .add_ring_member(ring_id, requester_id.as_str())
        .expect("add ring member");
    storage
        .set_ring_permission(ring_id, content_hash, RingPermission::Read)
        .expect("set ring read");
    ring_id
}

#[tokio::test]
async fn ring_member_in_ring_allowed_without_friendship() {
    let metadata_id = "file-ring-1";
    let content_hash = "11".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let requester = requester_pk();
    let requester_id = FriendId::from_public_key(requester);

    // Requester is NOT a friend, but belongs to a ring with Read on this file.
    grant_ring_read(&storage, &requester_id, &content_hash);

    let (handler, _blob_store) = test_handler(storage, friends);
    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");
    let request = make_request(metadata_id, &content_hash, row.version);

    let response = handler.check_permission(&requester, &request).await;
    assert!(
        matches!(response, FileAccessResponse::Granted(_)),
        "ring member without friendship should be granted, got {response:?}"
    );
}

#[tokio::test]
async fn ring_stranger_denied() {
    let metadata_id = "file-ring-2";
    let content_hash = "22".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let member_pk = requester_pk();
    let stranger_pk = requester_pk();
    let member_id = FriendId::from_public_key(member_pk);
    let stranger_id = FriendId::from_public_key(stranger_pk);

    // Only the member is in the ring.
    grant_ring_read(&storage, &member_id, &content_hash);

    let (handler, _blob_store) = test_handler(storage, friends);
    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");
    let request = make_request(metadata_id, &content_hash, row.version);

    // Member in ring is allowed.
    let member_resp = handler.check_permission(&member_pk, &request).await;
    assert!(
        matches!(member_resp, FileAccessResponse::Granted(_)),
        "ring member should be granted, got {member_resp:?}"
    );
    // Stranger (not friend, not in ring) is denied.
    let stranger_resp = handler.check_permission(&stranger_pk, &request).await;
    assert_eq!(
        stranger_resp,
        FileAccessResponse::PermissionDenied,
        "stranger should be denied"
    );
    let _ = stranger_id;
}

#[tokio::test]
async fn ring_membership_change_revokes_access_at_request_time() {
    let metadata_id = "file-ring-3";
    let content_hash = "33".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let requester = requester_pk();
    let requester_id = FriendId::from_public_key(requester);

    let ring_id = grant_ring_read(&storage, &requester_id, &content_hash);

    let (handler, _blob_store) = test_handler(storage, friends);
    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");
    let request = make_request(metadata_id, &content_hash, row.version);

    // Allowed while a member.
    let before = handler.check_permission(&requester, &request).await;
    assert!(
        matches!(before, FileAccessResponse::Granted(_)),
        "member should be granted, got {before:?}"
    );

    // Remove membership — the next request-time check must deny
    // (no stale catalogue state).
    handler
        .storage
        .remove_ring_member(ring_id, requester_id.as_str())
        .expect("remove ring member");
    let after = handler.check_permission(&requester, &request).await;
    assert_eq!(
        after,
        FileAccessResponse::PermissionDenied,
        "removed member should be denied at request time"
    );
}

#[tokio::test]
async fn open_ring_grants_read_to_any_peer() {
    let metadata_id = "file-ring-4";
    let content_hash = "44".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let anonymous = requester_pk();
    let anonymous_id = FriendId::from_public_key(anonymous);

    // Open ring with a Read association on the file — any peer may read.
    let ring_id = storage
        .create_ring("owner-profile-id", "open", true)
        .expect("create open ring");
    storage
        .set_ring_permission(ring_id, &content_hash, RingPermission::Read)
        .expect("set open ring read");

    let (handler, _blob_store) = test_handler(storage, friends);
    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");
    let request = make_request(metadata_id, &content_hash, row.version);

    let response = handler.check_permission(&anonymous, &request).await;
    assert!(
        matches!(response, FileAccessResponse::Granted(_)),
        "open ring should grant read to any peer, got {response:?}"
    );
    let _ = anonymous_id;
}

#[tokio::test]
async fn ring_deny_grant_still_wins_over_ring() {
    let metadata_id = "file-ring-5";
    let content_hash = "55".repeat(32);
    let (storage, friends) = setup_storage_with_file(metadata_id, &content_hash, true);
    let requester = requester_pk();
    let requester_id = FriendId::from_public_key(requester);

    // Ring grants Read, but an explicit per-peer deny exists for the
    // same file.  The explicit deny must win.
    grant_ring_read(&storage, &requester_id, &content_hash);
    storage
        .grant_permission(
            &content_hash,
            "owner-profile-id",
            requester_id.as_str(),
            "deny",
            None,
        )
        .expect("grant explicit deny");

    let (handler, _blob_store) = test_handler(storage, friends);
    let row = handler
        .storage
        .get_shared_file_by_metadata_id("owner-profile-id", metadata_id)
        .expect("get shared file")
        .expect("shared file exists");
    let request = make_request(metadata_id, &content_hash, row.version);

    let response = handler.check_permission(&requester, &request).await;
    assert_eq!(
        response,
        FileAccessResponse::PermissionDenied,
        "explicit deny grant must win over ring read"
    );
}
