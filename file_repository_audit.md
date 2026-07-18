# File Repository APIs — Audit Report

**Task:** Inspect the actual completed storage and local file-library implementation.
**Date:** 2026-07-18
**Source:** `src/storage.rs`, `src/user_profile.rs`, `src/friends.rs`, `src/file_indexer.rs`, `src/chat_core.rs`, `src/whisper/mod.rs`

---

## 1. File Objects (`src/storage.rs`)

Content-addressed immutable file data. The single source of truth for file content.

### Backing Table: `file_objects`

| Column | Type | Notes |
|--------|------|-------|
| `content_hash` | TEXT PK | blake3 hash as hex |
| `size` | INTEGER | |
| `mime_type` | TEXT | |
| `filename` | TEXT | |
| `created_at_ms` | INTEGER | |
| `data` | BLOB | NULL for imported/blob-ref files |
| `blob_hash` | TEXT | iroh-blobs hash, NULL for inline files |
| `imported_from_peer` | TEXT | hex public key |
| `imported_at_ms` | INTEGER | |

### Existing APIs
- **`put_file_object(hash, size, mime, filename, data)`** — store. Idempotent (INSERT OR IGNORE).
- **`put_imported_file_object(hash, size, mime, filename, blob_hash, peer)`** — store imported/blob-referenced file.
- **`get_file_object(hash)`** → `Option<FileObject>` — lookup with inline data.
- **`file_object_exists(hash)`** → `bool` — existence check.
- **`delete_file_object(hash)`** → `bool` — delete, fails if FK references exist.

### Missing APIs
- **`file_object_has_references(hash)`** → `bool` — check whether any FK references exist across `message_attachments`, `shared_files`, `file_collection_items`, `shared_file_permissions`, `downloads` before deciding deletion is safe. Referenced by integration test `integration_db_record_without_associations`.
- **`list_file_objects(limit, offset)`** — general listing. Currently only accessible via individual hash lookups.
- **`query_file_objects_by_blob_hash(blob_hash)`** — look up by blob reference (for blob-download completion callbacks).
- **`get_imported_file_object(hash)`** → `Option<ImportedFileObject>` — lookup with blob_hash and source_peer separately (currently must read `FileObject` and check blob_hash manually).

---

## 2. Message Attachments

Links chat messages → file objects.

### Backing Table: `message_attachments`

| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER PK | autoincrement |
| `event_id` | INTEGER | local message event-id |
| `content_hash` | TEXT | FK → file_objects |
| `display_filename` | TEXT | |
| `position` | INTEGER | ordinal within message |

### Existing APIs
- **`attach_file_to_message(event_id, hash, display_filename, position)`** → `i64` — attach. FK constraint prevents orphan.
- **`get_message_attachments(event_id)`** → `Vec<MessageAttachment>` — list for a message.
- **`remove_message_attachment(id)`** → `bool` — remove by row id.
- **`find_messages_for_file(hash)`** → `Vec<u64>` — find messages referencing a file.

### Missing APIs
- None identified for current milestone. The attach/list/remove/find surface is complete.

---

## 3. Shared-File Offers

Profile-offered files published in ProfileUpdate gossip messages.

### Backing Table: `shared_files`

| Column | Type | Notes |
|--------|------|-------|
| `content_hash` | TEXT | FK → file_objects |
| `profile_user_id` | TEXT | hex public key |
| `metadata_id` | TEXT | stable id from `SharedFile.id` |
| `display_filename` | TEXT | |
| `description` | TEXT | nullable |
| `offered` | INTEGER | 0/1 |
| `created_at_ms` | INTEGER | |
| `updated_at_ms` | INTEGER | |

### Existing APIs
- **`upsert_shared_file(hash, profile, metadata_id, filename, description, offered)`** — create or update.
- **`list_shared_files(profile, offered_only)`** → `Vec<SharedFileRow>` — list for a profile.
- **`get_shared_file(profile, hash)`** → `Option<SharedFileRow>` — get one.

### Missing APIs
- **`delete_shared_file(hash, profile)`** → `bool` — remove the shared-file offer row without cascading to the file_object. Required by integration tests (`test_file_library_integration.rs` lines 89, 154, 191) and `file_library_ops.rs`.
- **`update_shared_file_metadata(hash, profile, display_filename, description, metadata_id)`** — update only metadata fields without touching the `offered` flag. Currently you must re-upsert the whole row. Required by `test_file_library_integration.rs` line 63.
- **`set_shared_file_offered(hash, profile, offered)`** — toggle the `offered` boolean in isolation. Required by `test_file_library_integration.rs` lines 76, 83.

---

## 4. File Collections

Named groups of shared files belonging to a profile.

### Backing Tables
- `file_collections` — named groups
- `file_collection_items` — membership

### Existing APIs
- **`ensure_collection(profile, name, description)`** → `i64` — create or return existing.
- **`list_collections(profile)`** → `Vec<FileCollection>` — list.
- **`add_to_collection(collection_id, hash, position)`** — add file.
- **`list_collection_items(collection_id)`** → `Vec<FileCollectionItem>` — list items.
- **`remove_from_collection(collection_id, hash)`** → `bool` — remove item.

### Missing APIs
- **`rename_collection(collection_id, new_name)`** → `bool` — update collection name. Required by `test_file_library_integration.rs` line 236 (“Rename collection”).
- **`delete_collection(collection_id)`** → `bool` — delete collection (cascade removes items via ON DELETE CASCADE). Required by `test_file_library_integration.rs` line 241.
- **`update_collection_description(collection_id, description)`** — update collection description.
- **`get_collection(collection_id)`** → `Option<FileCollection>` — single lookup by id (currently only list by profile).

---

## 5. Per-Peer Permissions

Discrete permission grants on shared files, per grantee.

### Backing Table: `shared_file_permissions`

| Column | Type | Notes |
|--------|------|-------|
| `content_hash` | TEXT | FK → file_objects |
| `grantor_user_id` | TEXT | |
| `grantee_user_id` | TEXT | |
| `permission` | TEXT | e.g. "read", "download" |
| `created_at_ms` | INTEGER | |
| `expires_at_ms` | INTEGER | nullable |

### Existing APIs
- **`grant_permission(hash, grantor, grantee, permission, expires_at_ms)`** — grant/upsert.
- **`revoke_permission(hash, grantor, grantee, permission)`** → `bool` — revoke specific.
- **`check_permission(hash, grantee, permission)`** → `bool` — check with expiry.
- **`list_permissions_for_grantee(grantee)`** → `Vec<SharedFilePermission>` — all grants to a peer.

### Missing APIs
- **`list_permissions_for_grantor(hash, grantor)`** — list grants on a specific file by the profile owner.
- **`list_permissions_for_file(hash)`** — list all grants on a file (across all grantors).
- **`revoke_all_permissions(hash, grantee)`** — revoke all permissions for a peer on a file.
- **`check_any_permission(hash, grantee)`** → `(has_any, has_expired)` — quick check with a single query.
- No permission model exists for **broadcast-gossip public rooms** (only per-file, per-peer grants on shared files).

---

## 6. Blocked Contacts

### Backing Storage
- `friends.json` via `FriendsStore`
- `FriendRelationship::Blocked` enum variant on `FriendRecord`
- `ChatCallbacks::is_blocked(peer)` trait method (defaults to `false`)
- Actual enforcement: `chat_core.rs` line 1517-1521 silently drops all messages from blocked peers

### Existing APIs
- **`FriendsStore::set_relationship(id, FriendRelationship::Blocked)`** — block via the general relationship setter.
- **`ChatCallbacks::is_blocked(peer)`** — check trait method.
- **`FriendRecord::relationship`** field — read current status.

### Missing APIs
- **No dedicated `FriendsStore::is_blocked(id) -> bool`** — callers must get the record and match the relationship manually.
- **No `FriendsStore::list_blocked()`** — no way to enumerate all blocked peers in a single call.
- **No unblock convenience method** (call `set_relationship(id, NotFriend)` manually).
- Block enforcement is per-peer-gossip-message level, not per-file. There is no file-access-level block check: if a blocked peer's file hash is known, there's nothing preventing access to it through the file transfer protocol. This should be verified at the file access handler level.

---

## 7. Catalogue / Profile Manifest Revision State

Monotonically increasing revision counter used to detect profile/collection changes.

### Backing Table: `profile_manifest_state`

| Column | Type | Notes |
|--------|------|-------|
| `user_id` | TEXT PK | |
| `revision` | INTEGER | monotonic counter |
| `manifest_hash` | TEXT | blake3 of serialised manifest |
| `created_at_ms` | INTEGER | |

### Existing APIs
- **`bump_manifest_revision(user_id, hash)`** → `u64` — increment and set hash in one call.
- **`get_manifest_state(user_id)`** → `Option<ProfileManifestState>` — read current state.

### Missing APIs
- **`get_manifest_revision(user_id)`** → `u64` — read only the revision number without the hash.
- **`manifest_revision_changed_since(user_id, known_revision)`** → `bool` — efficient change detection for peers comparing their known revision. Currently callers must read full state and compare.
- **Multi-profile support** — there's no `list_manifest_states()` to enumerate all known remote profiles' manifest states.

---

## 8. Remote-Safe File Views / Shared-File Visibility

How shared files are advertised to and viewed by remote peers.

### Wire Format
- `ProfileUpdate` gossip messages carry `Vec<SharedFileMeta>` — each with `id`, `filename`, `size`, `mime_type`, `modified_time`, `hash` (content hash).
- `SharedFileMeta` is the wire-safe representation (no local paths).
- `UserProfile::shared_files: Vec<SharedFileMeta>` is the in-profile advertisement list.

### Existing APIs
- **`SharedFile::to_shared_file_meta()`** — converts local file metadata to wire-safe format.
- **`SharedFile::is_announceable()`** — checks `over_limit` and `extension_blocked` flags.
- **`UserProfile::is_file_announce_allowed(size, extension)`** — profile-level filter.

### Missing APIs
- **Visibility filter by peer** — no API to compute a per-peer visible file list by combining shared files + permissions. Currently all offered files are broadcast to all peers. Permissions are stored but never applied at the profile-broadcast layer.
- **No `get_visible_files_for_peer(profile, peer)`** → `Vec<SharedFileRow>` — would need to join `shared_files` with `shared_file_permissions`.
- **No `is_file_visible_to_peer(hash, profile, peer)`** → `bool` — peer-level visibility check.

---

## 9. Durable Downloads

State machine for file transfers that survive restarts.

### Backing Table: `downloads`

| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER PK | |
| `content_hash` | TEXT | FK → file_objects |
| `remote_peer` | TEXT | |
| `state` | TEXT | queued/active/completed/failed |
| `bytes_downloaded` | INTEGER | |
| `total_bytes` | INTEGER | |
| `created_at_ms` | INTEGER | |
| `updated_at_ms` | INTEGER | |
| `last_error` | TEXT | |
| `retry_count` | INTEGER | |
| `next_retry_at_ms` | INTEGER | |

### Existing APIs
- **`create_download(hash, remote_peer, total_bytes)`** → `i64` — create in queued state.
- **`update_download_progress(id, bytes_downloaded, state)`** — update progress.
- **`fail_download(id, error, next_retry_at_ms)`** — mark failed.
- **`get_download(id)`** → `Option<Download>` — get by id.
- **`list_downloads_by_state(state)`** → `Vec<Download>` — list.

### Missing APIs
- **`list_downloads_for_peer(remote_peer, state_filter)`** — list downloads targeting a specific peer.
- **`list_downloads_for_hash(content_hash)`** — list downloads for a specific file.
- **`complete_download(id)`** — mark as completed (currently must call `update_download_progress` with state="completed").
- **`retry_download(id, next_retry_at_ms)`** — transition from failed → queued with retry scheduling.
- **`get_due_downloads(now_ms)`** → fetch downloads in failed state with `next_retry_at_ms <= now_ms`.
- **`prune_completed_downloads()`** — remove old completed download rows.

---

## 10. Object Availability / File Verification

Tracking whether a shared file's source is present on disk and valid.

### Missing APIs (no backing table exists!)
- **`set_file_availability(hash, profile, availability, last_verified_at_ms, hash, size)`** — currently referenced by integration tests but not implemented. The tests create these calls as planned/future APIs.
- **`get_file_availability(hash, profile)`** → returns some availability state — also not implemented.
- **`record_file_replacement(old_hash, new_hash, profile)`** — track that a file was replaced.

There is no `file_availability` table in the current schema. The integration tests (`test_file_library_integration.rs` lines 123-151, 294-305) test against these APIs as though they exist, but they will fail at compile time — these methods do not exist on `Storage`. This is the most significant gap found.

---

## 11. Imported and Referenced Files

### File Types
- **Inlined files** — `data` column is `Some(bytes)`. Used for small files (chat attachments).
- **Imported/blob-referenced files** — `data` is `None`, `blob_hash` is `Some(hash)`, `imported_from_peer` is set. For files fetched via iroh-blobs.
- **Referenced files** — `data` is `Some(empty)` or `None`, backed by a local file on disk. The `file_indexer` tracks local files.

### Existing APIs
- **`FileIndexer::scan()`** — scans the shared folder, returns `Vec<SharedFile>` metadata.
- **`FileIndexer::watch()`** — starts a `notify`-based filesystem watcher.
- **`put_imported_file_object(...)`** — stores a blob-reference file object.
- **`ImageStore`** — separate image-only content-addressed store at `<data_dir>/files/`.

### Missing APIs
- **No bridge between `FileIndexer` and `Storage`** — `FileIndexer::list_shared_files()` returns `Vec<SharedFile>` (domain model from `user_profile.rs`), but `Storage::list_shared_files()` returns `Vec<SharedFileRow>` (DB row model). There is no method to synchronise the two or to derive one from the other.
- **No lazy hash computation query** — no API to find files whose `hash` field is `None` and need blake3 computation.
- **No API for `hash_file(path) -> [u8; 32]`** — blake3 hashing is done ad-hoc via `blake3::Hasher` at call sites.

---

## Summary

### Data Model Completeness

| Feature | Schema | CRUD APIs | Wire Protocol | Integration Tests |
|---------|--------|-----------|---------------|-------------------|
| File objects | ✅ `file_objects` table | ✅ put/get/exists/delete | — | ✅ |
| Message attachments | ✅ `message_attachments` | ✅ attach/list/remove/find | Embedded in chat msg | ✅ |
| Shared-file offers | ✅ `shared_files` | ✅ upsert/list/get | `ProfileUpdate` → `SharedFileMeta` | ✅ |
| File collections | ✅ `file_collections` + `file_collection_items` | ✅ ensure/list/add-item/list-items/remove-item | — | ✅ |
| Per-peer permissions | ✅ `shared_file_permissions` | ✅ grant/revoke/check/list-grantee | — | ✅ |
| Blocked contacts | ✅ `FriendRelationship::Blocked` in friends.json | ❌ no dedicated `is_blocked`/`list_blocked` | Enforced at msg recv | ✅ (hostile input tests) |
| Profile manifest revision | ✅ `profile_manifest_state` | ✅ bump/get | — | ✅ |
| Durable downloads | ✅ `downloads` | ✅ create/update/fail/get/list-by-state | Whisper `FileTransfer` | ✅ |
| File availability/verification | ❌ No table exists | ❌ Methods don't exist on `Storage` | — | ❌ (test code references unimplemented APIs) |
| File replacement tracking | ❌ No table exists | ❌ Methods don't exist on `Storage` | — | ❌ (test code references unimplemented APIs) |

### APIs Referenced by Tests/Examples but NOT Implemented

These will cause **compile errors** if the test file is built:

1. `Storage::delete_shared_file(hash, profile)` — `test_file_library_integration.rs` lines 89, 154, 191
2. `Storage::update_shared_file_metadata(hash, profile, filename, description, metadata_id)` — `test_file_library_integration.rs` line 63
3. `Storage::set_shared_file_offered(hash, profile, offered)` — `test_file_library_integration.rs` lines 76, 83
4. `Storage::set_file_availability(hash, profile, availability, last_verified_at_ms, hash, size)` — `test_file_library_integration.rs` lines 123, 136, 299
5. `Storage::get_file_availability(hash, profile)` — `test_file_library_integration.rs` lines 126, 139, 294
6. `Storage::record_file_replacement(old_hash, new_hash, profile)` — `test_file_library_integration.rs` line 150
7. `Storage::rename_collection(id, name)` — `test_file_library_integration.rs` line 236
8. `Storage::delete_collection(id)` — `test_file_library_integration.rs` line 241
9. `Storage::file_object_has_references(hash)` — `test_file_library_integration.rs` line 340

### Key Architectural Observations

1. **No catalogue module exists.** The task description uses "catalogue" language but the actual code calls it "profile manifest state" (`profile_manifest_state`). The wire format is `SharedFileMeta` carried in `ProfileUpdate` gossip messages.

2. **Permissions are stored but not enforced at the file-transfer level.** The `shared_file_permissions` table exists with full CRUD, but there is no code path that checks `check_permission(hash, grantee, "download")` before serving a file through the file access handler or whisper protocol.

3. **No file availability/verification table exists.** The `file_objects` schema has `blob_hash`/`imported_from_peer`/`imported_at_ms` columns for imported files, but there is no general-purpose "is this file's source present and has it changed" tracking table. The integration tests expect `set_file_availability` / `get_file_availability` APIs that don't exist yet.

4. **The integration tests for file library (`test_file_library_integration.rs`) reference 9 APIs that do not exist on `Storage`.** These tests were written as a spec for Phase 23 but the storage methods were never added. The tests cannot compile without implementing these APIs first.

5. **`SharedFile` (domain model in `user_profile.rs`) vs `SharedFileRow` (DB row in `storage.rs`) are separate types** with no conversion functions between them. The domain model carries `path: PathBuf`, `hash`, `blob_id`, `over_limit`, `extension_blocked` — none of which are in the shared_file table. The DB model carries `content_hash`, `metadata_id`, `offered`, `created_at_ms`, `updated_at_ms` — some of which are not in the domain model.
