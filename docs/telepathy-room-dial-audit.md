# Telepathy RoomDialScheduler vs Boru room dial patterns — audit

Date: 2026-08-02
Scope: boru room/peer connection management vs Telepathy's `RoomDialScheduler`
(reconciliation pattern) in `rust/telepathy-core/src/internal/core.rs`
(telepathy repo @ chanderlud/telepathy, cloned at HEAD for this audit).

Telepathy reference points:
- `core.rs:71` `ROOM_DIAL_CONCURRENCY: usize = 4`
- `core.rs:72` `ROOM_DIAL_RECONCILE_INTERVAL = 1s`
- `core.rs:74-78` `ROOM_DIAL_BACKOFF_BASE_MS=100`, `ROOM_DIAL_BACKOFF_MAX_MS=30_000`,
  `ROOM_DIAL_MAX_RETRIES=10`, `ROOM_DIAL_EXISTING_SESSION_BACKOFF=5s`
- `core.rs:375-377` scheduler + reconcile interval timer
- `core.rs:487-511` reconcile invoked on dial-completion events, on
  `room_reconcile` notify, and on the 1s timer
- `core.rs:582-645` `reconcile_room_dials` — computes desired set, admitted
  sessions, rearm notifications, then launches ready dials
- `core.rs:3528-3540` `RoomDialScheduler` struct (dials + rearms + attempt ids)
- `core.rs:3856-3990` `reconcile` / `take_ready` / `complete` / `is_in_flight` / `rearm`
- `core.rs:3998-4008` `room_dial_backoff` — 100ms * 2^(n-1) capped at 30s

---

## 1. Periodic reconciliation loop

### Telepathy
`core.rs:375-377` creates a `RoomDialScheduler` and a 1-second
`interval(ROOM_DIAL_RECONCILE_INTERVAL)`. The manager loop re-runs
`reconcile_room_dials` on every timer tick (`core.rs:509-511`), on every
`room_reconcile` notification (`core.rs:497-499`, fired by `request_room_reconcile`
at `core.rs:137-138`, 900, 983, 2414), and after every dial event
(`core.rs:487-491`). `reconcile_room_dials` (`core.rs:582-645`):

1. Reads the current room state (generation + desired peer set, minus self and
   only peers where `local < peer` — a tiebreaker splitting the dial burden)
   (`core.rs:591-600`).
2. Snapshots active sessions (`core.rs:602-609`).
3. Computes *admitted* sessions (peers in the desired set whose session is
   admitted to the current room generation) and re-arms non-admitted retained
   members (`core.rs:612-631`).
4. Calls `room_dials.reconcile(desired, admitted, active, now)` (`core.rs:632`)
   which **retains** only dials matching the current room generation and desired
   set, **cancels** everything else, and inserts new dial states for desired
   peers with no existing state (`core.rs:3856-3890`).
5. Launches every ready dial from `take_ready` (`core.rs:634-643`).

So telepathy re-evaluates desired vs connected on a fixed 1s cadence *and* on
every relevant event. This is a true reconciliation loop.

### Boru
There is **no desired-vs-connected reconciliation loop**. Boru's periodic
machinery is:

- **Stale-dial cleanup timer** (`net.rs:115-118`, `428-437`): a 10s
  `STALE_DIAL_CHECK_INTERVAL_S` timer sends `CleanupStaleDials`; the actor aborts
  dials older than `STALE_DIAL_THRESHOLD_S=15s` and schedules retries for
  non-active peers (`net.rs:485-499`). This is age-based dial hygiene, not
  membership reconciliation.
- **HyParView mesh self-healing** (`proto/hyparview.rs`): per-topic active view
  is capped at `active_view_capacity=5` (`hyparview.rs:202`); when the active
  view drops below capacity, `refill_active_from_passive` (`hyparview.rs:600-640`)
  picks a random passive peer and dials it. Shuffle every 60s
  (`hyparview.rs:216`) and `neighbor_request_timeout=500ms` (`hyparview.rs:218`)
  keep the mesh populated. This is protocol-level mesh maintenance, not an
  application-level "these are the room's desired peers" check.
- **App-level periodic ticks** (`examples/iced_chat/app.rs:23609-23614`):
  `ConnMonitorTick` (1s) handles presence/outbox/UI; `MeshWatchdogTick` (30s)
  computes mesh health and triggers stored-conversation auto-subscribe
  (`app.rs:14441+`); `OutboxRetryTick` (30s) retries queued sends. None of them
  recompute a desired peer set against connected peers.
- **DHT discovery loops** (`public_room_continuous.rs`): publish every 300s,
  discover every 30s (`public_room_continuous.rs:123-124`); discovered peers
  feed the `DynamicPeerJoiner` (`public_room_continuous.rs:428-516`). One-way
  addition — no removal of peers that left.

Room membership *is* tracked as a roster document (`room_docs.rs:694-708`), but
it is metadata synced over the mesh (`room_docs.rs:1-28`); it is never used to
recompute a dial target set.

### Gap assessment
**Gap exists.** Boru reacts to events (join/leave, dial failure, neighbor down,
discovery batch) and relies on HyParView's bounded mesh to self-heal. There is no
periodic re-evaluation of "who should be connected for this room" against "who is
connected", so boru cannot proactively dial a desired-but-missing peer on an
interval — it only dials when a discovery batch or mesh refill happens to
introduce it. Practical symptom: a peer that dropped from every view (active,
passive, joiner-known) is not re-dialed until a DHT discovery cycle (up to 30s)
or a new join event re-introduces it. Telepathy guarantees re-dial within 1s of
the reconcile tick.

---

## 2. Concurrent dial cap

### Telepathy
`ROOM_DIAL_CONCURRENCY = 4` (`core.rs:71`). `take_ready` computes
`available = 4 − (in-flight room dials)` and launches at most that many
(`core.rs:3908-3918`), then only takes dials that are not in flight, have no
session (active or admitted), are under `ROOM_DIAL_MAX_RETRIES`, and are past
their backoff (`core.rs:3919-3930`). The cap is global across all room dials.

### Boru
There is **no global cap on concurrent outgoing dials in the gossip
`Dialer`** (`net.rs:1446-1464`): `queue_dial` dedupes per peer
(`is_pending`, `net.rs:1448-1450`) but every unique peer spawns into the
`JoinSet` (`net.rs:1457-1463`) with no semaphore or upper bound on distinct
concurrent dials.

Boru's effective caps are indirect:
- **HyParView active view = 5 per topic** (`hyparview.rs:202`) plus pending
  neighbor requests; `refill_active_from_passive` refuses to dial past capacity
  (`hyparview.rs:601-604`). So per room, at most ~5 active + a handful of pending
  dials. But there is no cap **across** topics, and the `Dialer` itself is
  topic-agnostic (a peer dialed for one topic occupies the peer entry globally).
- **DynamicPeerJoiner** caps concurrent *joins* at `max_concurrent_joins=5`
  (`dynamic_joiner.rs:73`, semaphore at `dynamic_joiner.rs:224-226`). This
  bounds DHT-discovered joins, not gossip dials.
- `public_room_config.rs:149-155` documents `max_concurrent_joins=5` as
  "informational / advisory".

### Gap assessment
**Partial gap.** Boru is bounded per-topic by HyParView capacity (5) and per-joiner
by a semaphore (5), but the raw gossip `Dialer` has no global concurrency limit.
With many subscribed rooms or large `join_peers` batches, concurrent dials can
exceed telepathy's hard cap of 4. No single number bounds all outgoing room
dials. This is a resource-exhaustion surface (`docs/resource-exhaustion-mitigations.md`
covers send paths; dial bursts are not similarly capped).

---

## 3. Stale dial cancellation (peer no longer desired)

### Telepathy
Dials carry a `CancellationToken` and a room generation
(`core.rs:3536-3546`, `RoomDialLaunch.cancel`). `reconcile` retains dials only if
`dial.room_generation == room_generation && desired_peers.contains(peer)`;
anything else gets `dial.cancel.cancel()` and is dropped (`core.rs:3863-3872`).
The in-flight connect task is `select!`-ed against `launch.cancel.cancelled()`
(`core.rs:655-665`), and after a successful connect the code re-checks
`launch.cancel.is_cancelled()` and closes the connection if the dial was
cancelled while in flight (`core.rs:669-675`). `cancel_all` (`core.rs:3981-3988`)
fires on room exit. So a peer that leaves the room has its in-flight dial
aborted and its completed connection torn down.

### Boru
- `OutEvent::DisconnectPeer` removes the peer from `self.peers`
  (`net.rs:1078-1082`) — this drops the *established* connection's senders.
- **In-flight dials are NOT cancelled when a peer stops being desired.**
  Dials live in the topic-agnostic `Dialer` (`net.rs:1420-1430`); there is no
  per-topic dial ownership and no cancellation call site on topic quit. When a
  topic is quit (`net.rs:828-839` → `ProtoCommand::Quit` → HyParView
  `handle_quit` sends disconnects for active-view members, `hyparview.rs:358-363`),
  the `Dialer`'s pending dials for those peers keep running.
- The only dial cancellation is age-based: `cleanup_stale_dials`
  (`net.rs:1478-1509`) aborts dials older than 15s. And the abort path
  **schedules a retry** (`schedule_retry`, `net.rs:496`, `647-676`) for peers
  that aren't active — i.e. boru retries dials it aborted for staleness rather
  than dropping them, and nothing checks whether the peer is still in any
  desired room.

### Gap assessment
**Gap exists.** Boru cannot cancel an in-flight dial because the target peer left
the room; stale dials are only aborted by age, and then retried. A completed-but-
unwanted dial is accepted as an `Active` connection (no desired-set check at
`handle_connection`, `net.rs:678-704`). Telepathy's generation-scoped cancellation
prevents both the wasted dial and the lingering connection.

---

## 4. Existing-session detection

### Telepathy
`RoomDialState` tracks `has_session` / `has_live_session`
(`core.rs:3530-3535`). `reconcile` updates these flags each pass
(`core.rs:3877-3893`); `take_ready` refuses to launch dials for peers with an
active or admitted session (`core.rs:3921-3925`). If a session is lost, the dial
state is re-armed with `ROOM_DIAL_EXISTING_SESSION_BACKOFF=5s` before retrying
(`core.rs:3877-3882`).

### Boru
Existing-session detection exists at multiple layers:
- **PeerState gating**: `handle_in_event_inner`'s send path routes to the active
  send channel when `PeerState::Active` (`net.rs:863-874`) and only dials when
  `PeerState::Pending` (`net.rs:875-877`). Dials complete into
  `handle_connection`, which resolves a collision by keeping the existing
  connection and rejecting the new one (`net.rs:695-703`,
  `accept_conn`/`should_keep_new_session` `net.rs:1140-1196`).
- **Dialer dedup**: `queue_dial` skips peers already being dialed
  (`net.rs:1448-1450`).
- **Retry gating**: `schedule_retry` only retries when the peer is not active
  (`net.rs:489-491`, `542-544`, `555-557`).
- **Joiner gating**: `DynamicPeerJoiner` skips peers already known
  (`dynamic_joiner.rs:338-341`) and removes pending entries on `NeighborEvent::Up`
  (`dynamic_joiner.rs:288-299`).

### Gap assessment
**No meaningful gap.** Boru already skips dialing peers that have an active
session, at both the gossip `PeerState` layer and the joiner layer, and it
resolves simultaneous-dial collisions deterministically. Telepathy's
`has_session` flag is functionally equivalent; boru's is distributed across
layers rather than centralized in one scheduler, but the behavior (don't dial
what's connected) is present.

---

## 5. Rearm throttling (minimum backoff before retrying same peer)

### Telepathy
Dial failures go through `complete` (`core.rs:3940-3952`): in-flight cleared,
`retries += 1`, `next_attempt_at = now + room_dial_backoff(retries)`, where
`room_dial_backoff` is `100ms * 2^(n-1)` capped at 30s (`core.rs:3998-4008`),
max 10 retries. `take_ready` won't launch a dial before `next_attempt_at`
(`core.rs:3925`). Session re-negotiation for retained non-admitted members is
additionally gated by `rearm` with the same backoff curve (`core.rs:3956-3975`).
So a failed peer cannot be re-dialed sooner than ~100ms, 200ms, 400ms… up to 30s,
and is dropped after 10 attempts.

### Boru
- **Gossip dial retry** (`net.rs:646-676`): `MAX_DIAL_RETRIES=3`, base 5s,
  `RETRY_BASE_DELAY_S * 2^(attempts-1)` capped at `RETRY_MAX_DELAY_S=60s`
  (`net.rs:109-114`, `647-676`). Retry map is cleared on success
  (`net.rs:536`) or exhaustion (`net.rs:672`). So a failed dial waits ≥5s, then
  ≥10s, then ≥20s, then gives up.
- **Joiner retry** (`dynamic_joiner.rs:406-465`): `max_retries_per_peer=3`
  (wired from `start_with_joiner`, `public_room_continuous.rs:449`), initial 1s
  doubling to 60s with jitter (`dynamic_joiner.rs:441-451`).
- **DHT op retry** (`public_room_continuous.rs:846-895`): 1s base doubling to
  60s with jitter.
- **Whisper**: `session_manager.rs:32-39` defines `BACKOFF_BASE=1s`,
  `BACKOFF_MAX=60s`, `MAX_RECONNECT_ATTEMPTS=10` — **but this module is dead
  code**. No code instantiates `SessionManager` or calls `start_session`
  (verified: the only reference is `pub mod session_manager;` in
  `src/whisper/mod.rs:19`). The real whisper path (`src/whisper/mod.rs`) does a
  single 15s-timeout connect per send (`mod.rs:604-609`) with no retry/backoff;
  on `Disconnected` the app merely logs (`app.rs:10949-10956`) and reconnects
  lazily on the next send via `get_or_connect` (`mod.rs:542-588`).

### Gap assessment
**Partial gap.** The gossip and joiner layers do have per-peer backoff (5s/3
retries and 1s/3 retries respectively), so the *minimum* backoff requirement is
met for mesh dials. But: (a) boru's retry budget (3) is far below telepathy's
10, so a flaky-but-eventually-reachable peer is abandoned much sooner; (b) the
whisper reconnect-with-backoff exists only as uninstantiated code — the one
place boru declared intent to throttle whisper reconnects never runs; and (c)
there is no rearm-style throttle for re-negotiating a retained-but-not-admitted
member (boru has no admission/generation concept per peer).

---

## Summary table

| # | Pattern | Boru status | Telepathy | Gap? |
|---|---------|-------------|-----------|------|
| 1 | Periodic reconciliation loop | None — event-driven + HyParView self-heal + 10s stale-dial timer (`net.rs:115-118,428-437`); app ticks are presence/health only (`app.rs:23609-23614`) | 1s reconcile timer + event + notify (`core.rs:375-377,487-511,582-645`) | **Yes** |
| 2 | Concurrent dial cap | No global cap on gossip `Dialer` (`net.rs:1446-1464`); indirect caps: HyParView 5/topic (`hyparview.rs:202`), joiner semaphore 5 (`dynamic_joiner.rs:224-226`) | Hard cap 4 (`core.rs:71,3908-3918`) | **Partial** |
| 3 | Stale dial cancellation | None for membership change; age-only abort that then *retries* (`net.rs:1478-1509,496`) | Per-dial CancellationToken + generation check + post-connect close (`core.rs:3863-3872,655-675`) | **Yes** |
| 4 | Existing-session detection | Yes — PeerState gating (`net.rs:863-877`), collision keep-existing (`net.rs:695-703`), dialer dedup (`net.rs:1448`), joiner known-set (`dynamic_joiner.rs:338`) | `has_session`/`has_live_session` flags (`core.rs:3877-3925`) | **No** |
| 5 | Rearm throttling | Gossip: 5s base, 60s cap, 3 retries (`net.rs:647-676`); joiner: 1s→60s, 3 retries (`dynamic_joiner.rs:406-465`); whisper backoff is **dead code** (`whisper/session_manager.rs:32-39`) | 100ms→30s curve, 10 retries, `rearm` gate (`core.rs:3940-3975,3998-4008`) | **Partial** |

## Concrete recommendations (if this were implemented)

1. **Add a reconciliation tick** (highest value): a ~5–10s timer in the app or
   net layer that recomputes, per subscribed room, the set of peers known via
   roster/ticket/discovery vs current `neighbors`, and calls `join_peers` for
   missing ones. Reuses existing `GossipSender::join_peers` — no new dial
   machinery needed. (Dials are already deduped by the `Dialer`.)
2. **Cap the `Dialer`**: add a global semaphore (e.g. 8–16) around
   `queue_dial` spawns in `net.rs:1457-1463`, mirroring
   `DynamicPeerJoiner`'s semaphore pattern.
3. **Make stale aborts desired-aware**: when `cleanup_stale_dials` aborts,
   check whether the peer is still in any topic's desired set before
   `schedule_retry`; drop the retry if not. Requires per-peer topic
   membership tracking or a simple "is peer in any active view" check.
4. **Wire or delete the whisper `SessionManager`**: it is the only
   implementation of throttled whisper reconnect and it never runs; either
   instantiate it (replacing the lazy `get_or_connect` path) or remove it to
   avoid dead-code drift.
5. **Raise retry budget** for dial failures from 3 to ~10 with a tighter base
   (e.g. 1–2s) to match telepathy's tolerance for flaky peers without
   hammering.

## Evidence index (boru file:line)

- `src/net.rs:109-118` — retry/backoff + stale-dial constants
- `src/net.rs:428-437` — stale-dial cleanup timer spawn
- `src/net.rs:485-499` — CleanupStaleDials handling (abort → schedule_retry)
- `src/net.rs:529-566` — dial completion handling (success/fail/disconnect → retry)
- `src/net.rs:646-676` — `schedule_retry` exponential backoff, MAX_DIAL_RETRIES=3
- `src/net.rs:678-704` — `handle_connection` collision / keep-existing
- `src/net.rs:828-839` — topic quit path
- `src/net.rs:863-877` — PeerState Active vs Pending send/dial gating
- `src/net.rs:1078-1082` — DisconnectPeer removes established peer
- `src/net.rs:1446-1464` — `queue_dial` (per-peer dedup, no global cap)
- `src/net.rs:1478-1509` — `cleanup_stale_dials` (age-only abort)
- `src/net.rs:1513-1585` — `next_conn` (JoinSet join with 20s timeout)
- `src/dynamic_joiner.rs:64-108` — joiner config (max_concurrent_joins=5, retries)
- `src/dynamic_joiner.rs:224-226` — concurrency semaphore
- `src/dynamic_joiner.rs:286-310` — NeighborEvent Up/Down handling
- `src/dynamic_joiner.rs:397-466` — per-peer join retry with backoff
- `src/public_room_continuous.rs:123-124` — publish/discover intervals
- `src/public_room_continuous.rs:428-516` — `start_with_joiner` wiring
- `src/public_room_continuous.rs:846-895` — `retry_with_backoff` (1s→60s)
- `src/public_room_config.rs:113-124` — retry_backoff_min/max fields
- `src/proto/hyparview.rs:202-218` — active view cap 5, shuffle 60s, neighbor timeout 500ms
- `src/proto/hyparview.rs:358-363` — `handle_quit` (disconnect active view)
- `src/proto/hyparview.rs:600-640` — `refill_active_from_passive` (mesh self-heal)
- `src/whisper/mod.rs:19` — `pub mod session_manager` (only reference)
- `src/whisper/mod.rs:542-629` — `get_or_connect` / `connect_to_peer` (15s timeout, no backoff)
- `src/whisper/session_manager.rs:32-39` — reconnect backoff constants (dead code)
- `examples/iced_chat/app.rs:23609-23614` — 1s/30s app subscription ticks
- `examples/iced_chat/app.rs:14441+` — MeshWatchdogTick (health, auto-subscribe)
- `examples/iced_chat/app.rs:10949-10956` — whisper Disconnected is log-only
- `src/room_docs.rs:694-708` — roster member entries (metadata, not dial targets)

## Evidence index (telepathy file:line)

- `rust/telepathy-core/src/internal/core.rs:71-78` — concurrency/backoff constants
- `core.rs:137-138` — `request_room_reconcile`
- `core.rs:375-377` — scheduler + reconcile interval
- `core.rs:487-511` — reconcile trigger points (dial event / notify / timer)
- `core.rs:582-645` — `reconcile_room_dials`
- `core.rs:655-684` — `open_room_session` (cancel + post-connect close)
- `core.rs:3528-3546` — `RoomDialScheduler` / `RoomDialState` / `RoomDialLaunch`
- `core.rs:3856-3990` — `reconcile` / `take_ready` / `complete` / `rearm` / `cancel_all`
- `core.rs:3998-4008` — `room_dial_backoff`
