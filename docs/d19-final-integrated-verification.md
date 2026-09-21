# D19 final integrated verification

Date: 2026-09-21
Branch: `wt/t_dc817a2b`
Integrated HEAD: `cb04dbb9582c4d26d399e321fa874952b415f94f`
Base checked: `origin/main` at `0e2d5cd781fcbbed13d783e28c50f9b8638340e6`
Decision: **NOT release-approved**. The integrated implementation is pushed to `origin/main`, but the evidence contract remains fail-closed: focused delivery gates pass while full-library, formatting, platform, and real offline-delivery gates remain unresolved.

## Integrated commits

The final branch contains the following six commits on top of `origin/main`:

- `aceba5dc741f965cc40ba8c609ddb35123376a2b` — legacy outbox migration/quarantine.
- `f3fbb407bc6959b74104158fa60f4dd82b5ba017` — D15 crash-boundary tests.
- `bf24db99fce602f2305dfc69062b7e954030ae93` — D16 real-process lifecycle evidence.
- `b17fa1f07b050726a01a2f2f920933384018031d` — D15.1 retry-envelope and receipt validation repair.
- `afe6ebaa31f56a1460c17bd8da672327eb9c4a88` — D17 compatibility/release verification.
- `cb04dbb9582c4d26d399e321fa874952b415f94f` — D18 behavior contract and evidence ledger.

`git diff --check origin/main...HEAD` passed before publication. No unrelated worktree changes were present.

## Final commands and results

All cargo commands below used DEBSRV through `/home/dan/bin/rb` with `RB_SLOTS=8`.

- `rb test --features gui --lib d15_`: **PASS**, 6 passed, 0 failed, 0 ignored, 2920 filtered.
- `rb test --features gui --lib d171_`: **PASS**, 1 passed, 0 failed, 0 ignored, 2925 filtered.
- `rb test --features gui --lib signed_message_roundtrip`: **PASS**, 12 passed, 0 failed, 0 ignored, 2914 filtered.
- `rb test --features gui --lib capability`: **PASS**, 17 passed, 0 failed, 0 ignored, 2909 filtered.
- `rb check --features gui --bin boru`: **PASS**, dev profile finished in 13.09s, 334 warnings.
- `rb test --features gui --lib`: **FAIL**, 2918 passed, 5 failed, 3 ignored, 0 measured. Failures: three legacy FileShare/Presence decoding tests, the diagnostics peer-address fixture assertion, and `file_indexer::tests::full_rescan_on_directory_create_still_indexes_nested_files` (expected 2 Added events, got 0). The file-indexer failure was not hidden or relabeled.
- `cargo fmt --all -- --check`: inherited repository formatting drift remains a known unresolved gate from D17; no broad formatting rewrite was applied.
- Package/script syntax checks from D17: **PASS** for the six release/install/smoke scripts.
- Real clean-exit smoke: **BLOCKED/UNVERIFIED** because the local `target/debug/boru` binary was absent in the recorded run.

Execution logs are retained outside the repository at `/home/dan/.hermes/profiles/codex-coder/cache/scratch/d19-{d15,d171,signed,capability,gui-check,full-lib}.log`.

## 24-scenario status

The supplied PDF acceptance artifact was not present in this checkout or in the D19 task attachments. The matrix below preserves the 24 acceptance dimensions represented by the D01–D18 cards and evidence ledger without inventing passes. `PASS` means reproducible evidence exists; `UNVERIFIED` means the implementation or a fixture exists but the required end-to-end/environmental proof does not; `BLOCKED` means the required host/binary/environment was unavailable.

| # | Scenario | Status | Evidence / limitation |
|---:|---|---|---|
| 1 | Legacy queued outbox import | PASS | D04 implementation and focused migration evidence. |
| 2 | Ambiguous legacy Sent/Delivered quarantine | PASS | D04 implementation inspection and migration tests. |
| 3 | Stable ID/envelope across retry | PASS | D15 focused tests, 6/6. |
| 4 | New send receives a distinct intent/ID | PASS | D15 focused test. |
| 5 | Sender admission rollback after fault/restart | PASS | D15 focused test. |
| 6 | Receiver admission deduplication after restart | PASS | D15 mailbox test evidence. |
| 7 | Forged receipt rejection | PASS | D15 receipt-binding test. |
| 8 | Wrong recipient/message receipt rejection | PASS | D15 receipt-binding and ACK integration evidence. |
| 9 | Invalid/late duplicate receipt safety | PASS | D15 receipt-fault test. |
| 10 | Acknowledged rows remain terminal during retry race | PASS | D15 terminal-state test. |
| 11 | v27→v28 envelope backfill and reopen | PASS | D17.1 upgrade test, 1/1. |
| 12 | Signed current/legacy message round-trip | PASS | 12/12 focused tests; full older FileShare/Presence compatibility still fails. |
| 13 | Capability negotiation and stale/unknown handling | PASS | 17/17 focused capability tests. |
| 14 | Older peer without receipt remains unconfirmed | UNVERIFIED | Behavior is specified and tested deterministically; no seeded older-peer network fixture. |
| 15 | Real Boru startup/process soak | PASS | D16 three-process real-process soak. |
| 16 | Same-data-dir process restart | PASS | D16 scheduled restart evidence. |
| 17 | Two independently persisted process probes | PASS | D16 MCP/node/gui/outbox/store probes. |
| 18 | Recipient offline, sender queues, sender restarts without resubmission | UNVERIFIED | D18 procedure documented; D16 did not inject a queued DM. |
| 19 | Recipient receive commit followed by lost ACK and retry | UNVERIFIED | No real receipt-loss kill boundary was exercised. |
| 20 | Exactly one durable receiver row and one UI bubble | UNVERIFIED | No real queued-message/bubble capture. |
| 21 | Genuine verified Delivered transition | UNVERIFIED | No real application ACK observed in D16. |
| 22 | Read/Seen only while conversation is visible | UNVERIFIED | Contract is documented; no end-to-end UI visibility run. |
| 23 | Expiry, retention, page/room limits and negative cases | UNVERIFIED | Deterministic limits exist, but the complete live acceptance sequence was not run. |
| 24 | LAN/WAN/relay/route/sleep and Windows/macOS runtime matrix | BLOCKED | No seeded network fixture and no Windows/macOS runtime host available. |

This matrix intentionally does not turn deterministic tests, mock `golden-recovery` output, screenshots, or process liveness into proof of real message delivery.

## Publication

After `git fetch origin` and confirmation that `origin/main` remained at `0e2d5cd781fcbbed13d783e28c50f9b8638340e6`, the integrated branch was pushed to `origin/main`. The remote exact-head check must report `cb04dbb9582c4d26d399e321fa874952b415f94f`; this report is only complete once that read-back check succeeds.

The project is therefore integrated and published, but not release-approved. The remaining gaps are explicitly recorded rather than silently claimed as passes.
