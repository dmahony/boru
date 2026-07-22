# DHT Audit — Discovery Architecture & Iroh API Baseline

Date: 2026-07-21
Scope: Task DHT-1 (t_af7f001d)
Parent: t_9dd02beb (LAN conversation delivery & persistence)

---

## 1. Architecture Overview

Two **independent** DHT systems coexist; they serve different purposes and
use separate crates:

### Layer 1: Address Resolution (`iroh-mainline-address-lookup`)

```
EndpointId ──→ DhtAddressLookup ──→ EndpointAddr (relay + direct IPs)
```

- **Crate**: `iroh-mainline-address-lookup = "0.4"`
- **Type**: `DhtAddressLookup` (implements `iroh::address_lookup::AddressLookup`)
- **Purpose**: Resolve an `EndpointId` to transport addresses (relay URLs, direct IPs)
  so iroh can dial the peer. Uses Pkarr (signed DNS packets on Mainline DHT).
- **Usage**: `main.rs:496-504` — registered on the iroh `Endpoint`'s address lookup chain.
- **Gating**: `--no-dht` flag suppresses registration.
- **Default filter**: `AddrFilter::relay_only()` — only relay URLs are published,
  not direct IPs (privacy).
- **Underlying DHT**: `n0_mainline::Dht` (a patched mainline v7.0.0).

### Layer 2: Topic Discovery (`distributed-topic-tracker`)

```
TopicId ──→ MainlineDhtBackend ──→ Vec<EndpointId> (peer candidates)
```

- **Crate**: `distributed-topic-tracker = "0.3.5"`
- **Types**: `MainlineDhtBackend` (wraps `distributed_topic_tracker::Dht`)
- **Purpose**: Discover which peers are part of a room by looking up signed records
  published under a room-specific namespace. Returns `EndpointId` values (not
  transport addresses).
- **Splits into two subsystems**:

  | Subsystem | Tracker | Background Loop | CLI Gate |
  |---|---|---|---|
  | Public rooms | `PublicRoomTracker` | `ContinuousTracker` | N/A (always on) |
  | Private rooms | `PrivateRoomTracker` | `PrivateContinuousTracker` | `--no-dht` |

- **Underlying DHT**: `mainline::async_dht::AsyncDht` (same patched crate,
  separate instance).

---

## 2. File-by-File Trace

### Core DHT Abstraction

| File | Role | Key Lines |
|---|---|---|
| `src/discovery_backend.rs` | `TopicDiscoveryBackend` trait + `MainlineDhtBackend` + `InMemoryDiscoveryBackend` | 98-111 (trait), 228-297 (MainlineDhtBackend), 120-222 (InMemory) |
| `src/discovery_record.rs` | `DiscoveryRecordPayload` wire format, `create_discovery_record()`, `decode_discovery_record()` | 77-102 (payload), 129-142 (create), 165-168 (decode) |
| `src/discovery_secret.rs` | `DiscoverySecret` — 32-byte CSPRNG secret with CT comparison | 44-115 |
| `src/discovery_validation.rs` | Full validation pipeline: size → timestamp → decode → identity → signature | 325-464 |

### Trackers

| File | Role | Key Lines |
|---|---|---|
| `src/public_room_tracker.rs` | `PublicRoomTracker` — publish/discover for public rooms | 73-329 |
| `src/private_room_tracker.rs` | `PrivateRoomTracker` — same API but with `DiscoverySecret` for isolation | 117-400 |
| `src/public_room_continuous.rs` | `ContinuousTracker` — background publish/discover loops for public rooms | 148-600 + tests |
| `src/private_room_tracker.rs` | `PrivateContinuousTracker` — same for private rooms | 403-491 |

### Network Layer

| File | Role | Key Lines |
|---|---|---|
| `src/net/address_lookup.rs` | `GossipAddressLookup` — addresses from gossip Join/ForwardJoin, NOT DHT | 41-184 |
| `src/net/address_resolution.rs` | `resolve_candidates()` — priority-ordered resolution policy | 71-108 |
| `src/net.rs` | Gossip actor — wires `GossipAddressLookup` into address lookup chain | 204-216 |

### Patched DHT Crates

| File | Role |
|---|---|
| `patched/mainline/src/` | Patched mainline v7.0.0 — Windows socket fix (WSAETIMEDOUT) |
| `patched/iroh-dns/src/` | Patched iroh-dns — Windows DoH fallback + increased timeout |

### CLI & Integration (GUI)

| File | Role | Key Lines |
|---|---|---|
| `examples/iced_chat/main.rs` | CLI args, endpoint construction, DhtAddressLookup, dht_for_private creation | 418-505 (relay + endpoint), 496-504 (DhtAddressLookup), 751-755 (dht_for_private) |
| `examples/iced_chat/app.rs` | GUI state, private room DHT init, mDNS events → `DiscoveredPeersUpdate` channel | 5556-5605 (private room DHT), 1784 (continuous_tracker field), 616-690 (mDNS events) |

### Examples & Tests

| File | Role |
|---|---|
| `examples/dht_harness.rs` | Manual live DHT test with `MainlineDhtBackend` + `PublicRoomTracker` |
| `tests/test_private_room_dht_discovery.rs` | 10 integration tests (in-memory backend) |
| `tests/test_private_room_invitation_discovery.rs` | 9 integration tests (invitation flow + DHT) |
| `tests/test_public_lobby_integration.rs` | 8 integration tests (public room ContinuousTracker) |

---

## 3. Data Flow: End-to-End Paths

### Path A: Public Lobby Discovery (what *should* happen)

```
ContinuousTracker::start()
  ├─ publish_loop() every 5min
  │   └─ PublicRoomTracker::publish_once()
  │       └─ MainlineDhtBackend::publish() → distributed_topic_tracker::Dht::put_mutable()
  └─ discover_loop() every 30s
      └─ PublicRoomTracker::discover_once()
          └─ MainlineDhtBackend::lookup() → distributed_topic_tracker::Dht::get()
              → DiscoveryRecordValidator::filter_and_build()
              → Vec<EndpointId> → mpsc channel → GossipSender::join_peers()
```

**ISSUE**: This path is **never activated in the GUI**. The `IcedChat` struct
has `continuous_tracker: Option<ContinuousTracker>` (line 1784, marked
`#[expect(dead_code)]`), but `IcedChat::new()` at `main.rs:904` passes `None`.
No code path populates it. The `dht_harness` example and unit tests exercise
the components, but the GUI never starts the public lobby discovery loop.

### Path B: Private Room Discovery (functional)

```
JoinFromTicket flow (app.rs:5556-5605):
  if !private_dht_disabled && secret.is_some():
    dht = Dht::new(DhtConfig::default())          // fallback or passed-in
    backend = MainlineDhtBackend::new(dht, ns)
    tracker = PrivateRoomTracker::new(backend, topic, secret, ep, sk)
    tracker.discover_once()                        // one-shot at join time
    PrivateContinuousTracker::start(tracker, config, tx)  // background loop
    spawn_join_fanout(rx, sender, cancel)          // bridge discovered peers to gossip
```

This path works correctly. The `--no-dht` flag gates both the one-shot
`discover_once()` and the background loop.

### Path C: Address Resolution

```
Endpoint::builder() → .address_lookup(mdns)        // mDNS registered as primary
                       .relay_mode(...)
                       .bind()
endpoint.address_lookup().add(memory_lookup)       // MemoryLookup added
if !no_dht:
    DhtAddressLookup::builder().secret_key(sk).build()
    endpoint.address_lookup().add(dht)              // DhtAddressLookup added

Resolution priority (address_resolution.rs:71-108):
    Current > Persisted > Mdns > Configured > Relay > Dht > TrustedPeer
```

The iroh endpoint's address lookup chain is: mDNS first, then MemoryLookup,
then DhtAddressLookup (if not disabled). The resolution policy in
`address_resolution.rs` uses **source priority, not chain order**.

---

## 4. Configuration & CLI

| Flag | Type | Default | Scope |
|---|---|---|---|
| `--relay <URL>` | `RelayUrl` | `https://boru.chat:8443` | Relay transport |
| `--no-relay` | Flag | Off | Disable relay entirely |
| `--no-dht` | Flag | Off | Disable private-room DHT + DhtAddressLookup |
| `--bind-port` | `u16` | 0 (OS-assigned) | Local QUIC port |
| `--data-dir` | `PathBuf` | `~/.local/share/boru/` | Persistence |

### `--no-dht` scope (documented + implemented)

1. **`DhtAddressLookup`**: Not registered on endpoint (`main.rs:496`)
2. **`distributed_topic_tracker::Dht`**: `dht_for_private = None` (`main.rs:751`)
3. **Private room discovery**: Skipped in `JoinFromTicket` (`app.rs:5556-5605`)
4. **Public lobby**: *Not affected* by this flag (by design)

### `--no-relay` scope

1. Uses `presets::N0DisableRelay` instead of `presets::N0` (`main.rs:465-468`)
2. Skips `endpoint.online()` (`main.rs:479-481`)
3. Sets `RelayMode::Disabled`

---

## 5. Security Analysis

### Strengths

1. **DiscoverySecret** — 32-byte CSPRNG, constant-time comparison, Debug redacts,
   manual Clone with doc warning. (`discovery_secret.rs:44-115`)

2. **Domain separation** — Public and private room namespaces use distinct
   domain separators (`PUBLIC_LOBBY_KEY_DOMAIN` vs `PRIVATE_ROOM_DOMAIN_SEPARATOR`).
   Private rooms use `BLAKE3(domain || topic || secret)` (`private_room_tracker.rs:86-93`).

3. **Signed records** — Each discovery record is Ed25519-signed with the
   publisher's secret key. The `pub_key` in the record header must match the
   `endpoint_id` in the decoded payload (`discovery_validation.rs:370-374`).

4. **HPKE encryption** — Private-room discovery records are encrypted with
   per-minute keys derived from the `DiscoverySecret`. Peers without the
   secret cannot decrypt (`private_room_tracker.rs:233`).

5. **Record validation pipeline** — 5-stage check: size → timestamp →
   decode → identity → signature. Cheap checks first, expensive signature
   verification last (`discovery_validation.rs:325-382`).

6. **Hard bounds** — `HARD_MAX_RECORDS_PER_LOOKUP = 20`,
   `HARD_MAX_CANDIDATE_PEERS = 20`, `HARD_MAX_RECORD_SIZE = 256`.
   Caller config cannot exceed these (`discovery_validation.rs:312-316`).

7. **Self-filtering + dedup** — Local `EndpointId` is filtered out;
   duplicate `EndpointId` values are collapsed (`discovery_validation.rs:433-446`).

8. **Timestamp freshness** — Records older than 10 minutes or with >2
   minutes future skew are rejected (`discovery_validation.rs:58-62`).

9. **No secret leakage in logs** — Debug impls redact secrets.
   Structured logs carry only short topic/peer hex prefixes.

10. **Legacy ticket compatibility** — `discovery_secret` is `#[serde(default)]`,
    old tickets deserialize to `None` without error (`chat_core.rs:1028-1029`).

### Weaknesses

1. **Public lobby DHT never starts in GUI** — The `ContinuousTracker` for
   the public lobby is never spawned. This is a functional gap, not a security
   issue: public rooms still work via mDNS (LAN) and tickets (out-of-band).
   See Issue #1 below.

2. **DhtAddressLookup publishes relay URL by default** — `AddrFilter::relay_only()`
   means the DHT address lookup only publishes relay URLs, not direct IPs.
   This is a privacy feature (prevents IP leakage) but means DHT-resolution
   provides only relay connectivity.

---

## 6. Test Coverage Summary

### All 111 DHT-related tests pass (0 failures)

| Test suite | Count | Coverage |
|---|---|---|
| `discovery_backend::tests` | 18 | InMemoryBackend: publish, lookup, expiry, bounds, clear, validation, trait object |
| `discovery_record::tests` | 17 | Payload roundtrip, create/decode, verify, bytes, determinism |
| `discovery_validation::tests` | 25 | All rejection reasons, hard bounds, self-filter, dedup, Send+Sync |
| `discovery_secret::tests` | 9 | Generate, debug redact, serde, constant-time eq, hash |
| `public_room_tracker::tests` | 14 | Publish/discover, self-filter, multiple peers, shutdown |
| `public_room_continuous::tests` | 16 | Background loops, peer discovery, backoff, cancellation, joiner |
| `private_room_tracker::tests` | 18 | Namespace isolation, encryption, discovery roundtrip, shutdown |
| `test_private_room_dht_discovery` | 10 | Multi-peer chains, offline peers, namespace isolation |
| `test_private_room_invitation_discovery` | 9 | Invitation flow, malformed records, 5-peer scenario |
| `test_public_lobby_integration` | 8 | Multi-peer lobby, stale records, graceful degradation |
| `net::address_lookup::tests` | 3 | Friend address persistence, retention |
| `net::address_resolution::tests` | 4 | Resolution order, dedup, identity check, failure categories |

### Gap: No test exercises the actual `MainlineDhtBackend` + `DhtAddressLookup`
chain end-to-end with the real mainline DHT. The `dht_harness` example does
this manually but isn't an automated test. The `iroh-mainline-address-lookup`
crate has its own `#[ignore = "flaky"]` test.

---

## 7. Identified Issues

### Issue #1 (Medium): Public lobby ContinuousTracker never started in GUI

**Location**: `examples/iced_chat/app.rs:1784` (field declaration),
`examples/iced_chat/main.rs:904` (passes `None`)

**Root cause**: When `IcedChat::new()` is called from `main.rs`, parameter
position 19 (`continuous_tracker`) receives `None`. The field exists and
has infrastructure (`DiscoveredPeersUpdate` channel, mDNS event wiring)
but the DHT-based peer discovery loop is never created.

**Impact**: Public lobby users in the GUI cannot find each other via DHT.
They rely entirely on:
- mDNS (LAN only)
- Tickets (out-of-band)
- Manual peer entry

**To fix**: Create a `PublicRoomTracker` and `ContinuousTracker` for the
lobby topic and pass it through. This requires:
1. A `distributed_topic_tracker::Dht` instance (shared with the private-room
   system or separate)
2. A `MainlineDhtBackend` wrapping it
3. A `PublicRoomTracker::start()` with the appropriate `PublicNetwork`
4. A `ContinuousTracker::start()` or `start_with_joiner()` with a channel to
   forward discovered peers to the gossip sender

**Severity**: Medium — this affects feature parity (public lobby peer discovery)
but is not a correctness or security issue. The TUI-based `dht_harness` example
demonstrates the system works.

### Issue #2 (Low): Redundant Dht creation fallback in app.rs

**Location**: `examples/iced_chat/app.rs:5558-5562`

The private-room DHT fallback `dht.unwrap_or_else(|| Dht::new(...))` creates
a fresh `distributed_topic_tracker::Dht` whenever the passed-in value is `None`.
This is a valid defensive pattern, but since the value comes from
`main.rs:751-755` which already creates it when `!no_dht`, the fallback is
dead code under normal operation.

**Recommendation**: Remove the fallback or `assert!(dht.is_some())` when
`!private_dht_disabled` to surface misconfiguration early.

### Issue #3 (Info): mDNS drives the discovered-peers UI, not DHT

**Location**: `examples/iced_chat/main.rs:612-690`

The `DiscoveredPeersUpdate` channel receives updates exclusively from mDNS
`DiscoveryEvent`s. Even if DHT discovery were wired up, the discovered peers
would need to be forwarded to this channel (or a separate UI section) to be
visible. Currently, DHT-discovered peers from private rooms go directly to
`spawn_join_fanout()` which calls `GossipSender::join_peers()` — they join
the gossip mesh but don't appear in the "discovered peers" UI.

---

## 8. Baseline Verification

### Build Status

| Target | Status |
|---|---|
| `cargo check --lib --features net` | Clean |
| `cargo check --features "net,test-utils"` | Clean |
| `cargo check --example dht_harness --features net` | Clean |
| `cargo check --example doctor --features net` | Clean |

### Test Status (all pass)

| Test Target | Count |
|---|---|
| Unit tests (discovery_backend + discovery_record + discovery_validation + discovery_secret) | 69 |
| Unit tests (public_room_tracker + public_room_continuous + private_room_tracker) | 48 |
| Integration tests (private_room_dht_discovery + private_room_invitation_discovery + public_lobby_integration) | 27 |
| net tests (address_lookup + address_resolution) | 7 |
| **Total** | **151 relevant, all passing** |

### Iroh 1.x API Compatibility

The codebase uses iroh 1.0.2 with these API surfaces:

| API | Usage | Status |
|---|---|---|
| `Endpoint::builder()` → `.address_lookup()` → `.relay_mode()` → `.bind()` | `main.rs:465-476` | OK |
| `endpoint.address_lookup()` (returns `Result<&AddressLookupRegistry>`) | `main.rs:478,497` | OK |
| `AddressLookupRegistry::add(impl AddressLookup)` | `main.rs:478,502` | OK |
| `endpoint.addr()` / `endpoint.watch_addr()` | `main.rs:482` / `app.rs:5644` | OK |
| `endpoint.online()` | `main.rs:480` | OK |
| `AddressLookup` trait (resolve → BoxStream) | `net/address_lookup.rs:165-184` | OK |
| `presets::N0` / `presets::N0DisableRelay` | `main.rs:466-468` | OK |
| `RelayMode::Disabled` / `RelayMode::Custom(...)` | `main.rs:420-427` | OK |

The patched crates (`mainline` v7.0.0, `iroh-dns`) are pinned via
`[patch.crates-io]` in `Cargo.toml:279-281` and compile cleanly.

---

## 9. Next Steps (for DHT-2+)

1. **Wire up public lobby DHT in GUI** — Create `ContinuousTracker` for the
   lobby in `main.rs` and pass it to `IcedChat::new()`. Forward discovered
   peers to the gossip mesh. (See Issue #1)

2. **Unify Dht instances** — The `distributed_topic_tracker::Dht` for private
   rooms creates a separate UDP socket per room. Consider sharing one instance
   across all rooms to reduce resource usage and bootstrap time.

3. **DHT-discovered peers in UI** — Currently private-room DHT peers go directly
   to gossip join without appearing in the "discovered peers" list. Consider
   forwarding them through a shared channel for UI display.

4. **Auto-test the `dht_harness`** — The manual DHT test requires network access.
   Consider adding an integration test with `Testnet` (the mainline crate's
   simulated DHT network) that exercises `MainlineDhtBackend` + `PublicRoomTracker`
   end-to-end.
