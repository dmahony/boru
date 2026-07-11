# Concurrency & Rejoin Behavior in Cleanup — Review

## Scope

Analyzed `src/room_cleanup.rs` (the core `delete_room_history` helper) and
`examples/iced_chat/app.rs` (the GUI frontend) for race conditions, partial
cleanup, and synchronization gaps during room switching, deletion, and rejoin.

---

## Finding 1: NetEvent has no topic scope — stale events cross-contaminate rooms

**Severity: Medium (data corruption)**

`NetEvent` (defined in `src/chat_core.rs:782–807`) carries no `topic` field.
When the app switches rooms:

1. `forward_handle.abort()` signals the old room's forward task to stop.
2. The abort is async — it fires at the next `.await` point inside
   `forward_room_events_for_chat`.  Events already enqueued in `net_tx` →
   `net_rx` remain in the channel.
3. The `AppMessage::NetEvent` handler (line 2400) processes **every** event
   without checking room affinity.  `chat_net_event` calls `push_remote`,
   `push_system`, `set_name`, etc. directly on `self` — modifying the
   **currently visible room's** state.
4. The `forward_room_events_for_chat` loop ends by sending `NetEvent::Closed`
   (line 837 of `room_docs.rs`), which is also untagged.

**Impact:** Messages, name changes, and neighbor events from the old room leak
into the new room's entry list and names map.  The window is small (depends on
channel depth and task scheduler latency) but real.

**Missing guard:** Either add a `topic` field to `NetEvent` and filter in the
handler, or clear `entries` + `names` and drain the `net_rx` channel between
`leave_current_room()` and setting up the new room.

---

## Finding 2: `/leave` does incomplete cleanup vs `DeleteRoom`/`ConfirmDeleteRoom`

**Severity: Medium (leaked state)**

The `/leave` command handler (lines 1855–1878 of `app.rs`) manually removes
`room_history` and `chat_history` entries for the topic, but **skips**:

- **Outbox** — pending/queued messages for the left room persist.
- **Friends store** — `friend_records` still reference the room topic.
- **Active-room file** (`room.json`) — survives and will be loaded on restart.
- **Legacy room-history file** (`rooms.json`) — left behind.

Meanwhile `purge_room_history()` (lines 3571–3614), used by
`DeleteRoom`/`ConfirmDeleteRoom`, performs a full cascade through
`delete_room_history()` covering all stores.

**Impact:** Rejoining the room after `/leave` encounters stale outbox entries
and friend metadata.  The active-room file can cause the room to be
auto-loaded on restart even though the user left it.

**Fix:** `/leave` should call `purge_room_history(self.topic)` instead of
manual partial cleanup.

---

## Finding 3: Deleting active room leaves live subscription

**Severity: Medium (potential crash / cross-contamination)**

The `GoToChatList` handler explicitly keeps the room subscription alive (line
1157: "Keep the room subscription alive so returning is instant").  If the
user then deletes this room from the chat-list UI:

1. `ConfirmDeleteRoom(topic)` → `purge_room_history(topic)` cleans stores.
2. `leave_current_room()` is **NOT** called — `forward_handle`, `sender`, and
   the gossip subscription all stay alive.
3. Incoming gossip events for the now-deleted room are still processed into
   `self.entries`, `self.names`, etc.
4. If the user then opens a new room, the old forward task (for the deleted
   room) is still running and its events mix with the new room's state.
5. When the user does eventually switch rooms, `leave_current_room()` aborts
   the deleted room's forward handle — not a currently active room's handle.

**Fix:** In `ConfirmDeleteRoom`, check if `topic == self.topic` and call
`leave_current_room()` + navigate to `Screen::ChatList` if so.  Similarly
for `DeleteRoom`.

---

## Finding 4: `pending_topic` is dead code — no guard against stale async completions

**Severity: Medium (race condition)**

`self.pending_topic` is declared (line 449) and set to `None` in `RoomOpened`
and `RoomJoinFailed`, but is **never assigned `Some(...)`** anywhere in the
codebase.  This means there is no mechanism to reject stale async task
completions.

**Sequence that triggers the race:**

1. User clicks Room A → `OpenRoom(topic_A)` spawns async task A.
2. User clicks Room B (before task A completes) → `OpenRoom(topic_B)` spawns
   async task B.
3. Task B completes first → `RoomOpened` sets up Room B correctly.
4. Task A completes → `RoomOpened` overwrites:
   - `self.forward_handle` with task A's handle (task B's handle is lost)
   - `self.sender` with Room A's sender
   - `self.screen` and `self.topic` to Room A
   - Entries, names, and chat history are all set up for Room A

**Impact:** UI snaps back to the first room after briefly showing the second
room.  Task B's forward handle is orphaned (aborted only when the user
switches rooms again, which then aborts A's handle instead of B's).

**Fix:** Set `self.pending_topic = Some(topic)` in each entry point
(`OpenRoom`, `CreateNewRoom`, `JoinFromTicket`) and check in `RoomOpened`:
```rust
if self.pending_topic != Some(topic) {
    return iced::Task::none(); // stale
}
```

---

## Finding 5: `forward_handle_slot` timing gap under rapid room switching

**Severity: Low (self-healing)**

The `forward_handle_slot` is an `Arc<StdMutex<Option<JoinHandle>>>`.  The
intended pattern is:

1. `leave_current_room()` aborts both `forward_handle` and the slot handle.
2. Async task stores its new handle in the slot.
3. `RoomOpened` takes the handle from the slot into `forward_handle`.

The gap: if `leave_current_room()` runs **after** the async task has already
stored its handle in the slot (e.g., the in-flight task completes between
step 1 and the spawn of a new task), the new handle is aborted unnecessarily.
In practice this is self-healing because the next room switch correctly sets
up a fresh subscription, but it wastes subscriptions and broadcasts.

**Impact:** Transient duplicate subscriptions during rapid room switching.

**Fix:** Use a generation counter or `tokio::sync::watch` channel instead of
a shared `Mutex<Option<JoinHandle>>` to unambiguously associate handles with
their generation.

---

## Finding 6: `self_sent_events` never cleared on room switch

**Severity: Low (memory leak, minor correctness)**

`self_sent_events` (a `HashMap<MessageHash, u64>`) is populated when a
message is sent (line 2334) and read during echo / ReadReceipt processing
(lines 2411, 2472).  It is **never cleared** in `leave_current_room()` or
anywhere else.

**Impact:**
- The map grows without bound over the session lifetime.
- After switching rooms, if a peer from the old room processes a very old
  message and sends a ReadReceipt, the receipt lookup succeeds but the
  `self.entries.iter_mut().find(|e| e.event_id == event_id)` silently
  misses — harmless today but fragile.

**Fix:** Clear `self_sent_events` in `leave_current_room()`.

---

## Finding 7: The `delete_room_history` helper is well-isolated and correct

**Positive finding**

The core cleanup function in `src/room_cleanup.rs` operates on explicit
mutable references to each store (`RoomHistoryStore`, `ChatHistoryStore`,
`OutboxStore`, `FriendsStore`).  It:

- Is idempotent (tested — `delete_room_history_is_idempotent`).
- Correctly cascades across all stores.
- Only deletes the active-room file when it matches the target topic.
- Has no global/shared state — all operations are deterministic given inputs.

No synchronization issues within the helper itself; the concerns are
entirely in how it is called from the frontend.

---

## Finding 8: No concurrency tests

Tests in `src/room_cleanup.rs` are single-threaded and sequential.  There
are no tests for:
- Concurrent deletion while gossip events are in-flight.
- Rejoin-after-delete with stale events in the channel.
- Multiple rapid room switches.
- Deleting a room while subscribed.

---

## Summary

| # | Issue | Severity | Location |
|---|-------|----------|----------|
| 1 | Stale NetEvents cross-contaminate rooms (no topic scoping) | Medium | `app.rs:2400`, `chat_core.rs:1051` |
| 2 | `/leave` does incomplete cleanup vs `DeleteRoom` | Medium | `app.rs:1869–1874` |
| 3 | Deleting active room leaves live subscription | Medium | `app.rs:3538–3544` |
| 4 | `pending_topic` is dead code — stale async completions not rejected | Medium | `app.rs:449,1431` |
| 5 | `forward_handle_slot` timing gap under rapid switching | Low | `app.rs:960,1387` |
| 6 | `self_sent_events` never cleared | Low | `app.rs:2334,2411` |
| 7 | `delete_room_history` is correct (positive) | — | `room_cleanup.rs` |
| 8 | No concurrency/race tests | — | `room_cleanup.rs, app.rs` |
