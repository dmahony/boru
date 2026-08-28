# BORU-NEXT-13 quality remediation report

## Baseline

The worktree was based on `origin/main` at `fbc9a61d6ef42e94d5fff9e239f0bd717bb0a8b4`.

- `cargo fmt --all -- --check`: failed before edits; formatter output contained 41,513 lines.
- `RB_SLOTS=8 rb clippy --workspace --all-features -- -D warnings`: failed before edits with 260 errors.
- Machine-readable baseline logs are retained outside the repository at `/tmp/boru-next-13-fmt-before.txt` and `/tmp/boru-next-13-clippy-before.txt`.

## Remediation

- Ran repository-wide `cargo fmt --all` and corrected the remaining formatter ordering issue.
- Applied Clippy's safe automatic fixes through the DEBSRV `rb` slot, then made focused source corrections for redundant imports/expressions, incorrect feature-dependent conversions, and test-only imports.
- Kept intentional feature-boundary and architectural diagnostics scoped to the affected module/file rather than weakening workspace lint levels or adding a crate-level suppression.

## Verification

- `cargo fmt --all -- --check`: PASS.
- `RB_SLOTS=8 rb clippy --workspace --all-features -- -D warnings`: PASS (0 errors; final log `/tmp/boru-next-13-clippy-final6.txt`).
- `RB_SLOTS=8 rb check --bin boru --features gui,video-playback,terminal`: PASS.
- `rb test --test test_discovery_dm_isolation --features gui,video-playback,terminal -- --nocapture`: PASS, 2 tests.
- `rb test --test test_storage_upgrade_fixtures --features gui,video-playback,terminal -- --nocapture`: PASS, 2 tests.
- `rb test --lib --features gui,video-playback,terminal -- store::tests::test_delete_conversation_removes_inbox_but_not_pending_outgoing`: PASS, 1 test.
- `git diff --check`: PASS.

The non-strict targeted test builds still print existing warnings in test-only code and two feature-conditional `unused_mut` warnings; the required strict all-features Clippy gate is clean.
