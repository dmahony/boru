# BORU-DISC-02: Startup flow and lobby UI insertion points

Audit scope: committed worktree after `git fetch origin && git merge origin/main`. This is a read-only trace; no runtime code was changed. The current implementation uses the canonical Mainnet public-lobby topic (`IcedChat::default_lobby_topic()`), documented in `docs/discovery-refactor/01-lobby-constants-map.md`.

## Executive finding

Startup joins the canonical lobby twice:

1. `main.rs` subscribes once before the GUI starts (`gossip.subscribe` at `examples/iced_chat/main.rs:1110-1113`) and keeps that subscription's receiver in a background drain task for mesh/DHT/mDNS discovery (`:1168-1218`).
2. The GUI then schedules `AppMessage::OpenRoom` for the same topic (`examples/iced_chat/main.rs:1671-1689`). The `OpenRoom` slow path performs another `gossip.subscribe` (`examples/iced_chat/app.rs:11848-11903`) and the resulting `RoomOpened` path treats the lobby as a normal room.

The second path is the user-facing coupling to remove or relocate. It selects `Screen::Chat`, creates room-history state, emits chat system messages, replays chat history, and stores the lobby sender in `self.conversations` so it survives room switches. The first subscription is discovery/mesh infrastructure, but its sender is not transferred into a discovery-owned application service; only the second subscription is stored in the generic conversation map.

## Ordered startup trace

### 1. Parse arguments and choose the initial room

* `examples/iced_chat/main.rs:465-488` enters `main`, parses `Args`, migrates the data directory, selects the data directory, and initializes logging.
* `examples/iced_chat/main.rs:536-540` creates the Tokio runtime.
* `examples/iced_chat/main.rs:542-590` computes `initial_room`:
  * explicit `open`/`join` commands select the requested topic and bootstrap peers;
  * with no command, `:582-588` selects `IcedChat::default_lobby_topic()` and no peers. The comments explicitly say the public lobby is opened so the user can type immediately.
* `examples/iced_chat/app.rs:7886-7895` defines `default_lobby_topic()` as the canonical Mainnet `public_lobby_topic` derivation. This is an identity helper, not itself a subscription.

**Future refactor insertion point A — default selection:** remove the no-command fallback to the lobby as `initial_room` (or replace it with the intended user-facing home/ChatList state). Do not alter direct-topic derivation or explicit public-room open/join behavior.

### 2. Initialize endpoint, address lookup, gossip, and protocol router

* `examples/iced_chat/main.rs:760-810` creates the mDNS lookup/subscriber, binds the Iroh endpoint, registers address lookup providers, and waits up to 15 seconds for relay readiness.
* `examples/iced_chat/main.rs:814-871` registers the DHT address lookup (unless disabled). This is endpoint address resolution and is separate from the public-room DHT tracker.
* `examples/iced_chat/main.rs:873-876` creates the gossip actor with `Gossip::builder().spawn(endpoint.clone())`.
* `examples/iced_chat/main.rs:1062-1076` builds/spawns the protocol router, including the gossip ALPN.

At this point networking is ready, but no lobby topic has yet been subscribed in this startup block.

### 3. Subscribe the raw lobby gossip topic for mesh bootstrap

* `examples/iced_chat/main.rs:1078-1086` creates the discovered-peer and directory-room UI channels.
* `examples/iced_chat/main.rs:1088-1103` creates the shared member-discovery DHT client (unless `--no-dht` is supplied). `--no-dht` does not disable the lobby gossip subscription.
* `examples/iced_chat/main.rs:1105-1108` allocates `continuous_tracker: Option<ContinuousTracker>` so the tracker handle remains alive through the GUI lifetime.
* **Exact first join:** `examples/iced_chat/main.rs:1110-1113` computes the canonical lobby topic, displays `Joining lobby...`, and calls `gossip.subscribe(lobby_topic, Vec::new()).await`. On success it splits the returned `GossipTopic` into `sender` and `receiver`.
* `examples/iced_chat/main.rs:1126-1166` starts `PublicRoomTracker` and then `ContinuousTracker::start_with_joiner` at `:1140-1145`, passing `sender.clone()`. The tracker identity is asserted equal to `lobby_topic` at `:1138`.
* `examples/iced_chat/main.rs:1168-1218` spawns the receiver drain. `NeighborUp`/`NeighborDown` are forwarded to the dynamic joiner and `discovered_peers_tx`; all other lobby events are discarded by the match at `:1188-1215`. This receiver is intentionally kept alive to avoid backpressure.
* `examples/iced_chat/main.rs:1220-1277` handles mDNS discoveries, joins discovered peers to the same lobby sender, and forwards peer IDs to the Discover UI channel.
* `examples/iced_chat/main.rs:1279-1282` reports subscription success/failure to splash/logging.

**Future refactor insertion point B — discovery ownership:** retain one internal discovery subscription and its receiver/neighbor handling, but move its sender/receiver lifetime out of the generic room model. The raw startup subscription currently performs discovery work correctly; the problem is the later duplicate user-room subscription, not the canonical topic derivation.

### 4. Construct `IcedChat` state

* `examples/iced_chat/main.rs:1578-1627` derives `initial_topic`, calls `IcedChat::new(...)`, and passes the full `initial_room` plus `return_to_chat_list_after_open = initial_topic.is_some() && args.command.is_none()` at `:1612`. The initial topic is not opened inside the constructor.
* `examples/iced_chat/app.rs:7132-7176` defines the constructor. At `:7177-7178`, `initial_room` is unpacked into `initial_topic` and initial bootstrap addresses.
* `examples/iced_chat/app.rs:7334-7353` loads the persisted `ConversationStore`; it does not seed a lobby conversation merely because `initial_topic` is the lobby.
* `examples/iced_chat/app.rs:7382-7403` initializes the UI on `Screen::ChatList`, creates an empty `conversations: HashMap`, and initializes room-history state. Thus the lobby is not inserted during `IcedChat::new` itself.

**Future refactor insertion point C — constructor state:** keep discovery state independent from `IcedChat::conversations`. The constructor currently receives the continuous tracker and discovered-peer channel, but no discovery-owned lobby sender; future code must add that networking state without creating a hidden `ConversationLive`.

### 5. Start Iced and schedule the second lobby join

* `examples/iced_chat/main.rs:1671-1690` starts the Iced application. The boot closure takes the constructed state and, when `initial_topic` exists, chains `iced::Task::done(AppMessage::OpenRoom(topic))` at `:1678-1681`. It also independently schedules `SubscribeDirectoryTopic` at `:1686-1689`.
* With no command, `initial_topic` is the lobby from `main.rs:586-588`; therefore the boot task is `OpenRoom(canonical_lobby_topic)`.
* The initial `IcedChat` screen was `ChatList`, so this task is the transition that selects the lobby in the chat UI. It is separate from the raw `main.rs` subscription.

**Future refactor insertion point D — automatic `OpenRoom`:** remove the automatic `OpenRoom(lobby_topic)` task for the internal discovery topic. Explicit user room creation/open/join tasks must remain user-facing and continue to use their own topics.

### 6. `OpenRoom` performs the duplicate subscription

* `examples/iced_chat/app.rs:11732-11745` enters the first-time/slow `OpenRoom` path, sets `pending_topic`, and increments the room generation.
* `examples/iced_chat/app.rs:11751-11776` saves/leaves the previous room before opening the requested topic. For the startup lobby this is the initial empty ChatList state.
* `examples/iced_chat/app.rs:11844-11846` marks room loading and pushes the user-visible `Connecting to lobby...` mesh event.
* **Exact second join:** `examples/iced_chat/app.rs:11848-11903` starts an async room-open task and calls `gossip.subscribe(topic, bootstrap_peers)` at `:11867-11873` when there are no bootstrap peers (the startup lobby case). It splits the subscription into `sender` and `receiver` at `:11902-11903`, then creates the generic room forwarder at `:11983-11989`.
* `examples/iced_chat/app.rs:12163-12186` has a lobby-specific branch after the subscription: it finds pending discovered peers and calls `sender.join_peers` for each. This is discovery behavior embedded inside the normal room-open path.

**Future refactor insertion point E — duplicate join and discovery join fanout:** relocate the pending-peer `join_peers` behavior to the discovery service and prevent the internal discovery topic from entering this generic room-open path. Do not route direct/group/public chat payloads through the discovery topic.

### 7. `RoomOpened` inserts lobby state into UI/chat state

The continuation after the subscription completion is the decisive UI insertion path:

* `examples/iced_chat/app.rs:12208-12217` unarchives an existing `ConversationStore` entry, if one exists, and bumps `chats_sidebar_revision`. This is not normally the initial lobby insertion (the constructor loaded the store without adding the lobby), but it would surface a persisted lobby entry in the CHATS sidebar.
* `examples/iced_chat/app.rs:12219-12232` records `RoomJoined`, sets `self.screen = Screen::Chat { topic }`, sets `self.topic = topic`, stores the ticket, clears the active timeline/indexes, and marks onboarding complete. This is the **selected-chat insertion point** for the startup lobby.
* `examples/iced_chat/app.rs:12234-12291` computes background subscriptions from active conversation-store entries and known friends. The lobby is not intended to be a direct conversation, but after it is treated as a normal open room it participates in this surrounding generic lifecycle.
* `examples/iced_chat/app.rs:12292-12305` pushes `Chat joined.` and the `/help` hint into the active entries. These are **chat-rendering/system-message insertion points** reached by the lobby.
* `examples/iced_chat/app.rs:12308-12465` replays persisted history for the opened topic and overlays delivery/outbox state. This is the **chat-history persistence/replay insertion point** for the lobby.
* `examples/iced_chat/app.rs:12467-12483` handles backfill behavior for the opened topic.
* **Exact room/recent-history insertion:** `examples/iced_chat/app.rs:12485-12488` executes `self.room_history.upsert(topic, &self.local_label, true)` and persists it. For the startup lobby this creates/updates the room-history record used by recent-room/history UI.
* **Exact sender lifetime/conversation-map insertion:** `examples/iced_chat/app.rs:12524-12541` checks `topic == default_lobby_topic()`, removes or creates a `ConversationLive`, assigns `sender`, `forward_handle_slot`, and `ticket_str`, then calls `self.conversations.insert(topic, lobby_conv)` at `:12536`. The comment at `:12524-12526` states the purpose: keep the lobby sender alive across room switches so mDNS-discovered peers can still be joined to the lobby mesh.

The sidebar is derived from the durable conversation store, not directly from `self.conversations`: `examples/iced_chat/app/sidebar.rs:491-559` iterates `conversation_store.active_iter()`, looks up room-history preview/unread data, and reads unread values from `self.conversations`. Therefore the startup `OpenRoom` path directly selects the lobby and creates room-history data, while a CHATS row appears only if a corresponding conversation-store entry exists/unarchives. The live `ConversationLive` insertion at `app.rs:12524-12541` is nevertheless the runtime state that makes lobby unread/sidebar-related behavior reachable.

**Future refactor insertion point F — remove all UI/chat insertion:** for an internal discovery topic, remove or relocate the `RoomOpened` behaviors above: `Screen::Chat`/`self.topic`, system entries, history/outbox/backfill replay, room-history upsert, and `ConversationLive` insertion. Discovery must not create a conversation-store record, recent-chat row, unread state, persisted history, or rendered message. Replace the sender-lifetime workaround with discovery-owned state.

### 8. Steady-state Iced loop

* `examples/iced_chat/main.rs:1692-1694` supplies `IcedChat::update` and `IcedChat::view` to the Iced application.
* `examples/iced_chat/main.rs:1702-1712` builds recurring subscriptions, including connecting/reconnect ticks based on `state.screen` and room sender state.
* After the startup `OpenRoom` task completes, the lobby is therefore both the selected `Screen::Chat` and a live entry in `self.conversations`; switching rooms changes the selected state but does not remove the lobby sender.

## Migration checklist for the next implementation tasks

1. Preserve `default_lobby_topic()` and all existing direct/group/public topic derivations unless a separate compatibility decision proves one wrong.
2. Keep the raw startup gossip subscription, neighbor lifecycle handling, DHT tracker, and mDNS join fanout as networking/discovery infrastructure.
3. Eliminate the automatic `OpenRoom(lobby_topic)` convergence for the internal discovery topic.
4. Do not use `ConversationLive`, `ConversationStore`, `RoomHistoryStore`, chat-history/outbox replay, system entries, unread counters, or `Screen::Chat` as a hidden discovery container.
5. Keep explicit public-room creation/joining on ordinary user-selected conversation topics.
6. Add the early topic-kind routing guard before discovery payloads can reach generic chat deserialization/event handling, as required by the Phase 1 PDF constraints.
