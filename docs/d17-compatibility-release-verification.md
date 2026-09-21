# D17 compatibility and release verification

Date: 2026-09-21
Branch: `wt/t_46f411e4`
HEAD: `eebdaf3a97dadf99ccca35fa289da5228a2ad1c4`
Decision: **NOT release-approved**. Focused delivery and compatibility evidence passes, but repository-wide quality and full compatibility gates remain unresolved.

## Verified implementation boundary

- D04 legacy outbox import is present at schema version 27/28 lineage and is idempotent. Proven `Queued` rows retain their event identity, hash, signed envelope bytes, retry count, and timestamps; ambiguous `Sent`/`Delivered`/invalid-binding rows are quarantined rather than replayed.
- D15/D17.1 preserves retry envelopes and rejects non-acceptance or forged receipts. The repair commit is integrated in this worktree and matches `origin/wt/t_7af93ee5` commit `b3e818813cb606cff211feeb12d1bd29e9b4156d`.
- Signed protocol objects use domain-separated canonical bytes with explicit versions. `docs/protocol-signing.md` documents canonical and legacy verification for migration compatibility, including `boru/chat-message` and `boru/mailbox-ack`.
- Capability advertisements are explicit, bounded, versioned, and treated as negotiation hints rather than authorization. Unknown capability IDs are preserved/ignored safely; stale capability updates cannot downgrade the cached set. A peer that never advertises capabilities is represented as unknown, not as an empty supported set.
- Older peers that do not emit receipt evidence cannot be promoted to `Delivered`/`Seen`; the UI/state machine must remain unconfirmed rather than fabricating receipt state. Full real-network older-peer round-trip evidence was not available in this environment.

## Commands and exact results

All remote cargo work used `/home/dan/bin/rb` as required.

- `RB_SLOTS=8 /home/dan/bin/rb check --features gui --bin boru` — PASS; dev profile finished in 1m51s; 334 existing warnings.
- `RB_SLOTS=8 /home/dan/bin/rb test --features gui --lib d15_` — PASS: 6 passed, 0 failed, 0 ignored; 2920 filtered.
- `RB_SLOTS=8 /home/dan/bin/rb test --features gui --lib d171_` — PASS: 1 passed, 0 failed, 0 ignored; 2925 filtered.
- `RB_SLOTS=8 /home/dan/bin/rb test --features gui --lib signed_message_roundtrip` — PASS: 12 passed, 0 failed, 0 ignored; 2914 filtered.
- `RB_SLOTS=8 /home/dan/bin/rb test --features gui --lib capability` — PASS: 17 passed, 0 failed, 0 ignored; 2909 filtered.
- Earlier full run `RB_SLOTS=8 /home/dan/bin/rb test --features gui --lib` — FAIL: 2919 passed, 4 failed, 3 ignored. Failures are `old_wire_format_file_share_decodes_correctly`, `old_wire_format_file_share_decodes_to_single_file_defaults`, `old_wire_format_presence_decodes_correctly` (legacy decoding `DeserializeUnexpectedEnd`), and `diagnostics::tests::test_peer_state_and_build_evidence` (fixture address assertion). This is not claimed as a clean full-suite pass.
- `bash -n scripts/install.sh scripts/package_windows.sh scripts/package-windows.sh scripts/release-sign.sh scripts/release-checksums.sh scripts/clean-exit-smoke.sh` — PASS.
- `xvfb-run -a ./scripts/clean-exit-smoke.sh` — NOT RUN to completion: local `target/debug/boru` is absent; script fail-closed with exit 2 and instructed an `rb build` first.
- Bounded formatting check: `timeout 45s cargo fmt --all -- --check` — FAIL (exit 1; output was captured separately at `/home/dan/.hermes/cache/scratch/d17-review-fmt.log`, approximately 2.4 MB). The repository has inherited formatting drift; no broad formatting rewrite was applied.
- `./scripts/install.sh --help` was not used as a smoke test because the script does not provide a help-only path and attempted a build until the 60-second command timeout. No install success is claimed.

## Upstream evidence incorporated

- D16 real-process lifecycle evidence (`docs/d16-real-process-lifecycle-evidence.md`): DEBSRV GUI build, local process soak, same-data-dir restart schedule, and two-process probes passed. It explicitly does not prove queued direct-message kill/restart recovery, genuine Delivered receipts, arbitrary network topologies, sleep/resume, or Windows runtime behavior.
- D15.1 focused evidence: D15 6/6, upgrade/backfill/reopen regression 1/1, mailbox 21/21, ACK integration 5/5, and outgoing-DM integration 9/9 passed on DEBSRV; GUI check passed.

## Release limitations

1. Full library tests are not green because four tests fail, including three legacy wire-format decoding tests. Compatibility is therefore not release-approved for all older FileShare/Presence encodings.
2. Repository-wide `cargo fmt --check` is failing due to existing drift.
3. Fresh-install, pending-message upgrade, failed-migration recovery, and bounded large-queue checks were not all exercised as a single clean-machine matrix. Deterministic migration/restart tests and D16 lifecycle evidence are narrower than that acceptance claim.
4. No Windows runtime/package host was available. No macOS native gate was run.
5. No real seeded older-peer network fixture was available to prove bidirectional current↔older delivery and the older-peer unconfirmed state.

The branch preserves the verified focused evidence and intentionally does not claim release readiness or advance `origin/main`.
