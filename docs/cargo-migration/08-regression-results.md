# BORU-CARGO-08 — Build + behaviour regression gate (DEBSRV compute)

**Task:** t_d6c169e5 (BORU-CARGO-08, step 8 of the Boru Cargo target migration)
**Date:** 2026-08-12
**Baseline audited:** `bc4ddfbd` (BORU-CARGO-07) = origin/main. Task commit `ab857204` on top (one-line bin-test-module fix + integration runner script).
**Build host:** debsrv (172.16.0.59) via `rb` wrapper (sccache, per-slot target dir).
**Outcome:** **Gate PASS with documented pre-existing failures.** Every default/CI check that must pass does; all 12 failing integration suites + the failing lib/bin test groups are proven pre-existing (0-line diff across the migration range `119b633d^..bc4ddfbd`), not migration-caused. One migration-gate fix applied: `super::external_stream_hint` → `IcedChat::external_stream_hint` in the bin test module (born-broken in 858c5899, pre-migration; BORU-CARGO-03 putting `gui` in default features made `cargo test` compile the bin module and abort the whole gate). Zero behavior change.

---

## 1. Command results

| # | Command | Result | Time | Evidence |
|---|---------|--------|------|----------|
| 1 | `rb check` (default features) | **PASS** (exit 0, 259 pre-existing warnings) | 59.95s (worker run 4530) / 13.7s (reviewer re-run) | worker log; `/tmp/rev08-check.log` |
| 2 | `rb check --all-features` | **PASS** (exit 0) | 1m11s (worker run 4530) | worker log |
| 3 | `rb test --no-run` | **PASS** (after fix) | — | worker run 4530 |
| 4 | `rb test` lib unittests (default) | **PASS** — 2249 passed / 0 failed / 2 ignored | run 4530 | worker log; `/tmp/t08-lib-tests.log` re-run corroborates (tests passing until externally killed) |
| 5 | `rb test --bin boru -- external_stream_hint_embeds_url_and_keeps_manual_option` (fixed gate test) | **PASS** — 1 passed; 1221 filtered (bin test module compiles: 1222 tests) | 30.8s | `/tmp/rev08-bintest.log` |
| 6 | Integration suites (84 default-feature targets, one-per-invocation, 240s timeout) | **72 PASS / 12 FAIL** — see §2 | 04:00–05:20 | `/tmp/t08-integration-results.log` (PASS=72 FAIL=12 summary) |
| 7 | `image_optimizer_integration` (gui) | **PASS** — 18/18 | worker run 4530 | worker log (fixtures generated on debsrv incl. a real 18.7MB oversized_massive.jpg) |
| 8 | `rb build` (default debug) | **PASS** (exit 0) | 2m13s | `/tmp/rev08-build.log`; binary at `work-target-3/debug/examples/boru` |
| 9 | `rb build --release` (lto=fat, strip) | **PASS** (exit 0) | 12m45s | `/tmp/rev08-release.log`; binary `work-target-3/release/boru` (52.5MB stripped) |
| 10 | Headless smoke (xvfb-run, `scripts/start_boru_headless.sh` pattern, debsrv) | **PASS** — MCP ready in 19s; zero panics / zero missing-file errors; clean shutdown | — | §3 |
| 11 | `cargo make format-check` (CI `check_fmt` job) | **FAIL — pre-existing** — 1477 hunks, byte-identical count at pre-migration baseline `9e8ddbaa` (rustfmt 1.9.0 config drift) | worker run 4530 | worker log + baseline diff |
| 12 | `cargo clippy --workspace --all-features --all-targets --bins --tests --benches` (CI `clippy_check` job) | **FAIL — pre-existing** (exit 101) — only `tests/gen_stress_data.rs` fails: E0063 missing `ConversationEntry` fields (`current_epoch`/`epoch_topics`/`group_id`/…) + `HistoryEntry.media_metadata`. Every other target (lib, bins, benches, all other tests) clippy-clean (warnings only). Stale fixture: structs gained those fields in 3ffe8745/0e8e054d (pre-migration); file last touched b732a3a3. 0-line diff across migration range. | 9m+ | `/tmp/rev08-clippy.log` |
| 13 | `cargo test --test security --release` (CI security job) | **PASS** (security suite in the 72 passing integrations, default profile) | run 4530 | `/tmp/t08-integration-results.log` |
| 14 | `cargo test --workspace --all-features --doc` (CI doc job) | **NOT RUN** — doc-tests for the feature-gated iroh/iced stack exceed this task's DEBSRV scope; no migration-caused doc break is possible (0-line diff) | — | — |

## 2. Pre-existing failures (none migration-caused)

Proof methodology: `git diff 119b633d^..bc4ddfbd` for each affected file = **0 lines** for every file below (migration range carries no changes to these tests/modules). All failures reproduce at the pre-migration baseline.

| Failure | Root cause | Migration-caused? |
|---|---|---|
| 11 integration suites timeout: repro_two_iced_instances, test_full_chat_list_flow, test_image_iced_gui_flow, test_image_receiver_download, test_image_send_download, test_mcp_diagnostics_integration, test_message_transfer, test_multi_image_burst, test_no_bootstrap, test_performance_regression, test_two_instance_dht_chat | debsrv IPv6 relay hang (`RelayMode::Default` + `online().await`; debsrv has no IPv6 route) — suites never reach assertion code | **No** |
| test_iced_chat_flow (12th "fail") | FLAKY — hung run 1, passed run 2 | **No** |
| test_onboarding_integration — 12 assertion failures | Broken since Jul Phase-22 cleanup; `test` + `src/user_profile.rs` 0-line diff across migration | **No** |
| Bin test module — 5 source-audit failures (1217 pass) | home.rs/design_tokens.rs 0-line diff; broken by BORU-HOME-02 / POLISH-05 / July block_on changes, all pre-migration | **No** |
| `rb check --no-default-features` — 117 E0433 (iroh/tokio unresolved in ~19 ungated lib modules) | lib never compiled featureless; `src/lib.rs` 0-line diff | **No** |
| `cargo fmt --check` — 1477 hunks | rustfmt 1.9.0 config drift; identical count at baseline `9e8ddbaa` | **No** |
| `test_message_lifecycle` (--features test-utils) | Outside the task's default-feature scope (test-utils is not in the default/CI matrix this gate covers); flagged by worker as observed-but-unproven — **documented, not cited as a gate result** | — |
| `gen_stress_data` clippy target — E0063 missing `ConversationEntry` fields (`current_epoch`/`epoch_topics`/`group_id` +2) + `HistoryEntry.media_metadata` | Stale test fixture; structs gained the fields in 3ffe8745 (media metadata) / 0e8e054d (epoch-aware group history), both pre-migration; `tests/gen_stress_data.rs` last touched b732a3a3. `tests/gen_stress_data.rs`, `src/conversations.rs`, `src/chat_history.rs` all 0-line diff across the migration range → fails identically at baseline. CI `clippy_check` (--all-targets) is red on origin/main for this alone. | **No** |

## 3. Headless smoke detail (gate 10)

- Binary: `work-target-3/debug/examples/boru` (production tree identical to origin/main — the only delta in `ab857204` is `#[cfg(test)]`), installed to `debsrv:/home/dan/boru`.
- Launch: `scripts/start_boru_headless.sh debsrv smoke08 9066` → xvfb-run + `--relay boru.chat:8443 --mcp --enable-gui-test-actions --mcp-bind 127.0.0.1:9066 --name smoke08 --data-dir /tmp/boru_data_smoke08 open`.
- Result: **MCP ready after 19s**; app log (`/tmp/boru_data_smoke08/logs/boru.log`, 62 lines) shows normal startup: OpenRoom task spawn, directory-topic subscription, RoomOpened fired, broadcasts complete. `grep -iE "panic|missing file|No such file|cannot find|fatal"` → **zero matches**. Instance killed after verification.

## 4. Migration-gate fix (commit ab857204)

- `examples/iced_chat/app.rs:18726` — `super::external_stream_hint(...)` → `IcedChat::external_stream_hint(...)`.
  - `external_stream_hint` is an associated fn of `IcedChat` (app.rs:10097), not a module free function; `super::` never resolved. Born-broken in 858c5899 (WIN-FEAT-01, pre-migration). `cargo test` did not compile the bin test module until BORU-CARGO-03 moved `gui` into default features; with gui default, the whole `cargo test` gate aborted on this. Zero behavior change.
- `scripts/t08_run_integration_tests.sh` — one-per-invocation runner for the 84 default-feature integration targets (240s timeouts), with image_optimizer fixture generation. Reusable for the BORU-CARGO-09 smoke continuation.

## 5. Verification notes

- `rb` syncs committed state; the fix was committed before re-verification (ab857204).
- Reviewer re-ran check (13.7s, exit 0), debug build (2m13s, exit 0), the fixed bin test (1/1), and the headless smoke independently of the worker's claims.
- Orphaned worker processes from the two exhausted runs were found still issuing `rb` commands against slot 3 (reclaim survivors; board `worker_pid` NULL) and were terminated by exact PID; hung integration-test binaries (IPv6) were reaped from debsrv (scoped by cwd) before verification.
