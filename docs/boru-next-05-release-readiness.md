# BORU-NEXT-05 final release-readiness synthesis

Date: 2026-08-27
Branch under review: `wt/boru-next-5`
Required evidence chain: BORU-NEXT-01 through BORU-NEXT-04
Decision: **NOT RELEASE-READY; do not merge this gate to `main`**

This is a fail-closed synthesis of the available repository, DEBSRV, and Linux
GUI evidence. It records verified passes separately from unavailable platform
or publication gates; no credentials, private keys, tickets, room secrets, or
message bodies are included.

## Acceptance audit

| Area | Result | Evidence and boundary |
|---|---|---|
| Chat routing and direct-address privacy | PASS | `tests/test_discovery_dm_isolation.rs` captures both topics and passes the bidirectional A→B/B→A isolation assertions; direct messages verify as chat only on the deterministic direct topic and discovery samples verify as discovery only. |
| Friendship state and direct conversation boundary | PASS (focused) | The discovery fixture documents the friendship/direct-topic setup and the `OpenFriendChat` → `BackgroundSubscribe` path; the focused test passed 2/2. This is not a claim of a full native GUI friendship workflow. |
| Room membership | PASS | Seeded-peer smoke reports both nodes joined the same explicit room and records reciprocal mesh membership; structured evidence reports `room_membership: PASS`. |
| `--no-dht` behavior | PASS (bounded release soak) | The packaged DEBSRV gate ran three isolated real Boru processes with `no-dht`; cleanup completed with no failures. This validates startup/soak behavior, not every DHT-disabled feature permutation. |
| Persistence | PARTIAL | The existing change persists conversation deletion state, and the release chain includes persistence-related changes. BORU-NEXT-03 explicitly did **not** validate deletion persistence because the seeded-peer fixture has no supported deletion action; no end-to-end pass is claimed here. |
| Visible UI behavior | PASS (Linux seeded-peer smoke) | Both Ubuntu VMs launched the exact artifact with visible X11 windows; room navigation, composer update/submission, bidirectional GUI message flow, and a visible file-share/download round trip passed. Calls and screen sharing were unavailable in the fixture. |
| Linux packaged release | PASS with baseline warnings | DEBSRV release build, package assembly, executable smoke, SPDX SBOM, validators, checksums, provenance subject validation, and bounded soak passed. The build emitted 340 existing warnings. |
| Windows native/package gate | NOT RUN / BLOCKED | No Windows packaged candidate was available and the documented host `172.16.0.17` was unreachable. Cross-checks are blocked by missing MinGW/MSVC toolchain components on DEBSRV; these are environment limitations, not Windows runtime passes or failures. |
| macOS arm64 native gate | NOT RUN / BLOCKED | No native macOS runner or artifact was available; DEBSRV lacks the target's Rust core. Native macOS screen sharing remains experimental/unsupported and test-pattern-only. |

## Verification performed for this synthesis

- `rb check --bin boru --features gui,video-playback,terminal` — PASS.
- `rb test --test test_discovery_dm_isolation --features gui,video-playback,terminal -- --nocapture` — PASS, 2 passed.
- `python3 scripts/check-release-feature-matrix.py` — PASS.
- `python3 scripts/soak_harness.py --self-test` — PASS.
- `bash -n scripts/package_windows.sh scripts/package-windows.sh scripts/release-sign.sh scripts/release-checksums.sh` — PASS.

The check emitted the repository's existing warning baseline; no unrelated
warning cleanup or broad formatting rewrite was made.

## Exact remaining blockers

1. **Repository quality baseline:** repository-wide `cargo fmt --check` and
   strict workspace clippy with `-D warnings` remain failing due to existing
   unrelated drift/debt. These must be triaged or explicitly waived by the
   release owner.
2. **External provenance attestation:** GitHub Actions attestation was not run;
   it requires a publication context. No release, tag, signing key, or
   publication was created in this task.
3. **Windows runtime/package verification:** requires an accessible Windows
   host and a real packaged Windows candidate (or an equivalent CI artifact).
4. **macOS arm64 verification:** requires a native macOS runner/artifact;
   native ScreenCaptureKit implementation is also absent and must not be
   advertised as supported.
5. **Persistence UI round trip:** conversation deletion persistence needs a
   fixture action and a real restart/reopen verification before it can be
   called complete.
6. **Optional UI capabilities:** call and screen-share lifecycle were not
   available in the seeded-peer fixture, so they remain unverified.

Because blockers 1–5 remain, `main` was intentionally not advanced. The
verified evidence and this synthesis are pushed only on the task branch for
review and release-owner disposition.
