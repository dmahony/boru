# Protocol Layers

boru-chat uses multiple distinct QUIC-based protocols, each with its own ALPN
(Application-Layer Protocol Negotiation) string and purpose.

## Protocol Overview

| Protocol | ALPN | Type | Purpose | Persistence |
|---------|------|------|---------|-------------|
| Gossip | `/iroh-gossip/1` | Broadcast | Room-based message broadcasting via PlumTree; one independent mesh per room topic | None (transient) |
| Inbox | `/iroh-chat-inbox/1` | Direct request/response | Offline message delivery, ACKs, sync responses, and deletion tombstones; not a gossip topic | InboxEvent emission |
| Backfill | `/iroh-gossip-chat/backfill/1` | Direct | Historical message sync for late-joining peers | Reads ChatHistoryStore |
| Whisper | `/iroh-gossip-chat/whisper/1` | Direct session | Online private 1:1 QUIC messages, control frames, and file transfer | None (transient) |
| Friend Ping | `/iroh-gossip-chat/friend-ping/1` | Direct | Connectivity checks between friends | None (transient) |
| Catalogue retrieval | `/boru-file-catalog/1` | Direct | Signed, requester-filtered file catalogue retrieval | Per-peer verified cache |
| Transfer authorisation | `/boru-file-access/1` | Direct | Request-time permission check and signed blob descriptor | None (short-lived descriptor) |
| Blob transfer | iroh-blobs | Direct | Content-addressed file transfer | iroh-blobs store + download state |

## Responsibility boundaries

| Concern | Single owner | Not responsible for |
|---|---|---|
| Peer discovery | mDNS/DHT/memory lookup plus the room discovery tracker | QUIC connection success, topic membership, or message delivery |
| Relay fallback | iroh endpoint address resolution/transport | Selecting application retries or re-enqueueing messages |
| Gossip room membership | `GossipTopic`/`GossipSender` for that room or conversation | Offline mailbox delivery or direct-message ACKs |
| Online 1:1 session | Whisper and `session_manager` | Durable mailbox state |
| Offline mailbox wire path | Inbox ALPN (`INBOX_ALPN`) | Peer discovery, room gossip, or retry policy |
| Durable offline retry | `OutboxDeliveryWorker` design owner | A second retry loop in Whisper or Inbox |
| History recovery | Backfill request/response | Live room membership maintenance |

A successful discovery event is not evidence of a connection; a connected
Whisper session is not evidence of a delivered offline envelope; and writing to
a QUIC stream is not an acknowledgement. Each boundary must be verified by its
own protocol event or signed application acknowledgement.

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

All protocol messages are namespaced by a 32-byte `TopicId`. Each room or direct
conversation topic is an independent gossip swarm with its own routing tables
and connections. The GUI retains each live room/conversation subscription while
it is active or backgrounded; dropping the sender and receiver leaves the topic.
The inbox protocol deliberately has no personal inbox gossip topic: offline
delivery uses the direct inbox ALPN below, so it does not depend on room
membership or an unrelated gossip subscription.

### Wire Format

Messages on the gossip wire are length-prefixed, postcard-encoded protocol
messages wrapped in the iroh QUIC stream abstraction.

## Inbox Protocol (`/iroh-chat-inbox/1`)

### Responsibility

The inbox protocol is the single wire path for asynchronous/offline mailbox
delivery. The sender owns durable queuing and retry scheduling; this protocol
only authenticates, transports, persists/forwards, and acknowledges one
envelope at a time. It does not discover peers, maintain gossip membership, or
own retry policy.

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
4. The reconnecting peer processes each envelope through the durable incoming
   acceptance transaction, which is idempotent (replayed envelopes return the
   authenticated payload without duplicate insertion).

`pending_fn` is the inbox handler's provider for producing bounded
`SyncResponse` pages. Production startup installs it from the durable mailbox
store; it filters by authenticated recipient and clamps the requester cursor to
the retention window before applying count/size limits. A received
`SyncRequested` event by itself is only an observation, not a response.

### Delete Tombstone Protocol

When a user deletes a message they authored, a signed `AuthorDeleteProof`
is propagated to peers who received the original message. Recipients verify
the proof cryptographically before applying the deletion locally.

The proof covers `msg_id || conversation_id || created_at_unix_secs` and is
signed by the original message author. The outer `SignedInboxMessage`
authenticates the forwarder. See [`offline-direct-messaging.md`](offline-direct-messaging.md)
for the full delivery state machine and retry/ack semantics.

## Backfill Protocol (`/iroh-gossip-chat/backfill/1`)

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

## Whisper Protocol (`/iroh-gossip-chat/whisper/1`)

### Responsibility

Whisper owns the live, online 1:1 session: direct chat frames, control frames,
and file-transfer coordination. It is separate from room gossip and from the
inbox ALPN. Mailbox wire variants remain for compatibility with older peers,
but the active offline fallback is `send_deliver`/`send_ack` on the inbox ALPN;
frontends must not enqueue or process the same envelope through both paths.

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

`session_manager` owns whisper reconnect/backoff and collision resolution.
`OutboxDeliveryWorker` is the sole owner of durable mailbox retry/lease state;
whisper sessions must not add a second mailbox retry loop.

## Friend Ping Protocol (`/iroh-gossip-chat/friend-ping/1`)

### Purpose

Lightweight connectivity checks between friends to detect online/offline
status. Implemented in `chat_core::friend_ping`.

| Parameter | Default |
|-----------|---------|
| Ping interval | 30 seconds |
| Connect timeout | 10 seconds |
| ALPN | `/iroh-gossip-chat/friend-ping/1` |

## Discovery System

boru-chat uses two independent DHT systems for different purposes:

| Layer | Purpose | Crate | DHT Instance |
|---|---|---|---|
| **Address resolution** | Resolve `EndpointId` to transport addresses | `iroh-mainline-address-lookup` | `n0_mainline::Dht` (separate UDP socket) |
| **Topic discovery** | Discover peer `EndpointId` values per room | `distributed-topic-tracker` | `mainline::async_dht::AsyncDht` (separate UDP socket) |

### Discovery Mechanisms

| Mechanism | Scope | Technology |
|-----------|-------|------------|
| mDNS | LAN | `iroh-mdns-address-lookup` |
| DhtAddressLookup | WAN | Mainline DHT / Pkarr — resolves EndpointId to addresses |
| Public-room discovery | WAN | `distributed-topic-tracker` under a deterministic namespace |
| Private-room discovery | WAN | Same, but namespace-isolated via `DiscoverySecret` + HPKE encryption |
| Memory lookup | Local | In-process address book for bootstrap peers |
| GossipAddressLookup | Mesh | Addresses learned from gossip Join/ForwardJoin messages |

### Public Rooms

Public rooms use deterministic identities derived from
(network, room name, protocol version) with domain-separated topic and
discovery key derivation. Background loops publish local presence on the DHT
every 5 minutes and discover new peers every 30 seconds with jitter.
A `PublicationPolicy` coordinates with the DHT minute to avoid redundant
publishes and applies exponential backoff on failure.

### Private Rooms

Private rooms derive their DHT namespace from `BLAKE3(domain_sep \|\| topic \|\| secret)`
where the secret is a 32-byte CSPRNG key. Discovery records are HPKE-encrypted
using per-minute keys so only peers who know the secret can find each other.
The `--no-dht` CLI flag gates private-room discovery.

### Validation Pipeline

Every discovery record goes through 5 checks in order (cheapest first):

1. **Size** — reject oversized records (>256 bytes)
2. **Timestamp** — reject stale (>10 min) or future-skewed (>2 min) records
3. **Decode** — reject records with unparseable content
4. **Identity** — reject records where pub_key ≠ payload endpoint_id
5. **Signature** — verify Ed25519 signature against the room's topic

Batch processing bounds records examined (max 20), deduplicates, filters self,
and caps discovered peers (max 20).

### DHT Outage

Existing connections and known addresses continue working. Exponential backoff
delays retries (1s → 60s cap). mDNS and ticket-based joins are unaffected.
Normal operation resumes automatically on DHT recovery.

### Privacy

- Default: relay-only mode — direct IPs never published (DhtAddressLookup uses
  `AddrFilter::relay_only()`)
- `--publish-direct-addresses`: exposes public IP on Mainline DHT
- Private rooms: DHT namespace undetectable without `DiscoverySecret`; records
  HPKE-encrypted; secret is never logged (Debug redacted to 4 hex chars)
- Public rooms: deterministic key means anyone who knows the room name can
  discover all members

### Wire Format

| Field | Size |
|-------|------|
| Topic hash | 32 B |
| Unix minute | 8 B |
| Publisher pub_key | 32 B |
| Content (version + EndpointId) | ~35 B |
| Ed25519 signature | 64 B |
| **Total envelope** | **~171 B** |

Private room records add HPKE encryption (~270 B ciphertext). The payload
format is versioned (currently version 1); unknown versions are rejected
at decode time.

### Known Limitations

- **Public lobby DHT not wired in GUI**: `ContinuousTracker` exists and is
  unit-tested but is never spawned in `main.rs`. Public-lobby users rely on
  mDNS (LAN) and tickets (out-of-band) for discovery.
- **DHT-discovered peers not in UI**: Private-room DHT peers join the gossip
  mesh directly via `spawn_join_fanout()` but do not appear in the
  "discovered peers" panel (which is mDNS-only).
- **Mainline mutable-record limits**: No atomic multi-value operations, no
  ordering, no deletion, probabilistic propagation delay.

See [`docs/discovery-architecture.md`](docs/discovery-architecture.md) for
the full architecture, operator guidance, and detailed module reference.

## Remote File Catalogue

The file-sharing protocols are separate from gossip. A catalogue is an
advertisement, not a download capability. The requester fetches a fresh,
requester-filtered signed snapshot over `/boru-file-catalog/1`; the client
verifies the frame version, transport identity, owner identity, signature,
bounds, duplicate/reference rules, and field limits before caching it.
`known_revision` enables `NotModified`; a revision change during pagination
returns `RevisionChanged` and the client restarts. There is no separate
implemented push-notification ALPN or continuous catalogue-polling worker;
applications may trigger refresh after observing a profile revision change,
manually, when stale, or when an item is missing.

The owner builds each snapshot for `Connection::remote_id`. Blocked peers are
denied. With no selected-peer grants, confirmed friends see enabled, available
offers by default and other peers see an empty catalogue. When a file has
explicit `read` grants, only granted peers see it; explicit denials, disabled
offers, unavailable file objects, and empty collections are omitted. The
signed projection contains hash, safe display metadata, size, MIME type,
collection IDs, and file revision, but no source path, username, database ID,
permission row, blob ticket, or unrestricted address.

### Transfer authorisation

To retrieve one entry, the requester sends its shared-file/content hash and
expected file revision over `/boru-file-access/1`. The owner repeats the
block, relationship, permission, offer, availability, expected-hash, and
expected-version checks at request time. A grant returns a signed download
descriptor bound to owner, requester, file, content hash, size, blob format,
issue/expiry times, and a random nonce. The default expiry is 60 seconds;
there are no permanent capabilities. Errors intentionally do not reveal whether
an inaccessible file exists.

The descriptor authorises an iroh-blobs content-addressed transfer. The client
streams into a temporary file, verifies expected size and BLAKE3 content hash,
and atomically renames only verified output. Retained iroh-blobs chunks may be
reused by a later content-addressed request, but this is not byte-range resume
of the temporary/destination file: the application does not append to a prior
output offset. If partial chunks are garbage-collected, transfer starts over.

### Limits and errors

Requests are capped at 256 KiB, full responses at 4 MiB, file-details responses
at 256 KiB, pages at 1 MiB/500 files, catalogues at 10,000 files and 1,000
collections, and individual advertised files at 10 TiB. Handlers apply
global/per-peer concurrency caps, deadlines, rate limits, and preparation
limits. Invalid signatures, malformed metadata, oversized payloads, permission
failures, unavailable/changed files, and rate limits have structured error
categories. See [`catalogue-limits.md`](catalogue-limits.md).

## Transport Security

All protocols run over iroh's QUIC transport with TLS 1.3 encryption.
The Tor transport module (`tor_transport.rs`) provides scaffolding for
.onion address support but is not yet a production-ready transport.
