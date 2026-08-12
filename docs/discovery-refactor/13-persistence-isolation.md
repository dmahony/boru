# BORU-DISC-13: Persistence & rendering isolation — blocking discovery traffic from chat state

Audit scope: committed worktree (`wt/t_30ed0a2f`) after `git fetch origin && git merge origin/main`
(origin/main @ 65cabaf4, BORU-DISC-12). Phase 3 of the hidden-discovery refactor
(PDF T11): **no discovery packet may appear as a user message or produce an unread
badge, and discovery traffic must leave zero side effects in chat persistence,
attachments, notifications, typing indicators, or read receipts.**

This document records the audit of every persistence / rendering / notification
surface and the guards that make discovery-topic traffic unreachable from them.
It is the write-up for the code changes shipped in this task.

---

## 1. Threat model / acceptance criterion

The internal discovery gossip topic (`BORU_DISCOVERY_TOPIC_V1`,
`src/discovery_topic.rs`) carries `DiscoveryMessage` payloads only
(`src/discovery_message.rs`) and is owned by `DiscoveryService`
(`src/discovery_service.rs`). The acceptance criterion is:

> No discovery packet can appear as a user message or produce an unread badge.

The stronger invariant enforced here: **discovery-topic traffic cannot reach any
stage of the chat pipeline** — history, message DB, attachments, notifications,
typing indicators, read receipts, rendering, unread counts, room metadata, or
backfill. The guard architecture is layered so a failure at one layer is caught
by the next.

## 2. Layer map (transport → UI), with the guard at each layer

| # | Layer / stage | Location | Discovery traffic today? | Guard / result |
|---|---|---|---|---|
| 0 | Gossip subscription | `main.rs` startup `DiscoveryService::join` (`main.rs:1121-1142`); `src/discovery_service.rs:629-706` | Own subscription, own receiver | Service-owned drain; **no `ConversationNetEvent` is ever produced** |
| 1 | Forwarder-spawn boundary | `spawn_conversation_forwarder` (`src/conversations.rs:674-726`) | Would be handed a discovery receiver on a mis-wiring | **BORU-DISC-10 guard**: `topic_kind(topic) == Discovery ⇒ drain & drop` (never deserializes into chat state) |
| 2 | Iced `NetEvent` bridge | `AppMessage::NetEvent` (`examples/iced_chat/app/chat.rs:4788-4806`) | None today | **BORU-DISC-10 backstop**: discovery-topic events dropped before `touch_and_bump` / `ConversationLive` / preview / unread / `process_net_event_sync` |
| 3 | Room-open catch-all | `AppMessage::OpenRoom` (`app.rs:11654`) | None today | **BORU-DISC-13 (new)**: refuses to open a Discovery topic as a conversation |
| 4 | Room-open async completion | `AppMessage::RoomOpened` (`app.rs:12062`) | None today | **BORU-DISC-13 (new)**: drops `RoomOpened` for a Discovery topic (defense in depth) |
| 5 | Auto-subscribe paths | `SubscribeStoredConversations` / `BackgroundSubscribe` (`app/discover.rs:1735-1836`) | None today | **BORU-DISC-13 (new)**: discovery topics filtered out / refused |
| 6 | Protocol dispatch | `handle_net_event_for_topic` (`src/chat_core/net_event.rs:167-567`) | Unreachable for discovery (guards 1–2) | Downstream of guard; no kind routing needed at this layer |
| 7 | ChatCallbacks (persistence) | `persist_remote_message` / `persist_remote_file_share` (`app.rs:16458+`, `16493+`) | Unreachable today | **BORU-DISC-13 (new)**: explicit `is_discovery_topic` early-return before `MessageStore::insert_chat_message` |
| 8 | Backfill server authorization | `BackfillAuthorizer::authorize` (`src/backfill.rs:233-252`) | Denied implicitly (not group / direct / public room) | **BORU-DISC-13 (new)**: explicit discovery-topic exclusion as the FIRST check |
| 9 | Rendering / unread / notifications | `entries_push`, `conversation.unread`, `emit_message_notification` | Unreachable (guards 2–5) | Downstream of the conversation lifecycle; discovery can never be the active conversation |

## 3. Surface-by-surface audit (the PDF T11 checklist)

### 3.1 Chat history (`ChatHistoryStore`, `src/chat_history.rs`)
- Write path: `entries_push` → `save_room_to_history` (`app.rs:10411-10452`) and
  `RoomOpened` replay; the JSON save hook is a documented no-op (SQLite is the
  write target).
- Reachability for discovery: `entries_push` only runs on the active
  conversation (`self.entries`), and the active conversation can only be a
  Discovery topic if a `RoomOpened` for it succeeded — **guards 3 + 4 prevent
  that**. The `BackgroundSubscribed` replay (`app/discover.rs:1877-1920`) runs
  only for topics that passed guard 5.
- Verdict: **blocked**.

### 3.2 Message DB tables (`message_store.db`, `MessageStore::insert_chat_message`)
- Write path: `persist_remote_message` / `persist_remote_file_share`
  (`app.rs:16458+`, `16493+`) — reached from `handle_net_event_for_topic`
  (gossip + backfill converge here).
- Reachability for discovery: requires a discovery-topic `NetEvent` to survive
  guards 1 + 2. **Guard 7 now adds a belt-and-braces early-return at the
  persistence callback itself**: even a mis-routed discovery payload is never
  written to the DB.
- Verdict: **blocked (two layers)**.

### 3.3 Attachment handling (`set_pending_file/folder/image/gif`)
- Write path: `set_pending_*` (`app.rs:16539-16681`) from
  `handle_net_event_for_topic` (FileShare / ImageShare / SharedGif), drained in
  `AppMessage::NetEvent` (`app/chat.rs:4937-4942`) and `drain_pending_transfers`.
- Reachability for discovery: requires a discovery-topic `NetEvent` to survive
  guards 1 + 2. Guard 7 covers file shares at the persistence layer too.
- Verdict: **blocked**.

### 3.4 Notifications
- `emit_message_notification` (`app.rs:15539-15594`) is **dead code** (never
  called; message notifications are explicitly deferred at `app/chat.rs:4907-4910`
  due to a borrow refactor). Only `emit_incoming_call_notification` is wired.
- Reachability for discovery: **no live path exists today**. The doc record in
  `docs/discovery-refactor/03-message-handling.md` §Stage 10 stands: any future
  wiring of `emit_message_notification` must go through the `NetEvent` guard (2).
- Verdict: **no live path; guard 2 protects any future wiring**.

### 3.5 Typing indicators
- The protocol `Message::Typing` no longer exists; both frontends removed the
  handler. No live path.
- Verdict: **not applicable (removed protocol)**.

### 3.6 Read receipts
- Auto `ReadReceipt` broadcast + `ReadReceipt → Seen` transitions run inside
  `process_net_event_sync` (`app.rs:15706-15813`), which is only reachable from
  `AppMessage::NetEvent` (guard 2) for conversation topics.
- Verdict: **blocked**.

### 3.7 Message rendering (`ChatEntry` / `entries_push`)
- `push_remote` / `push_system` → `entries_push` (`app.rs:9177-9230`) operate on
  the active conversation's `entries`. The discovery topic can never be the
  active conversation (guards 3 + 4) and discovery events never reach
  `push_remote` (guard 2).
- Verdict: **blocked**.

### 3.8 Unread counts / sidebar badges
- `conversation.unread` increments only in the inactive branch of
  `AppMessage::NetEvent` (`app/chat.rs:4893-4930`) — guarded by 2. The sidebar
  badge renders from the live `conversations` map; a Discovery topic can only
  appear there if a `ConversationLive` was created for it (guards 2, 3, 4, 5 all
  prevent that; BORU-DISC-12 removed the last startup-created lobby
  `ConversationLive`).
- Verdict: **blocked**.

### 3.9 Room metadata / room list (`RoomStore`, `RoomHistoryStore`, `RoomMetadata`)
- `RoomOpened` upserts room history + `RoomStore::with_peers` writes peer data
  (`app.rs:12062+`, `:12460-12463`); the room list in `room_history` drives the
  sidebar. All reachable only via a successful room open — guards 3 + 4.
- Verdict: **blocked**.

### 3.10 Backfill
- **Server side**: `BackfillAuthorizer::authorize` (`src/backfill.rs:233`)
  previously denied the discovery topic only implicitly (it is not a group
  epoch, not a deterministic direct topic, and not a public room). Guard 8 now
  denies it **explicitly as the first check** — the discovery topic is declared
  not-a-conversation-store in the policy itself, so it can never be served as
  history even if storage state changes later.
- **Client side**: `pending_backfill_topics` is only populated in `RoomOpened`
  (`app.rs:12449-12458`) — unreachable for discovery (guard 4). `SubscribeStoredConversations`
  (guard 5) filters discovery topics before they could become background
  subscriptions with backfill eligibility.
- New test: `authorize_denies_discovery_topic` (`src/backfill.rs`) asserts denial
  for all three networks for both the local key and an arbitrary peer, plus a
  positive control that a real direct-chat topic still authorizes.
- Verdict: **blocked (explicit)**.

### 3.11 Directory-topic special case (`RoomAdvertisement` → `directory_store`)
- `process_net_event_sync` (`app.rs:15635-15659`) upserts verified
  `RoomAdvertisement`s. A discovery-topic payload can only reach it through a
  `NetEvent` that survived guards 1 + 2 — impossible by construction. The
  discovery topic is also not the directory mesh (`src/directory.rs` domain
  separator differs; pinned by `discovery_topic_differs_from_directory_topic`).
- Verdict: **blocked**.

### 3.12 MCP / GUI test actions
- `validate_gui_test_command::OpenRoom` (`app.rs:10787-10806`) requires the topic
  to exist in `room_history`, and the discovery topic is never upserted there;
  guard 3 additionally refuses it in the handler itself.
- The stale MCP lobby literal (`mcp_server.rs:2451`,
  `b"iroh-gossip-chat/default-lobby/v1"`) is a **Conversation-kind** diagnostic
  hash, not the discovery topic; it is out of scope here (handled by the
  terminology / migration tasks, BORU-DISC-15+).
- Verdict: **blocked**.

## 4. Guards added in this task (diff summary)

| File | Guard | Kind |
|---|---|---|
| `src/backfill.rs` `BackfillAuthorizer::authorize` | `is_discovery_topic(topic)` → `false` (first check) | policy |
| `src/backfill.rs` tests | `authorize_denies_discovery_topic` (+ direct-chat positive control) | test |
| `examples/iced_chat/app.rs` `AppMessage::OpenRoom` | refuse Discovery topics before any conversation side effect | UI catch-all |
| `examples/iced_chat/app.rs` `AppMessage::RoomOpened` | drop Discovery topics before any state mutation | UI defense-in-depth |
| `examples/iced_chat/app.rs` `persist_remote_message` | `is_discovery_topic` → skip `insert_chat_message` | persistence |
| `examples/iced_chat/app.rs` `persist_remote_file_share` | `is_discovery_topic` → skip `insert_chat_message` | persistence |
| `examples/iced_chat/app/discover.rs` `SubscribeStoredConversations` | filter Discovery topics from store auto-subscribe | UI |
| `examples/iced_chat/app/discover.rs` `BackgroundSubscribe` | refuse Discovery topics | UI |

Guardrails honoured: deterministic direct-topic derivation untouched; discovery
state never merged into conversation state (no hidden chat object — the
discovery topic cannot even become a `ConversationLive`); public chat
creation/joining stays explicit; private DMs / chat payloads never route through
the discovery topic (the classifier keeps every non-discovery topic a
Conversation; `topic_kind` is pinned by unit tests in `src/discovery_topic.rs`).

## 5. Test evidence

- `authorize_denies_discovery_topic` (new, lib): discovery topic denied on
  Mainnet/Development/Test for the local key and an arbitrary peer; a direct-chat
  topic between the two participants is still authorized (positive control).
- `discovery_topic_forwarder_never_reaches_conversation_handling` (BORU-DISC-10,
  `src/conversations.rs:1182-1253`): a valid chat payload + NeighborUp fed on the
  discovery topic produce **no** `ConversationNetEvent`.
- `conversation_topic_forwarder_still_forwards_chat` (BORU-DISC-10 positive
  control): normal conversation traffic still flows unchanged.
- `topic_kind_*` classifier tests (`src/discovery_topic.rs:320-382`): discovery
  topics classify Discovery; lobby / direct / arbitrary topics classify
  Conversation.

The full UI-isolation proof (discovery traffic produces no rendered entry or
badge in a live UI) is BORU-DISC-25; the assertions above cover the
persistence-layer invariants that are feasible at the lib/unit level.

## 6. Remaining known limitations / out of scope

- GUI-level end-to-end isolation test → BORU-DISC-25.
- Public-chat retention of the legacy lobby as an explicit user room →
  BORU-DISC-14.
- Terminology (lobby → discovery) → BORU-DISC-15.
- Migration / stale-row cleanup → BORU-DISC-16+.
- The stale MCP lobby literal (`mcp_server.rs:2451`) stays untouched; it is a
  Conversation-kind diagnostic id, not the discovery topic.
