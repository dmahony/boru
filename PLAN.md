# Boru DHT Discovery Improvements — Implementation Plan

Source: `Boru_DHT_Discovery_Implementation_Plan.pdf` (attached to kanban task `t_f62da9b8`)
Extracted: 2026-08-19 by orchestrator task `t_92a63bfa`

## Objective

Improve Boru's Mainline DHT usage so it becomes a reliable decentralised bootstrap and room-rendezvous
layer, while keeping live presence, capabilities, room directory state, and application traffic on Iroh
gossip.

**Core rule:** Do not turn the DHT into a distributed application database. Use it to find peers and
resolve peer addresses, then hand off to gossip as quickly as possible.

**Architecture guardrails (from PDF §1):**
- Preserve the two existing DHT roles: address resolution (`EndpointId` -> transport addresses) and
  member/rendezvous discovery (topic/namespace -> `EndpointId`s).
- Keep `DhtAddressLookup` and the distributed-topic-tracker as separate DHT instances.
- Keep relay-only DHT address publication as the privacy-preserving default; never enable direct-address
  publication automatically.
- Global bootstrap record contains only the minimum: `EndpointId` + record integrity/version fields.
  No usernames, profile data, friendships, room memberships, presence, capabilities, or IPs.
- Do not route chat messages or control-plane payloads through the DHT.
- Do not auto-create conversations or friendships from DHT discovery.
- After bootstrap join, use the existing internal discovery gossip topic for ongoing presence/adverts.
- All new loops must have cancellation, bounded memory, bounded concurrency, structured tracing.

---

## 1. Requirements Inventory (from PDF Tasks 1-8)

| # | Task | Priority | Summary | Key files |
|---|------|----------|---------|-----------|
| 1 | Global DHT bootstrap for discovery mesh | P0 | Fresh internet-only node obtains bootstrap peers via DHT (no mDNS/friend/ticket). New `DiscoveryBootstrapTracker`; deterministic Mainnet namespace `BLAKE3("boru-chat/discovery-bootstrap/v1" \|\| network-byte)`; reuse shared `distributed_topic_tracker::Dht` handle; startup lookup after DiscoveryService running; randomise, select 3-8 (max 16) valid `EndpointId`s, feed existing join path; low-cost refresh on ~5-min lease; skip entirely under `--no-dht`; degrade gracefully; cancel on shutdown. | `src/bin/boru/main.rs`, `src/discovery_service.rs`, `src/discovery/connectivity.rs`, NEW `src/discovery_bootstrap.rs`, `src/discovery_backend.rs`, `docs/discovery-architecture.md` |
| 2 | Bounded pending join queue | P0 | Replace `try_acquire_owned() -> continue` with bounded `VecDeque` (capacity 64, configurable, min 1) in `DynamicPeerJoiner`; queue eligible peers when join slots (default 5) are full; drain on worker completion; dedupe (known/pending/queued/joining/known states); overflow policy = reject newest + drop counter; NeighborUp removes from queue, NeighborDown allows requeue; deterministic cancel. | `src/dynamic_joiner.rs` |
| 3 | Rolling candidate admission | P0 | Deprecate `max_candidates_per_session` as hard lifetime total; keep `connection_attempts_per_window` (10/60s) as short-term abuse bound; candidate cooldown/stale TTL (default 10 min); bounded remembered set (128-256, evict oldest); count a peer when handed to joiner, not on DHT return; per-cycle cap 20. | `src/public_room_continuous.rs`, `src/private_room_tracker.rs`, candidate tests |
| 4 | Real periodic DHT jitter | P1 | Fix discarded `apply_jitter(...)` duration over fixed `tokio::time::interval`; cancellation-aware sleep scheduling (one completed op schedules exactly one next wait); jitter both publish and discover; sanitised `jitter_factor`; no flaky wall-clock tests — inject/abstract jitter source. | `src/public_room_continuous.rs`, `src/private_room_tracker.rs` |
| 5 | Adaptive DHT discovery cadence | P1 | `DiscoveryCadencePolicy` unit-testable, UI-independent; mesh-health signals: known/connected neighbour count, recent successful join, recent DHT success/failure; startup/isolated fast (2s/5s/10s/20s/30s), healthy 2-5 min, zero-neighbour immediate, DHT failure bounded backoff+jitter; min delay to avoid tight loops; immediate lookup on explicit join/create. | cadence policy module, `src/public_room_continuous.rs`, `src/private_room_tracker.rs`, `src/discovery_bootstrap.rs` |
| 6 | Randomise candidate selection | P1 | Validate deterministically first, then shuffle/reservoir-sample valid unique `EndpointId`s; app RNG (crypto secrecy not required for ordering); bootstrap prefers diverse small sample; never shuffle unvalidated records; preserve self-filter/dedup; never exceed per-cycle/queue limits. | `src/public_room_tracker.rs`, `src/private_room_tracker.rs`, `src/discovery_bootstrap.rs`, `src/discovery_validation.rs` |
| 7 | Bound retries + DHT degraded state | P2 | `max_attempts_per_cycle` (default 4); exponential backoff with jitter capped at max delay; return error to outer cadence loop after final attempt; degraded-state delay; expose `Healthy / Degraded / Disabled` + `last_success_at`, `consecutive_failed_cycles`, `last_error_category`; success resets counters; never tear down gossip on DHT degradation. | `src/public_room_continuous.rs` (`retry_with_backoff`), private/bootstrap users, diagnostics state |
| 8 | DHT effectiveness metrics + diagnostics | P2 | Reuse existing diagnostics/counter infra (no second framework). Counters: lookup cycles/failures, records received/valid/rejected-by-reason, unique candidates, queued/dropped, join attempts/success, time-to-first-candidate/neighbour, degraded transitions. One concise snapshot: lookup -> valid -> new -> queued -> join attempts -> successful neighbours, suitable for MCP/doctor tooling. Prefer `EndpointId::fmt_short()`; never log full secret keys/private-room secrets. | diagnostics module, counters, bootstrap/public/private trackers |

---

## 2. Workstream Assignment

Two isolated workstreams, each with its own workspace and GitHub feature branch from `main`.
Both branches are pushed at the end of each workstream; final merge into `main` is performed by the
merge task (`t_5ef0daa7`) after both are pushed.

### workspace-a — standard (non-compute-intensive) implementation
- Workspace name: **workspace-a**
- Branch: **feat/workspace-a** (created from `main`)
- Executor profile: **google-coder** (card `t_0f9fccb7`)
- Scope: implement PDF **Tasks 1-8** (all core code changes + unit tests), in the PDF execution order
  (see §3), following the suggested 8-commit breakdown (see §4). Update `docs/discovery-architecture.md`
  to match actual behaviour.
- Out of scope: heavy integration-matrix/soak/stress verification (that is workspace-b), final merge.

### workspace-b — compute-intensive verification (debsrv)
- Workspace name: **workspace-b**
- Branch: **feat/workspace-b** (created from `main`)
- Executor profile: **debsrv** (card `t_2aeacd83`); all heavy computation executes on the **debsrv host**
- Scope: cross-cutting integration test matrix (PDF §9), compute/stress/soak tests, and final full-suite
  verification:
  - Large/hostile DHT result sets (invalid/duplicate/oversized/stale records; validation caps hold;
    memory + CPU bounded)
  - Join saturation (more candidates than join slots; bounded queue, nothing lost)
  - Long-running session soak (many peers over time; recovery after cooldown; no lifetime dead-end)
  - Shutdown-during-retry / cancellation stress (loops exit promptly, tasks drained)
  - Full test-matrix runs and `cargo fmt` / `cargo clippy` for the relevant feature set
- Integration note: matrix tests exercise the real implementation. On debsrv, temporarily merge
  `feat/workspace-a` into a local verification tree when running the matrix (do not force-push or
  rewrite published branches); the published `feat/workspace-b` branch carries only workspace-b's test
  harnesses and verification artifacts.

---

## 3. Execution Order (from PDF §10)

1. Baseline: run existing discovery/DHT/unit/integration tests; record pre-existing failures (do not
   attribute them to this work).
2. Task 2 (pending queue) + tests — local, testable reliability fix.
3. Task 3 (rolling candidate budget) + tests.
4. Task 4 (real jitter) + tests.
5. Task 1 (global DHT bootstrap) using shared member-discovery DHT handle + existing validation/join paths.
6. Task 6 (randomisation) — fair sampling for bootstrap and room trackers.
7. Task 5 (adaptive cadence) after fixed scheduling/jitter behaviour is corrected.
8. Task 7 (finite retry cycles / degraded state).
9. Task 8 (metrics), including queue/bootstrapping counters from earlier tasks.
10. Run the complete test matrix (workspace-b) and update `docs/discovery-architecture.md`.
11. `cargo fmt`, `cargo clippy` for relevant feature set, `cargo test`, discovery simulator/integration
    tests; fix warnings introduced by the patch.
12. Declare complete only after inspecting the final diff for accidental changes to chat routing,
    friendship state, room membership semantics, direct-address privacy defaults, and `--no-dht` behaviour.

---

## 4. Suggested Commit Breakdown (per PDF)

1. `dynamic joiner pending queue + tests` (Task 2)
2. `rolling candidate admission + tests` (Task 3)
3. `scheduling jitter fix + tests` (Task 4)
4. `discovery-bootstrap tracker + startup integration + tests` (Task 1)
5. `candidate randomisation + tests` (Task 6)
6. `adaptive discovery cadence + tests` (Task 5)
7. `bounded retry / degraded state + diagnostics` (Task 7)
8. `effectiveness metrics + documentation + integration test cleanup` (Task 8)

Agent constraint: prefer small, reviewable changes. Reuse current Boru abstractions where correct, but
update tests to express the new intended contract when a behaviour is explicitly replaced by this plan.

---

## 5. Cross-Cutting Integration Test Matrix (PDF §9)

| Scenario | Expected behaviour | Pass condition |
|----------|--------------------|----------------|
| Same LAN, DHT healthy | mDNS remains fastest join path; DHT must not create duplicates | Peers connect; dedup prevents repeated join workers |
| Separate networks, DHT healthy | Global bootstrap yields a small peer set, then gossip discovery continues | Fresh nodes obtain discovery neighbours without manual ticket |
| DHT unavailable / `--no-dht` | No DHT address lookup, room tracker, or global bootstrap; mDNS, relay transport, known peers, tickets, existing gossip continue | Startup succeeds; DHT state degraded; no DHT socket/client created |
| Private room | Secret-derived namespace/encryption intact | Only holders of discovery secret produce usable candidates |
| Public room | Deterministic room rendezvous intact | Valid room peers found and joined |
| Join saturation | More candidates than join slots | Excess candidates bounded in queue, not lost |
| Long-running session | Many peers appear over time | New peers attempted after cooldown; no lifetime dead-end |
| Large/hostile result set | Invalid/duplicate/oversized/stale records present | Validation caps hold; memory and CPU bounded |
| Shutdown during retry | DHT call loop or join retry active | Cancellation exits promptly; tasks drained |

---

## 6. Final Acceptance Criteria (PDF §11)

- Fresh Boru node, internet but no LAN peers/friends/tickets, bootstraps into the internal discovery
  mesh via DHT when DHT is enabled.
- DHT bootstrap only supplies candidate `EndpointId`s; all continuing presence/discovery stays on gossip.
- No DHT candidate is lost solely because join concurrency is momentarily full (except explicit
  bounded-queue overflow).
- Candidate admission recovers over long sessions; still rate-limited and memory-bounded.
- Periodic publication/discovery delays are genuinely jittered.
- DHT query rate drops when mesh healthy, increases when isolated.
- Candidate order not persistently biased to earliest DHT results.
- DHT retries finite per cycle; outage yields observable degraded state, not endless inner retry.
- Diagnostics show lookup -> valid records -> unique candidates -> queued -> join attempts ->
  successful neighbours.
- Relay-only address publication remains default; direct public IP publication requires explicit opt-in.
- `--no-dht` continues to disable DHT cleanly.
- All existing public/private room security validation remains intact.
- No DHT change creates friendships, conversations, room memberships, unread counts, or chat payload
  routing.

---

## 7. Final Merge (owned by `t_5ef0daa7`)

After `feat/workspace-a` and `feat/workspace-b` are pushed: review both branches for conflicts, run
integration tests against the combined tree, resolve conflicts, merge all work into `main`, and push
`main` to GitHub. Verify `main` contains all code from every workspace.