# BORU 0.227 release-gate report

Date: 2026-08-21
Workstream: BORU-0227-J
Decision: **NOT RELEASE-READY**

The workstreams were integrated in the prescribed low-conflict order: registry
(C), storage upgrades (E), soak harness (H), app decomposition (B), discovery
decomposition (D), release feature matrix (A), release integrity (G), and
support bundle (I). The macOS capability evidence from F was integrated as
release documentation. No tag, version change, push, or publication was made.

## Gate results

| Gate | Result | Evidence / limitation |
|---|---|---|
| Curated architecture guardrails | PASS | `./scripts/check-module-size.sh --enforce`; `app.rs` 35,681 lines (cap 36,000), `discovery_service.rs` 2,312 (cap 2,500). CI now invokes `--enforce`; broad >2,500-line reporting remains advisory. |
| Release feature matrix | PASS | `python3 scripts/check-release-feature-matrix.py` and its 2 parser tests. |
| Release script syntax/validators | PASS | Release validation for `v0.225.0`, Python compilation, and `bash -n` for packaging/signing/checksum scripts. Current Cargo version is 0.225.0; no version bump was authorized. |
| Soak harness smoke | PASS | `python3 scripts/soak_harness.py --self-test`; real Boru process soak was not run because no freshly packaged integration binary was produced in this gate. |
| Workspace all-features check | PASS | `RB_SLOTS=8 rb check --workspace --all-features`. Existing warnings remain. |
| Workspace net/no-default and default checks | PASS | `RB_SLOTS=8 rb check --workspace --no-default-features --features net` and `rb check --workspace`. |
| Registry hostile-input tests | PASS | 8 targeted `room_registry` tests passed. |
| Discovery decomposition tests | PASS | 115 targeted `discovery_service::tests` passed. |
| Support-bundle security tests | PASS | 2 targeted `support_bundle` tests passed. |
| Upgrade fixtures | PASS | 2 `test_storage_upgrade_fixtures` tests passed, including reopen/idempotence, integrity, future-schema rejection, and backup/restore. |
| `cargo fmt --check` | FAIL / baseline drift | Repository-wide formatting check reports extensive existing drift across unrelated files; no broad reformat was applied. |
| Clippy with `-D warnings` | FAIL / baseline debt | `RB_SLOTS=8 rb clippy --workspace --all-features -- -D warnings` fails with 260 existing warnings/errors across unrelated modules, including large-error and screen-share warning debt. |
| Exact packaged release matrix | NOT RUN | Linux/Windows/macOS release packaging and artifact smoke/checksum/SBOM publication were not executed in this convergence worktree. The release workflow now contains validation, checksum, SPDX SBOM, signing hooks, and provenance attestation. |
| macOS arm64 | UNSUPPORTED / untested | The remote host lacks `aarch64-apple-darwin` (`E0463: can't find crate for core`). Native macOS screen sharing remains Experimental/unsupported and must not be advertised; the backend is test-pattern-only. |

## Focused audit

The merged changes retain the existing direct-message topic/routing, friendship
and auto-conversation boundaries, private-room handling, relay-only address
publication, `--no-dht` path, and capability advertisement code. The release
feature matrix explicitly records platform feature status and the release
workflow validates it. No credentials, private keys, tickets, message bodies,
or generated sensitive files were added.

## Blockers and follow-ups

1. Release readiness is blocked by the repository-wide formatting baseline,
   strict clippy debt, and the unexecuted packaged release matrix.
2. A DEBSRV release-candidate run is still required for exact Linux artifact
   packaging/checksums/SBOM smoke and for the bounded real-process soak.
3. Windows hardware/runtime checks and macOS arm64 checks require their
   respective platforms. macOS native ScreenCaptureKit remains a separate
   capability task, not a release claim.

This report is intentionally a truthful gate record rather than a publication
approval.
