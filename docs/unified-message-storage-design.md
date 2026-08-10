# Unified Message Storage Design

## Status

> **Status: design completed.** This document is the Phase 2 design for the
> unified storage work. The implementation is complete (BORU-AUDIT-18..21):
> `boru.db` is at **schema V19**, the GUI outgoing queue reads/writes SQLite
> `outgoing_messages` (V10), and chat history is persisted to
> `message_store.db` (`messages` table). JSON stores listed below as "still
> active" are now read-only migration inputs. See
> [`message-storage-design.md`](message-storage-design.md) for the current
> schema, migration model, and authority map.

- **Phase 1** — Audit complete (`docs/storage-unification-audit.md`).
- **Phase 2** — This document: define storage invariants, authority boundaries, and architecture.
- **Phases 3–5** — Implementation (repository API, UI event wiring, lifecycle tests).

---

## 1. Design Principles

1. **SQLite is the authoritative store.** One database (`boru.db`, WAL mode) holds all state that must survive restarts. JSON files are either migration sources or transient caches — never the ground truth for data the application depends on.

2. **Single write path.** All state mutations route through `Storage` methods. No module writes directly to JSON without going through the coordinator layer. The GUI reads from in-memory projections, not from disk.

3. **Exactly-once persistence.** SQL constraints (PRIMARY KEY, UNIQUE, ON CONFLICT DO NOTHING) prevent duplicate rows even across crash–restart cycles.

4. **Forward-only schema.** Migrations are additive and idempotent. There is no downgrade path. A database created by a newer version is refused with a clear error.

5. **Crash-truncation safe.** WAL journal mode, `synchronous = NORMAL`, and crash-state recovery on open guarantee the database is internally consistent after an ungraceful shutdown. Sent→Pending recovery ensures no outbox entry is silently lost.

6. **UI is a projection, not an authority.** The iced GUI subscribes to `Storage` changes via events and rebuilds its view state from scratch; it never writes directly to SQLite.

---

## 2. Authority Map

| Domain | Authoritative Store | Secondary / Cache | Migration Target |
|---|---|---|---|
| Messages (inbox/gossip) | `boru.db` — `inbox`, `outbox` tables | `chat_history.json` (legacy frontend) | `inbox` + `outbox` |
| Direct messages | `boru.db` — `dm_messages`, `dm_outbox` | — | Already in SQLite |
| Conversations (rooms + DM) | `boru.db` — `conversation_meta` | `conversations.json` (JSON, still active) | V10 SQLite table |
| Friends list | `boru.db` — future `friends` table | `friends.json` (JSON, still active) | V10 SQLite table |
| Friend requests | `boru.db` — future `friend_requests` table | `friend_requests.json` (JSON, still active) | V10 SQLite table |
| Mailbox (offline envelopes) | `boru.db` — future `mailbox` table | `mailbox.json` (JSON, still active) | V10 SQLite table |
| User profile | `boru.db` — future `profiles` table | `profile.json` (JSON, still active) | V10 SQLite table |
| Room history | Transient in-memory | `rooms.json` (legacy, deleted on discovery) | None (not persisted) |
| Settings | `settings.json` (JSON) | — | Out of scope (UI config) |
| File objects | `boru.db` — `file_objects` | `files/` directory (binary content) | Already in SQLite |
| File attachments | `boru.db` — `message_attachments` | — | Already in SQLite |
| Downloads | `boru.db` — `downloads` | — | Already in SQLite |
| Sync dedup | `boru.db` — `sync_dedup` | — | Already in SQLite |
| Tombstones | `boru.db` — `message_tombstones` | — | Already in SQLite |
| Node identity | `secret_key.txt` (file) | — | Out of scope (key material) |

### What stays outside SQLite

- **`secret_key.txt`** — Node identity key material. Must remain a standalone file for portability and security boundaries.
- **`files/` directory** — Binary image content (JPEG, PNG, etc.). Large blobs accessed by content hash; `file_objects` stores metadata and optional inline data only.
- **`settings.json`** — UI configuration (theme, window size). Not message-critical state; can be reset without data loss.
- **Transient state** — Active gossip subscriptions, mDNS cache, DHT records, in-flight QUIC connections.

### What moves into SQLite (V10+ migrations)

The following JSON files are migration targets for V10–V12:

| JSON File | Target Table(s) | Rationale |
|---|---|---|
| `conversations.json` | `conversations` | Unread counts, mute/archive/delete flags, last-message preview |
| `friends.json` | `friends` | Relationship state, known addresses, mutual status |
| `friend_requests.json` | `friend_requests` | State machine (Pending→Accepted/Declined/Cancelled) |
| `mailbox.json` | `mailbox_envelopes` | Encrypted offline-message envelopes with TTL |
| `profile.json` | `profiles` | Display name, bio, sharing config, shared file metadata |
| `chat_history.json` | `chat_messages` (gossip room messages projection) | UI display of room messages |
| `outbox.json` | `outbox` (extends existing) | Room-message delivery tracking (currently JSON-only for gossip) |

---

## 3. Message Identity Rules

### 3.1 Gossip / Inbox Messages (`inbox`)

- **Identity**: `msg_id` — blake3 hash of `(conversation_id || author_user_id || author_device_id || created_at_ms)`.
- **Uniqueness**: `msg_id` is the PRIMARY KEY in `inbox`. Insert with `INSERT … ON CONFLICT(msg_id) DO NOTHING`.
- **Ordering**: `ORDER BY created_at_ms DESC`. Clocks are logical (POSIX millis from the author); skew is bounded by the gossip replay window (~24h).
- **Scope**: One `inbox` row per recipient. Each recipient independently inserts the same `msg_id`.

### 3.2 Direct Messages (`dm_messages`)

- **Identity**: `message_id` — blake3 hash of `("boru-chat/dm/request/v1" || sender_pk || conversation_id || request_key)`.
- **Uniqueness**: `message_id` is the PRIMARY KEY. Additionally `request_key` has a UNIQUE constraint for idempotent re-queue.
- **Sequence**: Per-(conversation_id, sender_id) monotonic `sequence` column for deterministic ordering.
- **Ordering**: `ORDER BY sequence ASC`, stable across restarts (retries/duplicates cannot create a new sequence number or move rows).
- **Scope**: One row shared across sender and recipient (the DM transport ensures each side persists the same logical message blob).

### 3.3 Room / Gossip Messages (`chat_history.json` → SQLite projection)

- **Identity**: `event_id` (U64, from the gossip subscription stream). Each room message carries a monotonically increasing LocalU64 scoped to the topic.
- **Uniqueness**: `event_id` per topic. The same event id from the same topic is deduplicated.
- **Transaction semantics**: The gossip layer emits each event exactly once per subscribed peer. Local persistence must accept the event before acknowledging it to the gossip engine.

### 3.4 Delivery Tracking (`outbox` — gossip + DM)

- **Gossip outbox**: Keyed by `(msg_id, recipient_device_id)`. The same message may have multiple outbox rows (one per target recipient in the room).
- **DM outbox**: Keyed by `message_id` (one-to-one with `dm_messages`). Each DM has exactly one outbox row per delivery attempt lifecycle.

---

## 4. Conversation Identity Rules

### 4.1 Room Conversations

- **Identity**: `TopicId` (32-byte blake3 hash of the room's topic string).
- **Scope**: Visible to all subscribed peers. No ownership — the topic identifies the room, not a pair of users.
- **Storage**: `conversation_meta` table (in `boru.db`), keyed by `conversation_id` (topic bytes).

### 4.2 DM Conversations

- **Identity**: `conversation_id` — blake3 hash of the two participants' sorted public keys (`min(A_pk, B_pk) || max(A_pk, B_pk)`).
- **Scope**: Exactly one pair of peers. The same `conversation_id` is generated independently by both sides.
- **Storage**: `dm_conversations` table with `(conversation_id PK, peer_id)`.

### 4.3 Uniqueness Invariants

- A room topic maps to exactly one `conversation_id` in `conversation_meta`.
- A peer pair maps to exactly one `conversation_id` in `dm_conversations`.
- There is no overlap between room and DM identifiers (room topics are structured strings; DM identifiers are deterministic public-key concatenation — no collision possible).

---

## 5. Delivery-State Transitions

### 5.1 Transport Delivery State (`DeliveryStatus` — `outbox` table)

```
Pending ──→ Sent ──→ Acked
  │                   │
  └──→ Expired        └──→ Expired (via inbox expiry)
```

| Status | Meaning | Persistence |
|---|---|---|
| `Pending` (0) | Queued, awaiting delivery attempt | Survives restart |
| `Sent` (1) | Wire-level send attempted | On crash, reset to `Pending` |
| `Acked` (2) | Recipient confirmed receipt | Durable via `dm_acknowledgements` |
| `Expired` (3) | Message TTL passed, will not retry | Idempotent checkpoint |

**Transition rules:**
- `Pending → Sent` — on first or subsequent delivery attempt.
- `Sent → Pending` — on crash recovery (only for rows still `Sent`).
- `Pending/Sent → Acked` — on receipt of signed acknowledgement.
- `Any → Expired` — when `expires_at_ms` passes. Also triggered by deletion/cancel.
- `Acked → Expired` — when inbound inbox message expires.
- No transition from a terminal state (`Acked`, `Expired`) is valid.

### 5.2 User-Visible Delivery State (`DeliveryState` — `chat_history` / GUI projection)

```
Queued ──→ Sent ──→ Delivered ──→ Seen
  │                   │
  └──→ Failed         └──→ Failed
```

| State | Semantic | Source |
|---|---|---|
| `Queued` | Composed, persisted locally, not yet broadcast | Outbox entry is `Pending` |
| `Sent` | Broadcast accepted by local gossip engine | Outbox entry is `Sent` |
| `Delivered` | Remote peer's node confirmed receipt | Transport acknowledgment received |
| `Seen` | Remote user viewed the message | Read-receipt signal |
| `Failed` | Delivery permanently failed | Retry budget exhausted or explicit error |

**GUI projection rules:**
- The GUI reads `DeliveryState` from the in-memory projection rebuilt from storage events.
- `Queued` and `Sent` are shown as a single-tick indicator.
- `Delivered` is shown as a two-tick indicator.
- `Seen` is shown as two blue ticks (or the configured read-receipt style).
- `Failed` is shown with an error icon and retry affordance.
- The `OutboxDeliveryWorker` advances the underlying `DeliveryStatus`; the event layer projects it to `DeliveryState` for the GUI.

### 5.3 Friend Request State Machine (`FriendRequestStatus`)

```
Pending ──→ Accepted
  │
  ├──→ Declined
  │
  └──→ Cancelled
```

- Only `Pending` can transition. Terminal states (`Accepted`, `Declined`, `Cancelled`) are immutable.
- A new `Pending` request may be created between the same pair after any terminal state.

### 5.4 File Download State Machine (`downloads.state`)

```
queued ──→ active ──→ completed
  │         │
  └──→ failed
```

- `queued → active` — when the transfer begins.
- `active → completed` — when the full file is received and verified.
- `active/queued → failed` — on unrecoverable error or cancelled.
- `paused → active` — resume from checkpoint.
- Durable via `bytes_downloaded`, `retry_count`, `next_retry_at_ms`.

---

## 6. Deduplication Rules

| Layer | Mechanism | Applies to |
|---|---|---|
| SQL PK constraint | `INSERT … ON CONFLICT DO NOTHING` | `inbox`, `outbox` (v1), `dm_messages`, `dm_outbox`, `message_tombstones`, `file_objects` |
| SQL UNIQUE constraint | `request_key UNIQUE` | `dm_messages.request_key` |
| Composite index | `UNIQUE(conversation_id, sender_id, sequence)` | `dm_messages` sequence ordering |
| Sync dedup table | `sync_dedup(message_id, recipient_id)` — served message tracking | DM sync protocol |
| Seen-message set | In-memory LRU (not persisted) | Per-peer gossip dedup on reconnect |
| Tombstone check | `is_tombstoned(msg_id)` gate before insert | `inbox` insert path |
| Atomic write | File-level atomic rename | Legacy JSON stores |
| Idempotent migration | `IF NOT EXISTS` on CREATE, `INSERT OR IGNORE` on legacy import | Schema migrations |

**Key invariant:** No message row is ever silently overwritten. A duplicate insert returns success (idempotent) without modifying the existing row.

---

## 7. Transaction Boundaries

### 7.1 Storage-Level Transactions

All `Storage` mutation methods open an `Immediate` transaction, preventing write–write deadlocks at the SQLite level.

| Operation | Transaction Scope | Tables Involved |
|---|---|---|
| Insert inbox message | Single tx | `inbox`, optionally `conversation_meta` (unread increment) |
| Queue outgoing DM | Single tx | `dm_messages`, `dm_outbox`, `dm_sender_sequences` |
| Enqueue outbox (gossip) | Single tx | `outbox` |
| Claim due outbox rows | Single tx | `outbox` (UPDATE lease) |
| Mark ACK received | Single tx | `outbox`, `dm_acknowledgements`, `dm_messages.acknowledged_at_ms` |
| Delete message locally | Single tx | `message_tombstones` (insert), `inbox` (delete), `outbox` (set Expired) |
| Delete conversation | Single tx | `inbox`, `outbox`, `message_tombstones`, `conversation_meta` |
| Replace remote catalogue | Single tx | `file_objects`, `shared_files`, `file_collections`, `file_collection_items` |
| Legacy JSON import | Single tx per file | Bulk INSERT OR IGNORE |
| Schema migration | Single tx per version | DDL + `schema_version` insert |

### 7.2 Cross-Store Consistency

Because JSON stores write independently (no shared transaction), the following invariants must hold:

1. **Outbox write before broadcast.** An outgoing message is written to `outbox.json` (or `outbox` SQLite table) *before* the gossip `broadcast()` call. If the broadcast crashes, the `Pending` entry is retried on restart.
2. **Mailbox append after inbox verify.** The off-device mailbox is written after the local inbox entry is confirmed (to avoid storing an envelope that was already processed locally).
3. **No cross-store rollback.** A failure after writing one store but before writing the other requires manual reconciliation on restart. The crash-recovery path (`recover_crash_state`) handles `Sent → Pending` recovery for the SQLite `outbox`. JSON stores rely on atomic file writes for single-file consistency.

### 7.3 Future Unified Write Path

When all domains live in SQLite, every mutation is a single `Immediate` transaction:

```
queue_outgoing_message(tx):
  1. INSERT INTO inbox/chat_messages (outgoing copy)
  2. INSERT INTO outbox (Pending)
  3. UPDATE conversation_meta (last_message, increment unread)
  → commit
```

---

## 8. Crash-Recovery Rules

### 8.1 On Every `Storage::open()`

Runs inside `recover_crash_state()`:

1. **Integrity check** — `PRAGMA integrity_check`. On failure, returns a clear error; never silently repairs.
2. **Sent → Pending reset** — all `outbox` rows with `status = Sent` are reset to `Pending` with `last_error_code = 'crash_recovered'` and `next_attempt_at_ms = now`. This prevents the "lost send" class of crashes: a message that was dispatched to the wire without an ACK will be retried.
3. **Stale timestamp recovery** — `outbox` rows with `status = Pending` and `next_attempt_at_ms > now` are reset to `next_attempt_at_ms = now`, making them due immediately.
4. **Preserved ACKs** — rows with `status = Acked` are never touched.
5. **Lease cleanup** — any `locked_until_ms` in the past is cleared (`lease_owner = NULL`, `locked_until_ms = NULL`) so orphaned worker leases don't block delivery.

### 8.2 Unrecoverable States

| Condition | Behaviour |
|---|---|
| `PRAGMA integrity_check` fails | `Storage::open()` returns error; application reports to user |
| Schema version > `CURRENT_SCHEMA_VERSION` | `Storage::open()` returns error; requires app upgrade |
| `files/` directory with stale temps | Cleaned up but not a crash-consistency concern |
| `secret_key.txt` missing or corrupt | Application generates new key; previous identity is lost |

### 8.3 Partial Migration Recovery

If a migration crashes mid-way, the next `open()` re-runs only the unapplied migrations (already-applied versions are skipped via `schema_version`). Each migration runs in its own transaction, so a crash inside `migrate_v3` leaves V2 applied and V3 unapplied — the next open reapplies V3 cleanly.

---

## 9. Migration Rules

### 9.1 Schema Versioning

- Tracked in `schema_version` table (`version INTEGER PK, applied_at_ms INTEGER NOT NULL`).
- `CURRENT_SCHEMA_VERSION` (`u32`) is incremented with each new migration.
- All migrations are applied in version order during `Storage::open()`.

### 9.2 Migration Invariants

| Rule | Enforcement |
|---|---|
| Forward-only | No downgrade path exists |
| Idempotent | `IF NOT EXISTS` on CREATE, `ALTER TABLE` guarded by version check |
| Transactional | Each migration runs in its own `Immediate` transaction |
| Future-schema guard | `version > CURRENT_SCHEMA_VERSION` → error |
| Out-of-order guard | `version < current` → skip (not re-applied) |
| ALTER TABLE safety | Add-only (columns, indexes), never DROP or RENAME |
| Legacy import | `import_legacy_db()` is idempotent (`INSERT OR IGNORE`) |

### 9.3 Recommended Migration Test Steps

1. Create a database at version N.
2. Insert sample data.
3. Close and reopen (triggers N→N+1 migration).
4. Verify migrated schema and that sample data survived.
5. Open with a future `CURRENT_SCHEMA_VERSION` — verify error.
6. Verify rollback (crash mid-migration) — next open completes the missing migration.

---

## 10. UI Event Rules

### 10.1 Event Model

The `Storage` layer broadcasts changes through an in-memory event channel. The GUI subscribes and rebuilds its view state from scratch on each event batch, never mutating storage state directly.

### 10.2 Event Types

| Event | Trigger | Payload |
|---|---|---|
| `MessageReceived` | `insert_inbox` succeeded | `(msg_id, conversation_id, author, created_at_ms)` |
| `MessageSent` | `enqueue_outbox` succeeded | `(msg_id, conversation_id, recipient)` |
| `DeliveryStateChanged` | outbox status advanced | `(msg_id, old_status, new_status)` |
| `AckReceived` | ACK inserted into `dm_acknowledgements` | `(message_id, sender, recipient)` |
| `ConversationUpdated` | conversation meta changed | `(conversation_id, field, old_value, new_value)` |
| `UnreadChanged` | unread count incremented/decremented/reset | `(conversation_id, delta, new_count)` |
| `MessageDeleted` | tombstone inserted | `(msg_id, conversation_id, is_local)` |
| `FriendStatusChanged` | friend added/removed/status changed | `(peer_id, old_status, new_status)` |
| `FriendRequestReceived` | new friend request persisted | `(request_id, requester, message)` |
| `FriendRequestUpdated` | request accepted/declined/cancelled | `(request_id, new_status)` |
| `FileAttached` | file linked to a message | `(event_id, content_hash, filename)` |
| `ProfileUpdated` | local or remote profile changed | `(user_id, field)` |
| `DownloadStateChanged` | download advanced | `(content_hash, old_state, new_state)` |

### 10.3 Event Delivery Rules

1. **Ordered** — events from a single mutation appear in causal order on the channel.
2. **Idempotent downstream** — the GUI should tolerate seeing the same event twice (it recomputes projection from scratch per event).
3. **Non-blocking** — the event channel uses a bounded MPSC; if the GUI is slow, older events are dropped (the GUI will re-read authoritative state on resume).
4. **Batch coalescing** — rapid mutations (e.g. receiving 100 messages) may be coalesced into a single "reload conversation" event to avoid flooding the GUI.

### 10.4 What the GUI Does NOT Do

- The GUI never calls `INSERT`, `UPDATE`, or `DELETE` directly on SQLite.
- The GUI never writes to JSON files.
- The GUI never manages `Storage` transactions.
- The GUI never interprets delivery state machine transitions — it only displays the current state.

---

## 11. Retry Rules

### 11.1 Outbox Delivery Retry (`outbox` table)

| Parameter | Value |
|---|---|
| Initial backoff | 1 second |
| Max backoff | 5 minutes |
| Jitter | ±25% (random) |
| Max attempts | 10 |
| TTL | 7 days (`expires_at_ms = created_at + 7d`) |

**Mechanism:**
- `next_attempt_at_ms` is set after each attempt via exponential backoff.
- Multiple workers claim rows via lease (`lease_owner`, `locked_until_ms`) for 30s.
- Rows with `expires_at_ms ≤ now` are set to `Expired` and never retried.

### 11.2 Outbox Claim / Lease Protocol

1. `claim_due_outbox(worker_id, max_leases)` — `UPDATE outbox SET lease_owner=?, locked_until_ms=now+30s WHERE locked_until_ms IS NULL OR locked_until_ms < now AND status IN (Pending, Sent) AND next_attempt_at_ms ≤ now AND (expires_at_ms IS NULL OR expires_at_ms > now) AND lease_owner IS NULL`.
2. Worker delivers the claimed rows.
3. On success: `mark_acked()` or advance status + clear lease.
4. On crash: locked lease expires naturally in 30s; another worker picks it up.
5. On crash of the sole worker: `recover_crash_state()` resets `Sent` rows to `Pending` on next startup.

### 11.3 DM Delivery Retry (`dm_outbox` table)

- Same lease protocol as gossip outbox.
- `status` is 0=Pending, 1=Sent, 2=Acked (no Expired — DM TTL is managed by the inbox protocol).
- Retry uses same backoff logic but with a shorter TTL (24h) because DM delivery is more time-sensitive.

### 11.4 File Download Retry

- `retry_count` and `next_retry_at_ms` on `downloads` table.
- Exponential backoff from 10s to 10 minutes.
- Max 5 retries, then `state = failed`.

---

## 12. ACK Handling

### 12.1 Gossip / Room Message ACK

- No per-message ACK at the gossip layer. Delivery confirmation is inferred:
  - Message observed back through gossip receive path → `Delivered` (two-tick).
  - Read receipt received → `Seen` (two blue ticks).
- The `outbox` table's `DeliveryStatus::Acked` is used for direct-message transport ACKs only.

### 12.2 DM Transport ACK (`dm_acknowledgements`)

- Signed by the recipient's mailbox identity.
- Stored in `dm_acknowledgements` table: `(message_id PK, original_sender_id, recipient_id, acknowledged_at_ms, status, signature)`.
- On ACK receipt: `dm_messages.acknowledged_at_ms` is set, and the corresponding `dm_outbox` row advances to `Acked`.
- Idempotent: same ACK inserted twice → `INSERT OR IGNORE`.

### 12.3 Inbox Protocol ACK (Legacy)

- The legacy `inbox` table (v1) has `acked_at_ms` column set on message receipt.
- Used for the older inbox/outbox transport; superseded by `dm_acknowledgements` for DM.

### 12.4 Read Receipts

- A read receipt is an application-level signal sent by the GUI when the user opens a conversation.
- Persisted as a `Seen` event in the outbox timeline (not a separate storage table).
- The sender advances `DeliveryState → Seen` on receipt.

---

## 13. Attachment Relationships

### 13.1 Content-Addressed File Store

```
file_objects (content_hash PK, size, mime_type, filename, data, blob_hash, source_path)
     │
     ├── message_attachments (event_id FK, content_hash FK, display_filename, position)
     │     └── Links chat messages to file objects. Position determines display order.
     │
     ├── shared_files (content_hash FK, profile_user_id PK, metadata_id, offered, ...)
     │     └── Profile-offered files (local user's share catalogue).
     │
     └── downloads (content_hash FK, remote_peer, state, ...)
           └── File transfer state machine.
```

### 13.2 Ownership Rules

| Relationship | Owner | Cascade |
|---|---|---|
| Chat message + attachment | The message (`event_id`) owns the attachment row | Deleting the message removes `message_attachments` rows but NOT `file_objects` |
| Profile + shared file | The profile owns the `shared_files` row | Deleting the profile removes `shared_files` but NOT `file_objects` |
| File object itself | No single owner — it may be referenced by multiple messages, profiles, and collections | GC is manual (orphaned content hashes with 0 references) |
| Download | The download row owns itself | Removing a download does NOT remove the file object |

### 13.3 Orphan Detection

- `content_hash` values with zero references across `message_attachments`, `shared_files`, `file_collection_items` are candidates for garbage collection.
- GC is not automatic — a maintenance query (`Storage::purge_orphan_files()`) is available for explicit cleanup.

### 13.4 File Integrity

- `size` and `data` (when present) can be verified by re-hashing and comparing to `content_hash`.
- `file_verification` table tracks availability checks on referenced files.
- `file_replacements` tracks when a file was replaced (content changed → new hash).

---

## 14. Retention / Deletion Behaviour

### 14.1 Message-Level Deletion

| Kind | Action | Storage Impact |
|---|---|---|
| Local delete | `delete_message(msg_id)` | Insert tombstone, remove inbox row, cancel outbound |
| Remote delete | `insert_tombstone(author_proof)` | Insert remote tombstone, remove inbox row, cancel outbound |
| Cancel outbound | `cancel_pending_outbound(msg_id)` | Set outbox rows to `Expired` |

**Tombstone invariants:**
- Once inserted, a tombstone blocks future insertion of that `msg_id` regardless of source (backfill, duplicate, restart replay).
- Tombstones persist in `message_tombstones` indefinitely (pruning is future work, safe after the 24h gossip replay window passes).
- Both local and remote tombstones coexist in the same table.

### 14.2 Conversation-Level Deletion

| Method | Scope |
|---|---|
| `delete_conversation(conv_id)` | Removes inbox messages, inserts tombstones, soft-deletes conversation meta (`is_deleted = 1`). Preserves pending outbound (the user may still want to deliver previously queued messages). |
| `hard_delete_conversation(conv_id)` | Removes all inbox + outbox rows + tombstones + conversation meta. Destructive — use only on explicit user confirmation. |

### 14.3 Time-Based Expiry

| Store | TTL | Enforcement |
|---|---|---|
| `outbox` | 7 days | `expires_at_ms` column; expired rows set to `Expired` status |
| `dm_outbox` | 24h | Shorter TTL for time-sensitive DM delivery |
| `mailbox.json` | 7 days (`DEFAULT_MAILBOX_TTL`) | Retention window enforced by `MailboxStore::expire_old()` |
| `chat_history.json` / projection | No automatic expiry | Future: configurable message retention period |
| `message_tombstones` | No automatic expiry | Safe to prune after the gossip replay window (24h) |
| `sync_dedup` | No automatic expiry | Small table bounded by active peers |
| `file_objects` | No automatic expiry | Orphan GC is manual |
| `downloads` | No automatic expiry | Manual cleanup on completion |

### 14.4 Deletion Cascading (Future)

- Deleting a conversation should cascade to all its messages (tombstone each).
- Deleting a message should cascade to its attachments (`message_attachments` rows).
- Deleting a profile should cascade to its offers (`shared_files`) and collections.
- Cascading should NOT delete `file_objects` rows (they may be referenced elsewhere).

---

## 15. Architectural Summary

### 15.1 Data Flow

```
  ┌──────────────┐     UiEvent broadcast     ┌──────────────┐
  │  Transport   │ ─────────────────────────►  │  GUI (Iced)  │
  │  (gossip,    │       (via channel)        │  Projection  │
  │   inbox, DM) │                             │  (in-memory) │
  └──────┬───────┘                             └──────────────┘
         │                                            ▲
         │ Insert/update                              │ Read (from
         ▼                                            │ projection,
  ┌──────────────┐                                    │ not disk)
  │  Storage     │
  │  (SQLite)    │
  │              │
  │  immutables  │
  │  - inbox     │
  │  - outbox    │
  │  - dm_*      │
  │  - file_*    │
  │  - sync_*    │
  │  - tombstone │
  │              │
  │  V10+ tables │
  │  - friends   │
  │  - friend_req│
  │  - profiles  │
  │  - conv_meta │
  │  - mailbox   │
  └──────┬───────┘
         │
         │ Legacy import on startup
         ▼
  ┌──────────────┐
  │ JSON files   │ (migration sources)
  │ - friends    │
  │ - friend_req │
  │ - profile    │
  │ - conv       │
  │ - mailbox    │
  │ - chat_hist  │
  │ - outbox     │
  └──────────────┘
```

### 15.2 Startup Sequence

```
1. Resolve data directory (CLI > env > XDG > PWD fallback)
2. Read secret_key.txt (generate if missing)
3. Init iroh endpoint and protocol handlers
4. Storage::open(data_dir)
   a. Create boru.db (if missing)
   b. PRAGMA journal_mode=WAL, synchronous=NORMAL, busy_timeout=5000
   c. Run PRAGMA integrity_check
   d. Run pending schema migrations
   e. recover_crash_state() — Sent→Pending reset, stale timestamps
   f. Import legacy JSON files (if present) — V10+ migrations
5. GUI subscribes to UiEvent channel
6. Start network services (gossip, inbox, etc.)
```

### 15.3 Write Consistency Guarantees

| Guarantee | Mechanism |
|---|---|
| Exactly-once local persistence | SQL PK + UNIQUE constraints, ON CONFLICT DO NOTHING |
| At-least-once transport | Outbox retry with crash recovery |
| No phantom messages | Tombstone check on every insert path |
| Deterministic ordering | per-(conversation, sender) sequence for DM; created_at_ms for gossip |
| Crash-consistent outbox | Sent→Pending reset on startup |
| No silent data loss | Integrity check + future-schema guard |
| Idempotent operations | All mutation methods are safe to call twice |
| Atomic multi-table writes | Immediate transactions for every mutation |

---

## 16. Open Questions & Future Work

1. **Tombstone pruning policy** — Safe after the gossip replay window (24h). Should be configurable (default: 90 days) and run as a maintenance step.
2. **Batch tombstone protocol** — Conversation-level deletion has local support but no protocol message for peer propagation.
3. **GC for orphan file objects** — `content_hash` values referenced by zero rows should be periodically purged, respecting `downloads` in-progress state.
4. **SQLite encryption** — No file-level encryption today. For higher threat models, consider SQLite SEE or OS-level encryption (eCryptfs, LUKS).
5. **Streaming file (de)duplication** — Very large files use `blob_hash` (iroh-blobs reference) instead of inline `data`. The dedup boundary between inline and blob-referenced files needs a size threshold policy.
6. **Cross-device sync for profile/friends** — Currently single-device. A future peer-to-peer sync protocol could replicate `friends`, `profiles`, and `friend_requests` across user devices.
7. **Retention policy for message history** — No automatic expiry for chat messages. Configurable retention (30/90/365 days or forever) could be added as a user-facing feature.
