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
├── boru.db               # SQLite authoritative store (V21 schema; local FTS)
├── chat_history.json      # Legacy JSON — reads only (writes deprecated)
├── outbox.json            # Legacy JSON — reads only (writes deprecated)
├── conversations.json     # Legacy JSON — reads only (writes deprecated)
├── rooms.json             # Legacy JSON — reads only (writes deprecated)
├── friends.json           # Legacy JSON — reads only (writes deprecated)
├── friend_requests.json   # Legacy JSON — reads only (writes deprecated)
├── mailbox.json           # Legacy JSON — reads only (writes deprecated)
├── settings.json          # UI / app preferences (JSON, active)
├── user_profile.json      # Profile display name (JSON, active)
├── secret_key.txt         # Node identity secret key (hex-encoded)
│
├── message_store.db       # Legacy SQLite V1 — migration source (read-only)
│
└── files/                 # Per-user image store (content-addressed)
    ├── <user-hash1>/
    │   ├── <content-hash>.jpg
    │   ├── <content-hash>.png
    │   └── ...
    └── <user-hash2>/
        └── ...
```

### File Descriptions

| File | Format | Module | Purpose | Status |
|---|---|---|---|---|
| `boru.db` | SQLite V19 | `storage::Storage` | **Authoritative** — inbox, outbox, contacts, sync cursors, file objects, attachments, DM messages, outgoing messages, shared files, collections, permissions, downloads, profile state, tombstones, sync dedup, acknowledgements, groups, rings, directory ads, group encryption state, chat_messages (backfill history) | **Active** |
| `message_store.db` | SQLite V1 | `store::MessageStore` | Live chat-message history (`messages` table) — written by the GUI on every message. Also the legacy envelope store read by `Storage::import_legacy_db()` | **Active (chat history)** / Legacy source (envelopes) |
| `chat_history.json` | JSON V1 | `chat_history::ChatHistoryStore` | One-time migration input: `migrate_legacy_json()` imports entries into the SQLite `messages` table; `save()` is a deprecated no-op | Legacy (migration input only) |
| `outbox.json` | JSON V1 | `outbox::OutboxStore` | Outgoing message queue and delivery state | Legacy (reads only) |
| `conversations.json` | JSON V1 | `conversations::ConversationStore` | Conversation metadata (last message, unread count) | Legacy (reads only) |
| `rooms.json` | JSON V1 | `room_history::RoomHistoryStore` | Room topic registry | Legacy (reads only) |
| `friends.json` | JSON V1 | `friends::FriendsStore` | Friend contact list with per-peer addresses | Legacy (reads only) |
| `friend_requests.json` | JSON V1 | `friend_request::FriendRequestStore` | Pending/accepted/declined/cancelled friend requests | Legacy (reads only) |
| `mailbox.json` | JSON V1 | `mailbox::MailboxStore` | Encrypted offline-message envelopes | Legacy (reads only) |
| `settings.json` | JSON | `AppSettings` | UI preferences (theme, etc.) | Active |
| `user_profile.json` | JSON V1 | `user_profile::UserProfile` | Display name, sharing settings | Active |
| `secret_key.txt` | hex | | Node identity key (generated on first run) | Active |

> **Legacy JSON status:** All JSON `save()` methods are `#[deprecated]` and are
> no-ops that log a deprecation warning. The files remain on disk so existing
> readers can still load historical data during a transition period.
> `ChatHistoryStore` additionally exposes `migrate_legacy_json()`, a one-time
> transactional import of `chat_history.json` into the SQLite `messages`
> table; after a successful import the legacy file is renamed to
> `chat_history.json.imported`. `AppSettings` and `UserProfile` remain active
> JSON stores (they have no SQLite equivalent).

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

On every `Storage::open()`, `recover_crash_state()` runs four recovery passes:

1. **Sent→Pending reset** — outbox rows left in `Sent` status by a crash are
   reset to `Pending` with `last_error_code = 'crash_recovered'` and their
   retry timestamp set to now, so the delivery engine retries them immediately.
2. **Sending→Pending reset** — rows left in `Sending` status (claimed by a
   worker that crashed) are reset to `Pending` with the same error code,
   freeing them for other workers.
3. **Stale timestamp reset** — `Pending` rows with `next_attempt_at_ms` in the
   future are reset to now so they become due immediately.
4. **Stale lease clear** — outbox worker leases (`locked_until_ms <= now`)
   are cleared so no row is stranded by an unreachable worker.

Preserved ACKs: rows already in `Acked` status are never touched.

### Delivery State Machine

The outbox table tracks delivery state for each message+recipient pair through
a five-state machine implemented in SQLite:

```
        ┌──────────────────────────────────┐
        │                                  │
        ▼                                  │
    ┌─────────┐    ┌─────────┐    ┌──────┐ │
    │ Pending │───►│ Sending │───►│ Sent │─┘
    │  (0)    │    │  (4)    │    │ (1)  │──► Acked (2)
    └────┬────┘    └────┬────┘    └──────┘
         │              │
         ▼              ▼
      Expired (3)   Expired (3)
```

| Status | Code | Meaning |
|---|---|---|
| `Pending` | 0 | Ready for delivery, waiting for next_attempt_at_ms |
| `Sending` | 4 | Claimed by a worker for active delivery (lease-locked) |
| `Sent` | 1 | Transmitted to the recipient, awaiting ACK |
| `Acked` | 2 | Recipient confirmed receipt — terminal success |
| `Expired` | 3 | Message expired or cancelled — terminal failure |

Key behaviours:

- **Lease-based claiming** — workers claim rows by setting `Sending` status,
  `lease_owner`, and `locked_until_ms`. A crashed worker's lease expires and
  the row becomes available again.
- **Sent→Acked transition** — on receiving a valid ACK, `mark_acked()` sets
  status to `Acked`.
- **Record attempt** — `record_attempt()` sets status to `Sent` and advances
  `next_attempt_at_ms` for retry scheduling.
- **Expiry** — messages whose inbox row has expired (`expires_at_ms <= now`)
  have their corresponding outbox rows set to `Expired`.
- **Crash recovery** — `Sent` rows become `Pending`; `Sending` rows with
  expired leases become `Pending`; stale future timestamps reset to now.

### Outbox Worker Leases (V4)

Schema V4 introduced worker leases to support concurrent delivery workers:

- `outbox.lease_owner` — opaque worker identifier string.
- `outbox.locked_until_ms` — deadline for the lease (default 30s).
- `outbox.expires_at_ms` — per-row expiry override.

`claim_due_outbox()` atomically selects unclaimed due rows, sets their status
to `Sending`, assigns a lease, and returns them to the caller. The lease
prevents two workers from delivering the same message concurrently.

### Delivery Retries

The `OutboxDeliveryWorker` (`src/outbox_delivery.rs`) processes due outbox rows:

- **Exponential backoff** — `RetryPolicy` implements delays with 50% jitter:
  5s → 10s → 20s → 40s → 80s → 180s max.
- **Failure classification** — `DeliveryFailure::class()` returns
  `Transient`, `Permanent`, or `RetryableOnlyAfterUserAction`.
- **Structured error codes** — each failed attempt records
  `last_error_code` for diagnostics.
- **Peer-reconnect trigger** — when a disconnected peer reconnects,
  pending outbox entries for that peer are retried immediately.
- **Manual retry** — `retry_now()` resets `next_attempt_at_ms` for
  immediate retry.

### Schema: Version 1 (message delivery)

The V1 schema is identical to the legacy `MessageStore` schema and provides a
linear migration path from `message_store.db`.

| Table | Purpose | Key columns |
|---|---|---|
| `inbox` | Received messages | `msg_id` (BLOB PK), `conversation_id`, `author_user_id`, `ciphertext`, `signature`, `created_at_ms`, `acked_at_ms` |
| `outbox` | Outgoing message delivery | `(msg_id, recipient_device_id)` primary key, `status` (0=Pending,1=Sent,2=Acked,3=Expired), `attempts`, `next_attempt_at_ms` |
| `contacts` | Known peer identities | `(user_id, device_id)` primary key, `endpoint_addr`, `identity_key`, `last_seen_ms`, `expires_at_ms` |
| `sync_cursor` | Per-peer sync state | `peer_device_id` (PK), `last_seen_msg_clock`, `last_sync_at_ms` |

### Schema: Version 2 (content-addressed files + DM)

Extends V1 with file-object storage, sharing infrastructure, and direct-message
tables.

| Table | Purpose | Key columns |
|---|---|---|
| `file_objects` | Content-addressed immutable file store | `content_hash` (TEXT PK, blake3 hex), `size`, `mime_type`, `filename`, `data` (inline BLOB), `blob_hash` (iroh-blobs ref), `source_path` |
| `message_attachments` | Links a message to file objects | `(event_id, content_hash)` UNIQUE, `position` for ordering |
| `shared_files` | Profile-offered files | `(content_hash, profile_user_id)` PK, `offered` flag, `metadata_id` |
| `file_collections` | Named groups of shared files | Auto-increment `id`, `(profile_user_id, name)` UNIQUE |
| `file_collection_items` | Membership in a collection | `(collection_id, content_hash)` PK with ON DELETE CASCADE |
| `shared_file_permissions` | Per-peer grants on shared files | `(content_hash, grantor, grantee, permission)` PK, optional `expires_at_ms` |
| `downloads` | Durable download state machine | Auto-increment `id`, `state` (queued/active/paused/completed/failed), `bytes_downloaded`, retry tracking; V8 adds `temp_path`, `destination_path` |
| `profile_manifest_state` | Manifest revision tracking | `user_id` PK, monotonically increasing `revision`, `manifest_hash` |
| `dm_conversations` | Direct-message conversation registry | `conversation_id` (BLOB PK), `peer_id`, `created_at_ms` |
| `dm_sender_sequences` | Per-sender sequence counters | `(conversation_id, sender_id)` PK, `next_sequence` |
| `dm_messages` | Durable DM message store | `message_id` (PK), `conversation_id`, `sender_id`, `recipient_id`, `sequence`, `request_key`, `plaintext`, `logical_message` |
| `dm_outbox` | DM outbound envelope queue | `message_id` (PK FK), `recipient_id`, `envelope`, `status`, `created_at_ms` |

### Schema: Version 3 (outgoing DM tables, standalone)

Adds the `dm_*` tables as a standalone migration for databases that already
completed V2 but were created before the DM tables were part of V2.

### Schema: Version 4 (worker leases)

Adds outbox worker lease columns and the `next_attempt_at` index:

- `outbox.lease_owner` — TEXT, worker identifier
- `outbox.locked_until_ms` — INTEGER, lease expiry deadline
- `outbox.expires_at_ms` — INTEGER, per-row expiry override
- `idx_outbox_next_attempt` — index on `outbox(next_attempt_at_ms)`

### Schema: Version 5 (DM acknowledgements)

Adds acknowledgement tracking for direct messages:

- `dm_messages.acknowledged_at_ms` — INTEGER, when recipient acknowledged
- `dm_acknowledgements` — durable ACK records: `message_id` (PK),
  `original_sender_id`, `recipient_id`, `acknowledged_at_ms`, `status`,
  `signature`

### Schema: Version 6 (sync dedup)

Prevents re-serving the same envelope during repeated sync requests:

- `sync_dedup(message_id, recipient_id)` — records each envelope served
  via `SyncResponse` so subsequent sync requests from the same peer only
  receive newly-pending envelopes.

### Schema: Version 7 (file verification & replacements)

Adds file integrity verification tracking and replacement history:

- `file_verification(content_hash, profile_user_id)` — tracks per-file
  availability state (`Unknown`/`Available`/`Unavailable`) and verification
  timestamps.
- `file_replacements` — records when a shared file's content is replaced
  (old hash → new hash), supporting stale-projection detection.

### Schema: Version 8 (download paths)

Adds filesystem paths to downloads for interrupted download recovery:

- `downloads.temp_path` — temporary output path during download
- `downloads.destination_path` — final install path after verification

### Schema: Version 9 (file source paths)

Adds `file_objects.source_path` — the original filesystem path for files
that are referenced (not imported into iroh-blobs).

### Schema: Version 10 (GUI outgoing queue)

Replaces the GUI's dependency on `outbox.json` for the outgoing message queue:

- `outgoing_messages(event_id, topic_blob, hash, signed_bytes,
  delivery_state, retry_count, created_at_ms, updated_at_ms)` — the GUI
  reads/writes delivery state from SQLite instead of JSON. Each row stores
  the raw signed message bytes for replay/retry.
- `idx_outgoing_topic` — index on `outgoing_messages(topic_blob)`.

### Schema: Versions 11–19 (groups, rings, durable chat history)

Later migrations extended `boru.db` for group chat, rings, and the backfill
history table. They follow the same forward-only, idempotent migration model.

| Version | Adds | Purpose |
|---|---|---|
| V11 | `directory_ads` | Public-room directory advertisements |
| V12 | `groups` | Group-chat persistence |
| V13 | `group_encryption_state` | Encrypted group state blobs (fail-closed checksummed records) |
| V14 | `group_invites.ticket` | Room ticket on pending group invites |
| V15 | `group_invites.group_name` | Display name on pending group invites |
| V16 | `shared_files.version`, `transfer_activity` | Shared-file revisions + bounded transfer activity projection |
| V17 | `transfer_activity.direction` | Deterministic inbound/outbound transfer direction |
| V18 | `rings`, `ring_members`, `ring_resource_permissions` | Named-ring permission groups |
| V19 | `chat_messages` | Durable chat-message history (`msg_hash` UNIQUE, `(topic, timestamp_ms)` index) used by the backfill service |

### Remote catalogue projections

Remote catalogue storage uses the V2 relational tables rather than creating a
separate cache database:

| Data | Storage | Semantics |
|---|---|---|
| Peer/revision/generated/fetched metadata | `profile_manifest_state` keyed by the remote public-key string | `revision` is the advertised monotonic revision; `manifest_hash` stores the generated timestamp string; `created_at_ms` is the local fetch time |
| Remote file projection | `file_objects` plus `shared_files` keyed by remote profile and `metadata_id` | Stores safe display metadata, content hash, size, MIME type, and description; no source path or permission row is imported |
| Remote collections | `file_collections` keyed by remote profile | Stores collection display metadata for local browsing |

`Storage::replace_remote_catalogue` is called only after the client validates
the catalogue signature, owner identity, fields, limits, duplicate IDs/hashes,
and collection references. It upserts the entries returned by the latest
snapshot. The cache is a local display/reconciliation projection: it is not an
authorization source.

The `shared_file_permissions.expires_at_ms` column stores optional grant
expiry metadata. Permission evaluation and download authorization are separate
from cached catalogue reads; descriptor issuance re-checks the live permission
rows and the descriptor itself has an enforced expiry.

### Migration System

- Schema version tracked in `schema_version` table (`version` INTEGER PK,
  `applied_at_ms` INTEGER).
- Migrations are idempotent (use `IF NOT EXISTS`, `INSERT OR IGNORE`).
- Each migration runs in its own transaction.
- **Forward-only** — no downgrade path.
- **Future-schema guard** — opening a database with a version higher than
  `CURRENT_SCHEMA_VERSION` (currently 20) returns a clear error:
  ```
  Database has schema version <N>, but this application only supports up to
  version <MAX>. The database was created by a newer version. Upgrade the
  application or restore from a backup created by an older version.
  ```
- **Partial migration recovery** — if a migration crashes mid-way, the next
  `open()` re-runs only the unapplied migrations (already-applied versions
  are skipped via `schema_version`).
- **Current schema version** is defined in `src/storage.rs` as
  `CURRENT_SCHEMA_VERSION: u32 = 21`. A doc-consistency test
  (`docs_reference_current_schema_version` in `src/storage.rs`) fails when
  this constant changes and the architecture docs are not updated, so the
  documented schema version cannot drift silently.

### Local search (V21)

`chat_messages` remains the authoritative durable message table. V21 adds
 decrypted, user-visible projections (`search_body`, `search_filename`, and
`search_kind`) plus an SQLite FTS5 index. Only ordinary text, edit text,
profile names, and attachment filenames are indexed; attachment bytes,
typing, call signalling, and network/control-plane payloads are excluded.
Queries use `Storage::search_local` and never enter any gossip, DHT, relay, or
HTTP path. `Storage::rebuild_local_search_index` verifies signed payloads and
repairs projections after migration or corruption.

### Legacy Migration

`import_legacy_db()` reads the old `message_store.db` schema and copies inbox,
outbox, contacts, and sync cursors into the new storage. Re-import is
idempotent — `INSERT OR IGNORE` prevents duplicates.

Legacy JSON stores (`chat_history.json`, `outbox.json`, `friends.json`, etc.)
remain on disk for backward-compatible reads. Their `save()` methods are
`#[deprecated]` and silently drop writes. To migrate data out of legacy JSON
files, the application reads them at startup (the stores are still loaded).

**Do not remove migration support in the same release that introduced unified
storage.** Legacy import support is retained for at least one documented
compatibility period to allow existing users to migrate their data.

---

## Message Persistence Semantics

### Exactly-Once Local Persistence

- **`INSERT … ON CONFLICT(msg_id) DO NOTHING`** — inserting a message with a
  `msg_id` that already exists in the `inbox` table is silently ignored.
- The `(msg_id, recipient_device_id)` primary key on `outbox` provides the
  same guarantee for outbound entries.
- These constraints survive restarts: reopening the database preserves all
  rows exactly as they were.
- Tombstones in `message_tombstones` prevent resurrection of deleted messages
  by backfill, duplicate delivery, or restart replay.

### At-Least-Once Transport

- Outbox entries are created with `status = Pending` and `attempts = 0`.
- On each delivery attempt `record_attempt()`:
  - Increments `attempts`.
  - Sets `status = Sent`.
  - Records `next_attempt_at_ms` for retry scheduling.
  - Does NOT touch rows already in `Acked` state.
- After a crash, `Sent` and `Sending` rows are reset to `Pending` by crash
  recovery so no message falls through a gap.
- `fetch_due_outbox()` returns rows where `status != Acked AND status !=
  Expired AND next_attempt_at_ms <= now`.
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

Deletion state is managed in the `message_tombstones` table. See
[`docs/storage-redesign.md`](storage-redesign.md#deletion-and-tombstone-semantics-step-12)
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
- **DM messages**: the `plaintext` column in `dm_messages` stores decrypted
  message content. These are plaintext at rest.
- **Encrypted transport**: wire transport uses iroh's QUIC-based encryption
  (TLS 1.3 / QUIC crypto). On-the-wire messages are always encrypted between
  peers. The at-rest storage is separate: the database is not encrypted at
  the file level (no SQLite encryption extension).

### Transport Protocols

| Protocol | ALPN | Purpose | Persistence |
|---|---|---|---|
| Gossip | `/iroh-gossip/1` | Room-based broadcast | None (transient) |
| Inbox | `/iroh-chat-inbox/1` | Direct message sync + signed deletions | Inbox event emission |
| Backfill | `/iroh-gossip-chat/backfill/1` | Historical message requests | None (reads from SQLite) |
| Whisper | `/iroh-gossip-chat/whisper/1` | Private 1:1 QUIC channels | None (transient) |

---

## Content-Addressed Attachments

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
- The `BORU_FILES_DIR` env var can override the files root (legacy
  `BORU_CHAT_FILES_DIR` also accepted).

---

## Backup and Portability

- `boru.db` is a standard SQLite file in WAL mode. Backups should use
  `VACUUM INTO` or `.backup` to capture a consistent checkpoint.
- JSON files (`chat_history.json`, `settings.json`, etc.) can be backed up
  at any time (atomic writes ensure each file is self-consistent). Note that
  most JSON files are now read-only legacy stores — the authoritative data
  is in `boru.db`.
- All paths are relative to the data directory; moving the entire directory
  to a new machine recreates the full application state.
- **Do NOT mix data directories between different application versions** —
  the forward-only migration system will refuse to open a database created
  by a newer version.

---

## Current Limitations & Future Work

1. **No SQLite encryption** — the database file is unencrypted on disk. Anyone
   with filesystem access to the data directory can read ciphertext blobs.
   Transport-layer encryption (QUIC/TLS 1.3) protects messages in flight; at
   rest the storage depends on filesystem permissions. The `dm_messages`
   `plaintext` column stores decrypted message content — this is plaintext
   at rest.

2. **Tombstone pruning** — old tombstones accumulate indefinitely. A future
   step should add configurable TTL-based pruning (e.g. 90 days) — protocol
   replay windows already limit re-insertion risk.

3. **Batch tombstone protocol** — conversation-level deletion has local
   support (`delete_conversation`, `hard_delete_conversation`) but no
   corresponding batch protocol message for propagation to peers.

4. **Image format mismatch** — `ImageStore::save_image` preserves the original
   extension (via `safe_extension()`), but `optimize_chat_image` always emits
   JPEG. The store works around this with magic-byte detection, but the
   extension contract is not fully clean.

5. **GUI offline DM direct wiring** — the GUI offline DM fallback path
   (`/whisper` → mailbox fallback) does not currently insert messages into
   the SQLite `MessageStore`/`Storage` tables, does not start `RetryWorker`
   for them, and ignores `WhisperEvent::MailboxEnvelope` and
   `MailboxAck` events. The SQLite DM tables and retry infrastructure exist
   but are not wired through the GUI event loop.
