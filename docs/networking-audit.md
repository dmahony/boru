# Networking Audit — Task 1.2

Audited: 2026-07-18
Codebase: iroh-gossip-chat (`boru-chat`), commit 9ed4f23

## 1. Protocol Landscape

### 1.1 Implemented & Registered Protocols

| Protocol | ALPN | Module | Handler | Router Registration | Purpose |
|----------|------|--------|---------|-------------------|---------|
| Gossip | `/iroh-gossip/1` | `src/net.rs:47` | `Gossip` (`net.rs:149`) | Every Router (`examples/iced_chat/main.rs:581`, `examples/setup.rs:93`, all integration tests) | Room-based broadcast mesh (HyParView + PlumTree) |
| Blob transfer | `iroh_blobs::ALPN` | iroh-blobs crate | `BlobsProtocol` | `examples/iced_chat/main.rs:582`, blob-size tests | Content-addressed file transfer |
| Friend Ping | `/iroh-gossip-chat/friend-ping/1` | `src/chat_core/friend_ping.rs:36` | `PingHandler` (`friend_ping.rs:124`) | `examples/iced_chat/main.rs:583` | Connectivity checks between friends |
| Backfill | `/iroh-gossip-chat/backfill/1` | `src/backfill.rs:62` | `BackfillProtocolHandler` (`backfill.rs:195`) | `examples/iced_chat/main.rs:584` | Historical message sync for late-joining peers |
| Whisper | `/iroh-gossip-chat/whisper/1` | `src/whisper/mod.rs:42` | `WhisperProtocol` (`whisper/mod.rs:334`) | `examples/iced_chat/main.rs:585` | Online private 1:1 messages, control frames, file-transfer coordination |
| Inbox | `/iroh-chat-inbox/1` | `src/inbox.rs:44` | `InboxProtocol` (`inbox.rs:463`) | `examples/iced_chat/main.rs:586` | Offline message delivery, ACKs, sync, deletion tombstones |

### 1.2 Planned But Not Yet Implemented

| Protocol | ALPN | Defined At | Status |
|----------|------|------------|--------|
| File Catalogue | `/boru-file-catalog/1` | `src/net.rs:56` (`FILE_CATALOG_ALPN`) | Constant exists, comment says "not yet registered" |
| File Access | `/boru-file-access/1` | `src/net.rs:65` (`FILE_ACCESS_ALPN`) | Constant exists, comment says "not yet registered" |

Pre-written integration tests import from modules that do not yet exist:
- `catalogue_client`, `catalogue_handler`, `catalogue_model`, `catalogue_protocol`, `protocol_version`
- Test files: `tests/test_remote_catalogue_integration.rs`, `tests/test_ui_file_sharing_integration.rs`

### 1.3 ALPN String Inconsistency (Minor)

The design doc at `docs/protocol-layers.md` lists these ALPNs, which differ slightly from source:

| Protocol | Actual ALPN (source) | Doc says |
|----------|---------------------|----------|
| Backfill | `/iroh-gossip-chat/backfill/1` | `/iroh-chat-backfill/1` |
| Whisper | `/iroh-gossip-chat/whisper/1` | `/iroh-chat-whisper/1` |
| Friend Ping | `/iroh-gossip-chat/friend-ping/1` | `/iroh-chat-ping/1` |

**Action:** Update `docs/protocol-layers.md` to match source constants. The doc appears stale — source uses `-gossip-chat-` prefix, doc uses `-chat-`.

---

## 2. Router Registration Pattern

The canonical pattern (from `examples/iced_chat/main.rs:580-587`):

```rust
let router = iroh::protocol::Router::builder(endpoint.clone())
    .accept(GOSSIP_ALPN, gossip.clone())          // ProtocolHandler impl
    .accept(iroh_blobs::ALPN, blobs_protocol.clone())
    .accept(FRIEND_PING_ALPN, PingHandler)         // unit-struct impl
    .accept(BACKFILL_ALPN, backfill_handler)
    .accept(WHISPER_ALPN, whisper_handler)
    .accept(INBOX_ALPN, inbox_protocol)
    .spawn();
```

Every `ProtocolHandler` must implement:
```rust
impl ProtocolHandler for X {
    fn accept(self: Arc<Self>, conn: Connection) -> BoxFuture<Result<(), AcceptError>>;
}
```

Test helpers reuse `Router::builder(ep).accept(GOSSIP_ALPN, gossip).spawn()` (sometimes with `iroh_blobs::ALPN` for blob transfer tests).

---

## 3. Iroh Endpoint Configuration

**Production (`examples/iced_chat/main.rs:459-478`):**
- Builder preset: `presets::N0` (or `presets::N0DisableRelay` when `--no-relay` flag is set)
- Secret key: loaded from `secret_key.txt`
- Address lookup: `MdnsAddressLookup` (mDNS) + `MemoryLookup` (bootstrap) + optionally `DhtAddressLookup`
- Relay mode: configurable (`RelayMode::Custom(relay_map)`, `Disabled`, or from config URL)
- Bind address: `0.0.0.0:<bind_port>` (default 0 = auto-assign)
- On relay mode: `endpoint.online().await` waits for relay connectivity

**Test helpers:**
- `presets::Minimal` (no relay) in most unit/integration tests
- `presets::N0` with `RelayMode::Disabled` or `Custom`
- `test_dummy()` (`net.rs:254`): `presets::N0`, `RelayMode::Disabled`, `MemoryLookup`

---

## 4. Address Resolution

### Resolution Order

`resolve_candidates()` in `src/net/address_resolution.rs:71` implements deterministic priority:

```
Current → Persisted → Mdns → Configured → Relay → Dht → TrustedPeer
```

- Identity-mismatch short-circuits to `IdentityMismatch` failure
- Empty candidates → `NoCandidates` failure
- Stable failure category codes: `address_resolution.no_candidates`, `address_resolution.identity_mismatch`

### Address Lookup Cache

`GossipAddressLookup` in `src/net/address_lookup.rs`:
- Time-bounded: 5-minute retention, 30-second eviction interval
- Populated from gossip Join/ForwardJoin messages
- Writes through to `FriendsStore` when the peer is a known friend (persisting discovered addresses)
- Implements `iroh::address_lookup::AddressLookup` trait

### Discovery Sources

| Source | Technology | Module |
|--------|-----------|--------|
| mDNS | LAN multicast | `iroh_mdns_address_lookup::MdnsAddressLookup` |
| DHT (public rooms) | Mainline DHT | `iroh_mainline_address_lookup::DhtAddressLookup` |
| DHT (private rooms) | Namespaced via DiscoverySecret | `private_room_tracker` |
| Memory | In-process bootstrap | `iroh::address_lookup::memory::MemoryLookup` |
| Gossip messages | Join/ForwardJoin propagation | `GossipAddressLookup` |

---

## 5. Connection Timeouts

| Layer | Timeout | Location |
|-------|---------|----------|
| Gossip dial (overall) | 15s cancellation | `net.rs:1264-1268` (`Dialer::queue_dial`) |
| Gossip dial (direct first) | 5s before relay fallback | `net.rs:1279-1281` |
| Address lookup resolution | 5s | `net.rs:771-774` |
| Backfill request/response | 5s | `backfill.rs:72` |
| Friend ping connect | 10s | `friend_ping.rs:42` |
| Topic join wait (tests) | 3–10s | Various test files |

The 15s dial cancellation and 5s direct→relay fallback are the two gossiplayer timeout mechanisms. Neither is configurable by end users.

---

## 6. File Sharing Networking

### Current Implementation (Whisper + iroh-blobs)

1. **Whisper `send_file()`** (`whisper/mod.rs:231`): sends a serialized `BlobTicket` (addr + hash + format) over a whisper DM session
2. **Receiver** parses the `BlobTicket` and calls `iroh_blobs::downloader::Downloader::download()` with provider candidates
3. **`download_blob_with_safety()`** in `chat_core.rs:2100+` wraps the download with progress events, safety checks, and cancellation
4. **`download_candidates()`** in `chat_core.rs:1999` builds provider list: original sender first, then online neighbors
5. **Profile images** are uploaded to blob store, then their `BlobTicket` is embedded in `UserProfile` and broadcast via `Message::ProfileUpdate`
6. **`SharedFileMeta`** (`chat_core.rs:917-932`): id, filename, size, mime_type, modified_time, hash — announced in `ProfileUpdate`
7. **`user_profile.rs`**: on-disk `profile.json` controls max file size (100MB), shared folder path, file enable/disable

### Planned Implementation (Catalogue + File Access)

The two unregistered ALPNs (`/boru-file-catalog/1`, `/boru-file-access/1`) and pre-written tests indicate a two-phase protocol:
1. **Catalogue retrieval**: signed, requester-filtered file listing over `/boru-file-catalog/1`
2. **Transfer authorisation**: request-time permission check that issues a short-lived signed download descriptor over `/boru-file-access/1`

The design doc (`protocol-layers.md:232-275`) specifies: 64KiB request cap, 1MiB response cap, 1000 entries max, 30 req/min rate limit, 5-minute descriptor expiry.

---

## 7. Profile-Update Messages

- **Variant**: `Message::ProfileUpdate(UserProfile)` at `chat_core.rs:914`
- **Throttle**: 30-second minimum interval (`ProfileUpdateThrottle`, `chat_core.rs:934-961`)
- **Contents**: `UserProfile` (display name, bio, avatar blob_id, avatar ticket, shared files list, preferences)
- **Transport**: Broadcast over gossip like any other room message
- **Frontend callback**: `ChatCallbacks::on_profile_update(peer, profile)` (`chat_callbacks.rs:297`)
- **Wire format**: Discriminant 13 (inserted mid-enum with backward-compat code at `chat_core.rs:4164-4167`)

---

## 8. Diagnostics & Observability

### `src/diagnostics.rs` — Core diagnostics singleton
- Bounded event store (5000 events, 1000 received probes, default)
- `DiagnosticEventKind` covers full lifecycle: discovery, address lookup, connection, subscription, probes
- `PeerDiagnosticState` tracks per-peer state machine
- `DiagnosticProbe` — wire-format probe sent through gossip mesh for latency testing
- `ReceivedProbe` — enhanced with delivery metadata and duplicate counting
- Thread-safe (sequence-numbered, retained in `VecDeque`, evicted by count)

### `src/observability.rs` — Documentation-only
- Tracing level conventions (trace=per-record, debug=success path, info=lifecycle, warn=recoverable, error=unrecoverable)
- Redaction rules: never log full discovery keys, decrypted record payloads, invitation material
- Safe identifiers: `short_id()` for rooms, `fmt_short()` for public keys

### `src/gossip_debug.rs` — Optional detailed tracing
- Enabled via `BORU_DEBUG=1`
- Append-only event log for mesh-forwarding debugging
- Auto-initialised by the gossip actor

### `src/perf.rs` — Performance instrumentation
- Timing samples, RAII timers
- Enabled via `BORU_PERF=1`

---

## 9. Obsolete or Overlapping Code Paths

### Already Removed
- **`tor_transport.rs`**: removed in `e753cc2` ("chore: remove dead tor_transport code")
- **`room_secret_migration` module**: commented out in `lib.rs:129`

### Potentially Dead
- **`src/outbox.rs.bak`**: orphaned backup file, not referenced from any module
- **Backward-compat whisper mailbox envelope variants**: docs note these exist "for compatibility with older peers" but the active path is inbox ALPN (`send_deliver`/`send_ack`). Frontends must not enqueue through both paths.

### Overlapping/duplicated capabilities
- **File sharing has two paths**: the live whisper+blobticket path and the planned catalogue+file-access path. Once catalogue+access is implemented, there will be two ways to share files — the old whisper-mediated approach and the new dedicated protocol. The design doc doesn't say whether the whisper path will be removed.
- **Discovery sources are redundant by design**: mDNS, DHT, GossipAddressLookup, and MemoryLookup all resolve addresses. This is intentional (tiered fallback), not an accidental duplication.

---

## 10. Potentially Reusable Code & Patterns

| Component | Reuse Potential |
|-----------|----------------|
| `GossipAddressLookup` (address_lookup.rs) | Reusable time-bounded address cache for any iroh app |
| `resolve_candidates()` (address_resolution.rs) | Policy-based address selection pattern — pluggable priority sources |
| `Diagnostics` singleton (diagnostics.rs) | Generic bounded event store with sequence numbering and probe tracking |
| `Dialer` (net.rs:1237) | Direct-then-relay dial strategy with cancellation — reusable for any protocol that needs connect fallback |
| Length-prefixed frame I/O (`net/util.rs:64-78`, `recv_loop`) | Reusable `read_frame`/`write_frame` primitives for postcard-over-stream |
| `ProfileUpdateThrottle` (chat_core.rs:939) | General monotonic debounce pattern |
| `DynamicPeerJoiner` (dynamic_joiner.rs) | Bounded peer join with dedup, backoff, and retry — room-agnostic |
| `Event` enum with diagnostic probes | Pattern for in-band health/latency measurement (DiagnosticProbe variant) |
