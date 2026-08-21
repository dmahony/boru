# Peer profile integration verification

Date: 2026-08-21
Branch: `wt/peer-profile-integration`
Base: `14d01b54` (`origin/wt/peer-profile-phase5`)

## Automated verification

All compile and test commands were run through `rb` from this worktree.

- `git fetch origin && git merge --no-edit origin/ui/move-chat-connection-bar && git merge --no-edit origin/wt/peer-profile-phase5`
  - Passed; the worktree fast-forwarded to `14d01b54`.
- `rb test --lib -- profile`
  - Passed: 39 passed, 0 failed, 2,844 filtered out.
  - Covered public profile wire round trips, malformed/oversized payload rejection,
    protocol dispatch, revision handling, SQLite persistence/reopen, malformed-row
    cleanup, and profile cache expiry behavior.
- `rb test --bin boru --features gui,video-playback,terminal -- profile`
  - Passed: 5 passed, 0 failed, 1,690 filtered out.
  - Covered peer-profile UI state/back navigation and profile-image ticket
    de-duplication.
- `rb check --bin boru --features gui,video-playback,terminal`
  - Passed. Existing warning set was emitted; no errors.
- `git diff --check`
  - Passed.

The profile wire tests confirm that private local file-sharing policy fields and
filesystem paths are absent from `PublicUserProfile`; malformed public payloads
are rejected or dropped before caching/persistence.

## Two-peer runtime verification

Not run in this environment. No configured Boru two-instance harness or available
peer processes were present in this isolated worktree, so this report does not
claim live peer propagation, restart behavior, or bidirectional chat delivery.
Those scenarios remain the required manual/runtime gate: edit peer A's profile,
observe it on peer B, change bio/avatar, restart one peer, verify persistence and
expiry, and send normal chat in both directions.
