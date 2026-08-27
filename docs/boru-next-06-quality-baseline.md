# BORU-NEXT-06 repository quality baseline

Date: 2026-08-27
Branch: `wt/t_bb50ecf2`
Parent integrated: `origin/wt/boru-next-5` at `dcd8968f661908534b39907146a262033cd9cf68`

## Commands and results

The required repository-wide checks were run from this worktree.

| Command | Result | Classification |
|---|---|---|
| `cargo fmt --all --check` | **FAIL (exit 1)** | Pre-existing repository-wide formatting drift. The output contains extensive diffs in unrelated benchmark, GUI, protocol, and integration-test files. No repository-wide reformat was applied. |
| `RB_SLOTS=8 rb clippy --workspace --all-features -- -D warnings` | **FAIL (exit 101)** | Pre-existing strict-lint debt: Cargo reports 260 errors before aborting. The diagnostics include `missing_debug_implementations`, `result_large_err`, `large_enum_variant`, `type_complexity`, `let_unit_value`, `non_upper_case_globals`, and an unfulfilled `clippy::too_many_arguments` expectation. |
| `git diff --check` | **PASS (exit 0)** | No whitespace errors in the task changes. |

The strict clippy command was run through `rb` on DEBSRV as required. The formatter
check is intentionally the exact plain repository-wide command; it fails before any
formatting changes are made.

## Baseline disposition

These failures are not introduced by BORU-NEXT-06: the task branch contains no
production-code changes, and the parent branch's focused discovery-test correction
is already integrated. Existing baseline evidence in
`docs/architecture-refactor/baseline.md` records the same repository-wide format
drift and strict-lint failure family, including the pre-existing screen-share and
group-encryption diagnostics.

This task therefore records an actionable release-owner disposition rather than
silently applying a broad reformat or unrelated refactor:

1. Keep repository-wide formatting as a release gate until a dedicated formatting
   convergence change can be reviewed; that change must be deliberately scoped and
   must not be mixed into feature work.
2. Triage strict clippy by subsystem, beginning with the configured
   `missing_debug_implementations` baseline and the group-encryption error-size
   diagnostics, then the screen-share FFI naming/debug diagnostics and stale lint
   expectations. Each subsystem change requires its own tests and review.
3. Do not grant a blanket `-D warnings` waiver. If release proceeds before this
   convergence work, the release owner must explicitly record a time-bounded waiver
   naming the remaining diagnostics and an owner for their remediation.

No credentials, keys, tickets, room secrets, or message bodies are included here.
