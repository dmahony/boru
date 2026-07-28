# Chat UI Redesign Baseline

Audit performed: 2026-07-28
Branch: `ui/chat-screen-redesign` (from `main` at `ca864a1d`)
Purpose: Establish a regression-safe baseline before incremental chat-UI redesign.

---

## 1. Relevant Files and Components

### Primary UI files (`examples/iced_chat/`)

| File | Lines | Role |
|------|-------|------|
| `app.rs` | 24,361 | Core Iced application: `IcedChat` struct, `Screen` enum, `AppMessage` enum (308 variants), `ConversationLive`, all view functions, `update()` handler |
| `main.rs` | 2,044 | Bootstrap: CLI args, iroh endpoint, gossip actor, protocol handlers, tokio runtime, Iced launch |
| `presentation.rs` | 335 | Shared view helpers: message grouping (`continues_message_group`), day keys, date dividers, delivery labels, initials |
| `design_tokens.rs` | 279 | Central palette, spacing (SPACE_4..SPACE_32), radius, typography sizes (TYPO_XS..TYPO_H1), button/container styles, icon constants (ICON_CHAT, ICON_FRIEND, etc.) |
| `fonts.rs` | — | Font loading: Manrope, SourceSans3, JetBrainsMono, Raleway-ExtraBold |
| `connection_details.rs` | — | Connection-details popover dialog |
| `download_progress_view.rs` | — | Per-entry download progress with speed tracking |
| `link_preview.rs` | — | Async URL detection and preview card rendering |
| `invitation_qr.rs` | — | QR-code generation for room invite tickets |
| `file_library.rs` | — | Local file library management UI |
| `file_library_ops.rs` | — | File library I/O operations |
| `log_viewer.rs` | — | Standalone log viewer application |
| `perf_tracker.rs` | — | Non-invasive render-timing spans |
| `gui_test_actions.rs` | — | MCP-driven GUI automation actions |
| `mcp_server.rs` | — | JSON-RPC 2.0 diagnostic server |
| `notification/` | 7 files | Desktop notification event emission, platform backends, focus tracking, settings, rendering |

### Core library files affecting chat UI state (`src/`)

| File | Role |
|------|------|
| `chat_core.rs` | `Message` enum (Text, ImageShare, FileShare, FileRequest, UserProfile, Presence, Heartbeat, SystemAnnouncement, etc.), `AppState`, `handle_net_event()`, `TransferProgress` |
| `chat_callbacks.rs` | `ChatCallbacks` trait — implemented by `AppState` and `IcedChat` |
| `chat_history.rs` | Legacy JSON chat history (`ChatHistoryStore`) |
| `store.rs` | SQLite `MessageStore` — inbox/outbox tables, `outgoing_messages` for delivery tracking |
| `room.rs` | `RoomStore` — room topic persistence |
| `room_history.rs` | `RoomHistoryStore` — room list with last-active timestamps |
| `friends.rs` | `FriendsStore` — friend records, known addresses, room tickets |
| `conversations.rs` | `ConversationStore` — conversation metadata |
| `user_profile.rs` | `UserProfile` — display name, bio, avatar ticket |
| `whisper/mod.rs` | Whisper (DM) protocol: friend requests, conversation invites, private messaging |
| `inbox.rs` | Inbox protocol: offline message store-and-forward |
| `net.rs` | Gossip actor, address lookup, dial retry |
| `ui_events.rs` | `ConversationNetEvent` — scoped net events |
| `backfill.rs` | History backfill over QUIC ALPN |

### Test files (`tests/`)

46+ integration tests covering: message exchange, DHT chat, relay, file sharing, image send/download, friend requests, onboarding, outbox throughput, branding rename, deterministic harness, etc.

### Existing architecture docs

- `docs/gui-architecture.md` — existing architecture overview (complementary, not replaced)
- `docs/configuration.md`
- `docs/networking-audit.md`
- `docs/discovery-architecture.md`

---

## 2. Component Map: Chat Screen

### 2.1 Application Shell (`view()` at app.rs:14147)

```
┌──────────────────────────────────────────────────┐
│ Sidebar (300px)  │  Main Panel (Fill)            │
│                  │                               │
│ [Header + logo]  │  Screen-dependent content     │
│ [Identity block] │  (ChatList/Chat/Settings/...) │
│ [CHATS ▼]        │                               │
│ [FRIENDS ▼]      │                               │
│ [DISCOVER ▼]     │                               │
│ [PUBLIC ROOMS ▼] │                               │
│ [REQUESTS ▼]     │                               │
└──────────────────────────────────────────────────┘
```

Overlays (rendered on top of base layout):
- `view_connection_details_dialog()` — connection info popover
- `view_create_room_dialog()` — new room dialog
- `view_sidebar_add_menu()` — slide-over Add menu (Add Friend, Join Ticket, Import Friend)
- `view_image_lightbox()` — full-screen image viewer (toggle 200px ↔ 480px)

### 2.2 Left Sidebar (`view_sidebar()` at app.rs:14621)

A scrollable Column (300px width) with collapsible sections:

1. **Header row** — Boru logo + "+" button (ICON_PLUS → `ToggleAddMenu`)
2. **Identity row** — Avatar (initials circle or profile image), display name, online status dot, settings gear (ICON_SETTINGS → `OpenSettings`)
3. **CHATS section** (`view_sidebar_chats`) — Conversation list with unread counts, touch+bump ordering, delete confirm
4. **FRIENDS section** (`view_sidebar_friends`) — Friend rows with online dot, profile image, open-chat button. Sorted: online first, then alphabetical
5. **DISCOVER section** (`view_sidebar_discovered_peers`) — Gossip-discovered peers, filtered to online-only
6. **PUBLIC ROOMS section** (`view_sidebar_public_rooms`) — Relay-advertised public room directory entries
7. **REQUESTS section** (`view_sidebar_requests`) — Incoming/outgoing friend request count

Each section caches its dependency to avoid rebuilding on every frame. Revision counters (`chats_sidebar_revision`, `friends_sidebar_revision`, etc.) trigger cache invalidation.

### 2.3 Conversation Header (`view_chat_header()` at app.rs:16591)

```
[← Back]  [Room Name / Peer Display Name]           [⋯ Options]
```

- Back button: `← Back` at TYPO_SM, BUTTON_GHOST_BG style, padding [SPACE_6, SPACE_12]
- Room name: derived from `self.ticket_str` or peer display name
- Options button (⋯): toggles `view_chat_options_popover()` — room info, delete chat, advertise toggle

### 2.4 Message List (`view_chat_log()` at app.rs:16843)

A `Scrollable` column with `anchor_bottom()` for auto-follow-latest behavior.

Each entry rendered as a `ChatEntry` widget:
- **System messages** — centered, muted, small text (no avatar, no bubble)
- **Local messages** — right-aligned, green bubble, delivery state label (Sending/Sent/Delivered/Read/Failed), no avatar
- **Remote messages** — left-aligned, avatar circle (initials or profile image), sender name label (Semibold), white/grey bubble, timestamp

**Date dividers** inserted when day changes between messages (Today/Yesterday/Day-of-week).

**Per-entry features:**
- Message text with URL detection → link preview cards (async fetch)
- Image inline rendering (cached `image_handle`, toggle 200px ↔ 480px on click)
- File download attachments with progress bars (Active/Completed states)
- Reactions display (emoji row below bubble)
- Edited indicator
- Timestamp (HH:MM for today, "Mon HH:MM" for this week, "Jan 5" for older)
- Delivery state for local messages (Queued→Sending, Sent→Sent, Delivered→Delivered, Seen→Read, Failed→Failed)

**Message grouping:** `continues_message_group()` in `presentation.rs` determines whether two adjacent entries share sender/avatar treatment (same sender within 5-minute window). System messages always break groups.

### 2.5 Composer (`view_composer()` at app.rs:17424)

```
[Attach 📎] [________________text input________________] [Send →]
```

- Text input: `text_input` with `on_input(InputChanged)`, `on_submit(SendPressed)`
- Send button: primary accent button, disabled when text is empty
- Attach button: opens native file picker → routes to `ExecuteImageSend` for images, `ExecuteFileSend` for other files

Upload feedback: animated placeholder card in chat log for image/file uploads in progress.

### 2.6 Search

No dedicated search screen. Search exists as:
- Friend request search (`FriendRequestSearchChanged` — filters by peer key prefix)
- No full-text message search implemented yet

### 2.7 Shared Files

Accessible via `PeerProfile` screen and `PeerCatalogue` screen (remote catalogue browsing). Local file library managed in `file_library.rs` / `file_library_ops.rs`.

---

## 3. State and Event Flows

### 3.1 Select a Conversation

```
User clicks sidebar conversation row
  → AppMessage::RoomSelected(topic)
  → update(): set self.pending_topic = Some(topic)
  → AppMessage::OpenRoom(topic)
  → update(): guard (topic != self.topic || self.sender.is_none())
  → leave_current_room() — clear neighbors, abort forwarder
  → iced::Task::perform(async { gossip.subscribe(topic, bootstrap_peers) ... })
  → AppMessage::RoomOpened { topic, ticket, sender, neighbor_count, neighbor_ids }
  → update(): switch screen to Screen::Chat { topic }
  → Load/replay chat history entries
  → Set self.sender = Some(sender), self.sender_ready = true
  → Emit retroactive NeighborUp events for neighbor_ids
  → Check for pending events, process delivery state
```

### 3.2 Send a Message

```
User types text → InputChanged(text)
  → update(): self.composer_text = text; set typing flag (removed)

User presses Enter or clicks Send → SendPressed
  → update():
      if composer_text.is_empty() → return
      let body = composer_text.take()
      let event_id = chat_history.next_event_id()
      push local ChatEntry with DeliveryState::Queued
      assign event_id, update event_id_to_index
      sender.broadcast(Message::Message { text: body })
      → spawn async task: MessageSent(body, event_id, hash)
  → MessageSent handler:
      update delivery_state to Sent
      persist to chat_history
  → DeliveryState updated on delivery receipt / ack
```

### 3.3 Receive a Message

```
Gossip mesh delivers Message bytes
  → forwarder task receives Event::GossipMessage
  → deserialize via SignedMessage::verify_and_decode (postcard)
  → handle_net_event(msg, from, content_hash)
      → self-message filter: from != cb.local_public()
      → cb.push_message(body, from, content_hash, timestamp)
      → cb.set_delivery_state(event_id, Delivered) for matching self-sent hash
  → Forwarded as NetEvent → ConversationNetEvent
  → app.rs: NetEvent(ConversationNetEvent::Message { ... })
  → update(): push remote ChatEntry
  → If conversation not selected: buffer in pending_events, increment unread
  → If conversation selected: display immediately, scroll to bottom
```

### 3.4 Attach a File

```
User clicks Attach → native file dialog
  → Image detected (.png/.jpg/.jpeg/.gif/.webp/.bmp):
      → ExecuteImageSend(path)
      → tokio::fs::read → blob_store.add_path → broadcast ImageShare { name, hash }
      → Pending image upload spinner shown in chat log
  → Other file detected:
      → ExecuteFileSend(path)
      → Stream upload, track progress → broadcast FileShare message
      → File upload feedback card in chat log
```

### 3.5 Receive an Image

```
Receiver gets Message::ImageShare { name, hash }
  → cb.set_pending_image(name, hash, from)
  → Next event loop tick: pending_image.take()
  → blob_store.downloader().download(hash) → read bytes
  → ImageDownloaded { sender, name, display_name, image_bytes, message_hash, image_identifier }
  → update(): ChatEntry::image(name, image_bytes, ...) — decodes handle once, caches
  → Rendered inline in view_chat_log() via cached image_handle
```

### 3.6 Scroll and Load History

```
User scrolls chat log → Scrolled(offset, viewport_height)
  → update(): self.scroll_offset = offset, self.viewport_height = viewport_height
  → Windowed rendering: view_chat_log computes visible range from offset/viewport
  → History replay: entries already loaded in self.entries (in-memory)
  → ChatHistoryStore provides persistent history on room open
  → HistoryBackfill protocol (QUIC ALPN) for fetching missed messages from peers
```

### 3.7 Opening Search / Shared Files

No global search exists. Shared files are accessed through peer profiles:
- `OpenPeerProfile(pk)` → `Screen::PeerProfile(pk)` → shows shared files with Download buttons
- `BrowsePeerCatalogue(pk)` → `Screen::PeerCatalogue(pk)` → remote file catalogue with download requests
- Local file library: accessible via settings or direct navigation

---

## 4. UI State Provenance

### From Real Application State

| UI Element | Source Field | Mechanism |
|------------|-------------|-----------|
| Message text | `ChatEntry.body` | Populated from gossip `Message::Message.text` or `Message::FileShare`/`ImageShare` |
| Sender name | `self.names[peer]` or `presentation::initials()` | Updated via `AboutMe` gossip messages |
| Sender avatar | `ChatEntry.avatar_handle` | Downloaded from `UserProfile.avatar_ticket` blob |
| Message timestamp | `ChatEntry.timestamp` | `sent_at` from protocol, or `Utc::now()` for local |
| Delivery state | `ChatEntry.delivery_state` | `DeliveryState` enum — Queued→Sent→Delivered→Seen |
| Online status | `self.friend_online_cache` | Updated on NeighborUp/NeighborDown |
| Connection status | `self.mesh_health`, `self.neighbors` | `MeshHealth` from quiescence watchdog |
| Direct vs relay peers | `self.direct_peers`, `self.relayed_peers` | Updated via `ConnMonitorTick` → `ConnCountsResult` |
| Unread count | `conversation.unread` | Incremented on NetEvent when conversation not selected |
| Room name / ticket | `conversation.ticket_str` | Set on RoomOpened |
| File download progress | `DownloadAttachment` on `ChatEntry` | `TransferProgress::Progress` events |
| Link preview | `ChatEntry.link_preview` | Async HTTP fetch + metadata extraction |
| Discovered peers | `self.neighbors` ∩ gossip-discovered | mDNS + DHT + relay gossip |
| Public rooms | Directory gossip topic | `DirectoryRoomUpdate` events |
| Friend requests | `self.friend_request_store` | Whisper protocol incoming/outgoing |
| Shared file catalogue | Remote `CatalogueHandler` | QUIC ALPN catalogue protocol |

### Information NOT Currently Available to the Chat UI

| Desired Information | Status |
|--------------------|--------|
| Per-message latency / ping time | Not measured or stored |
| Peer ISP / geolocation | Not available |
| Encryption protocol details (key exchange, cipher suite) | Not exposed to UI layer |
| Message read receipts for remote messages | Only local delivery state tracked |
| Typing indicators | Removed (July 2026) |
| Voice/video call state | Not implemented |
| Network quality score (jitter, packet loss) | Not measured |
| Per-conversation data usage | Not tracked |
| Message edit history | Single `edited` flag only |
| Reaction sender details | Only aggregated emoji counts |

---

## 5. Build and Test Results

### Build

```
Command: cargo build --example boru --features gui
Result:  SUCCESS (15.57s, debug profile)
Warnings: 88 (all pre-existing)
```

Pre-existing warning categories (all benign, not blocking):
- Dead code in notification/service.rs (unused `WindowFocusState`, `Notifier` methods) — notification system partially wired
- Unused functions in `presentation.rs` (`initials`, `initials_color`, `format_last_seen`, `count_label`) — retained as library utilities
- Dead code in `perf_tracker.rs` (unused `reset()`) — performance tracking utility
- Unfulfilled `#[expect(dead_code)]` on AppMessage enum — the attribute is on the enum itself, but individual variants are used

### Test Suite

```
Command: cargo test --lib
Result:  All completed tests PASSED (~140 unit tests across src/)
Pre-existing hang: 2 outbox_delivery tests hang (>60s) — test_different_peers_deliver_concurrently, test_same_peer_serialized
  These are pre-existing issues unrelated to UI changes; they involve async delivery synchronization
```

Integration tests (46+ files under `tests/`) require network-capable test environment and were not run. They are excluded from the baseline scope.

---

## 6. Functionality That Must Not Be Disturbed

Per the safety preamble constraints:

1. **Networking**: Gossip subscriptions, peer discovery (mDNS/DHT), address lookup, dial retry, QUIC connections
2. **Storage**: SQLite `boru.db` (V10), JSON stores (friends, rooms, conversations, chat_history, outbox, mailbox), ImageStore
3. **Protocols**: Message serialization (postcard + ed25519 signing), whisper (friend requests/DMs), inbox (offline messages), backfill, catalogue, file access
4. **Identity**: PublicKey/SecretKey handling, friend ID derivation, direct topic derivation
5. **File transfer**: Blob upload/download, image optimization, content addressing
6. **Every currently working control**: Send, attach, back navigation, sidebar sections, room creation, join ticket, friend add/remove, settings toggles, dark mode, delete chat, image lightbox, file downloads, copy to clipboard
7. **No invented information**: Connection quality, encryption details, latency, delivery guarantees — only show what real application state provides

---

## 7. Iced 0.14 Widget Patterns in Use

| Pattern | Location | Notes |
|---------|----------|-------|
| `scrollable(...).anchor_bottom()` | `view_chat_log` | Auto-follow latest message |
| `button(text(...)).on_press(...)` | Sidebar rows, header buttons | Clickable text via button wrapper |
| `container(...).style(custom_fn)` | Throughout | Theme-aware background colors |
| `column![...].spacing(n).align_x(...)` | All views | Layout composition |
| `text(...).size(n).color(...)` | Message bubbles, labels | Sized at `TYPO_SM` (13), `TYPO_BODY` (15) |
| `image(handle).content_fit(...).width(...).height(...)` | Inline images | ScaleDown for thumbnails, Contain for lightbox |
| `lazy` widget | Sidebar sections | Cached per-section dependency keys |
| `Stack::new().push(...)` | Overlays | Chat options popover, add menu |
| `text_input(...).on_input(...).on_submit(...)` | Composer, search fields | Text entry |
| `rfd::AsyncFileDialog` | Attach button | Native file picker |

---

## 8. Design Token Summary

Palette: PRIMARY (#2F6B4F green), APP_BACKGROUND (#F4F6F4), SURFACE (white/grey), TEXT (#202522), ONLINE (#28A45D), DESTRUCTIVE (#B64141)

Spacing scale: SPACE_4 (4px), SPACE_8, SPACE_12, SPACE_16, SPACE_24, SPACE_32

Radius: RADIUS_SM (8px), RADIUS_MD (10px), RADIUS_LG (12px), RADIUS_XL (14px)

Typography: TYPO_XS (10px), TYPO_SM (13px), TYPO_BODY (15px), TYPO_H3 (16px), TYPO_H2 (20px), TYPO_H1 (24px)

Button styles: BUTTON_CARD, BUTTON_PRIMARY, BUTTON_PRIMARY_GREEN, BUTTON_OUTLINE, BUTTON_GHOST, BUTTON_GHOST_BG, BUTTON_ICON, BUTTON_DANGER, BUTTON_MUTED

Icon constants: ICON_CHAT, ICON_FRIEND, ICON_FILES, ICON_RETRY, ICON_SETTINGS, ICON_CLOSE, ICON_PLUS, ICON_SEARCH, ICON_MORE, ICON_ACTIVITY, ICON_NOTIFICATION, ICON_ONLINE, ICON_OFFLINE

Color functions (theme-aware): `primary()`, `surface()`, `text()`, `text_secondary()`, `text_muted()`, `border()`, `online()`, `bg_surface()`, `bg_hover()`, etc.
