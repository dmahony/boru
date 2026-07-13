# iroh-gossip-chat

A decentralized, peer-to-peer chat application built on [iroh](https://iroh.com) gossip networking. Messages are disseminated via *epidemic broadcast trees* — there is no central server. Peers join a shared topic, discover each other through a layered address lookup system (DNS/Pkarr, mDNS, DHT, and gossip), and exchange messages directly over encrypted iroh connections.

## Origin

This project is a chat application built from the gossip protocol code originally published by [n0-computer/iroh-gossip](https://github.com/n0-computer/iroh-gossip). The protocol foundation remains under the original project's license; this repository adds the Boru chat protocol, durable identity and history, offline direct-message delivery, image sharing, and TUI/GUI frontends.

_Boru_ — after Brian Boru, the Irish king who unified Ireland's fractured, warring clans into a single kingdom in the early 11th century, ending centuries of fragmented rule without a fixed central capital or throne — he held power through alliances between chieftains rather than a single seat of authority.

That's the metaphor: boru-chat has no server, no central authority holding the network together — just peers connecting directly and gossiping messages between each other, the same way Boru's Ireland held together through direct bonds between chieftains rather than a hierarchy.

## Features

- **Group chat rooms** — create or join a topic-based chat room via a shareable ticket; messages are broadcast to all peers in the swarm.
- **Direct conversations** — add peers as contacts and open one-to-one direct chats with a deterministically derived private topic.
- **Offline mailbox** — store-and-forward delivery of direct messages when the recipient is offline (X25519 + AES-256-GCM, sender-signed, idempotent acks).
- **Image sharing** — inline image upload and display with content-addressed per-user storage.
- **Durable identity and history** — the node identity and chat transcript persist across restarts.
- **Two frontends** — a terminal UI ([ratatui](https://ratatui.rs)) and a desktop GUI ([iced](https://iced.rs)) with shared protocol code.

## Frontends

| Frontend | UI framework | Run |
|---|---|---|
| TUI | ratatui | `cargo chat open` / `cargo chat join <ticket>` |
| GUI | iced | `cargo iced-chat open` / `cargo iced-chat join <ticket>` |

Both share the same protocol types, `ChatCallbacks` trait, and `handle_net_event` core, so features work identically across both interfaces.

---

# Protocol

This crate implements the `boru-chat` protocol.
It is based on *epidemic broadcast trees* to disseminate messages among a swarm of peers interested in a *topic*.
The implementation is based on the papers [HyParView](https://asc.di.fct.unl.pt/~jleitao/pdf/dsn07-leitao.pdf) and [PlumTree](https://asc.di.fct.unl.pt/~jleitao/pdf/srds07-leitao.pdf).

The crate is made up from two modules:
The `proto` module is the protocol implementation, as a state machine without any IO.
The `net` module implements networking logic for running `boru-chat` on `iroh` connections.

The `net` module is optional behind the `net` feature flag (enabled by default).

# Getting Started

The `boru-chat` protocol was designed to be used in conjunction with `iroh`. [Iroh](https://docs.rs/iroh) is a networking library for making direct connections, these connections are how gossip messages are sent.

Iroh provides a [`Router`](https://docs.rs/iroh/latest/iroh/protocol/struct.Router.html) that takes an [`Endpoint`](https://docs.rs/iroh/latest/iroh/endpoint/struct.Endpoint.html) and any protocols needed for the application. Similar to a router in webserver library, it runs a loop accepting incoming connections and routes them to the specific protocol handler, based on `ALPN`.

Here is a basic example of how to set up `boru-chat` with `iroh`:
```rust,no_run
use iroh::{protocol::Router, Endpoint, EndpointId, endpoint::presets};
use boru_chat::{api::Event, Gossip, TopicId};
use n0_error::{Result, StdResultExt};
use n0_future::StreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    // create an iroh endpoint that includes the standard discovery mechanisms
    // we've built at number0
    let endpoint = Endpoint::bind(presets::N0).await?;

    // build gossip protocol
    let gossip = Gossip::builder().spawn(endpoint.clone());

    // setup router
    let router = Router::builder(endpoint)
        .accept(boru_chat::ALPN, gossip.clone())
        .spawn();

    // gossip swarms are centered around a shared "topic id", which is a 32 byte identifier
    let topic_id = TopicId::from_bytes([23u8; 32]);
    // and you need some bootstrap peers to join the swarm
    let bootstrap_peers = bootstrap_peers();

    // then, you can subscribe to the topic and join your initial peers
    let (sender, mut receiver) = gossip
        .subscribe(topic_id, bootstrap_peers)
        .await?
        .split();

    // you might want to wait until you joined at least one other peer:
    receiver.joined().await?;

    // then, you can broadcast messages to all other peers!
    sender.broadcast(b"hello world this is a gossip message".to_vec().into()).await?;

    // and read messages from others!
    while let Some(event) = receiver.next().await {
        match event? {
            Event::Received(message) => {
                println!("received a message: {:?}", std::str::from_utf8(&message.content));
            }
            _ => {}
        }
    }

    // clean shutdown makes sure that other peers are notified that you went offline
    router.shutdown().await.std_context("shutdown router")?;
    Ok(())
}

fn bootstrap_peers() -> Vec<EndpointId> {
    // insert your bootstrap peers here, or get them from your environment
    vec![]
}
```

The `examples/chat.rs` demo runs over direct iroh connectivity. It prints a base32 ticket containing the topic and endpoint addresses for peers to join.

## Identity Persistence

Both the `chat` and `setup` examples persist the node identity (secret key) to disk so your iroh peer ID remains stable across restarts.

**Storage location** (checked in order):
1. `$BORU_CHAT_DATA_DIR/secret_key.txt` — if the env var is set
2. `$XDG_DATA_HOME/boru-chat/secret_key.txt` — typical: `~/.local/share/boru-chat/secret_key.txt`
3. `$HOME/.local/share/boru-chat/secret_key.txt` — fallback when `XDG_DATA_HOME` is unset
4. `$LOCALAPPDATA/boru-chat/secret_key.txt` — Windows
5. `./.boru-chat/secret_key.txt` — current directory fallback

**File format:** The secret key is stored as lowercase hex-encoded bytes (64 hex chars) with a trailing newline. The file is created with restrictive permissions (`0o600`, owner read/write only) on Unix systems.

**Resetting the identity:** Delete the `secret_key.txt` file. The next run will generate a fresh keypair.

The chat room transcript is stored separately in `chat_history.json` under the same data directory. That file is durable local history, not live network state. To clear the current room's transcript from the UI, use `/leave`; to clear everything, delete `chat_history.json`.

```text
rm ~/.local/share/boru-chat/secret_key.txt
```

You can also set `BORU_CHAT_DATA_DIR` to a different path to use a separate identity for different sessions.

**Overriding via CLI flag:** The `chat` example accepts `--secret-key <hex>` to use a specific key for one session without writing it to disk.

The examples use Cargo aliases (defined in `.cargo/config.toml`) for
convenience.  The long-form `cargo run --example ...` is always an
alternative.

To run two peers with the **TUI (ratatui)** frontend:
```text
# Terminal 1 — create a room
cargo chat open

# Terminal 2 — join with the printed ticket
cargo chat join <ticket>

# Long form (also works without the alias)
cargo run --example chat -- open

# Direct iroh mode (the only supported transport)
cargo run --features examples --example chat -- open
```

## GUI frontend (iced)

A **GUI** frontend built with [iced](https://iced.rs) is
available behind the `gui` feature flag.

**`iced-chat`** (modular, split across `main.rs` + `app.rs`):
```text
cargo iced-chat open                   # open a room
cargo iced-chat join <ticket>          # join a room

# Long form
cargo run --features gui --example iced_chat -- open
```

The GUI replicates the full chat feature set: text messages, file
sharing (`/send <path>`, `/download`), dark mode toggle, and a
scrolling chat log.  Networking runs in background tokio tasks with
events flowing into the iced event loop via a channel.

## DHT Room Discovery

Private rooms can optionally use the Mainline BitTorrent DHT to discover active
room members without putting endpoint addresses in the room ticket. This is
separate from the public-room address lookup described below: it publishes
short-lived, signed discovery records under a namespace derived from the room
topic and a random 32-byte `discovery_secret`.

### How to use it

Create a room normally; when DHT discovery is enabled, the generated ticket
contains the room topic, bootstrap addresses (if any), and the discovery
secret. A joining client uses the ticket's known addresses first, then queries
the private namespace and merges validated endpoint IDs into its bootstrap set.
After joining, the continuous tracker republishes presence and refreshes the
peer set. Legacy tickets without a discovery secret continue to use their
existing bootstrap peers.

```text
# TUI: DHT private-room discovery is enabled by default in current builds
cargo chat open
cargo chat join <ticket>

# Explicitly disable private-room DHT discovery; public-room discovery is
# unaffected and ticket-only rooms remain compatible.
cargo chat --no-dht open
cargo chat --no-dht join <ticket>

# Builds/configurations that default DHT off may explicitly enable it with:
# cargo chat --dht open
# cargo chat --dht join <ticket>
# (The current examples enable private-room DHT by default and expose
# --no-dht as the explicit disable switch.)

# GUI uses the room-creation checkbox for per-room opt-in/out, and also
# accepts --no-dht for a legacy-only session.
cargo iced-chat --no-dht
```

`--dht` and `--no-dht` control private-room discovery only; they do not
replace Boru's gossip implementation or the public lobby's address lookup.
The GUI's room creation dialog provides the equivalent per-room toggle.

### Compatibility and privacy

DHT discovery is additive and non-fatal. If UDP is blocked or a lookup fails,
clients fall back to the addresses in the ticket and normal iroh address
lookup. DHT lookup returns endpoint IDs, not complete endpoint addresses, so a
reachable ticket bootstrap peer or another address lookup method may still be
needed to turn a discovered ID into a connection.

The discovery secret is a bearer capability: anyone who holds it can query the
room namespace. Keep tickets private and do not log or paste them into public
channels. The DHT records are encrypted and endpoint-signed, but this feature
is **not message encryption** and does not make membership anonymous. Ticket
holders can discover publishers, while the DHT can still observe ordinary
network metadata such as packet timing and source addresses. See
[ARCHITECTURE.md](ARCHITECTURE.md) for the component and security models.

## Address Lookup Methods

boru-chat uses a layered address lookup system to discover peer addressing information. Each method has different tradeoffs:

### DNS/Pkarr (default, enabled by `presets::N0`)

**How it works:** The endpoint publishes signed records (EndpointID + relay URL)
to a DNS server run by n0.computer.  Other endpoints resolve by querying
`_iroh.<z32-endpoint-id>.dns.iroh.link` for a TXT record.

**When to use:** Everywhere.  Fast, simple, works out of the box with the
default iroh preset.  Requires trust that the DNS server is available and
honest (signatures protect record integrity, but a compromised server could
withhold records).

**Limitations:** Single point of dependency on n0's infrastructure.  No
discovery inside a LAN without internet access.

### mDNS (enabled by default)

**How it works:** mDNS address lookup broadcasts endpoint info on the local
network.  Other endpoints on the same subnet receive it without any relay or
server.

**When to use:** LAN-only scenarios — same office, home network, conference
WiFi.  No internet needed.  Fastest local discovery.

**Limitations:** Does not cross subnets or work over VPNs that don't forward
multicast.  Not suitable for global peer discovery.

### DHT — Mainline BitTorrent DHT (enabled manually)

**How it works:** Uses the [BitTorrent Mainline DHT](https://en.wikipedia.org/wiki/Mainline_DHT)
to publish and resolve signed endpoint records (same record format as DNS/Pkarr).
No central server required — any endpoint can publish and resolve directly on
the DHT.

**When to use:**
- Global peer discovery **without depending on n0's DNS server**
- Censorship-resistant / air-gapped deployments where a DNS server isn't
  reachable (DHT runs over UDP directly)
- Combined with [mDNS](#mdns-enabled-by-default) for fully decentralized
  address lookup (local via mDNS, global via DHT)

**Limitations:**
- **Slower than DNS** — DHT lookups are iterative (query several nodes before
  finding the record).  Expect 500ms–5s for a fresh lookup vs ~100ms for DNS.
- **Publish lag** — publishing a record to the DHT also takes time (~seconds).
  If the endpoint changes relays, there's a window where old DHT records point
  to a stale relay.
- **Network filtering** — some corporate or ISP firewalls rate-limit or block
  DHT traffic (UDP on a wide port range).
- **Not default** — must be explicitly added via
  `DhtAddressLookup::builder()`, which both examples already do.

### MemoryLookup (programmatic)

**How it works:** An in-memory table of (EndpointID → addressing info) that
your code populates directly.  Used to bootstrap known peers before they can
be discovered via other methods.

**When to use:** Bootstrap — when joining a room you already know the relay
address of a bootstrap peer.  Also used internally by `GossipAddressLookup`
to distribute gossip-learned addresses.

### GossipAddressLookup (internal)

**How it works:** The gossip protocol itself distributes endpoint addressing
information via `Join` and `ForwardJoin` messages.  When a peer joins a topic,
everyone it connects to learns its addressing info.  This is automatic and
always active.

**When to use:** Always active — no configuration needed.  Complements other
methods by seeding addresses that were learned through gossip.

### Ticket (out-of-band)

**How it works:** A base32-encoded ticket containing the topic, relay URL, and
direct addresses of the room creator.

**When to use:** Joining a room for the first time before any discovery
method has data on that peer.

**Limitations:** Expires — if the peer changes network or relay, old tickets
can't find them.  Must be shared through a side channel (copy-paste, QR code,
etc.).

### Summary

| Method                | Scope   | Server needed | Speed     | Requires config |
| --------------------- | ------- | ------------- | --------- | --------------- |
| DNS/Pkarr             | Global  | Yes (n0)      | Fast      | No (default)    |
| mDNS                  | Local   | No            | Instant   | Yes             |
| DHT (Mainline)        | Global  | No            | Slow      | Yes             |
| MemoryLookup          | Manual  | No            | Instant   | Yes (code)      |
| GossipAddressLookup   | Swarm   | No            | Real-time | No              |
| Ticket                | One-off | No            | Instant   | Yes (side ch.)  |

Both examples (`chat` and `iced_chat`) enable **mDNS** and **DHT** alongside
the default DNS/Pkarr.  mDNS is gated on `iroh_mdns_address_lookup`
building successfully (it may fail in headless/container environments).  DHT
is gated on the `net` feature flag, which is automatically enabled by
`examples` and `gui`.

## Contact Negotiation and Direct Conversations

The `contact` module provides signed control-plane messages that run over
the encrypted whisper channel.  Messages carry an additional layer signature
so they remain verifiable when replayed from offline mailbox storage.

### ContactAction Messages

| Action | Purpose |
|--------|---------|
| `FriendRequest` | Ask a peer to become friends |
| `FriendRequestAccepted` | Accept a pending friend request |
| `FriendRequestRejected` | Reject a pending friend request |
| `ConversationInvite` | Agree on a stable one-to-one gossip topic (must already be friends) |
| `AddressUpdate` | Refresh bootstrap addresses for a direct session |
| `MailboxAdvertise` | Advertise the peer's encrypted mailbox key |

All messages are signed with the sender's identity key and include a
wall-clock timestamp that is validated against a 24-hour replay window.

### Direct Topic Derivation

The stable one-to-one gossip topic shared by two contacts is derived
deterministically and order-independently:

```text
topic = blake3("iroh-gossip-chat/direct/v1" || min(a, b) || max(a, b))
```

This means both sides derive the same topic without any negotiation — the
topic is a function of the two peer identities alone.

## Offline Mailbox (Store-and-Forward)

The `mailbox` module implements encrypted recipient-hosted storage for
direct messages when the recipient is offline.  Key design points:

- **Never decrypts.**  The mailbox only stores opaque ciphertext.  It
  never has access to plaintext.
- **X25519 + AES-256-GCM.**  Each envelope uses an ephemeral X25519 key
  per message, with a symmetric key derived via blake3 for AES-GCM.
- **Sender signatures.**  Every envelope is signed by the sender and
  verified by the recipient before decryption.
- **Authorization.**  The recipient controls an allow-list of senders.
  Unauthorized envelopes are rejected without consuming storage.
- **Idempotent acknowledgements.**  The recipient signs an acknowledgement
  after successfully processing each message.  Acknowledgements are signed
  and verified; duplicate ack removal is idempotent.
- **TTL expiration.**  Unacknowledged envelopes expire after the configured
  retention period (default 7 days).
- **Atomic persistence.**  The mailbox is persisted atomically via
  `atomic_write_json` so partial writes never corrupt state.

### Mailbox Identity

The encryption identity is derived from the node's iroh secret key and is
advertised alongside the signing public key:

```rust,norun
let mailbox_id = MailboxIdentity::from_secret(&secret_key);
let pub_key = mailbox_id.public_key();        // MailboxPublicKey { identity, encryption }
```

Senders use `MailboxPublicKey.encryption` to seal envelopes; the recipient
uses its private key to open them.

### Mailbox Flow

1. **Sender** encrypts and signs the payload, producing a `MailboxEnvelope`.
2. **Sender** delivers the envelope over the whisper channel (or queues it
   in the outbox if the recipient is offline).
3. **Recipient** receives the envelope, verifies the sender signature,
   decrypts, processes the message, and signs a `MailboxAck`.
4. **Recipient** sends the `MailboxAck` back to the sender.
5. **Sender** or **mailbox** removes the envelope after receiving a valid
   acknowledgement.

Both the whisper transport (`WhisperWireMessage::MailboxEnvelope` /
`MailboxAck`) and the mailbox store support this lifecycle.

## Stale Address Behavior and Bootstrap

When a peer's cached address fails to connect, the system falls back
through a layered resolution chain:

1. **Cached addresses** from the FriendsStore or the most recent
   `AddressUpdate` message.
2. **Endpoint remote info** — addresses learned from the current
   iroh endpoint's connection state.
3. **DNS/Pkarr** — default iroh discovery (presets::N0).
4. **DHT** — Mainline BitTorrent DHT (if configured).
5. **GossipAddressLookup** — addresses learned via gossip protocol.
6. **ID-only lookup** — the endpoint resolves using all configured
   discovery mechanisms without any explicit address.

The friend ping system sends periodic probes through all candidates using
750ms per-attempt timeouts.  Address updates discovered through this
process are emitted as `FriendEvent::AddressUpdated` events so the
frontend can persist fresher addresses.

### Bootstrap

Room bootstrap requires at least one address to join.  When no bootstrap
addresses are available (e.g., after all cached peers are stale), the
room creator must provide an updated ticket.  The timeout for bootstrap
connection is 30 seconds, after which the room is subscribed but the user
is warned that addresses may be stale.

### First-Contact Handshake

A first-contact handshake (`FriendRequest` → `FriendRequestAccepted` / `FriendRequestRejected`)
is required to establish a friendship.  Accepting a friend request does **not**
auto-open a conversation — the user must explicitly invite their established
friend to a direct conversation via a separate `ConversationInvite`.  Until a
friend accepts, the initiator's messages are queued in the outbox but not
delivered to the direct topic.  The recipient must be online at least briefly
for the initial handshake to complete.

## Outbox and Delivery Lifecycle

Outgoing direct messages are durably stored in the `OutboxStore` until
transport delivery is observed.  On restart, the outbox is replayed:

- Messages with `DeliveryState::Pending` are re-sent over the whisper
  channel.
- Messages with `DeliveryState::Sent` are skipped (already delivered).
- On successful delivery, the state transitions from `Pending` -> `Sent`.

The chat history is also persisted (via `ChatHistoryStore`) so room
transcripts survive process restarts.

## Image Storage

The `image_store` module (feature-gated behind `net`) provides secure, local per-user image storage with content-addressed identifiers.

### Storage Location

Images are stored below the application's **data directory** in a `files/` subdirectory:

```text
<data_dir>/files/
```

The data directory is resolved in the same way as identity and chat history (see [Identity Persistence](#identity-persistence)):

1. `$BORU_CHAT_DATA_DIR` — if the environment variable is set
2. `$XDG_DATA_HOME/boru-chat/` — typical: `~/.local/share/boru-chat/`
3. `$HOME/.local/share/boru-chat/` — fallback when `XDG_DATA_HOME` is unset
4. `$LOCALAPPDATA/boru-chat/` — Windows

Example default path:

```text
~/.local/share/boru-chat/files/
```

### Overriding the Storage Path

Two constructors control the root:

| Constructor | Purpose |
|---|---|
| `ImageStore::at(data_dir)` | Root is `<data_dir>/files`. Use the application's resolved data directory. |
| `ImageStore::from_files_dir(files_dir)` | Root is an explicit directory. Useful for tests and custom deployments. |

Override the whole data directory with `$BORU_CHAT_DATA_DIR`. The iced GUI also honors `$BORU_CHAT_FILES_DIR` when you want to point image caching at an explicit files root for tests or alternate layouts. There is no separate env var for the files subdirectory alone in the default layout — the `files/` suffix is always appended to the data directory.

### Per-User Directory Layout

User names are never used as raw path components. Each user is assigned a **blake3 hash** of their name string, and images are stored under that hash directory:

```text
<data_dir>/files/
  <user-hash-a>/
    <content-hash-1>.png
    <content-hash-2>.jpg
  <user-hash-b>/
    <content-hash-3>.webp
```

- User directories are created automatically on first save.
- Unix permissions: directories `0o700`, files `0o600`.

### Public API

All operations are synchronous file I/O and return `n0_error::Result<T>`.

#### `save_image(user, filename, bytes) -> Result<String>`

Saves image bytes and returns a stable, portable identifier of the form:

```text
<user-hash>/<content-hash>.<extension>
```

- **user** — a non-empty string identifying the owning user (any string; the user's blake3 hash is used as the directory key).
- **filename** — the original filename. Only its extension matters: matched against the allow-list (see below). The name itself is never used as a path component.
- **bytes** — the raw image data. Must not be empty.
- **Identifier format** — `<user-hash>/<content-hash>.<extension>` where both hashes are 64-hex-char blake3 digests.
- **Atomicity** — writes to a temporary file first, then `rename`s to the final path. If the write or sync fails, the temp file is cleaned up.

```rust,norun
use crate::image_store::ImageStore;

let store = ImageStore::at(data_dir);
let id = store.save_image("alice", "photo.png", &image_bytes)?;
// id = "a1b2c3d4.../e5f6g7h8....png"
```

#### `resolve_image(user, identifier) -> Result<PathBuf>`

Validates and resolves an identifier to a **relative** path within the store's root. The returned path is never absolute.

```rust,norun
let relative = store.resolve_image("alice", &id)?;
// relative = "a1b2c3d4.../e5f6g7h8....png"
```

The raw relative path should not be serialized. Persist the identifier string instead (see [Portable Identifiers](#portable-identifiers)).

#### `image_exists(user, identifier) -> Result<bool>`

Returns whether the identified file exists as a regular file on disk.

```rust,norun
if store.image_exists("alice", &id)? {
    // serve from local disk
}
```

#### `delete_image(user, identifier) -> Result<bool>`

Deletes the identified file. Returns `true` if a file was removed, `false` if it did not exist. Does not fail on a missing file.

```rust,norun
let removed = store.delete_image("alice", &id)?;
```

### Portable Identifiers

**Callers must persist the identifier string returned by `save_image`, not the filesystem path.** The identifier is relative by design — it survives data-directory moves, renames, and restores from backup. The filesystem path returned by `resolve_image` is a runtime convenience, not a portable storage key.

```rust,norun
// ✅ Correct — persist the identifier
let id = store.save_image("alice", "photo.png", &bytes)?;
database.store_image_metadata(message_id, &id);

// ❌ Wrong — do not persist the resolved filesystem path
let relative = store.resolve_image("alice", &id)?;
database.store_path(&relative); // fragile across data-dir moves
```

### Security

#### User Isolation

Each user is assigned a separate hash directory. `save_image` always writes to the calling user's directory; the user parameter in `resolve_image`, `image_exists`, and `delete_image` authenticates access. There is no cross-user read or write path through the API. Two users with the same name string produce the same hash, matching the semantics of a stable user database key.

#### Filename Sanitization

Original filenames are **never used as path components**. Only the file extension is extracted, lowercased, and matched against a strict allow-list:

| Supplied extension | Stored extension |
|---|---|
| `.png` | `.png` |
| `.jpg` | `.jpg` |
| `.jpeg` | `.jpeg` |
| `.gif` | `.gif` |
| `.webp` | `.webp` |
| `.bmp` | `.bmp` |
| Any other or no extension | `.bin` |

The original filename (including any directory separators, parent-path references, or special characters) is discarded entirely.

#### Traversal Prevention

Directory-traversal and symlink attacks are rejected at multiple points:

- Identifiers must have exactly one `/` separating the user hash and filename with no `..` components.
- The user-hash segment must match the hash of the authenticated user string.
- The filename must be exactly 64 hex characters (a blake3 digest) followed by a dot and an allowed extension.
- Both the user directory and the resolved file path are checked for symlinks and rejected if found — a compromised filesystem cannot inject a symlink to redirect reads.

#### Identifier Validation Matrix

| Input | Result |
|---|---|
| `a1b2.../e5f6....png` | Valid — accepted |
| `../secret` | Rejected — no `/` separator |
| `x/../../secret.bin` | Rejected — directory traversal |
| `alice/../secret.bin` | Rejected — directory traversal |
| `a1b2/short.bin` | Rejected — filename not 64 hex chars |
| `a1b2/e5f6...exe` | Rejected — `.exe` not in allow-list |
| `different-hash/e5f6...png` | Rejected — user hash mismatch |
| `a1b2/e5f6...png` (user dir is symlink) | Rejected — symlink detected |

### Backup and Migration

Backing up or migrating the `files/` directory is safe because:

- **All paths are content-addressed.** The identifier for an image depends only on user identity and content bytes, not on directory layout. Restoring `files/` to a different base path works without updating any references.
- **No absolute paths are stored anywhere.** The identifier strings that callers persist are relative path fragments; they resolve correctly under any data directory.
- **Permissions are set on write.** Restoring with `rsync -a` preserves Unix permissions. If permissions are lost, the app re-creates them on the next write but reads are unaffected.
- **Empty hash directories are harmless** — they consume only an inode and can be pruned manually.

Migration steps:

```text
# Backup
tar czf images-backup.tar.gz ~/.local/share/boru-chat/files/

# Restore to a new machine under a different data directory
tar xzf images-backup.tar.gz -C /new/data/path/files/
```

After migration, update `$BORU_CHAT_DATA_DIR` (or the platform default) on the new machine. All stored identifiers remain valid.

# License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   <http://www.apache.org/licenses/LICENSE-2.0>)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
