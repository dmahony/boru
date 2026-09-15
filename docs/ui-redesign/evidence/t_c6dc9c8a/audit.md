Boru Home screen baseline audit — UI-00 / t_c6dc9c8a

Date/evidence context

- Checkout: /home/dan/iroh-gossip-chat/.worktrees/t_c6dc9c8a
- Branch: wt/t_c6dc9c8a; HEAD 00009f1d (Boru 0.241.1).
- Working tree was clean before this artifact; no tracked source behavior was changed.
- Rust edition is 2021 and MSRV is 1.91 (Cargo.toml:18,37). Iced is 0.14 (Cargo.toml:197-198). GUI defaults to net + metrics + gui + wgpu-renderer (Cargo.toml:289-350).

Baseline verification

- PASS: `rb check --bin boru --features gui,video-playback,terminal` (DEBSRV, 48.99s, exit 0).
- PASS: `rb build --bin boru --features gui,video-playback,terminal` (DEBSRV, 2m15s, exit 0).
- Both runs emit a large pre-existing warning set (326 bin warnings in the check/build output), including unused imports, dead code, missing Debug implementations, unfulfilled lint expectations, and an f32 literal fallback warning. No compile error occurred.
- The remote build does not place target/debug/boru in this worktree, so local Xvfb screenshot capture was not possible without copying a binary. Xvfb and xdotool are installed. The existing reproducible capture script is `scripts/ui14_home_evidence.sh`; it requires a local executable and fixture/MCP startup. This limitation is explicit rather than substituting unverified screenshots.
- Recommended capture commands once a local binary is available: `BORU_BIN=/path/to/boru scripts/ui14_home_evidence.sh` (the script currently captures populated 1280x800 home/chat/file-sharing/create-group scenarios). For this audit's requested matrix, additionally run the same flow at 1024x768 and 640x800, with an empty fixture and a populated fixture; record app log and PNG paths outside git.

Home view boundaries and data flow

- `src/bin/boru/app/home.rs:63-121`: `ChatListDependency`, the lazy-cache snapshot. It captures theme/layout revisions, window dimensions, mesh health, relay/peer counts, mesh events, People & Activity data, tunnel data, opacity, reduced motion, network map, and local network info.
- `src/bin/boru/app/home.rs:313-445`: selectors. `online_peers_card_data()` reads `friends`, `peer_presence`; `recent_activity_card_data()` reads `notifications_state.recent_activity` and `activity_tick`; `people_activity_card_data()` combines both; `tunnels_card_data()` reads `tunnel_service.list_tunnels()` and `tunnels_state.shared_tunnels`/names.
- `src/bin/boru/app/home.rs:1233-1273`: `view_main_empty_state()` obtains the dependency, live `BoruTheme`, live Home/Sidebar/Responsive layout, then renders through `iced::widget::lazy`.
- `src/bin/boru/app/home.rs:1275-1376`: dependency construction, including relay watcher (`endpoint.home_relay_status()`), mesh/network snapshots, designer preview width, and `home_menu_item_opacity`.
- `src/bin/boru/app/home.rs:1478-2050`: `view_chat_list_content()`. Current composition is hero, Quick Actions, Mesh Health/status card, People & Activity, and Tunnels, arranged through HomeLayout section visibility/order and responsive tier rules. Wide layout puts Hero first, Quick Actions + Mesh Health in a row, then People Activity + Tunnels; narrow layout stacks.
- `src/bin/boru/app/home.rs:1381-1476`: `view_photo_home_hero()`. Uses bundled `assets/home/hero-mountains.png`, white greeting/welcome text, connection/transport/friends/identity metrics. This is a protected area for redesign work.
- `src/bin/boru/app/home.rs:2072-2118`: `HomeConnectionVariant` and `home_connection_variant()` pure mapping. Offline > Degraded > Good+peers Ready > relay-only Connecting > otherwise Starting. `view_chat_list_content()` additionally treats a connected home relay as reachable before applying the non-ready mapping.
- `src/bin/boru/quick_actions.rs:39-64,276-319`: four full-card buttons and responsive grid. It is a stable component boundary with existing messages; descriptions are i18n keys and cards are content-height driven.
- `src/bin/boru/card_shell.rs:101-179,614+`: data-agnostic reusable CardShell for title/subtitle/count/status/header action/body/footer/list/empty states. Home passes live radius and opacity; non-home callers rely on defaults.
- `src/bin/boru/status_card.rs:112-162,700-744`: status-card dependency and Retry/Details buttons. The card is shared presentation code but is currently called from Home with `view_status_card_with_location()` at `home.rs:1634-1663`.

Visible Home actions and real handlers

| Visible action | Render site / message | Handler and edge state |
| --- | --- | --- |
| New Chat | `quick_actions.rs:39-45` -> `AppMessage::OpenFriendRequests` | `app.rs:11708-11719` -> `update_contacts`; `contacts.rs:386-408` sets `Screen::FriendRequests`, remembers return screen, refreshes activity/shared-file/summary data. Despite the label, this opens friend requests, not a direct conversation picker. |
| Public Rooms | `quick_actions.rs:47-51` -> `AppMessage::OpenDirectory` | `app.rs:11865-11895` -> `update_discover`; `discover.rs:2853-2858` sets `Screen::Discover` and remembers return screen. Directory subscription/refresh is separate; navigation itself is synchronous. |
| Create Room | `quick_actions.rs:52-56` -> `AppMessage::CreateNewRoom` | `app.rs:9898` routes to `update_rooms`; `rooms.rs:283-306` resets and opens the create dialog, defaults visibility to PublicUnlisted, clears fields/errors, and focuses the name input. Cancel is guarded while submit is in flight (`rooms.rs:308-317`). |
| Share File | `quick_actions.rs:58-62` -> `AppMessage::OpenFileSharing` | `app.rs:11721-11753` sets `Screen::FileSharing`, resets tab/search/popovers, then asynchronously refreshes sharing summary and Shared by Me. It is not the native picker directly; picker flow is later in `files.rs` (`AddSharedFile` at 9046+). |
| Friend conversation | `home.rs:750-810` peer tile -> `AppMessage::OpenConversation(pk)` | `app.rs:8441` names it; `app/chat.rs:8989-9010` handles it by resolving/upserting the deterministic direct topic and returning `OpenRoom`. The invitation path is in `contacts.rs:976-1060`: updates conversation store/sidebar, signs and sends invite, and starts background subscription. Errors become `ErrorMsg`; no friendship relationship is implied. |
| View all activity | No current Home button/message was found for Recent Activity. `view_recent_activity_card()` (`home.rs:614-701`) has no `.on_view_all`; combined People & Activity header uses `OpenFriendRequests` (`home.rs:994-1005`), which is not an activity screen. Treat this as an absent action requiring an explicit product decision before implementation. |
| Create tunnel | `home.rs:1108-1127,1181-1211` -> `AppMessage::ShowCreateTunnelDialog` | `app.rs:9917` routes to tunnel state; `tunnels.rs:533-546` opens the friend-picker dialog. Empty and populated cards both retain this action; populated header changes to `OpenSettings` (`home.rs:1110-1114`). |
| Join tunnel | `home.rs:1192-1205` uses `centered_disabled_button` with no message | Intentionally disabled. The card copy says the advertised-offer flow is unavailable (`home.rs:1124-1150`). There is no Home Join message/handler to preserve; do not invent one during visual work. |
| Retry connection | `status_card.rs:733` -> `AppMessage::RetryConnection` | Root update arm `app.rs:11930-11953` starts the existing reconnect/bootstrap path. Only shown for Offline (`home.rs:1589`). |
| View connection details | `status_card.rs:744` -> `AppMessage::OpenConnectionDetails` | Root update arm `app.rs:11930+`; opens connection details UI. Status card shows it for Offline or Degraded (`home.rs:1590-1593`). |
| Tunnels populated View all | `home.rs:1113` -> `AppMessage::OpenSettings` | `app.rs:11700-11701` -> `update_settings`; tunnel manager is the existing destination. |
| People card View all / Find friends / +N | `home.rs:590,821,956-1005` -> `AppMessage::OpenFriendRequests` | Same contact screen path above; return screen is preserved. Peer tiles use `OpenConversation`, +N uses requests. |
| Tunnel row close | `home.rs:1083-1091` -> `AppMessage::CloseTunnel(id)` | Tunnel domain handler in `app/tunnels.rs`; row is derived from live TunnelService, status can be Active/Connecting/Connected/Revoked/Failed/Disconnected/Reconnecting or computed Expired. |

Theme, settings, and update paths

- `src/bin/boru/theme.rs:1261-1333`: `HomeTheme` owns Home geometry and typography defaults (peer/activity heights, gaps, quick-action sizing, status geometry, `show_activity_feed`). `BoruTheme` owns the merged theme.
- `src/bin/boru/app/settings.rs:1915-1965`: settings update owns `dark_mode`, live theme replacement, and increments `theme_revision`; `app.rs:16077+` exposes `boru_theme()`. Home dependency includes `theme_revision` and `layout_revision`, so iced lazy caches invalidate on theme/layout changes.
- `src/bin/boru/layout.rs:102-180+` defines `HomeSection`, `HomeLayout`, mode/order/visibility/grid/padding/gaps/card sizing. `layout_merge.rs` merges persisted TOML overrides. `boru-ui.example.toml:269-290` documents Home override keys.
- Accent and semantic colors resolve through `design_tokens`/BoruTheme functions used by Home/status/card code (`accent_primary`, `accent_green`, `color_warning`, `color_error`, `text_system`, `text_muted`). Do not hard-code a new global accent or change global widget defaults.
- `app.rs:9715-9745` wraps `update_inner`; `app.rs:9853+` handles leaving Home and saves room history; `app.rs:11330-11332` routes Home's join-ticket input. Network/presence/activity/tunnel updates are handled by existing update domains and invalidate their dependency snapshots through state/ticks.

Protected regions and likely conflict boundaries

- Protected: `view_photo_home_hero()`/hero asset and identity/connection metric semantics; sidebar/navigation (`app/sidebar.rs`); chat timeline/composer and connection-card shared consumers; protocol/network/persistence code; global `design_tokens`, fonts, and Iced defaults.
- Home-local likely files for follow-on UI work: `app/home.rs`, `quick_actions.rs`, `card_shell.rs`, `status_card.rs`, `home_people_activity.rs`, `home_tunnels.rs`, `theme.rs` (HomeTheme only), `layout.rs`/`layout_merge.rs` (Home overrides only), and Home locale keys in `locales/en.json` + `fr.json`.
- Avoid broad `app.rs` edits: it contains AppMessage, the update router, state, tests, and many unrelated domains. If callback/data contracts must change, update only the existing message/route arms and both locale files.
- Narrow-width logic is content-width based (`home.rs:1515-1529`, `layout.rs`), not raw window width. Existing thresholds include 360/520/560/720/1000/1440 depending on component; preserve this distinction when changing layout.

Existing focused test coverage

- `quick_actions.rs:321-437`: card visibility/layout, four action labels/icons/descriptions/messages, breakpoints.
- `app/home.rs` tests near `2180+`: pure Home connection variant/tone and activity/rail behavior.
- `app.rs` tests near `19202+`, `22064+`, `26634+`, `27504+`, `28900+`, `33031+`: connection details, files/friends/directory action routing, Home card projections, layout and create-room flows.
- `app/discover/action_tests.rs`, `card_tests.rs`, `ticket_tests.rs`: directory navigation and room-opening behavior.
- `app/tunnels.rs` tests near `1479+`: tunnel dialog state and row/action helpers.

Baseline state matrix (source-backed, screenshots pending local binary)

- Empty: connection card remains rendered; People Activity shows the online-peers empty copy and recent-activity empty copy; Tunnels shows illustration, no-active copy, explanation, Create tunnel, and disabled Join (`home.rs:723-748,848-873,1129-1155`).
- Populated: peer rows sorted by presence/name/key and capped visually; activity rows capped at four in combined card; tunnel rows preview max three and size naturally (`home.rs:750-845,875-938,1164-1173`).
- Narrow: Home switches to one-column stack when responsive tier asks for one column or content width is below `stack_breakpoint`; People/Activity itself switches row to column below 560; tunnel actions stack below 360 (`home.rs:975-992,1185-1211,1870-1919`).
- Empty/populated peer state source semantics: total count includes messageable friends; available count is Online/Away (`home.rs:316-360`, `home_people_activity.rs:63+`). Offline friends remain in the dependency rows but are not counted as available.
- Loading: no skeleton is intentionally used; Home comments at `home.rs:1715-1732` state sources are synchronously available at first render and later updates are event-driven. Profile images can arrive asynchronously and replace initials.

No source behavior was changed by this audit. The only intended tracked change is this textual audit artifact.
