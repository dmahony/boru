Boru Home redesign — UI-09 final diff review and handoff

Review checkout

- Worktree: /home/dan/iroh-gossip-chat/.worktrees/t_c151f8aa
- Branch: wt/t_c151f8aa
- Baseline: origin/main 00009f1d
- Integrated review HEAD: 726beb11
- Integration commits: a1f7ae2d (UI-07), 726beb11 (UI-08)
- `git fetch origin && git merge origin/main`: PASS; origin/main was already current.

Implementation coverage

- UI-00 / t_c6dc9c8a: baseline audit, f63073fd.
- UI-01 / t_8ff42b63: scoped Home visual foundation, 3716a812.
- UI-02 / t_faf70c5a: compact horizontal Quick Actions with existing handlers, 813c7509.
- UI-03 / t_a779a070: readable truthful People rows and actions, 61e51654.
- UI-04 / t_417f7fdb: bounded Recent Activity feed and Activity Log action, 91ac3eec.
- UI-05 / t_0d93cf67: compact responsive Tunnels empty state, 12fc063f.
- UI-06 / t_e33312e9: responsive dashboard integration, 1d4e4c22.
- UI-07 / t_28aa995f: keyboard focus/activation and tunnel close labeling, 04a14a65.
- UI-08 / t_0da77b2a: verification evidence and limitations, b9fc42ee.
- UI-09: this final review and handoff artifact.

Diff review

- `git diff --stat origin/main...HEAD`: 11 files, 626 insertions, 218 deletions.
- Changed source is limited to Home rendering, Quick Actions, the Home visual helper, the Activity Log route, and Home locale keys. Cargo.lock only reflects the pre-existing Boru version 0.241.0 -> 0.241.1.
- No Cargo.toml dependency addition, asset/font/icon replacement, protocol/networking/persistence change, sidebar change, hero/logo change, connection-card interior change, or private runtime data was found.
- `git diff --check origin/main...HEAD`: PASS.
- The review found no directly evidenced scoped regression requiring an additional code change.

Verification commands and results

- `cargo metadata --no-deps --format-version 1`: PASS.
- `rb check --bin boru --features gui,video-playback,terminal`: BLOCKED before Cargo; rsync failed with `No space left on device (28)` writing DEBSRV `/home/dan/boru-build/work-1/.git`.
- `rb test --bin boru --features gui,video-playback,terminal -- home_visual home_people_activity quick_action_grid focusable_button --test-threads=1`: BLOCKED before Cargo by the same DEBSRV ENOSPC failure.
- `cargo fmt --all -- --check`: FAIL due repository-wide pre-existing formatting differences outside this review (including benches/compression_bench.rs and unrelated app/test files); no formatter changes were applied.
- Previously recorded upstream checks remain attributed to their workers only; this final integrated checkout has no compile/test pass claim because the remote prerequisite failed.

Evidence and limitations

- Source/diff evidence: this file and `docs/ui-redesign/evidence/t_c6dc9c8a/audit.md`.
- Upstream verification limitation: `docs/ui-redesign/evidence/t_0da77b2a/verification.md`.
- Runtime screenshots: none available in this review checkout. The documented Xvfb scripts require a local GUI binary, while the prescribed DEBSRV build could not complete its rsync step.
- Runtime visual matrix (1891x1014, 1440x900, 1280x800, 1024x768, minimum size; 100/125/150% scales; light/dark/alternate accent; empty/populated/loading/error states) and interactive mouse/keyboard checks remain untested here.

Final status

- Final diff is focused and reviewable by source inspection.
- Functional behavior is preserved by existing AppMessage routes and the source-level review; runtime behavior and compile/test results remain unverified in this final checkout because DEBSRV storage is full.
- No release, push, or unrelated infrastructure change was performed.
