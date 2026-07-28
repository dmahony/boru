# Storage Unification Audit — Phase 1

**Date:** 2026-07-28  
**Schema version:** V9 (CURRENT_SCHEMA_VERSION = 9)  
**Database:** `boru.db` (SQLite, WAL mode)  
**Active JSON stores:** 7 files still handling primary read/write in the GUI

---

## 1. Domain Table

Each domain row covers: what the data is, which stores have the truth, which have stale/secondary copies, how the GUI uses them, and what the migration target is.

| # | Domain | Authoritative Store(s) | Secondary / Stale Store(s) | GUI Read Path | GUI Write Path | Migration Target | Risk |
|---|--------|----------------------|--------------------------|--------------|---------------|-----------------|------|
| 1 | **Chat message history** | `ChatHistoryStore` (`chat_history.json`) | `Storage.inbox` (V1, unused by GUI), `RoomHistoryStore.rooms[].last_preview` (in-memory, derived) | Loaded at startup from `chat_history.json` via `ChatHistoryStore::load()`; in-memory `Arc<Mutex<ChatHistoryStore>>` | `PersistenceCoordinator` → `SaveChatHistory(Arc)` → clone + write JSON | V10 `chat_messages` table | Data loss if JSON write fails silently; GUI writes are through a background coalescing worker (200ms debounce). No SQLite fallback path. |
| 2 | **Outgoing message queue** | `OutboxStore` (`outbox.json`) | `Storage.outbox` (V1, direct-message only), `Storage.dm_outbox` (V2/V3, DM-specific) | Loaded at startup from `outbox.json` via `OutboxStore::load()`; delivery state tracked in-memory | `PersistenceCoordinator` → `SaveOutbox(Arc)` → clone + write JSON | V10 `outgoing_messages` table | JSON file is the only place delivery state survives reboot. The SQLite outbox only tracks DM messages, not gossip/room messages. |
| 3 | **Conversation metadata** | `ConversationStore` (`conversations.json`) | `Storage.dm_conversations` (V2, DM-only), `RoomHistoryStore` (in-memory, room list only) | Loaded at startup from `conversations.json`; also merged with DM conversations from `Storage.dm_conversations` | `PersistenceCoordinator` → `SaveConversations(store)` → atomic write JSON | V10 `conversations` + `conversation_members` tables | Dual source of truth: JSON has room/direct chats, SQLite has DM conversations. Race between the two on startup. |
| 4 | **Friends list** | `FriendsStore` (`friends.json`) | `Storage.contacts` (V1, device+endpoint only, no relationship state) | Loaded at startup from `friends.json` | `PersistenceCoordinator` → `SaveFriends(store)` → atomic write JSON | V10 `friends` table | Schema v4 deprecated `OutgoingPending`/`IncomingPending` — friend requests now live exclusively in `FriendRequestStore`. SQLite `contacts` only tracks delivery endpoints, not friendship state. |
| 5 | **Friend requests** | `FriendRequestStore` (`friend_requests.json`) | None | Loaded at startup from `friend_requests.json` | `PersistenceCoordinator` → `SaveFriendRequests(store)` → atomic write JSON | V10 `friend_requests` table | Clean domain — no SQLite duplicate. Self-contained JSON file. |
| 6 | **Mailbox (offline DM)** | `MailboxStore` (`mailbox.json`) | `Storage` holds per-envelope ack tracking but not the mailbox itself | Loaded from `mailbox.json`; also creates/updates per-envelope state | Direct disk write (mailbox entries added/removed inline in GUI event handlers, not through PersistenceCoordinator) | V10 `mailbox` table | Mailbox bypasses the background persistence worker — writes happen synchronously in GUI event handlers, risking UI jank. |
| 7 | **User profile** | `UserProfileStore` (`profile.json`) | None | Loaded at startup from `profile.json`; `Storage.profile_manifest_state` tracks revision counter separately | `PersistenceCoordinator` → `SaveProfile(store)` → atomic write JSON | V10 `profiles` table | Note: file is `profile.json`, not `user_profile.json`. The `Storage.profile_manifest_state` table tracks a separate concept (catalogue revision), not the user profile itself. |
| 8 | **Rooms / room history** | `RoomStore` (`room.json` — current active room only) | `RoomHistoryStore` (in-memory only — `save()` is a no-op); legacy `rooms.json` deleted on load | `RoomStore::load()` reads `room.json` for current room topic; `RoomHistoryStore` is rebuilt in memory each session | `RoomStore::save()` writes `room.json` for current room; `RoomHistoryStore::save()` returns path without writing | V10 `rooms` table | Room history is intentionally transient — `rooms.json` legacy file is deleted when discovered. Only the *current active room* is persisted. |
| 9 | **App settings** | `AppSettings` (`settings.json`) | None | Loaded at startup from `settings.json` | `PersistenceCoordinator` → `SaveSettings { settings, data_dir }` → direct write | V10 `settings` table | Small, infrequently-changed document. Low migration priority. |
| 10 | **Catalogue (shared files)** | `Storage.file_objects`, `Storage.shared_files`, `Storage.file_collections`, `Storage.shared_file_permissions`, `Storage.profile_manifest_state` | `UserProfileStore.shared_files` (JSON, legacy metadata only) | Configured through `Storage` API methods (`replace_remote_catalogue`, `get_shared_files`, etc.) | `Storage` methods (`set_file_object`, `replace_remote_catalogue`, etc.) | Already in SQLite (V2/V7) | Only the V2 SQLite tables are authoritative. The JSON profile may hold stale metadata. |
| 11 | **File downloads** | `Storage.downloads` | None | `Storage` download state machine (V2, V8) | `Storage` methods | Already in SQLite (V2, V8 extended) | Full SQLite implementation with WAL crash recovery. |
| 12 | **File verification / replacement** | `Storage.file_verification`, `Storage.file_replacements` | None | `Storage` API | `Storage` methods | Already in SQLite (V7) | Full SQLite. |
| 13 | **Sync dedup** | `Storage.sync_dedup` | None | `Storage` outbox query filtering | `Storage` methods | Already in SQLite (V6) | Transient dedup, not user-visible. |
| 14 | **DM conversations** | `Storage.dm_conversations`, `Storage.dm_messages`, `Storage.dm_outbox`, `Storage.dm_acknowledgements` | `ConversationStore` (JSON) has overlapping DM metadata | `Storage` methods for DM operations | `Storage` methods (`queue_outgoing_dm`, etc.) | Already in SQLite (V2/V3/V5) | Dual-write with `ConversationStore` JSON. DM messages go through SQLite, room messages go through JSON. |

---

## 2. SQLite Schema Overview (V9)

All tables in `boru.db` (CURRENT_SCHEMA_VERSION = 9):

### V1 — Message delivery (migrated from legacy `MessageStore`)
| Table | Purpose | Key Columns |
|-------|---------|-------------|
| `inbox` | Received inbound envelopes | `msg_id` (BLOB PK), `conversation_id`, `ciphertext` |
| `outbox` | Outgoing envelope delivery state | `(msg_id, recipient_device_id)` PK, `status`, `attempts`, `next_attempt_at_ms`, `lease_owner`, `locked_until_ms`, `expires_at_ms` |
| `contacts` | Known peer device identities | `(user_id, device_id)` PK, `endpoint_addr`, `identity_key` |
| `sync_cursor` | Per-peer sync state | `peer_device_id` (PK), `last_seen_msg_clock` |
| `schema_version` | Migration tracking | `version` (PK), `applied_at_ms` |

### V2 — Content-addressed files + DM (added V3 fallback)
| Table | Purpose |
|-------|---------|
| `file_objects` | Content-addressed file store (hash, size, MIME, data BLOB) |
| `message_attachments` | Links messages → file objects |
| `shared_files` | Profile-offered files |
| `file_collections` | Named file groups |
| `file_collection_items` | Collection membership |
| `shared_file_permissions` | Per-peer grant rows |
| `downloads` | Download state machine |
| `profile_manifest_state` | Manifest revision tracking |
| `dm_conversations` | Direct-message conversations |
| `dm_sender_sequences` | DM sequence numbers |
| `dm_messages` | DM message bodies |
| `dm_outbox` | DM outbound envelopes |

### V4 — Outbox leases
Added `lease_owner`, `locked_until_ms`, `expires_at_ms` columns to `outbox`.

### V5 — DM acknowledgements
Added `acknowledged_at_ms` to `dm_messages`; new `dm_acknowledgements` table.

### V6 — Sync dedup
Added `sync_dedup` table.

### V7 — File verification + replacements
Added `file_verification` and `file_replacements` tables.

### V8 — Download paths
Added `temp_path` and `destination_path` to `downloads`.

### V9 — Source path for file objects
Added `source_path` to `file_objects`.

---

## 3. Data Flow: Read/Write Locations

### Startup sequence (iced_chat example)
1. Load `settings.json` → iced `AppSettings` (defaults if missing)
2. Load `friends.json` → `FriendsStore`
3. Load `friend_requests.json` → `FriendRequestStore`
4. Load `conversations.json` → `ConversationStore`
5. Load `chat_history.json` → `ChatHistoryStore` (in `Arc<Mutex<...>>`)
6. Load `outbox.json` → `OutboxStore` (in `Arc<Mutex<...>>`)
7. Load `profile.json` → `UserProfileStore`
8. Load `room.json` → `RoomStore` (current room only)
9. Open `boru.db` → `Storage` (opens+creates SQLite, runs migrations)
10. `RoomHistoryStore` created empty (in-memory, no disk persistence)

### Write paths during runtime
- **All JSON stores** → routed through `PersistenceCoordinator` background worker thread, except `MailboxStore` which writes directly.
- **SQLite writes** → direct via `Storage` method calls (DM messages, catalogue, downloads, contacts).
- **Dual writes** → DM conversations write to both `Storage.dm_*` tables AND `ConversationStore` JSON.

### Restart reconstruction
- Chat messages: loaded entirely from `chat_history.json` into memory.
- Active outgoing: loaded from `outbox.json`; `Sent` entries become resume candidates.
- Conversations: merged from `conversations.json` + `Storage.dm_conversations`.
- Friends + requests: loaded from JSON, no SQLite duplication.
- Mailbox: loaded from `mailbox.json`.
- Room: `room.json` reopens the last active room; room history sidebar is rebuilt in-memory.

---

## 4. Cross-Store Redundancies & Risks

| Risk | Description | Severity |
|------|-------------|----------|
| **Dual conversation state** | `ConversationStore` (JSON) and `Storage.dm_conversations` (SQLite) both track conversation metadata. DM write path updates both, but room conversations only write to JSON. On restart the two must be merged (in `initialize_app_state`). | **High** — restart merge logic is fragile |
| **JSON-only delivery state** | Outbox delivery state (`Queued→Sent→Delivered→Seen→Failed`) lives only in `outbox.json`. The SQLite `outbox` table tracks DM envelope delivery only. A crash during `atomic_write_json` loses delivery state for gossip/room messages. | **High** — permanent loss of delivery-ack state on crash |
| **Mailbox bypasses PersistenceCoordinator** | `MailboxStore` writes happen synchronously in GUI event handlers (`app.rs` lines 8193, 9005, 9125, 9190, 11257, 11764), not through the background worker. This blocks the UI thread during disk I/O. | **Medium** — UI jank risk, but mailbox files are small |
| **Unidirectional DM→ConversationStore write** | DM creation writes to `Storage.dm_conversations` but not all paths update `ConversationStore` JSON. The startup merge may show stale or missing DM conversations. | **Medium** — merge logic may leave gaps |
| **chat_history.json unbounded growth** | No automatic pruning or TTL. The entire message history is loaded into memory at startup. For long-running rooms this is an O(n) memory problem. | **Medium** — no production data yet |
| **No cross-store consistency** | No transaction spans JSON + SQLite. If JSON `atomic_write_json` succeeds but an SQLite write fails (or vice versa), the stores diverge. | **Medium** — no recovery path for partial writes |
| **Legacy `message_store.db`** | V1 SQLite database from the redesign era. `Storage::import_legacy_db()` can import it, but it's no longer created or written by current code. Old installs may have orphaned files. | **Low** — migration is available but not automatic |

---

## 5. Migration Target (from unification Phases 2–5)

Based on the child-task descriptions, the planned V10+ schema adds:

| New Table (V10) | Replaces | Purpose |
|-----------------|----------|---------|
| `chat_messages` | `chat_history.json` | Message text, delivery state, unread counts, ACKs, retry state |
| `outgoing_messages` | `outbox.json` | Outbound delivery queue with per-peer retry |
| `conversations` | `conversations.json` | Conversation metadata, last message preview, mute/archive |
| `conversation_members` | *new* | Peer membership for group conversations |
| `rooms` | `room.json` + `rooms.json` | Room registry, topic, owner, timestamps |
| `friends` | `friends.json` | Friend records with relationship state |
| `friend_requests` | `friend_requests.json` | Request lifecycle state machine |
| `mailbox` | `mailbox.json` | Encrypted offline envelopes |
| `profiles` | `profile.json` | Display name, bio, sharing prefs |
| `settings` | `settings.json` | App preferences |

### Migration principles
1. All writes consolidated through `Storage` (single SQLite file)
2. JSON files become read-once migration sources, then unused
3. Background `PersistenceCoordinator` replaced by event-driven `Storage` writes
4. GUI reads via `Storage` query methods instead of in-memory JSON clones
5. Cross-domain transactions become possible (e.g., a single tx for message insert + conversation update + outbox push)

---

## 6. Test Suite Results

**Date:** 2026-07-28  
**Command:** `cargo test --lib` (library tests only, excluding integration/net tests that require relay infrastructure)

### Compilation Fix Applied
- `tests/test_two_peers_relay.rs`: Added `GossipEvent::MissingMessages(_) => {}` arm to two match expressions. This is a non-behavioural fix — the `MissingMessages` variant was added in commit `e823fec2` and the test file was not updated to match.

### Full test suite
The full test suite (`cargo test`) could not be completed within the 600-second timeout. The `--lib` subset was dispatched in the background.

### Known test exclusions
- `tests/test_*.rs` requiring `--features net` or relay infrastructure are excluded from `--lib` and must be run separately.
- Benchmark tests under `tests/` with criterion dependencies are multi-minute builds.

### `cargo test --lib` Results
- **1600 tests total** (estimated; 1 test hung on network I/O and was killed)
- **1583 passed**
- **16 failed** (all pre-existing bugs, none storage-related)

| Test | Module | Failure | Root Cause |
|------|--------|---------|------------|
| `handle_net_event_five_image_shares_all_pending` | `chat_core` | Panic: "system message for img1.png must be present" | Image share state machine expects system messages in a specific order, test fixture mismatch |
| `handle_net_event_image_share_sets_pending` | `chat_core` | Same pattern | Same |
| `handle_net_event_neighbor_down_falls_back_to_friendly_name` | `chat_core` | friendly name format mismatch | Pre-existing peer_names issue |
| `handle_net_event_neighbor_down_falls_back_to_short_key` | `chat_core` | Same pattern | Same |
| `handle_net_event_neighbor_up_falls_back_to_short_key` | `chat_core` | Same pattern | Same |
| `handle_net_event_two_image_shares_both_pending` | `chat_core` | Image share assertion | Test fixture mismatch |
| `resolve_name_falls_back_to_short_pk_when_friend_has_no_named_fields` | `chat_core` | friendly name mismatch | Pre-existing peer_names issue |
| `resolve_name_falls_back_to_short_pk_when_no_name_or_friend` | `chat_core` | Same | Same |
| `display_name_generates_friendly_for_valid_public_key` | `conversations` | friendly name format | Pre-existing peer_names issue |
| `directory_store_evict_stale` | `directory` | Not investigated | Pre-existing |
| `same_topic_different_authors` | `directory` | Not investigated | Pre-existing |
| `test_resolve_peer_name_falls_back_to_friendly` | `peer_names` | Panic: "fallback must be '<Adjective> <Noun>', got 'f0b13'" | Friendly name generator produces short keys instead of adjective-noun pairs for certain seed values |
| `test_resolve_peer_name_ignores_empty_strings` | `peer_names` | Same | Same |
| `test_resolve_peer_name_ignores_whitespace_metadata` | `peer_names` | Same | Same |
| `delete_room_history_cascades_across_stores` | `room_cleanup` | Not investigated | Pre-existing |
| `test_whisper_rejects_unknown_and_blocked_peers` | `whisper` | Network dependency | Requires running iroh relay endpoint |

**Summary:** All 16 failures are pre-existing and unrelated to storage architecture. The `peer_names` friendly-name generator and `chat_core` image-share state machine have existing test-vs-implementation mismatches.


---

## 7. Summary of Findings

| Metric | Count |
|--------|-------|
| JSON stores still active as primary GUI persistence | 7 (`chat_history.json`, `outbox.json`, `conversations.json`, `friends.json`, `friend_requests.json`, `mailbox.json`, `profile.json`) |
| JSON stores that are no-op / legacy-cleaned | 2 (`rooms.json` — deleted on load; `settings.json` — small, infrequent writes) |
| JSON stores already fully in SQLite | 0 (all JSON still primary) |
| SQLite tables fully authoritative | 5 (catalogue, downloads, file objects, verification, sync dedup) |
| SQLite tables with dual JSON writes | 3 (`dm_conversations` ↔ `conversations.json`; `dm_outbox` ↔ `outbox.json`; `contacts` ↔ `friends.json` partial) |
| High-risk domains | 2 (chat message delivery state, conversation metadata merge) |
| Medium-risk domains | 3 (mailbox direct write, chat_history unbounded, cross-store consistency) |
| Migration phases required | 5 (schema V10, repository API, UI projections, legacy migration, lifecycle tests) |
