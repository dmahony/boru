# UI-RESTYLE-08/09/10 Merge Decision

Date: 2026-08-05
Task: t_8dd35cdc (orchestrator decision)
Status: **DECISION: MERGE (option a)**

## Context

UI-RESTYLE-13 regression check (docs/ui-restyle-13-regression-check.md, commit
c236e8c1 on `wt/t_8c4e8ebe`, Finding F2) discovered a merge-state gap:

- Board shows UI-RESTYLE-08/09/10 as done, but their commits are NOT in the
  `main` lineage (HEAD e8c8676a = UI-RESTYLE-02..07 via merge 78c0c450 only).
- The duplication reduction verified by UI-RESTYLE-11/12 exists only on the
  task branches, not in shipped `main`.

The gap was captured as board task t_bc83f838 and decomposed; this document
records the orchestrator decision between two options:

- (a) Merge the missing commits into `main` using the review-merge pattern.
- (b) Accept `main` = UI-RESTYLE-02..07 only and close 08/09/10 as superseded.

## Evidence gathered

### Missing commits (verified absent from main lineage)

| Commit | Task | What it adds |
|---|---|---|
| `146c366b` | UI-RESTYLE-09 | Refactor: shared `SelectablePeerList` + `messageable_friends()` helper; converts `view_invite_member_dialog` off raw iced scaffolding. 3 files, +198/-120. |
| `ce58d080` | UI-RESTYLE-10 | docs/ui-restyle-10-entry-points.md (80 lines): entry-point audit for restyled dialogs. |
| `646c4a60` | UI-RESTYLE-08 | DESIGN_SYSTEM.md (+48/-10): documents BoruDialog creation dialogs. |
| `f5fa6392` | UI-RESTYLE-11 | docs/ui-restyle-11-verification.md (129 lines): functional verification report. |
| `c236e8c1` | UI-RESTYLE-13 | docs/ui-restyle-13-regression-check.md (187 lines): regression check report (this decision's reference). |

`git log main..wt/t_2da7ff1a`, `main..wt/t_25e03dbe`, `main..wt/t_98959dff`,
`main..wt/t_9827c02b` confirm all five commits are absent from the main
lineage. `DESIGN_SYSTEM.md` exists on main, so `646c4a60` is a doc update, not
a new file. The 04/05/06 variants listed on those branches are duplicate
instances of changes already present in main (02-07 chain); they do not need
re-cherry-picking.

### Merge feasibility (trial performed 2026-08-05)

A trial worktree at current `origin/main` (393435c9) cherry-picked all five
commits in order: `146c366b` → `ce58d080` → `646c4a60` → `f5fa6392` →
`c236e8c1`.

- Only ONE conflict occurred, in `146c366b` (`view_create_tunnel_dialog`'s
  `use` block): main imports `BORU_DIALOG_WIDTH_STANDARD` + `peer_list`,
  incoming imports `SelectablePeerList`. Resolution keeps
  `BORU_DIALOG_WIDTH_STANDARD` (still used by the dialog body) and drops
  `peer_list` in favour of `SelectablePeerList` + `SelectablePeerRow`.
- The other four commits applied cleanly (docs only, no overlap with main).
- Post-merge compile: `cargo check --bin boru --features gui` → Finished
  dev profile, 0 errors (217 pre-existing warnings, unchanged). 5m24s.

## Rationale — why merge (option a)

1. **The missing work is real and verified.** UI-RESTYLE-09 is a genuine
   duplication reduction (a third dialog — invite-member — was still on raw
   iced scaffolding in main; the shared `SelectablePeerList` /
   `messageable_friends()` work was functionally and visually verified by
   UI-RESTYLE-11/12/13 on the task branches, with no regressions reported).
2. **The docs close the audit trail.** UI-RESTYLE-08 (DESIGN_SYSTEM.md),
   UI-RESTYLE-10 (entry-point audit), UI-RESTYLE-11 (verification report) and
   UI-RESTYLE-13 (regression-check report) document the restyle workstream;
   leaving them off main makes the shipped tree inconsistent with the board
   and with the verification evidence.
3. **Merge risk is low.** Trial cherry-pick onto current main produced exactly
   one trivial import conflict, resolved in minutes; the merged tree compiles.
   The review-merge pattern (rust-dev prepares branch → reviewer approves →
   google-coder merges) is already wired as board tasks t_4dc5eec6 →
   t_7ca54614 → t_e479021f.
4. **Closing as superseded would be wrong on the facts.** 08/09/10 are not
   superseded by anything in main — main simply never received them. The
   alternative would leave a known duplication (parallel peer-picker dialog
   scaffolding) in the shipped code with no record of why.

## Decision

**Proceed with option (a): merge UI-RESTYLE-08/09/10 (plus the UI-RESTYLE-11
and UI-RESTYLE-13 reports) into `main` using the review-merge pattern.**

Merge task t_4dc5eec6 should cherry-pick, in order: `146c366b`, `ce58d080`,
`646c4a60`, `f5fa6392`, `c236e8c1` onto a branch from current
`origin/main` (393435c9). Resolve the single known conflict in
`view_create_tunnel_dialog`'s `use` block as described above (keep
`BORU_DIALOG_WIDTH_STANDARD`, drop `peer_list`). Do NOT re-cherry-pick the
04/05/06 variants — they are already present in main's 02-07 lineage.

After merge, UI-RESTYLE-08/09/10/11/12/13 verification claims align with the
actual main state, and Finding F2 from the regression check is resolved.
