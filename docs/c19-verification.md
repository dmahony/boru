# C19 fault, network, and packaged verification

Date: 2026-09-21
Source under test: merge commit `1908539ccfcdfec888482651292e6ffa218a1bb6` (C18 `a4554112` merged into `wt/t_39af948a`).
Evidence directory: `/tmp/boru-c19-evidence/` (kept outside the repository).

This is a fail-closed verification record. The local runner had no reachable second desktop peer, WAN-like isolated path, forced-relay fixture, Windows desktop host, or mobile runtime. Those scenarios are explicitly `UNVERIFIED`; Linux unit/integration results are not promoted to network, Windows, or mobile acceptance.

## Executed evidence

| Command | Result |
|---|---|
| `rb test --lib companion` | PASS: 20 passed, 0 failed, 2928 filtered; includes wire-shape, v1 negotiation, pairing claim/expiry, rejection, authenticated history pagination, snapshot boundary, revocation, restart persistence, operation deduplication, reconnection, and diagnostics tests. |
| `rb test --test test_pairing_integration` | PASS: 5 passed, 0 failed; local-only endpoint, pairing persistence/reload, invalid key, unreachable peer, and multiple pending pairings. |
| `rb check --lib` | PASS, exit 0. Existing warnings remain; no errors. |
| `bash -n scripts/package-windows.sh scripts/package_windows.sh scripts/t29_run_integration_gate.sh scripts/t29_resume_integration_gate.sh` | PASS, exit 0. |
| `python3 -m py_compile scripts/check-release-feature-matrix.py scripts/release-validate.py scripts/gst_windows_manifest.py` | PASS, exit 0. |
| `git diff --check` | PASS, exit 0. |

Exact command output is in the evidence directory. The `rb` invocations used the repository's existing remote build wrapper and the current source snapshot.

## 24-scenario disposition

1. Desktop message host -> real peer: **UNVERIFIED** — no reachable peer fixture.
2. Real peer -> desktop message host: **UNVERIFIED** — no reachable peer fixture.
3. Normal desktop messaging in both directions: **UNVERIFIED** — no two-process desktop run.
4. LAN transport recording: **UNVERIFIED** — no second LAN process available.
5. WAN-like separate internet path: **UNVERIFIED** — no isolated internet-path peer available.
6. Forced relay transport recording: **UNVERIFIED** — no forced-relay fixture available.
7. Mid-send interruption and recovery: **UNVERIFIED** — no live transfer to interrupt.
8. Mid-sync interruption and recovery: **UNVERIFIED** — no live sync to interrupt.
9. Commit fault hook: **PASS (unit evidence)** — companion store mutation/revocation and operation-dedup tests passed.
10. Reply fault hook: **PASS (unit evidence)** — protocol rejection/idempotency and reconnection tests passed.
11. Snapshot-boundary fault hook: **PASS (unit evidence)** — strict snapshot boundary/resume and concurrent-arrival tests passed.
12. Revocation fault hook: **PASS (unit evidence)** — grant rotation, epoch reset, queued mutation, and cached-result revocation tests passed.
13. Independent probe-cache isolation: **PASS (unit evidence)** — snapshot/cache invalidation tests passed; no network probe run.
14. Relay/privacy policy preservation during interruption: **UNVERIFIED** — requires live transport instrumentation.
15. Hidden-window behavior: **UNVERIFIED** — no desktop GUI session was launched.
16. Sleep/resume behavior: **UNVERIFIED** — no suspend/resume-capable peer session.
17. Explicit Quit behavior: **UNVERIFIED** — no desktop GUI session was launched.
18. Abrupt process death: **UNVERIFIED** — no two-process session was launched.
19. Second launch/restart recovery: **PASS (local integration evidence)** — pairing round-trip and pending-pairing restart tests passed.
20. Restore after restart with revocation state: **PASS (unit evidence)** — registration/epoch restart and revocation tests passed; arbitrary database-file replacement was not claimed safe.
21. Linux package smoke: **PARTIAL/PASS** — Rust library check and package-script syntax/validator checks passed; no distributable Linux package artifact was assembled in this run.
22. Windows package smoke: **UNVERIFIED** — no Windows artifact or reachable Windows host; script and validator syntax only.
23. Android/mobile runtime: **UNVERIFIED** — no mobile runtime; mobile is not complete.
24. iOS/mobile runtime and push delivery: **UNVERIFIED** — no iOS runtime/provider; mobile is not complete.

## Limits and follow-up

The existing `scripts/t29_run_integration_gate.sh` remains the reusable per-test remote gate, with one Cargo test target per invocation and a 240-second timeout for relay-dependent suites. C19 did not claim that script's broad suite run as completed here because it requires the remote test environment and several suites can hang without a relay/IPv6 route.

A future run needs a real two-process desktop harness/peer fixture with transport-path recording and injectable commit/reply/snapshot/revocation faults, plus reachable LAN/WAN/relay and native Windows/mobile environments. Until then, the four unit-backed protocol/fault rows and five local pairing tests are the only positive C19 evidence; all other rows retain their explicit unverified disposition.
