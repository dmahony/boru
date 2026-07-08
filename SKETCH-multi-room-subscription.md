# Sketch: Keep one room subscribed while switching the visible chat room

## Problem

Right now every frontend subscribes to exactly one topic at a time.  When you
switch rooms the old `GossipTopic` is dropped, the gossip actor sees
`still_needed() == false`, sends a `Quit`, and removes all state for that
topic.  Messages arrive only for the currently visible room.  This means:

- You never receive messages from rooms you aren't looking at.
- Pings, file shares, and friend-status updates from other rooms are lost.
- Switching back requires a fresh `subscribe()` call and re-joining the mesh.

## Goal

Keep a background subscription alive for every room the user has visited.
When the user switches to a different room, the UI shows the new room's
chat log while the old room stays subscribed in the background, receiving
messages silently.  When the user switches back, all missed messages are
already in that room's event buffer.

---

## 1. Current room-switch lifecycle (traced per frontend)

### 1a. Gossip actor side (src/net.rs)

```
gossip.subscribe(topic, peers)
  → RpcMessage::Join { topic_id, bootstrap }
  → Actor::handle_rpc_msg → creates TopicState if absent
  → joins command_rx to stream_group (keyed by topic)
  → spawns topic_subscriber_loop (broadcast rx → mpsc tx to GossipReceiver)
  → sends ProtoCommand::Join(bootstrap) to protocol state
```

When the `GossipTopic` (or both halves) is dropped:

```
command_rx stream returns None
  → state.command_rx_keys.remove(&key)
  → state.still_needed()? → false (no more command rx, no more event rx)
  → self.quit_queue.push_back(topic_id)
  → process_quit_queue → ProtoCommand::Quit → state.states.remove(&topic_id)
  → self.topics.remove(&topic_id)
```

**Key insight:** The actor already holds `topics: HashMap<TopicId, TopicState>`
and protocol `states: HashMap<TopicId, topic::State>`.  There is no limit on
the number of concurrent topic subscriptions.  Each topic is completely
independent — separate HyParView member list, separate PlumTree broadcast
tree, separate connection sets.  The architecture already supports multi-room
subscription; the frontends just don't use it.

**Relevant symbols:**
- `src/net.rs` lines 293, 298-299: `topics: HashMap<TopicId, TopicState>`, `quit_queue: VecDeque<TopicId>`
- `src/net.rs` lines 812-845: `TopicState { neighbors, event_sender, command_rx_keys }`, `still_needed()`
- `src/net.rs` lines 655-665: `process_quit_queue()` — sends `Quit`, removes topic state
- `src/proto/state.rs` lines 159, 278-279: `states.remove(&topic)` on Quit

### 1b. Modular iced frontend (examples/iced_chat/)

**Room enter** (`app.rs:333-374`, `AppMessage::OpenRoom`):
1. `leave_current_room()` — aborts old `forward_handle`, drops `sender`, clears state
2. `gossip.subscribe(topic, vec![])` — creates new `GossipTopic`
3. `sub.split()` → `(GossipSender, GossipReceiver)`
4. `task::spawn(forward_gossip_events(receiver, net_tx))` — starts forwarding
5. Sets `self.sender = Some(sender)`
6. Stores the handle in `self.forward_handle`

**Room leave** (`app.rs:232-241`, `leave_current_room()`):
1. `handle.abort()` — kills the forwarding task
2. `self.sender = None` — drops the GossipSender
3. Clears entries, names, pending_file

**Event routing** (`app.rs:652-657`):
- `NetEvent` arrives on shared `net_rx` channel (no topic info)
- `update_room_preview()` + `handle_net_event()` process it

### 1c. Monolithic iced frontend (examples/chat-gui.rs)

Same pattern, factors the subscribe logic into `subscribe_to_topic()` (line 514).

### 1d. TUI frontend (examples/chat.rs)

Simplest — single room forever.  No room-switch at all; always connected to
the topic from `main()`.

---

## 2. Can the app hold multiple GossipTopic handles at once?

**Yes, trivially.**

- `GossipApi::subscribe()` is an `&self` method on a `Clone` handle.
- Each call returns a fresh `GossipTopic` with its own sender/receiver.
- The gossip actor keeps per-topic state in `topics: HashMap<TopicId, TopicState>`.
- Multiple forward-gossip tasks can pump into the same `net_tx` channel
  concurrently (unbounded channel, no backpressure).

Only two things prevent multi-room use today:

1. **The frontends drop the old handles when switching.**  This is
   intentional — the code is written for single-room — but the
   infrastructure below it handles N topics with no changes.

2. **The `NetEvent` type in `chat_core.rs` has no topic discriminator.**
   `forward_gossip_events()` receives from a topic-bound `GossipReceiver`
   and sends to a shared channel, but the topic identity is lost in
   transit.  Events arrive at the frontend without telling the handler
   which room they belong to.

---

## 3. Protocol and cleanup implications

- **No protocol changes needed** at the iroh-gossip level.  The protocol
  already routes messages by topic; each topic subscription is its own
  independent mesh with its own connections.

- **No change to `TopicState::still_needed()` logic.**  The actor already
  manages per-topic lifetime correctly.  If a subscriber stays open (not
  dropped), the topic stays alive.  If the user wants to explicitly leave
  a room, they'd drop the `GossipSender` handle (same as today's quit
  flow).

- **Memory per room:** ~a few KB for topic state (HyParView tables,
  PlumTree eager/lazy sets, `TOPIC_EVENT_CAP=256` event broadcast buffer,
  `TOPIC_EVENTS_DEFAULT_CAP=2048` per-subscriber mpsc channel).  For
  typical usage (5-20 rooms) this is negligible.

- **Connection cost per room:** Each topic maintains up to
  `active_view_capacity=5` connections to peers.  If two rooms share some
  peers, the connections are separate (each topic's HyParView is
  independent).  This is inherent in the protocol design — each topic is a
  separate swarm.

---

## 4. Minimum file-level change set

### 4a. `src/chat_core.rs` — Tag `NetEvent` with `topic: TopicId`

Add a `topic` field to `NetEvent` variants so the frontend can route.

**Before:**
```rust
pub enum NetEvent {
    Message { from: PublicKey, message: Message },
    NeighborUp { peer: PublicKey },
    NeighborDown { peer: PublicKey },
    Closed,
    Error(String),
}
```

**After:**
```rust
pub enum NetEvent {
    Message { topic: TopicId, from: PublicKey, message: Message },
    NeighborUp { topic: TopicId, peer: PublicKey },
    NeighborDown { topic: TopicId, peer: PublicKey },
    Closed,
    Error(String),
}
```

### 4b. `src/chat_core.rs` — Thread topic through `forward_gossip_events`

Change signature to accept a `topic: TopicId` to annotate events:

```rust
pub async fn forward_gossip_events(
    topic: TopicId,
    mut receiver: GossipReceiver,
    net_tx: tokio::sync::mpsc::UnboundedSender<NetEvent>,
) {
    while let Ok(Some(event)) = receiver.try_next().await {
        match event {
            Event::Received(msg) => match SignedMessage::verify_and_decode(&msg.content) {
                Ok((from, message)) => {
                    if net_tx.send(NetEvent::Message { topic, from, message }).is_err() {
                        return;
                    }
                }
                ...
            },
            Event::NeighborUp(id) => {
                ...
            }
            ...
        }
    }
}
```

### 4c. `examples/iced_chat/app.rs` — Keep active subscriptions per room

The main structural change.  Replace single-sender/forward-handle with a map:

```rust
// In IcedChat struct:
let subscriptions: HashMap<TopicId, GossipSender>;  // keep sender alive so topic stays subscribed
// Remove: sender: Option<GossipSender>
// Remove: forward_handle: Option<JoinHandle>
```

- On `OpenRoom` / `CreateNewRoom` / `JoinFromTicket`:
  - Subscribe if not already subscribed (check map)
  - Mark the room as "visible" — switch the active chat panel
  - The forwarding task stays alive; events come through the shared channel
- On `RoomOpened`:
  - Insert `(topic, sender)` into map
  - Set active screen to `Chat { topic }`
  - Clear and populate entries for that room (from its own event buffer)
- On `NetEvent` arrival:
  - Route to the matching room's entry buffer based on `event.topic`
  - If it matches the active room, display it
- On explicit leave (`/leave` or `DeleteRoom`):
  - Remove the sender from `subscriptions` (drops GossipSender → actor sends Quit)
  - Remove room from history

The trickiest part is that `AppState` currently has one `entries` vec.
Each room needs its own entry buffer.  Change to `HashMap<TopicId, Vec<ChatEntry>>`
or store room data in a dedicated struct.

### 4d. `examples/iced_chat/app.rs` — Per-room state storage

```rust
struct RoomState {
    entries: Vec<ChatEntry>,
    names: HashMap<PublicKey, String>,
    pending_file: Option<(String, String)>,
    ticket_str: String,
    // forward_handle lives in the JoinHandle — no need to store it
}
```

Then in `IcedChat`:

```rust
// ── Current active room ──
active_topic: TopicId,
// ── All subscribed rooms ──
rooms: HashMap<TopicId, RoomState>,
// ── Alive GossipSenders (keeps subscription alive) ──
subscriptions: HashMap<TopicId, GossipSender>,
```

### 4e. `examples/chat-gui.rs` — Same pattern as 4c/4d

Same structural changes: `HashMap<TopicId, (RoomState, GossipSender)>` in
state, route `NetEvent` by `topic`, switch visible screen.

### 4f. `examples/chat.rs` — Minimal change (optional)

The TUI is single-session.  Could optionally support room-switch via a
`/join` command that subscribes without dropping the old one.  Since the
TUI uses `AppState` (from `chat_core`), it would need the same entry-buffer
per-topic change, but this is lower priority.

### 4g. Update callers of `forward_gossip_events`

All three frontends plus the `chat_core.rs` spawn sites need the new
`topic` argument:

- `examples/chat.rs` line 457: `task::spawn(chat_core::forward_gossip_events(receiver, net_tx))`
- `examples/chat-gui.rs` line 541-543: `task::spawn(async move { forward_gossip_events(receiver, tx).await })`
- `examples/iced_chat/app.rs` line 309: `task::spawn(forward_gossip_events(receiver, net_tx))`
  and lines 354, 427 (same pattern)
- `examples/iced_chat/main.rs` — no direct call, goes through `IcedChat` methods

Each becomes:
```rust
task::spawn(forward_gossip_events(topic, receiver, net_tx))
```

---

## 5. Recommended approach (ordered)

### Step 1 (foundation): Tag NetEvent with topic

Change `src/chat_core.rs` first — the `NetEvent` enum and
`forward_gossip_events` signature.  This is a self-contained change that
touches one file.  Fix all callers to pass `topic`.

Result: every event carries a topic id.  Frontends ignore it for now
(backward-compatible since `NetEvent` is frontend-internal — `chat_core.rs`
doesn't consume it, only the examples do).

Build and test all examples after this step.

### Step 2 (one frontend): Modular iced_chat multi-room

The iced_chat frontend is already the most structured (has `Screen::ChatList`
vs `Screen::Chat { topic }`).  Convert it:

1. Replace `sender: Option<GossipSender>` / `forward_handle: Option<JoinHandle>`
   with `subscriptions: HashMap<TopicId, GossipSender>`.
2. Store per-room state in `rooms: HashMap<TopicId, RoomState>`.
3. Route `NetEvent` by `event.topic` on arrival.
4. On `GoToChatList`: just switch the visible screen, don't abort the
   forwarding task or drop the sender.
5. On `/leave` / `DeleteRoom`: explicitly remove the sender from the map
   (triggers actor-side Quit via drop).

### Step 3 (second frontend): Monolithic chat-gui

Apply the same pattern as Step 2 to `examples/chat-gui.rs`.

### Step 4 (third frontend, optional): TUI chat.rs

If multi-room support is desired in the TUI, add the same room-state map.
The TUI is simpler (no chat-list screen), so `/join <ticket>` would stay
in the same render loop but switch the visible room.

### Step 5 (cleanup): Consider background event buffering

With multiple rooms subscribed, events arrive for all rooms.  The per-room
`forward_gossip_events` task pushes into the shared `net_tx`.  If the
frontend is not processing the events fast enough, the unbounded channel
grows.  Consider:

- Adding a bounded per-room event buffer (e.g.,
  `tokio::sync::mpsc::channel(2048)`) per room, with the frontend reading
  from the active room's buffer directly and draining non-active buffers.
- Or relying on the bounded `GossipReceiver` channel (capacity
  `TOPIC_EVENTS_DEFAULT_CAP = 2048`) as backpressure — the gossip actor
  drops events for slow subscribers.

The simplest approach: keep the unbounded `net_tx` as-is and limit the
number of background rooms to a reasonable count (e.g., 20).  Rooms beyond
that could warn the user or auto-leave oldest.

---

## 6. Files cited

| File | Role |
|---|---|
| `src/chat_core.rs` | `NetEvent` enum (line 334), `forward_gossip_events` (line 590) |
| `src/api.rs` | `GossipTopic` (line 212), `GossipReceiver` (line 271), `Event` (line 336), `GossipSender` (line 172) |
| `src/net.rs` | `TopicState` (line 813), `still_needed()` (line 835), `process_quit_queue()` (line 655), `handle_rpc_msg` join (line 598) |
| `src/proto/state.rs` | `State::states: HashMap<TopicId, topic::State>` (line 159), `states.remove(&topic)` on Quit (line 278) |
| `src/proto/topic.rs` | `Command::Quit` (line 168), `InEvent::Command(Command::Quit)` handling in `handle()` (line 276) |
| `examples/iced_chat/app.rs` | `leave_current_room` (line 232), `OpenRoom` (line 333), `GoToChatList` (line 278), `NetEvent` routing (line 652) |
| `examples/chat-gui.rs` | `subscribe_to_topic` (line 514), `GoToChatList` (line 561), `OpenRoom` (line 588) |
| `examples/chat.rs` | Single-room event loop (line 495), no room-switch support |
