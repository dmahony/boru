# Message Storage Design

## Overview

All persistent data lives under a single **data directory**. The default path
depends on the environment:

| Precedence | Source |
|---|---|
| 1 (highest) | `--data-dir` CLI flag |
| 2 | `BORU_CHAT_DATA_DIR` environment variable |
| 3 | `$XDG_DATA_HOME/boru-chat` (typically `~/.local/share/boru-chat/`) |
| 4 (fallback) | `$PWD/.boru-chat` |

On Unix the data directory and its SQLite database are created with restrictive
permissions: `0o700` for the directory, `0o600` for the database file.

---

## On-Disk File Layout

```
<data_dir>/
├── boru.db               # SQLite relational storage (V5, current)
├── chat_history.json      # Per-room chat history (legacy JSON, still active)
├── outbox.json            # Outgoing message delivery state (legacy JSON, still active)
├── conversations.json     # Conversation metadata (JSON)
├── rooms.json             # Room history / topic registry (JSON)
├── friends.json           # Friends list (JSON)
├── friend_requests.json   # Friend request state (JSON)
├── mailbox.json           # Encrypted offline-message delivery (JSON)
├── settings.json          # UI / app settings (JSON)
├── user_profile.json      # Profile settings + shared file metadata (JSON)
├── secret_key.txt         # Node identity secret key (hex-encoded)
│
├── message_store.db       # Legacy SQLite store (migration source, read-only)
│
└── files/                 # Per-user image store
    ├── <user-hash1>/
    │   ├── <content-hash>.jpg
    │   ├── <content-hash>.png
    │   └── ...
    └── <user-hash2>/
        └── ...
```

### File Descriptions

| File | Format | Module | Purpose |
|---|---|---|---|
| `boru.db` | SQLite V5 | `storage::Storage` | Primary relational store: inbox, outbox, contacts, sync cursors, file objects, attachments, shared files, collections, permissions, downloads, profile manifest state, and transactional outgoing DMs |
| `message_store.db` | SQLite V1 | `store::MessageStore` | Legacy store, now a migration source. Read by `Storage::import_legacy_db()` |
| `chat_history.json` | JSON V1 | `chat_history::ChatHistoryStore` | Per-room chat message history. Still the active frontend persistence layer |
| `outbox.json` | JSON V1 | `outbox::OutboxStore` | Outgoing message queue and delivery state. Still active |
| `conversations.json` | JSON V1 | `conversations::ConversationStore` | Conversation metadata (last message, unread count, mute/archive/delete flags) |
| `rooms.json` | JSON V1 | `room_history::RoomHistoryStore` | Room topic registry for reopening rooms |
| `friends.json` | JSON V1 | `friends::FriendsStore` | Friend contact list with per-peer addresses |
| `friend_requests.json` | JSON V1 | `friend_request::FriendRequestStore` | Pending/accepted/declined/cancelled friend requests |
| `mailbox.json` | JSON V1 | `mailbox::MailboxStore` | Encrypted offline-message envelopes for direct-message delivery |
| `settings.json` | JSON | `AppSettings` | UI preferences (theme, etc.) |
| `user_profile.json` | JSON V1 | `user_profile::UserProfile` | Display name, sharing settings, shared file metadata |
| `secret_key.txt` | hex |  | Node identity key (generated on first run) |

---

## SQLite Storage (`boru.db`)

### Connection & Safety

- **WAL journal mode** — crash-safe writes, concurrent reads during writes.
- **`synchronous = NORMAL`** — balances crash safety with write throughput.
- **`busy_timeout = 5000`** — 5-second wait before failing on lock contention.
- **Integrity check** — `PRAGMA integrity_check` runs on every `Storage::open()`.
  - A corrupt database is never silently repaired; it returns a clear error.
- **Foreign keys** — enforced via `PRAGMA foreign_keys = ON`.
- **File permissions** — `0o600` (owner-only) on Unix.

### Crash Recovery

On every `Storage::open()`:

1. **Sent→Pending reset** — outbox rows left in `Sent` status by a crash are
   reset to `Pending` with `last_error_code = 'crash_recovered'` and their
   retry timestamp set to now, so the delivery engine retries them immediately.
2. **Stale timestamp reset** — `Pending` rows with `next_attempt_at_ms` in the
   future are reset to now so they become due immediately.
3. **Preserved ACKs** — rows already in `Acked` status are never touched.

### Schema: Version 1 (message delivery)

The V1 schema is identical to the legacy `MessageStore` schema and provides a
linear migration path. See [`offline-direct-messaging.md`](offline-direct-messaging.md)
for the delivery state machine, retry policy, ack semantics, and ordering
guarantees built on top of this schema.

| Table | Purpose | Key columns |
|---|---|---|
| `inbox` | Received messages | `msg_id` (BLOB PK), `conversation_id`, `author_user_id`, `ciphertext`, `signature`, `created_at_ms`, `acked_at_ms` |
| `outbox` | Outgoing message delivery | `(msg_id, recipient_device_id)` primary key, `status` (0=Pending,1=Sent,2=Acked,3=Expired), `attempts`, `next_attempt_at_ms` |
| `contacts` | Known peer identities | `(user_id, device_id)` primary key, `endpoint_addr`, `identity_key`, `last_seen_ms`, `expires_at_ms` |
| `sync_cursor` | Per-peer sync state | `peer_device_id` (PK), `last_seen_msg_clock`, `last_sync_at_ms` |

### Schema: Version 2 (content-addressed files)

Extends V1 with file-object storage and sharing infrastructure.

| Table | Purpose | Key columns |
|---|---|---|
| `file_objects` | Content-addressed immutable file store | `content_hash` (TEXT PK, blake3 hex), `size`, `mime_type`, `filename`, `data` (inline BLOB), `blob_hash` (iroh-blobs ref) |
| `message_attachments` | Links a message to file objects | `(event_id, content_hash)` UNIQUE, `position` for ordering |
| `shared_files` | Profile-offered files | `(content_hash, profile_user_id)` PK, `offered` flag, `metadata_id` |
| `file_collections` | Named groups of shared files | Auto-increment `id`, `(profile_user_id, name)` UNIQUE |
| `file_collection_items` | Membership in a collection | `(collection_id, content_hash)` PK with ON DELETE CASCADE |
| `shared_file_permissions` | Per-peer grants on shared files | `(content_hash, grantor, grantee, permission)` PK, optional `expires_at_ms` |
| `downloads` | Durable download state machine | Auto-increment `id`, `state` (queued/active/paused/completed/failed), `bytes_downloaded`, retry tracking |
| `profile_manifest_state` | Manifest revision tracking | `user_id` PK, monotonically increasing `revision`, `manifest_hash` |

### Remote catalogue and download state

The SQLite file stores the authoritative local file objects, shared-file offers,
collections, per-peer permission grants, and manifest revision. It does not
store a reusable remote access ticket. A remote catalogue is a signed,
requester-specific snapshot derived from these rows at request time; the
frontend may cache the last verified snapshot per peer, but that cache is not an
authority and is never reused for another requester.

The manifest revision is monotonic and changes whenever the offered catalogue
changes. A revision notification is only a refresh hint. Retrieval returns
`NotModified` when the caller's known revision is current. File access still
re-checks the current `offered` flag, block/contact relationship, permission,
availability, content hash, and file revision before creating a descriptor.

The `downloads` table records durable local operation state, progress, retries,
and pause/cancel/failure status. Blob bytes may live in the iroh-blobs store;
large file rows can retain a `blob_hash` reference rather than inline data.
Temporary output files are not authoritative storage. A completed download is
installed only after size and BLAKE3 verification and atomic rename. Pause or
cancellation removes the temporary output, while verified chunks retained by
the blob store may be reused on a later attempt; byte-range resume of the
partial destination is not supported.

### Schema: Version 3 (file verification)

Extends V2 with file availability tracking.

| Table | Purpose | Key columns |
|---|---|---|
| `file_verification` | Per-file availability state | `(content_hash, profile_user_id)` PK, `availability` (Unverified/Available/Missing/Changed), `verified_at_ms`, `original_hash`, `original_size` |

Tracks whether the source file is available, missing, or changed since the last
verification. The `original_hash` and `original_size` are preserved even when
the file changes on disk, requiring explicit owner action to accept a new
version.

### Schema: Version 4 (replacement tracking + operations)

Extends V3 with file replacement tracking, storage management, and operation
progress.

| Table | Purpose | Key columns |
|---|---|---|
| `file_replacements` | File version replacement history | `(replaced_hash, replacement_hash)` PK, `profile_user_id`, `replaced_at_ms` |
| `cleanup_operations` | Unreferenced file cleanup tracking | Auto-increment `id`, `content_hash`, `state` (pending/in_progress/completed/failed), `bytes_freed` |
| `operation_progress` | Long-running operation progress | `id` (TEXT PK), `kind`, `stage`, `bytes_processed`, `total_bytes`, `status` (running/completed/failed/cancelled), `progress_pct` |

Additionally, V4 adds a `revision` column to `shared_files` for versioning.

### Current Schema Version

The database is currently at **V5** (`CURRENT_SCHEMA_VERSION = 5`). V5 adds
`dm_conversations`, `dm_sender_sequences`, `dm_messages`, and `dm_outbox`.
`Storage::queue_outgoing_dm` creates the conversation, allocates a persistent
sender sequence, signs the logical message, seals the mailbox envelope, and
inserts the visible message plus exact retry envelope in one transaction.

### Migration System

- Schema version tracked in `schema_version` table (`version` INTEGER PK, `applied_at_ms`).
- Migrations are idempotent (use `IF NOT EXISTS`, `INSERT OR IGNORE`).
- Each migration runs in its own transaction.
- **Forward-only** — no downgrade path.
- **Future-schema guard** — opening a database with a version higher than
  `CURRENT_SCHEMA_VERSION` returns a clear error instead of silently dropping
  tables or data.
- **Partial migration recovery** — if a migration crashes mid-way, the next
  `open()` re-runs only the unapplied migrations (already-applied versions
  are skipped via `schema_version`).
- **Legacy JSON→SQLite migration** — `import_legacy_db()` reads the old
  `message_store.db` schema and copies inbox, outbox, contacts, and sync
  cursors into the new storage. Re-import is idempotent (`INSERT OR IGNORE`
  prevents duplicates).

---

## Message Persistence Semantics

### Exactly-Once Local Persistence

- **`INSERT … ON CONFLICT(msg_id) DO NOTHING`** — inserting a message with a
  `msg_id` that already exists in the `inbox` table is silently ignored.
- The `(msg_id, recipient_device_id)` primary key on `outbox` provides the
  same guarantee for outbound entries.
- These constraints survive restarts: reopening the database preserves all
  rows exactly as they were.

### At-Least-Once Transport

- Outbox entries are created with `status = Pending` and `attempts = 0`.
- On each delivery attempt `record_attempt()`:
  - Increments `attempts`.
  - Sets `status = Sent`.
  - Records `next_attempt_at_ms` for retry scheduling.
  - Does NOT touch rows already in `Acked` state.
- After a crash, `Sent` rows are reset to `Pending` by crash recovery so no
  message falls through a gap.
- `fetch_due_outbox()` returns rows where `status != Acked AND status != Expired
  AND next_attempt_at_ms <= now`.
- Messages whose inbox row has expired (`expires_at_ms <= now`) have their
  corresponding outbox rows set to `Expired`.
- **Key distinction**: local persistence is *exactly-once* (SQL constraints
  prevent duplication), while transport delivery is *at-least-once* (retries
  with ACK dedup at the recipient).

### Message Ordering

- **Inbox**: queried with `ORDER BY created_at_ms DESC`.
- **Outbox due-queue**: returned by `fetch_due_outbox()` in FIFO order (no
  explicit ORDER BY; SQLite returns rows in `rowid` order which matches
  insertion order when no deletions occur).
- **Message attachments**: ordered by `position` column.

### Deletion / Tombstone Semantics

Deletion state is managed in the `message_tombstones` table, added in Step 12.
See [`docs/storage-redesign.md`](storage-redesign.md#deletion-and-tombstone-semantics-step-12)
for a full description of tombstone insertion, local vs. remote deletion,
conversation-level deletion, and edge cases.

---

## Transport Layer

### At-Rest Encryption Caveat

- **Inbox ciphertext**: messages are stored as opaque ciphertext blobs
  (encrypted by the sender). The storage layer never inspects or decrypts the
  payload. However, the ciphertext is **plaintext at rest** relative to the
  filesystem — anyone who can read the `boru.db` file can read the encrypted
  payload bytes (though they cannot decrypt without the recipient's key).
- **Outbox data**: similarly stored as ciphertext blobs that the storage layer
  does not decrypt.
- **File objects**: inline `data` column stores raw bytes. For user-uploaded
  images this may be plaintext image data. Large files use a `blob_hash`
  reference to iroh-blobs instead.
- **Encrypted transport**: wire transport uses iroh's QUIC-based encryption
  (TLS 1.3 / QUIC crypto). On-the-wire messages are always encrypted between
  peers. The at-rest storage is separate: the database is not encrypted at
  the file level (no SQLite encryption extension).

### Transport Protocols

| Protocol | ALPN | Purpose | Persistence |
|---|---|---|---|
| Gossip | `/iroh-chat-gossip/1` | Room-based broadcast | None (transient) |
| Inbox | `/iroh-chat-inbox/1` | Direct message sync + signed deletions | Inbox event emission |
| Backfill | `/iroh-chat-backfill/1` | Historical message requests | None (reads from ChatHistoryStore) |
| Whisper | `/iroh-chat-whisper/1` | Private 1:1 QUIC channels | None (transient) |

---

## Content-Addressed Attachments (V2)

File objects are identified by their blake3 content hash (64-character hex
string). This provides:

- **Deduplication** — the same file shared in two messages is stored once.
- **Integrity** — content hash is the primary key; tampering changes the hash
  and creates a different object.
- **No local filesystem exposure** — remote peers never receive filesystem
  paths; only content hashes are exchanged.

### Attachment Types

| Type | Table(s) | Ownership |
|---|---|---|
| Chat message attachment | `file_objects` + `message_attachments` | The chat message owns the attachment row |
| Profile-offered file | `file_objects` + `shared_files` | The user profile owns the offer |
| Downloaded file | `file_objects` + `downloads` | The download state machine owns the download row |

### File Integrity

The `file_objects` table stores `size` and `data` separately. Callers can
verify integrity by re-hashing the data and comparing to `content_hash`, or
comparing `data.len()` to `size`. Tests validate that corruption is detectable
after reopening the database.

---

## ImageStore (`files/` directory)

Images uploaded by users are stored outside SQLite, rooted at `<data_dir>/files/`.

```
<data_dir>/files/
├── <user-hash-64>/
│   ├── <content-hash-64>.jpg
│   └── <content-hash-64>.png
└── <user-hash-64>/
    └── ...
```

- User directories are keyed by blake3 hash of the user identifier (never the
  identifier itself as path component).
- Image filenames are content-addressed: `<blake3-hex>.<extension>`.
- File extensions are validated against an allow-list (`png`, `jpg`, `jpeg`,
  `gif`, `webp`, `bmp`); everything else becomes `.bin`.
- Images from `optimize_chat_image` always output JPEG; the store auto-detects
  JPEG magic bytes (`FF D8 FF`) and overrides the extension to `jpg`.
- Directories have `0o700` permissions on Unix; the store prevents symlink
  traversal by rejecting symlinked user directories.
- The `BORU_CHAT_FILES_DIR` env var can override the files root.

---

## Backup and Portability

- `boru.db` is a standard SQLite file in WAL mode. Backups should use
  `VACUUM INTO` or `.backup` to capture a consistent checkpoint.
- JSON files (`chat_history.json`, `outbox.json`, etc.) can be backed up
  at any time (atomic writes ensure each file is self-consistent).
- All paths are relative to the data directory; moving the entire directory
  to a new machine recreates the full application state.
- **Do NOT mix data directories between different application versions** —
  the forward-only migration system will refuse to open a database created
  by a newer version.

---

## Current Limitations & Future Work

1. **JSON compatibility layer** — `ChatHistoryStore` (JSON) and `OutboxStore`
   (JSON) still serve as the active persistence layer in the GUI. The SQLite
   `Storage` module is fully implemented and tested but not yet wired as the
   primary frontend store. A future integration pass will replace the JSON
   stores with SQLite-backed equivalents and redirect all reads/writes through
   `Storage`.

2. **No SQLite encryption** — the database file is unencrypted on disk. Anyone
   with filesystem access to the data directory can read ciphertext blobs.
   Transport-layer encryption (QUIC/TLS 1.3) protects messages in flight; at
   rest the storage depends on filesystem permissions.

3. **Tombstone pruning** — old tombstones accumulate indefinitely. A future
   step should add configurable TTL-based pruning (e.g. 90 days) — protocol
   replay windows already limit re-insertion risk.

4. **Batch tombstone protocol** — conversation-level deletion has local
   support (`delete_conversation`, `hard_delete_conversation`) but no
   corresponding batch protocol message for propagation to peers.

5. **Image format mismatch** — `ImageStore::save_image` preserves the original
   extension (via `safe_extension()`), but `optimize_chat_image` always emits
   JPEG. The store works around this with magic-byte detection, but the
   extension contract is not fully clean.

---

## Local Profile File Library

The local profile file library manages files that the user offers to share
with peers. It is implemented in `examples/iced_chat/file_library.rs` (state
and types) and `examples/iced_chat/file_library_ops.rs` (operations).

### Architecture

The file library has two storage modes:

| Mode | Description | Disk Behavior | Durability |
|------|-------------|---------------|------------|
| **Import** | File content is copied into the content-addressed managed store | Copy to `library_dir/<prefix>/<hash>` | Survives deletion of the original; uses extra disk space |
| **Reference** | Points to the original file on disk; no copy | Source path stored in `library_dir/.refs/<hash>` | Original must remain accessible |

### Module Structure

| File | Purpose |
|------|---------|
| `file_library.rs` | Types: `FileLibraryFilter`, `FileLibrarySort`, `LocalFileLibraryState`, `FileLibraryRow`, `AddFileState`, `StorageMode`, `ChangedFileAction`, `RemovalMode`, `CleanupCandidateRow`, `FileDetailData`, `PaginationState`, `OperationProgress`, file validation functions (`validate_file_for_library`, `validate_offer_metadata`), filter/sort application |
| `file_library_ops.rs` | Operations: hashing (`hash_file_streaming`), import (`import_file`), reference (`offer_referenced_file`), object reuse (`find_or_create_file_object`), change detection (`detect_changed_file`, `update_referenced_file_to_new_version`), removal (`remove_offer_from_profile`, `delete_imported_copy`), cleanup (`cleanup_unreferenced_imported_objects`), startup recovery (`startup_recovery`), path privacy (`sanitize_path_for_log`, `verify_row_safe_for_remote`) |

### Key Design Decisions

1. **Content-addressed storage** — Imported files are stored at
   `library_dir/<first-2-hex-chars>/<full-hex-hash>`. The 2-character prefix
   creates 256 buckets, keeping directory listings manageable even with
   millions of files.

2. **Private source path registry** — Referenced file source paths are stored
   in a side directory (`library_dir/.refs/<hash>`) rather than in the
   database. This ensures source paths are never exposed in DB dumps or
   backups.

3. **Object reuse** — Files with the same BLAKE3 hash and size share one
   `file_objects` row, regardless of whether the first instance was imported
   or referenced. This is enforced by the `shared_files` PK
   `(content_hash, profile_user_id)` and the `INSERT OR IGNORE` semantics of
   `put_file_object`.

4. **No local paths in remote data** — `FileLibraryRow` never exposes full
   source paths. The `display_filename` field contains only the user-chosen
   display name. The `verify_row_safe_for_remote()` function enforces this
   contract at the API boundary.

### File Hashing

- Uses BLAKE3 in streaming mode (64 KiB chunks, never loads whole file into
  memory).
- Supports cancellation via `CancellationToken` and progress reporting via
  `watch::Sender<HashProgress>`.
- Progress is reported every 1 MiB for large files.

### Import Workflow

```
1. Validate source file (exist, regular, readable, not symlink, non-zero)
2. Stream-hash with BLAKE3 (cancellable, with progress)
3. Compute managed path: library_dir/<prefix>/<hash>
4. Copy to temp file: library_dir/.tmp_<hash>.import
5. Verify copied file hash (streaming, no progress)
6. Atomic rename: .tmp_<hash>.import → <hash>
7. Insert/reuse file_object row in DB
8. Insert shared_file row
9. Assign to selected collections
10. Increment profile manifest revision
```

If the file object already exists (same hash+size), steps 4-6 are skipped.

### Startup Recovery

On application startup, `startup_recovery()` performs:

1. **Stale temp cleanup** — removes `.tmp_*.import` files left by crashes
2. **Operation recovery** — marks interrupted `operation_progress` rows as failed
3. **Orphan detection** — finds shared_files without file_objects and vice versa
4. **Existence checks** — verifies referenced file sources exist, imported
   managed files exist; marks missing files as `"Missing"`
5. **No auto-hashing** — specifically avoids re-hashing all referenced files
   to prevent startup delay

### Privacy Protections

- **`sanitize_path_for_log()`**: Redacts home directories (`~`), temp paths
  (`<temp>`), and library paths (`<library>`) from log output.
- **`verify_row_safe_for_remote()`**: Rejects `FileLibraryRow` entries where
  display_filename contains path separators, absolute path indicators, or
  the description contains sensitive patterns (`/home/`, `.ssh`, `secret`,
  `password`).
- **Bulk check** `verify_all_rows_safe_for_remote()`: Validates all rows at
  once, returning violations with row indices.

### What Is Not Yet Networked

**The local profile file library prepares and manages offered files. It does
not yet publish catalogues or serve downloads to peers.** Specifically:

- No catalogue publication — shared file metadata is not broadcast to peers
- No download transport — peers cannot request or receive file bytes
- No remote browsing — peers cannot see each other's file libraries
- No sync protocol — file library changes are not propagated to connected peers

These features are planned for future iterations. The current implementation
provides the local storage and management infrastructure on which the
networked features will be built.
