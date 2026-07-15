# Room Lifecycle & Bootstrap Flow — Architecture Audit

**Date:** 2026-07-12
**Task:** t_a0f62cb9 — CARD 01
**Auditor:** deepseek-coder
**Status:** Investigation only — no code changes made.

---

## 1. Scope of this audit

This note audits the **existing committed codebase** (HEAD commit 516a018) plus the **uncommitted working-tree changes** (`git diff HEAD` across 11 files). The previous ARCHITECTURE_BOOTSTRAP.md (authored by task t_43cfbb2a) is referenced where it still applies; this document extends it with gaps from the current checklist.

---

## 2. Ticket Definition & Encoding

**Covered by ARCHITECTURE_BOOTSTRAP.md §1** — no change.

- `Ticket` defined at `src/chat_core.rs:960-1004`: `{ topic: TopicId, peers: Vec<EndpointAddr> }`
- Encoding: `postcard::to_stdvec()` → `data_encoding::BASE32_NOPAD` (lowercase, no padding)
- Decoding: `BASE32_NOPAD.decode` → `Ticket::from_bytes()` → `postcard::from_bytes()`
- Tests at `src/chat_core.rs:2098+`

---

## 3. Room Topic Creation & Persistence

**Covered by ARCHITECTURE_BOOTSTRAP.md §2–3** — verified against current code.

| Scenario | Method | File:Line |
|---|---|---|
| New room (no saved topic) | `TopicId::from_bytes(rand::random())` | `chat.rs:297`, `iced_chat/main.rs:328` |
| Reopen saved room | `RoomStore::load_or_none()` → `store.topic` | `chat.rs:285`, `iced_chat/main.rs:315` |
| Join via ticket | `Ticket::from_str(ticket)` → `ticket.topic` | `chat.rs:312`, `iced_chat/main.rs:341` |
| Personal inbox topic | `blake3("iroh-chat-inbox/v1" + pubkey)[:32]` | `inbox.rs:283-288` (passive subscription) |

`RoomStore` at `src/room.rs:39-185` persists to `{data_dir}/room.json` via `atomic_write_json`.

---

## 4. RoomStore & Cached Bootstrap-Peer Handling

**Covered by ARCHITECTURE_BOOTSTRAP.md §3, §7** — verified against current code.

Key findings:
- `RoomStore::set_peers()` and `clear_peers()` persist immediately.
- `RoomStore::delete()` removes `room.json`.
- `Peer refresh` via `refresh_bootstrap_peers()` (chat_core.rs:88-116) collects `EndpointAddr` from `endpoint.remote_info()` after successful join.
- Two peer sources merged at subscribe time: ticket peers (from CLI) + RoomStore peers (from last session's refresh).

---

## 5. MemoryLookup Setup

**Covered by ARCHITECTURE_BOOTSTRAP.md §4** — confirmed.

- `MemoryLookup` (`iroh::address_lookup::memory::MemoryLookup`) created per-session in both TUI and GUI.
- Seeded via `seed_memory_lookup()` (chat_core.rs:71-78) before `subscribe()`/`subscribe_and_join()`.
- Registered onto endpoint address-lookup chain: `endpoint.address_lookup()?.add(memory_lookup)`.

---

## 6. mDNS, DHT/Pkarr, iroh-mainline-address-lookup Setup

**Covered by ARCHITECTURE_BOOTSTRAP.md §6 step 2** — extended here with current code:

Both TUI (`chat.rs`) and GUI (`iced_chat/main.rs`) compose the endpoint's `address_lookup` chain:

```
endpoint.address_lookup chain (order of add):
├── 1. MemoryLookup           — seeded with ticket/RoomStore addresses before subscribe
├── 2. n0 DNS/Pkarr           — from Endpoint::builder(presets::N0) preset (default)
├── 3. MdnsAddressLookup      — optional, best-effort LAN discovery
└── 4. DhtAddressLookup       — optional, Mainline DHT peer lookup (iroh-mainline-address-lookup v0.4)
```

- **TUI** (`chat.rs:373-400`): adds mDNS then DHT address lookup.
- **Iced GUI** (`iced_chat/main.rs:414-456`): adds MemoryLookup, mDNS, then DHT address lookup.

The `iroh-mainline-address-lookup` v0.4 crate wraps `n0-mainline` (Mainline DHT) internally — it does NOT use `distributed-topic-tracker`.

---

## 7. subscribe & subscribe_and_join

**Covered by ARCHITECTURE_BOOTSTRAP.md §5** — confirmed against current code.

**Current working-tree change:** `src/api.rs` now carries `our_endpoint_id` in `GossipApi` and `GossipSender`, enabling `join_peers()` filtering (self-skip, dedup, error reporting via `JoinSummary`).

The 30-second timeout pattern for `subscribe_and_join` is now in both TUI and GUI:

- TUI (`chat.rs:537-549`): `subscribe(topic, [])` if empty, else `timeout(30s, subscribe_and_join)`.
- Iced GUI (`app.rs:2537-2561`): same split, with additional `direct_conversation` check for 1:1 rooms.

---

## 8. Peer Refresh & Stale-Bootstrap Handling

**Covered by ARCHITECTURE_BOOTSTRAP.md §7** — confirmed.

Critical flow for stale-bootstrap resilience:
1. Initial join via ticket peers (may include stale addresses from offline creator).
2. After successful join, `refresh_bootstrap_peers()` stores live addresses.
3. On next reopen, live addresses from RoomStore are used instead of the (possibly-dead) ticket creator.
4. `tests/stale_bootstrap.rs` verifies this end-to-end with 3 peers.

---

## 9. TUI Room Creation/Join Flows

**Extends ARCHITECTURE_BOOTSTRAP.md §6.**

### TUI `open` flow (`chat.rs:276-310`):
1. `RoomStore::load_or_none(&data_dir)` → reuse topic + saved peers, OR `rand()` + `RoomStore::save()`.
2. Build endpoint with MemoryLookup + mDNS + DHT address lookup.
3. `collect_bootstrap_peers([saved_peers])` → dedup'd (ids, addrs).
4. `seed_memory_lookup(memory_lookup, addrs)`.
5. If `peer_ids.is_empty()`: `subscribe(topic, [])` — else `timeout(30s, subscribe_and_join(topic, ids))`.
6. Create `room_docs` (metadata + roster), forward events.
7. `refresh_bootstrap_peers()` → save live addresses to RoomStore.
8. Subscribe to personal inbox topic.

### TUI `join` flow (`chat.rs:311-316`):
1. `Ticket::from_str(ticket)` → `{ topic, peers }`.
2. Same bootstrap flow as `open` but with peers from ticket.
3. RoomStore saved with combined ticket+refresh peers.

### TUI `public` flow:
- Resolves to `PUBLIC_LOBBY_TOPIC` (blake3("iroh-gossip-chat/default-lobby/v1")), empty bootstrap.
- Skips RoomStore save. Subscribes passively.

---

## 10. Iced GUI Room Creation/Join Flows

**Extends ARCHITECTURE_BOOTSTRAP.md §6.**

### Iced GUI `OpenRoom(topic)` flow (`app.rs:2473-2638`):
1. Save current room to history, drain bootstrap peers.
2. `collect_bootstrap_peers([initial_addrs, saved_peers])`.
3. `seed_memory_lookup(memory_lookup, bootstrap_addrs)`.
4. **Routing decision** (`app.rs:2542-2561`):
   - `direct_conversation || peers.is_empty()` → `subscribe(topic, peers)` (no wait).
   - `subscribe_and_join` with 30s timeout otherwise.
5. Create `room_docs`, broadcast AboutMe + Presence.
6. Save RoomStore with combined peers.

### Iced GUI `JoinFromTicket` flow (`app.rs:2717-2856`):
1. Parse ticket string → `{ topic, peers }`.
2. `collect_bootstrap_peers([ticket.peers, saved_addrs])`.
3. `seed_memory_lookup(memory_lookup, bootstrap_addrs)`.
4. `subscribe_and_join(topic, peers)` with 30s timeout.
5. Same post-subscribe steps as Open flow.

### Iced GUI `NewRoom` (`app.rs:2340-2470`):
1. Generate random TopicId.
2. `subscribe(topic, [])` (no bootstrap — we're the creator).
3. Create room_docs, broadcast AboutMe.
4. Save RoomStore with own local address.

---

## 11. Shared State Representation

### IcedChat struct (`examples/iced_chat/app.rs:721+`)
```
IcedChat {
    // Navigation
    screen: Screen,                   // ChatList | Chat{topic} | Help | Settings | FriendRequests | LogViewer
    pending_topic: Option<TopicId>,   // topic being connected to
    
    // ChatList state
    room_history: RoomHistoryStore,   // transient room list (never persisted)
    join_ticket_input: String,        // ticket text field
    
    // Chat state (active room)
    topic: TopicId,                   // current room's gossip topic
    ticket_str: String,               // current room's ticket
    sender: Option<GossipSender>,     // active GossipSender for the current room
    entries: Vec<ChatEntry>,          // chat messages for current room
    names: HashMap<PublicKey, String>, // peer display names
    
    // Network
    gossip: Gossip,
    endpoint: iroh::Endpoint,
    memory_lookup: MemoryLookup,
    forward_handle: Option<JoinHandle>, // room_docs forwarding task
    
    // Peers
    neighbors: HashSet<PublicKey>,
    mesh_health: MeshHealth,
}
```

**Room switching** (`app.rs:2473-2500`): leaves current room (saves history, tries save chat history, drops sender), then subscribes to new topic. The `GossipTopic` is split into `(sender, receiver)` — sender is stored in `IcedChat`, receiver feeds `forward_gossip_events` which translates gossip messages into `NetEvent` on the shared `net_tx` channel.

### TUI chat state (`chat.rs`):
- Uses a basic `AppState` struct with `entries`, `names`, `ticket`, `gossip`, `sender` fields.
- No screen enum — single chat view with status bar.
- Bootstrap peers stored in a local `Vec<EndpointAddr>` from `RoomStore::load_or_none()`.

---

## 12. Test Coverage

**Covered by ARCHITECTURE_BOOTSTRAP.md §8** — extended here:

### Current test matrix:

| Test | File | What it covers | Features |
|---|---|---|---|
| `stale_bootstrap` | `tests/stale_bootstrap.rs` | 3-peer stale bootstrap resilience, peer refresh, RoomStore roundtrip | net, test-utils |
| `room_e2e` | `tests/room_e2e.rs` | 2-peer subscribe_and_join, metadata/roster sync | net, test-utils |
| `three_peer_mesh` | `tests/test_three_peer_mesh.rs` | Full mesh routing, doc sync, --no-relay | net, test-utils |
| `test_two_peers_with_relay` | `tests/test_two_peers_relay.rs` | Relay-based gossip connectivity | net, test-utils |
| `test_two_peers_exchange` | `tests/test_two_peers_exchange.rs` | Direct message roundtrip (MemoryLookup) | net, test-utils |
| `test_no_bootstrap` | `tests/test_no_bootstrap.rs` | Empty bootstrap subscribe | net, test-utils |
| `test_multi_peer_discovery` | `tests/test_multi_peer_discovery.rs` | 6 test cases: discovery, empty, late arrival, stale+valid, malformed, shutdown | net, test-utils |

### Shutdown coverage:
- `spawn_peer` in `stale_bootstrap.rs` drops router + gossip → cleanup via `Drop`.
- `test_multi_peer_discovery.rs` tests clean shutdown via `TopicDiscoveryBackend::shutdown()`.
- No explicit `Router::shutdown()` test exists — teardown relies on `Drop`.
- **Gap:** No test verifies graceful shutdown ordering (router → gossip → endpoint).

### Multi-peer join coverage:
- `test_multi_peer_discovery.rs` (InMemoryDiscoveryBackend) covers peer discovery with 0, 1, 2, and late-arriving peers.
- `stale_bootstrap.rs` covers 3-peer mesh with creator-offline rejoin.
- `test_three_peer_mesh.rs` covers full mesh message routing.

### Room persistence tests:
- RoomStore unit tests in `src/room.rs:188-275` (load/save/delete roundtrip, corruption, reopen).
- `stale_bootstrap.rs` verifies JSON roundtrip of RoomStore across peer lifecycle.

---

## 13. distributed-topic-tracker Analysis

**Covered by ARCHITECTURE_BOOTSTRAP.md §9** — key findings replicated here for reference:

- **v0.3.5, MIT license**, repo: https://github.com/rustonbsd/distributed-topic-tracker
- Has `iroh-gossip` integration (default feature) and standalone DHT/crypto modules.
- `boru-chat` currently does **not** depend on `distributed-topic-tracker` in its committed Cargo.toml.
- The `iroh-mainline-address-lookup` v0.4 crate used in both examples wraps `n0-mainline`, not `distributed-topic-tracker`.

**If DHT-based bootstrap peer discovery is wanted**, option 3 (use DHT + crypto modules with `default-features = false`) is cleanest — zero coupling to iroh-gossip.

---

## 14. Generic DHT/Crypto/Record Primitives Availability

**Covered by ARCHITECTURE_BOOTSTRAP.md §9** — confirmed:

Available from `iroh` (committed deps):
- `iroh::SecretKey` — ed25519 signing
- `iroh_base::PublicKey`, `EndpointId`
- `iroh::EndpointAddr`, `RelayUrl`
- `Endoint.address_lookup()` chain with `MemoryLookup`, `MdnsAddressLookup`, `DhtAddressLookup`
- `iroh::Signature`

Available from `distributed-topic-tracker` (if added as dep with `default-features = false`):
- `Dht` — Mainline DHT client (get/put_mutable)
- `DhtConfig`
- Record types for signed/encrypted DHT records
- ed25519 key derivation from discovery secrets

Available from current crate:
- `blake3` — hashing (already a dep)
- `postcard` — serialization (already a dep)
- `x25519-dalek` + `aes-gcm` — encryption (already in deps)

---

## 15. Dependency Compatibility Summary

### Direct dependencies (committed Cargo.toml + working-tree changes):

| Dependency | boru-chat version | Status |
|---|---|---|
| `iroh` | `= "1"` | Stable, no breaking changes expected |
| `iroh-base` | `= "1"` | Stable |
| `iroh-blobs` | `= "0.103"` | Stable |
| `iroh-mdns-address-lookup` | `= "0.4"` | Stable |
| `iroh-mainline-address-lookup` | `= "0.4"` | Stable |
| `iroh-metrics` | `= "1.0.1"` | Stable |
| `tokio` | `= "1"` | Stable |
| `postcard` | `= "1"` (with `experimental-derive`) | Stable |
| `serde` | `= "1.0.164"` | Stable |
| `ed25519-dalek` | _(not a direct dep)_ | Used via `iroh`'s deps |
| `mainline`/`n0-mainline` | _(not a direct dep)_ | Used via `iroh-mainline-address-lookup`'s deps |
| `x25519-dalek` | `= "2.0.1"` | Stable |
| `blake3` | `= "1.8"` | Stable |
| `aes-gcm` | `= "0.10.3"` | Stable |
| `rand` | `= "0.10.1"` | Stable |
| `iced` | `= "0.14"` | GUI feature only |
| Rust edition | `2021` | Stable |
| MSRV | `1.91` | Comitted |

### If adding `distributed-topic-tracker` (v0.3.5) with `default-features = false`:

| Dep | boru-chat | d-t-t | Compat? |
|---|---|---|---|
| `iroh` | `= "1"` | `= "1"` (optional) | ✓ |
| `tokio` | `= "1"` | `= "1"` | ✓ |
| `serde` | `= "1.0.164"` | `= "1"` | ✓ |
| `rand` | `= "0.10.1"` | `= "0.10"` | ✓ |
| `ed25519-dalek` | _(not direct)_ | `= "3.0.0-rc.0"` | **New transitive dep** |
| `mainline` | _(not direct)_ | `= "7"` | **New transitive dep** |

**Potential version conflicts:** None identified. The `d-t-t` crate's optional `iroh` dep matches `= "1"`. Minimal risk with `resolver = "2"`.

---

## 16. Licence Compatibility

| Crate | License |
|---|---|
| `boru-chat` | MIT / Apache-2.0 (dual) |
| `iroh` (all n0 crates) | MIT / Apache-2.0 (dual) |
| `distributed-topic-tracker` | MIT |
| `postcard` | MIT / Apache-2.0 |
| `serde` | MIT / Apache-2.0 |
| `blake3` | CC0-1.0 / Apache-2.0 |
| `tokio` | MIT |
| `iced` | MIT |
| `ed25519-dalek` | BSD-3-Clause |
| `aes-gcm` | Apache-2.0 / MIT |
| `x25519-dalek` | BSD-3-Clause |
| `mainline` (n0 variant) | MIT / Apache-2.0 |

**Verdict:** All dependencies are permissive open-source (MIT, Apache-2.0, BSD-3-Clause, CC0). No licence conflicts. The crate's dual MIT/Apache-2.0 licensing is compatible with every listed dependency.

---

## 17. Integration Points — Exact Files & Functions to Change

If implementing public-room DHT discovery (the next card's scope), these are the integration points:

### Files to modify:
| File | What to change | Purpose |
|---|---|---|
| `Cargo.toml` | Add `distributed-topic-tracker = { version = "0.3.5", default-features = false }` dep | DHT dependency |
| `src/lib.rs` | Add `pub mod discovery;` and `pub mod public_room;` modules (already added in working tree) | Module declarations |
| `src/api.rs` | Use new `our_endpoint_id` field in `GossipSender` for `join_peers()` self-filter | Filter out local peer ID |
| `src/chat_core.rs` | Add `public_room_discovery_secret()` helper | Generate deterministic discovery key per room |
| `examples/chat.rs` | Wire `TopicTracker` or `PublicRoomTracker` into the bootstrap flow | Background DHT discovery |
| `examples/iced_chat/app.rs` | Wire into `IcedChat::new()` and room join tasks | Background DHT discovery for GUI |
| `examples/iced_chat/main.rs` | Pass discovery config to app | Supply config at startup |

### New files to create (if DHT discovery is added):
| File | Purpose |
|---|---|
| `src/discovery/mod.rs` | Module header, re-exports |
| `src/discovery/topic_tracker.rs` | Mainline DHT wrapper for publish/discover |
| `src/discovery/validation.rs` | Discovery record validation |
| `src/discovery/public_record.rs` | PublicDiscoveryRecord type and signing |
| `src/discovery/backend.rs` | Injectable backend trait (in-memory + DHT) |
| `src/discovery/public_room_tracker.rs` | Backend-agnostic public room tracker |
| `src/discovery/namespace.rs` | Deterministic namespace derivation |
| `src/discovery/invite.rs` | Invite-based peer migration |
| `src/public_room.rs` | Canonical public-room identity constants |
| `src/public_room_config.rs` | Centralised public-room safety limits |
| `src/topic_derivation.rs` | Deterministic TopicId derivation (already exists) |
| `src/room_docs.rs` | Room metadata/roster document sync (already exists) |

---

## 18. Decision: Adapter, Patch, or Fork?

**Recommendation: Use distributed-topic-tracker with `default-features = false` — no fork needed.**

**Rationale:**
1. We only need the `Dht` client (get/put_mutable) + `DhtConfig`.
2. The `iroh-gossip` feature of d-t-t is tightly coupled to `iroh_gossip::net::Gossip` — which our fork (`boru_chat::net::Gossip`) mirrors structurally but is a different type.
3. We already have our own record model (`PublicDiscoveryRecord`) and validation pipeline — we don't need d-t-t's record types.
4. The `Dht` client is a standalone Mainline DHT wrapper with no iroh dependency — it can be used directly.

**Small adapter needed:** A thin `TopicTracker` wrapper around `distributed_topic_tracker::Dht` that:
- Owns the DHT handle and derived key material (seed, salt)
- Exposes `publish_once()` and `discover_once()` using our `PublicDiscoveryRecord` type
- Spawns background `publish_loop` / `discovery_loop` tasks via `start_continuous()`
- Uses the existing `PublicRecordValidator` for record validation
- Uses `CancellationToken` for lifecycle management

**No fork.** No patches to vendored crates. No `[patch.crates-io]` entries needed (compare: `iroh-dns` is patched for Windows DoH — we don't need that).

---

## 19. Shutdown & Lifecycle Assessment

**Current shutdown pattern (all test harnesses and examples):**
1. Drop `GossipTopic` handle → gossip actor processes `Quit`.
2. Drop `Router` → stops accepting connections.
3. Drop `Endpoint` → tears down QUIC connections.
4. Tokio runtime drops remaining tasks.

**Gap:** No explicit `Router::shutdown()` ordering — relies on `Drop`. The `IROH_AUDIT.md` (F-02) notes that backfill, friend-ping, and whisper tasks lack explicit abort paths. If deterministic shutdown is required, add explicit shutdown calls in order: cancel background tasks → drop gossip → shutdown router → drop endpoint.

---

## 20. Summary of Findings

| # | Finding | Status |
|---|---|---|
| 1 | Ticket type, encoding, decoding fully documented | ✓ |
| 2 | Room topic generation and persistence covered | ✓ |
| 3 | RoomStore + cached bootstrap peers handled | ✓ |
| 4 | MemoryLookup seeded before subscribe | ✓ |
| 5 | mDNS + DHT + Pkarr address lookup chain established | ✓ |
| 6 | subscribe / subscribe_and_join split with 30s timeout | ✓ |
| 7 | Peer refresh after join persists live addresses | ✓ |
| 8 | TUI and Iced room creation/join flows documented | ✓ |
| 9 | Shared state representation (IcedChat, AppState) documented | ✓ |
| 10 | Test coverage adequate for multi-peer, stale, persistence | ✓ |
| 11 | distributed-topic-tracker API assessed | ✓ |
| 12 | Generic DHT/crypto primitives available without iroh-gossip integration | ✓ |
| 13 | Dependency compatibility: all compatible | ✓ |
| 14 | Licence: all permissive, no conflicts | ✓ |
| 15 | Adapter (not fork) recommended — ~200 lines of wrapper | ✓ |
