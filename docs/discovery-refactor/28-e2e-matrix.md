# BORU-DISC-28: Required end-to-end test matrix (PDF Phase 7)

PDF Phase 7 — the *"Required end-to-end test matrix"* section of
`Boru_Hidden_Discovery_Implementation_Tasks.pdf`. This is the integration
proof that the discovery refactor holds under every connectivity scenario the
PDF lists. Each scenario is covered by a dedicated test in
`tests/test_discovery_e2e_matrix.rs` (registered as the
`test_discovery_e2e_matrix` test target in `Cargo.toml`,
`required-features = ["net"]`), plus the per-scenario suites from
BORU-DISC-21..26 that already covered parts of the matrix.

## 1. Scenario-to-test map

| # | PDF required scenario | Covering test(s) | Result |
|---|---|---|---|
| 1 | A starts first, then B starts | `scenario_1_a_starts_first_then_b` (new); also `test_discovery_two_node::two_nodes_discover_each_other_without_lobby_chat` | PASS |
| 2 | B starts first, then A starts | `scenario_2_b_starts_first_then_a` (new) | PASS |
| 3 | Both start while the other is offline, then one reconnects | `scenario_3_both_offline_then_one_reconnects` (new); also `test_discovery_restart::both_start_offline_then_one_reconnects` | PASS |
| 4 | LAN direct path available | `scenario_4_lan_direct_path_available` (new); also `tests/test_local_address_lookup.rs` (mDNS), `tests/test_two_peers_exchange.rs` (deterministic direct) | PASS |
| 5 | Relay path required / LAN direct path unavailable | `scenario_5_relay_path_required` (new — local relay server, relay-only addressing) | PASS |
| 6 | Direct conversation open: neither side / one side only / both sides | `scenario_6a_direct_open_neither_side`, `scenario_6b_direct_open_one_side`, `scenario_6c_direct_open_both_sides` (new); also `test_discovery_dm_isolation` (both sides), `test_discovery_two_node` (neither) | PASS (3/3) |
| 7 | Multiple simultaneous conversations plus discovery traffic | `scenario_7_multiple_conversations_plus_discovery` (new); also `test_discovery_group_isolation` (one group + discovery) | PASS |

## 2. What each scenario asserts

Every scenario asserts the three refactor invariants:

1. **Discovery works** — the peers rendezvous on the internal discovery
   topic (`discovery_topic(network)`, `TopicKind::Discovery`) and each
   node's `DiscoveryService` peer registry learns the other node, with live
   `Presence` heartbeats flowing in both directions (`PeerSource::Presence`).
2. **No lobby chat appears** — no node ever has a conversation entry for
   the discovery topic; the topic classifies as Discovery (never
   Conversation) and differs from the public lobby topic
   (`public_room_topic(network, "public-lobby", 1)`).
3. **Conversation traffic stays on its topic** — wire spies on the
   discovery topic decode every captured payload as a `DiscoveryMessage`
   and NONE verifies as a chat `SignedMessage` (the hard rule); conversation
   spies (direct / group) decode every payload as a chat `SignedMessage` and
   never see a `DiscoveryMessage`.

Scenario-specific behavior:

- **1 / 2 (start order)** — the node that starts first joins the discovery
  topic with no bootstrap peers; the second node bootstraps to the first.
  Both directions of the rendezvous work (the exchange is
  direction-symmetric).
- **3 (offline then reconnect)** — separate address books mean neither node
  has address knowledge of the other while "offline" (`peer_count() == 0`
  on both after the offline window). One node then gains the other's address
  (the persisted known-peer path) and dials it into the discovery mesh; both
  rediscover each other and exchange live presence.
- **4 (LAN direct)** — `RelayMode::Disabled` + shared in-memory address book
  (the deterministic direct-path pattern). The test additionally asserts the
  address book entry contains a direct IP address and no relay address.
- **5 (relay required)** — a local relay server
  (`iroh::test_utils::run_relay_server`) with `RelayMode::Custom` and
  `CaTlsConfig::insecure_skip_verify`; the shared address book publishes
  ONLY relay addresses (`is_relay()`), so the LAN direct path is structurally
  unavailable. Discovery still succeeds via the relay.
- **6 (direct open states)** —
  - neither: pure discovery infrastructure, zero conversations anywhere;
  - one side only: A opens the direct topic (subscription + conversation
    entry) while B never opens it; discovery still works, A's DM never
    crosses the discovery topic, and B has no conversation entry for the
    unopened direct topic;
  - both sides: DMs flow in both directions using ONLY the direct topic
    while discovery presence continues concurrently.
- **7 (multiple conversations)** — a direct topic AND a group topic are both
  open simultaneously alongside discovery. DM payloads stay on the direct
  topic, group payloads stay on the group topic, discovery carries only
  discovery payloads, and the group membership stays exactly {A, B} (the
  discovery traffic does not grant membership).

## 3. Execution evidence (debsrv, 2026-08-12)

All builds/tests ran on debsrv (172.16.0.59, 8 cores) through the `rb`
wrapper. Root disk before work: **17G free (97% used)** — above the 5G
threshold, so no cleanup was required; nothing was freed.

### 3.1 Compile gate

```
$ rb check --test test_discovery_e2e_matrix --features net
    Checking iroh-mdns-address-lookup v0.4.0
    Checking iroh-blobs v0.103.0
    Compiling boru-core v0.200.1 (/home/dan/boru-build/work-2)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.25s
```

### 3.2 Matrix run (single invocation)

```
$ rb test --test test_discovery_e2e_matrix --features net
running 9 tests
test scenario_6a_direct_open_neither_side ... ok
test scenario_4_lan_direct_path_available ... ok
test scenario_6c_direct_open_both_sides ... ok
test scenario_6b_direct_open_one_side ... ok
test scenario_5_relay_path_required ... ok
test scenario_7_multiple_conversations_plus_discovery ... ok
test scenario_3_both_offline_then_one_reconnects ... ok
test scenario_1_a_starts_first_then_b ... ok
test scenario_2_b_starts_first_then_a ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.22s
```

### 3.3 Per-scenario pass/fail

| Test | Scenario | Pass/Fail | Time (suite total) |
|---|---|---|---|
| `scenario_1_a_starts_first_then_b` | 1 | PASS | 1.22s (all 9) |
| `scenario_2_b_starts_first_then_a` | 2 | PASS | same |
| `scenario_3_both_offline_then_one_reconnects` | 3 | PASS | same |
| `scenario_4_lan_direct_path_available` | 4 | PASS | same |
| `scenario_5_relay_path_required` | 5 | PASS | same |
| `scenario_6a_direct_open_neither_side` | 6a | PASS | same |
| `scenario_6b_direct_open_one_side` | 6b | PASS | same |
| `scenario_6c_direct_open_both_sides` | 6c | PASS | same |
| `scenario_7_multiple_conversations_plus_discovery` | 7 | PASS | same |

### 3.4 Existing per-scenario suites (BORU-DISC-21..26) still green

```
$ rb test --test test_discovery_startup --features net
running 4 tests ... ok (0.12s)
$ rb test --test test_discovery_two_node --features net
running 2 tests ... ok (0.39s)
$ rb test --test test_discovery_dm_isolation --features net
running 2 tests ... ok (0.63s)
$ rb test --test test_discovery_group_isolation --features net
running 2 tests ... ok (0.64s)
$ rb test --test test_discovery_ui_isolation --features net
running 3 tests ... ok (1.19s)
$ rb test --test test_discovery_restart --features net
running 2 tests ... ok (1.21s)
```

Total: **15 pre-existing discovery tests + 9 matrix tests = 24 tests, all
PASS.**

## 4. Simulation notes (headless/offline constraints)

- **Scenario 4 (LAN direct)** is simulated in-process: both endpoints bind
  loopback (`127.0.0.1:0`), share one `MemoryLookup` address book, and run
  with `RelayMode::Disabled`. This is the same deterministic direct-path
  pattern used by `tests/test_two_peers_exchange.rs` and every BORU-DISC
  discovery test. A real multi-host LAN run (two machines + mDNS) is not
  possible headless; `tests/test_local_address_lookup.rs` covers the mDNS
  plumbing itself. If a real-LAN run is required, launch two `boru`
  instances on the same LAN with default settings and verify the Discover
  sidebar populates — that is the app-level equivalent of this scenario.
- **Scenario 5 (relay required)** is simulated with a **local relay server**
  (`iroh::test_utils::run_relay_server`) instead of the production n0 relay.
  Both endpoints use `RelayMode::Custom(relay_map)` and the shared address
  book publishes only relay addresses, so direct dialing is impossible by
  construction. This exercises the same code path as a real relay (QUIC to
  relay, relay-forwarded packets) without depending on external network.
  A real-WAN relay run against `relay.iroh.link` would additionally verify
  public relay reachability and NAT traversal — out of scope for a headless
  CI gate (and subject to the documented debsrv relay-hang pitfall).
- Scenario 6/7 conversation opening is modeled at the wire level
  (`GossipTopic::subscribe` on the direct/group topic + optional
  `ConversationStore::upsert`), mirroring the app's
  `OpenFriendChat → BackgroundSubscribe` / group-join paths.

## 5. Guardrail compliance

- **Deterministic topic derivation unchanged** — no source code changed in
  this task; the tests use `discovery_topic(network)`, `direct_topic(a,b)`
  and random group topics exactly as the product derives them.
- **Discovery state not merged with conversation state** — asserted per
  scenario (no conversation entry for the discovery topic; topic-kind
  classification).
- **No hidden "chat" object** — the tests exercise the real
  `DiscoveryService`; conversation topics are opened explicitly.
- **Hard rule: private DMs and chat payloads never route through discovery**
  — asserted on the wire in every scenario (discovery spies see only
  `DiscoveryMessage`; conversation spies see only `SignedMessage`).

## 6. Files

| File | Change |
|---|---|
| `tests/test_discovery_e2e_matrix.rs` | NEW — 9 matrix tests covering scenarios 1–7 |
| `Cargo.toml` | NEW `[[test]] test_discovery_e2e_matrix` entry, `required-features = ["net"]` |
| `docs/discovery-refactor/28-e2e-matrix.md` | THIS DOCUMENT |

No runtime code was modified (test-only task).
