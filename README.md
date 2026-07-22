# boru-core

Private, peer-to-peer communication built on
[iroh](https://github.com/n0-computer/iroh).

> Private communication, directly between people.


- **Gossip protocol** — room-based message broadcasting over QUIC
- **Direct messaging** — inbox protocol for offline delivery, whisper protocol
  for private 1:1 channels
- **Backfill** — late-joining peers can request missed messages from existing
  peers
- **Friend management** — signed contact and friend-request negotiation
- **File sharing** — content-addressed file attachments, profile-offered files
  with signed, requester-filtered catalogues and per-peer permissions
- **Relational storage** — SQLite-based persistence with managed migrations

## Storage

All persistent data lives under a single data directory, resolved in this order:

1. `--data-dir` CLI flag
2. `BORU_CHAT_DATA_DIR` environment variable
3. `$XDG_DATA_HOME/boru-chat` (typically `~/.local/share/boru-chat/`)
4. `$PWD/.boru-chat`
### File Layout

```text
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

## Remote file sharing

Profiles advertise shared-file metadata through signed, requester-specific
catalogue snapshots. A catalogue contains safe display metadata and a
monotonic revision; it never contains local filesystem paths, permission rows,
or a download capability. The client verifies the owner's signature and the
owner identity before caching the projection. `known_revision` can produce a
`NotModified` response, while a revision change during pagination requires a
restart. There is no continuous catalogue-polling worker.

Clicking download performs a fresh authorization request over
`/boru-file-access/1`. The owner re-checks the live relationship, grants,
offer, availability, expected hash, size, and version, then issues a
requester-bound signed descriptor that expires after 60 seconds. Cached
catalogue visibility does not authorize access.

Iroh-blobs transfers the bytes. The receiver writes temporary output and
verifies the exact size and BLAKE3 content hash before atomically installing
the file and recording completion. Pause/resume re-resolves the peer and
re-authorizes; it is not byte-range resume of the destination file. Queue,
concurrency, size, timeout, and hash-verification limits bound resource use.

See [`docs/remote-file-sharing.md`](docs/remote-file-sharing.md),
[`docs/security-model.md`](docs/security-model.md), and
[`docs/privacy-model.md`](docs/privacy-model.md) for the protocol workflow,
security properties, privacy guarantees, storage behavior, and manual tests.

## Discovery

Peers find each other through multiple layered discovery mechanisms.
The system separates **address resolution** (finding transport addresses for
a known peer) from **member discovery** (finding which peers are in a room).

### Address Resolution (How to dial a known peer)

| Source | Technology | Scope |
|--------|-----------|-------|
| Current | In-memory active connection | Node-local |
| Persisted | `FriendsStore.known_addrs` | Node-local |
| mDNS | LAN multicast | Local network |
| Configured | Bootstrap addresses | Node-local |
| Relay | iroh relay server | WAN |
| **DHT** | Mainline DHT / Pkarr | Global |
| TrustedPeer | Config file | Node-local |

Resolution priority: `Current → Persisted → Mdns → Configured → Relay → Dht → TrustedPeer`

- **mDNS** discovers peers on the local network automatically (always active).
- **DhtAddressLookup** resolves `EndpointId` to transport addresses on the
  global Mainline DHT using Pkarr-signed records. Gated by `--no-dht`.
- By default, only relay URLs are published (`--publish-direct-addresses`
  exposes direct IPs — use with caution for privacy).

### Member Discovery (Finding room peers)

- **Public rooms**: Deterministic identity derived from (network, room name,
  protocol version). Peers use `distributed-topic-tracker` to publish and
  discover each other on the DHT. Continuous background loops re-publish
  presence every 5 minutes and discover new peers every 30 seconds.
- **Private rooms**: Same DHT mechanism but with namespace isolation via a
  32-byte `DiscoverySecret`. Records are HPKE-encrypted so only members with
  the secret can read them. Discovery is gated by `--no-dht`.
- **Tickets**: Both room types support out-of-band invitation tickets that
  encode the room identity (topic + optional secret + bootstrap relay),
  bypassing DHT entirely.

### Wire Format

Discovery records are ~171-byte Ed25519-signed envelopes carrying a 33-byte
payload: version byte + 32-byte `EndpointId`. Private-room records are
HPKE-encrypted per-minute. The validation pipeline checks size, timestamp,
decoding, identity match, and signature — in that order, cheapest first.

### Privacy

| Setting | Implication |
|---------|-------------|
| Default (relay-only) | IP addresses never published to DHT |
| `--publish-direct-addresses` | Public IP published on Mainline DHT (faster P2P) |
| Private rooms (with secret) | DHT namespace is undetectable without the secret; records encrypted |

### DHT Outage Behaviour

- Existing connections and known addresses continue working.
- Exponential backoff on publish/discover failures (1s → 2s → 4s → 60s cap).
- mDNS and ticket-based joins unaffected.
- Once DHT recovers, normal operation resumes automatically.

See [`docs/discovery-architecture.md`](docs/discovery-architecture.md) for
the full architecture, namespace derivation, validation pipeline, DHT outage
fallback, and operator guidance.

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
