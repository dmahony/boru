# BORU-DISC-01: Current Lobby Constants and Read/Write Path Map

Audit performed against the committed worktree after `git fetch origin && git merge origin/main`.
This is an inventory only; no runtime code was changed by this task.

## Scope and terminology

The current default lobby is one gossip topic that is used for two different purposes:

1. discovery infrastructure (mDNS/DHT peer bootstrap and gossip neighbor lifecycle), and
2. a normal user-facing room (initial selection, `ConversationLive`, chat rendering, history,
   sidebar ordering/unread state, and message send paths).

That dual use is the central coupling to remove in the later discovery refactor. A path marked
`DISCOVERY` below is networking/DHT/mesh behavior. A path marked `UI/CHAT` enters the application
conversation model or a user-facing diagnostic/UI path. Some paths intentionally do both and are
marked `MIXED`.

## Canonical identity and derivation

### Identity inputs

| Item | Location | Value / behavior | Classification |
|---|---|---|---|
| Discovery-key domain separator | `src/public_room.rs:34-39` | `b"boru-chat discovery-key v1"` | DISCOVERY identity input |
| Application namespace | `src/public_room.rs:41-44` | `"boru-chat"` | DISCOVERY/public-room identity label |
| Canonical room name | `src/public_room.rs:46-47` | `PUBLIC_ROOM_NAME = "public-lobby"` | DISCOVERY + legacy UI label |
| Public-room protocol version | `src/public_room.rs:49-50` | `PROTOCOL_VERSION = 1` | DISCOVERY wire/identity input |
| Network discriminators | `src/public_room.rs:71-84` | Mainnet `0x00`, Development `0x01`, Test `0x02` | DISCOVERY identity input |
| Gossip-topic domain separator | `src/topic_derivation.rs:10-16` | `b"boru-chat public-room v1"` | DISCOVERY identity input |
| Tracker namespace separator | `src/topic_derivation.rs:61-72` | `b"boru-chat room discovery v1"` | DISCOVERY identity input |
| DHT lobby-key domain | `src/discovery_backend.rs:15-24` | `b"boru-chat/public-lobby/v1"` then BLAKE3 with the discovery key | DISCOVERY identity input |

### Derivation chain

* `src/topic_derivation.rs:42-55`, `public_room_topic(network_byte, room_name, version)` computes:
  `BLAKE3(PUBLIC_ROOM_DOMAIN_SEPARATOR || network_byte || LE_u16(name length) || name bytes || version)`.
* `src/public_room.rs:141-154`, `public_discovery_key(...)` computes the same structured input
  with the separate discovery-key domain separator.
* `src/public_room.rs:156-167`, `public_room_identity(network)` combines the canonical room name,
  protocol version, topic derivation, and discovery-key derivation.
* `src/public_room.rs:170-176`, `public_lobby_topic(network)` returns the canonical identity's
  gossip topic.
* `src/topic_derivation.rs:110-117`, `tracker_namespace_from_topic(...)` derives a separate
  SHA-256 distributed-topic-tracker namespace from raw topic bytes. This is not the gossip topic.
* `src/discovery_backend.rs:20-24`, `canonical_lobby_key(...)` derives the public-room DHT key
  namespace from the public-room discovery key. `src/public_room_tracker.rs:191-195` and
  `:241-243` use it for publication and lookup.

Known-answer vectors in `src/public_room.rs:262-304` and `src/topic_derivation.rs:271-303` establish
these current values. The canonical Mainnet lobby topic is:

`ebab66f60ff734452d4fd83283b4d5ee221dfa73a81cc2ef520b919378fe4016`

The canonical Mainnet discovery key is tested at `src/public_room.rs:269-275`, and the tracker
namespace is tested at `src/topic_derivation.rs:201-208`.

### GUI helper and a stale alternate derivation

* `examples/iced_chat/app.rs:7885-7895`, `IcedChat::default_lobby_topic()`, delegates to
  `public_lobby_topic(PublicNetwork::Mainnet)`. This is the authoritative GUI helper.
* `examples/iced_chat/main.rs:2593-2598` asserts the GUI helper equals the canonical Mainnet
  public-room helper; `:2605-2623` asserts the tracker identity topic equals the same helper.
* `examples/iced_chat/mcp_server.rs:2451-2452` independently hashes the legacy literal
  `b"iroh-gossip-chat/default-lobby/v1"` and uses that result as the diagnostic lobby room ID.
  This is not the current `public_lobby_topic` derivation (it omits network byte, name-length,
  and protocol-version framing), so it is a compatibility/diagnostic hazard and must be resolved
  explicitly in a later task. The branding audit also records this old literal at
  `docs/branding-rename-deliverables.md:128,387-389` and `docs/BORU_BRANDING_AUDIT.md:288-290`.

## Discovery-only paths

### Public-room DHT tracker

* `src/public_room_tracker.rs:54-87` defines `PublicRoomTracker` as a DHT/publication wrapper;
  its documentation explicitly says it does not integrate with the gossip actor.
* `src/public_room_tracker.rs:135-152`, `start(...)`, derives the canonical identity and creates
  the tracker for a selected `PublicNetwork`.
* `src/public_room_tracker.rs:165-220`, `publish_once(...)`, signs the local EndpointId
  advertisement and writes an encrypted record to the DHT namespace derived from the canonical
  discovery key. This is DISCOVERY-ONLY; it does not write chat history.
* `src/public_room_tracker.rs:222-330` (and the continuation through `:360+`), `discover_once(...)`,
  reads DHT records, validates size/timestamp/identity/signature, filters self and duplicates,
  and returns endpoint IDs. This is DISCOVERY-ONLY.
* `src/public_room_continuous.rs` supplies the continuous publish/discover policy and join fanout;
  the GUI starts it from `examples/iced_chat/main.rs:1126-1156` and keeps its handle alive in
  `continuous_tracker` (`:1105-1108`).

### Startup gossip subscription and mesh bootstrap

* `examples/iced_chat/main.rs:1078-1086` creates the discovered-peer UI channel, then
  `:1088-1103` creates/configures the shared member-discovery DHT client. `--no-dht` only disables
  DHT discovery; the lobby gossip subscription remains enabled.
* `examples/iced_chat/main.rs:1110-1113` computes the canonical lobby topic and calls
  `gossip.subscribe(lobby_topic, Vec::new())` at startup.
* `examples/iced_chat/main.rs:1126-1166` starts `PublicRoomTracker` and
  `ContinuousTracker::start_with_joiner` using the lobby sender. This is DISCOVERY-ONLY until
  the sender/receiver is handed into the application state.
* `examples/iced_chat/main.rs:1168-1218` drains the lobby receiver. `NeighborUp`/`NeighborDown`
  are forwarded to the dynamic joiner and to `discovered_peers_tx`; other gossip events are
  discarded. The joiner/UI channel forwarding is DISCOVERY/diagnostics behavior.
* `examples/iced_chat/main.rs:1220-1277` handles mDNS discoveries: caches endpoint addresses,
  calls `sender.join_peers`, and forwards peer IDs to the Discover sidebar channel. This is
  discovery networking plus a UI Discover-section update, not a chat message path.
* `examples/iced_chat/main.rs:1279-1282` logs successful/failed lobby subscription and updates
  the splash text. This is diagnostic UI behavior, not chat history.

### Discovery identity and safety tests

* `src/public_room.rs:189-357` tests deterministic keys, network/version separation, domain
  separation, and known-answer identities.
* `src/topic_derivation.rs:126-231` tests tracker namespace separation and known answers; the
  original public-room topic tests continue at `:236-310`.
* `src/discovery_backend.rs:483-488` tests canonical lobby-key domain separation.
* `src/public_room_tracker.rs` tests (search hits around its test module) exercise tracker
  publication/lookup and validation without making a chat conversation.
* `tests/test_public_lobby_integration.rs:1-454` is an in-memory DHT/public-lobby integration
  suite. It exercises public-room advertisements, stale records, multiple peers, and validation;
  it is discovery/public-room behavior, not GUI chat persistence.
* `src/backfill.rs:208-258` classifies all canonical Mainnet/Development/Test lobby topics as
  open public-room topics. `src/backfill.rs:1573-1589` verifies that any authenticated peer can
  request lobby backfill. This is a DISCOVERY/transport-policy read of the lobby identity, but
  it currently permits a discovery topic to be treated as a normal readable room.

## UI/chat paths coupled to the lobby

### Startup selection and visible conversation creation

* `examples/iced_chat/main.rs:542-590` computes `initial_room`. With no subcommand, it returns
  `(IcedChat::default_lobby_topic(), Vec::new())`, explicitly selecting the lobby as the initial
  room. `Some(Command::Open/Join)` instead selects an explicit user room.
* `examples/iced_chat/main.rs:1578-1612` passes that initial room/topic into `IcedChat::new`.
* `examples/iced_chat/main.rs:1671-1689` schedules `AppMessage::OpenRoom(initial_topic)` at Iced startup,
  then also starts the directory-topic subscription. Thus the startup lobby subscription and the
  UI `OpenRoom` flow are separate operations that converge on the same topic.
* `examples/iced_chat/app.rs:11732+` is the slow `OpenRoom` subscription path. It creates the
  normal `RoomOpened` event for the lobby just as it does for a user room.
* `examples/iced_chat/app.rs:12208-12217` unarchives an existing conversation-store entry after
  joining; `:12219-12232` records `RoomJoined`, selects `Screen::Chat { topic }`, sets active
  topic/ticket state, clears the active timeline, and marks onboarding complete.
* `examples/iced_chat/app.rs:12292-12305` emits user-visible system entries (`Chat joined.` and
  the `/help` hint) after every room join, including the lobby.

### Runtime conversation map and lobby special cases

* `examples/iced_chat/app.rs:12163-12186` checks `topic == default_lobby_topic()` and
  retroactively joins pending discovered peers on the lobby sender. This is discovery work still
  embedded inside the generic room-open path.
* `examples/iced_chat/app.rs:12524-12541` removes/creates a `ConversationLive` for the lobby,
  installs its gossip sender/forwarder/ticket, and inserts it into `self.conversations`. This is
  the key UI/chat coupling: the discovery mesh is deliberately retained as a live conversation
  so its sender survives room switches.
* `examples/iced_chat/app.rs:14061-14071` treats the canonical lobby as a known room in the
  diagnostic GUI action path, allowing `boru_gui_open_room`-style actions without room history.
* `examples/iced_chat/app.rs:16096-16121`, `should_announce_new_peer`, suppresses system
  “new peer” announcements for non-friend lobby participants, but only after the peer has already
  entered the generic conversation event path.

### Generic inbound gossip handling (lobby messages are not isolated)

* `examples/iced_chat/app/chat.rs:4788-4810` receives `AppMessage::NetEvent`, bumps the durable
  conversation ordering for user-visible events, updates room previews, and creates a
  `ConversationLive` for any topic. There is no early lobby/discovery routing guard here.
* `examples/iced_chat/app/chat.rs:4811-4874` updates per-topic neighbor/sender state for all
  topics, including the lobby, and schedules neighbor retries.
* `examples/iced_chat/app/chat.rs:4876-4913` queues non-visible protocol events for inactive
  conversations only after filtering. This explicitly mentions dense public lobby traffic, but the
  lobby still owns a bounded pending-events queue and an unread counter for visible events.
* `examples/iced_chat/app/chat.rs:4915-4949` routes active-topic events into
  `process_net_event_sync`, transfer handling, profile-image work, and link-preview work.
* `examples/iced_chat/app.rs:15647-15717` handles decoded messages, direct-conversation auto-create
  checks, room-advertisement directory updates, conversation ordering/preview updates, and then
  calls `handle_net_event_with_safety_for_topic(..., Some(topic))`. These are generic topic paths;
  a lobby `Message` can reach them unless a later guard prevents it.
* `examples/iced_chat/app.rs:15721-15743` treats a local lobby echo as a delivery-state transition;
  `:15746-15783` may generate a read-receipt broadcast while the lobby is active. Both are
  user-chat behaviors that must not remain reachable from an internal discovery topic.

### Outbound, persistence, unread, and rendering paths

* `examples/iced_chat/app/chat.rs:7226-7293` allows `SendMessage` for any topic present in
  `self.conversations`; because the lobby is inserted there, it can be sent to as a normal chat.
  Both active and background branches call `persist_outgoing_message` and broadcast through the
  topic's sender.
* `examples/iced_chat/app.rs:9326-9370` signs/persists outgoing text into the chat-history and
  SQLite/outgoing-message stores keyed by topic. This is reachable for the lobby through the
  normal send path.
* `examples/iced_chat/app.rs:8049-8054` documents that SQLite is now the live message-history
  write target; the legacy `chat_history.json` save hook is a no-op.
* `examples/iced_chat/app.rs:12296-12389` replays persisted history for any `RoomOpened` topic,
  reading `ChatHistoryStore`/legacy rows and `message_store.db`, then renders entries and overlays
  delivery state. The lobby receives this generic replay path when opened.
* `examples/iced_chat/app/discover.rs:1877-1977` replays persisted history for any
  `BackgroundSubscribed` topic and creates/updates a `ConversationLive`; this is the other history
  entry point, although the startup lobby is not itself selected by the active conversation-store
  auto-subscription loop unless persisted as a conversation.
* `examples/iced_chat/app/chat.rs:4792-4805` calls `ConversationStore::touch_and_bump` and
  `update_room_preview` for user-visible lobby events, affecting recent-chat/sidebar ordering and
  previews. `:4894-4913` increments pending/unread state for inactive lobby traffic.
* `examples/iced_chat/app.rs:15668-15674` can upsert direct conversation metadata for direct-topic
  messages; the lobby itself is not a direct topic, but this is part of the same unsegmented
  post-decode handler and must be kept separate from discovery routing.
* `examples/iced_chat/app.rs:12208-12217` and `:15668-15674` are the explicit conversation-store
  write sites closest to the lobby flow; the generic store implementation is in
  `src/conversations.rs:96-203,277+` and persists metadata such as preview/unread/archive state.

## Adjacent paths that must remain separate

* `src/directory.rs:1-49` derives a distinct relay-scoped public-room directory topic. Its test at
  `:261-269` asserts the directory topic differs from the canonical lobby topic. Directory ads are
  not lobby messages.
* `examples/iced_chat/main.rs:1285-1309` subscribes to the directory topic independently and shares
  its sender with MCP. This must not be merged into the future internal discovery topic.
* `src/private_room_tracker.rs` and the `OpenRoom` private-room tracker path at
  `examples/iced_chat/app.rs:11910-11929` are per-conversation DHT discovery and must remain
  independent of the internal discovery topic.
* Direct chats use `direct_topic(...)` and their own subscriptions; the generic auto-subscribe loop
  is constructed from persisted conversations/friends (`examples/iced_chat/app.rs:12234-12289`).
  The future discovery topic must never carry these direct payloads.

## Tests, docs, scripts, and operational assumptions

* `tests/test_branding_rename.rs:176-216,270-296` protects the public-room domain separator,
  canonical name, and deterministic topic behavior for compatibility.
* `docs/discovery-architecture.md:167-197` documents the current GUI flow as
  canonical identity -> lobby gossip subscription -> DHT tracker -> dynamic joiner, and explicitly
  says the same lobby topic is used for gossip, tracker identity, and initial selected room.
* `docs/compatibility-identifiers.md:47-81` records the domain separators and warns that changing
  `PUBLIC_ROOM_NAME`, network bytes, version, or framing splits peers.
* `docs/mcp-agent-instructions.md:95-176,289-291` treats the lobby as a diagnostic room and
  recommends checking “subscribed to lobby” logs; this is operational/UI behavior, not protocol
  identity.
* `examples/iced_chat/mcp_server.rs:2438-2497` exposes `boru_join_lobby_room` as a GUI
  room-opening action. It uses the stale literal hash noted above, so its room ID must be audited
  before any lobby removal/migration.
* `scripts/ui08_home_hero_screenshots.sh:5-18,130+`, `scripts/ui18_responsive_evidence.sh:46-50,118-122`,
  and related UI evidence scripts assume startup opens/subscribes to the visible lobby and may
  navigate back to ChatList after the lobby race. These are test-harness assumptions, not runtime
  protocol requirements.
* `docs/cargo-migration/evidence/t01-baseline/startup-boru.log:24-26,54` records the observable
  startup messages (`public-lobby continuous DHT tracker started`, `subscribed to lobby topic`) and
  the canonical short room ID `ebab66f6`.
* `docs/configuration.md:16-18` documents `--no-dht` as disabling private-room DHT discovery while
  leaving the public lobby unaffected; this wording will need updating when discovery is split.

## Audit conclusions / migration guardrails

1. The canonical identity is stable and well-tested: `public_lobby_topic(Mainnet)` is the
   `ebab66f6...4016` topic, with separate DHT key and tracker namespace derivations. Preserve all
   direct-topic derivation and public-room compatibility vectors.
2. Startup currently joins the canonical gossip topic in `main.rs`, then separately opens that
   same topic through `IcedChat::OpenRoom`; the latter creates a visible `Screen::Chat` and
   `ConversationLive`.
3. The strongest coupling is `app.rs:12524-12541`: the lobby sender is kept in the generic
   conversation map specifically to survive room switches. Removing the visible lobby must replace
   this lifetime mechanism with a discovery-owned sender/receiver, not a hidden Conversation.
4. Inbound gossip is decoded and dispatched through generic `NetEvent` handling before any
   discovery/topic-kind distinction. That is where a future routing guard must distinguish
   discovery packets from conversation messages.
5. The lobby currently has reachable outbound chat, history persistence/replay, unread/sidebar
   ordering, system messages, delivery/read-receipt, attachment/link-preview, and diagnostics
   paths. All must be explicitly excluded for the internal discovery topic.
6. The MCP diagnostic hash (`mcp_server.rs:2451`) is a stale alternate derivation and does not
   identify the canonical current lobby. It must not be silently treated as the canonical identity
   during migration; add an explicit compatibility decision in the next audit task.
