# Storage Redesign — Steps 1–12

## Overview

The storage redesign replaces the in-memory/JSON-based persistence layer with
a single SQLite database (`message_store.db`) that provides deterministic
message ordering, unread tracking, durable deletion tombstones, and
conversation metadata. This document focuses on Step 12: deletion semantics.

## Progress: Steps 13–19

| Step | Description | Status | Artifact |
|---|---|---|---|
| 13 | V2 schema: content-addressed file objects | **Done** | `storage::Storage::migrate_v2` — 8 new tables (file_objects, message_attachments, shared_files, file_collections, file_collection_items, shared_file_permissions, downloads, profile_manifest_state) |
| 14 | Legacy JSON → SQLite migration | **Done** | `storage::Storage::import_legacy_db()` copies inbox, outbox, contacts, sync_cursor; idempotent via ON CONFLICT DO NOTHING |
| 15 | Schema versioning and migration framework | **Done** | `schema_version` table, `CURRENT_SCHEMA_VERSION=2`, forward-only, future-schema guard, partial-migration recovery |
| 16 | Crash and corruption resilience | **Done** | `PRAGMA integrity_check` on open, `recover_crash_state()` (Sent→Pending recovery, stale timestamp reset), `PRAGMA journal_mode=WAL`, `busy_timeout=5000`, `synchronous=NORMAL` |
| 17 | Repository integration test suite | **Done** | `tests/test_storage_integration.rs` — 8 deterministic tests covering outgoing queue lifecycle, exactly-once, ordering, key rotation, deletion tombstone, legacy migration, attachment integrity, mixed operations |
| 18 | Redirect legacy message-store access | **Done** | `Storage::open()` imports existing `message_store.db`; storage integration in GUI is deferred |
| 19 | Update storage documentation | **Done** | `docs/message-storage-design.md` (comprehensive design doc), `docs/storage-redesign.md` (this file), `README.md` updated |

### Remaining Risk

- `ChatHistoryStore` and `OutboxStore` (JSON) remain as the active frontend
  persistence layer. The SQLite `Storage` is fully implemented and tested but
  not yet wired as the primary store in the GUI.
- A future integration pass should eliminate JSON writes entirely and make
  `Storage` the authoritative state for the GUI.

## Deletion and Tombstone Semantics (Step 12)

### Design Principles

1. **Once deleted, never resurrected.** A deleted message (local or remote)
   is permanently unrecoverable via normal message flow. The `message_tombstones`
   table durably records that a message id was deleted; every insert path
   (`insert_inbox`, `insert_inbox_with_conversation_update`) and read path
   (`get_inbox`) checks the tombstone table first.

2. **Local deletion ≠ remote deletion.** Local deletion is a client-side
   decision to remove a message from the local device. It does not notify
   peers and does not affect their copies. Remote deletion is initiated by
   the original message author and propagates via a signed protocol message
   that other peers cryptographically verify before applying.

3. **Tombstones outlive the message row.** When a message is deleted, its
   inbox row is removed, but a tombstone row remains in
   `message_tombstones`. This prevents resurrection by:
   - Backfill (fetching missed messages after reconnection)
   - Duplicate delivery (same message arriving from multiple peers)
   - Restart replay (re-processing old envelopes after a restart)

4. **Tombstoned outbound is cancelled.** Both local and remote deletion
   cancel any pending outbound deliveries for the deleted message by
   setting their status to `Expired`.

### SQL Schema

```sql
CREATE TABLE IF NOT EXISTS message_tombstones (
    msg_id          BLOB PRIMARY KEY,
    conversation_id BLOB NOT NULL,
    deleted_at_ms   INTEGER NOT NULL,
    deleted_by      BLOB NOT NULL,       -- PublicKey for remote, zeros for local
    signature       BLOB NOT NULL,       -- Author's signature (remote), empty (local)
    is_local        INTEGER NOT NULL DEFAULT 1  -- 1=local delete, 0=remote protocol
);
```

### API Methods

#### Local Deletion

- **`delete_message(msg_id) -> Result<bool>`** — Locally deletes a single
  message. Reads the conversation id from the inbox row, inserts a local
  tombstone (`is_local=1`), removes the inbox row, and sets any pending
  outbound deliveries to `Expired`. Returns `false` if the message was
  not found.

- **`delete_conversation(conversation_id) -> Result<usize>`** — Removes all
  inbox messages for a conversation and soft-deletes the metadata row
  (`is_deleted=1`). Does **not** touch outbox rows — pending outgoing
  messages are preserved for delivery.

- **`hard_delete_conversation(conversation_id) -> Result<usize>`** — Removes
  all inbox messages, pending outgoing messages, **and** the metadata row
  entirely. Use only when the user explicitly confirms they want to discard
  pending sends as well.

- **`cancel_pending_outbound(msg_id) -> Result<usize>`** — Sets pending
  outbound delivery rows to `Expired`. Idempotent — cancelling an already
  cancelled/acked message returns 0. Used both standalone and internally
  by `delete_message`.

#### Remote Deletion (Signed Tombstones)

- **`insert_tombstone(msg_id, conversation_id, deleted_by, signature)
  -> Result<bool>`** — Inserts a remote tombstone (`is_local=0`) after the
  caller has cryptographically verified the author's signature. Removes the
  inbox row and cancels pending outbound. Returns `true` if a new tombstone
  was inserted, `false` if the message was already tombstoned.

#### Queries

- **`is_tombstoned(msg_id) -> Result<bool>`** — Returns `true` if a
  tombstone exists for the given message id, regardless of whether it was
  local or remote.

- **`get_inbox(msg_id) -> Result<Option<StoredEnvelope>>`** — Returns `None`
  for tombstoned messages, even if the inbox row was already removed and
  only the tombstone remains. Non-tombstoned messages are returned normally.

### Protocol Layer (inbox.rs)

The inbox protocol supports signed delete tombstones via:

1. **`AuthorDeleteProof`** — A struct signed by the original message
   author, containing `(msg_id, conversation_id, created_at_unix_secs,
   author, author_signature)`. The signing covers `msg_id || conversation_id
   || created_at_unix_secs` to prevent replay.

2. **`InboxPayload::DeleteTombstone(AuthorDeleteProof)`** — A protocol
   message variant that carries the proof inside a `SignedInboxMessage`
   (outer envelope provides sender authentication and replay protection).

3. **`InboxEvent::DeleteTombstoneReceived { from, proof }`** — An event
   emitted by the inbox protocol handler after the handler:
   - Verifies the outer `SignedInboxMessage` signature
   - Verifies the inner `AuthorDeleteProof` author signature
   - Checks the inner proof's timestamp is within the 24-hour replay window
   - Deduplicates against the `seen_message_ids` set

4. **`send_delete_tombstone(endpoint, secret_key, peer, msg_id,
   conversation_id, author_sk)`** — Constructs an `AuthorDeleteProof`
   signed by `author_sk`, wraps it in a `SignedInboxMessage` signed by
   `secret_key`, and sends it to the peer's inbox over QUIC.

#### Protocol Handler Flow

When the inbox protocol handler receives a `DeleteTombstone`:

```
1. Verify outer SignedInboxMessage signature
2. Verify inner AuthorDeleteProof author signature
3. Replay-protect inner proof timestamp (24h max skew)
4. Deduplicate by inner msg_id in seen_message_ids
5. Emit InboxEvent::DeleteTombstoneReceived { from, proof }
```

The frontend (iced_chat) receives the event and calls
`MessageStore::insert_tombstone()` to persist the tombstone.

#### Sending Flow

When a user deletes a message they authored:

```
1. Locally: delete_message(msg_id) → local tombstone + inbox removal
2. For each peer with a copy: send_delete_tombstone(endpoint, ..., author_sk)
   → QUIC connection to peer's inbox → SignedInboxMessage →
   DeleteTombstone(AuthorDeleteProof)
3. Each peer: verifies proof → insert_tombstone() → remote tombstone
```

### Edge Cases and Guarantees

| Scenario | Behaviour |
|---|---|
| Backfill after local delete | `insert_inbox` checks tombstone → rejected |
| Backfill after remote delete | Same — tombstone check is author-agnostic |
| Duplicate after delete | `insert_inbox` returns `false` |
| Restart replay of deleted msg | Tombstone persists in SQLite → rejected |
| ACK received after delete | `mark_acked` is safe — doesn't touch tombstone |
| Outbound retry after deletion | `cancel_pending_outbound` sets status to Expired; `record_attempt` guards against Expired rows |
| Mixed local+remote tombstones | Both types coexist in `message_tombstones`, both block re-insertion |
| Non-tombstoned messages | `get_inbox` unaffected by tombstones of sibling messages |
| Reopen DB with tombstones | All tombstones survive reopen via SQLite durability |

### Future Work

- **Tombstone pruning:** Old tombstones could be cleaned up after a
  configurable TTL (e.g., 90 days) to reclaim space. Pruning must not
  allow re-insertion of messages that were deleted by the author — the
  protocol-layer replay window (24h) already prevents replay of old
  messages, so old tombstones could be safely removed once the message
  has expired from the sync window.
- **UI reflection:** The `DeleteTombstoneReceived` event currently only
  persists the deletion. A future step should update the chat UI to show
  "[message deleted]" when the deleted message was displayed.
- **Batch deletion:** Delete all messages in a conversation (already
  supported via `delete_conversation`), but there's no batch tombstone
  protocol message for peers yet.
