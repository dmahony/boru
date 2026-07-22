# Message Storage Design

## Overview

All persistent data lives under a single **data directory**. The default path
depends on the environment:

| Precedence | Source |
|---|---|
| 1 (highest) | `--data-dir` CLI flag |
| 2 | `BORU_DATA_DIR` environment variable (also checks legacy `BORU_CHAT_DATA_DIR`) |
| 3 | `$XDG_DATA_HOME/boru` (typically `~/.local/share/boru/`) |
| 4 (fallback) | `$PWD/.boru` |

Legacy paths (`~/.local/share/boru-chat/`, `$PWD/.boru-chat`) and the legacy
`BORU_CHAT_DATA_DIR` env var are also checked for backward compatibility.

On Unix the data directory and its SQLite database are created with restrictive
permissions: `0o700` for the directory, `0o600` for the database file.

---

## On-Disk File Layout

```
<data_dir>/
├── boru.db               # SQLite relational storage (V2, current)
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
| `boru.db` | SQLite V2 | `storage::Storage` | Primary relational store: inbox, outbox, contacts, sync cursors, file objects, attachments, shared files, collections, permissions, downloads, profile manifest state |
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
linear migration path.

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

### Remote catalogue projections

Remote catalogue storage currently reuses the V2 relational tables rather than
creating a separate cache database:

| Data | Storage | Semantics |
|---|---|---|
| Peer/revision/generated/fetched metadata | `profile_manifest_state` keyed by the remote public-key string | `revision` is the advertised monotonic revision; `manifest_hash` stores the generated timestamp string in the current implementation; `created_at_ms` is the local fetch time |
| Remote file projection | `file_objects` plus `shared_files` keyed by remote profile and `metadata_id` | Stores safe display metadata, content hash, size, MIME type, and description; no source path or permission row is imported |
| Remote collections | `file_collections` keyed by remote profile | Stores collection display metadata for local browsing |

`Storage::replace_remote_catalogue` is called only after the client validates
the catalogue signature, owner identity, fields, limits, duplicate IDs/hashes,
and collection references. It upserts the entries returned by the latest
snapshot. The cache is a local display/reconciliation projection: it is not an
authorization source, and the current replacement path does not itself remove
older rows that are absent from a newer snapshot. Callers must use a live
`/boru-file-access/1` request before transferring bytes.

The `shared_file_permissions.expires_at_ms` column stores optional grant
expiry metadata. Permission evaluation and download authorization are separate
from cached catalogue reads; descriptor issuance re-checks the live permission
rows and the descriptor itself has an enforced expiry.

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
- The `BORU_FILES_DIR` env var can override the files root (legacy `BORU_CHAT_FILES_DIR` also accepted).

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
