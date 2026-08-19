# workspace-b — compute-intensive DHT discovery verification

`workspace-b` is the compute/stress/soak verification workstream of the Boru DHT
discovery plan (PLAN.md §2, PDF §9). It is implemented as an **isolated Git
worktree + feature branch** (`feat/workspace-b`) created from `main`, so it
never collides with workspace-a's implementation branch.

The remote **debsrv** host (ssh `debsrv` → 172.16.0.59, 8 cores) is the BUILD /
COMPUTE node. All heavy compilation and test execution runs there.

## Scope (PLAN.md §2 workspace-b)

| Scenario (PDF §9) | Deliverable |
|---|---|
| Large / hostile DHT result sets | `tests/discovery_compute_matrix.rs` — `hostile_flood_caps_hold`, `hostile_categories_produce_rejections`, `oversized_record_rejected` |
| Join saturation (> join slots) | `tests/discovery_compute_matrix.rs` — `join_saturation_bounded_concurrency` |
| Long-running session soak | `tests/discovery_compute_matrix.rs` — `soak_retry_recovery_no_dead_end`, `soak_many_waves_bounded` |
| Shutdown-during-retry / cancellation | `tests/discovery_compute_matrix.rs` — `shutdown_during_retry_prompt`, `cancellation_stress_repeated_cycles` |
| Full-suite verification + clippy/fmt | `scripts/discovery_matrix_run.sh` |
| Matrix runs against merged workspace-a | `scripts/discovery_matrix_run.sh --merge-workspace-a` |

These tests assert the §9 contract through the **current public API** only, so
the workspace-b branch compiles and goes green standalone. The rows that
depend on workspace-a's new code (bounded pending queue "nothing lost",
`DiscoveryBootstrapTracker`, adaptive cadence, degraded-state diagnostics) are
exercised by the runner in a **local verification tree** on debsrv where
`feat/workspace-a` is merged — the published `feat/workspace-b` branch is never
rewritten.

## Why a mock GossipSender?

The `DynamicPeerJoiner` join path (`GossipSender::join_peers`) is exercised
with a controllable command-window mock (capacity + drain). Filling the window
without draining makes `join_peers` fail, driving the **real** retry/backoff /
cold-recovery path — the same code a failing DHT feed would hit. This keeps the
compute tests hermetic (no relay, no flaky network) while still exercising the
production joiner logic.

## Run

On debsrv (via the `rb` wrapper, which rsyncs this worktree and runs cargo
remotely), or directly on a build host with Rust:

```bash
# Compile + run the compute matrix only (fast)
rb test --test discovery_compute_matrix --features test-utils

# Full matrix: all integration suites + compute matrix + clippy
bash scripts/discovery_matrix_run.sh --features net,test-utils --limit 240

# Same, but merge origin/feat/workspace-a into a LOCAL verification tree first
bash scripts/discovery_matrix_run.sh --features net,test-utils --merge-workspace-a
```

Env knobs: `BORU_MATRIX_RECORDS` (hostile flood size, default 50,000),
`BORU_SOAK_ROUNDS` (default 25), `BORU_CANCEL_CYCLES` (default 30),
`BORU_MATRIX_LIMIT` (per-suite timeout, default 240 s).

The runner emits `docs/workspace-b/matrix-report-<stamp>.md`.

## Rules honored

- **Heavy compute on debsrv** — the local 6-core i5 machine never compiles boru
  (`rb` enforces this).
- **Isolation** — workspace-b lives in its own worktree / branch; merging
  workspace-a only ever happens in a throwaway verification worktree on debsrv.
- **No published-branch rewrite** — `--merge-workspace-a` never force-pushes.
- **No repo-wide `cargo fmt`** — targeted `rustfmt`/patch-only formatting to keep
  the diff scoped (the tree is not rustfmt-clean at HEAD).
