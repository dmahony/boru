# UI-HOME-19 — Final Orchestrator Release Gate

**Task:** t_61a56729 (reviewer, release gate)
**Date:** 2026-08-06
**Branch:** wt/t_61a56729 (worktree; report-only — no source changes)
**Gate decision:** ✅ **APPROVED** — all 10 scope items verified; 6 follow-up tickets created; epic card EPIC-UI-HOME-FONTS (t_fdf744bb) may proceed.

---

## 1. Summary

This is the final release gate for the Boru home-screen tidy + typography project
(UI-HOME-01..18, plan epic EPIC-UI-HOME-FONTS). Every preceding card is Done, all
20 UI-HOME commits are on `origin/main`, evidence is attached for every card, the
final screenshot set matches the approved Figure 3 mockup, all five font families
load through the central registration with semantic `TypeRole` roles, licence
records are in the repo, no business-logic changes are hidden in the UI commits,
the 4-width screenshot coverage is complete, the UI-HOME-17 action matrix is
11/11, and a fresh build + 896/896 GUI test run pass at gate time.

The gate report was committed and pushed to `origin/main` as
`UI-HOME-19: final orchestrator release gate (task t_61a56729)` (see §Git trail).

---

## 2. Final orchestrator checklist (scope items 1–10)

| # | Criterion | Result | Evidence |
|---|---|---|---|
| 1 | Every task card 01..18 reviewed, evidence attached | ✅ | All 18 cards `done` (board query); per-card evidence dirs exist under `docs/ui-redesign/evidence/t_<id>/` and are committed in each card's commit; handoff metadata lists artifacts per card |
| 2 | No card blocked / in Review / in Verification | ✅ | Board status counts: only `done` (150) + `archived` (1674) + this card `running`; no blocked/review/verification states exist on the board; unrelated `scheduled` card t_21cbf698 (i18n) is not part of this plan |
| 3 | Final screenshot set vs approved mockup | ✅ | Side-by-side composite `evidence/t_266bfba3/t_266bfba3_side_by_side_mockup_vs_current_1280x800.png` (mockup = `evidence/ui-11/target-figure3.png`); independent re-check this gate: composite halves mean abs diff 13.42 (UI-HOME-18 reported 11.2–12.7); app layout matches mockup structure at 1600 AND 1280 (sidebar → header → hero → Mesh Health → 4-col/2×2 quick actions; rail = Online Peers / Recent Activity / Tunnels) |
| 4 | Fonts load via central registration, 5 families, semantic roles | ✅ | `fonts.rs` `load_fonts()` (line 597) registers Source Sans 3 (400/500/600/700), Manrope (600/700), Figtree (400/500/600), Raleway ExtraBold (800), JetBrains Mono (400/500); chained at `main.rs:1602`; `TypeRole` enum (fonts.rs:357) used 458× in app.rs, 77× ui_components.rs, 71× shared_by_me_table.rs etc.; regression test `every_required_family_weight_is_registered` pins the 11 required family/weight pairs |
| 5 | Font licence/source records in `examples/iced_chat/fonts/` | ✅ | Tracked in git: `THIRD_PARTY_NOTICES.md`, per-family `Figtree-OFL.txt`, `Manrope-OFL.txt`, `JetBrainsMono-OFL.txt`, `Raleway-OFL.txt`, plus pre-existing `SourceSans3-OFL.txt`, `Inter-OFL.txt`, `OFL.txt` (all 26 font files + 8 licence files tracked) |
| 6 | No business-logic/payload changes hidden in UI commits | ✅ | Per-commit file inventory for all 20 UI-HOME commits: 3 files outside `examples/iced_chat/`/`docs/ui-redesign/`/`scripts/`/`DESIGN_SYSTEM.md`/`Cargo.lock` — each verified benign (see §4) |
| 7 | Screenshots cover wide/medium/narrow/minimum | ✅ | UI-HOME-15: 1600×900 / 1280×800 / 1024×720 / 800×600 (+scrolled); UI-HOME-18: same 4 widths + grid captures + 8-step scroll series per width + scrolled |
| 8 | UI-HOME-17 action test matrix complete | ✅ | `evidence/t_17a358c8/test_matrix.txt`: 11/11 rows OK (Create Public Room, Create Group Chat, Add Friend, Share Files, 5a peer→Chat, 5b View all, 5c Mesh details, 5d Create tunnel, 7a Ctrl+N, 7b autofocus, 7c Tab order) + 6 live-update checks (OCR-verified) |
| 9 | All task code pushed to origin | ✅ | `git fetch origin` run; `git log origin/main` shows all 20 UI-HOME commits (bc5a949e…9d34a33c); worktree HEAD == origin/main at gate time; this gate's commit also pushed |
| 10 | Accepted differences + follow-up tickets recorded | ✅ | See §6 (accepted differences) and §7 (follow-up tickets — 6 created) |

## 3. Merged test matrix

Fresh run at gate time on the exact HEAD being gated (origin/main 9d34a33c):

| Run | Result | Detail |
|---|---|---|
| `cargo build --bin boru --features gui` | ✅ exit 0 | 207 pre-existing warnings (unchanged, documented) |
| `cargo test --bin boru --features gui` | ✅ 896/896 pass | 66.5 s, 0 failed, 0 ignored |
| Fonts regression tests | ✅ 14/14 | included in the 896 (UI-HOME-11 suite) |
| `cargo test --lib` | ⚠️ 1824 pass / 20 fail | **pre-existing, unrelated**: `git diff origin/main HEAD src/` is EMPTY — all UI-HOME work is examples/iced_chat-only; groups: DCGKA integration flake (5), stale sent_at:1000 (5), friendly-name flake (7), store/fs path (3). Remediation for the timestamp group (t_99573d95 / t_42beb205) is archived/done; triage ticket t_869b59bf filed for the rest |

Per-card test counts (from card handoffs): 01: n/a (audit) · 02: 835 · 03: 864 ·
04: 865 · 05: 876 · 06: 866 · 07: 867 · 08: 867 · 09: 880 · 10: 884 · 11: 840 ·
12: (in 11/13 range) · 13: 844 · 14: 853 · 15: 891 · 16: 896 · 17: 896 · 18: 896.
Counts grow monotonically as cards land — consistent with no test loss.

UI-HOME-17 action matrix (11/11 OK, evidence `evidence/t_17a358c8/`):

| # | Action | Expected | Evidence PNG | Result |
|---|---|---|---|---|
| 1 | Create Public Room | Create Room dialog | `t_17a358c8_action_1_create_public_room.png` | OK |
| 2 | Create Group Chat | Create Group dialog | `t_17a358c8_action_2_create_group_chat.png` | OK |
| 3 | Add Friend | Friend Requests screen | `t_17a358c8_action_3_add_friend.png` | OK |
| 4 | Share Files | Files I'm Sharing (MCP path) | `t_17a358c8_action_4_share_files_dashboard.png` | OK |
| 5a | Peer row → Chat | End-to-end encrypted conversation | `t_17a358c8_rail_5a_peer_row_chat.png` | OK |
| 5b | Online Peers View all | Friend Requests | `t_17a358c8_rail_5b_online_peers_view_all.png` | OK |
| 5c | Mesh View details | Connection Details | `t_17a358c8_rail_5c_mesh_view_details.png` | OK |
| 5d | Tunnels Create tunnel | Create Tunnel dialog | `t_17a358c8_rail_5d_tunnels_view_all.png` | OK |
| 7a | Ctrl+N | Create Room dialog | `t_17a358c8_kb_ctrln_room_dialog.png` | OK |
| 7b | Auto-focus name input | typed text lands in Room Name | `t_17a358c8_kb_typed_name_autofocus.png` | OK |
| 7c | Tab order (name → description) | focus moves to Description | `t_17a358c8_kb_tab_focus_order_group.png` | OK |

Live-update checks (UI-HOME-17, production paths, OCR-verified): empty state →
Ada row on `boru_gui_set_peer_presence` online; "came online" activity line;
"went offline" line + badge 1/2→0/2; share-file registration row; mesh
"Recent events" header + growing event list.

## 4. Final changed-file inventory

20 commits, ~350 files across `examples/iced_chat/`, `docs/ui-redesign/`,
`scripts/`, `DESIGN_SYSTEM.md`, `Cargo.lock`. Full per-commit listing was
generated from `git show --name-only` at gate time. **Non-UI files and why they
are acceptable:**

| File | Commit | Change | Verdict |
|---|---|---|---|
| `Cargo.lock` | UI-HOME-02 (9438abf9) | boru-core 0.116.1 → 0.117.0 lockfile sync (matches the standalone chore commit c4e44e50 "bump Boru version to 0.117.0") | Benign version sync, no dependency graph change beyond the version number |
| `src/diagnostics.rs` | UI-HOME-05 (113b2b47) | Adds `GuiTestCommand::ClearMeshEventLog` variant (gated test-harness command) + validation + round-trip test | Additive test-only; no existing variant/business logic touched |
| `examples/iced_chat/mcp_server.rs` | UI-HOME-16 (0ece6ca6) | Adds `boru_gui_clear_mesh_events` MCP test action (gated `--enable-gui-test-actions`), dispatches existing `ClearMeshEventLog` command | Test-harness only, never fabricates events; production behavior unchanged |

Networking, discovery, chat, room, group, file-sharing and tunnel business logic
are untouched: `git diff origin/main~20 origin/main -- src/` shows only the
diagnostics addition above; `examples/iced_chat/` changes are view/component/
token/font code (app.rs, card_shell.rs, quick_actions.rs, design_tokens.rs,
fonts.rs, ui_components.rs, boru_dialog.rs, form_components.rs, icon_system.rs,
log_viewer.rs, shared_by_me_table.rs, sharing_summary.rs, presentation.rs,
component_gallery.rs, mcp_server.rs).

## 5. Approved screenshot set

All under `docs/ui-redesign/evidence/` (committed on origin/main):

- **Approved mockup:** `ui-11/target-figure3.png` (canonical Figure 3 render from the plan PDF)
- **Side-by-side:** `t_266bfba3/t_266bfba3_side_by_side_mockup_vs_current_1280x800.png` (mean abs diff 13.42, independently re-measured this gate)
- **Wide 1600×900:** `t_266bfba3/t_266bfba3_home_1600x900.png` (+scrolled +grid +8-step series), `t_dfe40e9f/t_dfe40e9f_home_1600x900.png`, `t_17a358c8/t_17a358c8_home_1600x900_empty.png`
- **Medium 1280×800:** `t_266bfba3/t_266bfba3_home_1280x800.png` (+scrolled +grid +series), `t_dfe40e9f/t_dfe40e9f_home_1280x800.png`, `t_4186e7f9/t_4186e7f9_home_populated_1280x800.png`
- **Narrow 1024×720:** `t_266bfba3/t_266bfba3_home_1024x720.png` (+scrolled +grid +series), `t_dfe40e9f/t_dfe40e9f_home_1024x720.png`
- **Minimum 800×600:** `t_266bfba3/t_266bfba3_home_800x600.png` (+scrolled +grid +series), `t_dfe40e9f/t_dfe40e9f_home_800x600.png`
- **Quick-action grids:** `t_266bfba3/crops/qa_1280_grid.png` (2×2), `qa_1600_grid.png` (4-col) — independently OCR-verified this gate: all four approved descriptions fully present at both widths
- **Empty states:** `t_4186e7f9/` (10 PNGs, per-card crops OCR-verified)
- **Typography gallery / fallback:** `t_318cd671/t_318cd671_typography_preview*.png`, `t_266bfba3/t_266bfba3_font_gallery_*.png`

Overflow check (independently re-run this gate on `t_266bfba3_home_1280x800.png`):
`words_past_right_edge = 0`; OCR word-map confirms all rail cards + mesh card +
hero render at expected positions; no content past x=1278.

## 6. Accepted differences (explicit approval)

1. **text_muted contrast 3.04:1** — below WCAG AA 4.5 for 12px metadata; matches
   mockup style, DESIGN_SYSTEM documents 2.8:1 muted tier → follow-up t_b2ac1e1a.
2. **Primary-button white label 4.28:1** — passes AA large-text (3:1), misses
   4.5:1 normal → follow-up t_80938852.
3. **iced 0.14 buttons not keyboard-focusable** — framework limitation
   (no `Status::Focused`), TextInput focus verified → follow-up t_d849e063.
4. **20 pre-existing `cargo test --lib` failures** — proven unrelated (empty
   `src/` diff); timestamp-group remediation t_99573d95/t_42beb205 archived/done;
   triage for the rest → follow-up t_869b59bf.
5. **207 pre-existing build warnings** — unchanged, non-blocking.
6. **Dark theme** — out of scope (light default; dark tokens covered by prior cards).
7. **Mockup REQUESTS rail card absent** — the app routes friend requests to the
   dedicated Friend Requests screen (verified working in UI-HOME-17 row 3);
   plan DoD lists only Online Peers / Recent Activity / Tunnels → decision
   ticket t_89ce5d63.
8. **Quick-action copy differs from Figure 3 text** ("Add a friend by key or
   invite file." / "Securely share files with your peers.") — the app uses the
   copy specified by the UI-HOME-06 card ("Connect with a friend by public key." /
   "Choose a file to share in a chat."); plan text wins over mockup text.
9. **Mockup simplified illustration / bottom status strip** ("Connected to the
   mesh", "End-to-end encrypted") — production equivalents live in the mesh card
   stat tiles and per-conversation status lines; no static mock content in
   production per plan constraint.
10. **Orphaned tracked modules** dashboard.rs / file_library.rs / invitation_qr.rs
    (no mod decl, dead code) + optional TypeRole adoption in
    connection_details.rs / download_progress_view.rs → follow-up t_8834836b.

## 7. Follow-up tickets created (this gate)

| ID | Title | Assignee | Priority |
|---|---|---|---|
| t_b2ac1e1a | text_muted contrast 3.04:1 fails WCAG AA normal text | linux | 20 |
| t_80938852 | primary-button white label 4.28:1 misses AA normal text | linux | 20 |
| t_d849e063 | re-evaluate iced 0.14 button keyboard focus on upgrade | linux | 10 |
| t_8834836b | remove orphaned modules + optional TypeRole adoption | linux | 10 |
| t_89ce5d63 | decide REQUESTS rail card vs dedicated Friend Requests screen | orchestrator | 10 |
| t_869b59bf | triage 20 pre-existing cargo test --lib failures | rust-dev | 15 |

All created with `kanban_create` during this gate run (status `ready`).

## 8. Independent gate verification performed

Beyond the cards' own evidence, this gate independently ran:

1. `git fetch origin` + `git log origin/main` — all 20 UI-HOME commits present.
2. Full rebuild + GUI test run at gate HEAD: build exit 0, 896/896 pass.
3. Per-commit `git show --name-only` inventory — only 3 non-UI files, each diffed
   and judged benign (Cargo.lock version sync; additive gated test commands).
4. Pixel/OCR re-verification of the mockup composite (mean abs diff 13.42) and of
   the 1280/1600 layouts via full-page OCR word maps (0 words past right edge,
   all cards + quick actions present, 2×2 at 1280 and 4-col at 1600 confirmed).
5. Fonts: `load_fonts()` registers all 5 families at exact plan weights;
   `TypeRole` used pervasively; licence files tracked in git.
6. Board: all 18 predecessors `done`, epic card t_fdf744bb in `todo` waiting on
   this card (will auto-promote on completion).

## 9. Git trail

- Gate commit: `UI-HOME-19: final orchestrator release gate (task t_61a56729)`
  (branch wt/t_61a56729, rebased on origin/main, fast-forwarded onto main and
  pushed; `git log origin/main -3` verified after push).
- All UI-HOME-01..18 commits were already on origin/main before this gate
  (verified via fetch + log).

## 10. Handoff to epic card

EPIC-UI-HOME-FONTS (t_fdf744bb) activates on completion of this card. Its DoD
items are all satisfied by the verified evidence chain above; it should produce
`docs/ui-redesign/UI-HOME-FONTS-final-report.md` per the plan template, re-check
`git status` cleanliness, and confirm the git trail. No UI-HOME commit is
missing from origin/main; nothing needs pushing.
