# D18 evidence ledger and release hand-off

Date: 2026-09-21
Branch: `wt/t_31e9220a`

This ledger is intentionally fail-closed. “Inspection” means source/document
inspection only; “deterministic” means unit/integration/state-machine tests;
“real process” means a Boru executable was started; “sleep/network/package”
means the corresponding environmental gate. A PASS in one category is not
promoted into another category.

## D01–D18 ledger

| Work item | Commit / files available to this checkout | Evidence class | Commands/tests and result |
| --- | --- | --- | --- |
| D01 | No D01-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. Recover the D01 task card or branch before publishing a release ledger. |
| D02 | No D02-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D03 | No D03-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D04 | Referenced by D17 as legacy outbox import/backfill; standalone D04 commit/report is not present here. | Inspection via D17 | D17 records idempotent import and quarantine behavior; the original D04 command/count is not re-invented. |
| D05 | No D05-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D06 | No D06-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D07 | No D07-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D08 | No D08-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D09 | No D09-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D10 | No D10-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D11 | No D11-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D12 | No D12-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D13 | No D13-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D14 | No D14-labelled commit or report is present in the fetched refs/worktree. | Not recoverable here | No result is claimed. |
| D15 | `1720f13b` / `src/mailbox.rs`, `src/storage/tests.rs`; duplicate recording refs also exist. | Deterministic crash/fault tests | Six named tests: `d15_receiver_admission_is_restart_safe_and_fail_closed`, `d15_receipt_sender_binding_rejects_forgery_and_is_idempotent`, `d15_admission_faults_leave_no_partial_rows_after_restart`, `d15_retry_after_cleanup_and_restart_reuses_identity_but_new_intent_is_distinct`, `d15_receipt_fault_invalid_and_late_duplicate_are_safe`, `d15_acknowledged_rows_remain_terminal_across_retry_race`. D17 hand-off reports 6 passed, 0 failed, 0 ignored on DEBSRV. |
| D16 | `8a86ad10` / `docs/d16-real-process-lifecycle-evidence.md` (same evidence was recorded by later duplicate commits). | Real process; deterministic fixture separated | `rb build --features gui --bin boru` PASS; 3-process 8-second soak PASS; 3-node same-data-dir restart PASS; two independent MCP processes responded; no queued DM or genuine receipt was injected. Fixture workflow reported 15/15 but remains fixture-only. |
| D17 | `74e9c943` / `docs/d17-compatibility-release-verification.md`. | Deterministic, package/script, compatibility inspection; not release approval | Capability 17/17 PASS; D15 6/6 PASS; D17.1 1/1 PASS; signed round-trip 12/12 PASS; GUI check PASS with 334 warnings; full library 2919 passed, 4 failed, 3 ignored; `cargo fmt --check` FAIL; clean-exit smoke blocked by missing local binary. Release decision: NOT APPROVED. |
| D18 | This hand-off: `docs/d18-delivery-behavior-and-recovery.md` and this ledger. | Documentation and evidence boundary | No new cargo/network claim. The required two-process offline/restart/receipt-loss procedure is documented as UNVERIFIED because D16 did not run it. |

The missing D01–D14 entries are a provenance limitation, not implied success or
failure. Before using this as a complete historical release ledger, import the
corresponding task-card handoffs or commit IDs and replace each “not recoverable”
row with its exact files, commands, counts, and result.

## Verified source references

- `docs/offline-direct-messaging.md` records that the currently active GUI
  `/whisper` fallback still uses legacy in-memory mailbox APIs and does not
  establish durable SQLite retry/ACK behavior.
- `docs/message-storage-design.md` records SQLite as authoritative, lease-based
  outbox recovery, at-least-once transport, recipient deduplication, expiry, and
  forward-only migration behavior.
- `docs/e2e-test-control-contract.md` explicitly reports offline mailbox replay
  as a capability boundary rather than a fabricated pass.
- `docs/d16-real-process-lifecycle-evidence.md` distinguishes real process
  restart evidence from fixture-only recovery and lists the unexercised receipt
  and queued-message scenarios.
- `docs/d17-compatibility-release-verification.md` records the focused PASS
  counts and the unresolved full-suite/formatting/platform limitations.

## Evidence classification rules for future runs

1. Record the exact command, working tree/commit, host/platform, timeout, and
   exit status. Include test filter and passed/failed/ignored counts.
2. Label sleep, network topology, relay/DHT, package/install, and OS-runtime
   checks separately from deterministic storage tests.
3. For a delivery claim, record the stable message ID, sender outbox row,
   recipient row, ACK verification event, restart boundary, and sender/receiver
   bubble counts. A screenshot alone is insufficient.
4. If a prerequisite is unavailable, write `BLOCKED` or `UNVERIFIED` with the
   missing capability. Never convert it to PASS because a mock or local probe
   succeeded.
5. Keep release publishing separate from this evidence hand-off. D17 is not
   release-approved, and D18 does not change that decision.
