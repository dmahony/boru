# Step 1: Audit — Existing Event Flow in Boru Chat

## Modules receiving incoming events
- `AppMessage::NetEvent(ConversationNetEvent)` — dispatched from `subscription_stream` via `forward_gossip_events`
- `AppMessage::FriendEvent(FriendEvent)` — from `FriendPingManager`
- `AppMessage::WhisperEvent(WhisperEvent)` — from whisper/DM protocol
- `AppMessage::InboxEvent(InboxEvent)` — offline mailbox events
- All processed in `IcedChat::update()` (~line 5020)

## Unread state
- Per-conversation `unread: u64` on `ConversationLive` (line 1574)
- Incremented at line 7364 when `is_inactive` is true AND event is user-visible
- `is_inactive = topic != self.topic || screen != Screen::Chat` (line 7347-7348)
- Cleared to 0 when the conversation becomes active (line 7369)

## Active conversation detection
- `self.topic` — topic being displayed in the main panel
- `self.screen == Screen::Chat { topic }` — verifies we're on chat view
- No distinction between "focused" and "selected" — only "visible" vs "background"

## Window focus / visibility
- **NOT tracked at all** — no `window_focused`, `window_visible`, `window_minimised` state
- Only `window_width: f32` exists for responsive layout
- Subscription only includes `iced::window::resize_events()` (line 15242)
- No `close_events()`, `focus_events()`, or `mode_events()` subscriptions

## Background event loop
- Tokio runtime with 1s `ConnMonitorTick`, 30s `MeshWatchdogTick`, 30s `OutboxRetryTick`

## Friend requests
- `FriendRequestStore` persists to disk (friend_request.rs)
- Events arrive via `WhisperEvent::Control → SignedContactMessage::FriendRequest`
- UI: `Screen::FriendRequests` with accept/decline/cancel actions

## File transfers
- `DownloadManager` + progress queue + `DownloadDone`/`DownloadFailed` events
- Progress updates via `download_progress_queue` drained on ConnMonitorTick

## Settings persistence
- `AppSettings` struct (line 141) saved as `settings.json` in data dir
- Fields: `dark_mode`, `sound_enabled`, `chat_text_size`, `share_direct_addresses`, `display_name`

## Tray support
- **Not implemented** — no tray icon, no minimise-to-tray behaviour

## Notification libraries
- `notify` crate (line 102-103) is for **filesystem** monitoring, not desktop notifications
- No `notify-rust` or similar notification library present

## GUI / platform details
- Iced 0.14 with `tokio`, `x11`, `wayland`, `tiny-skia`, `image`, `lazy`, `wgpu`
- Linux-only (x11 + wayland)
- Display names resolved via `FriendsStore::display_label()`, `fmt_short()`
- No macOS or Windows specific code

## Display name resolution
- `peer_names` module
- `FriendsStore::display_label(&fid, pk)` returns friendly name or truncated peer ID
- `pk.fmt_short()` for truncated public key strings
- `sanitize_display_text()` / `sanitize_single_line()` in `abuse_controls` module
