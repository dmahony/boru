# BORU-DISC-20: Discovery logging and diagnostics (PDF Phase 6)

Phase 6 of the hidden-discovery implementation ("Logging and diagnostics").
This step adds discovery-specific tracing and atomic counters so debugging can
prove **which topic a node actually joined** and **which traffic flows where**,
without ever logging private chat contents.

All changes are additive — no discovery topic derivation, no conversation
state, and no routing behaviour changed. Discovery stays networking
infrastructure: the new logs/counters only observe the existing paths.

## 1. Discovery join vs conversation join — separate log lines

The internal discovery topic join is logged **independently** of every
conversation-topic join, with a distinct message prefix (`discovery:` /
`joined internal discovery topic`) so a debug log makes it obvious which topic
a node actually joined.

| Path | Log line (tracing) | Level |
|---|---|---|
| Discovery join success (startup, `main.rs`) | `joined internal discovery topic` (topic field) | info |
| Discovery join failure (startup, `main.rs`) | `failed to join internal discovery topic; continuing without discovery service` | warn |
| Discovery service constructed (`from_subscription`) | `discovery service joined` (topic) | info |
| Discovery join hello announced | `discovery hello announced on join` (topic) | info |
| Discovery join hello suppressed by throttle | `discovery hello suppressed on join` | debug |
| Discovery join hello publish failed | `discovery hello on join failed; continuing without it` | warn |
| Discovery drain loop lifecycle | `discovery service drain loop started` / `... exited` (event_count) | info |
| Discovery shutdown | `discovery service shut down` (topic) | info |
| Conversation topic joined (background subscribe) | `background subscribed to direct conversation topic` / `background subscribed to group conversation topic` (topic) | info |
| Conversation topic opened explicitly | `room opened: direct conversation topic joined` / `room opened: group conversation topic joined` (topic) | info |
| Conversation subscribe failure | `background subscribe failed for {topic}` | warn |

The two families never share a log line: everything discovery-related carries
the `discovery:` prefix or the `joined internal discovery topic` message, and
conversation joins always state `direct` vs `group`.

## 2. Peer advertisements at debug level

Every accepted `PeerAdvertisement` on the discovery topic logs at **debug**
with the sender node id, the advertised peer, and the source topic:

```text
discovery: peer advertisement received node=<short> advertised=<short> source_topic=<topic>
```

Discovery payloads carry only node ids (no chat payloads are ever routed
through the discovery topic), so this log can never expose message contents or
private chat data. Hello/Presence registration logs are unchanged and also
carry the source topic:

```text
discovery: new peer seen node=<short> source=Hello topic=<topic>   (info)
discovery: peer refresh node=<short> source=Presence topic=<topic> (trace)
```

## 3. Direct/group subscription logging — independent counters

The four diagnostic counters live on the shared diagnostics singleton
(`boru_core::diagnostics::DIAGNOSTIC_COUNTERS`, an atomic
`DiagnosticCounters`):

| Counter | Incremented at |
|---|---|
| `discovery_peers_seen` | `DiscoveryService` receive path, on a **fresh** peer registration (`UpsertOutcome::New`). Refreshes and duplicate event ids do NOT bump it. |
| `direct_topics_joined` | iced frontend: `BackgroundSubscribed` success for a `ConversationKind::Direct` topic, and `RoomOpened` success for a direct topic. |
| `group_topics_joined` | iced frontend: `BackgroundSubscribed` success for a `ConversationKind::Group` topic, and `RoomOpened` success for a group topic. |
| `malformed_discovery_packets` | `DiscoveryService` receive path, on an undecodable (malformed) payload. |
| `unsupported_version_packets` | `DiscoveryService` receive path, on an unsupported-protocol-version payload (BORU-DISC-19 gate). Bonus counter beyond the PDF's four. |

Counters are `AtomicU64` behind `Arc`s inside `DiagnosticCounters`; clones
share the same atomics, so the discovery service (lib) and the iced frontend
(example) bump/read the same values lock-free. `DiagnosticCounters::snapshot()`
returns a `DiagnosticCountersSnapshot` with all five values.

Malformed discovery packets are also logged at debug (`discovery: undecodable
payload dropped`) and unsupported versions at warn (`discovery: unsupported
protocol version dropped`), so the counters and the log lines stay in sync.

## 4. Observing the counters

The counters are readable from anywhere that can reach the singleton:

```rust
use boru_core::diagnostics::DIAGNOSTIC_COUNTERS;
let s = DIAGNOSTIC_COUNTERS.snapshot();
println!(
    "peers_seen={} direct={} group={} malformed={} unsupported_version={}",
    s.discovery_peers_seen, s.direct_topics_joined, s.group_topics_joined,
    s.malformed_discovery_packets, s.unsupported_version_packets,
);
```

## 5. Log format and enabling

Log lines use the existing `tracing` macros (`info!` / `debug!` / `warn!` /
`trace!`) with structured fields (`topic`, `node`, `source`, `advertised`,
`source_topic`). To observe the full discovery trail:

```bash
RUST_LOG=info,boru_core::discovery_service=debug,boru_core::discovery_topic=debug ./boru
```

`discovery_service=debug` turns on the peer-advertisement and malformed-packet
lines; `trace` additionally shows self-message filtering, peer refreshes, and
duplicate-event drops.

## 6. Verification

- `rb check --lib --features net` PASS
- `rb check --bin boru --features gui,video-playback,terminal` PASS
- `rb test --lib --features net -- discovery_service` — 41/41 PASS
  (6 new counter tests: new-peer increments peers-seen only; malformed
  increments malformed only; unsupported-version increments unsupported only;
  duplicate does not bump peers-seen; refresh does not bump peers-seen; self
  message bumps nothing)
- `rb test --lib --features net -- counters_tests` — 3/3 PASS
  (record/snapshot accumulation; clone shares atomics; snapshot Eq)
