# Public Rooms — Hybrid DHT + Gossip Registry Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Persist public room data, make rooms globally discoverable via DHT, add MCP tools, and fix member counts — unifying the two disconnected discovery systems.

**Architecture:** Keeps random-topic rooms but adds DHT-published metadata records alongside peer discovery, persists DirectoryStore to SQLite, uses gossip directory as a push-refresh channel, and adds proper room lifecycle management.

**Tech Stack:** Rust, SQLite (rusqlite), Mainline DHT (distributed-topic-tracker), iroh gossip, postcard serialization, MCP JSON-RPC.

---

## Phase 1: Persist DirectoryStore to SQLite

### Task 1: Add `directory_ads` table via migration v11

**Objective:** Define SQLite schema for persisting room advertisements.

**Files:**
- Modify: `src/storage.rs` — bump `CURRENT_SCHEMA_VERSION` to 11, add `migrate_v11`
- Modify: `src/directory.rs` — no schema changes yet, just prep

**What to do:**
1. Bump `CURRENT_SCHEMA_VERSION` from 10 to 11
2. Add `migrate_v11` method creating `directory_ads` table:
   ```sql
   CREATE TABLE IF NOT EXISTS directory_ads (
       topic BLOB NOT NULL,
       author BLOB NOT NULL,
       room_name TEXT NOT NULL,
       description TEXT NOT NULL DEFAULT '',
       ticket TEXT NOT NULL,
       member_count INTEGER NOT NULL DEFAULT 0,
       last_activity INTEGER NOT NULL,
       received_at_ms INTEGER NOT NULL,
       PRIMARY KEY (topic, author)
   );
   ```
3. Register migration in `run_migrations` match
4. Run existing tests to confirm migration works

**Verification:** `cargo test -p boru-core --lib storage -- --nocapture` passes

---

### Task 2: Add persistence methods to DirectoryStore

**Objective:** Add SQLite-backed save/load to DirectoryStore without changing its public API.

**Files:**
- Modify: `src/directory.rs` — add `save_to_db`, `load_from_db` methods
- Modify: `src/bin/boru/app.rs` — wire up save calls

**What to do:**
1. Add `save_to_db(&self, conn: &rusqlite::Connection)` that upserts all entries
2. Add `load_from_db(conn: &rusqlite::Connection) -> Self` that loads all entries
3. Call `save_to_db` after `upsert` (for individual insertions)
4. Call `load_from_db` during app startup after storage init
5. Add periodic save on `ConnMonitorTick`

**Verification:** Create a room, restart app, verify room persists in PUBLIC ROOMS sidebar

---

## Phase 2: DHT Room Metadata Publishing

### Task 3: Extend discovery record payload to carry room metadata

**Objective:** Add room name and ticket to the DHT discovery record so peers can discover rooms globally.

**Files:**
- Modify: `src/discovery_record.rs` — add `room_name`, `ticket` fields to `DiscoveryRecordPayload`
- Modify: `src/public_room_tracker.rs` — update publish to include metadata
- Modify: `src/bin/boru/app.rs` — pass room metadata when starting tracker

**What to do:**
1. Add optional `room_name: Option<String>` and `ticket: Option<String>` to `DiscoveryRecordPayload`
2. Bump `DISCOVERY_RECORD_CONTENT_VERSION` to 2
3. Make v1 records still parseable (backward compat) — treat missing fields as `None`
4. Update `create_discovery_record` to accept optional metadata
5. Wire room name + ticket through `PublicRoomTracker::publish_once()`
6. Start ContinuousTracker for user-created rooms, not just public-lobby

**Verification:** `cargo test -p boru-core --lib discovery_record` passes

---

### Task 4: Subscribe to DHT-discovered room topics automatically

**Objective:** When a peer discovers room metadata via DHT, automatically subscribe to the room's gossip topic and join the mesh.

**Files:**
- Modify: `src/bin/boru/app.rs` — handle discovered rooms from DHT

**What to do:**
1. When `ContinuousTracker` returns discovered peers with room metadata, upsert into `DirectoryStore`
2. Auto-background-subscribe to newly discovered room topics
3. Wire the discovery callback into the `ConnMonitorTick` event loop

**Verification:** Room created on one VM appears on the other without manual ticket exchange

---

## Phase 3: Fix Member Counts & Add MCP Tools

### Task 5: Track room-specific member counts

**Objective:** Replace `self.neighbors.len()` (lobby count) with room-specific neighbor tracking.

**Files:**
- Modify: `src/bin/boru/app.rs` — add `room_neighbor_counts: HashMap<TopicId, u32>`

**What to do:**
1. Maintain a `HashMap<TopicId, u32>` mapping rooms to their active neighbor counts
2. Update on `NeighborUp`/`NeighborDown` for each topic
3. Use room-specific count when broadcasting `RoomAdvertisement`
4. Display accurate count in PUBLIC ROOMS sidebar

**Verification:** Join a room with 2 peers, verify member_count shows 2 (not lobby count)

---

### Task 6: Add MCP tools for public room operations

**Objective:** Add `boru_list_public_rooms`, `boru_create_public_room`, `boru_delete_public_room` to the MCP server.

**Files:**
- Modify: `src/bin/boru/mcp_server.rs` — add 3 new handlers
- Modify: `src/diagnostics.rs` — add new GuiTestCommand variants if needed

**What to do:**
1. `boru_list_public_rooms` — read-only, returns all entries from `DirectoryStore`
2. `boru_create_public_room(name, description?)` — one-call room creation + advertise
3. `boru_delete_public_room(topic)` — remove advertisement from store + gossip unpublish
4. Register in the bridge's `ALL_TOOLS` list

**Verification:** Use `boru_list_public_rooms` via MCP, confirm existing rooms appear

---

### Task 7: Room lifecycle — delete and stale eviction

**Objective:** Add GUI "Delete Room" button for advertised rooms, and auto-evict stale rooms.

**Files:**
- Modify: `src/bin/boru/app.rs` — add `DeleteRoomAdvertisement` handler
- Modify: `src/directory.rs` — add `remove` method

**What to do:**
1. Add "Delete" button next to "Join" in PUBLIC ROOMS sidebar (only for rooms created by local user)
2. `DeleteRoomAdvertisement` handler: remove from `DirectoryStore` + SQLite, stop gossip broadcast
3. Call `directory_store.evict_stale()` on `ConnMonitorTick` (evict ads older than 1 hour)
4. Also call SQLite DELETE for evicted entries

**Verification:** Create room, delete it, confirm it's gone from sidebar and DB

---

## Implementation Order (dependency chain)

```
Task 1 (migration) → Task 2 (persistence) → Task 5 (member counts)
                                           → Task 7 (lifecycle)
Task 3 (DHT record) → Task 4 (DHT subscribe)
Task 2 (persistence) → Task 6 (MCP tools)
```
