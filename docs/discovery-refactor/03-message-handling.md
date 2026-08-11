# BORU-DISC-03: Inbound message handling trace — discovery/chat separation point

Audit scope: committed worktree (`wt/t_5c2db929`) after `git fetch origin && git merge origin/main`
(origin/main @ ce170822, BORU-CARGO-10). This is a read-only trace; no runtime code was changed.
All line numbers were verified against the current worktree, not copied from predecessor docs
(several shifted since BORU-DISC-01/02).

## Executive finding

Inbound gossip is decoded and dispatched through one generic pipeline that is **topic-tagged but
not topic-kind aware**. The only piece of the receive path that is intentionally discovery-only is
the *raw startup lobby receiver drain* in `main.rs` — and it is a separate subscription from the
one the GUI creates. The GUI's `OpenRoom`/`BackgroundSubscribe` forwarders treat the canonical
lobby topic exactly like a conversation topic: the same `ConversationNetEvent` channel, the same
`handle_net_event_for_topic` dispatch, the same persistence (`message_store.db`), the same
rendering/unread/sidebar machinery. **Discovery traffic and chat traffic are currently
indistinguishable once a message reaches the GUI forwarder path.** The earliest safe separation
point is the forwarder-spawn boundary (`spawn_conversation_forwarder` call sites), with a
defensive topic-kind guard at the `AppMessage::NetEvent` handler — this is the location the
Phase-4 routing guard (BORU-DISC-10) must occupy.

---

## 1. End-to-end trace of an inbound lobby gossip event

### Stage 0 — Gossip actor → raw receiver (transport)

Two separate subscriptions exist for the canonical lobby topic (documented in BORU-DISC-02):

1. **Startup raw subscription** — `examples/iced_chat/main.rs:1110-1113` computes
   `IcedChat::default_lobby_topic()` and calls `gossip.subscribe(lobby_topic, Vec::new())`.
2. **GUI slow-path subscription** — `examples/iced_chat/app.rs:11867-11901` (OpenRoom task) calls
   `gossip.subscribe(topic, bootstrap_peers)` (or `subscribe_and_join` with a 30 s timeout at
   `:11876-11897`), then `sub.split()` at `:11902`.

The directory topic is a third, unrelated gossip mesh (`main.rs:1285-1362`); it carries
`RoomAdvertisement` messages only and is separate from both the lobby and the future discovery
topic (guardrail: "do not merge into the internal discovery topic" — BORU-DISC-01 §Adjacent paths).

### Stage 1 — Raw receiver drain (discovery-only, message-discarding)

`main.rs:1179-1218` drains the **startup** lobby receiver. It forwards only
`Event::NeighborUp`/`Event::NeighborDown`:

- `NeighborUp` → dynamic joiner (`NeighborEvent::Up`) + Discover-sidebar channel
  (`DiscoveredPeersUpdate { added }`) at `:1189-1201`.
- `NeighborDown` → `NeighborEvent::Down` + `DiscoveredPeersUpdate { removed }` at `:1202-1214`.
- **Every other event (including `Event::Received` chat payloads) falls into `_ => {}` at
  `:1215` and is dropped.**

This receiver is the model for a discovery-owned subscription: it keeps the mesh healthy (join/leave
fanout) and never creates chat state. Its sender is handed to the DHT tracker
(`main.rs:1126-1166`) and the mDNS joiner (`main.rs:1220+`), but its receiver never produces a
`NetEvent`.

### Stage 2 — GUI forwarder bridge (metadata/roster consumed, chat forwarded)

The GUI's own subscriptions (OpenRoom slow path, BackgroundSubscribe, create-room, join-from-ticket)
all converge on `spawn_conversation_forwarder` (`src/conversations.rs:674-726`), which:

1. Spawns `forward_room_events_for_chat` (`src/room_docs.rs:1128-1262`) against the gossip receiver.
2. That forwarder consumes room-doc messages locally:
   - Metadata marker `0xFE` → `process_gossip_event(&metadata_doc, ...)` (`room_docs.rs:1155-1176`)
   - Roster marker `0xFF` → `process_roster_event(&roster_doc, ...)` (`room_docs.rs:1178-1195`)
   - Marker is only a fast hint; structural decode gates consumption so a legitimate
     `SignedMessage` starting with `0xFE`/`0xFF` still decodes as chat.
3. Chat / neighbour events are decoded with `SignedMessage::verify_and_decode` at
   `room_docs.rs:1198`, wrapped in `NetEvent::Message { from, message, sent_at }` at `:1200-1204`.
   `NeighborUp`/`NeighborDown` are wrapped directly (`:1232-1248`). `Lagged` and
   `MissingMessages` are dropped (`:1250-1257`).
4. `spawn_conversation_forwarder` then **tags each event with the topic**:
   `ConversationNetEvent::new(topic, event)` at `conversations.rs:704`, pushed to the shared
   `net_tx` channel (`main.rs:1373-1376`, capacity 256).

Call sites (all identical in shape, all currently use `safety: None`):

| Site | Purpose | Location |
|---|---|---|
| Create private room | new room subscription | `app.rs:11578-11585` |
| OpenRoom slow path (lobby + user rooms) | room open | `app.rs:11983-11990` |
| Join-from-ticket | private room join | `app.rs:12821-12828` |
| BackgroundSubscribe | auto-subscribe stored conversations | `app/discover.rs:1836-1843` |

### Stage 3 — Iced event-loop subscription

`main.rs:1671-1690` starts Iced with `IcedChat::subscription(...)`; the combined subscription
stream (`app.rs:17772+`) reads the shared `net_rx` and emits `AppMessage::NetEvent(conv_event)`
at `app.rs:17849`.

### Stage 4 — `AppMessage::NetEvent` handler (first topic-aware dispatch)

`app/chat.rs:4788-4950`:

- `conv_event.topic` is read immediately (`:4790`) — the event is **topic-tagged** here.
- User-visible events (text/file/image/GIF) bump sidebar ordering:
  `conversation_store.touch_and_bump` + `chats_sidebar_revision` at `:4796-4799`
  (filtered by `_is_user_visible_event`, `app.rs:15520-15532`).
- Inactive conversations get a sidebar preview update (`update_room_preview`, `:4804-4806`,
  implemented at `app.rs:15456-15504`).
- `conversations.entry(topic).or_insert_with(|| ConversationLive::new(topic))` at `:4807-4810` —
  **any topic is materialized as a live conversation on first event**, including the lobby.
- `NeighborUp`/`NeighborDown` update per-conversation neighbor sets and sender readiness
  (`:4815-4875`), with a 500 ms `join_peers` retry on NeighborDown (`:4864-4872`).
- **Inactive branch** (`:4876-4913`): only user-visible events are queued
  (`should_count` at `:4884`); non-visible protocol events early-return (`:4885-4887`). Visible
  events go into `conversation.pending_events` (bounded by `MAX_PENDING_EVENTS`) and increment
  `conversation.unread` (`:4894-4912`). Notification emission is deliberately deferred here
  (`:4890-4893` comment).
- **Active branch** (`:4915-4949`): `unread = 0`, then `process_net_event_sync`, pending transfer
  drains, profile-image tickets, and link-preview fetch tasks.

### Stage 5 — `process_net_event_sync` (topic-aware post-processing)

`app.rs:15613-15830`:

- Telemetry for message delivery (`:15618-15646`).
- **Direct-topic special case** (`:15647-15676`): if `direct_topic(local, from) == *topic`, ensures
  the friend record and upserts an active direct `ConversationEntry`. Only for direct topics.
- **`RoomAdvertisement` special case** (`:15677-15701`): verifies signature and upserts into
  `directory_store`; returns early. This is the directory-topic payload handler, present on the
  shared channel because directory traffic and conversation traffic share the same gossip decode.
- Ordering bump + preview (`:15705-15709`).
- **Core dispatch:** `handle_net_event_with_safety_for_topic(event.clone(), self, safety, Some(topic))`
  at `:15711-15718` (`self.public_room_safety` is applied here — the forwarders pass `None`).
- Delivery-state echo for self-messages (`:15722-15744`), auto ReadReceipt broadcast when
  `follow_latest` (`:15748-15783`), ReadReceipt→Seen transitions (`:15786-15813`), LatencyPing→Pong
  (`:15821+`).

### Stage 6 — `handle_net_event_for_topic` (shared protocol dispatch)

`src/chat_core/net_event.rs:167-567`:

- `NetEvent::Message` → dedup via `SEEN_MESSAGES` (`:191-216`), blocked/muted checks
  (`:222-235`), TTL/future-skew checks (`:237-267`), diagnostics dedup (`:274-296`), then a
  `match message` over every protocol variant (`:298-567`).
- **The `topic: Option<TopicId>` parameter is used only for**:
  - diagnostics records (`PeerJoinedRoom`/`PeerLeftRoom`, `MessageReceived`, `ProbeReceived`);
  - `accepts_group_peer(topic, from)` in the friend/group gating for FileShare/ImageShare/SharedGif
    (`:389`, `:431`, `:442`);
  - `persist_remote_message(topic, ...)` / `persist_remote_file_share(topic, ...)` calls.
- **There is no topic-kind routing.** Text messages always call
  `cb.persist_remote_message(...)` then `cb.push_remote(...)` (`:330-369`). FileShare always calls
  `cb.persist_remote_file_share(...)`, `cb.push_system(...)`, and `cb.set_pending_file/folder`
  (`:371-427`). ImageShare/GIF call `cb.set_pending_image/gif` (`:428-449`). AboutMe calls
  `cb.set_name` and may `cb.push_system` (`:298-324`). A payload arriving on the lobby topic is
  treated identically to one arriving on a direct/group topic.
- NeighborUp/Down record diagnostics and call `cb.on_neighbor_status_change` (`:569-625`).

### Stage 7 — `ChatCallbacks` implementations

Trait: `src/chat_callbacks.rs:144-452`.

- **IcedChat** (`examples/iced_chat/app.rs`): `push_system` `:16483-16486`; `push_remote`
  `:16488-16498`; `persist_remote_message` `:16500-16533` — writes into SQLite `message_store.db`
  via `MessageStore::insert_chat_message` keyed by topic bytes; `persist_remote_file_share`
  `:16535-16567`; `set_pending_file` `:16581-16638` (creates a download card entry);
  `set_pending_folder` `:16640-16668`; `set_pending_image` `:16670-16672`;
  `set_pending_gif` `:16674-16681`; edit/delete/reaction `:16687-16720+`.
- **AppState** (`src/chat_core/state.rs:164-313+`, used by headless/TUI/tests): `push_system`
  `:233-235`; `push_remote` `:237-255`; `set_pending_file/image/gif` `:257-280`; edit/delete/
  reaction `:288-313+`. Note: AppState does **not** override `persist_remote_message`, so the
  no-op default applies in the headless path.

### Stage 8 — Rendering path (ChatEntry construction)

`push_remote`/`push_system` → `entries_push` (`app.rs:9177-9230`):

- Dedup by `message_hash` (`:9178-9182`).
- Avatar handle cache (`:9198-9214`).
- `entry.update_cache()` + layout cache append (`:9215-9222`).
- `self.entries.push`, `index_entry`, `keep_latest_visible`, image budget, entry cap
  (`:9223-9229`; cap calls `save_room_to_history` at `:9291`).

`ChatEntry` struct: `app.rs:2545-2619`. `ChatEntry::remote` `:2783-2819`; `ChatEntry::system`
`:2717-2749`; `ChatEntry::system_download` `:2874+`. Rendering happens in `view_chat_log` /
windowed renderer using `entries`.

### Stage 9 — Persistence paths

- **Durable inbound text/file persistence:** `persist_remote_message`/`persist_remote_file_share`
  (`app.rs:16500-16567`) write `message_store.db` rows keyed by `topic.as_bytes()`. **A lobby
  message would be persisted here today** — no topic-kind guard exists.
- **Room history previews:** `update_room_preview` (`app.rs:15456-15504`) updates
  `room_history` preview/sender; `RoomOpened` upserts the room record at `app.rs:12486`.
- **History replay on open:** `RoomOpened` replays SQLite rows + legacy `chat_history.json`
  entries into the timeline (`app.rs:12308-12406`), overlays outgoing delivery states
  (`:12382-12405`, `:12408-12430`).
- **Background replay:** `BackgroundSubscribed` replays `chat_history.json` for auto-subscribed
  conversations (`app/discover.rs:1877-1920`).
- **Legacy JSON store:** `ChatHistoryStore` (`src/chat_history.rs`) is still used for replay; its
  save hook is a documented no-op since SQLite became the write target (BORU-DISC-01 cites
  `app.rs:8049-8054`; verified present at that range).
- **Room metadata:** `RoomStore` / `src/room.rs`, `RoomHistoryStore` / `src/room_history.rs`
  (`upsert` `:189`, `update_preview` `:206`, `update_preview_with_sender` `:213`).

### Stage 10 — Unread counts, notifications, typing/read-receipt/attachments

- **Unread:** only the inactive-conversation branch increments `conversation.unread`
  (`app/chat.rs:4894-4912`). The sidebar reads it from the live `conversations` map
  (`app/sidebar.rs:547-551`) and renders the badge (`sidebar.rs:1073-1081`). The lobby is subject
  to this because it is materialized as a `ConversationLive` on the OpenRoom path
  (`app.rs:12527-12541`).
- **Notifications:** `emit_message_notification` (`app.rs:15539-15594`) is **dead code** — it is
  never called. `app/chat.rs:4890-4893` explicitly defers message notifications (double-borrow
  refactor). Only `emit_incoming_call_notification` (`app.rs:15599-15611`, called from
  `app/calls.rs:201`) is wired to the notification service. So today the GUI emits **no**
  desktop notification for inbound chat messages at all — the PDF's "no notification for
  discovery" requirement has no live path to guard yet, but any future wiring of
  `emit_message_notification` must be discovery-guarded.
- **Typing indicators:** removed from both frontends (protocol `Message::Typing` no longer
  exists; `handle_net_event` match has no Typing arm).
- **Read receipts:** `ReadReceipt` messages update delivery icons (`net_event.rs:532-535`); auto
  ReadReceipt broadcast at `app.rs:15748-15783`; `ReadReceipt→Seen` at `:15786-15813`. Both run on
  the active topic regardless of topic kind.
- **Attachments:** FileShare/ImageShare/SharedGif → `set_pending_file/folder/image/gif` →
  pending queues drained in the NetEvent handler (`app/chat.rs:4920-4937`) and via
  `drain_pending_transfers`; downloads flow through `blob_store`/`download_manager`.

### Stage 11 — Lobby-specific special cases embedded in the generic path

- `app.rs:12163-12186` — after OpenRoom subscription, if `topic == default_lobby_topic()`, join
  pending discovered peers to the lobby sender (discovery work inside a room-open path).
- `app.rs:12527-12541` — keep the lobby `ConversationLive` in `self.conversations` so its
  `GossipSender` survives room switches (the central UI/chat coupling identified in BORU-DISC-01/02).
- `app.rs:14064` — MCP diagnostic `OpenRoom`-style action treats the canonical lobby as a known
  room without room history.
- `app.rs:16107-16121` — `should_announce_new_peer` suppresses "new peer" system entries for
  non-friend lobby participants, but only after the generic event path has already run.
- `mcp_server.rs:2451` — the MCP server hashes the stale literal `b"iroh-gossip-chat/default-lobby/v1"`
  for its diagnostic lobby room id; this is NOT the canonical `public_lobby_topic` derivation
  (BORU-DISC-01 §GUI helper and stale alternate derivation). Any discovery guard keyed on topic id
  must not treat this stale hash as canonical.

---

## 2. Which topics flow through each stage, and is the stage topic-aware?

| # | Stage | Lobby topic traffic today? | Direct/group/public traffic today? | Topic-aware? |
|---|---|---|---|---|
| 0 | Gossip subscribe (`main.rs:1112`, `app.rs:11867`, `discover.rs:1801`) | Yes | Yes | **Yes** — subscription is per-topic; caller knows the topic at subscribe time |
| 1 | Raw startup lobby drain (`main.rs:1179-1218`) | Yes (NeighborUp/Down only; chat dropped) | No | **Yes** — bound to the lobby subscription; intentionally discovery-only |
| 2 | `forward_room_events_for_chat` (`room_docs.rs:1128-1262`) | Yes | Yes | **No** — receiver is bound to one topic but the function never inspects topic identity |
| 2b | `spawn_conversation_forwarder` bridge (`conversations.rs:674-726`) | Yes | Yes | **Yes** — receives `topic` and tags every event (`ConversationNetEvent::new(topic, event)`) |
| 3 | Iced subscription (`app.rs:17849`) | Yes | Yes | Yes — carries `conv_event.topic` |
| 4 | `AppMessage::NetEvent` handler (`app/chat.rs:4788-4950`) | Yes | Yes | **Yes — topic-tagged but not topic-KIND aware** (lobby == any conversation) |
| 5 | `process_net_event_sync` (`app.rs:15613+`) | Yes | Yes | Partially — direct-topic and RoomAdvertisement special cases, but no Discovery kind |
| 6 | `handle_net_event_for_topic` (`net_event.rs:167-567`) | Yes | Yes | **No** — `topic` used for diagnostics/gating/persistence only; no kind routing |
| 7 | `ChatCallbacks` impls (`app.rs:16483+`, `state.rs:164+`) | Yes | Yes | No — no topic parameter on `push_remote`/`push_system`/`set_pending_*`; `persist_remote_*` receives `Option<TopicId>` only |
| 8 | Rendering (`ChatEntry`/`entries_push`, `app.rs:9177+`) | Yes | Yes | No — operates on the current conversation's entries |
| 9 | Persistence (`message_store.db`, `room_history`) | Yes | Yes | Keyed by topic bytes only; no kind |
| 10 | Unread/sidebar (`sidebar.rs:547-551`, `:1073-1081`) | Yes | Yes | Keyed by topic only |
| 10b | Notifications | **None today** (dead code) | **None today** | n/a until wired |

**Conclusion:** the pipeline is topic-tagged end-to-end after `spawn_conversation_forwarder`, but
no stage distinguishes Discovery from Conversation. Every stage that writes chat state
(persistence, ordering, preview, unread, rendering, read receipts) is reachable by lobby traffic.

---

## 3. Earliest separation point (feeds BORU-DISC-10 routing guard)

The topic identity is known **at subscribe time** (`main.rs:1110-1113` startup raw subscription;
`app.rs:11867` OpenRoom; `app/discover.rs:1800` BackgroundSubscribe; `app.rs:11578` create-room;
`app.rs:12768` join-from-ticket). The earliest practical point to introduce a
`TopicKind::{Discovery, Conversation}` classification, in increasing strictness:

1. **Forwarder-spawn boundary (recommended primary):** `spawn_conversation_forwarder` call sites
   (`app.rs:11578, 11983, 12821`, `app/discover.rs:1836`) and/or inside
   `src/conversations.rs:674-726`. Before a discovery-topic receiver is handed to a chat
   forwarder, classify the topic: Discovery ⇒ keep the raw-drain model (`main.rs:1179-1218`) or a
   dedicated `DiscoveryService`; Conversation ⇒ current forwarder path. This is the earliest point
   at which the app decides what a subscription is *for*, and it is exactly the PDF Phase-4 guard
   shape: `match topic_kind { Discovery => discovery_service.handle(payload),
   Conversation(id) => conversation_service.handle(id, payload) }`.
2. **Defensive guard (recommended secondary):** the `AppMessage::NetEvent` handler
   (`app/chat.rs:4788`) — the first point inside the frontend where `conv_event.topic` is
   available before ANY conversation side effect (ordering bump `:4796`, `ConversationLive`
   creation `:4807`, preview `:4804`, unread `:4894`, `process_net_event_sync` `:4917`). A guard
   here — early-return or route to the discovery service when
   `classify_topic(topic) == TopicKind::Discovery` — makes every later stage unreachable for
   discovery payloads without touching protocol dispatch.
3. **Earliest possible (transport):** at `gossip.subscribe` itself. The classifier must exist
   before the receiver is split; the startup raw subscription (`main.rs:1110-1113`) already
   behaves this way for the lobby. A future `DiscoveryService` would own this subscription.

**Recommended classifier shape:** a pure function `topic_kind(topic: TopicId) -> TopicKind`
centralizing the current ad-hoc `topic == default_lobby_topic()` checks (`app.rs:12164, 12527,
14064, 16117`) plus the future `BORU_DISCOVERY_TOPIC_V1` identity (Phase 2, BORU-DISC-05+). The
classifier must NOT rely on `mcp_server.rs:2451`'s stale hash, and must keep direct/group/public
topics classified as `Conversation` so the hard rule (private direct messages and normal chat
payloads never routed through discovery) is enforced by construction.

## Migration checklist (for BORU-DISC-05+)

1. Add `TopicKind` + `topic_kind()` classifier; keep `default_lobby_topic()` and all existing
   direct/group/public derivations untouched unless demonstrably wrong.
2. At forwarder-spawn sites (`app.rs:11578, 11983, 12821`, `discover.rs:1836`), route Discovery
   topics to a discovery-owned receiver path instead of `spawn_conversation_forwarder`.
3. Add the defensive guard at `app/chat.rs:4788` before `touch_and_bump`/`ConversationLive`/
   preview/unread/`process_net_event_sync`.
4. Remove/relocate the lobby special cases (`app.rs:12163-12186`, `:12527-12541`, `:14064`,
   `:16117`) so discovery state never lives in `self.conversations`.
5. Ensure `persist_remote_message`/`persist_remote_file_share` (`app.rs:16500-16567`) and
   `save_room_to_history` (`app.rs:10411-10452`) are unreachable for Discovery topics — the guard
   in (3) covers them.
6. If/when `emit_message_notification` (`app.rs:15539`) is wired, guard it with the same
   classifier; today it is dead code and needs no discovery action.
7. Keep the directory-topic mesh (`main.rs:1285-1362`) separate from the internal discovery topic.
8. Resolve the MCP stale-lobby-hash (`mcp_server.rs:2451`) before any lobby removal/migration.
