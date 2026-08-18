# Architecture Refactor Baseline

Tracking document for the BORU-ARCH refactor series (Boru_Code_Improvement_Action_Plan.pdf).
This baseline is captured BEFORE structural changes begin so that later refactor PRs can be
attributed correctly and regressions can be measured against a known starting point.

- Created: 2026-08-18 (BORU-ARCH-001, task t_00b685f6)
- PDF source: `Boru_Code_Improvement_Action_Plan.pdf`, Phase 0 task BORU-ARCH-001

## 1. Point-in-time reference

| Item | Value |
|------|-------|
| main commit SHA | `69b4e639a4677f565c3af0976affbfa62e4c6e65` |
| main commit date | 2026-08-17 22:55:29 +0000 |
| main commit subject | `chore: bump Boru version to 0.215.3` |
| Boru version (Cargo.toml) | `0.215.3` |
| Crate name | `boru-core` |
| Edition | 2021 |
| Default features | `net`, `metrics`, `gui` |
| Binary target | `boru` (`examples/iced_chat/main.rs` via `[[bin]]`) |
| PDF "reviewed baseline" | main branch, Boru v0.215.2 (plan document predates v0.215.3) |

> **Note (post-measurement):** origin/main advanced to `55dcb1f3` (Boru v0.215.4 —
> `chore: bump Boru version to 0.215.4`, `chore: sync Cargo.lock to 0.215.4`,
> `fix(chat): sender's own card gets the video thumbnail in DirectOffer sends`) while this
> baseline was being captured. ALL measurements in this document were taken at
> `69b4e639` (v0.215.3). The version bump and the files.rs thumbnail fix landed after the
> measurements; they are unrelated to the refactor series and do not change any of the
> recorded results (no test/protocol/storage-affecting change in that window).

How to refresh this table for a later task:

```bash
git fetch origin
git log -1 --format='%H %ci %s' origin/main
grep -m1 '^version' Cargo.toml
```

## 2. Formatting, Clippy, tests — baseline results

All heavy checks ran on DEBSRV (172.16.0.59) via the `rb` wrapper with default features
(`net`, `metrics`, `gui`) unless noted. Commands run from a clean worktree at the commit above.

### 2.1 `cargo fmt --check` — KNOWN PRE-EXISTING DRIFT (fails at baseline)

The tree is NOT rustfmt-clean at this baseline. This is pre-existing and NOT caused by any
refactor work (also documented in the iroh-gossip-chat-workflows skill).

| Command | Result |
|---------|--------|
| `cargo fmt --all --check` (plain, rustfmt 1.9.0) | exit 1 — ~1834 diff hunks |
| `cargo fmt --all --check -- --config unstable_features=true --config imports_granularity=Crate,group_imports=StdExternalCrate,reorder_imports=true` (CI's `cargo make format-check`) | exit 1 — 323 files with drift |

Do NOT run a repo-wide `cargo fmt --all` on a feature branch as part of this series; it would
reformat ~140+ files and create massive merge noise. Use targeted `rustfmt <new-files>` and
rustfmt-style `patch` edits for new/modified code.

### 2.2 Clippy

| Command | Result |
|---------|--------|
| `rb clippy --workspace --lib --bins` (default features) | **exit 0 — clean, no errors** (lib + binary only) |
| `rb clippy --workspace --all-targets` (default features, CI's clippy leg) | **exit 101 — FAILS to compile**: 9 `E0061` errors in test targets (`test_discovery_startup`, `test_extensions_metadata`, `test_required_matrix`), same root cause as §3.2 |

The `--all-targets` clippy failure is a compile break in the integration test targets, not a
lint issue. lib + bin lint cleanly (exit 0, warnings only).

### 2.3 Default tests

| Suite | Command | Result |
|-------|---------|--------|
| lib unit tests | `rb test --lib` (default features) | **2705 passed, 0 failed, 2 ignored** (358.4 s) |
| bin unit tests (app.rs module) | `rb test --bin boru` (default features) | **1630 passed, 0 failed, 0 ignored** (244.8 s) |

### 2.4 Two-node / discovery integration tests

**CRITICAL BASELINE FINDING:** the most important two-node/discovery suites FAIL TO COMPILE on
main at this commit. They call `DiscoveryService::join(...)` with the pre-BORU-CP-17 4-argument
signature, but `src/discovery_service.rs:2010` now takes a 5th `SecretKey` argument. The
signature changed in `34499756` (BORU-CP-17 "sign control envelopes so relayed capability
negotiation is trustworthy", 2026-08-16); no commit since updated the test call sites. This is
a PRE-EXISTING break on origin/main, NOT caused by this task or any refactor work.

Suites that fail to compile (exit 101, `error[E0061]: this function takes 5 arguments but 4
arguments were supplied`):

- `test_discovery_two_node` (tests/test_discovery_two_node.rs:192,199)
- `test_discovery_startup` (tests/test_discovery_startup.rs:99)
- `test_discovery_restart` (tests/test_discovery_restart.rs:138)
- `test_discovery_e2e_matrix` (tests/test_discovery_e2e_matrix.rs:180)
- `test_discovery_dm_isolation` (tests/test_discovery_dm_isolation.rs:215,219)
- `test_discovery_group_isolation` (tests/test_discovery_group_isolation.rs:194,198)
- `test_discovery_ui_isolation` (tests/test_discovery_ui_isolation.rs:586,590,853)
- `test_public_room_directory` (tests/test_public_room_directory.rs:175)
- `test_reconnect_asymmetric` (tests/test_reconnect_asymmetric.rs:327)
- `test_required_matrix` (tests/test_required_matrix.rs:280,696,700,818,981,1077,1121)

Two additional test files contain the same 4-arg call and would fail the same way if built
(`test_extensions_metadata.rs:105`, `test_health_view.rs:120,126,296,302`; they are
auto-discovered, not declared `[[test]]` targets).

All of these use `RelayMode::Disabled` (no relay hang risk); the compile break is the only
blocker. The fix is mechanical — pass the local `SecretKey` as the 5th argument — and should be
a separate test-fix task BEFORE discovery/refactor work relies on these suites (see §5).

Other two-node-adjacent suites that DO build and were run with `--features test-utils`:

| Suite | Result |
|-------|--------|
| `room_e2e` | **PASS** |
| `stale_bootstrap` | **FAIL — 1 failed** (`test_stale_bootstrap_does_not_block_rejoin`, pre-existing, see §3) |
| `test_stable_identities` | **FAIL — 30 passed, 1 failed** (`multiple_files_visibility_consistent_across_changes`, pre-existing) |
| `test_peer_lifecycle` | **FAIL — 25 passed, 1 failed** (`peer_goes_offline_adds_happen_peer_restarts`, pre-existing) |
| `test_message_lifecycle` | **FAIL — 30 passed, 9 failed** (outbox save/load persistence assertions, pre-existing) |

## 3. Known failing / flaky tests (pre-existing — do not attribute to refactor work)

The following are documented from the debsrv integration-test gate history (BORU-CARGO-08) and
the iroh-gossip-chat-workflows skill. They are environmental or pre-existing and are recorded
here so refactor PRs do not misattribute them.

### 3.1 Relay-hang suites on debsrv (RelayMode::Default + `endpoint.online().await`)

Root cause: prod relay hostnames resolve IPv6-first, debsrv has no IPv6 route, so
`endpoint.online().await` never resolves. These suites hang at ~0-1% CPU until killed. They are
NOT code failures and NOT caused by refactor work.

Known hang suites (confirmed via `grep -c "RelayMode::Default"` in the test file):

- `repro_two_iced_instances`
- `test_full_chat_list_flow`
- `test_image_iced_gui_flow`
- `test_image_receiver_download`
- `test_image_send_download`
- `test_mcp_diagnostics_integration`
- `test_message_transfer`
- `test_multi_image_burst`
- `test_no_bootstrap`
- `test_performance_regression`
- `test_two_instance_dht_chat`

Flaky: `test_iced_chat_flow` hung on run 1 (killed) but passed on run 2 — relay connectivity
to debsrv is intermittent.

Also hangs: `build_join_request_test_app` (in the lib test module) — same `RelayMode::Default`
+ `online().await` pattern. New tests that do not need a real peer should use
`build_prewarm_test_app()` (`RelayMode::Disabled`, skips `online()`).

When running the full integration gate: wrap each `rb test --test <name>` in `timeout 240`
and run suites one-per-invocation.

### 3.2 Discovery/directory test suites FAIL TO COMPILE (E0061) — most severe pre-existing issue

Root cause: BORU-CP-17 (`34499756`, 2026-08-16) added a 5th `SecretKey` argument to
`DiscoveryService::join` (`src/discovery_service.rs:2010`). Ten declared `[[test]]` targets and
two auto-discovered test files still call it with 4 arguments, so they fail to compile under
default features. Full list with call sites is in §2.4. This breaks the discovery test gate and
the CI `clippy --all-targets` leg at baseline. Mechanical fix: pass the local `SecretKey` as
the 5th argument in each call site.

### 3.3 Other pre-existing test failures (default-features gate, from BORU-CARGO-08)

- `test_onboarding_integration`: 12 assertion failures (inference/persistence). Pre-existing;
  broken since the Jul-28 Phase-22 cleanup. Test + `src/user_profile.rs` were 0-line-diff
  across the default-features migration.
- 5 source-audit test failures in the bin module (`examples/iced_chat/app.rs`): tests that
  `include_str!` app.rs/home.rs and grep for literal source patterns drift when feature work
  changes the source without updating the audit test (e.g. greeting simplification, SPACE_28
  spacing change, block_on additions). Pre-existing.
- `image_optimizer_integration` fixture precondition: `tests/image_optimizer_integration.rs`
  loads fixtures from `/tmp/optimizer_test_images` on the run host; `test_oversized_input_rejected`
  needs a real >10 MiB JPEG at `oversized_massive.jpg` (the stock generator's file is only
  ~3.4 MiB). Environment precondition, not a code failure.

### 3.4 Pre-existing failures confirmed by this baseline run (2026-08-18)

These were re-confirmed at the baseline commit and fail independently of refactor work:

- `stale_bootstrap::test_stale_bootstrap_does_not_block_rejoin` — panics at
  tests/stale_bootstrap.rs:261:60 on `RoomStore::load_or_none(tmp_dir.path()).expect("room store")`
  (the freshly-created store returns `None`/Err).
- `test_stable_identities::multiple_files_visibility_consistent_across_changes` — assertion
  `left == right` failed at tests/test_stable_identities.rs:108: "Bob should see an empty
  catalogue from Alice" (`left: 1, right: 0`). 30 other tests in the suite pass.
- `test_peer_lifecycle::peer_goes_offline_adds_happen_peer_restarts` — assertion failed at
  tests/test_peer_lifecycle.rs:65: "Bob sees 3 files in Alice's catalogue, expected 1"
  (`left: 3, right: 1`). 25 other tests in the suite pass.
- `test_message_lifecycle` — 9 failures, all outbox save/load persistence assertions
  (`saved empty outbox should load`, `should exist`, `should have saved data`, and
  `write history fixture: Os { code: 2, kind: NotFound, message: "No such file or directory" }`).
  The failures are consistent with the deprecated MailboxStore save/load no-op documented in
  `references/security-test-suite-patterns.md` (outbox save is a no-op, so reload assertions
  fail). 30 tests in the suite pass.

### 3.5 fmt drift

`cargo fmt --check` fails at this baseline with pre-existing drift (section 2.1). CI's
`check_fmt` job (`cargo make format-check`) uses the unstable-imports config and reports
drift in 323 files at this commit.

## 4. Oversized modules — current file sizes

The required mega-modules plus other large files in the tree (sizes at the baseline commit):

| File | Bytes | Lines | Notes |
|------|-------|-------|-------|
| `examples/iced_chat/app.rs` | 1,824,155 | 41,831 | Iced application shell (top refactor target; ~1.82 MB) |
| `src/diagnostics.rs` | 432,769 | 11,622 | diagnostics mega-module |
| `src/discovery_service.rs` | 400,148 | 9,030 | discovery coordinator (~400 KB) |
| `src/storage.rs` | 382,910 | 9,473 | storage mega-module |
| `examples/iced_chat/app/chat.rs` | 456,291 | ~11,000 | emerging app module tree — chat |
| `examples/iced_chat/app/files.rs` | 372,863 | ~9,300 | emerging app module tree — files |
| `examples/iced_chat/mcp_server.rs` | 319,607 | ~7,900 | MCP server |
| `src/file_access_handler.rs` | 144,047 | 3,758 | file access handler |
| `src/store.rs` | 122,530 | 3,249 | message store |
| `src/net.rs` | 117,472 | 2,883 | networking |

Required by BORU-ARCH-001 acceptance criteria: app.rs, discovery_service.rs, storage.rs,
diagnostics.rs, file_access_handler.rs, store.rs, net.rs — all present above.

How to refresh:

```bash
for f in examples/iced_chat/app.rs src/discovery_service.rs src/storage.rs src/diagnostics.rs \
         src/file_access_handler.rs src/store.rs src/net.rs; do
  printf "%-45s %8d bytes  %7d lines\n" "$f" "$(wc -c < "$f")" "$(wc -l < "$f")"
done
```

## 5. Run summary (BORU-ARCH-001, 2026-08-18)

All heavy checks ran on DEBSRV (172.16.0.59) via `rb` from a clean worktree at
`69b4e639a4677f565c3af0976affbfa62e4c6e65` (origin/main).

| Check | Command | Result |
|-------|---------|--------|
| fmt (plain) | `cargo fmt --all --check` | FAIL (pre-existing drift, ~1834 hunks) |
| fmt (CI config) | `cargo fmt --all --check -- --config ...` | FAIL (pre-existing drift, 323 files) |
| clippy lib+bin | `rb clippy --workspace --lib --bins` | PASS (exit 0, warnings only) |
| clippy all-targets | `rb clippy --workspace --all-targets` | FAIL (9 E0061 in test targets — compile break, §3.2) |
| lib tests | `rb test --lib` | PASS: 2705 passed, 0 failed, 2 ignored |
| bin tests | `rb test --bin boru` | PASS: 1630 passed, 0 failed, 0 ignored |
| `room_e2e` | `rb test --test room_e2e --features test-utils` | PASS |
| 7 discovery suites | `rb test --test test_discovery_*` | FAIL to compile (E0061, §3.2) |
| `stale_bootstrap` | `rb test --test stale_bootstrap --features test-utils` | 1 failed (§3.4) |
| `test_stable_identities` | `rb test --test test_stable_identities --features test-utils` | 30 passed, 1 failed (§3.4) |
| `test_peer_lifecycle` | `rb test --test test_peer_lifecycle --features test-utils` | 25 passed, 1 failed (§3.4) |
| `test_message_lifecycle` | `rb test --test test_message_lifecycle --features test-utils` | 30 passed, 9 failed (§3.4) |

Key takeaways:

1. lib + bin unit tests and lib/bin clippy are clean (4335 unit tests passing).
2. The discovery/directory/reconnect integration gate is **compile-broken on main** (E0061) —
   the single most important thing recorded by this baseline. Any later refactor PR that
   reports "discovery tests failing" will be hitting this pre-existing break, not its own diff.
3. A recommended follow-up task (not done here — out of scope, no production code changes):
   fix the 4-arg → 5-arg `DiscoveryService::join` call sites in the 10 declared + 2
   auto-discovered test files listed in §2.4, then re-run the discovery gate.

## 6. Out of scope / notes

- No production code changed by this baseline task.
- The canonical tree `/home/dan/iroh-gossip-chat` may contain unrelated uncommitted leftovers
  (Cargo.lock, examples/iced_chat/app/files.rs) — those are not part of this series and must be
  left alone.
- The PDF plan says "Reviewed baseline: main branch, Boru v0.215.2" — the actual baseline is
  v0.215.3 (commit 69b4e639). The plan document predates the last version bump.
