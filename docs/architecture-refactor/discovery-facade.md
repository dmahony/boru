# DiscoveryService facade — final data flow (BORU-DISC-010)

Status: describes the **end state** of the discovery decomposition
(BORU-DISC-004..010). Task BORU-ARCH-22 / PDF BORU-DISC-010.

This document records the final architecture of the internal discovery
subsystem after the extraction series shrunk `src/discovery_service.rs` into
a **facade / coordinator**. It answers: *"what does a gossip frame do from
the moment it arrives on the discovery topic to the events/effects it emits?"*

---

## 1. What the facade keeps

`DiscoveryService` is now a **facade / coordinator** — it keeps only:

1. **Lifecycle orchestration** — `join` / `start` / `stop` / `shutdown`,
   `from_subscription(_with_counters)`, the join-time announcement burst.
2. **High-level subscription wiring** — the `drain_loop` background task that
   reads the gossip receiver and dispatches each frame into the receive core.
3. **Module composition** — construction of the shared `Arc<Mutex<...>>`
   handles and the spawn of every background task, plus the thin facade
   accessors buildercallers (`with_*`, `announce_*`, `reconnect_*`,
   `connectivity_*`, capability-gate reads).

It no longer owns: the peer registry, announcement/presence scheduling, the
caps/extensions advertiser, the room-directory cache + expiry sweep, the
connectivity dial wiring, the path-classification sweep, the control-plane
receive dispatch, or the reconnect loop. Each of those is a dedicated module
(§ 3). No shared mutable state lives in the facade beyond the handles; no
module holds a second copy of any store.

### Concerns → module map

| Concern | Module |
|---------|--------|
| Peer registry + `(node_id, event_id)` dedup | `src/discovery/peer_registry.rs` |
| Announce throttles, announce handles, presence refresh/expiry loops | `src/discovery/presence_scheduler.rs` |
| Capabilities / extensions advertisement + neighbour-up re-announce | `src/discovery/caps_advertise.rs` |
| Room-directory cache + advert/withdrawal announce + TTL sweep | `src/discovery/directory_lifecycle.rs` |
| Connectivity wiring (dial discovered peers into the mesh) + single dial | `src/discovery/connectivity.rs` |
| Per-peer path classification sweep (BORU-CP-14) | `src/discovery/path_refresh.rs` |
| Control-plane receive dispatch (decode → validate → emit) | `src/control_plane/dispatch.rs` |
| Peer connectivity state machine | `src/control_plane/connectivity.rs` |
| Reconnect scheduler + loop + confirmation timeout | `src/control_plane/reconnect.rs` |
| Privacy/abuse guard + TTL presence store | `src/control_plane/privacy.rs` |

---

## 2. Architectural shape

```
                    examples/iced_chat (app layer)
                          │
        ┌─────────────────┼──────────────────────┐
        │                 │                      │
   service.peer_updates()  service.control_events()  service.reconnect_events()
   (PeerUpdate broadcast) (ControlEvent broadcast)   (ReconnectSignal broadcast)
        │                 │                      │
        ▼                 ▼                      ▼
   ┌──────────────────────────────────────────────────────────────┐
   │              DiscoveryService  (FACADE / COORDINATOR)         │
   │                                                              │
   │  drain_loop (high-level subscription wiring)                 │
   │   └── ReceiveCore::handle_incoming (magic sniff + dispatch)  │
   │                                                              │
   │  Composition of shared Arc handles + spawn of every task:    │
   │   drain · connectivity · presence-expiry · presence-refresh  │
   │   reconnect · directory-expiry · path-refresh                │
   └──────────────────────────────────────────────────────────────┘
        │                                  │
   gossip subscription               gossip sender
   (discovery topic)                  (broadcast)
```

All shared mutable state is behind `Arc<Mutex<...>>` / atomics owned by the
focused modules (§ 3 below); the facade constructs each once and passes
`Arc` clones around, so **no state exists in two places**.

---

## 3. Data flow — gossip input → state transition → emitted events/effects

### 3.1 Receive path (the short one)

The gossip receiver yields frames on the internal discovery topic. The drain
loop feeds each into `ReceiveCore::handle_incoming(content, delivered_from)`:

```
gossip frame
   │
   ▼
drain_loop (facade)
   │  event = receiver.next()
   ▼
ReceiveCore::handle_incoming
   │
   ├── magic == "BC"  (control-plane envelope)
   │     └──▶ control_plane::dispatch::ControlPlaneDispatcher::handle_incoming
   │            1. ControlEnvelope decode   → UnknownType/UnsupportedVersion/malformed ⇒ drop (count)
   │            2. self-filter             → SelfMessage ⇒ ignore
   │            3. guard.admit(...)        → Reject(SpoofedSender|RateLimited|Duplicate|AdvertViolation) ⇒ drop
   │            4. on accept:
   │                 connectivity.apply(DiscoverySeen)          (state machine)
   │                 PUBLIC_ROOM_ADVERTISEMENT ⇒ verify_signed + cache.apply → ControlEvent::RoomAdvertisement
   │                 PUBLIC_ROOM_WITHDRAWAL   ⇒ verify + authority    → ControlEvent::RoomWithdrawal
   │                 other                                     → ControlEvent::Received
   │
   └── otherwise  (legacy DiscoveryMessage)
        1. postcard decode / version check  → Undecodable | UnsupportedVersion ⇒ drop (count)
        2. self-filter                      → SelfMessage ⇒ ignore
        3. peer_registry.upsert: New|Refreshed  ⇒ PeerUpdate::Seen broadcast
                      Duplicate ⇒ restart-rediscovery if peer Degraded/OfflineStale, else ignore
        4. connectivity.apply(DiscoverySeen)     (state machine)
        5. PeerAdvertisement ⇒ PeerUpdate::Advertised broadcast
```

Every accepted legacy or control frame also updates the **peer connectivity
state machine** (`control_plane::connectivity`, `PeerConnectivityStore::apply`).
Rejected frames touch **no** registry, connectivity, directory, or chat state
(the guard is a hard boundary).

The two broadcast channels are the service's public event surface:
- `peer_updates` — `PeerUpdate::Seen / Advertised / Expired`.
- `control_events` — decoded control-plane events (`RoomAdvertisement`,
  `RoomWithdrawal`, generic `Received`).
- `reconnect_events` — `ReconnectSignal` (emitted only on real recovery).

### 3.2 Background tasks (each owned by a focused module, spawned by the facade)

| Task | Owned by | Input → transition → effect |
|------|----------|------------------------------|
| **drain_loop** | facade | gossip receiver → `ReceiveCore::handle_incoming` → broadcasts / state machine (above) |
| **connectivity_loop** | `discovery::connectivity` | `peer_updates` broadcast → `maybe_dial` (once per endpoint) via `GossipSender::join_peers` → connectivity `EndpointConnecting/Connected/Failed`; dial success resets reconnect backoff |
| **presence_expiry_loop** | `discovery::presence_scheduler` | TTL sweep → legacy registry `prune_older_than` (`PeerUpdate::Expired`), guard presence expiry, connectivity `Timeout` → OfflineStale, reconnect `reset` |
| **presence_refresh_loop** | `discovery::presence_scheduler` | tick (+ jitter, caps/ext every-N) → control PRESENCE / CAPABILITIES / EXTENSIONS announce via `ControlAnnounceHandle` / `CapsAdvertiser` |
| **reconnect_loop** | `control_plane::reconnect` | 1 s tick → drain due attempts → `join_peers` → confirmed ⇒ `ReconnectSignal::PeerReachable` + backoff reset; failed ⇒ `EndpointFailed` + `on_failure` |
| **directory_expiry_loop** | `discovery::directory_lifecycle` | sweep every `DEFAULT_DIRECTORY_SWEEP_INTERVAL` → `RoomDirectory::evict_expired` (TTL safety net) |
| **path_refresh_loop** | `discovery::path_refresh` | tick → iroh `remote_info` per peer → classify path → connectivity `PathChanged*` (diagnostic only, never transitions) |

### 3.3 Outbound (announcement) paths

- **Join-time burst** (facade `join`): legacy HELLO → control HELLO →
  CAPABILITIES → EXTENSIONS, each through its own throttle so the burst
  passes; failures are non-fatal (`warn!` + continue).
- **Neighbour-up re-announce** (drain loop): on `NeighborUp`, fire-and-forget
  legacy hello + `CapsAdvertiser::reannounce_on_neighbor_up` (caps/extensions
  immediately, `force=true, bypass_throttle=true`).
- **Periodic presence** (`presence_refresh_loop`): control PRESENCE with
  jitter, caps/extensions on an every-N-tick cadence.
- **Room advert/withdrawal** (`announce_room_advertisement` /
  `announce_room_withdrawal` → `directory_lifecycle`): signed control-plane
  envelopes, each with its own advert throttle, visibility guard refusing
  non-`PublicDiscoverable` rooms.
- **Raw** `publish` / `send_control`: unthrottled gateway for callers that
  already hold a built envelope.

---

## 4. Invariants preserved across the decomposition

(Wire and state invariants are unchanged; see also
`src/discovery_service.rs` module docs and each focused module's docs.)

- **Domain separation**: the service never creates/inserts conversations,
  never touches chat persistence/notification/rendering; discovery carries
  only `DiscoveryMessage` (legacy) + `ControlEnvelope` (magic `BC`).
- **Wire stable**: legacy postcard framing and control framing (`BC`,
  `BORU_APP_PROTOCOL_VERSION = 1`), event-id/sequence monotonicity, and the
  throttles are unchanged.
- **Receive gate order fixed**: legacy = deserialize → version → self-filter →
  registry; control = decode → version → self-filter → guard → connectivity →
  dispatch.
- **Guard is a hard boundary**: rejections drop the frame and touch no other
  domain.
- **State machine fed only by real network events** + the TTL `Timeout`;
  path changes and timestamp-only diagnostics never transition it.
- **No duplicate mutable state**: every store lives in exactly one module and
  is shared via `Arc` clones.
- **Connectivity is connectivity-only**: dialing never grants friendship,
  group membership, or a conversation.
