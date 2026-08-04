# Boru UI baseline map (UI-00)

## TASK
UI-00 Repository and UI architecture audit for the Boru Modern Home and Chat UI redesign.

## STATUS
Baseline captured on 2026-08-03. The GUI example builds and launches under Xvfb. The full library test command did not reach completion within the 600-second command limit and exposed pre-existing failures before timing out; see the attached logs.

## SUMMARY
Boru is a Rust 2021/Iced 0.14 desktop application. The GUI is an example binary (`boru`) split between a very large application module (`examples/iced_chat/app.rs`) and a bootstrap/network module (`examples/iced_chat/main.rs`). `IcedChat::view()` always composes a fixed-width left sidebar with a screen-dependent main panel, optional details panel, and modal overlays. Home is `Screen::ChatList`; an active room is `Screen::Chat { topic }`.

No production UI source was changed for this audit. The only new files are this note and baseline evidence files under `docs/ui-redesign/`.

## CHANGED FILES

- `docs/ui-redesign/current-ui-map.md` (this audit)
- `docs/ui-redesign/baseline-build.log`
- `docs/ui-redesign/baseline-tests.log`
- `docs/ui-redesign/baseline-test-list.log`
- `docs/ui-redesign/baseline-launch.log`
- `docs/ui-redesign/baseline-home-1280x800.png`

The pre-existing worktree modification `examples/iced_chat/app.rs` was present before this task and was not edited.

## DESIGN DECISIONS / ARCHITECTURE BOUNDARIES

- Preserve `Screen`, `AppMessage`, persistence keys, network event types, and protocol handlers. Visual work should remain in view/style helpers unless behavior is explicitly part of a later card.
- Treat `examples/iced_chat/app.rs` as the primary collision hotspot. It contains state, the complete message enum, update routing, all target views, and many non-target screens. Parallel workers should own disjoint functions or extract new modules rather than reformatting the whole file.
- Extend `examples/iced_chat/design_tokens.rs` and `fonts.rs` for shared visual primitives. Avoid screen-local palette literals.
- Keep `main.rs` owned by bootstrap/network work. UI workers should not modify endpoint/router startup unless required for an explicitly tested UI capability.
- The existing `iced::widget::lazy` sidebar dependencies and revision counters are performance-sensitive; changing row identity or dependency inputs can cause stale UI or unnecessary rebuilds. The home-rail cards use the same lazy pattern (see `docs/ui-redesign/home-cards-reactivity.md`) — keep each card's dependency restricted to exactly the state slice it renders.
- Keep Xvfb/screenshot evidence in `docs/ui-redesign/`; do not put test data or credentials in the default Boru data directory.

## BUILD, RUN, AND TOOLCHAIN BASELINE

Toolchain recorded in `baseline-build.log`:

- `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- `cargo 1.97.1 (c980f4866 2026-06-30)`
- `stable-x86_64-unknown-linux-gnu (default)`
- GUI dependency: `iced = 0.14`
- Package: `boru-core 0.108.0`

Build command:

```text
cargo check --features gui --example boru
```

Result: exit 0, finished successfully. It emitted 67 warnings for the GUI example and 3 warnings for the library (unused imports/variables, unfulfilled expectations, deprecated legacy history save, dead code, and private-interface warnings). These warnings are baseline noise, not introduced by this audit.

Binary build command:

```text
cargo build --features gui --example boru
```

Result: exit 0.

Headless launch smoke test:

```text
xvfb-run -a target/debug/examples/boru --data-dir <temporary-directory> --no-dht --no-relay
```

The process stayed alive for the 15-second timeout (exit 124 from `timeout`), which demonstrates that the application reached and maintained its GUI event loop. Xvfb/llvmpipe emitted a non-fatal `libEGL` DRI3 warning. A screenshot was captured at 1280x800 after 8 seconds; see `baseline-home-1280x800.png`.

Normal CLI usage documented by `examples/iced_chat/main.rs`:

```text
cargo run --features gui --example boru
cargo run --features gui --example boru open
cargo run --features gui --example boru join <ticket>
```

The package manifest names the actual example binary `boru` at `examples/iced_chat/main.rs` (`[[example]]`, required feature `gui`). The application supports `--data-dir`, `--no-dht`, `--no-relay`, `--mcp`, `--bind-port`, `--name`, `open`, `join`, and `logs`. `DISPLAY` or `WAYLAND_DISPLAY` is required; `main.rs` explicitly recommends `xvfb-run` for headless smoke tests.

## TEST BASELINE

`baseline-test-list.log` records the complete test inventory:

- `cargo test --lib -- --list`: 1,789 library tests listed.
- `cargo test --features gui --example boru -- --list`: 514 GUI/example tests listed.
- Combined inventory output contains 2,303 test entries.

Command attempted:

```text
cargo test --lib
```

Result: the command reached `running 1789 tests`, exposed 33 failures in the captured output, and was terminated by the 600-second command timeout while two `outbox_delivery` tests were still running. Therefore the baseline is **not healthy** and must not be reported as a passing full-suite baseline. Representative failures include existing `chat_core` image/name fallback tests, `download_initiation` tests, group encryption integration tests, `room_cleanup::delete_room_history_cascades_across_stores`, and `storage::test_partial_migration_resumes_on_reopen`. Full raw output is in `baseline-tests.log`; test inventory is in `baseline-test-list.log`.

## SOURCE MAP

### Application bootstrap and runtime

- `examples/iced_chat/main.rs:1-20`: declares GUI modules.
- `examples/iced_chat/main.rs:94-106`: rejects launch without `DISPLAY`/`WAYLAND_DISPLAY`.
- `examples/iced_chat/main.rs:108-165`: CLI `Args` and `Command` definitions.
- `examples/iced_chat/main.rs:178-210`: data directory and secret-key loading; generated key/data directory permissions are restrictive (`0700`/`0600`).
- `examples/iced_chat/main.rs:214-264`: rotating file/terminal logging.
- `examples/iced_chat/main.rs:454-515`: data migration, logging, panic handling, Tokio runtime.
- `examples/iced_chat/main.rs:518-566`: initial room selection; no subcommand opens the canonical lobby topic.
- `examples/iced_chat/main.rs:685-760`: endpoint, address lookup, relay mode, and gossip/router construction.
- `examples/iced_chat/main.rs:~1000-1400`: protocol handlers and event channel wiring.
- `examples/iced_chat/main.rs:~1350-1500`: `IcedChat` construction, snapshot throttling, and application launch.

### Application state and navigation

- `examples/iced_chat/app.rs:2373-2400`: `Screen` enum. Current screens are `ChatList`, `Chat`, `FriendRequests`, `Settings`, `PeerProfile`, `PeerCatalogue`, `FriendProfile`, `Discover`, `Groups`, and optional `Terminal`.
- `examples/iced_chat/app.rs:2419-2531`: `ConversationLive`; per-conversation sender, gossip forwarder, entries, composer, names, unread count, downloads, neighbors, pending events, and scroll state.
- `examples/iced_chat/app.rs:2561-~3190`: `IcedChat`; navigation, active display cache, network handles, friends, peer presence, mesh health, activity, tunnels, requests, settings, overlays, and caches.
- `examples/iced_chat/app.rs:3246-3290`: `RecentActivityEvent` and activity kinds.
- `examples/iced_chat/app.rs:3543-~4250`: `AppMessage`; the single UI command/event vocabulary. It includes navigation, room/group creation, tunnel actions, chat/composer actions, networking, friends/requests, downloads, profile/catalogue actions, settings, context menus, keyboard shortcuts, and GUI-test actions.
- `examples/iced_chat/app.rs:6850-~7810`: `IcedChat` methods and update preparation/helpers.
- `examples/iced_chat/app.rs:7812`: `IcedChat::update`, the central event reducer. Network and protocol events are handled in the ranges beginning at `11230` (`NetEvent`), `11376` (`FriendEvent`), and `11382` (`WhisperEvent`); inbox events follow in the same update match.
- `examples/iced_chat/app.rs:18638-18645`: theme selection from `dark_mode`.
- `examples/iced_chat/app.rs:18861-18952`: top-level `view()`, sidebar/main-panel composition, responsive details panel, and modal overlay order.

### Shell and sidebar

- `examples/iced_chat/app.rs:19540-19797`: `view_sidebar`; Boru header, add affordance, identity row, settings/optional terminal controls, and collapsible section wrapper.
- `examples/iced_chat/app.rs:19798-19918`: Chats section; conversation rows, selected topic, unread counts, room selection.
- `examples/iced_chat/app.rs:19920-20020`: Groups section; create-group action and group rows.
- `examples/iced_chat/app.rs:20022-20082`: Join-ticket input and submit path.
- `examples/iced_chat/app.rs:20479-20593`: Discover section; currently discovered peers, chat/open and catalogue/browse actions.
- `examples/iced_chat/app.rs:20595-20717`: Public Rooms section; directory entries and open/discover actions.
- `examples/iced_chat/app.rs:20718-20970`: Friends section; profile image/initials, online state, message/profile actions, and friend row caching.
- `examples/iced_chat/app.rs:20971-21205`: Requests section; pending incoming/outgoing requests, manage requests navigation, and request actions.

Visible sidebar data sources are `FriendsStore`, `ConversationStore`, `RoomHistoryStore`, discovered peer state, directory room state, `join_request_list`, `names`, and peer presence/online caches. Section collapse is stored in `sidebar_section_collapsed` and changed by `ToggleSidebarSectionCollapsed(usize)`.

### Home / landing screen (`Screen::ChatList`)

- `examples/iced_chat/app.rs:21209-21840`: `view_main_empty_state` (the screenshot's Home screen).
- Header: `Home` and compact connection indicator (`~21781-21800`).
- Connection card: greeting from `local_label` and time-of-day, connected/offline/reconnecting status from neighbor/direct/relay state and `mesh_health`, mesh health text, peer counts, and scrollable `mesh_event_log` (`~21218-21415`).
- Action grid: Create Public Room → `CreateNewRoom`; Create Group Chat → `ShowCreateGroupDialog`; Add Friend → `OpenFriendRequests`; Join Ticket → `JoinFromTicket`; Import Friend → `ImportFriendFromFile`; Create Tunnel → `ShowCreateTunnelDialog` (`~21417-21513`). The grid is 3 columns wide, 2 medium, 1 narrow.
- Share files strip: currently routes to `OpenSettings` (`~21515-21533`), despite its visual affordance suggesting a file picker.
- Online Now card: friend records filtered through `peer_presence` and `names`; per-friend `OpenConversation(pk)` message (`~21545-21618`).
- Recent Activity card: `CardShell` (see `card_shell.rs`) rendering the newest 15 `RecentActivityEvent` values as 48 px rows — action-specific icon, `presentation::relative_time_from_system` age text, `truncate_with_ellipsis` title — with a count badge and "No recent activity" empty state (`~22055-22112`).
- Tunnels card: `tunnel_service.list_tunnels()`, status/peer display, close action `CloseTunnel(id)` (`~21703-21770`).
- Card reactivity (t_9aaac275): each rail card renders through `iced::widget::lazy` with a fine-grained selector dependency — `online_peers_card_data()`, `recent_activity_card_data()`, `tunnels_card_data()` (selectors section in `app.rs` before `view_main_empty_state`; lazy wrappers at the right-column build). A data change in one card rebuilds only that card; the 1 Hz `ActivityTick` bumps `activity_tick` (included in the Activity + Tunnels deps, excluded from the Peers dep). Isolation harness + full audit: `docs/ui-redesign/home-cards-reactivity.md`.
- Responsive behavior: below 640px Home stacks left/right columns; below 900px action grid uses two columns (`~21216`, `~21490-21513`).

### Chat screen

- `examples/iced_chat/app.rs:21844-22351`: `view_chat_panel`; loading/connecting states, header, log, composer, and overlays for chat options, help, member list, and related dialogs.
- `examples/iced_chat/app.rs:22353-22757`: `view_chat_header`; room title/avatar/status and toolbar actions including back, search/profile-related actions, shared files, details, and more/options. Exact buttons should be rechecked in this range before a visual worker changes labels or icons.
- `examples/iced_chat/app.rs:22760-23728`: room options popover and details/related panel content.
- `examples/iced_chat/app.rs:23730-24401`: `view_chat_log`; virtualized/windowed scroll rendering, date dividers, local/remote/system message grouping, sender/profile buttons, URL links, image/file/video attachments, reactions, delivery labels, retries, and context-menu targets.
- `examples/iced_chat/app.rs:24403-~24600`: `view_composer`; attach, text input, send, focus ID, input-change and submit behavior. `COMPOSER_INPUT` is declared at `app.rs:284-287`.
- `examples/iced_chat/presentation.rs:1-335`: grouping, date divider labels, initials, relative times, and delivery labels. This is shared by chat rendering and should be treated as a stable presentation API.

### Shared styling/assets

- `examples/iced_chat/design_tokens.rs:1-503`: palette, semantic colors, spacing, control heights, radii, focus, avatar/layout sizes, shadows, theme helpers, and button/container styles. Existing layout constants include `SIDEBAR_WIDTH = 280`, `DETAILS_PANEL_WIDTH = 280`, avatar sizes, and message/image limits.
- `examples/iced_chat/fonts.rs:1-293`: bundled compile-time fonts. Inter Regular/Medium/SemiBold/Bold is the primary UI family; Manrope is legacy, Raleway ExtraBold is branding, and JetBrains Mono is technical text. `font::load`/font registration helpers are in this module; do not add network font loading.
- `assets/icons/boru-chat-256.png`: window icon included by `main.rs`.
- `examples/iced_chat/notification/`: notification event, rendering, focus, service, and platform backend code. It is not a target screen but shares `AppMessage`/state handling and can regress if update routing is reorganized.
- `examples/iced_chat/gui_test_actions.rs` and `mcp_server.rs`: loopback-only diagnostic/test actions, including navigation, composer manipulation, snapshots, and dark-mode toggling. Preserve their semantic routes while changing visuals.

## STATE AND EVENT FLOW

```mermaid
flowchart LR
  CLI[main.rs CLI/data dir] --> NET[Tokio + Iroh endpoint/router]
  NET --> CH[NetEvent / ConversationNetEvent]
  NET --> FR[FriendEvent]
  NET --> WH[WhisperEvent]
  NET --> IN[InboxEvent]
  CH --> U[IcedChat::update(AppMessage)]
  FR --> U
  WH --> U
  IN --> U
  T[iced subscriptions: ticks, resize, GUI actions] --> U
  U --> S[IcedChat state]
  S --> V[IcedChat::view]
  V --> SB[Sidebar]
  V --> HOME[Home / ChatList]
  V --> CHAT[Chat panel]
  V --> O[Dialogs, popovers, lightbox, details]
  SB --> U
  HOME --> U
  CHAT --> U
  O --> U
  S --> P[SQLite/legacy stores + runtime handles]
  P --> NET
```

Important flows:

1. Startup: CLI → data directory/secret key → endpoint and protocol router → IcedChat. No command selects the lobby topic, then Home is displayed while the lobby/network state initializes.
2. Sidebar room selection: row → `RoomSelected(topic)`/`OpenRoom(topic)` → generation guard → async gossip subscribe → `RoomOpened` → active `ConversationLive`/screen cache → Chat view.
3. Send: text input → `InputChanged(String)` → `composer_text`; Enter or Send → `SendPressed` → local queued entry + gossip sender → `MessageSent`/delivery state updates.
4. Incoming room data: forwarder → `NetEvent(ConversationNetEvent)` → hidden-room pending queue or active-room reducer → entries, names, presence, unread, activity, and mesh state.
5. Friend/presence: `FriendEvent` and periodic connection ticks update friend store, presence cache, sidebar rows, Home Online Now, and Recent Activity.
6. Home action: action button emits its `AppMessage`; dialog state is stored in `IcedChat`, and confirmation emits the existing room/group/tunnel/import command path.
7. GUI automation: loopback MCP → `GuiTestCommand` → `AppMessage` semantic route. It must remain behaviorally equivalent to visible controls.

## INTERACTIONS AND HANDLERS

### Home/sidebar interactions

- Boru settings gear → `OpenSettings`.
- Add/header controls → add menu and friend/import/join/group/tunnel routes; inspect `view_sidebar` and dialog helpers before changing.
- Collapse section headers → `ToggleSidebarSectionCollapsed(usize)`.
- Chat row → `RoomSelected(topic)` then room-open path.
- Group create → `ShowCreateGroupDialog`; group row → `OpenGroupChat(topic)`.
- Join ticket input → `JoinTicketInputChanged`; submit → `JoinFromTicket`.
- Friend search/input → friend-request and friend import messages; friend row → `OpenFriendChat`, `OpenFriendProfile`, or `OpenPeerProfile` as applicable.
- Discovered peer row → chat/profile/catalogue actions.
- Public room row → directory/open-room actions.
- Requests manage → `OpenFriendRequests`; request rows → send/accept/decline/cancel/retry messages.
- Home Retry → `RetryConnection`; Home Details → `OpenConnectionDetails`.
- Home Create Public Room → create-room dialog (`CreateNewRoomNameChanged`, DHT/advertise toggles, confirm/cancel).
- Home Create Group Chat → group dialog (name, description, member toggles, confirm/cancel).
- Home Add Friend → request/import route.
- Home Join Ticket and Import Friend → ticket/file-picker paths.
- Home Create Tunnel → tunnel dialog, friend selection, create/cancel.
- Home Online Now “Msg” → `OpenConversation(peer)`.
- Home active tunnel close → `CloseTunnel(tunnel_id)`.
- Home Share files → currently `OpenSettings`; this is a known semantic/visual mismatch to resolve in a separate approved card.

### Chat interactions

- Back → `GoToChatList`.
- Header toolbar → details/options/member/profile/search/file actions represented by `AppMessage` variants around the chat section.
- Composer typing → `InputChanged`; Enter/submit and Send → `SendPressed`; Attach → `AttachPressed`.
- URL text → `OpenUrl`; remote sender label → `OpenPeerProfile`.
- Image click → `OpenImageLightbox` / `CloseImageLightbox`.
- File rows → execute/pause/resume/cancel/reshare download messages.
- Failed local message → `RetryOutgoingMessage(event_id)`.
- Right-click/context menu → `ContextCopyText`, `ContextCopyImage`, `CloseContextMenu`.
- Emoji/GIF controls → toggle picker, search, insert/send, advance animation.
- Help/options/member dialogs → corresponding toggle/close messages.
- Keyboard: global shortcuts are represented by `AppMessage::Shortcut(Shortcut)`; the composer has stable ID `COMPOSER_INPUT` and the slash/focus behavior is implemented in the update/subscription path. Preserve Escape, Enter, and focus semantics during visual refactors.

## BEHAVIOR PRESERVATION

Do not rename or remove the following shared contracts during UI work:

- `Screen`, `AppMessage`, `ConversationLive`, `IcedChat::update`, `IcedChat::subscription`.
- `NetEvent`, `ConversationNetEvent`, `FriendEvent`, `WhisperEvent`, and `InboxEvent` bridges.
- `presentation.rs` grouping/delivery/date helpers.
- `gui_test_actions.rs`/`mcp_server.rs` semantic commands and loopback safety.
- Store/state fields used by visible values: `local_label`, `names`, `FriendsStore`, `peer_presence`, `neighbors`, `direct_peers`, `relayed_peers`, `mesh_health`, `mesh_event_log`, `recent_activity`, `tunnel_service`, `entries`, `composer_text`, delivery/download fields, and unread state.

## VISUAL EVIDENCE

`baseline-home-1280x800.png` shows the current Home screen under Xvfb:

- 280px sidebar at left with BORU header, identity, Chats, Groups, Friends, Discover, Public Rooms, and Requests.
- Main panel header “Home” with green connection dot.
- Left content: greeting/connection card, mesh health and event log, three visible action cards, and Share files strip.
- Right content: Online Now, Recent Activity, and Tunnels cards.
- Empty-state text is visible for no conversations/groups/friends/public rooms/requests/online friends/tunnels.
- At this clean launch, two discovered peers appear in Discover and Recent Activity records them as online.
- No rendering crash occurred. Xvfb emitted a non-fatal DRI3/EGL acceleration warning; the screenshot is valid software-rendered evidence.

## ACCEPTANCE CRITERIA

- [x] Current application build succeeds (`cargo check --features gui --example boru` and `cargo build --features gui --example boru`).
- [x] Current application launches and remains alive under Xvfb with isolated temporary data (`baseline-launch.log`).
- [x] Home and Chat source regions are mapped to exact functions/line ranges.
- [x] Visible Home sections and live data sources are mapped.
- [x] Chat header, message log, composer, overlays, and relevant event paths are mapped.
- [x] Existing toolbar/composer and major sidebar/Home interactions are listed.
- [x] Shared styling/assets, non-target shared modules, and parallel-edit collision risks are identified.
- [x] Test inventory and incomplete/failing baseline are documented (`baseline-test-list.log`, `baseline-tests.log`).
- [x] Visual evidence is attached as a repository artifact (`baseline-home-1280x800.png`).

## KNOWN LIMITATIONS / RISKS

- `app.rs` is approximately 31,900 lines and contains nearly all UI and reducer behavior. Large-scale edits will create merge conflicts and make regressions difficult to isolate.
- `cargo test --lib` is not a green baseline: it timed out after 600 seconds with failures already observed. UI workers should run focused GUI tests and record whether failures are pre-existing.
- The current screenshot is Home only. A deterministic populated Chat screenshot needs a separate controlled two-instance/fixture setup; do not infer Chat visual correctness solely from the Home smoke test.
- The app uses both authoritative SQLite state and legacy/read-compatible JSON stores (see `docs/gui-architecture.md`). Visual code must not silently switch data sources.
- `Home` action labels and some sidebar labels are coupled to semantic `AppMessage` routes. The Share files strip currently opens Settings, a likely follow-up behavior card rather than a styling-only change.
- Direct-address publication is opt-in and privacy-sensitive; baseline runs use `--no-dht --no-relay` and do not publish direct addresses.

## SUGGESTED FOLLOW-UP / OWNERSHIP BOUNDARIES

- UI-04 should add reusable primitives in new focused module(s), extending `design_tokens.rs` only for global tokens. Prefer new files over repeated edits to `app.rs`.
- Assign Home/landing visual work to the `view_main_empty_state` range (`app.rs:21209-21840`) and its directly related tokens, with no reducer changes unless necessary.
- Assign Chat shell/header/composer visual work to `view_chat_panel`, `view_chat_header`, and `view_composer` ranges. Keep `view_chat_log` behavior separate because it owns grouping, virtualization, attachments, delivery, and context menus.
- Keep sidebar work in `view_sidebar*` functions and cache/dependency helpers; coordinate changes to `Screen`, `AppMessage`, and shared state with the owner of the reducer.
- Keep bootstrap/network and event-channel changes in `main.rs`/core modules under a separate card.
- Use `presentation.rs`, `design_tokens.rs`, `fonts.rs`, and `gui_test_actions.rs` as shared-file review gates. Any edits there should be small and tested because many screens depend on them.
