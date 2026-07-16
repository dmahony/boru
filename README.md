# boru-chat

Gossip messages over broadcast trees — a peer-to-peer chat application built on
[iroh](https://github.com/n0-computer/iroh).

## Architecture

boru-chat is a Rust library (`boru_chat`) and example GUI application
(`examples/iced_chat`) that provides:

- **Gossip protocol** — room-based message broadcasting over QUIC
- **Direct messaging** — inbox protocol for offline delivery, whisper protocol
  for private 1:1 channels
- **Backfill** — late-joining peers can request missed messages from existing
  peers
- **Friend management** — signed contact and friend-request negotiation
- **File sharing** — content-addressed file attachments, profile-offered files
  with per-peer permissions
- **Relational storage** — SQLite-based persistence with managed migrations

## Storage

All persistent data lives under a single data directory, resolved in this order:

1. `--data-dir` CLI flag
2. `BORU_CHAT_DATA_DIR` environment variable
3. `$XDG_DATA_HOME/boru-chat` (typically `~/.local/share/boru-chat/`)
4. `$PWD/.boru-chat`

### File Layout

```
<data_dir>/
├── boru.db                # SQLite: inbox, outbox, file objects, attachments
├── chat_history.json       # Per-room chat message history
├── outbox.json             # Outgoing message delivery state
├── conversations.json      # Conversation metadata
├── rooms.json              # Room topic registry
├── friends.json            # Friend contact list
├── friend_requests.json    # Friend request state
├── mailbox.json            # Encrypted offline message delivery
├── settings.json           # UI / app preferences
├── user_profile.json       # Profile settings + shared file metadata
├── secret_key.txt          # Node identity secret key
├── message_store.db        # Legacy SQLite (migration source, read-only)
└── files/                  # Per-user image store
    └── <user-hash>/<content-hash>.<ext>
```

### Storage Layers

| Layer | Store | Backend | Purpose |
|---|---|---|---|
| **Primary relational** | `Storage` (SQLite) | `boru.db` | Inbox/outbox, contacts, file objects, attachments, shared files, permissions, downloads |
| **Chat history** | `ChatHistoryStore` | `chat_history.json` | Per-room message history (JSON, still active in GUI) |
| **Outgoing queue** | `OutboxStore` | `outbox.json` | Delivery state tracking (JSON, still active in GUI) |
| **Conversations** | `ConversationStore` | `conversations.json` | Conversation metadata (JSON) |
| **Friends** | `FriendsStore` | `friends.json` | Friend list (JSON) |
| **Friend requests** | `FriendRequestStore` | `friend_requests.json` | Pending/accepted/declined requests (JSON) |
| **Mailbox** | `MailboxStore` | `mailbox.json` | Encrypted offline-message envelopes (JSON) |
| **Room history** | `RoomHistoryStore` | `rooms.json` | Topic registry (JSON) |
| **User profile** | `UserProfile` | `user_profile.json` | Display name, sharing settings (JSON) |
| **Images** | `ImageStore` | `files/` | Content-addressed user-uploaded images |

### Key Design Properties

- **Exactly-once local persistence** — `INSERT … ON CONFLICT DO NOTHING`
  prevents duplicate message storage at the SQLite level.
- **At-least-once transport** — outbox rows survive crashes (Sent→Pending
  recovery), retry with configurable backoff, and ACK-based dedup at the
  recipient.
- **WAL mode + integrity checks** — crash-safe writes, automatic corruption
  detection on open.
- **Forward-only migrations** — schema is tracked; opening a newer DB on an
  older binary is safely rejected.
- **Content-addressed attachments** — file objects keyed by blake3 hash for
  deduplication and integrity.
- **Plaintext at rest** — ciphertext blobs are stored unencrypted in SQLite;
  transport-layer encryption (QUIC/TLS 1.3) protects messages in flight.
- **Restrictive permissions** — data directory and database are `0o700`/`0o600`
  on Unix.

### Schema Versions

| Version | What's added |
|---|---|
| 1 | `inbox`, `outbox`, `contacts`, `sync_cursor` (message delivery) |
| 2 | `file_objects`, `message_attachments`, `shared_files`, `file_collections`, `file_collection_items`, `shared_file_permissions`, `downloads`, `profile_manifest_state` |

See [`docs/message-storage-design.md`](docs/message-storage-design.md) for
the full storage architecture.

## Running

```sh
# CLI chat (with auto-discovery)
cargo run --example iced_chat --features gui -- --name <nickname>

# With a specific data directory
BORU_CHAT_DATA_DIR=~/.boru-chat cargo run --example iced_chat --features gui -- --name <nickname>
```

## Features

| Feature | Description |
|---|---|
| `net` | Networking stack (gossip, inbox, backfill, whisper, discovery) — enabled by default |
| `gui` | Iced GUI example with image optimization |
| `sim` | Deterministic simulation test framework |
