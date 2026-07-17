# Protocol Layers

boru-chat uses multiple distinct QUIC-based protocols, each with its own ALPN
(Application-Layer Protocol Negotiation) string and purpose.

## Protocol Overview

| Protocol | ALPN | Type | Purpose | Persistence |
|----------|------|------|---------|-------------|
| Gossip | `/iroh-gossip/1` | Broadcast | Room-based message broadcasting via PlumTree | None (transient) |
| Inbox | `/iroh-chat-inbox/1` | Direct | Offline message delivery, ACKs, deletion tombstones | InboxEvent emission |
| Backfill | `/iroh-chat-backfill/1` | Direct | Historical message sync for late-joining peers | Reads ChatHistoryStore |
| Whisper | `/iroh-chat-whisper/1` | Direct | Private 1:1 QUIC channels for DMs and file transfer | None (transient) |
| Friend Ping | `/iroh-chat-ping/1` | Direct | Connectivity checks between friends | None (transient) |
| Catalogue retrieval | `/iroh-chat-catalogue/1` | Direct | Signed, requester-filtered file catalogue retrieval | Per-peer verified cache |
| Transfer authorisation | `/iroh-chat-transfer-auth/1` | Direct | Request-time permission check and signed blob descriptor | None (short-lived descriptor) |
| Blob transfer | iroh-blobs | Direct | Content-addressed file transfer | iroh-blobs store + download state |

## Gossip Protocol (`/iroh-gossip/1`)

### Architecture

The gossip protocol combines two algorithms:

1. **HyParView** — Swarm membership protocol. Each peer maintains:
   - **Active view** (default: 5 peers) — actively connected peers
   - **Passive view** (default: 30 peers) — address book of known peers
   - Active connections are bidirectional; failed peers are replaced from the passive set
   - Shuffle operations periodically exchange passive-view entries with neighbors

2. **PlumTree** — Broadcast protocol built on HyParView:
   - **Eager set** — subset of active peers that receive full message pushes
   - **Lazy set** — subset that receive only message hashes (`Ihave` notifications)
   - On receiving an `Ihave`, a peer requests the message if not already received
   - The eager/lazy sets self-optimize based on observed latency

### Topic Model

All protocol messages are namespaced by a 32-byte `TopicId`. Each topic is an
independent gossip swarm with its own routing tables and connections. Joining
multiple topics increases the number of open connections.

### Wire Format

Messages on the gossip wire are length-prefixed, postcard-encoded protocol
messages wrapped in the iroh QUIC stream abstraction.

## Inbox Protocol (`/iroh-chat-inbox/1`)

### Purpose

The inbox protocol provides reliable, asynchronous direct message delivery
between peers. Messages are delivered even when the recipient is offline
(the sender stores them in the recipient's mailbox via the protocol).

### Security

- Every `Deliver` and `Ack` is wrapped in a `SignedInboxMessage` with sender
  signature and timestamp for replay protection.
- The handler rejects connections from unknown senders (must be in the
  `allowed_senders` set, populated from the contact/friend list).
- 24-hour clock-skew window prevents replay of old messages.
- Duplicate `message_id`s are deduplicated within the replay window.

### Message Types

| Type | Direction | Purpose |
|------|-----------|---------|
| `Deliver` | Sender → Recipient | Carries a `MailboxEnvelope` (encrypted message payload) |
| `Ack` | Recipient → Sender | Acknowledges receipt of a delivered message |
| `SyncRequest { since_ms }` | Receiver → Sender | Request missed envelopes since a timestamp |
| `SyncResponse { envelopes }` | Sender → Receiver | Batch of missed envelopes |
| `DeleteTombstone` | Either | Author-signed deletion proof for remote message removal |

### Delivery Lifecycle

1. Sender opens a bi-directional QUIC stream to the recipient's inbox endpoint.
2. Sender sends a `Deliver` envelope (length-prefixed, postcard-encoded).
3. Recipient validates: allowed sender check, signature verification, clock-skew
   check, message-id dedup, then stores the envelope in `MailboxStore`.
4. Recipient sends back an `Ack` (length-prefixed `SignedInboxMessage`).
5. Sender's protocol handler emits `InboxEvent::AckReceived` and the outbox
   advances `SentAwaitingAck → Acknowledged`.

### Sync Flow

When a peer reconnects after being offline:

1. The reconnecting peer sends `SyncRequest { since_ms }` where `since_ms` is
   the timestamp of the last envelope they processed.
2. The online peer queries its `MailboxStore` for pending envelopes addressed
   to the requester and created at or after `since_ms`.
3. The online peer responds with `SyncResponse { envelopes }`.
4. The reconnecting peer processes each envelope through `accept_incoming()`,
   which is idempotent (replayed envelopes return the authenticated payload
   without duplicate insertion).

### Delete Tombstone Protocol

When a user deletes a message they authored, a signed `AuthorDeleteProof`
is propagated to peers who received the original message. Recipients verify
the proof cryptographically before applying the deletion locally.

The proof covers `msg_id || conversation_id || created_at_unix_secs` and is
signed by the original message author. The outer `SignedInboxMessage`
authenticates the forwarder. See [`offline-direct-messaging.md`](offline-direct-messaging.md)
for the full delivery state machine and retry/ack semantics.

## Backfill Protocol (`/iroh-chat-backfill/1`)

### Purpose

Allows a peer that joins a topic (or reconnects after being offline) to
request missed message history from connected peers.

### Flow

1. Requester opens a bi-directional QUIC stream to a responder
2. Sends a `BackfillRequest` (length-prefixed, postcard-encoded)
3. Responder queries its `ChatHistoryStore` and replies with a
   `BackfillResponse` containing raw signed message bytes
4. Requester verifies and decodes each message, feeding it through the
   normal `NetEvent` channel

### Rate Limiting

At most one backfill request per remote `PublicKey` is served concurrently.

## Whisper Protocol (`/iroh-chat-whisper/1`)

### Purpose

Direct QUIC channels for private 1:1 conversations, separate from the
gossip broadcast mesh. Used for both direct messages and file transfer.

### Architecture

| Component | Description |
|-----------|-------------|
| `WhisperBuilder` / `Whisper::spawn` | Create and run the whisper actor |
| `WhisperHandle` | Cloneable handle for sending DMs and files |
| `WhisperProtocol` | Protocol handler registered on the Router for incoming connections |
| `WhisperEvent` | Events delivered to the frontend (messages, connect/disconnect) |
| `session_manager` | Per-peer session state and reconnection logic |

### Connection Model

Each whisper connection carries bi-directional streams with length-prefixed,
postcard-encoded frames. Connections are established on demand when the user
initiates a DM and are maintained for the duration of the conversation.

## Friend Ping Protocol (`/iroh-chat-ping/1`)

### Purpose

Lightweight connectivity checks between friends to detect online/offline
status. Implemented in `chat_core::friend_ping`.

| Parameter | Default |
|-----------|---------|
| Ping interval | 30 seconds |
| Connect timeout | 10 seconds |
| ALPN | `/iroh-chat-ping/1` |

## Discovery System

boru-chat uses several discovery mechanisms, not all of which are QUIC protocols:

| Mechanism | Scope | Technology |
|-----------|-------|------------|
| mDNS | LAN | iroh-mdns-address-lookup |
| DHT (public rooms) | WAN | iroh-mainline-address-lookup (Mainline DHT) |
| DHT (private rooms) | WAN | Same DHT, but namespace-isolated via `DiscoverySecret` |
| Memory lookup | Local | In-process address book for bootstrap peers |

### Public Rooms

Public rooms use deterministic identities derived from
(network, room name, protocol version) with domain-separated topic and
discovery key derivation. Continuous publication loops re-publish local
presence on the DHT at configurable intervals.

### Private Rooms

Private rooms derive their DHT namespace from BLAKE3(topic || secret)
where the secret is a 32-byte random key. Only peers who know both the
topic and the secret can find each other on the DHT.

## Remote File Catalogue

The file-sharing protocols are separate from gossip. A revision notification is
advisory and contains no file bytes or access grant. The requester fetches a
fresh signed snapshot over `/iroh-chat-catalogue/1`; the client verifies the
frame version, transport identity, owner identity, signature, bounds, and field
limits before caching it. `known_revision` enables `NotModified`; cache refresh
is event-driven (new revision, manual refresh, stale cache, or missing item),
not continuous polling.

The owner builds each snapshot for `Connection::remote_id`. Blocked peers are
denied. Confirmed friends see enabled, available offers by default; other
peers see only files with an explicit `read` permission. Disabled offers,
unavailable file objects, and empty collections are omitted. The signed
projection contains hash, display metadata, size, MIME type, collection name,
and file revision, but no source path, username, database ID, permission row,
blob ticket, or unrestricted address.

### Transfer authorisation

To retrieve one entry, the requester sends its shared-file/content hash and
expected file revision over `/iroh-chat-transfer-auth/1`. The owner repeats the
block, relationship, permission, offer, availability, expected-hash, and
expected-version checks at request time. A grant returns a signed download
descriptor bound to owner, requester, file, content hash, size, blob format,
issue/expiry times, and a random nonce. The default expiry is five minutes;
there are no permanent capabilities. Errors intentionally do not reveal whether
an inaccessible file exists.

The descriptor authorises an iroh-blobs content-addressed transfer. The client
streams into a temporary file, verifies expected size and BLAKE3 content hash,
and atomically renames only verified output. Pause/cancellation removes the
temporary file. iroh-blobs may reuse verified chunks on a later request, but the
output file is re-streamed; temporary-file byte-range resume is not supported.

### Limits and errors

Requests are capped at 64 KiB, responses at 1 MiB, catalogues at 1,000 entries
and 2 MiB encoded, and pages at 1–200 entries. Handlers apply global/per-peer
concurrency caps, deadlines, a 30 requests/minute sliding-window limit, and
stale-state pruning. File preparation also limits concurrent hashing/blob
registration and may apply per-peer cooldowns. Invalid signatures, malformed
metadata, oversized payloads, permission failures, unavailable/changed files,
and rate limits have structured error categories.

## Transport Security

All protocols run over iroh's QUIC transport with TLS 1.3 encryption.
The Tor transport module (`tor_transport.rs`) provides scaffolding for
.onion address support but is not yet a production-ready transport.
