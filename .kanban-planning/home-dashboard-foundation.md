# Boru Home Dashboard Redesign Foundation

Status: baseline mapping and implementation contract, 2026-09-11
Scope: PDF Home tasks 01-06 plus contracts needed by panel tasks 07-17.

## Confirmed implementation map

The home landing screen is `src/bin/boru/app/home.rs`, especially:

- `IcedChat::view_main_empty_state` (`home.rs:1067`) builds a lazy home tree from `ChatListDependency`.
- `IcedChat::chat_list_dependency` (`home.rs:1107`) snapshots live connection, activity, people, tunnel, theme, layout, and network-map state.
- `IcedChat::view_chat_list_content` (`home.rs:1323`) resolves responsive tier, section visibility/order, and renders the home sections.
- The current section assembly inserts `HomeSection::{Hero, MeshHealth, QuickActions, PeopleActivity, Tunnels}` at `home.rs:1830-1837`; wide mode places the hero and the primary cards at `home.rs:1912-1919`.
- Home navigation/update glue is in `app.rs`; `AppMessage` defines tunnel actions at `app.rs:3952-3971`, and the root update dispatch routes tunnel arms to `IcedChat::update_tunnels` (`app.rs:9919-9921`), while `JoinTicketInputChanged` routes to `update_home` (`app.rs:11327-11329`).
- `src/bin/boru/app/tunnels.rs` owns `TunnelsState`, `TunnelsMessage`, tunnel dialogs, and `IcedChat::update_tunnels` (`tunnels.rs:84-160`, `tunnels.rs:531-705`). Existing handlers include `ShowCreateTunnelDialog`, `CreateTunnel`, `CancelCreateTunnel`, and `CloseTunnel`; no new network/storage path is required.
- Existing navigation handlers are confirmed: `OpenFriendRequests` -> `Screen::FriendRequests` (`app/contacts.rs:384-408`), `OpenDirectory` -> `Screen::Discover` (`app/discover.rs:2853-2859`), and `CreateNewRoom` opens/resets the room dialog (`app/rooms.rs:280-286`). `OpenFileSharing` is a root navigation arm at `app.rs:11718-11721`.

## Presentation projections and callback contracts

These are projections only. Authoritative state remains `IcedChat`, `NotificationsState`, `FriendsStore`/presence maps, and `TunnelService`.

1. Quick Actions: `src/bin/boru/quick_actions.rs::QuickAction` is the tile contract (`icon`, i18n `label`, i18n `description`, one `AppMessage`). `ACTIONS` is ordered New Chat, Public Rooms, Create Room, Share File and `quick_action_card` has the sole hit target (`quick_actions.rs:29-60`, `116-179`). The existing routes are `OpenFriendRequests`, `OpenDirectory`, `CreateNewRoom`, and `OpenFileSharing`; do not add a second chevron handler or a new backend operation. `grid_columns_for`/`quick_action_grid` (`quick_actions.rs:250-305`) are the responsive projection.
2. People: `home.rs::OnlinePeerRow` (`home.rs:140-151`) carries stable `PublicKey`, resolved name, `PeerPresence`, and `SidebarAvatarHandle`; `online_peers_card_data` (`home.rs:282-316`) derives messageable friend totals and filters offline rows from live presence. The row callback is `AppMessage::OpenConversation(pk)` (`home.rs:748-750`); the card View all callback is `OpenFriendRequests` (`home.rs:913-917`). Do not manufacture friends or presence.
3. Activity: `home.rs::ActivityRow` (`home.rs:225-233`) projects real `notifications_state.recent_activity` entries with description, `ActivityKind`, and stable `SystemTime`; `recent_activity_card_data` (`home.rs:322-342`) takes the newest bounded slice and preserves the ring-buffer total. Relative text uses `presentation::relative_time_from_system`; no event insertion belongs in a view.
4. Tunnels: `home.rs::TunnelRow` (`home.rs:254-276`) projects `TunnelService::list_tunnels()` into stable `TunnelId`, authorized display name, target label, lifecycle status, and expiry. `TunnelService::TunnelStatus` is the confirmed state set (`src/tunnel/service.rs:80-114`), and `TunnelDefinition` includes `active_connections` (`service.rs:342-363`). `tunnels_card_data` (`home.rs:358-398`) is the selector; rows use `CloseTunnel(id)` (`home.rs:992-1000`) and the header uses `ShowCreateTunnelDialog` (`home.rs:1021-1025`). Never expose capability secrets, enrollment tokens, or invitation strings.

## Reusable visual parts and responsive rules

Use existing home-scoped parts, not new global design systems: `CardShell`/`card_shell.rs`, `Avatar` and existing icon helpers from `ui_components.rs`/`icon_system.rs`, `quick_action_card` in `quick_actions.rs`, and `BoruTheme` tokens. The typed theme exposes Home values in `theme.rs::HomeTheme` (`theme.rs:1261-1322`), including 128px peer minimum, 32px activity rows, 40px quick-action icon, 16px/14px title/description, and 1.45 description line height. TOML overrides mirror these in `theme_config.rs::HomeConfig` (`theme_config.rs:506-531`) and tunnel chip values in `TunnelConfig` (`theme_config.rs:635-642`).

Structural rules are authoritative in `layout.rs::HomeSection`/`HomeLayout` (`layout.rs:102-182`) and `QuickActionsLayout` (`layout.rs:256-289`). The default home order is Hero, QuickActions, MeshHealth, PeopleActivity, Tunnels. `ResponsiveLayout` uses one home column below 360px and two at 360px and above (`layout.rs:1294-1301`, `1440-1446`); Quick Actions default to 1/2/4 columns by content width (`quick_actions.rs:250-265`, 1000px and 520px breakpoints). Rendered cards must wrap content rather than clip it, preserve minimum hit targets, and keep card/widget identity stable through updates. The baseline visual canvas is 1200x800; existing captures are `captures/home_light.png` and `captures/home_dark.png` (both verified PNG 1200x800). Verified captures show a sidebar, greeting/header, connection hero, Mesh Health, and right-rail Online Peers, Recent Activity, and Tunnels cards. Light mode is pale/white with green accents; dark mode is charcoal/slate with green status accents and muted light text.

## Ownership for parallel panel work

Explicit PDF requirements do not require a new persistence or networking layer. To keep panel edits disjoint, the proposed implementation boundary is:

- Quick Actions worker: `src/bin/boru/quick_actions.rs` (already isolated; tests in its module).
- People & Activity worker: `src/bin/boru/app/home_people_activity.rs` (extract `OnlinePeerRow`, `ActivityRow`, selectors and card renderer from `home.rs` before substantive panel edits; tests stay in that module).
- Tunnels worker: `src/bin/boru/app/home_tunnels.rs` (extract `TunnelRow`, selector and card renderer from `home.rs`; tests stay in that module).
- Coordinator integration, if extraction is required, is a separate follow-up/reconciliation change because `app.rs` and `home.rs` are shared hotspots. No worker may modify protected shell/navigation, protocol, networking, storage, or global theme files merely to make a visual panel work.

The two new module paths are a proposed implementation choice, not a PDF requirement; the requirement is file-disjoint ownership and a single authoritative state path. Until extraction is coordinated, panel workers must not concurrently edit overlapping regions of `home.rs`.

## Protected regions and no-scope-change rules

Do not alter the existing sidebar, hero/greeting, BORU logo/branding, connection/status card or Mesh Health semantics; networking/bootstrap/discovery; SQLite/storage/history; gossip/protocol types; dependency versions/features; global theme tokens; or unrelated screens. Panel work may consume existing handlers and tokens, but must not fake unavailable capabilities, claim encryption/anonymity beyond existing state, add sample names/counts/events, or expose secrets. Existing dialogs, validation, permissions, cancel/confirmation, and pending-operation guards remain authoritative.

## Baseline and verification

- Repository manifest confirms package `boru-core` version 0.237.1, Rust edition 2021, and the locked Iced dependency resolves to Iced 0.14.0 (`Cargo.toml`, `Cargo.lock`).
- CI recipes are in `justfile`: `check-gui`, `build-gui`, `lint-gui`, `test-gui`, and `ci-gui`. For Kanban verification, the repository workflow requires DEBSRV `rb`; the repeatable baseline command was `rb check --bin boru --features gui,video-playback,terminal` from this worktree.
- Baseline result: PASS, remote debug check finished in 36.03s. It emitted 329 binary warnings and 5 library warnings, including existing unused imports/dead-code/private-interface/unfulfilled-expectation warnings; no compile error was reported. These warnings are pre-existing baseline noise and are not part of this foundation note's scope.
- Existing baseline scripts include `scripts/ui_home01_baseline.sh` and related `ui_home*` evidence scripts. The script is read-only with respect to application code and expects an existing `target/debug/boru`, Xvfb, xdotool/ImageMagick, and MCP helper; it records 1280x800 and 1920x1080 captures when those prerequisites are available. This run did not rerun it because the required binary/MCP runtime was not established and existing captures are already present.
- Required pre-commit synchronization: run `git fetch origin && git merge origin/main`; preserve unrelated worktree changes. This note intentionally makes no dependency or runtime changes.
