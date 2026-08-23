# BORU 0.227 Alpha-readiness gate report

Date: 2026-08-23 UTC
Integrated revision: `7e0c0992e7a218b19f888bcbc2007ddae4be4b77`
Worktree: `wt/t_d9705732`
Decision: **NOT RELEASE-READY**

This is the final integrated audit for the Alpha-readiness gate. The quality,
architecture, formatting, artifact-workflow, and soak evidence workstreams were
merged into this worktree in the prescribed order. No product-code change was
made solely to turn a gate green; no version bump, tag, signing, publication, or
platform claim was made.

## Gate results

| Gate | Result | Evidence / limitation |
|---|---|---|
| Architecture guardrails | PASS | `./scripts/check-module-size.sh --enforce`; all curated coordinator/facade caps passed, including `src/bin/boru/app.rs` at 17,799 lines against the 19,000 cap. |
| Formatting | FAIL | `cargo fmt --check` reports remaining formatting drift in `src/call/adaptation.rs` and `src/video_playback.rs`. The drift was not silently rewritten during this audit. |
| Strict workspace Clippy | FAIL | `rb clippy --workspace --all-features -- -D warnings` exits 101 with 244 errors. The first diagnostics include screen-share unused imports/unsafe blocks/unused assignments and existing `group_encryption` large-error/enum debt. |
| Build matrix | PASS | DEBSRV `rb check --workspace`, `rb check --workspace --no-default-features --features net`, and `rb check --workspace --all-features` all completed successfully. Non-strict warnings remain. |
| Regression sentinels | PASS | `python3 scripts/check-release-feature-matrix.py --test` (2 tests) and the matrix check passed; `rb test --lib --features net -- room_registry` passed 8/8; soak harness self-test passed. |
| Linux package | UNVALIDATED | No exact Linux release archive was produced in this gate, so archive extraction, executable smoke, checksum, and support-bundle evidence cannot be claimed. |
| Windows package | UNVALIDATED | Windows runner/package evidence is not available in this Linux worktree. The workflow contains the package and smoke steps, but checked-in workflow logic is not execution evidence. |
| macOS package | UNVALIDATED | No macOS arm64 runner/package evidence is available. Native macOS execution and signing were not claimed. |
| Integrity and signing | UNVALIDATED | Release scripts and validators pass syntax checks, and signing is explicitly represented as unsigned unless credentials are supplied. Actual per-target archive checksums/SBOM/provenance/signature evidence is absent. The fail-closed aggregate validator reported all three targets `UNVALIDATED` for missing artifacts. |
| Soak | NOT VALIDATED | Developer lifecycle/fault soak evidence is PASS for 7,200 seconds/3 nodes with cleanup verified. The exact 28,800-second/6-node RC evidence remains fail-closed `NOT_VALIDATED` because required application assertions, resource thresholds, clean isolation, and cleanup proof are absent. |

## Executed commands

- `git fetch origin && git merge origin/main` — already up to date.
- `./scripts/check-module-size.sh --enforce` — PASS.
- `cargo fmt --check` — FAIL; two files listed above.
- `python3 scripts/check-release-feature-matrix.py --test` — PASS, 2 tests.
- `python3 scripts/check-release-feature-matrix.py` — PASS.
- `python3 scripts/soak_harness.py --self-test` — PASS.
- `rb check --workspace` — PASS.
- `rb check --workspace --no-default-features --features net` — PASS.
- `rb check --workspace --all-features` — PASS.
- `rb clippy --workspace --all-features -- -D warnings` — FAIL, exit 101, 244 errors.
- `bash -n scripts/package_windows.sh scripts/release-sign.sh scripts/release-artifact-smoke.py scripts/release-evidence-check.py scripts/generate-spdx-sbom.py` — PASS.
- `python3 -m py_compile scripts/release-artifact-smoke.py scripts/release-evidence-check.py scripts/generate-spdx-sbom.py` — PASS.
- `rb test --lib --features net -- room_registry` — PASS, 8 passed/0 failed.
- `python3 scripts/release-evidence-check.py <empty-evidence-dir> --output ...` — expected fail-closed result: overall FAIL, Linux/Windows/macOS all `UNVALIDATED` because artifacts were missing.
- `git diff --check` — PASS; final worktree clean after report creation is verified below.

## Blockers

1. Repository-wide `cargo fmt --check` is not clean.
2. Strict workspace Clippy remains red, including non-app screen-share and
   group-encryption diagnostics.
3. Exact Linux, Windows, and macOS release archives and their smoke/checksum
   evidence were not produced.
4. The six-node, eight-hour RC soak is not validated under the fail-closed
   criteria.

Because intended-platform P0 gates lack evidence and quality gates are red, the
only supported Alpha decision is **NOT RELEASE-READY**. The older
`docs/release-gate-report-0.227.md` was not rewritten; this file is the new
integrated report for this audit.
