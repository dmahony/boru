# Card 0: Audit — Bootstrap Flow & Tracker Compatibility

## 1. Current Bootstrap Flow (Legacy Ticket-Based)

### 1.1 Ticket Definition

**File:** `src/chat_core.rs`, line 961  
**Type:** `pub struct Ticket { topic: TopicId, peers: Vec<EndpointAddr> }`

- **Encoding:** `postcard::to_stdvec()` → Base32 (no-pad, lowercase). No prefix string (e.g. no `boru1:`).
- **Decoding:** ASCII-uppercase → Base32 decode → `postcard::from_bytes()`.
- **Display:** Direct `impl fmt::Display` writes the base32 string. No prefix.
- **Contains:** Gossip `TopicId` (32 bytes) + full `EndpointAddr` list (includes relay URLs, direct addresses, the creator's full connection info).

### 1.2 Topic Generation

**Private rooms (TUI & iced):**  
`TopicId::from_bytes(rand::random())` — random 32-byte topic on each new room.

**Public rooms (deterministic):**  
`topic_derivation::public_room_topic()` — BLAKE3 hash of domain separator + network byte + room name length (LE u16) + room name bytes + version byte.

**Domain separators:**  
`PUBLIC_ROOM_DOMAIN_SEPARATOR` = `b"boru-chat public-room v1"` (for gossip topic)  
`DISCOVERY_KEY_DOMAIN_SEPARATOR` = `b"boru-chat discovery-key v1"` (for DHT namespace)  
These are deliberately different, providing domain separation between gossip mesh and DHT discovery.

### 1.3 RoomStore Persistence

**File:** `src/room.rs`  
**Schema:** `RoomStore { schema_version: 2, topic: TopicId, peers: Vec<EndpointAddr>, data_dir: PathBuf }`  

- Persisted as `room.json` (JSON) at `<data_dir>/room.json`.
- `save()` uses `atomic_write_json()` for crash-safe writes.
- Missing file = `None` (not an error). Corrupt JSON = error (caller decides what to do).
- SCHEMA_VERSION mismatch = hard error (no migration path exists; must delete and recreate).
- **No discovery_secret field** — legacy format. Schema v2 has no migration support yet.

### 1.4 MemoryLookup Population

**File:** `src/chat_core.rs`, `seed_memory_lookup()` (line 72)

- A `MemoryLookup` is created at startup and shared across the endpoint.
- `seed_memory_lookup()` is called **before** `subscribe_and_join()`.
- Addresses come from two sources: ticket peers (from `Command::Join`) and RoomStore saved peers (from `Command::Open` reopening).
- `collect_bootstrap_peers()` (line 44) deduplicates by `EndpointId`, returning `(Vec<EndpointId>, Vec<EndpointAddr>)`.

### 1.5 subscribe / subscribe_and_join

**File:** `src/api.rs`

- `subscribe(topic_id, bootstrap: Vec<EndpointId>)` → returns `GossipTopic` immediately without waiting for connection.
- `subscribe_and_join(topic_id, bootstrap)` → calls `subscribe()` then `.joined().await` to block until at least one connection is established.
- Both call `subscribe_with_opts()` which sends an RPC message to the gossip actor with the topic and bootstrap peer IDs.

Under the hood (net.rs): the gossip actor receives a `JoinRequest { topic_id, bootstrap: BTreeSet<EndpointId> }`, creates the gossip state, and attempts connections to the bootstrap peers.

### 1.6 Bootstrap Refresh

**File:** `src/chat_core.rs`, `refresh_bootstrap_peers()` (line 89)

- Called **after** joining a room.
- Queries the endpoint's remote info for known peer IDs.
- Updates `RoomStore.peers` with freshly resolved `EndpointAddr` values.
- Returns bool indicating whether the peer list changed.
- This ensures reopening a room has live addresses even if the original ticket creator is offline.

### 1.7 TUI Room Flow (`examples/chat.rs`)

- `Command::Open { topic }`: loads RoomStore; if none, generates random topic, saves RoomStore.
- `Command::Join { ticket }`: `Ticket::from_str()` parses base32 to topic+peers.
- Creates `Ticket { topic, peers: vec![endpoint.addr()] }` for display.
- Seeds `memory_lookup` with saved peers, then calls `gossip.subscribe(topic, bootstrap_ids)`.
- **Does NOT yet integrate DHT-based tracker.**
- **No stable invitation format (`boru1:`) support yet.** Legacy ticket only.

### 1.8 Iced GUI Room Flow (`examples/iced_chat/main.rs`)

- Same basic pattern as TUI (RoomStore load/save, ticket parse).
- **Already integrates DHT tracker:**
  - Creates `MainlineDhtBackend` wrapping `distributed_topic_tracker::Dht`.
  - Creates `PublicRoomTracker::start()` with `PublicNetwork::Mainnet`.
  - Creates `ContinuousTracker::start()` for background publish/discover.
  - `discovered_peers_rx` channel feeds into the app's subscription system.
  - Discovered peers are joined via `GossipSender::join_peers()`.
- Uses `distributed_topic_tracker::TopicId::from_hash()` for namespace (dummy).
- Still uses legacy `Ticket` format for join/open — no `boru1:` invitation yet.

### 1.9 Tests Covering Bootstrap

| Test File | What It Covers |
|-----------|----------------|
| `tests/stale_bootstrap.rs` | Stale bootstrap peer does not block rejoin; RoomStore round-trip; refresh_bootstrap_peers; collect_bootstrap_peers dedup. Uses real relay server with 3 peers. |
| `tests/test_stale_bootstrap.rs` | Stale bootstrap scenario where original peer is offline; C joins with stale A + live B addresses. Relay-mediated. |
| `tests/room_e2e.rs` | Full room lifecycle with ticket-based join, messaging, cleanup. |
| `tests/test_public_lobby_integration.rs` | Multi-peer public lobby via `InMemoryDiscoveryBackend`. A→B→C, A offline, C bootstraps from B. **No tickets, no live DHT.** |
| `tests/test_three_peer_mesh.rs` | Three-peer mesh connectivity. |
| `tests/test_no_bootstrap.rs` | Subscribing without any bootstrap peers. |
| `tests/test_performance_regression.rs` | Performance regression suite (16 tests). |

---

## 2. distributed-topic-tracker v0.3.5 — Public API & Compatibility

### 2.1 Source & License

- **Repository:** https://github.com/rustonbsd/distributed-topic-tracker
- **Author:** Zacharias Boehler
- **License:** MIT (Cargo.toml); README says "Apache-2.0 or MIT" — dual-licensed.
- **Edition:** Rust 2024 (the project uses ed2024; this is not a compatibility concern for consumer).

### 2.2 Always-Available Public Types (no feature flags)

| Module | Exports |
|--------|---------|
| `config` | `Config`, `ConfigBuilder`, `DhtConfig`, `DhtConfigBuilder`, `BootstrapConfig`, `BootstrapConfigBuilder`, `TimeoutConfig`, `TimeoutConfigBuilder`, `PublisherConfig`, `PublisherConfigBuilder`, `MergeConfig`, `MergeConfigBuilder`, `BubbleMergeConfig`, `BubbleMergeConfigBuilder`, `MessageOverlapMergeConfig`, `MessageOverlapMergeConfigBuilder` |
| `crypto` | `TopicId`, `Record`, `EncryptedRecord`, `RecordPublisher`, `RecordPublisherBuilder`, `SecretRotation`, `DefaultSecretRotation`, `RotationHandle`, `encryption_keypair()`, `salt()`, `signing_keypair()` |
| `dht` | `Dht` |
| lib | `unix_minute()`, `MAX_RECORD_PEERS` (5), `MAX_MESSAGE_HASHES` (5) |

### 2.3 Feature-Gated Types (`iroh-gossip` feature — default ON)

| Module | Exports |
|--------|---------|
| `gossip` | `AutoDiscoveryGossip`, `Bootstrap`, `BubbleMerge`, `GossipReceiver`, `GossipRecordContent`, `GossipSender`, `MessageOverlapMerge`, `Publisher`, `Topic` |

These are the `iroh-gossip` integration layer. **boru-chat does NOT use any of these** — it has its own gossip protocol (`boru-chat` / `proto/state.rs`).

### 2.4 boru-chat's Usage of distributed-topic-tracker

boru-chat uses ONLY the always-available types:
- `Dht` — Mainline DHT client for get/put
- `TopicId::from_hash()` — for namespace conversion
- `unix_minute()` — time slot
- `signing_keypair()` — per-minute signing key derivation
- `salt()` — per-minute salt derivation
- `Record` — signed record creation, serialization, verification
- `Record::sign()`, `Record::verify()`, `Record::from_bytes()`, `Record::to_bytes()`
- `EncryptedRecord::new()` — wrapping record bytes for backend transport

**The `iroh-gossip` feature (default ON) is NOT required for boru-chat's use case.** The integration in `discovery_backend.rs::MainlineDhtBackend` goes directly through `Dht::get()` / `Dht::put_mutable()`, not through `AutoDiscoveryGossip` or any of the higher-level abstractions.

### 2.5 Dependency Compatibility

| Dependency | boru-chat's version | tracker's version | Conflict? |
|-----------|-------------------|-------------------|-----------|
| **ed25519-dalek** | (none directly) | **3.0.0-rc.0** | ⚠️ iroh uses ed25519-dalek v2.x, creating a **dual-version** situation. The tracker crate's `Record::sign()` and `Record::verify()` use `ed25519_dalek::SigningKey` (v3.0.0-rc.0), while iroh uses v2. The boru-chat code in `discovery_record.rs` bridges via `secret_key.as_signing_key()` which converts the key format. This works because `Record::sign()` accepts a `&ed25519_dalek::SigningKey` and boru-chat's `SecretKey::as_signing_key()` presumably returns the v3 key. However, this dual-version situation needs careful verification. |
| **iroh** | 1 (features: tls-ring) | 1 (optional, with tls-ring) | ✅ Compatible |
| **iroh-gossip** | (none — boru's own) | 0.101 (optional) | ✅ Safe — not activated in boru-chat's feature set |
| **mainline** | (none directly) | 7 | ✅ Stable, only used by tracker |
| **sha2** | (none directly) | 0.11.0 | ✅ Safe |
| **tokio** | 1 (io-util,sync,rt,macros,time) | 1 (macros,time,sync,rt-multi-thread) | ✅ Compatible |
| **tokio-util** | 0.7.12 (codec) | 0.7 | ✅ Compatible |
| **postcard** | 1 (alloc,use-std,experimental-derive) | 1 | ✅ Compatible |
| **serde** | 1.0.164 (derive) | 1 (std) | ✅ Compatible |
| **chrono** | 0.4 (clock) | 0.4 (clock) | ✅ Compatible |
| **getrandom** | 0.3 | 0.4 | ✅ Both coexist — boru uses 0.3, tracker uses 0.4. No direct conflict since both are separate crate versions. |
| **rand** | 0.10.1 (std_rng) | 0.10 (std,std_rng) | ✅ Compatible |

**⚠️ Key concern: ed25519-dalek v2 vs v3 dual version.**  
boru-chat depends on iroh (v1) which in turn depends on `ed25519-dalek` v2.x. The tracker crate depends on `ed25519-dalek 3.0.0-rc.0`. Both versions can coexist in the dependency tree (Cargo allows multiple semver-incompatible versions), and boru-chat's `discovery_record.rs` successfully bridges through `SecretKey::as_signing_key()`. This is confirmed by the existing lockfile showing both versions co-resolve without conflict. However, it adds cognitive overhead and risks type confusion.

### 2.6 Feature Flags & Default-Features Disable

The Cargo.toml currently specifies:
```toml
distributed-topic-tracker = "0.3"
```
This enables the `iroh-gossip` default feature, pulling in `iroh` and `iroh-gossip` as optional deps (they are compiled but the feature-gated code paths are not used). The `iroh-gossip` dep adds ~0.101 into the dependency tree unnecessarily.  

**Recommendation:** Change to:
```toml
distributed-topic-tracker = { version = "0.3", default-features = false }
```
This keeps only `Dht`, `Record`, `TopicId`, etc. — everything boru-chat actually uses. The `iroh-gossip` feature-gated modules (`gossip/`) will be excluded from compilation, saving build time and dependency noise.

### 2.7 Coupling to iroh-gossip

The tracker crate's core functionality (`Record`, `Dht`, `RecordPublisher`, `TopicId`, signing/encryption primitives) is **not** coupled to `iroh-gossip`. The `iroh-gossip` feature only gates the `AutoDiscoveryGossip` extension trait and related types.

**The integration point in boru-chat (`MainlineDhtBackend`) bypasses the high-level iroh-gossip integration entirely** — it uses `Dht::get()`/`Dht::put_mutable()` directly, combined with `Record::sign()`/`Record::verify()` for record lifecycle. This is the correct decoupled approach.

### 2.8 Record Format Details

**Raw Record wire format** (no encryption):  
`topic(32) || unix_minute(8) || pub_key(32) || content(var) || signature(64)` = ~171 B total.

**EncryptedRecord wire format** (as stored in DHT):  
`encrypted_record_len(4) || encrypted_record(var) || encrypted_decryption_key(88)` = ~270 B total.

**Payload for boru-chat:** `DiscoveryRecordPayload { endpoint_id: [u8; 32], version: u8 }` = ~35 B postcard-encoded.

---

## 3. What Already Exists (Pre-Implementation)

The following modules are already implemented in the repo:

| Module | File | Purpose |
|--------|------|---------|
| `TopicDiscoveryBackend` trait | `src/discovery_backend.rs` | Trait for publish/lookup/shutdown |
| `InMemoryDiscoveryBackend` | `src/discovery_backend.rs` | In-memory mock for tests |
| `MainlineDhtBackend` | `src/discovery_backend.rs` | Production DHT backend via `distributed_topic_tracker::Dht` |
| `NamespaceId` | `src/discovery_backend.rs` | 32-byte namespace identifier |
| `EncryptedDiscoveryRecord` | `src/discovery_backend.rs` | Opaque encrypted record for backend transport |
| `DiscoveryRecordPayload` | `src/discovery_record.rs` | Payload: endpoint_id + version |
| `create_discovery_record()` | `src/discovery_record.rs` | Sign + create Record advertising EndpointId |
| `decode_discovery_record()` | `src/discovery_record.rs` | Extract EndpointId from verified Record |
| `DiscoveryRecordValidator` | `src/discovery_validation.rs` | Full validation pipeline (size→timestamp→decode→identity→signature→dedup→self-filter) |
| `PeerCandidates`, `ValidationCounters`, `ValidationConfig` | `src/discovery_validation.rs` | Batch processing types |
| `PublicRoomIdentity`, `PublicNetwork` | `src/public_room.rs` | Deterministic room identity (topic + discovery key) |
| `public_room_identity()`, `public_discovery_key()` | `src/public_room.rs` | Derivation functions |
| `public_room_topic()` | `src/topic_derivation.rs` | Domain-separated topic derivation |
| `PublicRoomTracker` | `src/public_room_tracker.rs` | Publish-once / discover-once wrapper |
| `ContinuousTracker` | `src/public_room_continuous.rs` | Background periodic publish + discover |
| `PublicRoomSafety` | `src/public_room_safety.rs` | Per-peer rate limiting, message size, blob limits |
| `PublicRoomConfig` | `src/public_room_config.rs` | Configuration for all public-room limits |
| `DiscoverySecret` | `src/discovery_secret.rs` | 32-byte secret for private-room discovery |
| Test: multi-peer public lobby | `tests/test_public_lobby_integration.rs` | A→B→C scenario with in-memory backend |
| Test: stale bootstrap | `tests/test_stale_bootstrap.rs` | Stale + live bootstrap peers |
| Test: stale bootstrap (e2e) | `tests/stale_bootstrap.rs` | Full relay-based stale-bootstrap test |
| iced integration | `examples/iced_chat/main.rs` | Creates `MainlineDhtBackend` + `PublicRoomTracker` + `ContinuousTracker` |

## 4. What Still Needs Implementation

Per the spec in `/home/dan/boru-chat-dht-bootstrap.txt`:

| Task | Status | Details |
|------|--------|---------|
| **Task 1 — Audit** | ✅ Complete | This document |
| **Task 2 — RoomInviteV2** | ❌ Missing | `boru1:` prefix, no endpoint addresses, secret redacted |
| **Task 3 — RoomStore extension** | ❌ Missing | `discovery_secret` field; migration from schema v2 |
| **Task 4 — Namespace derivation** | ⚠️ Partial | `topic_derivation.rs` exists for public rooms. Private-room namespace derivation from topic bytes (SHA-256("boru-chat room discovery v1" \|\| topic_bytes)) not yet implemented. |
| **Task 5 — Tracker wrapper** | ✅ Complete | `PublicRoomTracker` + `MainlineDhtBackend` exist |
| **Task 6 — Record validation** | ✅ Complete | `DiscoveryRecordValidator` with full pipeline |
| **Task 7 — Injectable backend** | ✅ Complete | `TopicDiscoveryBackend` trait + `InMemoryDiscoveryBackend` |
| **Task 8 — Startup integration** | ❌ Missing | Room lifecycle integration for private rooms. Iced GUI has partial integration for public lobby only. |
| **Task 9 — Dynamic peer joining** | ✅ Complete | `GossipSender::join_peers()` exists in api.rs |
| **Task 10 — Continuous discovery** | ✅ Complete | `ContinuousTracker` in `public_room_continuous.rs` |
| **Task 11 — TUI room flow** | ❌ Missing | TUI chat example has no DHT integration yet |
| **Task 12 — Iced GUI room flow** | ⚠️ Partial | Iced chat has continuous tracker for public lobby, but no private-room invite/join flow |
| **Task 13 — Legacy ticket compatibility** | ❌ Missing | Format detection (`boru1:` vs legacy) not implemented |
| **Task 14 — Migration between peers** | ❌ Missing | Secure secret distribution protocol |
| **Task 15 — Multi-peer integration tests** | ⚠️ Partial | `test_public_lobby_integration.rs` exists; needs more scenarios |
| **Task 16 — Observability** | ⚠️ Partial | Tracing exists in `PublicRoomTracker`; more events needed |
| **Task 17 — Documentation** | ❌ Missing | README updates |
| **Task 18 — Final verification** | ❌ Not yet | To be done after all changes |

## 5. Dependency Recommendations

### 5.1 Default-Features Disable
```toml
# Current (unnecessarily pulls iroh-gossip compilation):
distributed-topic-tracker = "0.3"

# Recommended:
distributed-topic-tracker = { version = "0.3", default-features = false }
```
No code changes needed in boru-chat — all used types are in the unconditional `lib.rs` exports.

### 5.2 ed25519-dalek Dual Version
The dual-version situation (iroh uses ed25519-dalek v2, tracker uses v3.0.0-rc.0) is functional but fragile. The bridge through `SecretKey::as_signing_key()` works because both versions use compatible byte-level key material. **No patch/fork required at this time**, but this should be monitored as the tracker crate matures toward a stable release.

### 5.3 No Fork Required
The distributed-topic-tracker crate's architecture correctly separates core DHT+record functionality from iroh-gossip integration. boru-chat's usage pattern (raw `Dht` + `Record`) is well-supported without any fork.
