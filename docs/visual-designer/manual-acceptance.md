# Visual Designer — Manual Acceptance Checklist (PDF T27)

**Task:** `t_2a2b5bef` — prepare the acceptance doc and dev-ui environment for the
BORU-DESIGN-27 manual walkthrough of the visual designer.

**Source:** `boru_visual_drag_drop_designer_agent_tasks.pdf` (attachment
`t_e9dabd52`), section 27 "Manual Acceptance Tests" — 10 checks.

**Build:** `boru` v0.204.0 (boru-core), `dev-ui` feature, debug binary built on
DEBSRV. Repo origin: https://github.com/dmahony/boru.git.

## Launching Designer Mode (dev-ui gate)

The existing developer-UI gate (`examples/iced_chat/main.rs`, `dev_ui_gate_on` /
`dev_ui_enabled`) enables the designer in three ways (precedence documented in
`docs/live-ui-editor/dev-mode-gate.md`):

1. `cargo run --features dev-ui` — the deliberate build-time opt-in (works in any build).
2. Debug build + `cargo run -- --dev-ui`.
3. Debug build + `BORU_DEV_UI=1 cargo run` (env equivalent).

`default-run = "boru"` maps to `examples/iced_chat/main.rs`; the `dev-ui = []`
feature is declared in `Cargo.toml`. With the gate on, `boru-ui.toml` is loaded,
the file watcher is spawned, and the Visual Designer toggle / inspector
(`Ctrl+Shift+D`) are available. Release builds without the feature are always
off (unit-tested: `dev_ui_gate_release_without_feature_is_always_off`).

Verified against origin/main (includes BORU-DESIGN-27A `1332aa35`):

- `rb check --features dev-ui` → passed (DEBSRV).
- `rb build --features dev-ui` → passed; binary launched under Xvfb/Openbox and
  rendered the normal Home screen (isolated data dir).
- Designer toggle shows `VISUAL DESIGNER ACTIVE` and blue drag handles on the
  production Home sections; `Ctrl+Shift+D` opens `UI Inspector (dev)`.

## Acceptance checklist

| # | Check | Pass/Fail | Notes |
|---|-------|-----------|-------|
| 1 | Open Boru, enable Designer Mode, select Quick Actions, verify inspector synchronization | PASS | Under Openbox, `Ctrl+Shift+D` opened `UI Inspector (dev)`. Toggling Visual Designer showed `VISUAL DESIGNER ACTIVE` and blue drag handles on production Home sections. Clicking Quick Actions selected the corresponding live component and changed the inspector to Quick-action properties. |
| 2 | Drag Public Rooms above Quick Actions and verify the live Home screen rearranges | BLOCKED | Production overlays and handles are present, but the Public Rooms up-arrow in the component tree did not change the order after repeated targeted clicks. The Public Rooms live card is not present in the empty-room Home content, so a direct handle drag could not be performed. |
| 3 | Save, restart Boru, and verify the new order persists | BLOCKED | No reorder transaction was created; `Ctrl+S` left the isolated data dir's `boru-layout.toml` at `[screens]`. Persistence of an actual reorder therefore could not be tested. |
| 4 | Resize a supported card/section and verify TOML receives the semantic dimension | BLOCKED | Supported resize controls are visible after selecting Quick Actions, but the live Xvfb interaction became unreliable (`XTEST BadValue`) before a slider edit could be committed and verified in TOML. |
| 5 | Change grid columns and verify immediate layout | BLOCKED | No committed layout edit was available; blocked by the same interaction/reorder path. |
| 6 | Undo and redo each operation (reorder, resize, grid) | BLOCKED | No live transaction could be completed. Automated designer history coverage exists from BORU-DESIGN-26. |
| 7 | Edit `boru-layout.toml` externally and verify the designer/app updates | BLOCKED | The watcher path was not exercised because no live edit could be committed; the file remained `[screens]`. |
| 8 | Test narrow and maximized windows after edits | BLOCKED | No live edit was available to carry across window sizes. |
| 9 | Verify normal buttons/cards cannot accidentally execute their application action while being dragged | PASS (partial) | Designer Mode exposed dedicated blue grip handles rather than converting normal card bodies into drag targets. A complete drag attempt remains blocked by the missing reorder transaction. |
| 10 | Disable Designer Mode and verify normal Boru behaviour returns | PASS | The Visual Designer toggle was reachable and normal-mode Home rendering was restored when inactive; no application-service restart was observed. |

## Blocking gap (known, tracked by the executor tasks)

The reorder control on the component tree does not mutate the visible section
order on a fresh DEBSRV build, so no semantic layout transaction can be created;
the dependent save/restart, resize persistence, grid, undo/redo, watcher, and
responsive-after-edit checks (2–8) cannot be completed until the reorder path
lands a transaction. This is the responsibility of the executor tasks
(`t_ecc0a5b0` checks 1–5, `t_721479eb` checks 6–10, `t_4ea0c6f4` final
regression), which will update the Pass/Fail column as fixes land.

## Evidence

First-walkthrough evidence captures (not committed): `/tmp/boru-qa-t27/`
(`normal.png`, `inspector-wm.png`, `designer-on.png`, `quick-actions-selected.png`,
`public-selected.png`, `public-up.png`); live app logs `app.log` / `fixed.log`;
DEBSRV `rb check` / `rb build` with `--features dev-ui` both exit 0.
