# Boru Baseline Build/Test Characterization — BORU-CALL-0.1

**Task:** t_f4c89b47 (BORU-CALL-0.1: Baseline build/test characterization)
**Date:** 2026-08-08
**Commit characterized:** `b709ad7c` (chore: bump Boru version to 0.136.1) — origin/main HEAD at review time
**Workspace:** worktree `wt/t_f4c89b47` fast-forwarded to origin/main before measuring
**Host:** debsrv (172.16.0.59, 8 cores) via `rb` remote build wrapper, sccache-enabled
**Profile:** debug (dev), consistent with the repo's normal dev workflow

## Commands and Results

| # | Command | Result |
|---|---------|--------|
| 1 | `cargo fmt --all -- --check` | **FAIL** (exit 1) |
| 2 | `cargo check --all-features` | **PASS** (exit 0) |
| 3 | `cargo build --example boru --features gui` | **PASS** (exit 0) |
| 4 | `cargo test` | **FAIL** (exit 101) — compile error in test target, no tests ran |

> Note: The task body suggested `--example iced_chat`; the actual example name in
> this repo is `boru` (path `examples/iced_chat/main.rs`), confirmed by
> `Cargo.toml [[example]]` and CI (`codeql.yml`: `cargo build --features gui --example boru`).
> The canonical GUI build command per README is `cargo run --example boru --features gui`.

## 1. cargo fmt --all -- --check — FAIL

560 formatting diffs across the tree. Heaviest offenders:

```
364 examples/iced_chat/app.rs
 22 examples/iced_chat/download_progress_view.rs
 18 examples/iced_chat/mcp_server.rs
 15 examples/iced_chat/form_components.rs
 13 tests/test_storage_integration.rs
 13 examples/iced_chat/shared_by_me_table.rs
 12 examples/iced_chat/video_file_card.rs
 12 examples/iced_chat/ui_components.rs
 10 examples/iced_chat/component_gallery.rs
  8 examples/iced_chat/main.rs
  7 tests/tunnel_reconnect.rs
  7 src/collection_transfer.rs
  5 src/tunnel/service.rs
  5 src/tunnel/reconnect.rs
  4 src/ticket_share.rs
  4 src/storage.rs
  4 src/local_service_scan.rs
  4 examples/iced_chat/fonts.rs
  3 src/tunnel/enrollment.rs
  3 src/catalogue_model.rs
  (plus ~10 more files with 1-2 diffs each)
```

Most diffs are import reordering / line-wrapping (rustfmt style drift), not
semantic issues. This is a pre-existing condition (no call code exists yet).

## 2. cargo check --all-features — PASS

- `boru-core` (lib) generated **4 warnings** (3 auto-fixable via `cargo fix`).
- Finished in 57.97s (warm debsrv slot).

## 3. cargo build --example boru --features gui — PASS

- `boru-core` (example "boru") generated **217 warnings** (70 auto-fixable).
- Finished in 1m 09s.
- Warnings include "associated items ... are never used" in form components,
  unused imports/variables, and dead-code lints — all pre-existing.

## 4. cargo test — FAIL (compile error, no tests executed)

The test suite **does not compile**. `tests/fs22_dashboard_coverage.rs` fails:

```
error[E0063]: missing fields `online` and `peer_display` in initializer of `PeerDownload`
  (3 occurrences, lines ~504-545)
error: could not compile `boru-core` (test "fs22_dashboard_coverage") due to 3 previous errors; 1 warning emitted
```

**Root cause:** `PeerDownload` gained `peer_display` and `online` fields in
`examples/iced_chat/dashboard_view_model.rs` (FS-08: resolved display identity +
presence-derived online flag, per field doc comments). The FS-22 test fixture
`tests/fs22_dashboard_coverage.rs` still constructs `dashboard_vm::PeerDownload`
with the old field set. The test file was committed under the FS-22 task and
never updated after FS-08 added the fields — a stale test fixture on main.

Because compilation fails, **zero tests ran**; there is no pass/fail count for
the suite at this commit.

Per-command warning totals observed during the test compile phase:
- `boru-core` (lib): 4 warnings
- `boru-core` (lib test): 8 warnings
- `boru-core` (test "fs17_activity_log"): 5 warnings
- `boru-core` (test "fs22_dashboard_coverage"): 1 warning
- `boru-core` (test "mailbox"): 3 warnings

## Pre-existing failure summary

1. `cargo fmt --all -- --check` fails (560 diffs, import-order/line-wrap drift).
2. `cargo test` fails to compile — stale `fs22_dashboard_coverage.rs` test
   fixture missing `online`/`peer_display` fields on `PeerDownload`.
3. 4 lib warnings (check), 217 example warnings (GUI build) — pre-existing.

Per task scope, **none of these were fixed** — this is the baseline the call
feature work starts from. The `cargo test` blocker will need a remediation
(update the FS-22 fixture or gate the test) before the full suite can run.
