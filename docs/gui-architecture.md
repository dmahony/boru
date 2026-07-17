# GUI Architecture — iced_chat

The iced_chat GUI is a [Iced](https://iced.rs/) (v0.14) desktop application providing
a conversation-first chat interface — like Telegram or Signal — with support for
rooms, direct messaging, file sharing, and peer discovery.

## Architecture

### Main Entry Point (`main.rs`)

`main.rs` is the CLI entry point that:

1. Parses CLI arguments via `clap` (see `docs/configuration.md`)
2. Initializes logging (file + terminal, dual-level filtering)
3. Creates a tokio runtime
4. Sets up the iroh endpoint, gossip actor, and all protocol handlers
5. Passes a `NetEvent` receiver to the Iced application
6. Starts the Iced event loop

### Application State (`app.rs`)

The `IcedChat` struct (~16k lines) implements `iced::Application` and manages:

- **Screens**: Chat list (inbox), individual chat rooms, file picker, settings
- **Network state**: Gossip subscriptions, inbox/backfill/whisper handles
- **Storage**: JSON stores for friends, chats, conversations, outbox, mailbox, rooms
- **Discovery**: mDNS, DHT (public rooms, private rooms)
- **File operations**: Local file library, image optimization, file transfers

Key subsystems within `app.rs`:

| Subsystem | Description |
|-----------|-------------|
| Room management | Open/close/join rooms, topic persistence |
| Conversation management | Unread counts, mute/archive/delete, conversation metadata |
| Message pipeline | Send → gossip → receive pipeline with delivery receipts |
| Friend management | Friend requests, accept/reject, friendship lifecycle |
| File sharing | Attach files to messages, optimize images, track downloads |
| Discovery | mDNS peer discovery, DHT publish/discover, address lookup |
| Peer overlay | Remote peer profiles (display names, shared files) |
| Diagnostics | IcedMessageJournal, GuiActionHistory, failure analysis |

### File Library (`file_library.rs`, `file_library_ops.rs`)

The file library manages files the local user offers to peers. It provides:

- Two storage modes: **Import** (copy into managed store) and **Reference** (point to original)
- Content-addressed BLAKE3 hashing with streaming (64 KiB chunks)
- Progress reporting and cancellation for large files
- Filtering and sorting (All, Available, Missing, Changed, Disabled, Imported, Referenced)
- Metadata editing (display name, description, collections)
- Change detection for referenced files
- Startup recovery (stale temp cleanup, orphan detection)
- Privacy protections (path sanitization, remote-safe verification)

### MCP Diagnostic Server (`mcp_server.rs`)

A JSON-RPC 2.0 server over TCP (loopback by default) providing:

- Health checks, node status, room status
- Diagnostic events and probes through the gossip mesh
- Peer state tracking and discovery orchestration
- GUI state snapshots and test actions (with `--enable-gui-test-actions`)
- Composer manipulation, navigation, and dark mode toggling
- Message pipeline verification

### Log Viewer (`log_viewer.rs`)

A standalone Iced application for viewing the persistent log file. Launched with
`iced_chat logs`.

### Performance Tracker (`perf_tracker.rs`)

Non-invasive timing spans recorded via `tracing::info!` and accumulated
in-memory. Enabled with `--perf` CLI flag. Prints a baseline summary at exit.

### GUI Test Actions (`gui_test_actions.rs`)

Automated actions for integration testing, exposed via the MCP server. Includes:
- Open room, join lobby, send messages, navigate screens, toggle dark mode
- Full message pipeline verification
- GUI state snapshots and wait-for-state notifications

## Screen Structure

```
┌──────────────────────────────────────────────┐
│  Chat List (Inbox)                           │
│  ┌────────────────────────────────────────┐  │
│  │ Room/Conversation 1       [2 unread]   │  │
│  │ Room/Conversation 2                    │  │
│  │ Friend DM — Alice       [1 unread]     │  │
│  │ ...                                    │  │
│  └────────────────────────────────────────┘  │
│  [New Chat] [Join Room] [Logs]              │
├──────────────────────────────────────────────┤
│  Chat Room / Conversation                    │
│  ┌────────────────────────────────────────┐  │
│  │ ┌──────┐                              │  │
│  │ │ Alice│ Hello everyone!              │  │
│  │ └──────┘                      10:30   │  │
│  │ ┌──────┐                              │  │
│  │ │  You │ Hey Alice!                   │  │
│  │ └──────┘                      10:31   │  │
│  │ ...                                    │  │
│  └────────────────────────────────────────┘  │
│  [Message input...                   [Send]] │
├──────────────────────────────────────────────┤
│  Profile / File Library                      │
│  ┌────────────────────────────────────────┐  │
│  │ Shared Files                          │  │
│  │ [Import] [Offer Reference] [Filter ▼] │  │
│  │ ┌──────────────────────────────────┐  │  │
│  │ │ photo.jpg   Available            │  │  │
│  │ │ doc.pdf     Missing              │  │  │
│  │ └──────────────────────────────────┘  │  │
│  └────────────────────────────────────────┘  │
└──────────────────────────────────────────────┘
```

## Networking Integration

The GUI connects to the networking layer via:

1. **`NetEvent` channel** — mpsc receiver for incoming events (messages, friend requests, discovery)
2. **Handle objects** — `GossipTopic`, `InboxHandle`, `BackfillHandle`, `WhisperHandle`
3. **`ChatCallbacks` trait** — typed callbacks for network events, transfers, and friend pings
4. **Tokio tasks** — spawned for inbox listening, backfill serving, continuous discovery

## State Persistence

The GUI uses a mix of persistence backends:

| Store | Backend | Purpose |
|-------|---------|---------|
| `Storage` (SQLite) | `boru.db` | Inbox/outbox, file objects, file library, operations progress |
| `ChatHistoryStore` (JSON) | `chat_history.json` | Per-room chat history (active frontend) |
| `OutboxStore` (JSON) | `outbox.json` | Outgoing message delivery state (active frontend) |
| `ConversationStore` (JSON) | `conversations.json` | Conversation metadata |
| `FriendsStore` (JSON) | `friends.json` | Friend contact list |
| `FriendRequestStore` (JSON) | `friend_requests.json` | Friend request state |
| `MailboxStore` (JSON) | `mailbox.json` | Encrypted offline envelopes |
| `RoomStore` (JSON) | `rooms.json` | Room topic registry |
| `UserProfile` (JSON) | `profile.json` | Display name, sharing settings |
| `ImageStore` (disk) | `files/` | Content-addressed images |

## Key Design Decisions

1. **Conversation-first UI** — The chat list shows all conversations (rooms + DMs)
   with unread counts, like Telegram/Signal. Room switching is dynamic.

2. **Dual logging** — High-volume discovery/debug messages go to file only;
   terminal shows a filtered subset. Prevents terminal noise while preserving
   debug data for post-hoc analysis.

3. **MCP over TCP** — The diagnostic MCP server binds to loopback by default,
   exposing JSON-RPC 2.0 for AI-agent integration. No authentication (assumes
   loopback security).

4. **mDNS + DHT** — LAN discovery uses mDNS; WAN discovery uses Mainline DHT
   through the public-room and private-room tracker systems.
