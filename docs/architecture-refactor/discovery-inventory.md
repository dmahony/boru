# DiscoveryService Responsibility Inventory (BORU-DISC-001)

Status: inventory only — **no production behaviour changes in this task**.
Task: BORU-ARCH-13, Phase 2 of the BORU-ARCH chain (Discovery and Distributed-State Hardening).
Scope: `src/discovery_service.rs`, `src/control_plane/`, `src/room_directory.rs`, `src/discovery_message.rs`.

This document maps everything the internal discovery subsystem owns, groups every
function into the PDF's responsibility categories, identifies the shared mutable
state and lock boundaries, enumerates every timer / timeout transition, and records
the invariants that must survive the decomposition work in BORU-DISC-002..009.

---

## 1. Snapshot (what is being inventoried)

| File | Lines | Role |
|------|-------|------|
| `src/discovery_service.rs` | 9 030 (~400 KB) | The discovery service facade: topic join, publish, receive dispatch, peer registry, announce handles, connectivity wiring, reconnect integration, presence expiry, control-plane presence refresh, room-directory TTL sweep, path classification. |
| `src/control_plane/mod.rs` | 81 | Control-plane module root + `CONTROL_PLANE_SIGNING_DOMAIN`. |
| `src/control_plane/message.rs` | 1 621 | BORU-CP-01 wire envelope: `ControlEnvelope`, `ControlPayload`, magic `BC`, forward-compatible decoder. |
| `src/control_plane/privacy.rs` | 1 954 | BORU-CP-03 guard: advert policy, rate limiter, dedup, TTL presence store (`PeerControlStateStore`). |
| `src/control_plane/connectivity.rs` | 1 503 | BORU-CP-05 explicit peer connectivity state machine (`PeerConnectivityStore`). |
| `src/control_plane/reconnect.rs` | 687 | BORU-CP-07 reconnect scheduler + `ReconnectHandle` + `ReconnectSignal`. |
| `src/control_plane/reconcile.rs` | 294 | BORU-CP-08 pure conversation-reconciliation decision (used by the app layer). |
| `src/control_plane/capabilities.rs` | 775 | BORU-CP-11 capability ids / sets / compatibility negotiation. |
| `src/control_plane/extensions.rs` | 836 | BORU-CP-16 metadata-only extensions payload model. |
| `src/control_plane/advertisement.rs` | 2 205 | BORU-DIR-01/02/09 room advertisement + withdrawal payload models, bounds, signing. |
| `src/control_plane/diagnostics.rs` | 530 | BORU-CP-13 share-safe per-peer diagnostic snapshots. |
| `src/control_plane/health.rs` | 536 | BORU-CP-15 developer networking health rows (debug-only, `doctor --health`). |
| `src/room_directory.rs` | 2 619 | BORU-DIR-10 bounded local room-directory cache (`RoomDirectory`). |
| `src/discovery_message.rs` | 617 | Legacy discovery wire types (`DiscoveryMessage`: Hello / Presence / PeerAdvertisement). |
| `src/dynamic_joiner.rs` | 1 082 | Dynamic peer joiner for mDNS/DHT results (sibling of the discovery service, out of scope for decomposition). |

The control plane is **already decomposed into focused modules** (BORU-CP chain).
What remains in `discovery_service.rs` is the orchestration layer that owns the
lifecycle, the gossip subscription, the legacy discovery receive path, and the
wiring between the control-plane modules and the network. The PDF's BORU-DISC
decomposition tasks therefore mostly move *orchestration* out of the service
facade and into the already-existing module homes — not create new wire formats.

---

## 2. Architecture overview

```
                        examples/iced_chat/main.rs (app layer)
                                     │
        ┌────────────────────────────┼────────────────────────────────┐
        │                            │                                │
   service.peer_updates()      service.reconnect_events()        service.control_events()
   (PeerUpdate broadcast)      (ReconnectSignal broadcast)       (ControlEvent broadcast)
        │                            │                                │
        ▼                            ▼                                ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                           DiscoveryService (facade)                           │
│                                                                              │
│  ┌─────────────┐  ┌───────────────────┐  ┌────────────────────────────────┐  │
│  │ AnnounceHandle │  │ ControlAnnounceHandle│  │          ReceiveCore         │  │
│  │ (legacy hello/  │  │ (CP hello/presence/  │  │  registry · guard · conn·   │  │
│  │  presence + evt │  │  caps/ext/advert)    │  │  reconnect · room_dir ·     │  │
│  │  id counter)    │  │  + 4 throttles       │  │  channels · counters        │  │
│  └─────────────┘  └───────────────────┘  └────────────────────────────────┘  │
│                                                                              │
│  Background tasks (each spawned in from_subscription_with_counters):          │
│   drain_loop · connectivity_loop · presence_expiry_loop                       │
│   presence_refresh_loop · reconnect_loop · directory_expiry_loop ·            │
│   path_refresh_loop (via with_endpoint)                                       │
└──────────────────────────────────────────────────────────────────────────────┘
        │                                │
        ▼                                ▼
  gossip subscription          gossip sender (broadcast)
  (discovery topic)
```

Ownership rule (module doc, `discovery_service.rs:13-23`): the service never
creates a `ConversationEntry`, never inserts into the conversation store, never
renders anything, and never routes chat payloads over the discovery topic. Chat
traffic stays on its own authenticated data-plane channels; the discovery topic
carries only `DiscoveryMessage` (legacy) and `ControlEnvelope` (magic `BC`).

---

## 3. State ownership

### 3.1 `DiscoveryService` struct fields (`discovery_service.rs:1859-1925`)

| Field | Type | Owner / shared |
|-------|------|----------------|
| `topic` | `TopicId` | Immutable for service lifetime; the joined discovery topic. |
| `announce` | `AnnounceHandle` | Legacy announce state (see 3.3). |
| `control_announce` | `ControlAnnounceHandle` | Control-plane announce state (see 3.3). |
| `core` | `ReceiveCore` | Receive-path shared state (see 3.2). |
| `cancel` | `CancellationToken` | Cancellation token shared by all background tasks. |
| `task` | `JoinHandle<()>` | drain task. |
| `connectivity_task` | `JoinHandle<()>` | connectivity wiring task. |
| `expiry_task` | `JoinHandle<()>` | presence-expiry sweep task. |
| `refresh_task` | `JoinHandle<()>` | control presence-refresh task. |
| `reconnect_task` | `JoinHandle<()>` | reconnect loop task. |
| `directory_expiry_task` | `JoinHandle<()>` | room-directory TTL sweep task. |
| `path_task` | `Option<JoinHandle<()>>` | path-classification sweep (`None` until `with_endpoint`). |
| `expiry_config` | `Arc<Mutex<PresenceExpiryConfig>>` | TTL + sweep interval; builder-tunable. |
| `refresh_config` | `Arc<Mutex<PresenceRefreshConfig>>` | refresh interval/jitter/cadence; builder-tunable. |
| `directory_expiry_config` | `Arc<Mutex<DirectoryExpiryConfig>>` | directory sweep interval; builder-tunable. |
| `local_caps` | `Arc<Mutex<CapabilitySet>>` | the capability set this node advertises. |
| `local_extensions` | `Arc<Mutex<ExtensionsPayload>>` | the extensions payload this node advertises. |

Dropping the handle **without** `shutdown` aborts the background tasks
(`discovery_service.rs:1857`). All task handles are owned by the service; the
app in `main.rs` holds the service for the app lifetime.

### 3.2 `ReceiveCore` fields (`discovery_service.rs:1245-1302`)

`ReceiveCore` is `Clone` and is shared between the drain task and the service
handle. It is the single place that owns receive-path state:

| Field | Type | Purpose |
|-------|------|---------|
| `local_node` | `PublicKey` | Local identity; used for self-filtering. |
| `topic` | `TopicId` | The discovery topic bound to this core. |
| `registry` | `Arc<Mutex<PeerRegistry>>` | Legacy peer registry (dedup anchor). |
| `peer_updates_tx` | `broadcast::Sender<PeerUpdate>` | Legacy peer notification stream (cap 256). |
| `control_events_tx` | `broadcast::Sender<ControlEvent>` | Control-plane event stream (cap 256). |
| `guard` | `Arc<Mutex<ControlPlaneGuard>>` | BORU-CP-03 privacy/abuse guard. |
| `connectivity` | `Arc<Mutex<PeerConnectivityStore>>` | BORU-CP-05 state machine. |
| `reconnect` | `Arc<Mutex<ReconnectScheduler>>` | BORU-CP-07 reconnect scheduler. |
| `reconnect_tx` | `broadcast::Sender<ReconnectSignal>` | Reconnect signals (cap 256). |
| `counters` | `DiagnosticCounters` | Peer/topic diagnostics (global by default, isolated in tests). |
| `directory_counters` | `DirectoryCounters` | Room-directory diagnostics (separate from peer counters). |
| `room_directory` | `Arc<Mutex<RoomDirectory>>` | Bounded room-directory cache (BORU-DIR-10). |

### 3.3 Announcement handles

**`AnnounceHandle`** (`discovery_service.rs:769-842`) — legacy discovery
announcements:

| Field | Type | Purpose |
|-------|------|---------|
| `sender` | `GossipSender` | Gossip sender half (keeps topic joined). |
| `local_node` | `PublicKey` | Local identity for Hello/Presence payloads. |
| `throttle` | `Arc<AnnounceThrottle>` | 30 s min-interval throttle. |
| `next_event_id` | `Arc<AtomicU64>` | Monotonic per-node event id; **seeded randomly per process** (BORU-CP-07 restart dedup fix). |

**`ControlAnnounceHandle`** (`discovery_service.rs:864-1228`) — control-plane
announcements:

| Field | Type | Purpose |
|-------|------|---------|
| `sender` | `GossipSender` | Gossip sender half. |
| `local_node` | `PublicKey` | Envelope `sender_node_id`. |
| `local_secret` | `Option<SecretKey>` | Ed25519 signing key for BORU-CP-17 (None in tests = legacy unsigned). |
| `sequence` | `Arc<AtomicU64>` | Per-sender sequence counter; **seeded with wall-clock seconds** (BORU-DIR-23 restart monotonicity). |
| `throttle` | `Arc<AnnounceThrottle>` | 30 s control announce throttle (HELLO/PRESENCE). |
| `caps_throttle` | `Arc<AnnounceThrottle>` | Separate CAPABILITIES throttle. |
| `last_announced_caps` | `Arc<Mutex<Option<Vec<String>>>>` | Wire-id list of last broadcast caps (idempotence). |
| `extensions_throttle` | `Arc<AnnounceThrottle>` | Separate EXTENSIONS throttle. |
| `advert_throttle` | `Arc<AnnounceThrottle>` | Separate room-advertisement throttle. |
| `last_announced_extensions` | `Arc<Mutex<Option<ExtensionsPayload>>>` | Last broadcast extensions payload (idempotence). |

Four throttles are deliberately separate so the join-time burst
(HELLO → CAPABILITIES → EXTENSIONS) and room advertisements can never starve
each other (`discovery_service.rs:855-862`, `1169-1172`).

### 3.4 `PeerRegistry` and `AnnounceThrottle`

**`PeerRegistry`** (`discovery_service.rs:491-627`): `HashMap<PublicKey,
PeerRegistryEntry>`. Entry = `last_seen: Instant`, `source: PeerSource`,
`source_topic: TopicId`, `last_event_id: Option<u64>`. Methods: `new`, `upsert`,
`contains`, `get`, `last_seen`, `peers`, `len`, `is_empty`, `prune_older_than`,
`clear`, `refresh_after_restart`.

**`AnnounceThrottle`** (`discovery_service.rs:662-733`): `Mutex<AnnounceThrottleState>`
holding `min_interval` + `last_announce: Option<Instant>`. Methods: `new`,
`with_min_interval`, `min_interval`, `set_min_interval`, `try_announce`.

### 3.5 Background-task local state

| Task | Local state (task-owned, not shared) |
|------|--------------------------------------|
| `drain_loop` | `event_count: u64` (diagnostic only). |
| `connectivity_loop` | `dialed: HashSet<EndpointId>` — once-per-service-lifetime dial dedup; 60 s debug dump interval. |
| `presence_refresh_loop` | `tick: u64` — drives caps/extensions `every N` cadence. |
| `reconnect_loop` | none (scheduler owns all state). |
| `presence_expiry_loop` | none (config owns all state). |
| `directory_expiry_loop` | none. |
| `path_refresh_loop` | none (endpoint clone held for lifetime). |

### 3.6 State already owned by `control_plane` modules (decomposition targets)

| Module | Owned state | Mutex |
|--------|-------------|-------|
| `privacy.rs` | `ControlPlaneGuard` = `ControlAdvertPolicy` + `ControlPlaneRateLimiter` + dedup set + `PeerControlStateStore` (presence hints: last seen, protocol version, caps, extensions, TTL) | `Arc<Mutex<>>` in `ReceiveCore.guard` |
| `connectivity.rs` | `PeerConnectivityStore` = `HashMap<PublicKey, PeerConnectivityEntry>` (state, path hint, direct-topic state, last error, bounded transition trail) | `Arc<Mutex<>>` in `ReceiveCore.connectivity` |
| `reconnect.rs` | `ReconnectScheduler` = per-peer `ReconnectState` (queue, attempts, next_attempt_at, in-flight flag) + backoff policy | `Arc<Mutex<>>` in `ReceiveCore.reconnect` |
| `room_directory.rs` | `RoomDirectory` = `HashMap<TopicId, DirectoryEntry>` (latest advert + provenance, expiry, compatibility, local join state) + bounds | `Arc<Mutex<>>` in `ReceiveCore.room_directory` |

---

## 4. Function groups by PDF category

Line references are against `src/discovery_service.rs` unless noted.

### 4.1 Wire decode / encode

**Outbound (encode):**
- `AnnounceHandle::publish` (802) — postcard-encode `DiscoveryMessage`, broadcast.
- `ControlAnnounceHandle::announce` (971) — build envelope, sign (BORU-CP-17), `encode()`, broadcast; sequence allocated only when the throttle passes.
- `ControlAnnounceHandle::announce_hello/presence/capabilities/extensions/room_advertisement/room_withdrawal` (991-1227) — typed envelope builders.
- `DiscoveryService::publish` (2372) — raw unthrottled legacy publish.
- `DiscoveryService::send_control` (2867) — signed encode + broadcast of a caller-supplied envelope.
- Legacy wire types: `DiscoveryMessage` in `src/discovery_message.rs` (Hello / Presence / PeerAdvertisement, `protocol_version`, `event_id`, `node_id`).
- Control wire types: `ControlEnvelope` / `ControlPayload` in `src/control_plane/message.rs` (magic `BC`, `MAX_CONTROL_PAYLOAD_LEN = 4096`, `BORU_APP_PROTOCOL_VERSION = 1`).

**Inbound (decode):**
- `ReceiveCore::handle_incoming` (1310) — legacy gate: magic sniff → postcard decode → `check_discovery_version` → self-filter → registry upsert → `PeerUpdate::Seen` / `Advertised`.
- `ReceiveCore::handle_control_incoming` (1470) — control gate: `ControlEnvelope::decode` → version check → self-filter → guard verdict → connectivity `DiscoverySeen` → room-advert / withdrawal decode + auth → `ControlEvent`.

The magic sniff is unambiguous: the legacy `DiscoveryMessage` postcard enum tags
live in `0..=2`, so `0x42 0x43` ("BC") can never be a legacy discovery message.

### 4.2 Peer registry

- `PeerRegistry` type + all methods (491-627).
- `ReceiveCore::handle_incoming` registry update path (1355-1418) — upsert + restart-rediscovery special case.
- `PeerSource` (240), `PeerUpdate` (267), `UpsertOutcome` (454), `PeerRegistryEntry` (469).
- `DiscoveryService::known_peers` (2883) / `peer_count` (2896) — read snapshots.
- `presence_expiry_loop` step 1 (3976-4001) — `prune_older_than(TTL)` → `PeerUpdate::Expired`.

### 4.3 Presence / announcement

- `AnnounceThrottle` (662-733) — the shared min-interval policy.
- `AnnounceHandle` (769-842) — legacy Hello/Presence announcements + event-id counter.
- `ControlAnnounceHandle` (864-1228) — control-plane HELLO/PRESENCE announcements + sequence counter.
- `DiscoveryService::announce_hello` (2382) / `announce_presence` (2388) / `announce_control_hello` (2404) / `announce_control_presence` (2412).
- Join-time announcement burst in `join` (2028-2109): legacy HELLO → control HELLO → CAPABILITIES → EXTENSIONS, each with its own throttle so the burst passes.
- Drain-loop neighbour-up re-announce (3289-3386): spawns fire-and-forget legacy hello + caps + extensions re-announcement.
- `presence_refresh_loop` (4084-4183): periodic control PRESENCE with jitter, plus every-N-tick caps/extensions refresh.
- Presence read APIs: `control_presence_count` (2907), `control_presence_peers` (2921) — snapshots of the BORU-CP-03 control presence store (hint cache, no authorisation).

### 4.4 Connectivity

- `connectivity_loop` (3452-3526): subscribes to `peer_updates` broadcast; on `Seen`/`Advertised` calls `maybe_dial` (once per endpoint per lifetime); 60 s debug snapshot dump.
- `maybe_dial` (3673-3732): `join_peers` dial with `dialed` dedup; feeds `EndpointConnecting`/`EndpointConnected`/`EndpointFailed` into the connectivity store; success cancels queued reconnect (`scheduler.reset`).
- `PeerConnectivityStore` (control_plane/connectivity.rs) — the explicit state machine (Unknown / Discovered / Connecting / Reachable / DirectTopicReady / Degraded / OfflineStale), deterministic transition table, bounded trail, `expire_stale`, `evict_one`.
- Event feeds into the state machine:
  - `DiscoverySeen` — legacy receive path (1435) and control receive path (1538).
  - `EndpointConnected` / `EndpointFailed` — drain-loop NeighborUp (3248) / NeighborDown (3399), `maybe_dial` (3703/3723), reconnect loop (3853/3877).
  - `Timeout` — presence-expiry sweep (3990, 4018).
  - `PathChanged*` — `path_refresh_loop` (3647-3650), diagnostic-only.
  - Data-plane reports — `DiscoveryService::report_connectivity_event` (3048) / `report_connectivity_failure` (3085): direct-topic join success/failure, direct message receipt, relay/direct path changes.
- `with_endpoint` (3111) + `path_refresh_loop` (3624) + `classify_peer_path` (3591) + `classify_path_addrs` (3566): BORU-CP-14 path classification (direct / relay / transitioning), 15 s cadence.
- Read APIs: `connectivity_state` (2942), `connectivity_trail` (2956), `connectivity_peers` (2971), `connectivity_store` (3032), `peer_diagnostics` (2996), `log_peer_diagnostics` (3013).

### 4.5 Reconnect scheduling

- `ReconnectScheduler` (control_plane/reconnect.rs): per-peer queue, exponential backoff (initial 2 s → max 300 s), max retry cadence, one active attempt per peer (`due` marks in-flight under the same lock).
- `ReconnectHandle` (control_plane/reconnect.rs:340): `queue_reconnect` (skips online peers), `report_topic_ready`, `is_reconnect_pending`.
- `reconnect_loop` (3759-3785): 1 s tick (`RECONNECT_LOOP_TICK`), drains due attempts.
- `drain_reconnect_attempts` (3789-3891): `join_peers` → `wait_for_reconnect_confirmation` (3 s) → on confirm: `scheduler.reset` + `ReconnectSignal::PeerReachable` (exactly once — drain-loop NeighborUp may already have surfaced it); on failure: `EndpointFailed` + `scheduler.on_failure`.
- `wait_for_reconnect_confirmation` (3896-3917): polls connectivity store every 100 ms until online or deadline.
- `DiscoveryService` surface: `reconnect_handle` (3142), `reconnect_events` (3153), `queue_reconnect` (3163), `reconnect_state` (3168), `with_reconnect_backoff` (3181).
- Drain-loop NeighborUp reconnect reset + signal emit (3269-3281); `report_connectivity_event` success reset (3061-3075); presence-expiry offline cancel (3997-3999, 4021-4024).
- App-layer trigger (main.rs): fresh announcement from a message-capable friend → `queue_reconnect`; `ReconnectSignal` forwarded to the app so it re-joins the direct topic.

### 4.6 Control-plane dispatch

- `ReceiveCore::handle_control_incoming` (1470-1812) — the full receive gate:
  1. `ControlEnvelope::decode` → `UnknownType` (forward-compat drop), `UnsupportedVersion` (drop), malformed (drop + count).
  2. self-filter.
  3. `guard.admit(&envelope, delivered_from, now)` → verdict: Accept / Reject(SpoofedSender | RateLimited | Duplicate | AdvertViolation).
  4. On accept: connectivity `DiscoverySeen`; then typed dispatch for PUBLIC_ROOM_ADVERTISEMENT (auth verify → cache apply → `ControlEvent::RoomAdvertisement`) and PUBLIC_ROOM_WITHDRAWAL (auth + authority check → cache apply → `ControlEvent::RoomWithdrawal`); otherwise generic `ControlEvent::Received`.
- `ControlPlaneGuard` (control_plane/privacy.rs): advert policy (whitelist + bounds), per-sender rate limiter (keyed on authenticated gossip source), `(sender_node_id, sequence)` dedup (bounded set), TTL presence store.
- `ControlEvent` (303), `IncomingOutcome` (388), `RoomAdvertisementEvent` (344), `RoomWithdrawalEvent` (371).
- Outbound: `send_control` (2867).

### 4.7 Capability / extension advertisement

- Local state: `local_caps` / `local_extensions` (`Arc<Mutex<>>`), replaced via `update_local_capabilities` / `update_local_extensions`.
- Announcement: `announce_capabilities` (1043) / `announce_extensions` (1097) with `force` (periodic refresh path) and `bypass_throttle` (neighbour-up path) semantics; `Unchanged` outcome for byte-identical payloads on the non-forced path.
- Cadence: `presence_refresh_loop` announces caps every `DEFAULT_CAPABILITIES_REFRESH_EVERY` (=3) ticks and extensions every `DEFAULT_EXTENSIONS_REFRESH_EVERY` (=3) ticks — roughly every 6 minutes at defaults.
- Neighbour-up: drain loop re-announces caps + extensions immediately (`force=true, bypass_throttle=true`).
- Negotiation view: `CapabilityGate` trait (1939), `DiscoveryCapabilityGate` (1959), `capability_gate` / `capability_gate_value`; read APIs `peer_capabilities` (1973 via guard presence store), `peer_extensions`, `peer_supports`.
- Models: `src/control_plane/capabilities.rs` (ids `files-v2`, versioned negotiation, unknown-id preservation), `src/control_plane/extensions.rs` (8 metadata-only extension sections, bounds validation).

### 4.8 Room-directory advertisement / expiry

- Outbound: `announce_room_advertisement` (1155) with BORU-DIR-04 emit-site guard (`NotDiscoverable` for non-PublicDiscoverable rooms) + `advert_throttle`; `announce_room_withdrawal` (1203).
- Inbound: advertisement decode + `verify_signed` + cache `apply_advertisement` (1623-1677) → `ControlEvent::RoomAdvertisement` on Added/Refreshed; withdrawal decode + auth + authority check + cache `apply_withdrawal` (1699-1755) → `ControlEvent::RoomWithdrawal`.
- Cache: `src/room_directory.rs` `RoomDirectory` — keyed by room_id, bounded (entry count + metadata bytes), merges duplicates deterministically, `sync_local_states` for BORU-DIR-12 local join facts, `evict_expired` for TTL.
- Expiry: `directory_expiry_loop` (4215-4247) every `DEFAULT_DIRECTORY_SWEEP_INTERVAL` (30 s) calls `evict_expired`; each entry carries its own `expires_after_secs` TTL (policy minimum 60 s, default 1 h). Wire: `directory_expiry_config`, `with_directory_sweep_interval`.
- Read handle: `room_directory()` (2855) handed to the app for `local_join_state` derivation; `directory_counters` (BORU-DIR-22: received / rejected / accepted / deduplicated / withdrawn / rate-limited / expired).

### 4.9 Lifecycle

- `join` (2010) — subscribe + split + `from_subscription_with_counters` + join-time announcement burst (legacy hello, control hello, caps, extensions).
- `start` (2126) — alias for `join` (PDF lifecycle API).
- `stop` (2141) / `shutdown` (3192) — cancel token + await all 7 task handles (drain, connectivity, expiry, refresh, reconnect, directory expiry, path).
- `from_subscription` (2155) / `from_subscription_with_counters` (2184) — the real constructor: builds all shared state, spawns 6 background tasks.
- `topic()` (2361), `joiner()` (3127) — `DiscoveryJoiner` for long-lived background sources (mDNS/DHT) to join peers into the mesh.
- Builder tunables: `with_announce_min_interval`, `with_control_announce_min_interval`, `with_capabilities_announce_min_interval`, `with_capabilities_refresh_every`, `with_extensions_announce_min_interval`, `with_extensions_refresh_every`, `with_advert_min_interval`, `with_presence_refresh_interval`, `with_presence_refresh_jitter`, `with_presence_ttl`, `with_presence_sweep_interval`, `with_directory_sweep_interval`, `with_reconnect_backoff`, `with_endpoint`.

---

## 5. Shared mutable state and lock boundaries

All shared state is behind `std::sync::Mutex` or `Atomic*`. There is no
`tokio::sync::Mutex` in this subsystem. Every lock is a `Mutex::lock().expect("... poisoned")`
— a panicking guard if a lock is poisoned. **No lock is held across an `.await`**
in the receive/announce paths (each lock scope is a short synchronous block).

| Shared object | Type | Lock boundary | Touched by |
|---------------|------|---------------|------------|
| `registry` | `Arc<Mutex<PeerRegistry>>` | one lock per upsert / prune / read | receive path, expiry sweep, read APIs |
| `guard` | `Arc<Mutex<ControlPlaneGuard>>` | one lock per `admit` / `expire_stale` / presence read | control receive path, expiry sweep, `DiscoveryCapabilityGate` reads |
| `connectivity` | `Arc<Mutex<PeerConnectivityStore>>` | one lock per `apply` / snapshot read | drain loop, connectivity loop, maybe_dial, reconnect loop, expiry sweep, path loop, report APIs, read APIs |
| `reconnect` | `Arc<Mutex<ReconnectScheduler>>` | one lock per `due` / `on_failure` / `reset` / `schedule` / state read | reconnect loop, drain loop (NeighborUp), maybe_dial, expiry sweep, `report_connectivity_event`, read APIs |
| `room_directory` | `Arc<Mutex<RoomDirectory>>` | one lock per `apply_advertisement` / `apply_withdrawal` / `evict_expired` / snapshot | control receive path, directory expiry loop, `room_directory()` consumers |
| `local_caps` | `Arc<Mutex<CapabilitySet>>` | one lock per read/replace | service API, refresh loop, drain loop |
| `local_extensions` | `Arc<Mutex<ExtensionsPayload>>` | one lock per read/replace | service API, refresh loop, drain loop |
| `expiry_config` / `refresh_config` / `directory_expiry_config` | `Arc<Mutex<...Config>>` | one lock per cycle read / builder write | builder, corresponding loop |
| `throttle` (×5 instances) | `Arc<AnnounceThrottle>` = `Mutex<AnnounceThrottleState>` | one lock per `try_announce` | announce handles (service + drain loop share the Arc) |
| `last_announced_caps` / `last_announced_extensions` | `Arc<Mutex<Option<...>>>` | one lock per compare/write | control announce handle |
| `next_event_id` | `Arc<AtomicU64>` | lock-free (Relaxed fetch_add) | announce handle (shared with drain) |
| `sequence` | `Arc<AtomicU64>` | lock-free (Relaxed fetch_add) | control announce handle |
| `counters` / `directory_counters` | `DiagnosticCounters` / `DirectoryCounters` (atomic) | lock-free | receive paths, expiry |
| `peer_updates_tx` / `control_events_tx` / `reconnect_tx` | `broadcast::Sender` (cap 256) | lock-free send; **lossy** — lagged receivers drop | producers + consumer tasks |
| `dialed` (connectivity loop) | `HashSet<EndpointId>` | task-local, single-threaded | connectivity loop only |
| `cancel` | `CancellationToken` | lock-free | all tasks |

Lock-order rule: when code takes two locks in one scope, the order is always
`connectivity` → `reconnect` (see `maybe_dial` 3690-3711, expiry sweep 3989-3999,
reconnect loop 3849-3864). No path takes `reconnect` then `connectivity`. The
receive path takes `registry` then `connectivity` (1356-1436). No code takes
`room_directory` with another lock held.

---

## 6. Timers and timeout-based transitions

### 6.1 Timer inventory (all in `discovery_service.rs` unless noted)

| Timer | Value | Where defined | Loop / usage | Re-read each cycle? |
|-------|-------|---------------|--------------|---------------------|
| `DEFAULT_ANNOUNCE_MIN_INTERVAL` | 30 s | :168 | legacy announce throttle (`AnnounceThrottle`) | n/a (throttle state) |
| `DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL` | 30 s | :175 | control / caps / extensions / advert throttles | n/a |
| `DEFAULT_PRESENCE_REFRESH_INTERVAL` | 120 s | :181 | `presence_refresh_loop` base delay | yes |
| `DEFAULT_PRESENCE_REFRESH_JITTER` | 60 s | :186 | `presence_refresh_loop` `sleep(interval + random(0..=jitter))` | yes |
| `DEFAULT_CAPABILITIES_REFRESH_EVERY` | 3 ticks | :195 | caps re-announce every N-th refresh tick (0 = off) | yes |
| `DEFAULT_EXTENSIONS_REFRESH_EVERY` | 3 ticks | :205 | extensions re-announce every N-th refresh tick (0 = off) | yes |
| `RECONNECT_LOOP_TICK` | 1 s | :210 | `reconnect_loop` sleep cadence (fresh sleep per iteration) | n/a |
| `RECONNECT_CONFIRM_TIMEOUT` | 3 s | :216 | `wait_for_reconnect_confirmation` deadline | n/a |
| `DEFAULT_PRESENCE_TTL` | 300 s | `control_plane/privacy.rs:76` | `presence_expiry_loop` legacy registry prune + control presence expiry | yes |
| `EXPIRY_SWEEP_INTERVAL` | 30 s | `control_plane/privacy.rs:109` | `presence_expiry_loop` sweep cadence | yes |
| `DEFAULT_DIRECTORY_SWEEP_INTERVAL` | 30 s | :232 | `directory_expiry_loop` sweep cadence | yes |
| `PATH_REFRESH_INTERVAL_SECS` | 15 s | :3535 | `path_refresh_loop` classification sweep | n/a |
| `DEFAULT_RECONNECT_INITIAL_BACKOFF` | 2 s | `control_plane/reconnect.rs:58` | `ReconnectScheduler` backoff start | n/a |
| `DEFAULT_RECONNECT_MAX_BACKOFF` | 300 s | `control_plane/reconnect.rs:62` | backoff cap | n/a |
| room advert TTL | min 60 s, default 1 h (per-entry `expires_after_secs`) | `room_directory.rs` | evicted by `directory_expiry_loop` | n/a |
| connectivity debug dump | 60 s | `connectivity_loop` :3466 | debug-only `snapshots_for` dump (guarded by `tracing::enabled!`) | n/a |
| reconnect confirmation poll | 100 ms | `wait_for_reconnect_confirmation` :3915 | poll interval inside the 3 s deadline | n/a |

All `tokio::time::interval` uses set `MissedTickBehavior::Skip`
(`path_refresh_loop` :3630, connectivity dump :3467). The `presence_refresh_loop`
and `reconnect_loop` use fresh `sleep` futures per iteration instead of `interval`
so the first tick is not immediate (deterministic one-tick cadence, test-friendly).

### 6.2 Timeout-based transitions

| Timeout | Trigger | Transition / effect |
|---------|---------|---------------------|
| Presence TTL (300 s) since last `DiscoverySeen` | `presence_expiry_loop` sweep | legacy registry `prune_older_than` → `PeerUpdate::Expired`; `connectivity.apply(Timeout)` → **OfflineStale**; `reconnect.reset` (cancels queued attempts) |
| Control presence TTL (300 s) | `presence_expiry_loop` sweep (guard `expire_stale`) | control presence store removal → same connectivity `Timeout` → OfflineStale + reconnect reset |
| Reconnect dial not confirmed within 3 s | `wait_for_reconnect_confirmation` deadline | `EndpointFailed` + `scheduler.on_failure` (exponential backoff) |
| Reconnect attempt fails | `join_peers` error | `EndpointFailed` + `scheduler.on_failure` |
| Room advert TTL elapsed | `directory_expiry_loop` sweep (`evict_expired`) | room removed from active directory (withdrawal is the fast path; TTL is the safety net) |
| 1 s reconnect tick | `reconnect_loop` | drains all due attempts (backoff deadlines checked every tick) |

No transition is driven by wall-clock alone except the TTL sweeps; the
connectivity state machine is fed only from real events plus the TTL `Timeout`
events (documented invariant, `control_plane/connectivity.rs`).

---

## 7. Invariants that must survive decomposition

These are the behaviours the BORU-DISC-002..009 extraction tasks must preserve
exactly. Each is enforced today by specific code; the extraction moves that code,
not the invariant.

### 7.1 Domain separation (PDF Core rule)
1. **The discovery service never creates conversations, never touches chat
   persistence, notifications, or rendering.** It has no reference to
   `ConversationStore` / `ChatCallbacks`; the module doc states the guarantee and
   the app must keep it structural after decomposition.
2. **The discovery topic carries only discovery metadata.** Legacy
   `DiscoveryMessage` (Hello / Presence / PeerAdvertisement) and control-plane
   `ControlEnvelope` (magic `BC`). No chat payload, attachment bytes, group
   history, tunnel payloads, or call media ever crosses it. The magic-byte
   disjointness (`0x42` cannot be a chat enum tag) must be preserved.
3. **`join_peers` is connectivity-only.** It forms a gossip mesh edge / resolves an
   address-book entry; it never grants friendship, group membership, or a
   conversation. Decomposition must not add friendship/trust decisions to the
   discovery path (the app owns friend-ness checks).

### 7.2 Wire and protocol compatibility
4. **Wire formats stay byte-stable.** Legacy `DiscoveryMessage` postcard framing,
   `ControlEnvelope` framing (magic `BC`, length-prefixed payload), and version
   checks (`check_discovery_version`, `BORU_APP_PROTOCOL_VERSION = 1`) are fixed
   unless a dedicated migration task changes them. Unknown control `message_type`
   → `UnknownControlType` drop; unknown payload fields → trailing bytes discarded;
   unsupported versions → dropped, client keeps running.
5. **Event-id / sequence semantics.** Legacy announcements: event ids are
   monotonic per process, seeded randomly per process; a suppressed (throttled)
   announcement does **not** consume an id. Control announcements: sequences are
   monotonic per process, seeded with wall-clock seconds; suppressed announcements
   do not consume a sequence. Receivers dedup by `(node_id, event_id)` / `(sender_node_id, sequence)`.

### 7.3 Peer registry and dedup
6. **Dedup key is `(node_id, event_id)`.** Same peer on two topics = one entry
   (`source_topic` updates). Same node + same event id = `Duplicate`, entry
   untouched. Legacy senders with no event id always refresh (never deduped).
7. **Restart re-discovery.** A same-id announcement from a peer currently
   `Degraded` / `OfflineStale` is a **restart**, not a duplicate: `refresh_after_restart`
   refreshes metadata without touching `last_event_id`. The registry is never
   mutated on a plain duplicate.
8. **Registry is authoritative; channels are lossy.** `PeerUpdate` /
   `ControlEvent` / `ReconnectSignal` broadcasts have capacity 256 and lagged
   receivers drop; no producer ever blocks on a full channel.

### 7.4 Announcement policy
9. **Throttle rules.** At most one legacy announcement per 30 s; at most one
   control HELLO/PRESENCE per 30 s; separate 30 s throttles for caps, extensions,
   and room advertisements. First announcement always passes. Neighbour-up
   re-announcements are fire-and-forget spawned tasks (never block the drain).
10. **Join burst order**: legacy HELLO → control HELLO → CAPABILITIES → EXTENSIONS
    (each with its own throttle so the burst passes). A failed announcement is
    non-fatal (`warn!` and continue).
11. **Capabilities/extensions idempotence**: non-forced announcement of an
    unchanged payload returns `Unchanged` (no broadcast); periodic refresh uses
    `force=true` so late-joining peers still learn the set; neighbour-up path uses
    `force=true, bypass_throttle=true`.

### 7.5 Receive gate
12. **Gate order is fixed**: legacy = deserialize → version check → self-filter →
    registry update; control = decode → version check → self-filter → guard
    `admit` → connectivity `DiscoverySeen` → typed dispatch. Self-originated
    messages are always ignored (both formats).
13. **Guard is a hard boundary**: `SpoofedSender` / `RateLimited` / `Duplicate` /
    `AdvertViolation` rejections drop the frame with bounded logging and counters;
    a rejected frame never touches the registry, connectivity, directory, or chat
    state. Rate limiting is keyed on the **authenticated gossip delivery source**,
    never the claimed envelope sender.
14. **Advertisement / withdrawal auth**: invalid signature → discard (count
    `AdvertisementAuthRejected` / `WithdrawalAuthRejected`); missing signature →
    emitted as clearly untrusted (`MissingSignature`), never canonical; verified →
    canonical only when the publisher is the room authority. A withdrawal may only
    remove a room when verified **and** signed by the room's designated authority
    (`WithdrawalNotAuthoritative` otherwise).

### 7.6 Connectivity state machine (BORU-DISC-002/003 targets)
15. **State machine is fed only from real networking events** (DiscoverySeen,
    EndpointConnected, EndpointFailed, TopicJoined, DirectMessageReceived,
    Timeout, PathChanged*). No timer, UI action, or gossip metadata fabricates a
    transition; TTL expiry produces the only time-driven transition (Timeout →
    OfflineStale).
16. **Transitions are deterministic and idempotent.** Duplicate events are
    no-ops; stale events cannot move a peer backward incorrectly; the transition
    table is table-tested. Duplicate dials are filtered by the connectivity loop's
    `dialed` set and by the state machine itself (no connection loops).
17. **Path classification is diagnostic-only.** `PathChanged*` events never move
    the state machine and never refresh liveness; a peer with no remote-info data
    stays `Unknown` and is skipped (a lack of information must not fabricate a
    path label or defeat TTL expiry). A relay-only path is NOT a failure — the
    peer stays `Reachable`.
18. **UI reads, never writes.** `connectivity_store()` is a read handle for the
    GUI; only the discovery service and the data-plane report API feed events
    (`report_connectivity_event` / `report_connectivity_failure`).

### 7.7 Reconnect (BORU-DISC-006 target)
19. **At most one active reconnect attempt per peer.** `ReconnectScheduler::due`
    marks attempts in-flight under the same lock that selects them. Repeated
    announcements while queued/in-flight are no-ops; already-online peers are
    never queued.
20. **Only real success clears backoff / emits signals.** `ReconnectSignal::PeerReachable`
    is emitted only after a confirmed dial; backoff is cleared only by
    `EndpointConnected` / `TopicJoined` / `DirectMessageReceived` (or the
    NeighborUp handler when a pending attempt existed). Discovery traffic alone
    never clears backoff. Exactly one signal per recovery (drain-loop NeighborUp
    and reconnect loop coordinate via `is_queued`).
21. **Offline cancels pending attempts.** The presence-expiry sweep resets the
    scheduler for expired peers so a later fresh announcement re-queues from an
    immediate attempt (no residual backoff).
22. **The scheduler is bounded** (max peers) and evicts deterministically when full.
23. **Reconciliation (BORU-CP-08) never resurrects relationships.** The pure
    decision restores only existing, non-archived, non-blocked direct
    conversations of the reconnected friend; groups and deleted/blocked
    relationships are never re-joined.

### 7.8 Room directory (BORU-DISC-009 target)
24. **The directory is a pure cache, never conversation state.** No
    `ConversationEntry`, no topic subscription, no history download, no permission
    grant. It has no reference to `crate::conversations` (structural guarantee).
25. **Bounded by construction**: entry-count and metadata-size bounds; duplicates
    merge deterministically (Added / Refreshed / Duplicate / Conflict / Unchanged);
    conflicts never churn UI. TTL remains the final cleanup mechanism (withdrawal
    is the fast path, sweep is the safety net).
26. **Only `PublicDiscoverable` rooms are advertised** (BORU-DIR-04 emit-site
    guard); local join state is derived from real local facts (BORU-DIR-12), never
    from the advertisement itself.

### 7.9 Lifecycle
27. **`shutdown` cancels and awaits every background task** (drain, connectivity,
    expiry, refresh, reconnect, directory expiry, path); dropping the handle
    without shutdown aborts them (documented). Every loop uses a `biased`
    `tokio::select!` with `cancel.cancelled()` first.
28. **Builder tuning takes effect without restart.** Loops re-read their shared
    config every cycle (sweep intervals, refresh interval/jitter/cadence), so
    `with_*` calls after construction are honoured.
29. **Counters stay separated**: peer/topic diagnostics vs room-directory
    diagnostics are distinct atomic sets; tests inject isolated instances.
30. **Offline testability**: `handle_incoming(&[u8], from)` is a pure byte-in,
    outcome-out function; the service must remain constructible without a live
    network (`from_subscription`), and the decomposition must not couple the
    receive core to the network.

---

## 8. Consumer / caller map

| Consumer | Surface used | Direction |
|----------|--------------|-----------|
| `examples/iced_chat/main.rs` | `DiscoveryService::start` (:1308), `with_endpoint`, `peer_updates()` (:1437), `reconnect_events()` + `reconnect_handle()` (:1347-1365), `joiner()` (:1376, mDNS), `room_directory()` (:1966), `capability_gate()` (:1954) | owns the service handle for app lifetime; feeds `DiscoveredPeersUpdate` to the sidebar; forwards `ReconnectSignal` to the app; queues reconnect for known message-capable friends |
| `examples/iced_chat/app.rs` | `CapabilityGate` (feature gating), `RoomDirectory` (public-rooms browsing + local join state), `ReconnectHandle` (reports `report_topic_ready`) | read + report; never writes discovery internals |
| `src/net.rs` / `src/dynamic_joiner.rs` | none directly — sibling joiner shares the `GossipSender` pattern | out of scope |
| Tests | `handle_incoming`, `announce_*`, loops with short configs, `peer_updates`/`control_events` receivers | offline |

The app owns: friend-ness checks (who is reconnect-eligible), direct-topic
re-join after `PeerReachable`, and room `local_join_state` facts. The discovery
service owns: everything in section 4. Keep that split when extracting.

---

## 9. Recommended decomposition targets (feeds BORU-DISC-004..009)

The PDF tasks map to already-existing module homes; the service facade keeps the
lifecycle + wiring. Recommended extraction boundaries:

| PDF task | What moves | From (discovery_service.rs) | To |
|----------|------------|------------------------------|-----|
| BORU-DISC-004 peer registry | `PeerRegistry`, `PeerRegistryEntry`, `PeerUpdate`, `PeerSource`, `UpsertOutcome` + registry update logic in `handle_incoming` (1355-1418) + expiry prune | 491-627, 1355-1418, 3976-4001 | new `src/peer_registry.rs` (or control_plane sibling); keep `ReceiveCore` holding `Arc<Mutex<PeerRegistry>>` |
| BORU-DISC-005 announcement/presence scheduling | `AnnounceThrottle`, `AnnounceHandle`, `ControlAnnounceHandle`, `announce_*` methods, `presence_refresh_loop`, join-time burst (2028-2109), neighbour-up re-announce (3289-3386) | 662-1228, 2382-2410, 4084-4183 | new `src/discovery_announce.rs` or `control_plane/announce.rs` |
| BORU-DISC-006 reconnect scheduler integration | `reconnect_loop`, `drain_reconnect_attempts`, `wait_for_reconnect_confirmation`, ReconnectHandle wiring, NeighborUp reset (3269-3281), report-API reset (3061-3075), expiry cancel (3997-3999) | 3759-3917 + call sites | `control_plane/reconnect.rs` (already owns the scheduler) |
| BORU-DISC-007 control-plane dispatch | `ReceiveCore::handle_control_incoming` (1470-1812), `ControlEvent` types, `send_control` | 1470-1812, 303-384, 2867 | `control_plane/` (guard already there); keep dispatch in `ReceiveCore` or a `control_plane/dispatch.rs` |
| BORU-DISC-008 capabilities/extensions advertisement | `local_caps`/`local_extensions` plumbing, `announce_capabilities`/`announce_extensions`, caps/extensions refresh cadence in `presence_refresh_loop` (4131-4178), neighbour-up caps/ext (3320-3386), `CapabilityGate`/`DiscoveryCapabilityGate` | 1959-1989, 2240-2252, 3320-3386, 4131-4178 | `control_plane/capabilities.rs` + `control_plane/extensions.rs` (models already there) |
| BORU-DISC-009 room-directory lifecycle | `announce_room_advertisement`/`withdrawal` (1155-1227, 2543-2572), advert/withdrawal receive handling (1569-1755), `directory_expiry_loop` (4215-4247), `room_directory()` handle | 1155-1227, 1569-1755, 2543-2572, 4215-4247 | `control_plane/advertisement.rs` + `room_directory.rs` (cache already there) |

Extraction rule from the PDF: move cohesive **state + messages + update logic
together**; keep `DiscoveryService` as a facade/coordinator; do not create
duplicate mutable state; keep wire compatibility unchanged.

---

## 10. Test coverage summary

- `discovery_service.rs` `#[cfg(test)] mod tests` (4265-9030, ~4 765 lines).
  Coverage includes: registry upsert/dedup/prune/restart-rediscovery, receive-gate
  outcomes (undecodable, unsupported version, self, duplicate, spoofed, rate
  limited, advert violation, advert/withdrawal auth), announcement throttle
  semantics, control announce sequencing, drain-loop forwarding + neighbour-up
  re-announce, connectivity-loop dial dedup + no-self-dial, presence-refresh and
  expiry loops, reconnect scheduler/handle, path classification.
- `control_plane/` modules each carry module-level unit tests (transition table
  determinism, wire round-trips, bounds validation, guard policies, scheduler
  backoff, reconciliation decisions).
- Integration coverage lives in `tests/` (end-to-end discovery/control-plane
  suites) — the BORU-DISC tasks must keep them green.
- Target test filter for the decomposition tasks: run the `discovery_service`
  module tests plus the `control_plane::*` module tests once via
  `rb test --bin boru --features gui,video-playback,terminal -- discovery` (and
  the targeted control-plane module names), then `rb check`.
