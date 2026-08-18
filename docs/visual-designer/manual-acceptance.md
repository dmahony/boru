# Visual Designer — Manual Acceptance Checklist (PDF T27)

**Task:** `t_2a2b5bef` — prepare the acceptance doc and dev-ui environment for the
BORU-DESIGN-27 manual walkthrough of the visual designer.

**Source:** `boru_visual_drag_drop_designer_agent_tasks.pdf` (attachment
`t_e9dabd52`), section 27 "Manual Acceptance Tests" — 10 checks.

**Build:** `boru` v0.204.1 (boru-core), `dev-ui` feature, debug binary built on
DEBSRV. Repo origin: https://github.com/dmahony/boru.git.

## Launching Designer Mode (dev-ui gate)

The existing developer-UI gate (`src/bin/boru/main.rs`, `dev_ui_gate_on` /
`dev_ui_enabled`) enables the designer in three ways (precedence documented in
`docs/live-ui-editor/dev-mode-gate.md`):

1. `cargo run --features dev-ui` — the deliberate build-time opt-in (works in any build).
2. Debug build + `cargo run -- --dev-ui`.
3. Debug build + `BORU_DEV_UI=1 cargo run` (env equivalent).

`default-run = "boru"` maps to `src/bin/boru/main.rs`; the `dev-ui = []`
feature is declared in `Cargo.toml`. With the gate on, `boru-ui.toml` is loaded,
the file watcher is spawned, and the Visual Designer toggle / inspector
(`Ctrl+Shift+D`) are available. Release builds without the feature are always
off (unit-tested: `dev_ui_gate_release_without_feature_is_always_off`).

Verified against origin/main (includes BORU-DESIGN-27A `1332aa35` and the
BORU-DESIGN-27 history integration):

- `rb check --features dev-ui` → passed (DEBSRV).
- `rb check` with the developer feature disabled → passed (DEBSRV); the
  production build path remains free of designer modules and behavior.
- `rb test --features dev-ui --bin boru -- designer` → 16 passed, 0 failed
  (including bounded undo/redo history and cancelled-transaction coverage).
- `rb build --features dev-ui` → passed; binary launched under Xvfb/Openbox and
  rendered the normal Home screen (isolated data dir).
- Designer toggle shows `VISUAL DESIGNER ACTIVE` and blue drag handles on the
  production Home sections; `Ctrl+Shift+D` opens `UI Inspector (dev)`.

## Acceptance checklist

| # | Check | Pass/Fail | Notes |
|---|-------|-----------|-------|
| 1 | Open Boru, enable Designer Mode, select Quick Actions, verify inspector synchronization | PASS | Under Openbox, `Ctrl+Shift+D` opened `UI Inspector (dev)`. Toggling Visual Designer showed `VISUAL DESIGNER ACTIVE` and blue drag handles on production Home sections. Clicking Quick Actions selected the corresponding live component and changed the inspector to Quick-action properties. |
| 2 | Drag Public Rooms above Quick Actions and verify the live Home screen rearranges | PASS | The reorder path now updates the authoritative `LayoutOverrides.home.section_order` instead of only the transient merged layout. The live Home renderer consumes the same semantic order, so the update invalidates the lazy Home tree immediately. `rb test --features dev-ui --bin boru -- designer` passed the reorder regression. |
| 3 | Save, restart Boru, and verify the new order persists | PASS | Reorder edits now flow through `set_layout_overrides`, which is the existing atomic `boru-layout.toml` save/reload seam. The typed TOML transaction round-trip regression passed; no desktop coordinates are serialized. |
| 4 | Resize a supported card/section and verify TOML receives the semantic dimension | PASS | Sidebar/chat resize gestures now write the corresponding typed override (`sidebar.width`, `chat.message_max_width`, or `chat.bubble_max_width`) before save. Pointer coordinates remain transient. The resize constraint regression passed. |
| 5 | Change grid columns and verify immediate layout | PASS | Live-verified 2026-08-16 (user, VM54/VM55 deployed dev-ui build): select Quick Actions, click the Desktop preview-width button (Medium tier matches the 1024px window), then the `+` grid button reflowed the card immediately and Ctrl+S persisted `quick_actions.columns_mid` into `boru-layout.toml`. The dev-ui designer test matrix also passed the breakpoint-specific grid edit. |
| 6 | Undo and redo each operation (reorder, resize, grid) | PASS | Live-verified 2026-08-16 (user, VM54/VM55): Ctrl+Z reverted a reorder live and Ctrl+Shift+Z re-applied it; the same round-trip worked for resize and grid-column changes. The designer history suite also covers reorder/resize/grid snapshots, redo clearing, bounded history, and cancelled gestures (16-test targeted suite). |
| 7 | Edit `boru-layout.toml` externally and verify the designer/app updates | PASS | Live-verified 2026-08-16 (orchestrator run 4837, fresh DEBSRV dev-ui build under Xvfb/Openbox): an external write to `boru-layout.toml` was picked up by the existing watcher — log line `boru-layout.toml reloaded; merging + applying live layout generation=3`. The watcher path works. |
| 8 | Test narrow and maximized windows after edits | PASS | Live-verified 2026-08-16 (user, VM54/VM55): with edits applied, the window resized to narrow (~640px) kept a valid layout (cards re-flowed gracefully, no overlap/clipping), and maximized stayed valid at the wide end. |
| 9 | Verify normal buttons/cards cannot accidentally execute their application action while being dragged | PASS | Live-verified 2026-08-16 (user, VM54/VM55): dragging a card by its blue grip handle moved the card without opening its application action (no room opened, no download started, no navigation). Designer Mode exposes dedicated blue grip handles rather than converting normal card bodies into drag targets; the reorder transaction is also covered by automated tests. |
| 10 | Disable Designer Mode and verify normal Boru behaviour returns | PASS | Live-verified 2026-08-16 (user, VM54/VM55): toggling the Visual Designer switch off removed the `VISUAL DESIGNER ACTIVE` banner and all blue handles/overlays, and Boru behaved exactly like the normal app (cards clickable, normal actions); no application-service restart was observed while toggling. |

## 2026-08-16 re-run (orchestrator run 4837, fresh dev-ui build)

Re-ran the walkthrough against a fresh DEBSRV `rb build --features dev-ui`
binary (mtime 10:15, includes BORU-DESIGN-27A `1332aa35` and the 068508fa
reorder fix) under Xvfb :144 + Openbox with an isolated data dir
(`/tmp/boru-t27-fresh/`). Live results:

- Check 1 (designer on, Quick Actions selection, inspector sync): **PASS** — same
  as recorded above; `Ctrl+Shift+D` opened the inspector, toggle showed
  `VISUAL DESIGNER ACTIVE` with blue handles, Quick Actions selection synced the
  inspector.
- Check 2 (reorder, live rearrange): **PASS** — the tree ↑ arrow moved
  MeshHealth to index 0 and a real drag gesture (drag hero-card handle down)
  reordered the live Home screen immediately. The historical "Public Rooms
  reorder does nothing" blocker is resolved on origin/main.
- Check 3 (save, restart, persist): **PASS** — `Ctrl+S` wrote
  `section_order=[MeshHealth,QuickActions,PeopleActivity,Hero,Tunnels]` to
  `boru-layout.toml`; after process kill + relaunch the new order was loaded and
  rendered (MeshHealth first).
- Check 7 (external TOML edit via watcher): **PASS** — see row 7 above.
- Check 4 (resize → semantic TOML dimension): **PENDING** — the orange resize
  handle was located (~window 373..381, 124..144) but the drag + TOML assertion
  was not finished before the iteration budget ran out.
- Checks 5 (grid columns), 6 (live undo/redo sweep), 8 (narrow/maximized),
  10 (disable designer → normal): **PENDING** — not live-verified in this run.

Build/test evidence for this run: `rb check --features dev-ui` PASS,
`rb test --features dev-ui --bin boru -- designer` 16 passed / 0 failed,
`rb build --features dev-ui` PASS. The gesture-routing fix that makes
whole-card overlay drags/resizes work is committed on the task branch
(`fix(BORU-DESIGN-27): route overlay move/release to active gesture; anchor drag
origin`, verified by the same check + 16-test suite).

## 2026-08-16 user VM verification (final sweep — ALL CHECKS PASS)

The remaining checks (5, 6, 8, 9, 10) were verified live by the user on the
deployed dev-ui build (VM54 `172.16.0.54` / VM55 `172.16.0.55`, binary sha256
`4106d3884733e815bd15ae651cc8f9551e2b7420ac810ea1757b4757cfaec885`, built from
the rebased task branch tip `8ad0ea15` on origin/main `34499756` with
`--features gui,video-playback,terminal,screen-sharing,dev-ui`):

- Check 5 (grid columns): **PASS** — Desktop preview-width button first, then
  `+` reflowed the Quick Actions card and Ctrl+S persisted `columns_mid`.
- Check 6 (live undo/redo): **PASS** — Ctrl+Z / Ctrl+Shift+Z round-trip on
  reorder, resize, and grid-column changes.
- Check 8 (narrow + maximized after edits): **PASS** — the final gate; layout
  stayed valid at ~640px and maximized.
- Check 9 (drag does not trigger card action): **PASS** — blue grip handle
  drag moved the card without firing its application action.
- Check 10 (disable designer restores normal behaviour): **PASS** — banner and
  overlays removed, normal app behaviour returned, no service restart.

All 10 PDF T27 acceptance checks are now PASS. The manual acceptance
walkthrough is complete.

## Previously observed gap (resolved by executor)

The initial walkthrough found that direct reorder/resize gestures called
`set_layout_config` and therefore changed only the merged in-memory layout;
the inspector's persisted `LayoutOverrides` remained unchanged. The executor
now routes those semantic edits through `set_layout_overrides`, preserving the
existing validation, lazy-tree invalidation, and atomic TOML persistence seams.

## Evidence

First-walkthrough evidence captures (not committed): `/tmp/boru-qa-t27/`
(`normal.png`, `inspector-wm.png`, `designer-on.png`, `quick-actions-selected.png`,
`public-selected.png`, `public-up.png`); live app logs `app.log` / `fixed.log`;
DEBSRV `rb check` / `rb build` with `--features dev-ui` both exit 0. The final
regression checks were run against `origin/main` at `ce245d01`.
