# Architecture Boundaries (BORU-ARCH-002)

Intended domain ownership for the Boru frontend shell before Phase 1 extraction starts.
This document is guidance for extraction PRs — it does not move or change any code, and it
does not propose a rewrite or new user-visible behaviour.

- Created: 2026-08-18 (BORU-ARCH-002, task t_a70626d1)
- PDF source: `Boru_Code_Improvement_Action_Plan.pdf`, Phase 0 task BORU-ARCH-002
- Pairs with: `docs/architecture-refactor/baseline.md` (BORU-ARCH-001)
- Applies to: Phase 1 extraction PRs (BORU-APP-*), then Phase 2 (discovery)

## 1. Current state (what this document is reacting to)

`examples/iced_chat/app.rs` (41,831 lines / ~1.82 MB) is a single `IcedChat` struct holding
~200 fields and a single `AppMessage` enum with ~250 variants; `update()` (line 12786),
`view()` (21400) and `subscription()` (23393) are one monolithic match each.

The emerging module tree `examples/iced_chat/app/*.rs` (chat, files, calls, discover,
settings, sidebar, home, groups, contacts, tunnels, dialogs, screen_share_surface,
screen_share_ui) is already a **view-layer split**: each module is `mod X; pub(crate) use
X::*;` and contains `impl IcedChat` view/update-helper methods that read state via
`use super::*`. No domain owns its state yet — every field still lives on `IcedChat`, and
`app.rs` re-exports everything back with glob `use`.

That split risks becoming "a different kind of spaghetti" (the exact failure BORU-ARCH-002
exists to prevent): files are smaller, but all state is still global to every module. The
target end state is the reverse: a domain owns its state, its messages, and its update
logic together, and `IcedChat` becomes a coordinator that composes domains and routes
messages between them.

`boru-core` (`src/`) already has the right shape at the service layer: `call/`, `tunnel/`,
`screen_share/`, `chat_core/`, `conversations`, `friends`, `friend_request`, `outbox/`,
`mailbox/`, `whisper/`, `inbox/`, `backfill/`, `file_offer*`, `file_indexer`,
`file_access_*`, `download*`, `catalogue_*`, `discovery_*`, `control_plane/`, `directory/`,
`room_directory`, `public_room*`, `room*`, `storage`, `store`, `diagnostics`, `net`,
`control_plane`. Domain modules in the app should wrap these services; they should not
reimplement them.

## 2. Layering rule

Three layers, dependencies only downward:

```
App shell (coordinator: navigation, composition, lifecycle, routing)
        │  AppMessage routing / typed commands
        ▼
Feature domains (chat, files, rooms, friends, calls, screen sharing,
                tunnels, discovery, settings, notifications)
        │  calls into boru-core services / read-only context
        ▼
Shared infrastructure (boru-core services, stores, endpoint handles)
```

- The **App shell** owns application startup/shutdown (in `main.rs`), the `Screen` route
  table, generation tokens, and message routing. It is the only place allowed to compose
  domains or hand one domain's command to another.
- **Domains never mutate another domain's state directly.** A domain that needs something
  from another domain emits a typed command/event to the shell or reads a read-only
  context handle. This is the stop condition from PDF Section 14 ("a domain module starts
  directly mutating multiple unrelated domains") applied as a rule.
- Each extraction moves **cohesive state + messages + update logic together** (PDF Section
  1 rule), not just view helpers.

## 3. Domain ownership

For each domain: state it owns, messages/events it consumes, commands/effects it emits,
and which other domains it may call. "May call" lists the only allowed edges; anything
else routes through the shell. Module paths are the current homes of the code.

### 3.1 App shell (coordinator) — `app.rs` + `main.rs`
- **Owns:** `Screen` + all `*_return_to` pointers, `pending_topic`, `room_loading`,
  `room_generation`, `conversation_generation`, prewarm cache + idle timer, active
  theme/layout values and revisions, app-wide tick counters that are pure timing
  (`ConnMonitorTick`, `CallUiTick`, `MeshWatchdogTick`, `OutboxRetryTick`, `IdleTick`).
- **Consumes:** every `AppMessage` (routes to domains); net-level events that affect
  navigation (`RoomOpened`, `RoomJoinFailed`, `CreateNewRoom`, `GroupCreated`).
- **Emits:** navigation tasks, domain command messages, prewarm cache fills,
  startup/shutdown orchestration.
- **May call:** every domain (route-only). Owns the top-level `view()` composition
  (sidebar + screen-specific main panel) and the `subscription()` receiver wiring.

### 3.2 Chat — `app/chat.rs`; boru-core `chat_core/`, `chat_history`, `outbox/`,
`mailbox/`, `whisper/`, `inbox/`, `backfill/`
- **Owns:** the `conversations: HashMap<TopicId, ConversationLive>` map (subscription,
  forwarder, entries, composer, per-conversation downloads, neighbors, `pending_events`,
  `unread`), the flattened active-conversation display cache (`topic`, `ticket_str`,
  `entries`, `composer_text`, `scroll_*`, `follow_latest`, `pending_*` queues,
  `transfer_id_to_index`, inline-video/streaming session state), `names`, history stores
  (`chat_history`, `room_history`) for the active room.
- **Consumes:** `ConversationNetEvent`, `WhisperEvent`, `InboxEvent`, transfer updates,
  history load results, composer/search/context-menu/emoji/gif/help messages, video/thumbnail
  fetch results.
- **Emits:** send/broadcast commands through the conversation's `GossipSender`, whisper
  sends, outbox enqueue, history save, `FileShare`/`FileOffer`/image/GIF messages,
  download initiation, presence/status events to the shell.
- **May call:** Files (share/transfer lifecycle), Rooms (open a conversation from a
  ticket/entry), Friends (display names, DM topics), Discovery (read-only capability
  gate), Notifications (message-received events), Settings (read formatting prefs).

> Known duplication to converge during extraction: `IcedChat` keeps both the
> `conversations` map **and** flattened fields that mirror the active conversation
> (`entries`, `composer_text`, `pending_file`, `download_entry_index`, …). The map is the
> source of truth; the flattened fields are the active-view projection. Extraction must
> **move** (not copy) this state into the Chat domain and drop the flattened fields from
> `IcedChat` — otherwise the stop condition "same state exists in both old and new module"
> is violated.

### 3.3 Files — `app/files.rs`; boru-core `file_offer*`, `file_indexer`, `file_access_*`,
`download*`, `transfer_state_projection`, `catalogue_*`, `storage`, `image_store`
- **Owns:** file-sharing dashboard tabs (Shared by Me / Downloading / Downloaded / Shared
  with Me / Activity Log), projection rows + caches, paused-inbound-transfer set,
  downloaded-history cache, activity-log cache, dashboard search/filter/sort state,
  shared-by-me access-grant state.
- **Consumes:** `TransferProjectionUpdate`, `TransferSnapshotResync`, download progress,
  `FileShare`/`FileOffer` offers, catalogue queries, dashboard messages, thumbnail renders.
- **Emits:** download start/stop/pause/resume, share/revoke commands, reveal/open OS
  effects, `FileShare` sends via Chat's sender, completion events to Notifications.
- **May call:** Chat (active sender/entries for attach flows), Rooms (per-peer catalogue),
  Discovery (capability gate read), Notifications (transfer completion).

### 3.4 Rooms — `app/home.rs`, `app/groups.rs`, `app/discover.rs` (browse side);
boru-core `room*`, `room_docs`, `room_history`, `room_cleanup`, `public_room*`,
`conversations`, `group_*`, `room_directory`
- **Owns:** conversation store cache (chat list), room metadata/visibility, group creation
  state, public-room browse/filter/sort state (`discover_*` fields), join-ticket input,
  chat-list error, sidebar CHATS/GROUPS/PUBLIC ROOMS section caches.
- **Consumes:** `RoomOpened`/`RoomJoinFailed`, room advertisements, directory updates,
  group invites, `JoinFromTicket`, create-room/group messages, `OpenRoomSettings`.
- **Emits:** subscribe/join commands, metadata/doc writes, room delete/clear commands,
  ticket regeneration, directory visibility changes, navigation into a Chat.
- **May call:** Chat (join result lands in the active room), Friends (member roster),
  Discovery (directory state sync), Notifications (room events).

### 3.5 Friends — `app/contacts.rs` (+ sidebar friends/requests sections); boru-core
`friends`, `friend_request`, `contact`, `peer_names`, `pairing_service`, `spake2_pairing`,
`peer_invitation`
- **Owns:** friends store cache + dirty flag, friend-request rows, friend-profile state,
  friends/requests sidebar caches + revisions.
- **Consumes:** `FriendEvent`, friend-request whispers, presence/neighbor events,
  request accept/decline messages.
- **Emits:** accept/decline commands, direct-topic subscriptions, presence broadcasts (via
  shell), peer-profile open, pairing commands.
- **May call:** Rooms (open a direct chat), Chat (DM topic subscription), Discovery
  (capability gate read).

### 3.6 Calls — `app/calls.rs` (+ incoming-call overlay in `app/dialogs.rs`); boru-core `call/`
- **Owns:** call state (`active_call_id`, outgoing/incoming/ringing, audio/camera
  selection, latest frames, call timers, call kind/origin, history outcome).
- **Consumes:** `CallEvent`, call UI messages (`AcceptIncomingCall`, `RejectIncomingCall`,
  `HangUp`, `ToggleCallMute`, `SelectMicrophone/Speaker/Camera`, `CallUiTick`).
- **Emits:** call-actor commands (start/accept/reject/hangup/media), `CallHistory` writes.
- **May call:** Friends (authorization check), Settings (device prefs), Notifications
  (incoming-call event), App shell (screen transitions).

### 3.7 Screen sharing — `app/screen_share_surface.rs`, `app/screen_share_ui.rs`;
boru-core `screen_share/`
- **Owns:** host/viewer session state, invitation state, view mode/pan, cursor overlay,
  audio-state mirror, dev diagnostics overlay, viewer stats.
- **Consumes:** `SessionEvent`, frame/stats watch receivers, pointer/key/wheel messages,
  `ScreenShare*` control messages.
- **Emits:** host commands (start/stop/switch-source/quality/audio), control grants,
  clipboard sync.
- **May call:** Friends (capability gate read), Calls (media pipeline reuse, read-only),
  App shell (screen composition).

### 3.8 Tunnels — `app/tunnels.rs`; boru-core `tunnel/`, `local_service_scan`, `vnc_tunnel`
- **Owns:** tunnel list/dialog state, create-tunnel form fields, local-service scan
  results, incoming tunnel-request state.
- **Consumes:** `TunnelStatus`, tunnel request/control events, tunnel UI messages.
- **Emits:** `TunnelService` create/close/accept/decline commands, port-forward effects.
- **May call:** Friends (peer picker), Settings (tunnel prefs), App shell (navigation).

### 3.9 Discovery — boru-core `discovery_service`, `discovery_backend`,
`control_plane/`, `directory/`, `room_directory`, `dynamic_joiner`, `local_service_scan`
(the app holds only read handles)
- **Owns:** endpoint presence, peer discovery state, connectivity store, capability
  negotiation, room-directory cache, bootstrap/reconnect state.
- **Consumes:** net events (neighbor up/down, presence, advertisements), discovery
  records, reconnect events.
- **Emits:** discovered peers, connectivity status, capability answers, directory
  entries, room advertisements.
- **May call:** Rooms (directory state sync), Friends (presence), App shell (read-only
  handles: `connectivity_store`, `capability_gate`, `room_directory`).

### 3.10 Settings — `app/settings.rs`; boru-core `user_profile`, `klipy_config`,
`download_limits`, catalogue config
- **Owns:** `AppSettings`, `dark_mode`, theme/layout configs + merged active values,
  notification preferences, profile identity/display name, sound/address-sharing toggles.
- **Consumes:** `Settings*` messages, theme/layout reload events, profile save results.
- **Emits:** theme/layout revision bumps, profile persist commands, settings write-through.
- **May call:** every domain **read-only** (provides config context), App shell (screen
  return). No domain may write settings except through Settings.

### 3.11 Notifications — `examples/iced_chat/notification/` (backend/event/focus/render/service)
- **Owns:** notification service, preferences, focus tracker, dedupe/group state,
  rendered-notification queue.
- **Consumes:** notification events sourced from Chat/Files/Friends/Calls/Rooms,
  `WindowFocusChanged`.
- **Emits:** platform-backend notification effects, sound toggles.
- **May call:** Settings (read prefs), App shell (focus). Never mutates chat state.

## 4. Currently shared global data — disposition

Everything below currently lives on `IcedChat` (or is passed into `subscription()` from
`main.rs`). Marking: **remain shared** = stays app-wide; **read-only context** = shared
but domains may only read it; **move into domain** = the owning domain takes it.

| Data (current `IcedChat` fields) | Disposition | Notes |
|---|---|---|
| `screen`, `*_return_to`, `pending_topic`, `room_loading`, generations | remain shared | shell navigation/route state |
| `prewarm_cache`, `prewarming`, `idle_timer`, `prewarm_*` | remain shared | shell perf cache; keyed by `Screen` |
| `active_theme`, `active_layout`, `theme_revision`, `layout_revision`, `ui_theme_config`, `layout_overrides` | remain shared (read-only context) | produced by Settings, read by every view |
| `secret_key`, `gossip`, `_router`, `endpoint`, `blob_store`, `memory_lookup`, `net_rx/tx`, `runtime_handle`, `local_label`, `local_public`, `relay_mode` | remain shared (read-only context) | network substrate; no domain mutates it |
| `data_dir`, `settings` (`AppSettings`) | remain shared (read-only context) | config + path context |
| `file_offer_registry`, `tunnel_service`, `backfill_handle`, `whisper_handle`, `call_handle` | remain shared (read-only context) | service handles injected from main.rs |
| `connectivity_store`, `capability_gate`, `room_directory` | remain shared (read-only context) | discovery read handles |
| `chat_history`, `image_store`, `storage`, `download_manager`, `room_history` | remain shared (read-only context) | stores; opened by main.rs |
| `subscription()` receivers (net, friend, whisper, inbox, discovered, reconnect, transfer, theme, layout, call, screen-share) | remain shared | per-domain channels, owned by the shell's subscription |
| `conversations` map + active-conversation display fields (`entries`, `composer_*`, `pending_*`, `download_*`, inline-video, `names`, `self_sent_events`, indexes, `follow_latest`, `scroll_*`) | **move into Chat** | §3.2; flatten/remove the mirror on `IcedChat` |
| `peer_presence_map`, `presence_away_peers`, `seen_peers`, `peer_latencies`, `known_peers`, `neighbors`, `room_neighbor_counts`, `direct_peers`, `relayed_peers`, `mesh_health`, `mesh_event_log`, `mesh_connected_at`, presence/heartbeat/latency counters, `ticket_extra_peers` | **move into Discovery** (presence/mesh subdomain) | Chat/Rooms render a read-only projection |
| `friends`, `friends_dirty`, `friend_mgr`, `friend_events_rx`, `friends_sidebar_revision`, `requests_sidebar_revision` | **move into Friends** | §3.5 |
| `room_history`/chat-list fields (`join_ticket_input`, `chat_list_error`, `sidebar_selected_topic`, `sidebar_section_collapsed`, `sidebar_fade_frame`, `chats_sidebar_revision`, `public_rooms_sidebar_revision`) | **move into Rooms** (chat-list/sidebar caches) | §3.4 |
| `discover_*` browse fields (`discover_search_query`, filters, tags, sort, `discover_sidebar_revision`) | **move into Rooms** (browse surface) | §3.4 |
| call fields (`active_call_id`, `outgoing_call_*`, `incoming_call`, `call_*`) | **move into Calls** | §3.6 |
| `screen_share_*` fields | **move into Screen sharing** | §3.7 |
| `paused_inbound_transfer_ids`, dashboard/activity-log caches | **move into Files** | §3.3 |
| `sound_enabled`, `share_direct_addresses`, `show_presence_indicator`, `chat_text_size`, `history_confirm_*`, `room_delete_confirm_topic`, `history_*` flags | **move into Settings** (or Rooms for room-level confirm) | §3.10 |
| notification service/prefs/focus | **move into Notifications** | already a module; owns its state |

## 5. Allowed cross-domain edges (summary)

| From | To | Purpose |
|---|---|---|
| Chat | Files, Rooms, Friends, Discovery(read), Notifications, Settings(read) | attach/share, open conversation, names/DM, capability gate, message events, format prefs |
| Rooms | Chat, Friends, Discovery, Notifications | join result, member roster, directory sync, room events |
| Friends | Rooms, Chat, Discovery(read) | open direct chat, DM subscribe, capability gate |
| Calls | Friends, Settings(read), Notifications | auth, device prefs, incoming-call event |
| Screen sharing | Friends, Calls(read), App shell | capability gate, media reuse, composition |
| Tunnels | Friends, Settings(read), App shell | peer picker, prefs, navigation |
| Discovery | Rooms, Friends, App shell | directory sync, presence, read handles |
| Settings | all (read-only) | config context |
| Notifications | Settings(read), App shell | prefs, focus |

Every other interaction goes through the App shell. No domain writes another domain's
state; no domain owns a second domain's lifecycle.

## 6. How to use this document (Phase 1 extraction rules)

1. **One domain per PR** (PDF §13 slicing). Pick the first low-risk target from
   BORU-APP-001's map; move that domain's state + `AppMessage` variants + update arms +
   view methods together into its `app/<domain>.rs` module, per §3.
2. **Move, don't copy.** If a field must exist in both the old `IcedChat` and the new
   module, the extraction is not done — stop and converge (§3.2 duplication note).
3. **Keep the message surface.** `AppMessage` stays the single app-level message type;
   a domain's messages are routed by the shell's `update()` to the domain's `update()`
   (BORU-APP-002 pattern). No protocol, storage, or UI behaviour changes.
4. **Tests capture behaviour first** (PDF §1). Identify/keep the tests that cover the
   domain being moved; add targeted ones where the module has none.
5. **Gate each PR:** compile with default features and the relevant feature
   combinations; targeted tests pass; `git diff --check` clean.
6. **Stop conditions (PDF §14) apply to every PR:** stop if protocol bytes or persistent
   storage bytes change unexpectedly; stop if extraction requires broad public API
   changes across unrelated domains (revisit the boundary); stop if a test only passes
   after increasing arbitrary sleeps; stop if the same state exists in both old and new
   module; stop if a domain module starts directly mutating multiple unrelated domains;
   stop if a PR mixes a large behaviour change with structural extraction unless
   essential and independently tested.

Target end state (§15 of the plan): `IcedChat` is a coordinator that composes domains
and routes messages, each domain owns its state/messages/update logic, and the app no
longer lives physically under `examples/iced_chat` — but every step is a small,
reversible, test-backed PR.
