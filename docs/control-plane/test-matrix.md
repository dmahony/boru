# BORU-CP-17: Required test matrix before merge (PDF Phase 7)

The *"Required test matrix before merge"* section of
`Boru_Hidden_Discovery_Control_Plane_Implementation_Steps.pdf` lists 12
scenarios that must hold before the hidden-discovery control plane ships.
This document maps every scenario to the tests that prove it, records the
pass/fail evidence, and notes the new integration tests added by BORU-CP-17
for the acceptance criteria that earlier suites only proved at unit level.

Test targets referenced below (all `required-features = ["net"]`):

| Target | File | Purpose |
|---|---|---|
| `test_required_matrix` | `tests/test_required_matrix.rs` | **NEW (BORU-CP-17)** — integration tests for the gaps |
| `test_reconnect_asymmetric` | `tests/test_reconnect_asymmetric.rs` | Restart + bidirectional direct chat (BORU-CP-09) |
| `test_discovery_e2e_matrix` | `tests/test_discovery_e2e_matrix.rs` | E2E matrix scenarios 1–7 (BORU-DISC-28) |
| `test_discovery_restart` | `tests/test_discovery_restart.rs` | Restart rejoin + reconnect (BORU-DISC-26) |
| `test_discovery_two_node` | `tests/test_discovery_two_node.rs` | Fresh A+B discovery, no lobby (BORU-DISC-22) |
| `test_discovery_ui_isolation` | `tests/test_discovery_ui_isolation.rs` | Discovery payloads never render (BORU-DISC-25) |
| `test_health_view` | `tests/test_health_view.rs` | Developer diagnostics (BORU-CP-15) |
| (unit) | `src/discovery_service.rs` | Registry dedup, malformed handling, expiry, reconnect, capability gate |
| (unit) | `src/control_plane/*.rs` | Message/capabilities/privacy/reconcile/connectivity |

## Scenario-to-test map

| # | PDF scenario | Expected result | Covering test(s) | Result |
|---|---|---|---|---|
| 1 | Fresh A + Fresh B | Both discover; both become direct-topic ready; A→B and B→A succeed | `test_reconnect_asymmetric::reconnect_asymmetric_messages_flow_both_directions` (phase 0); `test_discovery_e2e_matrix::scenario_1_a_starts_first_then_b` / `scenario_2_b_starts_first_then_a`; `test_discovery_two_node::two_nodes_discover_each_other_without_lobby_chat` | PASS |
| 2 | B starts later | A discovers B and reconnects automatically; no UI lobby appears | `test_discovery_e2e_matrix::scenario_1_a_starts_first_then_b` (A first, B later); `test_reconnect_asymmetric` phase 0 friend-watcher (fresh announcement → auto reconnect) | PASS |
| 3 | B restarts | Presence refresh triggers one reconnect; direct chat works both directions again | `test_required_matrix::restart_triggers_exactly_one_reconnect_and_chat_resumes` (phase 1 — online again, exactly one connectivity entry, no duplicate reconnect, both directions); `test_reconnect_asymmetric` phase 1; `test_discovery_restart::restarted_peer_rejoins_discovery_and_reconnects` | PASS |
| 4 | A restarts | Same as above with roles reversed | `test_required_matrix::restart_triggers_exactly_one_reconnect_and_chat_resumes` (phase 2); `test_reconnect_asymmetric` phase 2 | PASS |
| 5 | Relay-only path | Reachability works; diagnostics show relay if known; chat remains bidirectional | `test_required_matrix::relay_only_path_chat_bidirectional` (local relay, relay-only addressing, both directions delivered); `test_discovery_e2e_matrix::scenario_5_relay_path_required`; unit `classify_relay_when_only_active_relay_path` + `service_path_relay_keeps_peer_reachable` | PASS |
| 6 | Direct/LAN path | Reachability works; diagnostics show direct if known | `test_discovery_e2e_matrix::scenario_4_lan_direct_path_available`; unit `classify_direct_when_any_active_ip_path`; `test_health_view::two_nodes_produce_comparable_symmetric_dumps` | PASS |
| 7 | Old client / new client | Unknown capabilities ignored; supported baseline chat still works | `test_required_matrix::mixed_version_old_client_still_chats` (legacy-protocol old client + new client; gate fails closed; chat works both directions); unit `capabilities::test_unknown_capabilities_are_ignored_not_fatal`; `message::unknown_message_type_is_ignored_safely`; `discovery_service::handle_incoming_unknown_version_*` | PASS |
| 8 | Malformed discovery packet | Dropped safely; no chat/UI effects; bounded logging | unit `handle_incoming_undecodable_ignored`, `handle_incoming_truncated_payload_ignored_without_panic`, `handle_incoming_unknown_discriminant_ignored_without_panic`, `handle_incoming_empty_payload_ignored_without_panic`, `handle_incoming_control_malformed_dropped`, `counters_malformed_increments_malformed_only`; `test_discovery_ui_isolation` (valid+malformed never render); `test_hostile_input` (41 tests) | PASS |
| 9 | Duplicate presence flood | Dedup/rate limits prevent duplicate connections and state explosion | `test_required_matrix::duplicate_presence_flood_bounded` (200 duplicate control-PRESENCE envelopes over a real mesh → 1 registry entry, 1 control presence entry, 1 connectivity entry); unit `handle_incoming_duplicate_event_id_ignored`, `handle_incoming_control_duplicate_sequence_ignored`, `handle_incoming_control_rate_limited_sender_rejected`, `connectivity_loop_deduplicates_peer_dials`, `privacy::rate_limiter_window_expires`, `test_restart_storm_prevention` | PASS |
| 10 | Peer goes silent | Presence becomes stale/offline after TTL; no permanent online state | unit `expiry_sweep_removes_stale_peers_from_active_presence`, `expiry_sweep_keeps_refreshed_peers`, `connectivity_expiry_sweep_marks_peer_offline_stale`; `privacy::presence_store_expires_stale_peers_after_ttl`, `guard_expires_stale_presence`; `connectivity::expire_stale_moves_peers_to_offline` | PASS |
| 11 | Blocked/deleted peer | Discovery does not recreate trust relationship or conversation | `test_required_matrix::blocked_peer_not_resurrected` (blocked friend record; presence announcement → no reconnect queued, no signal, no conversation, not direct-topic ready); unit `reconcile::blocked_friend_yields_nothing_even_with_stale_records`, `archived_designated_conversation_not_resurrected`, `archived_store_entry_not_resurrected`; `test_hostile_input::blocked_sender_messages_silently_dropped` | PASS |
| 12 | Feature unsupported remotely | Feature action is disabled/declined cleanly; chat unaffected | unit `capability_gate_handle_reflects_negotiated_support` (old/unknown → fail closed); app.rs `voice_call_blocked_when_peer_lacks_capability`, `file_send_blocked_when_peer_lacks_capability`; `capabilities_advertisement_never_authorises` | PASS |

## New tests in this task (`tests/test_required_matrix.rs`)

### `restart_triggers_exactly_one_reconnect_and_chat_resumes` (scenarios 3 + 4)

Two real in-process nodes with the app's BORU-CP-07/08 reconnect wiring
(friend watcher → `queue_reconnect` → `PeerReachable` → join direct-topic
sender). After each restart the survivor must:

1. bring the restarted peer back **online automatically** (the reconnect
   happened — either the queued reconnect loop or the mesh self-heal via the
   restarted peer's bootstrap dial);
2. keep **exactly one** connectivity entry for the peer (no duplicate
   connections);
3. emit **no second** `PeerReachable` signal in a quiet window (presence
   refresh dedup — the "exactly one reconnect" acceptance; the unit-level
   dedup is proven by `reconnect_queue_queues_once_and_skips_online` and
   `reconnect::schedule_deduplicates_per_peer`);
4. resume direct chat in **both directions** at the application layer.

Phase 1 restarts B; phase 2 restarts A with roles reversed.

### `relay_only_path_chat_bidirectional` (scenario 5)

Both nodes run against a **local relay server** (`run_relay_server`) with
`RelayMode::Custom` and the shared address book publishing **only relay
addresses** (`is_relay()`), so the LAN direct path is structurally
unavailable. The test proves the discovery exchange succeeds over the relay,
both direct-topic subscriptions join through the relay, and A→B / B→A signed
chat messages are delivered at the wire level. Path classification
(diagnostics show relay) is additionally proven by the unit tests
`classify_relay_when_only_active_relay_path` and
`service_path_relay_keeps_peer_reachable`.

### `mixed_version_old_client_still_chats` (scenario 7)

A is a full (new) client with control-plane capabilities. B is an **old
client**: it speaks only the legacy discovery protocol (raw
[`DiscoveryMessage`] Hello/Presence, no control-plane envelopes, no
capabilities). A discovers B through the legacy path, A's capability gate
fails closed for B (`peer_capabilities` is `None` → `peer_supports` is
`None`), and the supported baseline chat still works in both directions on
the deterministic direct topic. Unknown capabilities are ignored; the
baseline protocol is unaffected.

### `duplicate_presence_flood_bounded` (scenario 9)

A runs a full discovery service. A raw attacker node floods the discovery
topic with one legacy Hello plus **200 duplicate control-plane PRESENCE
envelopes** (same sender + same sequence — the dedup key). The registry
dedup and the per-sender rate limiter (60 frames / 10 s window) keep the
state bounded: exactly one legacy registry entry, one control-plane presence
entry, and one connectivity entry. No duplicate connections, no state
explosion, no panic.

### `blocked_peer_not_resurrected` (scenario 11)

A has B **blocked** (friend record `FriendRelationship::Blocked`) and runs
the main.rs watcher mirror (only message-capable friends queue reconnects).
B joins the discovery topic and announces (fresh presence). A must NOT queue
a reconnect, no `PeerReachable` signal may fire, the conversation store stays
empty (discovery never creates a conversation), and B's connectivity state is
never `ready_for_direct` — discovery does not resurrect a blocked trust
relationship. Deleted/archived equivalents are covered at the unit level by
`reconcile.rs` (`archived_designated_conversation_not_resurrected`,
`archived_store_entry_not_resurrected`).

## Execution evidence (debsrv, 2026-08-13)

All builds/tests ran on debsrv (172.16.0.59, 8 cores) through the `rb`
wrapper. Root disk before work: **250G free (44%)** — no cleanup required.

### New matrix suite

```
$ rb test --test test_required_matrix --features net
test mixed_version_old_client_still_chats ... ok
test duplicate_presence_flood_bounded ... ok
test relay_only_path_chat_bidirectional ... ok
test blocked_peer_not_resurrected ... ok
test restart_triggers_exactly_one_reconnect_and_chat_resumes ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; finished in 5.74s
```

### Mapped existing suites (re-run for this gate)

| Suite | Result |
|---|---|
| `test_reconnect_asymmetric` (BORU-CP-09) | PASS (1/1, 1.67s) |
| `test_discovery_e2e_matrix` (BORU-DISC-28, 9 tests) | PASS (9/9, 1.23s) |
| `test_discovery_restart` (BORU-DISC-26) | PASS (5/5, 21.13s) |
| `test_discovery_two_node` (BORU-DISC-22) | PASS (5/5, 0.98s) |
| `test_discovery_ui_isolation` (BORU-DISC-25) | PASS (5/5, 0.92s) |
| `test_health_view` (BORU-CP-15) | PASS (3/3, 0.47s) |
| unit `discovery_service` (dedup / malformed / expiry / reconnect / gate) | PASS (94/94, 1.38s) |
| unit `control_plane` (message / capabilities / privacy / reconcile / connectivity) | PASS (124/124, 0.07s) |
| `test_hostile_input` (41 tests) | PASS (41/41, 0.29s) |
| full `--lib --features net` (regression gate) | PASS (2511 passed / 0 failed / 2 ignored, 355.64s) |

## Guardrail compliance

- **Control plane / data plane separation** — asserted on the wire in every
  new test: discovery topics carry only `DiscoveryMessage`/control envelopes,
  never chat; the direct topic carries only `SignedMessage`.
- **No authorisation by presence** — proven by
  `blocked_peer_not_resurrected` (blocked friend gets nothing) and
  `mixed_version_old_client_still_chats` (presence without capabilities
  grants no feature).
- **Bounded resources** — proven by `duplicate_presence_flood_bounded`
  (registry/control/connectivity each stay at exactly one entry) and the
  unit-level rate-limiter / cache-cap tests.
- **Backward compatibility** — proven by `mixed_version_old_client_still_chats`
  and the unit `unknown_*` / `unsupported_version_fails_closed` tests.
- **Idempotence** — proven by the flood test (duplicates are no-ops) and the
  restart test (exactly one connectivity entry / no duplicate reconnect).
- **No control-plane/chat coupling** — the new tests never call chat
  rendering/history paths; the `Recorder` app layer only consumes
  direct-topic `SignedMessage`s.

## Files

| File | Change |
|---|---|
| `tests/test_required_matrix.rs` | NEW — 5 integration tests covering scenarios 3/4, 5, 7, 9, 11 |
| `Cargo.toml` | NEW `[[test]] test_required_matrix` entry, `required-features = ["net"]` |
| `docs/control-plane/test-matrix.md` | THIS DOCUMENT |

No runtime code was modified (test-only task; no bugs were revealed by the
matrix that required a fix).
