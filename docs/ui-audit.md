# UI Audit: Boru Iced Desktop GUI

**Date:** 2026-07-22  
**Scope:** `examples/iced_chat/` — all view functions, state, and supporting modules  
**Codebase:** commit 26efc487 (latest — branding rename to "Boru")  
**Files covered:** `app.rs` (19,003 lines), `download_progress_view.rs`, `file_library.rs`, `file_library_ops.rs`, `gui_test_actions.rs`, `log_viewer.rs`, `mcp_server.rs`, `perf_tracker.rs`, `main.rs`

---

## 1. Current Component Hierarchy

### 1.1 Top-level layout (`view()` at `app.rs:10953`)

```
row![
  sidebar (Length::Fixed(280px), bg_surface background),
  main_panel (Length::Fill, bg_primary background)
]
```

The sidebar is always visible. The main panel switches content based on the active `Screen`.

### 1.2 Sidebar structure (`view_sidebar()` at `app.rs:11325`)

```
scrollable(
  Column[
    Header: "Boru" title + "＋" (add menu button) + "⚙" (settings button)
    Identity Row: local label + relay mode label (cached via iced::widget::lazy)
    CHATS section [collapsible]: conversation rows with avatars, names, preview text, timestamps, unread counts, online dots
    FRIENDS section [collapsible]: friend rows with avatars, names, online status, action buttons
    DISCOVER section [collapsible]: discovered-peer rows with chat/browse buttons
    REQUESTS section [collapsible]: friend-request rows with accept/decline buttons
    Spacer(Fill)
  ]
)
```

Four collapsible sections tracked by `sidebar_section_collapsed: [bool; 4]`.

### 1.3 Main panel screens (via `Screen` enum at `app.rs:1429`)

| Variant | View function | Description |
|---------|---------------|-------------|
| `ChatList` | `view_main_empty_state()` (line 12322) | Welcome/landing screen with branding, status card, action buttons, recent activity |
| `Chat { topic }` | `view_chat_panel()` (line 12558) | Header + scrollable chat log + composer |
| `FriendRequests` | `view_friend_requests()` (line 13700) | Incoming/outgoing request management |
| `Settings` | `view_settings_screen()` (line 13223) | Appearance, network, identity, shared files, about |
| `PeerProfile(pk)` | `view_peer_profile()` (line 14268) | Peer's public profile with shared files + download |
| `PeerCatalogue(pk)` | `view_peer_catalogue()` (line 14326) | Remote peer's file catalogue table |
| `FriendProfile(pk)` | `view_friend_profile()` (line 14476) | Full friend profile with context menu, rename, block, shared files |
| Chat (chat panel) | `view_chat_panel()` (line ~13807) | Header (room metadata, menu button) + message log (virtualized) + composer; system/remote/local messages with date separators and emoji reactions |
### 1.4 Chat panel structure (`view_chat_panel()` at line 12558)

```
Column[
  view_chat_header(): name + ticket + settings button + "?" help button
  view_chat_log(): scrollable Column of ChatEntry rows (bubbles, time separators)
  view_composer(): text_input + attach button + send button + help button
]
```

### 1.5 Overlays and dialogs

| Overlay | View function | Trigger |
|---------|---------------|---------|
| Create Room dialog | `view_create_room_dialog()` (line 11211) | "＋" → "Create New Chat" |
| Add Menu dropdown | `view_sidebar_add_menu()` (line 11008) | "＋" button in sidebar header |
| Help overlay | `view_help()` (line 13136) | "?" button in chat header |
| Remove confirmation | `view_remove_confirm_overlay()` (line 14964) | Remove/Block friend |
| Block confirmation | `view_block_confirm_overlay()` (line 15047) | Block friend |

Dialogs are rendered as `stack![]` overlays: backdrop -> base content -> dialog.

---

## 2. UI State Storage and Updates

### 2.1 Central state struct: `IcedChat` (line 1564)

~50 fields organized by category:

**Navigation (3 fields):** `screen: Screen`, `pending_topic`, `settings_return_to`

**Multi-conversation (1 field):** `conversations: HashMap<TopicId, ConversationLive>`

**ChatList state (6 fields):** `room_history`, `join_ticket_input`, `chat_list_error`, `sidebar_selected_topic`, `sidebar_section_collapsed`

**Active chat display cache (20+ fields):** `topic`, `ticket_str`, `entries`, `composer_text`, `names`, `sender`, `forward_handle`, `help_visible`, `pending_file`, `pending_image`, `download_entry_index`, etc.

**Shared network state (20+ fields):** `secret_key`, `gossip`, `endpoint`, `router`, `blob_store`, `local_label`, `local_public`, `relay_mode`, `friends`, `friend_mgr`, `neighbors`, `mesh_health`

**Persistent stores (7+ fields):** `data_dir`, `image_store`, `chat_history`, `outbox`, `storage`, `download_manager`, `profile_store`

**Settings (6 fields):** `settings`, `dark_mode`, `sound_enabled`, `share_direct_addresses`, `chat_text_size`

**Profile images (8 fields):** `profile_image_handle`, `profile_image_ticket`, `friend_image_handles`, `friend_image_tickets`, `pending_profile_image_tickets`, `friend_profile_versions`

**Caches & revisions (10+ fields):** `friend_online_cache`, `friends_sidebar_revision`, `requests_sidebar_revision`, `profile_cache`, `blocked_sharers`, `pending_neighbor_status`

**GUI test/MCP (10+ fields):** `iced_diagnostics`, `gui_action_rx`, `gui_action_history`, `pending_open_room_action`, etc.

### 2.2 Per-conversation state: `ConversationLive` (line 1457)

20+ fields per conversation:
- Subscription: `sender`, `forward_handle`, `topic`, `ticket_str`
- Chat: `entries: Vec<ChatEntry>`, `composer_text`, `follow_latest`, `names`, `self_sent_events`
- Downloads: `pending_file`, `pending_image`, `download_entry_index`, `active_download_transfer_id`
- Peers: `neighbors: HashSet<PublicKey>`, `pending_events`, `unread`

### 2.3 State update flow

1. **Network events** arrive via tokio mpsc channels, received by the Iced subscription stream (line 14173).
2. The subscription `unfold` polls 6 receivers: conversation net events, friend events, whisper events, inbox events, discovered peers, and GUI test actions.
3. Events map to `AppMessage` variants (line 2132) and enter the Iced event loop.
4. `update()` processes each message, mutating `IcedChat` state, dispatching async tasks, and returning `iced::Task`.
5. The `view()` method is called by the Iced runtime after each update to produce the new frame.
6. `iced::widget::lazy` is used for sidebar sections to skip re-rendering when dependencies haven't changed.

---

## 3. Peer ID → Visible Text Conversion

### 3.1 Local identity
- `local_label` set from CLI `--name` arg, falls back to `local_public.fmt_short()`.
- Displayed in the sidebar identity row via `profile_sidebar_identity_row()`.

### 3.2 Remote peer display names
- Stored in `names: HashMap<PublicKey, String>` per conversation (part of `ConversationLive`).
- Populated from incoming messages' labels, `AboutMe` broadcasts, and profile updates.
- `profile_cache: HashMap<PublicKey, PeerProfileData>` stores richer profile data (display_name, bio, last_updated).
- `PeerProfileData.display_name` is the canonical display name from the peer's `ProfileUpdate`.
- Fallback: `fmt_short()` of the PublicKey.

### 3.3 Sanitization
- `sanitize_display_text()` (from `boru_core::abuse_controls`) is applied to all display text.
- `sanitize_single_line()` removes newlines from single-line fields (labels, names).
- `DEFAULT_MAX_DISPLAY_LENGTH` bounds string lengths.

---

## 4. Nicknames, Remote Profile Names, and Device Names

### 4.1 Local nickname
- Set via CLI `--name`, stored in `IcedChat.local_label`.
- Can be updated at runtime via `AppMessage::SetNickname`, but the UI path to trigger this is currently limited (no dedicated edit-nickname field in settings, though the profile store supports it).

### 4.2 Remote profile names
- `profile_cache: HashMap<PublicKey, PeerProfileData>` stores display_name, bio, last_updated per peer.
- Populated from `ProfileUpdate` gossip messages.
- Friend profiles also access `UserProfile` from `UserProfileStore` (persistent store).

### 4.3 Device names
- Not explicitly tracked in the UI. The `local_label` serves as the local device/identity name.
- No per-device naming or multi-device UI.

---

## 5. Online/Offline State Determination

### 5.1 Mechanism
- Gossip neighbor events: `on_neighbor_up` / `on_neighbor_down` callbacks from the gossip protocol.
- Events flow through `ConversationNetEvent::NeighborUp` / `NeighborDown` → `handle_neighbor_event()`.
- `pending_neighbor_status: HashMap<PublicKey, bool>` debounces these events.
- Flushed on every `ConnMonitorTick` (~1s) into two separate caches:
  - `friend_online_cache: HashSet<PublicKey>` — for friends
  - `discovered_online_cache: HashSet<PublicKey>` — for discovered peers
- The two caches are kept separate to avoid conflating friend vs discovered-peer status.

### 5.2 Display
- Green "●" filled circle for online, "○" hollow circle for offline (both implemented as actual circle widgets in the sidebar conversation rows, not Unicode characters).
- Rendered via `Stack![]` overlay on the avatar with a 10px green dot at bottom-right.
- Offline status shows no dot (the avatar alone).

### 5.3 Mesh health
- `MeshHealth` enum (Good/Degraded/Offline) from the quiescence watchdog.
- Displayed on the landing page status card and in settings.

---

## 6. Functional Actions

### 6.1 Fully functional
- ✅ Message send/receive in chat rooms
- ✅ Room creation and ticket sharing
- ✅ Room joining via ticket
- ✅ Friend request send/accept/decline/cancel
- ✅ Friend removal and blocking
- ✅ Dark/light mode toggle
- ✅ Text size configuration
- ✅ File sharing (send files via chat)
- ✅ Download manager with pause/resume/cancel/retry
- ✅ Peer profile viewing with download buttons
- ✅ Remote file catalogue browsing
- ✅ Profile image setting (local)
- ✅ Profile image display (remote — via AboutMe/dedicated inbox channel)
- ✅ Ticket input and join
- ✅ DHT-based peer discovery
- ✅ mDNS-based LAN discovery
- ✅ Whisper (DM) messages
- ✅ Offline mailbox messages
- ✅ Inbox protocol (ack, delivery receipts)
- ✅ Clipboard copy (Copy Peer ID)
- ✅ Delete room / clear history
- ✅ Add Friend via public key input
- ✅ Help overlay with command reference
- ✅ MCP diagnostic server
- ✅ Performance tracking (`--perf` flag)

### 6.2 Partially functional
- 🟡 **Shared file library** (`file_library.rs`, `file_library_ops.rs`) — the data model and operations are fully implemented (import, reference, hash, verify), but the UI integration into the main app is not yet wired up (the settings screen shows `#[allow(dead_code)]` fields).
- 🟡 **Friend rename** — the profile store supports it, the UI has the rename input in friend profile, but it relies on the profile broadcast protocol which may have delays.
- 🟡 **Delivery state indicators** — tracked in `ChatEntry.delivery_state` but not visually surfaced in the chat log rendering (only cached in `label_text`).

### 6.3 Not functional / stub
- ❌ **Voice button** in friend profile — has padding and style but **no `on_press` handler** (line 14306-14321). Clicking it does nothing.
- ❌ **QR code scanning** — listed in the Add menu but disabled with no implementation.
- ❌ **Group chat creation** — listed in the Add menu but disabled.
- ❌ **Pair Device** — listed in the Add menu but disabled.
- ❌ **Context menus** — no right-click or long-press menus anywhere. All interactions use explicit buttons.
- ❌ **Export Friend** — Import Friend exists but no corresponding export.

---

## 7. Iced Framework Limitations

### 7.1 Known constraints affecting this UI
1. **No native context menus** — Iced does not have a built-in context menu widget. All context actions must be explicit buttons, hover panels, or modal overlays.
2. **No native drag-and-drop** — Iced lacks drag-and-drop support. The DnD in this app refers to a file-access protocol, not UI-level drag-and-drop.
3. **Scrollable auto-scroll** — Must be manually managed via `scroll_offset` and `follow_latest`.
4. **Widget caching** — `iced::widget::lazy` requires explicit dependency structs (e.g., `SidebarChatsDependency`) which must be re-created every frame.
5. **No built-in toast/notification** — Toast messages are manually implemented with a `toast_message: Option<String>` + `toast_counter: u32` pattern.
6. **Image memory management** — Manual via `MAX_IMAGE_BYTES`, `MAX_ENTRIES`, and eviction logic in the `entries_push` path.
7. **Async task coordination** — Iced's event loop is single-threaded. Long async operations must be bridged via channels (tokio mpsc → subscription stream → AppMessage).
8. **No CSS-like box model** — All spacing, padding, and layout must be explicitly specified in code.
9. **`button` needs minimum width/height** — Icon-only buttons (e.g., "●" settings indicator) require explicit dimensions, otherwise they collapse.
10. **No `z-index`** — Stacking is determined by child order in `stack![]`.

### 7.2 Patterns used to work around limitations
- Subscription streams for async events (network, timers)
- `iced::widget::lazy` with manual `Dependency` structs for sidebar caching
- Manual `PerfTracker` for frame timing analysis
- `Arc<Mutex<Channel>>` for bridging async tasks to Iced's sync event loop
- `Cell` / `RefCell` for &self interior mutability in view functions (e.g., `total_content_height`, `perf`)

---

## 8. Design Constants

### 8.1 Typography (app.rs:182-205)

| Token | px | Usage |
|-------|----|-------|
| TYPO_XXS | 10 | Fine print, ticket text, timestamps, size labels |
| TYPO_XS | 11 | Metadata, identity info, secondary labels, section headers |
| TYPO_SM | 13 | Secondary body, preview text, entry labels (default chat body) |
| TYPO_MD | 15 | Body text, section headers, primary button labels |
| TYPO_LG | 18 | Secondary heading, room name, sidebar app name |
| TYPO_XL | 24 | Primary heading, settings page title |

### 8.2 Spacing (app.rs:229-246)

| Token | px |
|-------|----|
| SPACE_2 | 2 |
| SPACE_4 | 4 |
| SPACE_6 | 6 |
| SPACE_8 | 8 |
| SPACE_10 | 10 |
| SPACE_12 | 12 |
| SPACE_16 | 16 |
| SPACE_24 | 24 |

### 8.3 Colours

**Light theme background:**
- `bg_primary`: `#f0f0f6` (main panel)
- `bg_surface`: `#ffffff` (sidebar, cards)
- `bg_hover`: `#e6e6f2`
- `border_muted`: `#d9d9e0`

**Dark theme background:**
- `bg_primary`: `#1a1a2e`
- `bg_surface`: `#2a2a3e`
- `bg_hover`: `#33334d`
- `border_muted`: `#383852`

**Accents (both themes):**
- `accent_primary`: light `#2e70cc`, dark `#4a9eff`
- `accent_green`: light `#1a8c33`, dark `#3ddc84`
- `color_error`: light `#bf2626`, dark `#e64040`

**Chat text colours:**
- `text_muted`: light `#666`, dark `#999`
- `text_system`: light `#595959`, dark `#999`
- `text_local_label`: light `#007300`, dark `#33cc33`
- `text_local_body`: light `#005900`, dark `#4de64d`
- `text_remote_label`: light `#0054A8`, dark `#66a6ff`
- `text_remote_body`: light `#222`, dark `#ccc`

### 8.4 Border radii
- 4px: sidebar selected row, small buttons
- 6px: buttons, state badge
- 8px: cards, chat bubbles, composer container
- 10px: state badge pill, download card
- 12px: avatars, dialogs, help panel

### 8.5 Layout dimensions
- Sidebar width: 280px fixed
- Settings content max-width: 520px
- Chat bubble max-width: 480px
- Window default: 1280×720 (Iced default)

---

## 9. Existing UI-Related Tests

### 9.1 `examples/iced_chat/gui_test_actions.rs` (19 tests)

These test the GUI test-action infrastructure — rate limiting, action history trimming, and channel capacity:

| Test | Lines | Verifies |
|------|-------|----------|
| `test_rate_limit_error_display_burst` | 429 | Error message formatting |
| `test_rate_limit_error_display_minute` | 441 | Error message formatting |
| `test_rate_limit_error_serde_json` | 453 | JSON serialization |
| `test_rate_limit_error_serde_json_minute` | 470 | JSON serialization |
| `test_rate_limiter_new_is_empty` | 488 | Constructor |
| `test_rate_limiter_default_is_new` | 494 | Default constructor |
| `test_rate_limiter_first_action_ok` | 502 | First action always passes |
| `test_rate_limiter_burst_rejects_fast_action` | 512 | Burst limit enforcement |
| `test_rate_limiter_burst_allows_after_interval` | 533 | Recovery after interval |
| `test_rate_limiter_burst_rejects_any_fast_action` | 547 | Burst limit enforced on all fast actions |
| `test_rate_limiter_minute_limit_hit` | 564 | Minute limit enforcement |
| `test_rate_limiter_minute_limit_exact_boundary` | 593 | Minute limit at boundary |
| `test_rate_limiter_minute_limit_prunes_old_entries` | 610 | Pruning of stale entries |
| `test_rate_limiter_mixed_burst_and_minute` | 635 | Both limits interplay |
| `test_rate_limiter_shared_via_arc_mutex` | 664 | Thread-safe access |
| `test_rate_limiter_arc_mutex_multiple_access` | 680 | Multiple accesses |
| `test_rate_limiter_load_is_bounded_and_reproducible` | 698 | Load behavior |
| `test_gui_action_history_trimming_under_load` | 722 | History trimming at MAX_HISTORY |
| `test_gui_action_queue_capacity_and_drain_throughput` | 749 | Queue capacity |

### 9.2 `examples/iced_chat/log_viewer.rs` (1 test)

| Test | Verifies |
|------|----------|
| `build_spawn_command_sets_data_dir_env_and_keeps_logs_as_the_only_argument` | Command args and env vars |

### 9.3 Tests in `file_library_ops.rs`

The file operations module (`file_library_ops.rs:2148`) contains extensive tests for file hashing, import, and referencing workflows (these are in the test module at the end of the file).

### 9.4 Limitations of current test coverage
- **No snapshot tests** of UI rendering — Iced does not have a built-in snapshot/approval testing framework.
- **No integration tests** exercising the full update→view→widget cycle.
- **No test for the dead "Voice" button** (line 14306).
- The MCP server (`mcp_server.rs`) has no test for the `boru_get_iced_state` path.
- No coverage of the `view_sidebar_collapsible_section_header` toggle logic in isolation.

---

## 10. Safe Sequence of Changes

Based on the audit, the recommended order for a UI redesign (existing `DESIGN_SYSTEM.md` already defines Steps 2-9):

### Phase 1: Infrastructure (no visual changes)
1. **Extract design tokens** → new `theme.rs` module (colours, spacing, typography constants from `app.rs`)
2. **Standardise button style helpers** → single `button_style()` function with a `ButtonKind` enum

### Phase 2: Component refactors
3. **Replace inline `" [N]`" unread badges** with dedicated pill containers
4. **Replace unicode status dots** (●/○) with proper circle widget (already partially done in sidebar conversations)
5. **Fix the dead "Voice" button** — either implement or disable with "coming soon" toast

### Phase 3: Layout and navigation
6. **Refine sidebar spacing** — consistent section gaps, section header padding
7. **Polish chat panel** — bubble borders, timestamp alignment, avatar layout in chat log
8. **Implement context menus** for conversation rows and friend rows

### Phase 4: New features
9. **Add friend search** to the FRIENDS sidebar section
10. **Add chat search** to the CHATS sidebar section
11. **Add search/filter** to discovered peers
12. **Add toast notification system** for transient actions
13. **Implement message delivery status indicators** (✓/✓✓) using existing `DeliveryState`

### Phase 5: UX improvements
14. **Add first-run onboarding overlay**
15. **Hide or explain disabled menu items** ("Scan QR Code", "Create Group Chat", "Pair Device")
16. **Move mesh/relay status** from landing page to Settings only
17. **Add room-level settings** (mute, leave room, share ticket)

### Phase 6: Testing
18. **Add snapshot tests** for key view functions (sidebar, chat log, settings)
19. **Add integration tests** for the MCP-driven GUI action path
20. **Add unit tests** for the `view_sidebar_conversation_row` rendering invariants

---

## 11. Key Files Reference

| File | Lines | Purpose |
|------|-------|---------|
| `examples/iced_chat/app.rs` | 19003 | Main application: IcedChat struct, AppMessage enum, all view functions, Application trait impl |
| `examples/iced_chat/main.rs` | 1481 | Entry point, CLI args, endpoint/router setup, data dir, logging |
| `examples/iced_chat/download_progress_view.rs` | 464 | Stateless download progress card widget |
| `examples/iced_chat/file_library.rs` | 1303 | File library state, filters, sorts, add-file wizard models |
| `examples/iced_chat/file_library_ops.rs` | 2148 | File hashing, import, reference operations |
| `examples/iced_chat/gui_test_actions.rs` | 778 | GUI test action types, rate limiter, action history, channel infrastructure |
| `examples/iced_chat/log_viewer.rs` | 165 | Standalone log viewer application |
| `examples/iced_chat/mcp_server.rs` | 5927 | MCP diagnostic server with GUI state inspection tools |
| `examples/iced_chat/perf_tracker.rs` | 252 | Performance instrumentation |
| `DESIGN_SYSTEM.md` | 646 | Complete design token specification |
| `UX_AUDIT.md` | 273 | UX audit with persona walkthroughs and recommendations |
| `Cargo.toml` | 376 | Package manifest (boru-core library) |

---

## 12. Critical Findings

### HIGH priority
1. **Dead "Voice" button** in `view_friend_profile` (app.rs:~14306) — has styling but no `on_press`. Clicking it visibly does nothing.
2. **No onboarding flow** — first-run users see action buttons with no guidance on what to do first.
3. **Delivery state tracked but not visually surfaced** — `ChatEntry.delivery_state` is updated but only rendered as a cached icon in `label_text`. No visible ✓/✓✓ indicators in the chat log.

### MEDIUM priority
4. **Chat search missing** — no way to filter conversations in the sidebar.
5. **Friend search missing** — no search/filter in the FRIENDS section.
6. **Settings button goes to global settings** — from within a chat, "Settings" opens global settings, not room-level settings.
7. **Disabled menu items shown without explanation** — "Scan QR Code", "Create Group Chat", "Pair Device" are greyed out with no tooltip.
8. **Mesh/relay jargon on landing page** — networking terms shown to every user on every launch.

### LOW priority
9. **No file library UI integration** — full file library operations exist in `file_library_ops.rs` but the UI is not wired into the main app flow.
10. **No context menus** — all actions use explicit buttons; no right-click menus.
11. **Missing "Export Friend"** to complement "Import Friend".
12. **Avatar "?" fallback in chat** — messages without an avatar handle show a bare "?`" in TYPO_XL, looking like an error state rather than a missing-avatar fallback.
