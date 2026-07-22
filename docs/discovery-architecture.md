# Discovery Architecture

Boru uses **two independent discovery layers** that serve different purposes
and use separate DHT instances:

1. **Address resolution** — resolves an `EndpointId` to transport addresses
   (relay URLs, direct IPs) so iroh can dial the peer.
2. **Member discovery** — finds which peers are part of a room by looking up
   signed records on a room-specific DHT namespace, yielding `EndpointId`
   values (not transport addresses).

These two layers are intentionally decoupled: a peer discovered via member
discovery still needs address resolution to connect, and an address that is
already known (e.g. from a friend's persisted `known_addrs`) does not need
DHT address resolution.

---

## 1. Two Discovery Layers

### 1.1 Address Resolution Layer

| Aspect | Detail |
|---|---|
| **Crate** | `iroh-mainline-address-lookup = "0.4"` |
| **Type** | `DhtAddressLookup` implements `iroh::address_lookup::AddressLookup` |
| **Purpose** | Resolve `EndpointId` → transport addresses (relay + optional direct IPs) |
| **Underlying DHT** | `n0_mainline::Dht` (patched mainline v7.0.0) |
| **CLI gate** | `--no-dht` suppresses registration |
| **Addr filter** | `AddrFilter::relay_only()` by default (privacy); `--publish-direct-addresses` switches to `unfiltered` |

The `DhtAddressLookup` publishes signed Pkarr records on the Mainline DHT
containing the node's relay URL and, when enabled, direct IP addresses.  Other
nodes resolve an `EndpointId` by querying the DHT for that node's record.

**Resolution priority** (defined in `src/net/address_resolution.rs:71`):

```
Current → Persisted → Mdns → Configured → Relay → Dht → TrustedPeer
```

Each source is tried in this order.  The first source with a candidate address
is preferred; all sources are still aggregated as fallbacks.  Identity
mismatch short-circuits to a `IdentityMismatch` failure.

### 1.2 Member Discovery Layer

| Aspect | Detail |
|---|---|
| **Crate** | `distributed-topic-tracker = "0.3.5"` |
| **Types** | `MainlineDhtBackend` wraps `distributed_topic_tracker::Dht` |
| **Purpose** | Discover `EndpointId` values of peers in a specific room |
| **Underlying DHT** | `mainline::async_dht::AsyncDht` (separate instance from address resolution) |
| **CLI gate** | `--no-dht` disables private-room discovery only |

Member discovery splits into two subsystems:

| Subsystem | Tracker | Background Loop | DHT Instance |
|---|---|---|---|
| Public rooms | `PublicRoomTracker` | `ContinuousTracker` | Always created (never suppressed by `--no-dht`) |
| Private rooms | `PrivateRoomTracker` | `PrivateContinuousTracker` | Created per room; gated by `--no-dht` |

---

## 2. DHT Instance Separation

Both layers use the same underlying `mainline` DHT crate (v7.0.0) but create
**separate actor threads with distinct UDP sockets**.  This is intentional:

- The address resolution layer (`iroh-mainline-address-lookup`) manages its own
  `n0_mainline::Dht` instance for Pkarr records.
- The member discovery layer (`distributed-topic-tracker`) creates its own
  `mainline::async_dht::AsyncDht` instance.

Each binds to an OS-assigned random UDP port (`mainline::Dht::client()`), so
there is no socket conflict.  The design avoids rate/timing interference
between the two systems.

**Do not** attempt to share a single `mainline::Dht` instance across both
systems unless the upstream crates explicitly support it (they do not — each
wraps its own internal `mainline::Dht`).

---

## 3. mDNS (LAN Discovery)

mDNS is provided by `iroh_mdns_address_lookup::MdnsAddressLookup` and is
registered as the **primary** address lookup on the iroh `Endpoint`.

- **Scope**: Local network only (link-local multicast).
- **Addresses**: Publishes and discovers direct IP addresses plus relay URLs.
- **UI integration**: mDNS events drive the "Discovered Peers" panel in the GUI
  via a dedicated `DiscoveredPeersUpdate` channel.
- **CLI**: Always active unless the underlying iroh endpoint is configured
  without mDNS.  No explicit `--no-mdns` flag exists.
- **Resolution priority**: mDNS is source-prioritised above Relay and Dht
  in the resolution policy.

mDNS is the **only** discovery mechanism that populates the GUI's
"discovered peers" list.  DHT-discovered peers join the gossip mesh directly
without appearing in that list.

---

## 4. Public Room Member Discovery

### 4.1 Identity Derivation

Public rooms use **deterministic identities** — every client derives the same
topic and discovery key for the same room, so peers find each other without
out-of-band coordination.

```
topic        = BLAKE3(domain_sep_topic     || network_byte || room_name || version)
discovery_key = BLAKE3(domain_sep_discovery || network_byte || room_name || version)
```

- **Domain separators** are distinct (`public-room-topic` vs `boru-chat discovery-key v1`),
  preventing cross-protocol confusion.
- **Network byte** (0x00 = Mainnet, 0x01 = Development, 0x02 = Test) isolates
  environments even with the same room name.
- **Version byte** (currently `1`) allows future protocol upgrades.

The result is a `PublicRoomIdentity { topic: TopicId, discovery_key: [u8; 32] }`.

### 4.2 Publication

`PublicRoomTracker::publish_once()`:

1. Creates a signed `discovery_record::Record` carrying the node's `EndpointId`.
2. Derives the DHT namespace via `canonical_lobby_key(discovery_key)` (domain-separated BLAKE3).
3. Wraps the record as an encrypted payload and publishes via `TopicDiscoveryBackend::publish()`.

### 4.3 Discovery

`PublicRoomTracker::discover_once()`:

1. Looks up records from the DHT namespace via `TopicDiscoveryBackend::lookup()`.
2. Deserialises encrypted payloads back into `Record` values (silently skipping malformed ones).
3. Runs the validation pipeline (size → timestamp → decode → identity → signature).
4. Filters self and deduplicates.
5. Returns bounded `Vec<EndpointId>` (default max 20).

### 4.4 Continuous Loop

`ContinuousTracker` (`public_room_continuous.rs`) spawns two background tokio
tasks:

- **Publish loop**: Re-publishes local presence at a configurable interval
  (default: 5 minutes) with uniform jitter (±10%).
- **Discover loop**: Queries the DHT for new peers at a configurable interval
  (default: 30 seconds) with uniform jitter.

**Publication policy** (`PublicationPolicy`) coordinates with the DHT minute
to avoid redundant writes:

- Within the same `unix_minute`, only one publish is performed regardless of
  how many ticks fire — subsequent ticks see `SkipMinuteNotElapsed` and sleep.
- After a successful publish, the policy reports `Published` and resets the
  backoff counter.
- After a failure, exponential backoff is applied (1s → 2s → 4s → ... capped at
  max_retry_delay, default 60s).
- A single success resets the backoff to zero.

### 4.5 Current Limitation: GUI Integration

The `ContinuousTracker` for the **public lobby** is never spawned in the GUI.
The field exists in `IcedChat` (`continuous_tracker: Option<ContinuousTracker>`)
but `IcedChat::new()` receives `None` from `main.rs`.  Public lobby users in
the GUI therefore rely on:

- **mDNS** (LAN only)
- **Tickets** (out-of-band)
- **Manual peer entry**

The `dht_harness` example and all unit tests exercise the components
correctly — this is a wiring gap, not a correctness issue.

---

## 5. Private Room Member Discovery

### 5.1 Namespace Derivation

Private rooms use a **secret-based namespace** that requires both the gossip
`TopicId` and a `DiscoverySecret` (32-byte CSPRNG value) to locate peers:

```
namespace = BLAKE3("boru-chat private-room v1" || topic || secret)
```

Only peers who know both the topic and secret can:
- Derive the DHT namespace.
- Publish valid discovery records.
- Verify and decrypt records published by other members.

### 5.2 DiscoverySecret

`DiscoverySecret` is a 32-byte cryptographically random value with
defensive properties:

- **CSPRNG-backed**: Generated via `getrandom` (OS entropy source).
- **Debug-redacted**: Only the first 4 hex bytes are shown.
- **Constant-time comparison**: XOR-and-check prevents timing side-channels.
- **Serde**: Serialised as bytes with length validation; `#[serde(default)]`
  on legacy ticket fields ensures old tickets deserialise to `None` without error.
- **V2 subkeys**: Domain-separated subkey derivation functions exist for
  assessment but are not used by the V1 wire format.  See "V2 Migration" below.

### 5.3 Encryption

Private-room discovery records are **HPKE-encrypted** using per-minute keys
derived from the `DiscoverySecret`:

```
encryption_key = encryption_keypair(secret_as_tracker_topic, rotation_handle, BLAKE3(secret), minute)
```

Peers without the secret cannot decrypt the records — they see only opaque
ciphertext on the DHT.

### 5.4 Lifecycle

`PrivateRoomTracker` is created per room on join:

1. **One-shot discovery** at join time: `discover_once()` fetches existing
   room members.
2. **Background loop** (`PrivateContinuousTracker`): Periodic publish and
   discover at configurable intervals (same `ContinuousTrackerConfig` structure).
3. **Join fanout**: Discovered `EndpointId` values are forwarded to
   `spawn_join_fanout()` which calls `GossipSender::join_peers()` to connect
   the gossip mesh.

The `--no-dht` flag gates both the one-shot discovery and the background loop.

### 5.5 Legacy Compatibility

- Old tickets without a `discovery_secret` field deserialise gracefully
  (the field is `#[serde(default)]` in chat_core types).
- Private-room DHT is skipped when `secret` is `None`, falling back to
  ticket-based join without DHT discovery.

---

## 6. Wire Format

### 6.1 Discovery Record

Each record is a `distributed_topic_tracker::Record` whose inner content is a
postcard-encoded `DiscoveryRecordPayload`:

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | Content version (`DISCOVERY_RECORD_CONTENT_VERSION` = 1) |
| 1 | 32 | EndpointId (Ed25519 public key, big-endian y-coordinate) |

Total payload: **~35 bytes** on the wire (postcard overhead ~2 bytes).

The outer `Record` envelope adds:

| Field | Size |
|-------|------|
| Topic hash | 32 B |
| Unix minute | 8 B |
| Publisher pub_key | 32 B |
| Content (variable) | ~35 B |
| Ed25519 signature | 64 B |
| **Total** | **~171 B** |

After HPKE encryption (private rooms), the ciphertext is ~270 B, well under
the tracker's `EncryptedRecord::MAX_SIZE` of 2048 B.

### 6.2 Security Properties

- **Publisher binding**: The record embeds the publisher's Ed25519 verifying
  key and is signed with the corresponding secret key. Signature verification
  proves authorship.
- **Time window**: Each record is bound to a `unix_minute` slot, enabling
  the tracker's minute-rotating key schedule and making replay attacks
  self-limiting (records are valid only within the validation window).
- **Topic binding**: The topic hash is signed into every record, so a record
  valid for one room's discovery key is useless for another.

### 6.3 Version

The current wire-format version is `1`.  The version byte is embedded in every
payload and is validated during decoding.  Records with an unknown version
are rejected with a `DecodeFailure` rather than silently treated as valid
peers.

---

## 7. Validation Pipeline

Every discovery record fetched from the DHT goes through a 5-stage validation
pipeline (`DiscoveryRecordValidator::validate_single`).  Cheap checks run
first; the expensive Ed25519 signature verification runs last.

1. **Size check** — Rejects records exceeding `max_record_size` (hard cap:
   256 bytes).  Catches garbage/DoS records early.
2. **Timestamp check** — Rejects records older than `max_record_age_minutes`
   (default: 10 minutes) or more than `max_clock_skew_minutes` (default:
   2 minutes) in the future.
3. **Decode payload** — Rejects records whose content cannot be deserialised
   as a `DiscoveryRecordPayload`.  Also rejects unknown payload versions.
4. **Identity match** — Rejects records where the embedded `pub_key` does not
   equal the payload's `endpoint_id` (defence against record forgery).
5. **Signature verify** — Rejects records whose Ed25519 signature does not
   validate for the expected topic and the record's own `unix_minute`.

### Batch processing

`filter_and_build()` processes a batch:

- At most `max_records_per_lookup` (hard cap: 20) records are examined.
- Self-filtering excludes the local node's own `EndpointId`.
- Deduplication collapses identical `EndpointId` values.
- At most `max_candidate_peers` (hard cap: 20) are returned.
- Structured counters track every rejection category for observability.

---

## 8. DHT Outage Behaviour

### Fallback mechanisms

When the DHT is unreachable or returns empty results:

1. **Known peers continue working.**  Addresses already cached in iroh's
   `GossipAddressLookup` (from gossip Join/ForwardJoin messages), persisted in
   `FriendsStore.known_addrs`, or registered in `MemoryLookup` remain usable
   regardless of DHT availability.  The gossip mesh itself continues to
   propagate messages over existing connections.

2. **Exponential backoff** on publish/discover failures.  Consecutive failures
   delay the next attempt (1s → 2s → 4s → ... capped at `max_retry_delay`,
   default 60s).  A single success resets the backoff to zero.

3. **Graceful degradation of discovery.**  A DHT outage means new peers cannot
   be discovered via the DHT, but:
   - mDNS (LAN) is unaffected.
   - Ticket-based joins (out-of-band) are unaffected.
   - Manual peer entry is unaffected.
   - Once the DHT recovers, the next successful tick resumes normal operation.

4. **No connection teardown.**  The gossip mesh is independent of DHT
   availability.  Existing connections are never torn down due to DHT failures.

### Simulated outage test

`test_deterministic_discovery_integration` includes a three-peer DHT outage
simulation: all peers publish, the backend is cleared (simulating an outage),
one peer recovers, and eventual discovery is verified with no continuous
republishing.

---

## 9. Privacy Implications

### 9.1 Relay vs Direct Addressing

By default, the `AddressLookup` layer publishes **relay URLs only**
(`AddrFilter::relay_only()`).  Direct IP addresses are not published on the
Mainline DHT.  This provides:

- **IP privacy**: Your public IP address is not exposed on a global DHT.
- **NAT traversal**: relay connectivity still works for all peers.
- **DHT fingerprinting resistance**: The relay URL pattern is the same for
  all nodes using the same relay, reducing linkability.

The `--publish-direct-addresses` flag switches to `AddrFilter::unfiltered`,
publishing direct IPs alongside relay URLs.  This improves connectivity
(peers can connect directly, reducing relay load and latency) but **exposes
your public IP address on the Mainline DHT**.

### 9.2 Private Room Secrecy

Private rooms add three layers of privacy beyond public rooms:

1. **Namespace isolation**: The DHT namespace is `BLAKE3(topic || secret)`,
   not a deterministic public key.  An attacker who knows only the gossip
   `TopicId` cannot find the DHT namespace.
2. **HPKE encryption**: Records are encrypted with per-minute keys derived
   from the `DiscoverySecret`.  An attacker who finds the namespace still
   cannot read the records.
3. **No secret in logs**: `Debug` impls of `DiscoverySecret` redact all but
   the first 4 bytes.  Structured logs carry only short hex prefixes of
   topics and peer IDs.

### 9.3 Public Room Visibility

Public rooms are **public** by design:

- The discovery key is deterministic from (network, room name, version).
- Anyone who knows the room name can compute the key and find room members
  on the DHT.
- Records are not encrypted (only wrapped in the tracker's standard envelope).
- This is intentional: public rooms trade privacy for discoverability.

---

## 10. Record Lifetime & Refresh Policy

| Parameter | Default | Description |
|---|---|---|
| `DISCOVERY_LEASE_SECS` | 600 (10 min) | How long a published record is considered valid by the backend |
| `DISCOVERY_REFRESH_SECS` | 300 (5 min) | Recommended refresh interval (half the lease) |
| `publish_interval` | 5 min | `ContinuousTracker` publish loop interval (with jitter) |
| `discover_interval` | 30 s | `ContinuousTracker` discover loop interval (with jitter) |
| `max_record_age_minutes` | 10 min | Maximum age for records considered valid during lookup |
| `max_clock_skew_minutes` | 2 min | Maximum future skew allowed for record timestamps |

**Refresh flow**:

1. Every `publish_interval` (5 min), the publish loop fires.
2. `PublicationPolicy` checks if the current `unix_minute` has elapsed since
   the last publish — if not, it skips (`SkipMinuteNotElapsed`).
3. If publish succeeds, the record's lease is refreshed (10 min from now).
4. On failure, exponential backoff delays the next attempt.
5. Records are validated on discovery using a 10-minute age window, meaning
   a record published 8 minutes ago is still valid, but one published 11
   minutes ago is rejected as stale.

---

## 11. Why Known Peers and Existing Connections Remain Usable

Discovery is **not required for ongoing communication**.  The peer-to-peer
gossip mesh, once established, is self-sustaining for existing connections:

- **GossipAddressLookup**: Addresses learned from gossip Join/ForwardJoin
  messages are cached with a 5-minute retention and 30-second eviction
  interval.  This cache is independent of DHT state.
- **FriendsStore persistence**: Friend `known_addrs` are persisted to disk
  and survive restarts.  Addresses learned from gossip or DHT are written
  through to the friends store when the peer is a known friend.
- **MemoryLookup**: Bootstrap peers and configured relay addresses are held
  in an in-process address book.
- **Ongoing connections**: Once a QUIC connection is established, the gossip
  mesh, whisper sessions, and inbox protocols operate independently of DHT
  or address resolution.

DHT is therefore a **discovery optimisation** for finding new peers, not a
core requirement for the chat application to function once connected.

---

## 12. Limitations

### 12.1 Mainline DHT Mutable Record Limitations

The `distributed-topic-tracker` crate uses Mainline DHT mutable records
(put/get with Ed25519 keys).  Inherent limitations:

- **No atomic multi-value operations**: Many peers publishing under the same
  namespace do so independently.  A lookup may or may not see all peers
  depending on DHT propagation timing.
- **No ordering**: Records are returned in DHT-node order, not publication
  order.  Each lookup is a new DHT query.
- **No deletion**: Once published, a record remains in the DHT until it ages
  out (lease expiry).  There is no authenticated delete operation.
- **Propagation delay**: Record propagation through the Mainline DHT is
  probabilistic and asynchronous.  A newly published record may not be visible
  to all peers simultaneously.
- **Rate limits**: The Mainline DHT has no built-in rate limiting.  The
  application-layer `PublicationPolicy` and `max_candidate_peers` bounds
  prevent application-level abuse but cannot control DHT-level query rates.

### 12.2 Two-Client Recommendation

When running two instances of Boru on the same machine (e.g. GUI + CLI):

- Each instance creates its own DHT sockets (separate UDP ports).  This is
  normal and does not cause conflicts.
- Both instances share the same Mainline DHT network — each maintains its
  own routing table.
- The `--no-dht` flag can be used on one instance to reduce DHT traffic if
  only discovery from one instance is needed.

### 12.3 Wire Format Compatibility

The current wire format (V1) is version-gated:

- Records carry a `version` byte in the payload.
- The validator rejects unknown versions at decode time.
- Adding fields requires incrementing the version and updating both publisher
  and validator.
- Old records with version 1 remain valid as long as they pass the time-window
  check, so forward compatibility is preserved during a transition period.

### 12.4 V2 Migration

The `DiscoverySecret` module includes V2 subkey derivation functions
(`subkey_namespace`, `subkey_encryption`, `subkey_signing`) that provide
domain-separated subkeys:

```
subkey_namespace  = BLAKE3("boru-chat private-room v2 namespace"  || secret || topic)
subkey_encryption = BLAKE3("boru-chat private-room v2 encryption" || secret)
subkey_signing    = BLAKE3("boru-chat private-room v2 signing"    || secret)
```

These exist for assessment and testing only — they are **not used** by the V1
wire format.  Benefits of V2 migration:

- Each subkey is independent: compromise of one does not affect the others.
- The namespace subkey is topic-bound (same secret, different room →
  different namespace).
- Full V1 backward compatibility is maintained during migration.

---

## 13. Module Reference

### Discovery System Modules

| File | Purpose |
|---|---|
| `src/discovery_backend.rs` | `TopicDiscoveryBackend` trait + `MainlineDhtBackend` + `InMemoryDiscoveryBackend` |
| `src/discovery_record.rs` | `DiscoveryRecordPayload` wire format, `create_discovery_record()`, `decode_discovery_record()` |
| `src/discovery_secret.rs` | `DiscoverySecret` — 32-byte CSPRNG secret with V2 subkey assessment |
| `src/discovery_validation.rs` | 5-stage validation pipeline, `ValidationConfig`, `ValidationCounters`, self-filter, dedup |

### Tracker Modules

| File | Purpose |
|---|---|
| `src/public_room.rs` | `PublicRoomIdentity`, `PublicNetwork`, deterministic key derivation |
| `src/public_room_tracker.rs` | Publish-once / discover-once for public rooms |
| `src/public_room_continuous.rs` | Background publish/discover loops, `PublicationPolicy`, exponential backoff |
| `src/public_room_config.rs` | Limits and defaults for DHT timing, message size, rate limits |
| `src/public_room_safety.rs` | Per-peer rate limiting for untrusted public-room message flows |
| `src/private_room_tracker.rs` | Namespace-isolated publish/discover with HPKE encryption, `PrivateContinuousTracker` |

### Network / Address Modules

| File | Purpose |
|---|---|
| `src/net/address_lookup.rs` | `GossipAddressLookup` — time-bounded address cache from gossip messages |
| `src/net/address_resolution.rs` | Deterministic address resolution policy (Current → Persisted → Mdns → ...) |
| `src/net.rs` | Gossip actor — wires address lookup chain |

### CLI / Integration

| File | Purpose |
|---|---|
| `examples/iced_chat/main.rs` | CLI args, endpoint construction, `DhtAddressLookup`, DHT instance creation |

---

## 14. Test Coverage

All DHT-related tests pass (1511+ total tests across lib + integration):

| Test suite | Count | What it covers |
|---|---|---|
| `discovery_backend::tests` | 18 | In-memory backend: publish, lookup, expiry, bounds, clear, trait object |
| `discovery_record::tests` | 17 | Payload roundtrip, create/decode, determinism, size bounds |
| `discovery_validation::tests` | 25 | All rejection reasons, hard bounds, self-filter, dedup |
| `discovery_secret::tests` | 9 | Generate, debug redact, serde, CT equality, V2 subkeys |
| `public_room_tracker::tests` | 14 | Publish/discover, self-filter, multiple peers, shutdown |
| `public_room_continuous::tests` | 16 | Background loops, peer discovery, backoff, cancellation |
| `private_room_tracker::tests` | 18 | Namespace isolation, encryption, roundtrip, shutdown |
| `test_private_room_dht_discovery` | 10 | Multi-peer chains, offline peers, namespace isolation |
| `test_private_room_invitation_discovery` | 9 | Invitation flow, malformed records, 5-peer scenario |
| `test_public_lobby_integration` | 8 | Multi-peer lobby, stale records, graceful degradation |
| `test_deterministic_discovery_integration` | 20 | Minute handling, backoff, dedup, 3-peer outage, caps, jitter, shutdown |
| `net::address_lookup::tests` | 3 | Friend address persistence, retention |
| `net::address_resolution::tests` | 4 | Resolution order, dedup, identity check, failure categories |
