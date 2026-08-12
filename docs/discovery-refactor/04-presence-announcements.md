# 04 — Minimal presence announcements (BORU-CP-04)

Task 2.1 of the Hidden Discovery / Control Plane implementation steps.
Status: implemented (BORU-CP-04).

## Goal

Let Boru know that a peer has been seen *recently* **without using chat
messages**. A peer's presence is a **hint** derived from recent control-plane
announcements plus a TTL — never a persisted "online" flag.

## Design

### Wire format (reused from BORU-CP-01)

The control-plane envelope (`src/control_plane/message.rs`, magic `BC`)
already defined the presence payloads:

| Message | Payload | Metadata carried |
|---|---|---|
| `HELLO` | `HelloPayload { app_protocol_version }` | stable peer identity (envelope `sender_node_id`) + application protocol version |
| `PRESENCE` | `PresencePayload { ttl_secs }` | suggested TTL (receivers clamp it to their own default) |

`BORU_APP_PROTOCOL_VERSION = 1` is the version Boru advertises in its HELLO.
Unknown versions fail closed for that feature without breaking the client.

### Announcement behaviour (`src/discovery_service.rs`)

- **One announcement shortly after the discovery subscription becomes
  ready**: `DiscoveryService::join()` (the `start()` path every launch uses)
  publishes a control-plane HELLO right after the legacy join hello. The
  HELLO carries identity + protocol metadata, so peers' in-memory cache
  learns the node without any chat message.
- **Low-frequency refresh with jitter**: a new background task
  `presence_refresh_loop` publishes a control-plane PRESENCE every
  `interval + random(0..=jitter)` while the service runs. Defaults:
  `DEFAULT_PRESENCE_REFRESH_INTERVAL = 120 s`,
  `DEFAULT_PRESENCE_REFRESH_JITTER = 60 s` — comfortably under
  `DEFAULT_PRESENCE_TTL = 300 s`, so presence never goes stale between
  refreshes, and the jitter desynchronises nodes so a fleet does not announce
  in synchronised bursts.
- **Throttled**: control-plane announcements pass their own
  `AnnounceThrottle` (`DEFAULT_CONTROL_ANNOUNCE_MIN_INTERVAL = 30 s`),
  separate from the legacy discovery announce throttle, so neither path can
  starve the other. Suppressed announcements do not consume a sequence
  number.
- **Per-sender monotonic sequences**: each control announcement gets the
  next sequence from a per-sender counter; receivers dedup on
  `(sender_node_id, sequence)` (BORU-CP-01/03).

### In-memory peer-state cache (`src/control_plane/privacy.rs`)

`PeerControlStateStore` (from BORU-CP-03) is the cache, keyed by `peer_id`.
This task adds:

- `discovery_seen_at: Instant` — when the peer was FIRST seen (set on the
  first advertisement, preserved across refreshes; `last_seen` tracks
  activity recency).
- `PresenceState { Active, Stale }` — **derived** at read time from
  `last_seen` + TTL via `PeerControlState::presence_state(now)`, never
  stored. A peer is `Active` only while heard from within its TTL.

The cache records `last_seen`, `discovery_seen_at`, `protocol_version`,
`app_protocol_version`, capabilities, and effective TTL for every valid
announcement from a known/relevant peer (the BORU-CP-03 guard already
validates, rate-limits, dedups, and attributes before the store is touched).

### Guardrails honoured

- **No control-plane/chat coupling**: announcements go through
  `send_control` → gossip broadcast. Nothing touches `ChatHistoryStore`,
  conversation stores, unread counts, or rendering (proven by the wire-spy
  assertions in the discovery test suites).
- **No authorisation by presence**: the cache is metadata only; it grants no
  friendship/group/file/tunnel access (BORU-CP-03 structural tests).
- **Bounded resources**: cache capped (`MAX_CONTROL_PEERS`), per-sender rate
  limit, `(sender, sequence)` dedup, TTL expiry (BORU-CP-03).
- **Backward compatibility**: unknown message types / versions fail closed
  for that feature (BORU-CP-01 decoder).
- **Observability**: logs state transitions (`control: presence refresh
  announced`), never message contents.
- **Deterministic topic ownership**: the DiscoveryService owns the discovery
  topic subscription; the refresh loop reuses the service's sender.
- **Idempotence**: duplicate announcements dedup on `(sender, sequence)`;
  the refresh loop shares the throttle so a tick near an explicit announce
  is suppressed.
- **Never persist 'online'**: presence is always derived from recent
  activity + TTL; stale entries are pruned by the expiry sweep.

## Builder knobs (tests / tuning)

```rust
service.with_control_announce_min_interval(d) // control throttle spacing
service.with_presence_refresh_interval(d)      // refresh base delay
service.with_presence_refresh_jitter(d)        // per-cycle jitter (0 = fixed)
```

## Tests

Unit (`src/discovery_service.rs`, `src/control_plane/privacy.rs`):
- `announce_control_hello_broadcasts_control_envelope` /
  `announce_control_presence_broadcasts_control_envelope` — wire format
  round-trip, identity + metadata, never a legacy DiscoveryMessage.
- `control_announce_sequences_are_monotonic` — HELLO then PRESENCE → 0, 1.
- `announce_control_throttle_suppresses_rapid_repeat` — control throttle is
  independent of the legacy throttle.
- `presence_refresh_loop_publishes_periodic_control_presence` /
  `presence_refresh_loop_stops_on_shutdown` — the loop announces PRESENCE
  and is cancelled by `shutdown()`.
- `control_announce_own_echo_is_ignored` — our own echo never registers us.
- `handle_incoming_control_hello_sets_discovery_seen_at_and_protocol` —
  cache records discovery_seen_at / protocol_version / app version, Active.
- `presence_store_tracks_discovery_seen_at_across_refresh` /
  `presence_state_is_derived_from_activity_and_ttl_never_stored` (privacy).

Integration (`tests/test_discovery_two_node.rs`, `tests/test_discovery_restart.rs`):
- `two_nodes_exchange_control_presence_without_chat` — A and B discover each
  other's control presence over a real loopback mesh; no chat open, no chat
  payload on the wire.
- `control_presence_goes_stale_after_peer_disappears` — after one node
  stops announcing, its presence expires from the other's cache past the TTL.
- `restart_restores_control_presence_without_manual_action` — a restarted
  node's startup HELLO restores its presence in the peer's cache.
