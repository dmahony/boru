# LAN Conversation & History Persistence — Diagnosis

## Architecture Summary

The chat has three tiers of conversation storage:

| Tier | Module | Persistence | Granularity |
|------|--------|-------------|-------------|
| Durable conversation list | `ConversationStore` (conversations.rs) | `conversations.json` atomic JSON | Per-topic metadata (name, kind, last_seen, archived) |
| Durable message history | `ChatHistoryStore` (chat_history.rs) | `chat_history.json` atomic JSON | Per-message entries with delivery state |
| In-memory conversation state | `ConversationLive` (app.rs) | None (lost on restart) | Draft text, in-flight events, entries list |

**Message delivery pipeline** (LAN/offline):

```
mDNS discovery → gossip join_peers → GossipReceiver events
  → spawn_conversation_forwarder (conversations.rs:463)
    → ConversationNetEvent tagged with topic
      → iced subscription (app.rs:13628)
        → AppMessage::NetEvent handler (app.rs:6888)
          → process_net_event_sync (app.rs:9817)
            → ConversationStore upsert + save
            → handle_net_event_with_safety_for_topic
              → ChatCallbacks::push_remote → entries_push
```

---

## Critical Defects

### 1. Durable ConversationEntry NOT created for background (inactive) conversations

**Location:** `examples/iced_chat/app.rs:6888–6903, 9822–9833`

**What happens:** When a `ConversationNetEvent` arrives for a topic that is NOT the currently active conversation (line 6899), the event is queued in the `ConversationLive` in-memory struct and `unread` is incremented. The code then returns `iced::Task::none()` — **`process_net_event_sync` is never called**.

Inside `process_net_event_sync` (lines 9822–9833), the durable `ConversationEntry` is created via `upsert`, but this code path is only reachable when the event topic matches the active conversation topic. For background conversations, the entry is never written to `conversation_store`.

**Impact:** A user receives a LAN/offline DM from a peer they haven't explicitly "opened a conversation" with. The message is displayed in the chat if they switch to that topic, but:
- No `ConversationEntry` exists in `conversations.json` → the conversation doesn't appear in the conversation list after restart
- `conversation_store.touch_and_bump(&topic)` at line 6894 is a no-op for unknown topics (returns `None` gracefully but silently)
- The `conversation_store.save()` at line 9837 is never called for background events

**Proposed fix:** Move the `ConversationEntry` upsert logic OUT of `process_net_event_sync` and into the `AppMessage::NetEvent` handler, before the active-topic guard. Check whether a `ConversationEntry` exists for the topic and create one if missing for all message events, not just active-conversation ones.

### 2. ConversationStore.save() called only on explicit actions — no periodic recovery

**Locations:** `conversation_store.save()` is called at:
- `app.rs:9599` — `OpenConversation` handler
- `app.rs:9621` — `CloseConversation` handler
- `app.rs:9837` — `process_net_event_sync` (only for active topic)

There is NO `save()` call on app exit, no periodic tick, and no `Drop` implementation. If the app crashes between events, the following are lost:
- `last_seen_at_unix_ms` updates from incoming messages
- `archived` status changes
- Any changes made via `touch_and_bump` that never reached a save call

**Proposed fix:** Add a periodic conversation_store save (e.g., every 30s in the subscription loop) or save in the `Drop`/exit path. At minimum, save after every `touch_and_bump` call that returns `Some` (indicating a real entry was bumped).

### 3. MailboxReplayed (offline DM sync) bypasses ConversationStore entirely

**Location:** `examples/iced_chat/app.rs:9560–9584`

The `MailboxReplayed` handler pushes messages directly to `self.entries` via `entries_push` but never creates a `ConversationEntry` in the durable store, never calls `touch_and_bump`, and never saves the conversation store.

**Impact:** Offline DMs received via the inbox sync protocol are displayed in the active chat, but no durable conversation record is created for the sending peer. If the app restarts before the user manually opens a conversation with that peer, the offline messages are visible in history but the conversation list shows nothing.

**Proposed fix:** In the `MailboxReplayed` handler, derive the conversation topic from the peer key, upsert a `ConversationEntry`, and save the store.

### 4. ChatHistoryStore thread-safety gaps

**Location:** `chat_history` is `Arc<Mutex<ChatHistoryStore>>`. Multiple code paths hold and release the lock:

- `save_room_to_history` (line 4273) — pushes entries, does NOT save
- `process_net_event_sync` (line 9858–9864) — updates delivery state AND saves
- `SendPressed` handler (line 9657) — pushes + saves

Since these all lock and unlock independently, the observable interleaving is:
```
Thread A: lock → push entry → unlock
Thread B: lock → update delivery state (entry may not exist yet!) → unlock
Thread A: lock → save → unlock
```

The `update_delivery_state` in thread B can fail because the entry pushed by thread A hasn't been flushed yet. This race is partially mitigated because delivery state updates only happen for previously-sent (self) messages that were already persisted before broadcast, but the gap still exists for edge cases like rapid send+echo.

**Proposed fix:** Use a single `Mutex` scope for atomic push+save operations, or use a message queue instead of immediate save.

### 5. Group messages never create a durable ConversationEntry

**Location:** `app.rs:9822–9833`

The conversation entry upsert guard is:
```rust
if *from != self.local_public && direct_topic(&self.local_public, from) == *topic {
```

This only matches direct one-to-one topics. Group conversations (where the topic is not a `direct_topic`) are never auto-created. If a group chat message arrives and no `ConversationEntry` for that group topic exists yet, the message is displayed but no durable record is created.

**Proposed fix:** Also upsert a `ConversationEntry` with `kind: ConversationKind::Group` when the topic doesn't match `direct_topic` (i.e., for non-DM topics) and no entry exists yet.

### 6. `persist_room_history` uses detached thread with ignored error

**Location:** `app.rs:9782–9785`
```rust
let store = self.room_history.clone();
let _ = std::thread::spawn(move || {
    let _ = store.save();
});
```

The save runs on a detached OS thread. Rapid room switches could spawn many threads. The `room_history.save()` is intentionally a no-op (it just returns the file path without writing), but the pattern is risky for future changes.

**Proposed fix:** Remove the thread spawn since the save is a no-op, or use a dedicated serial queue if actual IO is needed.

### 7. `save_room_to_history` drops signed_bytes

**Location:** `app.rs:4299`
```rust
Vec::new(), // signed bytes not available here
```

When saving entries to history storage, the signed bytes are set to an empty vec. This means replayed history entries do not carry the original signed bytes — they cannot be verified or re-broadcast from history. The `delivery_state` field in a re-loaded entry would be `Queued` (default) rather than whatever state the original entry had.

**Proposed fix:** Preserve the signed bytes and delivery state when creating `HistoryEntry` copies in `save_room_to_history`.

---

## Missing Test Coverage

The existing integration test (`test_conversation_integration.rs`) covers 15 scenarios. The following are untested:

| Missing Scenario | Component | Risk |
|-----------------|-----------|------|
| Background message auto-creates ConversationEntry | `ConversationStore.upsert` (active-topic guard bypass) | High — conversations silently missing after restart |
| Multiple simultaneous messages for different topics | In-memory conversation routing | Medium — race in `entry()` / `or_insert_with` |
| ConversationStore.save() crash recovery | Atomic write + periodic save | Medium — lost metadata on crash |
| Offline DM (MailboxReplayed) creates ConversationEntry | MailboxReplayed handler | High — conversation list breaks after restart |
| Group message creates ConversationEntry for unknown topic | NetEvent handler + process_net_event_sync | Medium — group conversations not persisted |
| Restart after only background DMs received | Full restart flow | High — messages visible in history but conversation list empty |
| Delivery state thread-safety under concurrent access | `ChatHistoryStore` + outbox | Medium — stale delivery icons |

---

## Actionable Changes

### Priority 1 (Data loss risk)
1. **Move ConversationEntry upsert before the active-topic guard** in `AppMessage::NetEvent` handler (app.rs:6888). Create entry if `find(&topic)` returns `None` and event is a `NetEvent::Message`.
2. **Add ConversationEntry upsert in MailboxReplayed handler** (app.rs:9560).
3. **Save ConversationStore after every `touch_and_bump`** that returns `Some`, not just after `process_net_event_sync`.

### Priority 2 (Correctness)
4. **Save `conversation_store` periodically** and on app-exit (e.g. `iced::window::Close` event).
5. **Extend the upsert condition in process_net_event_sync** to create group entries when `direct_topic` doesn't match but no entry exists.
6. **Preserve `signed_bytes` in `save_room_to_history`** by storing them alongside entries instead of `Vec::new()`.

### Priority 3 (Resilience)
7. **Remove the detached `thread::spawn`** in `persist_room_history` (save is a no-op).
8. **Add a `ChatHistoryStore` batch API** that pushes+saves atomically to eliminate the interleaving window.

### Files that need changes:
- `examples/iced_chat/app.rs` — lines 6888–6903, 9560–9584, 9782–9785, 9822–9837, 4273–4318
- `src/conversations.rs` — confirm `touch_and_bump` return value is checked
- `src/chat_history.rs` — possibly add an atomic push+save method
