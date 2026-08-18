# 05 — Explicit peer connectivity state machine (BORU-CP-05 / BORU-DISC-002)

Task 2.2 of the Hidden Discovery / Control Plane implementation steps
(BORU-CP-05) and BORU-DISC-002 of the architecture refactor chain.
Status: implemented; updated for BORU-DISC-002.

**"Seen on discovery" is NOT "ready for direct messaging".** A peer may
announce itself on the internal discovery topic (legacy `DiscoveryMessage`
or control-plane HELLO/PRESENCE) long before its endpoint is dialable, its
deterministic direct topic is joined, or a direct message can actually
flow. This module defines the explicit per-peer connectivity state machine
so Boru never conflates presence with direct-messaging readiness.

The source of truth is `src/control_plane/connectivity.rs`; the pure
transition function `transition(state, event) -> Option<next>` **is** the
documented table. Keep this document and the code (and the table-driven
test `transition_table_matches_documented_expected_values`) in sync.

## States (`PeerConnectivityState`)

| State | Meaning | Ready for direct? |
|-------|---------|-------------------|
| `Unknown` | No information about the peer. | no |
| `Discovered` | Seen on the discovery topic (control or legacy announcement). | **no** |
| `Connecting` | An endpoint dial / connection attempt is in flight. | no |
| `Reachable` | Endpoint connected — gossip mesh edge established (`NeighborUp`, `join_peers` ok). | no (topic not yet joined) |
| `DirectTopicReady` | Deterministic direct topic joined AND direct messaging possible. | **yes** |
| `Degraded` | Previously reachable but a failure occurred (dial failed, topic join failed) — explicitly NOT 'online'. A relay-only path is NOT a failure (BORU-CP-14). | no |
| `OfflineStale` | Not heard from within the presence TTL / explicitly timed out. | no |

Derived accessors (the state-machine replacement for scattered 'online'
booleans):

- `is_online()` — `Reachable` | `DirectTopicReady`. A `Degraded` peer with a
  failed direct-topic setup is **never** reported as online.
- `is_ready_for_direct()` — `DirectTopicReady` only.
- `is_known()` — anything but `Unknown`.

## Events (`ConnectivityEvent`)

The state machine is updated **only** from real networking events
(PDF Task 2.2 step 3). No timer, UI action, or gossip metadata ever
fabricates a transition.

| Event | Source | Moves state? |
|-------|--------|--------------|
| `DiscoverySeen` | valid legacy `DiscoveryMessage` or control HELLO/PRESENCE received on the discovery topic | yes |
| `EndpointConnecting` | a dial / `join_peers` attempt is initiated | yes |
| `EndpointConnected` | `join_peers` ok, gossip `NeighborUp` | yes |
| `EndpointFailed` | `join_peers` err, gossip `NeighborDown` | yes |
| `TopicJoined` | deterministic direct topic subscribe/join success (data plane reports) | yes |
| `TopicJoinFailed` | deterministic direct topic subscribe/join failure (data plane reports) | yes |
| `DirectMessageReceived` | a direct (non-discovery) message received (data plane reports) | yes |
| `Timeout` | presence TTL expiry sweep | yes |
| `PathChangedDirect` | relay/direct path changed to direct | **no** (diagnostic-only, BORU-CP-14) |
| `PathChangedRelay` | relay/direct path changed to relay-only | **no** (diagnostic-only, BORU-CP-14) |
| `PathChangedTransitioning` | addresses known but none currently active | **no** (diagnostic-only, BORU-CP-14) |
| `DirectMessageSent` | outbound direct broadcast attempted (BORU-CP-13) | **no** (timestamp-only) |
| `InboundGossipEvent` | any inbound gossip event from the peer (BORU-CP-13) | **no** (timestamp-only) |
| `ApplicationMessageDecoded` | a message from the peer decoded to app processing (BORU-CP-13) | **no** (timestamp-only) |

Path events update the per-peer `path_kind` diagnostic hint and log path
changes; they never move the state machine — a relay connection is still
considered reachable, and a path transition never resets or duplicates
conversation state. Timestamp-only events refresh per-peer timing fields
(`last_outbound_direct`, `last_inbound_gossip`, `last_decoded_message`)
without ever fabricating connectivity progress.

## Transition table

`transition(state, event) -> Option<next>` in
`src/control_plane/connectivity.rs` **is** the documented table. `None`
means the event does not move the peer (idempotent no-op / illegal /
stale / diagnostic-only).

| From | Event → To |
|------|-----------|
| `Unknown` | `DiscoverySeen`→`Discovered`, `EndpointConnecting`→`Connecting`, `EndpointConnected`→`Reachable`, `DirectMessageReceived`→`DirectTopicReady`, `TopicJoined`→`DirectTopicReady` |
| `Discovered` | `EndpointConnecting`→`Connecting`, `EndpointConnected`→`Reachable`, `EndpointFailed`→`Degraded`, `TopicJoined`→`DirectTopicReady`, `TopicJoinFailed`→`Degraded`, `DirectMessageReceived`→`DirectTopicReady`, `Timeout`→`OfflineStale` |
| `Connecting` | `EndpointConnected`→`Reachable`, `EndpointFailed`→`Degraded`, `TopicJoined`→`DirectTopicReady`, `TopicJoinFailed`→`Degraded`, `DirectMessageReceived`→`DirectTopicReady`, `Timeout`→`OfflineStale` |
| `Reachable` | `EndpointFailed`→`Degraded`, `TopicJoined`→`DirectTopicReady`, `TopicJoinFailed`→`Degraded`, `DirectMessageReceived`→`DirectTopicReady`, `Timeout`→`OfflineStale` |
| `DirectTopicReady` | `EndpointFailed`→`Degraded`, `TopicJoinFailed`→`Degraded`, `Timeout`→`OfflineStale` |
| `Degraded` | `EndpointConnecting`→`Connecting`, `EndpointConnected`→`Reachable`, `TopicJoined`→`DirectTopicReady`, `DirectMessageReceived`→`DirectTopicReady`, `Timeout`→`OfflineStale` |
| `OfflineStale` | `DiscoverySeen`→`Discovered` (fresh announcement revives), `EndpointConnecting`→`Connecting`, `EndpointConnected`→`Reachable`, `TopicJoined`→`DirectTopicReady`, `DirectMessageReceived`→`DirectTopicReady` |

Every pair not listed is a `None` no-op (e.g. a second `DiscoverySeen` while
`Discovered` / `Reachable` / `DirectTopicReady` refreshes `last_seen` but
does not change state; a duplicate `EndpointConnecting` while `Connecting`
does not re-enter `Connecting`; a `Degraded` peer is not revived by an
announcement alone — only a real connection/topic/DM event advances it).

## Idempotence & connection-loop safety

- **Duplicate announcements**: re-delivering the same (sender, sequence) is
  already filtered by the BORU-CP-03 guard; even a *new* sequence from an
  already-known peer is an idempotent no-op in the state machine — it never
  re-enters `Connecting`, never spawns a second dial, never appends a trail
  record.
- **Duplicate dials**: `connectivity_loop` dedups by endpoint id (one dial
  per peer per service lifetime); the state machine adds a second layer
  (`Connecting` + `EndpointConnecting` → no-op).
- **Reordered events converge**: every transition depends only on the
  current `(state, event)`, never on history, so reordered gossip events
  converge and cannot regress a more-advanced state (a stale `Timeout`
  cannot resurrect a peer that just reconnected; a late `DiscoverySeen`
  cannot downgrade `Reachable`).

## Wiring in `DiscoveryService`

| Networking event | Where it is fed |
|------------------|-----------------|
| Legacy discovery announcement accepted | `ReceiveCore::handle_incoming` → `DiscoverySeen` |
| Control HELLO/PRESENCE accepted | `ReceiveCore::handle_control_incoming` → `DiscoverySeen` |
| Dial attempt begins | `maybe_dial` → `EndpointConnecting` |
| Dial succeeds / fails | `maybe_dial` → `EndpointConnected` / `EndpointFailed` (with error) |
| Gossip neighbour up / down | `drain_loop` → `EndpointConnected` / `EndpointFailed` |
| Presence TTL expiry | `presence_expiry_loop` → `Timeout` |
| Direct-topic join success/failure, DM receive, path change | data plane calls `DiscoveryService::report_connectivity_event` / `report_connectivity_failure` (direction: data-plane → control-plane state; the discovery service never calls chat code) |
| Reconnect attempt succeeds / fails | `drain_reconnect_attempts` → `EndpointConnected` / `EndpointFailed` |

Read API for diagnostics and the UI indicator (BORU-CP-06):

- `connectivity_state(peer)` — current state (`Unknown` if untracked)
- `connectivity_trail(peer)` — deterministic transition trail (oldest first)
- `connectivity_peers()` — full snapshot (state, path, direct-topic state,
  last errors, trail)
- `peer_diagnostics()` — share-safe per-peer snapshot (BORU-CP-13)

## Data model

The PDF's suggested `PeerControlState` shape is split across the two
control-plane stores, each with a single owner:

| Field | Where |
|-------|-------|
| `peer_id`, `discovery_last_seen`, `presence_state`, `protocol_version`, `capabilities` | `PeerControlState` (BORU-CP-03/04 `privacy.rs`) |
| `connection_state`, `path_kind`, `direct_topic_state`, `last_inbound_direct`, `last_outbound_direct`, `last_error` | `PeerConnectivityEntry` (BORU-CP-05 `connectivity.rs`) |

`PeerConnectivityEntry` also carries a bounded transition trail
(`MAX_TRAIL_PER_PEER = 64` records, oldest dropped first) — the
deterministic per-peer trail required by the acceptance criteria.

## Invariants (BORU-DISC-002)

1. **The state machine is fed only from real networking events.**
   `DiscoverySeen`, `EndpointConnecting`, `EndpointConnected`,
   `EndpointFailed`, `TopicJoined`, `TopicJoinFailed`,
   `DirectMessageReceived`, `Timeout`, and the diagnostic `PathChanged*` /
   timestamp-only events. No timer, UI action, or gossip metadata fabricates
   a transition; TTL expiry produces the only time-driven transition
   (`Timeout` → `OfflineStale`).
2. **Transitions are deterministic and idempotent.** Duplicate events are
   no-ops; stale events cannot move a peer backward incorrectly; the
   transition table is table-tested
   (`transition_table_matches_documented_expected_values`). Duplicate dials
   are filtered by the connectivity loop's `dialed` set and by the state
   machine itself (no connection loops).
3. **The transition function is pure.** `transition(state, event)` performs
   no networking/transport side effects; all side effects (logging, trail
   append, timestamp refresh, path-hint update, error recording) live in
   `PeerConnectivityStore::apply`.
4. **Path classification is diagnostic-only.** `PathChanged*` events never
   move the state machine and never refresh liveness; a peer with no
   remote-info data stays `Unknown` and is skipped. A relay-only path is NOT
   a failure — the peer stays `Reachable`.
5. **Failed direct-topic setup is visible, not 'online'.** `TopicJoinFailed`
   → `Degraded` with `last_error` recorded and `direct_topic_state=Failed`;
   `is_online()` is false. Recovery is explicit: a later `TopicJoined`
   clears the error.
6. **UI reads, never writes.** `connectivity_store()` is a read handle for
   the GUI; only the discovery service and the data-plane report API feed
   events (`report_connectivity_event` / `report_connectivity_failure`).
7. **Bounded memory.** `MAX_CONNECTIVITY_PEERS = 1024`; at capacity the
   store evicts stale-then-oldest entries. Trails are capped per peer.
8. **No authorisation by connectivity.** Reaching `DirectTopicReady` never
   makes a peer a friend/group member/tunnel client/file recipient; the
   store has no authorisation surface.

## Desired-vs-observed reconciliation (BORU-DISC-003)

The state machine above records **observed** connectivity facts only. It
says nothing about what the local user *wants*. BORU-DISC-003 separates the
two and adds a pure reconciliation function (`reconcile` in
`src/control_plane/connectivity.rs`) that decides which side effect is
required *now* to drive the observed facts toward the desired connectivity:

* **Desired connectivity** — [`DesiredConnectivity`]: `None` (not a target),
  `EndpointReachable` (a gossip mesh edge), or `DirectTopicReady` (the
  deterministic direct topic joined). This is the explicit statement of
  intent; reconciliation never guesses it.
* **Observed facts** — [`ObservedConnectivity`]: the peer's state-machine
  state plus the explicit reconnect scheduling/backoff input (whether an
  attempt is already queued/in-flight and how many failures have
  accumulated). BORU-DISC-003 objective 4: reconnect scheduling/backoff is
  an **input** to reconciliation (a field), not a scattered timing check in
  the caller.
* **Reconciliation** — [`reconcile`]: a pure, idempotent function returning
  [`ReconcileDecision`] — either `ScheduleReconnect` (the required side
  effect) or `NoAction` with a structured [`ReconcileReason`]
  (`NotDesired` / `AlreadySatisfied` / `ReconnectPending`).

Design rules:

- **Pure and idempotent**: `reconcile` mutates nothing, so it is safe to
  call repeatedly. Calling it twice with unchanged input returns the same
  decision and schedules no duplicate dial/publish/reconnect work.
- **Never double-queue**: a peer with a reconnect attempt already queued or
  in flight yields `ReconnectPending` → no second schedule. The reconnect
  scheduler's own dedup (`ReconnectScheduler::schedule` returning `false`)
  is the second safety net against duplicate work under concurrency.
- **Convergence**: because every transition depends only on the current
  `(state, event)`, repeated `reconcile` calls stop scheduling once the
  observed state satisfies the desire — late and duplicate events converge
  to the same final state regardless of the order they arrived in.
- **Behaviour-preserving wiring**: `ReconnectHandle::queue_reconnect`
  (app reconnect trigger for known friends) now routes through `reconcile`
  with `DesiredConnectivity::EndpointReachable` — semantically identical to
  the pre-reconciliation `is_online()` skip + scheduler dedup. A new
  `queue_reconnect_for(peer, desired)` expresses a different desire (e.g.
  `DirectTopicReady`) without changing the default path.
- **Observable**: every decision is logged at `info!`/`trace!` with the
  peer as the correlation identifier plus `desired`, `observed`, and
  `reason` fields, so reconnect behaviour is structured-log-observable.

Tests: the `reconcile_*` unit tests in `src/control_plane/connectivity.rs`
and the `queue_reconnect_for_*` tests in
`src/control_plane/reconnect.rs` cover idempotence, convergence, backoff
input, and the no-double-queue guarantee.

## Bounded resources / guardrails honoured

- **Bounded memory**: `MAX_CONNECTIVITY_PEERS = 1024`; at capacity the
  store evicts stale-then-oldest entries. Trails are capped per peer.
- **No authorisation by connectivity**: reaching `DirectTopicReady` never
  makes a peer a friend/group member/tunnel client/file recipient; the
  store has no authorisation surface.
- **Observability**: logs state transitions (`connectivity: peer state
  transition from=… to=… event=…`), never message contents.
- **No control-plane/chat coupling**: discovery handlers feed only control
  metadata; the data plane reports topic/DM/path events into the machine,
  never the reverse.
- **Deterministic topic ownership**: the discovery service owns the
  discovery topic; direct-topic state is a *hint* recorded from the data
  plane's reports.

## Tests

Unit (`src/control_plane/connectivity.rs`):

- `transition_table_matches_documented_expected_values` — the full
  expected-value table (BORU-DISC-002): every legal `(from, event, to)` is
  listed explicitly and every `(state, event)` pair not in the table is
  asserted to be a `None` no-op; re-applying the triggering event from the
  destination state must be an idempotent no-op.
- `transition_table_is_deterministic_and_idempotent`
- `duplicate_announcements_do_not_cause_connection_loops` — 100 duplicate
  DiscoverySeen events: state stays `Discovered`, trail stays length 1,
  never re-enters `Connecting`.
- `discovered_peer_is_not_direct_topic_ready` — the core distinction.
- `failed_direct_topic_setup_is_visible_not_online` — `TopicJoinFailed` →
  `Degraded` with `last_error`, `direct_topic_state=Failed`, `is_online()`
  false; recovery clears the error.
- `transition_trail_is_deterministic_and_ordered`
- `reordered_events_converge_and_never_regress`
- `expire_stale_moves_peers_to_offline`
- `unknown_peer_negative_evidence_creates_no_entry`
- `store_is_bounded` / `trail_is_bounded_per_peer`
- `path_changes_are_reflected`
- `path_hint_recorded_on_noop_path_event`
- `relay_connection_remains_reachable`
- `path_transitioning_is_diagnostic_only`
- `path_events_never_move_the_state_machine`
- `direct_message_receive_advances_to_ready`
- `timestamp_only_diagnostics_events_do_not_move_state`
- `timestamp_only_events_do_not_create_unknown_entries`
- `full_stage_timeline_records_all_timestamps`

Wiring (`src/discovery_service.rs`):

- `connectivity_legacy_discovery_marks_peer_discovered_not_ready`
- `connectivity_control_hello_marks_peer_discovered_idempotently`
- `connectivity_failed_direct_topic_is_visible_not_online`
- `connectivity_expiry_sweep_marks_peer_offline_stale`
- `connectivity_drain_loop_neighbor_events_feed_state_machine`
- `connectivity_direct_message_receive_reports_topic_ready`
- `service_path_relay_keeps_peer_reachable`
- `peer_diagnostics_snapshot_covers_every_stage`

Integration (`tests/test_discovery_two_node.rs`):

- `two_nodes_state_machine_discovered_but_not_direct_topic_ready` — over a
  real loopback mesh, A and B see each other as `Discovered` (NOT
  `DirectTopicReady`), the transition trail is deterministic, and a reported
  direct-topic failure is visible as `Degraded` — with zero chat payloads on
  the wire.
