# BORU 0.227 release-gate report

Date: 2026-08-28
Workstream: BORU-0227-J
Decision: **NOT RELEASE-READY**

The workstreams were integrated in the prescribed low-conflict order: registry
(C), storage upgrades (E), soak harness (H), app decomposition (B), discovery
decomposition (D), release feature matrix (A), release integrity (G), and
support bundle (I). The macOS capability evidence from F was integrated as
release documentation. The final audit chain was recovered from
`wt/t_c0beb7d3` at `17f798e8` and its merge base `1b211d41`; the audit content
was cherry-picked into this release-candidate branch. The verified
BORU-NEXT-11 candidate was subsequently published and merged by BORU-NEXT-12;
no tag, version change, signing, publication, or external attestation was made.

## BORU-NEXT-12 closeout

- Candidate commit: `f659f169bbfa1dce1135e4001f3a164de432d8d5`
- Candidate remote: `origin/wt/boru-next-11` (push verified)
- Pre-merge `origin/main`: `d1e2a45b7af55a8e6f66bdc64dada7ab7d802f61`
- Merge commit: `72d49cf772c621c77bc27be7553095193274fa3b`
- Post-merge checks: GUI `rb check` PASS; discovery isolation 2/2 PASS;
  storage upgrade fixtures 2/2 PASS; conversation-delete regression 1/1
  PASS; release feature matrix, Python compilation, soak self-test, and
  `git diff --check` PASS.
- Final status remains **NOT RELEASE-READY**. The unresolved formatting
  baseline, strict-clippy baseline, and unavailable Windows/macOS runtime
  gates remain blockers above; external attestation/publication remains
  intentionally unrun.

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
| Linux packaged release | PASS with warnings; publication blocked | `docs/release-gate-report-0.227.1.md`: DEBSRV release build, package assembly, executable smoke, SPDX SBOM (964 packages), validators, checksums, local provenance-subject validation, and bounded no-DHT soak passed. The release build emitted the documented existing warning baseline; external attestation was not run. |
| Windows packaged runtime | BLOCKED / untested | `docs/boru-next-07-windows-runtime-gate.md`: no candidate artifact or reachable Windows host; DEBSRV and local GNU cross-build attempts failed at toolchain/linker prerequisites. No Windows runtime claim is made. |
| macOS arm64 | BLOCKED / untested | `docs/boru-next-08-macos-arm64-gate.md`: no native Apple Silicon runner or candidate artifact; the available host lacks the target standard library. Native macOS screen sharing remains Experimental/unsupported and must not be advertised; the backend is test-pattern-only. |

## Focused audit

The merged changes retain the existing direct-message topic/routing, friendship
and auto-conversation boundaries, private-room handling, relay-only address
publication, `--no-dht` path, and capability advertisement code. The release
feature matrix explicitly records platform feature status and the release
workflow validates it. No credentials, private keys, tickets, message bodies,
or generated sensitive files were added.

## Provenance and ancestry reconciliation

The required parent relationship was verified independently of this branch's
later UI commits: `git merge-base --is-ancestor origin/wt/boru-next-5
1b211d419fd43537f20272a16f15175d68357f1c` returned exit 0. The final audit
commit `17f798e8` was then applied to this branch, followed by the audit-chain
merge diff containing the gate reports and non-sensitive seeded-peer evidence.
This branch therefore has the audit evidence and its provenance inputs, but
does not claim that the unrelated remote parent ref is a direct ancestor of
the release-candidate tip.

## Blockers and follow-ups

1. Release readiness is blocked by the repository-wide formatting baseline,
   strict clippy debt, and unresolved Windows/macOS runtime gates.
2. The Linux packaged candidate gate passed with warnings, but external
   provenance attestation and publication remain intentionally unrun and
   release-owner controlled.
3. Windows hardware/runtime checks and macOS arm64 checks require their
   respective platforms. macOS native ScreenCaptureKit remains a separate
   capability task, not a release claim.

This report is intentionally a truthful gate record rather than a publication
approval.
