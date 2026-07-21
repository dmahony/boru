# boru-chat Architecture

## Overview

boru-chat is a peer-to-peer chat application built on [iroh](https://github.com/n0-computer/iroh), a QUIC-based networking library. Messages are broadcast over gossip trees (PlumTree/HyParView), and direct messaging uses dedicated QUIC protocols for offline delivery and private 1:1 channels.

The project provides a Rust library (`boru_chat`) and an example GUI application (`examples/iced_chat`).

## High-Level Architecture

```
┌─────────────────────────────────────────────────────────┐
│                   Frontend (iced_chat)                    │
│  ┌─────────┐ ┌────────────┐ ┌────────┐ ┌─────────────┐  │
│  │ Chat UI  │ │ File Library│ │Log     │ │ MCP Server  │  │
│  │ (app.rs) │ │ (file_*.rs) │ │Viewer  │ │(diagnostic) │  │
│  └────┬─────┘ └──────┬─────┘ └───┬────┘ └──────┬──────┘  │
│       │              │           │              │         │
│       └──────────────┴───────────┴──────────────┘         │
│                         │                                 │
└─────────────────────────┼─────────────────────────────────┘
                          │ ChatCallbacks trait
                          ▼
┌─────────────────────────────────────────────────────────┐
│               Core Library (boru_chat)                    │
│                                                          │
│  ┌──────────────┐  ┌──────────┐  ┌──────────────────┐   │
│  │ chat_core    │  │   net    │  │  proto            │   │
│  │ (state       │  │ (gossip  │  │ (HyParView/      │   │
│  │  machine,    │  │  actor,  │  │  PlumTree state  │   │
│  │  protocol    │  │  ALPN)   │  │  machine, IO-less)│  │
│  │  types)      │  └────┬─────┘  └──────────────────┘   │
│  └──────┬───────┘       │                                │
│         │               │                                │
│         ▼               ▼                                │
│  ┌──────────────────────────────────────────────────┐    │
│  │        Network Protocols                          │    │
│  │  ┌────────┐ ┌───────┐ ┌───────┐ ┌───────────┐   │    │
│  │  │ Gossip  │ │ Inbox  │ │Backfill│ │ Whisper   │   │    │
│  │  │(broad-  │ │(offline│ │(history│ │ (private  │   │    │
│  │  │ cast)   │ │  DM)   │ │  sync) │ │  1:1 DM)  │   │    │
│  │  └────────┘ └───────┘ └───────┘ └───────────┘   │    │
│  └──────────────────────────────────────────────────┘    │
│                                                          │
│  ┌──────────────────────────────────────────────────┐    │
│  │        Storage Layer                              │    │
│  │  ┌──────────────┐  ┌──────────┐  ┌───────────┐  │    │
│  │  │ SQLite:      │  │ JSON:    │  │ ImageStore │  │    │
│  │  │ Storage      │  │ Friends, │  │ (files/)   │  │    │
│  │  │ (boru.db)    │  │ Chats,   │  └───────────┘  │    │
│  │  │              │  │ Outbox,  │                 │    │
│  │  │              │  │ Profile  │                 │    │
│  │  └──────────────┘  └──────────┘                 │    │
│  └──────────────────────────────────────────────────┘    │
│                                                          │
│  ┌──────────────────────────────────────────────────┐    │
│  │        Discovery System                           │    │
│  │  ┌──────────────┐  ┌────────────┐  ┌──────────┐ │    │
│  │  │ Public Room  │  │ Private    │  │ DHT /    │ │    │
│  │  │ Trackers     │  │ Room       │  │ mDNS     │ │    │
│  │  │ (continuous) │  │ Trackers   │  │ Discovery│ │    │
│  │  └──────────────┘  └────────────┘  └──────────┘ │    │
│  └──────────────────────────────────────────────────┘    │
│                                                          │
│  ┌──────────────┐  ┌──────────┐  ┌────────────────┐     │
│  │ Diagnostics  │  │ Perf     │  │ Tor Transport  │     │
│  │ (events,     │  │(timing,  │  │ (.onion addr,  │     │
│  │  probes)     │  │ sampling)│  │  custom trans- │     │
│  └──────────────┘  └──────────┘  │  port)         │     │
│                                  └────────────────┘     │
└─────────────────────────────────────────────────────────┘
```

## Library Structure (`src/`)

### Core Protocol (`src/proto.rs`)

The IO-less protocol state machine implementing HyParView (membership) and PlumTree (gossip broadcast). See `docs/protocol-layers.md` for details.

| Module | Purpose |
|--------|---------|
| `proto::hyparview` | Swarm membership: active/passive peer sets, join/forward-join/shuffle |
| `proto::plumtree` | Broadcast: eager/lazy peer sets, gossip message propagation |
| `proto::state` | Combined protocol state machine (HyParView + PlumTree) |
| `proto::topic` | Topic-scoped protocol instance management |
| `proto::sim` | Deterministic simulation for protocol testing |
| `proto::util` | Peer selection, shuffling utilities |

### Network Layer (`src/net.rs`)

Wraps the IO-less protocol state machine in a tokio-based actor that manages QUIC connections, serializes/deserializes messages, and emits events to subscribers. The primary entry point is `Gossip::spawn()`.

### Chat Core (`src/chat_core.rs`)

Reusable state machine combining gossip networking with protocol message types (`Message`, `SignedMessage`, `Ticket`). Frontend-agnostic (no terminal/GUI dependencies). Used by both the iced GUI and headless tests.

### Public Rooms (`src/public_room*.rs`)

Deterministic public-room identities derived from (network, room name, protocol version). Supports DHT-based peer discovery and continuous publication.

| Module | Purpose |
|--------|---------|
| `public_room` | Public room identity: topic + discovery key derivation |
| `public_room_config` | Limits and defaults for DHT timing, message size, rate limits |
| `public_room_continuous` | Background publish/discover loop with jitter and backoff |
| `public_room_safety` | Per-peer rate limiting for untrusted public-room message flows |
| `public_room_tracker` | Publish-once / discover-once operations wrapping a discovery backend |

### Discovery System — Architecture

boru-chat uses **two independent DHT systems** for different purposes:

| Layer | Purpose | Crate | Type |
|---|---|---|---|
| **Address Resolution** | Resolve `EndpointId` → transport addrs (relay, IPs) | `iroh-mainline-address-lookup` | `DhtAddressLookup` |
| **Topic/Member Discovery** | Discover peer `EndpointId` values per room | `distributed-topic-tracker` | `MainlineDhtBackend` |

Each layer uses a **separate `mainline::Dht` instance** (distinct UDP sockets).
They serve different rate/timing profiles and should not be shared. The
address-resolution layer uses Pkarr-signed DNS packets on Mainline; the
topic-discovery layer uses the `distributed-topic-tracker` mutable-record
API with per-minute key rotation.

**Resolution priority** (for dialing a known peer):
```
Current → Persisted → Mdns → Configured → Relay → Dht → TrustedPeer
```

Member discovery splits into **public** and **private** subsystems:

| Subsystem | Tracker | Background Loop | Records Encrypted | CLI Gate |
|---|---|---|---|---|
| Public rooms | `PublicRoomTracker` | `ContinuousTracker` | No (deterministic key) | Always on |
| Private rooms | `PrivateRoomTracker` | `PrivateContinuousTracker` | Yes (HPKE, per-minute key) | `--no-dht` |

#### Discovery System Modules

| Module | Purpose |
|--------|---------|
| `discovery_backend` | `TopicDiscoveryBackend` trait + `MainlineDhtBackend` + `InMemoryDiscoveryBackend` (mock) |
| `discovery_record` | Wire-format signed discovery records; 33 B payload in ~171 B envelope |
| `discovery_secret` | 32-byte CSPRNG keys for private-room DHT isolation (V1); V2 subkey assessment |
| `discovery_validation` | 5-stage validation pipeline (size → timestamp → decode → identity → signature) with hard bounds |
| `public_room` | Deterministic public-room identity derivation (topic + discovery key) from (network, room name, version) |
| `public_room_config` | Limits and defaults for DHT timing, message size, rate limits |
| `public_room_tracker` | Publish-once / discover-once for public rooms |
| `public_room_continuous` | Background publish/discover loops with `PublicationPolicy`, exponential backoff, jitter |
| `public_room_safety` | Per-peer rate limiting for untrusted public-room message flows |
| `private_room_tracker` | Private-room DHT publish/discover with `DiscoverySecret`-based namespace isolation and HPKE encryption |

See [`docs/discovery-architecture.md`](docs/discovery-architecture.md) for the
full architecture: namespace derivation, wire format, validation pipeline,
privacy implications, record lifecycle, DHT outage fallback, limitations,
and operator guidance.

### Networking Protocols

| Module | ALPN | Purpose |
|--------|------|---------|
| `inbox` | `/iroh-chat-inbox/1` | Offline message delivery with ACK and delete-tombstone support |
| `backfill` | `/iroh-chat-backfill/1` | Late-joining peer history sync |
| `whisper` | `/iroh-chat-whisper/1` | Private 1:1 QUIC channels for DMs and file transfer |
| `net` (gossip) | `/iroh-gossip/1` | Room-based broadcast messaging |

### Friend & Contact System

| Module | Purpose |
|--------|---------|
| `contact` | Signed contact actions (friend request, accept, reject, conversation invite) |
| `friends` | Friends list with per-peer relationship state |
| `friend_request` | Pending/accepted/declined/cancelled friend requests |
| `chat_callbacks` | `ChatCallbacks` trait for frontend event notification |

### Storage Layer

| Module | Backend | Purpose |
|--------|---------|---------|
| `storage` | SQLite (`boru.db`) | Primary relational store: inbox, outbox, file objects, profiles (V4 schema) |
| `store` | SQLite (`message_store.db`, legacy) | Legacy store — migration source, read-only |
| `chat_history` | JSON | Per-room chat message history (active frontend) |
| `conversations` | JSON | Conversation metadata (unread, mute, archive) |
| `friends` | JSON | Friend contact list |
| `friend_request` | JSON | Pending/accepted/declined/cancelled friend requests |
| `mailbox` | Protocol types | Encrypted offline-message envelopes and signed ACKs; `mailbox.json` is legacy migration input |
| `outbox` | JSON | Outgoing message queue (active frontend) |
| `room` | JSON | Room topic + bootstrap peer persistence |
| `room_history` | In-memory | Transient room list for navigation |
| `user_profile` | JSON | Display name, sharing settings, shared file metadata |
| `image_store` | Disk (`files/`) | Content-addressed image storage |

### Image Processing

| Module | Feature | Purpose |
|--------|---------|---------|
| `image_store` | net | Secure local image storage with content-addressed paths |
| `image_optimizer` | gui | Sender-side resize + quality-retry JPEG compression |
| `compression` | (none) | Pure-Rust JPEG encode/resize |

### Diagnostics & Observability

| Module | Feature | Purpose |
|--------|---------|---------|
| `diagnostics` | (none) | Bounded event/probe storage with thread-safe query |
| `perf` | (none) | Performance timing with BORU_PERF env var |
| `gossip_debug` | net | Opt-in append-only gossip event log (BORU_DEBUG) |
| `observability` | (none) | Documentation-only: tracing guidelines and redaction rules |
| `metrics` | (none) | iroh-metrics integration |

### Other

| Module | Purpose |
|--------|---------|
| `api` | Public API for subscribing to topics and sending commands (local + RPC) |
| `dynamic_joiner` | Bounded dynamic peer joiner with dedup, backoff, retry |
| `file_indexer` | Shared folder scanner and filesystem change monitor |
| `retry` | Durable outbox retry worker |
| `room_cleanup` | Room history and metadata deletion helpers |
| `room_docs` | Room metadata and roster documents synced via gossip |
| `topic_derivation` | Deterministic topic derivation utilities |
| `tor_transport` | Tor .onion address scaffolding for custom transport |

## GUI Architecture (`examples/iced_chat/`)

The GUI is a single Iced application (the `IcedChat` struct in `app.rs`) with:

| Component | File | Purpose |
|-----------|------|---------|
| Main entry | `main.rs` | CLI args, endpoint setup, tokio runtime |
| Application | `app.rs` (~16k lines) | Iced Application, screens, networking, state |
| File library | `file_library.rs` | State types, filtering, sorting, validation |
| File library ops | `file_library_ops.rs` | Hashing, import, reference, change detection |
| Log viewer | `log_viewer.rs` | Standalone log file viewer |
| MCP server | `mcp_server.rs` | JSON-RPC 2.0 diagnostic server over TCP |
| Perf tracker | `perf_tracker.rs` | Non-invasive performance instrumentation |
| GUI test actions | `gui_test_actions.rs` | Automated test actions for integration testing |

## Build & Feature Flags

See `docs/build-release.md` for full documentation.

| Feature | Description | Default |
|---------|-------------|---------|
| `net` | Networking stack (gossip, inbox, backfill, whisper, discovery) | Yes |
| `metrics` | iroh-metrics integration | Yes |
| `gui` | Iced GUI + image optimization | No |
| `simulator` | Deterministic simulation binary | No |
| `test-utils` | Test helpers (chacha rng, humantime-serde) | No |
| `examples` | Setup example | No |

## Data Flow: Sending a Chat Message

```
User types message → IcedChat::update() →
  app.rs: Chat entry composed → SignedMessage created →
    gossip broadcast over PlumTree mesh →
      connected peers receive via GossipEvent →
        NetEvent delivered to IcedChat →
          local ChatHistoryStore persisted (JSON) →
            UI updated with new message
```

## Data Flow: Offline Direct Message

```
Sender                   Recipient
  │                          │
  │ MailboxStore.seal()      │
  │ (X25519 ECDH +           │
  │  AES-256-GCM AEAD)       │
  │                          │
  │ Outbox ← Queued          │
  │                          │
  │ InboxProtocol (QUIC      │
  │  /iroh-chat-inbox/1)     │
  │  Deliver(envelope) ───→ │
  │                          │ check allowed_senders
  │                          │ dedup by message_id
  │                          │ MailboxStore.enqueue()
  │                          │ decrypt → SignedMessage
  │ ←── Ack(MailboxAck) ────│
  │                          │
  │ Outbox → Acknowledged    │
  │ (end-to-end delivery)    │
```

The delivery state machine (8 states: Queued, Sending, SentAwaitingAck,
Acknowledged, RetryScheduled, FailedPermanent, Expired, Cancelled) enforces
valid transitions crash-robustly. Full details in
[`docs/offline-direct-messaging.md`](docs/offline-direct-messaging.md).

## Remote File Catalogue

Remote file sharing is split into catalogue retrieval, authorization, and
content transfer. A signed catalogue snapshot advertises safe metadata but
never grants access or carries file bytes. `known_revision` can produce
`NotModified`, and a revision change during pagination requires a restart. The
current implementation has no separate push-notification ALPN or continuous
catalogue-polling worker; callers trigger refresh explicitly or after observing
a profile revision change.

### Catalogue semantics

`SignedFileCatalogue` is an owner-signed snapshot containing the owner key, a
monotonic manifest revision, generation time, collections, and safe file
metadata. The signature covers every field. The remote projection excludes
source paths, usernames, internal IDs, permission rows, blob tickets, and
unrestricted addresses. A valid signature proves authorship and integrity, but
metadata remains untrusted input and is sanitized before UI or filesystem use.

Catalogues are built for the authenticated QUIC requester, never from a shared
cache. Blocked peers receive access denied. With no selected-peer grants,
enabled offers are visible to confirmed friends and other peers receive an
empty catalogue; when a file has explicit `read` grants, only granted peers
see it. Disabled offers and unavailable file objects are omitted. Two
requesters may therefore receive different catalogues for the same owner and
revision.

### Authorisation and transfer

The requester sends a shared-file ID plus expected content hash, size, and
version to `/boru-file-access/1`. The handler obtains identity from
`Connection::remote_id`, re-evaluates block/contact/permission state, checks
that the offer is enabled and the source has not changed, then prepares the
iroh-blobs object. A grant is a signed descriptor bound to owner and requester,
content hash, size, blob format, issue time, expiry, and a random nonce. The
default descriptor lifetime is 60 seconds; expired or replayed descriptors
must not be treated as access.

The transfer uses iroh-blobs' content-addressed store. Data is streamed in
256 KiB chunks to a temporary destination, then size and BLAKE3 are verified
before an atomic rename. Cancellation/pause removes the temporary output;
verified chunks retained by the blob store can be reused, but the output file
is re-streamed. If chunks were garbage-collected, the operation starts over.
There is no byte-range resume of the temporary file.

### Abuse limits and visibility

Catalogue requests are bounded to 256 KiB requests, 4 MiB responses, 10,000
files/1,000 collections, and a 10 TiB advertised file size. Pagination is
limited to 500 entries and 1 MiB per page. Catalogue and access handlers have
global and per-peer concurrency limits, bounded read/build/write deadlines,
rate limits, and preparation limits. File preparation additionally limits
global preparation and verification concurrency. See
[`docs/catalogue-limits.md`](docs/catalogue-limits.md).
Remote filenames are filesystem-sanitized before use; display text is sanitized
after signature verification.

Public sharing is not part of this design. Offers are contacts-only unless an
explicit grant exists; blocked peers cannot enumerate files or obtain access.
Unauthorised, missing, disabled, changed, unavailable, and rate-limited cases
are exposed only through bounded protocol error categories.

## Security Model

- **Transport**: All QUIC connections use TLS 1.3 (iroh's built-in encryption)
- **At-rest**: SQLite database and JSON files rely on filesystem permissions (0o600/0o700)
- **Message integrity**: `SignedMessage` wraps each payload with ed25519 signatures
- **Replay protection**: Timestamps with configurable clock-skew windows (default 24h)
- **Identity**: Ed25519 key pairs generated on first run, stored in `secret_key.txt`
- **Private rooms**: DHT namespace derived from BLAKE3(topic || secret) — 32-byte random key
- **Inbox authorisation**: Inbox protocol rejects connections from peers not in
  the `allowed_senders` set (populated from the contact/friend list)
- **Mailbox encryption**: Offline DM envelopes use X25519 ECDH + AES-256-GCM AEAD
  with ephemeral sender keys; each envelope has a fresh ephemeral key
- **All-or-nothing ack**: Outbox advances to `Acknowledged` only after verifying
  the recipient's ed25519 signature over the `MailboxAck`
- **Delete tombstones**: `AuthorDeleteProof` is ed25519-signed by the original
  message author, proving authorisation to delete; the outer
  `SignedInboxMessage` authenticates the forwarder
- **Catalogue authenticity**: catalogues and download descriptors are
  Ed25519-signed; catalogues are checked against the authenticated connection
  identity and descriptors are bound to the requesting peer
- **Access re-check**: catalogue visibility is not authorisation; permissions,
  block state, offer state, availability, hash, and version are checked again
  immediately before transfer
- **Content verification**: completed iroh-blobs downloads must match expected
  size and BLAKE3 hash before atomic installation
- **Abuse resistance**: payload caps, pagination bounds, semaphores, timeouts,
  request rate limits, and upload preparation limits constrain untrusted peers

## Privacy Model

- **Plaintext at rest**: Ciphertext blobs are stored unencrypted in SQLite;
  transport-layer encryption (QUIC/TLS 1.3) protects messages in flight
- **No filesystem paths in network data**: `FileLibraryRow` never exposes full
  source paths; `display_filename` is user-chosen; `verify_row_safe_for_remote()`
  enforces this at the API boundary
- **No message content inspection**: `MailboxStore` never decrypts — it stores
  opaque ciphertext; the storage layer (`Storage`) never inspects `inbox.ciphertext`
- **Content-addressed paths**: User directories under `files/` are keyed by blake3
  hash of the user identifier, never the identifier itself
- **Contact-based access control**: Only explicitly friended peers can deliver
  offline DMs; the `allowed_senders` set prevents unsolicited messages
- **Catalogue minimisation**: remote metadata contains no source path, local
  username, database ID, blob ticket, or unrestricted address
- **Per-requester isolation**: catalogues are filtered for the authenticated
  requester and are not shared across peers
- **No public default**: public catalogue/file sharing is deferred; blocked
  peers cannot enumerate offers
