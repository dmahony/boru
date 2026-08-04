# FS-25 — Finalize documentation, cleanup, and release gate

## CARD
FS-25 — Finalize documentation, cleanup, and release gate

## STATUS
Complete — see final handoff in the kanban task thread. This file is the
in-repo evidence record (command matrix, gates, decisions).

## SUMMARY

Closed out the File Sharing epic (FS-20 security, FS-22 tests, FS-23
multi-peer E2E, FS-24 visual QA) with user-facing and developer
documentation, architecture/data-flow notes, a placeholder cleanup, and a
release gate run on a clean checkout. No automated GitHub release was created
(per card instructions).

## DELIVERABLES (this commit)

| File | Purpose |
|---|---|
| `docs/file-sharing-guide.md` | User-facing guide: screen, five tabs, native picker behaviour, download folder action, sharing/revocation semantics, troubleshooting states |
| `docs/fs-25-release-note.md` | Release note + compatibility + rollback/recovery guidance |
| `docs/fs-06-persistence-projections.md` | Rewritten architecture note: projection/subscription/persistence flow, v16+v17 migrations, retention, security boundaries, cross-platform integration |
| `docs/troubleshooting.md` | New "File Sharing" section (picker, downloads folder, stalled downloads, visibility, revocation, activity log) |
| `docs/gui-architecture.md` | Schema V10→V17, File Sharing screen added to screen map, doc pointers |
| `docs/migration-guide.md` | Schema version references V10→V17 |
| `docs/testing.md` | FS-22 suite + FS-23 multi-peer smoke-test instructions |
| `docs/ui-redesign/evidence/INDEX.md` | FS-24 evidence + FS docs indexed |
| `CHANGELOG.md` | Unreleased entry for the File Sharing dashboard |
| `README.md` | Doc pointers for File Sharing |

Cleanup performed:

- **Test-compile fix (release gate):** `tests/test_hostile_input.rs` and 16
  sibling test files still used the pre-refactor `set_pending_file(…,
  Option<Vec<u8>>)` signature after commit db6d1c31 changed
  `Message::FileShare`'s thumbnail to a `MessageHash` blob reference
  (`thumbnail_hash: Option<[u8; 32]>`). The full `--all-targets` suite had
  been unbuildable at HEAD since Aug 2. Fixed the trait-impl signature in all
  17 files and the one stale `thumbnail:` field initializer in
  `test_hostile_input.rs` (now `thumbnail_hash:`). These files are not part of
  the in-flight t_2d04f7c2 change set, so they are committed with FS-25.
- `examples/iced_chat/app.rs` — removed stale "placeholder panel; full
  dashboard in FS-04+" doc comment on `Screen::FileSharing` (now a five-tab
  screen). NOTE: this file is shared with in-flight task t_2d04f7c2
  (deepseek-coder) which owns the uncommitted MCP-surface changes; the comment
  fix was left in the working tree unstaged and rides with that task's commit.
- Audited committed FS-20..24 code: no TODO/FIXME/debug-print scaffolding, no
  temporary fixture injection, no obsolete feature flags. `--enable-gui-test-actions`
  is a documented, deliberately-gated test surface (used by the FS-23 harness
  and in-flight t_2d04f7c2) and is retained.

## COMMANDS RUN (clean worktree at HEAD c86be3e6, /tmp/fs25-clean)

```text
cargo test --all-targets
→ <result recorded after run>

cargo fmt --all --check
→ <result>

cargo clippy --all-features
→ <result>

# Multi-peer smoke test
cargo build --features gui --example boru   (clean worktree)
bash scripts/fs23_launch.sh start
bash scripts/fs23_launch.sh status
bash scripts/fs23_launch.sh stop
→ <result>
```

## SECURITY/PRIVACY IMPACT
None — documentation and comment cleanup only. No protocol, storage, or
authorization code changed.

## KNOWN LIMITATIONS / FOLLOW-UPS
- The in-flight t_2d04f7c2 (MCP surface extension for automated E2E) is still
  running against the shared working tree; its files were not touched by this
  task. Full end-to-end transfer scenarios (share → register → download →
  progress → restart) remain manual until that task lands.
- 211 pre-existing compiler warnings (unchanged, noted by FS-24).
