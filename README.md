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
- **File sharing** — signed, per-requester file catalogues, explicit access
  authorisation, and content-addressed iroh-blobs transfers
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
├── mailbox.json            # Legacy mailbox migration input (not written)
├── settings.json           # UI / app preferences
├── profile.json             # Profile settings + shared file metadata
├── secret_key.txt          # Node identity secret key
├── message_store.db        # Legacy SQLite (migration source, read-only)
└── files/                  # Per-user image store
    └── <user-hash>/<content-hash>.<ext>
```

### Storage Layers

| Layer | Store | Backend | Purpose |
|---|---|---|---|---|
| **Primary relational** | `Storage` (SQLite) | `boru.db` | Inbox, outbox, contacts, file objects, attachments, profiles (V4 schema) — primary storage |
| **Delivery state** | `DeliveryState` | — | Formal FSM for outbound message lifecycle (8 states, enforced transitions) |
| **Chat history** | `ChatHistoryStore` | `chat_history.json` | Per-room chat message history (JSON, still active in GUI) |
| **Outgoing queue** | `OutboxStore` | `outbox.json` | Delivery state tracking (JSON, still active in GUI) |
| **Conversations** | `ConversationStore` | `conversations.json` | Conversation metadata (JSON) |
| **Friends** | `FriendsStore` | `friends.json` | Friend list (JSON) |
| **Friend requests** | `FriendRequestStore` | `friend_requests.json` | Pending/accepted/declined requests (JSON) |
| **Mailbox protocol** | `MailboxEnvelope` / `MailboxAck` | — | Encrypted X25519 + AES-256-GCM envelopes and signed acknowledgements |
| **Legacy mailbox reader** | `MailboxStore` | `mailbox.json` | Read-only migration compatibility; new writes use SQLite `Storage` |
| **Room history** | `RoomHistoryStore` | `rooms.json` | Topic registry (JSON) |
| **User profile** | `UserProfile` | `profile.json` | Display name, sharing settings (JSON) |
| **Images** | `ImageStore` | `files/` | Content-addressed user-uploaded images |

### Key Design Properties

- **Exactly-once local persistence** — `INSERT … ON CONFLICT DO NOTHING`
  prevents duplicate message storage at the SQLite level.
- **At-least-once transport** — outbox entries survive crashes, retry with
  exponential backoff, and ACK-based dedup at the recipient. See
  [`docs/offline-direct-messaging.md`](docs/offline-direct-messaging.md) for
  the full delivery state machine, retry policy, ack semantics, and
  ordering guarantees.
- **Authorised senders only** — inbox protocol rejects connections and
  envelopes from peers not in the contact list (`allowed_senders`).
- **Encrypted mailbox** — offline DM envelopes use X25519 ECDH + AES-256-GCM
  with ephemeral sender keys and sender-ed25519 authentication.
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
| 3 | `file_verification` (per-file availability tracking) |
| 4 | `file_replacements`, `cleanup_operations`, `operation_progress`, `revision` column on `shared_files` |

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
| `metrics` | iroh-metrics instrumentation — enabled by default |
| `gui` | Iced GUI example with image optimization (implies `net`) |
| `simulator` | Deterministic simulation framework (implies `test-utils`) |
| `test-utils` | Test helpers (deterministic RNG, humantime-serde) |
| `examples` | Setup example (implies `net`) |

## Additional Documentation

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — overall system architecture
- [`docs/offline-direct-messaging.md`](docs/offline-direct-messaging.md) — delivery state machine, retry policy, ack semantics, ordering, authorisation, sync
- [`docs/protocol-layers.md`](docs/protocol-layers.md) — protocol details (gossip, inbox, backfill, whisper)
- [`docs/gui-architecture.md`](docs/gui-architecture.md) — iced_chat GUI architecture
- [`docs/configuration.md`](docs/configuration.md) — CLI flags, env vars, settings
- [`docs/build-release.md`](docs/build-release.md) — build, features, release process
- [`docs/testing.md`](docs/testing.md) — test infrastructure and patterns
- [`docs/message-storage-design.md`](docs/message-storage-design.md) — storage architecture and SQLite schema
- [`docs/storage-redesign.md`](docs/storage-redesign.md) — storage redesign progress

## Remote file sharing at a glance

Remote file sharing is deliberately a two-stage operation:

1. A peer receives a **notification**, not file bytes. The notification names
   the owner's catalogue revision. The requester then retrieves a signed
   catalogue over `/iroh-chat-catalogue/1`; notifications are coalesced and the
   latest verified revision is cached per peer. There is no continuous polling.
2. A catalogue contains only safe metadata (BLAKE3 hash, display name, size,
   MIME type, collection name, and file revision). It contains no local paths,
   database IDs, blob tickets, or unrestricted addresses. The owner builds it
   separately for the authenticated requester: blocked peers are denied,
   confirmed friends see enabled offers by default, and non-friends require an
   explicit `read` grant. Unavailable and disabled files are omitted.
3. To retrieve a file, the requester sends the catalogue hash and revision to
   `/iroh-chat-transfer-auth/1`. The owner re-checks visibility, block state,
   offer state, availability, hash, and version. A successful response is a
   short-lived, requester-bound, Ed25519-signed download descriptor. The
   descriptor authorises an iroh-blobs transfer; it is not a permanent ticket.
4. The download streams through iroh-blobs to a temporary file, verifies the
   expected size and BLAKE3 hash, and atomically renames the file into place.
   Cancellation removes the temporary file. Previously verified chunks may be
   reused by iroh-blobs, but the destination file is re-streamed on resume;
   this is not byte-range resume of the temporary file.

Sharing is contact-oriented, not public by default. Public catalogue/file
sharing is deferred. See [`ARCHITECTURE.md`](ARCHITECTURE.md#remote-file-catalogue)
and [`docs/protocol-layers.md`](docs/protocol-layers.md#remote-file-catalogue)
for the complete protocol and security model.
