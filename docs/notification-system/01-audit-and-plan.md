# Step 1: Audit of Existing Event Flow

## Architecture Overview

Boru Chat is a Rust peer-to-peer messaging app built on:
- **iroh** v1 — P2P networking (QUIC, relays, hole-punching)
- **iroh-gossip** — Pub/sub gossip protocol for message distribution
- **iced** v0.14 — Cross-platform GUI framework (X11 + Wayland + tiny-skia)
- **boru-core** — Shared protocol library (no GUI dependencies)

The app lives in a workspace: core library at `src/`, GUI example at `examples/iced_chat/`.

---

## 1. Which modules receive incoming events

### Network-level
- **`iroh-gossip` gossip layer** (via `api::GossipReceiver`): receives raw signed messages from the gossip mesh
- **`friend_ping.rs`**: receives `FriendEvent` for direct-ping status changes
- **`whisper` module**: receives `WhisperEvent` for off-grid control messages (friend requests, chat invites)
- **`inbox.rs`**: receives `InboxEvent` for offline mailbox messages (ack sync)

### Application-level
- **`api.rs`**: `Event` enum from gossip topic subscription (decoded signed messages forwarded by protocol router)
- **`chat_core.rs` → `NetEvent` enum**: typed events decoded from gossip:
  - `NetEvent::Message { from, message, sent_at }` — user messages
  - `NetEvent::NeighborUp { peer }` — peer connected
  - `NetEvent::NeighborDown { peer }` — peer disconnected
  - `NetEvent::Closed` — topic closed
  - `NetEvent::Error(String)` — network error

### GUI-level
- **`app.rs` → `AppMessage::NetEvent(ConversationNetEvent)`**: Tagged with topic ID, processed in `update()`
- **`app.rs` → `AppMessage::FriendEvent(FriendEvent)`**: Friend status changes from direct pings
- **`app.rs` → `AppMessage::WhisperEvent(WhisperEvent)`**: Friend requests, chat invites, friend request acceptances
- **`app.rs` → `AppMessage::InboxEvent(InboxEvent)`**: Offline message mailbox events

---

## 2. Which components update unread state

Unread state is stored per-conversation in **`ConversationLive.unread: u64`** (runtime-only, not persisted).

Update logic in `app.rs` lines ~7356-7369:

```rust
if is_inactive {
    let should_count = Self::_is_user_visible_event(&event);
    conversation.pending_events.push_back(event);
    if should_count {
        conversation.unread = conversation.unread.saturating_add(1);
    }
    return iced::Task::none();
}
conversation.unread = 0; // Reset when viewing the conversation
```

The `_is_user_visible_event()` filter (line ~10648) considers:
- `Message::Message { text }` — user text messages
- `Message::FileShare { .. }` — file shares
- `Message::ImageShare { .. }` — image shares

Gossip protocol events (AboutMe, Presence, Heartbeat, NeighborUp/Down) are excluded.

**Key observations:**
- Unread state is **not persisted** to disk — lost on restart
- Unread is reset to 0 when any event arrives for the active conversation
- Sidebar reads `conversations.get(&entry.topic).map(|c| c.unread)` via `SidebarChatsRow`
- No total unread count is maintained (only per-conversation)

---

## 3. How the app determines whether a conversation is open

The app tracks the active screen via `Screen` enum:

```rust
pub enum Screen {
    Splash,
    ChatList,
    Chat { topic: TopicId },
    FriendRequests,
    Settings,
    PeerProfile(PublicKey),
    PeerCatalogue(PublicKey),
    ImagePreview { topic: TopicId, entry_index: usize },
    FriendProfile(PublicKey),
}
```

A conversation is considered "open" when:
- `self.screen == Screen::Chat { topic }` AND
- `topic == event.topic`

Determined by `is_inactive` check in the `NetEvent` handler (line ~7347-7348):

```rust
let is_inactive =
    topic != self.topic || !matches!(self.screen, Screen::Chat { .. });
```

**Key observations:**
- Only the **active** conversation (self.topic) can be "open"
- Background conversations always increment unread
- No per-conversation mute state exists yet
- No concept of "focus" separate from conversation selection

---

## 4. Whether the GUI framework exposes focus and visibility events

**Iced 0.14** provides:
- `iced::window::focus_events()` — emits `Focused` / `Unfocused` events
- `iced::window::close_events()` — window close button events
- `iced::window::resize_events()` — window resize (already used for `WindowResized`)
- Window `Mode::Fullscreen`, `Mode::Hidden` via `iced::window::mode`

**Current usage:** Only `resize_events()` is subscribed (line ~15242). No focus, close, or mode-change events are tracked anywhere in the application.

**Key observations:**
- Focus events are available but unused
- Window visibility/minimise state is not tracked
- No `WindowEvent` subscription exists

---

## 5. Whether the app already has a background event loop

**Yes** — the app runs multiple background event sources via Iced subscriptions:

1. **`ConnMonitorTick`** (every 1s) — connection refresh, presence broadcast, mesh health, DHT discovery, outbox retry, download progress
2. **`MeshWatchdogTick`** (every 30s) — mesh quiescence checks
3. **`OutboxRetryTick`** (every 30s) — retry failed outbox messages
4. **`SplashTick`** (100ms while loading)
5. **Custom stream** (`subscription_stream`) — drains `net_rx`, `friend_events_rx`, `whisper_events_rx`, `inbox_events_rx`, `discovered_peers_rx`, `gui_action_rx` via channel reads

The subscription stream uses `iced::Subscription::run_with` with an async unfold.

---

## 6. Which platforms are currently supported

The core library targets all platforms supported by Rust + iroh.

The GUI (`iced_chat` example) targets:
- **Linux** — X11 + Wayland (via `iced` features)
- **macOS** — (via `iced`, but not tested/configured)
- **Windows** — (via `iced`, but not tested/configured)

The `Cargo.toml` features include `x11` and `wayland` for the `iced` dependency. No platform-specific notification code exists.

---

## 7. Which notification libraries or native APIs are practical

### Cross-platform notification crates:
- **`notify-rust`** — Rust bindings for DBus Notifications (Linux), NSUserNotification (macOS), Toast (Windows). Well-maintained, supports actions, icons, timeouts. No async setup needed beyond DBus channel.
- **`alert`** — Apple Notification Center (macOS only).
- **`winrt-notification`** — Windows Toast notifications (Windows only).
- **`x11-notify`** / **`libnotify`** — Linux only via DBus.
- **`rodio`** — For notification sounds (already available, `sound_enabled` is a setting).

### Given the iced framework + Linux-first support:
- **`notify-rust`** is the most practical choice: cross-platform, maintained, supports notification actions and click handling, pure Rust, no C dependencies beyond the platform's notification service.
- For Linux: uses DBus (`org.freedesktop.Notifications`).
- For macOS: uses NSUserNotification (or UNNotification on newer macOS).
- For Windows: uses Windows Toast notifications.

### Alternative: `libnotify` C bindings — more complex, requires `libnotify` dev libraries installed.

---

## 8. Whether the app already has tray support

**No.** The app has no system tray support. The window always has a titlebar and close button. Closing the window shuts down the application cleanly (after async shutdown via `endpoint.close()` in `main.rs` line ~1144).

The `iced` framework has `iced::window::Settings::platform_specific` with tray icon support on some platforms, but this is not used.

The `notify` crate in dependencies (`Cargo.toml` line ~102-103) is for **filesystem notification**, not desktop notifications.

---

## 9. How settings are currently persisted

Settings are stored as a flat JSON file `settings.json` in the app data directory:

```rust
pub struct AppSettings {
    pub dark_mode: bool,
    pub sound_enabled: bool,
    pub chat_text_size: f32,
    pub share_direct_addresses: bool,
    pub display_name: Option<String>,
}
```

- **Load:** `AppSettings::load(data_dir)` — reads and parses JSON, returns defaults on failure
- **Save:** `AppSettings::save(data_dir)` — writes pretty-printed JSON atomically
- Persisted data stores use `atomic_write_json` helper in `chat_core.rs`
- Other persisted stores: `friends.json`, `conversations.json`, `friend_requests.json`, `boru.db` (SQLite)

---

## 10. The safest point at which to generate internal notification events

**The `AppMessage::NetEvent` handler** in `app.rs` `update()` method (line ~7338) is the safest injection point, specifically:

1. **After the `is_inactive` check** (line ~7347-7348) — we already know whether the conversation is open
2. **After the unread counter update** (line ~7363-7365) — unread tracking is finalised
3. **Before returning `iced::Task::none()`** for inactive conversations (line ~7367)

For other event types:
- **`FriendEvent` handler** (line ~7396) — friend online/offline status changes
- **`WhisperEvent` handler** (line ~7402) — friend request received/accepted
- **`AppMessage::ConnMonitorTick`** (line ~15236) — periodic checks for connection state loss
- **`ChatCallbacks::on_transfer_progress`** (line ~11314) — file transfer completion/failure

The notification service should be invoked here, and the decision logic (should we show a notification?) should be encapsulated in the notification service, not in the networking code.

---

## Implementation Plan Summary

The notification system will be implemented as a new module at `examples/iced_chat/notifications/` with:

1. **`NotificationEvent`** — Internal event enum (NewMessage, FriendRequest, FileTransferCompleted, etc.)
2. **`NotificationBackend` trait** — Platform-neutral interface (show/update/close/available/permission)
3. **`NotificationService`** — Central coordinator: receives events, checks preferences/focus/mute, deduplicates, groups, renders, dispatches to backend
4. **`WindowFocusState`** — Tracks window focus/visibility via Iced window::Event subscriptions
5. **Unread state persistence** — Extend `ConversationStore` or `AppSettings` to persist unread counts
6. **Development backend** — Log-based, for testing
7. **Native backend** — Using `notify-rust` for Linux desktop notifications
8. **Settings UI** — Notification preferences in the settings page
9. **Per-conversation mute** — Mute state in `ConversationEntry`
10. **DND (Do Not Disturb)** — Schedule-based suppression
11. **System tray** — Tray icon with unread badge and quick actions
12. **Notification click routing** — Open the correct conversation when a notification is clicked

No code changes will be made to the core protocol library (`boru-core`). All notification logic lives in the GUI frontend.
