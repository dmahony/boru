# 05 — Explicit peer connectivity state machine (BORU-CP-05)

Task 2.2 of the Hidden Discovery / Control Plane implementation steps.
Status: implemented (BORU-CP-05).

## Goal

**"Seen on discovery" is NOT "ready for direct messaging".** A peer may
announce itself on the internal discovery topic (legacy `DiscoveryMessage`
or control-plane HELLO/PRESENCE) long before its endpoint is dialable, its
deterministic direct topic is joined, or a direct message can actually
flow. This task introduces an explicit per-peer connectivity state machine
so Boru never conflates presence with direct-messaging readiness.

## States (`src/control_plane/connectivity.rs`)

| State | Meaning | Ready for direct? |
|-------|---------|-------------------|
| `Unknown` | No information about the peer. | no |
| `Discovered` | Seen on the discovery topic (control or legacy announcement). | **no** |
| `Connecting` | An endpoint dial / connection attempt is in flight. | no |
| `Reachable` | Endpoint connected — gossip mesh edge established (`NeighborUp`, `join_peers` ok). | no (topic not yet joined) |
| `DirectTopicReady` | Deterministic direct topic joined AND direct messaging possible. | **yes** |
| `Degraded` | Previously reachable but a failure occurred (dial failed, topic join failed) — explicitly NOT 'online'. A relay-only path is NOT a failure (BORU-CP-14): path type is a diagnostic, so a relay peer stays `Reachable`. | no |
| `OfflineStale` | Not heard from within the presence TTL / explicitly timed out. | no |

Derived accessors (the state-machine replacement for scattered 'online'
booleans):

- `is_online()` — `Reachable` | `DirectTopicReady`. A `Degraded` peer with a
  failed direct-topic setup is **never** reported as online.
- `is_ready_for_direct()` — `DirectTopicReady` only.
- `is_known()` — anything but `Unknown`.

## Events (real networking events ONLY)

The state machine is updated **only** from real networking events
(PDF Task 2.2 step 3). No timer, UI action, or gossip metadata ever
fabricates a transition.

| Event | Source |
|-------|--------|
| `DiscoverySeen` | valid legacy `DiscoveryMessage` or control HELLO/PRESENCE received on the discovery topic |
| `EndpointConnecting` | a dial / `join_peers` attempt is initiated |
| `EndpointConnected` | `join_peers` ok, gossip `NeighborUp` |
| `EndpointFailed` | `join_peers` err, gossip `NeighborDown` |
| `TopicJoined` | deterministic direct topic subscribe/join success (data plane reports) |
| `TopicJoinFailed` | deterministic direct topic subscribe/join failure (data plane reports) |
| `DirectMessageReceived` | a direct (non-discovery) message received (data plane reports) |
| `Timeout` | presence TTL expiry sweep |
| `PathChangedDirect` | **diagnostic only** (BORU-CP-14): an active direct (IP) path was observed — updates `path_kind`, never moves the state machine |
| `PathChangedRelay` | **diagnostic only** (BORU-CP-14): a relay-only path was observed — updates `path_kind`, never moves the state machine; a relay connection is still considered reachable |
| `PathChangedTransitioning` | **diagnostic only** (BORU-CP-14): addresses known but none active (path in flux) — updates `path_kind`, never moves the state machine |

## Path type is diagnostic (BORU-CP-14 / PDF Task 5.2)

Path events record **how** a peer is currently reachable (direct /
relay / transitioning) without coupling application logic to one path:

- Path type is diagnostic/optimization information, **not proof of
  application-level success**.
- A **relay connection is still considered reachable** — `PathChangedRelay`
  never degrades a `Reachable` / `DirectTopicReady` peer and never resets
  `DirectTopicReady`.
- **Path transitions do not reset or duplicate conversation state** — they
  never move the state machine, never append trail records, and never
  create conversations.
- If the networking layer provides no reliable classification the path
  stays `unknown` — Boru reports Unknown rather than guessing.

## Transition table

`transition(state, event) -> Option<next>` in
`src/control_plane/connectivity.rs` **is** the documented table. `None`
means the event does not move the peer (idempotent no-op).

| From | Event → To |
|------|-----------|
| `Unknown` | `DiscoverySeen`→`Discovered`, `EndpointConnecting`→`Connecting`, `EndpointConnected`→`Reachable`, `DirectMessageReceived`→`DirectTopicReady`, `TopicJoined`→`DirectTopicReady` |
| `Discovered` | `EndpointConnecting`→`Connecting`, `EndpointConnected`→`Reachable`, `EndpointFailed`→`Degraded`, `TopicJoined`→`DirectTopicReady`, `TopicJoinFailed`→`Degraded`, `DirectMessageReceived`→`DirectTopicReady`, `Timeout`→`OfflineStale` |
| `Connecting` | `EndpointConnected`→`Reachable`, `EndpointFailed`→`Degraded`, `TopicJoined`→`DirectTopicReady`, `TopicJoinFailed`→`Degraded`, `DirectMessageReceived`→`DirectTopicReady`, `Timeout`→`OfflineStale` |
| `Reachable` | `EndpointFailed`→`Degraded`, `TopicJoined`→`DirectTopicReady`, `TopicJoinFailed`→`Degraded`, `DirectMessageReceived`→`DirectTopicReady`, `Timeout`→`OfflineStale` |
| `DirectTopicReady` | `EndpointFailed`→`Degraded`, `TopicJoinFailed`→`Degraded`, `Timeout`→`OfflineStale` |
| `Degraded` | `EndpointConnecting`→`Connecting`, `EndpointConnected`→`Reachable`, `TopicJoined`→`DirectTopicReady`, `DirectMessageReceived`→`DirectTopicReady`, `Timeout`→`OfflineStale` |
| `OfflineStale` | `DiscoverySeen`→`Discovered`, `EndpointConnecting`→`Connecting`, `EndpointConnected`→`Reachable`, `TopicJoined`→`DirectTopicReady`, `DirectMessageReceived`→`DirectTopicReady` |

Every pair not listed is a `None` no-op (e.g. a second `DiscoverySeen` while
`Discovered` / `Reachable` / `DirectTopicReady` refreshes `last_seen` but
does not change state; a duplicate `EndpointConnecting` while `Connecting`
does not re-enter `Connecting`). Since BORU-CP-14 every `PathChanged*`
event is a `None` no-op from every state — they update the per-peer
`path_kind` hint and log path changes, but never move the state machine.

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

Read API for diagnostics and the UI indicator (BORU-CP-06, implemented):

- `connectivity_state(peer)` — current state (`Unknown` if untracked)
- `connectivity_trail(peer)` — deterministic transition trail (oldest first)
- `connectivity_peers()` — full snapshot (state, path, direct-topic state,
  last errors, trail)
- `connectivity_store()` — `Arc<Mutex<PeerConnectivityStore>>` read handle
  for the GUI. The iced chat (BORU-CP-06) holds this and projects each
  peer's state onto the four presence labels (Online / Recently seen /
  Connecting / Offline) via `peer_presence_from_connectivity` in
  `examples/iced_chat/app.rs`. The indicator is optional (Settings →
  PRESENCE); disabling it only hides the badge — the store keeps
  updating and discovery/reconnection are unaffected.

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
- `direct_message_receive_advances_to_ready`

Wiring (`src/discovery_service.rs`):

- `connectivity_legacy_discovery_marks_peer_discovered_not_ready`
- `connectivity_control_hello_marks_peer_discovered_idempotently`
- `connectivity_failed_direct_topic_is_visible_not_online`
- `connectivity_expiry_sweep_marks_peer_offline_stale`
- `connectivity_drain_loop_neighbor_events_feed_state_machine`
- `connectivity_direct_message_receive_reports_topic_ready`

Integration (`tests/test_discovery_two_node.rs`):

- `two_nodes_state_machine_discovered_but_not_direct_topic_ready` — over a
  real loopback mesh, A and B see each other as `Discovered` (NOT
  `DirectTopicReady`), the transition trail is deterministic, and a reported
  direct-topic failure is visible as `Degraded` — with zero chat payloads on
  the wire.
