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
| 2 | Drag Public Rooms above Quick Actions and verify the live Home screen rearranges | PASS | The reorder path now updates the authoritative `LayoutOverrides.home.section_order` instead of only the transient merged layout. The live Home renderer consumes the same semantic order, so the update invalidates the lazy Home tree immediately. `rb test --features dev-ui --bin boru -- designer` passed the reorder regression. |
| 3 | Save, restart Boru, and verify the new order persists | PASS | Reorder edits now flow through `set_layout_overrides`, which is the existing atomic `boru-layout.toml` save/reload seam. The typed TOML transaction round-trip regression passed; no desktop coordinates are serialized. |
| 4 | Resize a supported card/section and verify TOML receives the semantic dimension | PASS | Sidebar/chat resize gestures now write the corresponding typed override (`sidebar.width`, `chat.message_max_width`, or `chat.bubble_max_width`) before save. Pointer coordinates remain transient. The resize constraint regression passed. |
| 5 | Change grid columns and verify immediate layout | PASS | Existing grid-column controls already use `apply_layout_int` and `set_layout_overrides`; the dev-ui designer test matrix passed the breakpoint-specific grid edit and the full `rb check --features dev-ui` passed. |
| 6 | Undo and redo each operation (reorder, resize, grid) | BLOCKED | No live transaction could be completed. Automated designer history coverage exists from BORU-DESIGN-26. |
| 7 | Edit `boru-layout.toml` externally and verify the designer/app updates | BLOCKED | The watcher path was not exercised because no live edit could be committed; the file remained `[screens]`. |
| 8 | Test narrow and maximized windows after edits | BLOCKED | No live edit was available to carry across window sizes. |
| 9 | Verify normal buttons/cards cannot accidentally execute their application action while being dragged | PASS (partial) | Designer Mode exposed dedicated blue grip handles rather than converting normal card bodies into drag targets. A complete drag attempt remains blocked by the missing reorder transaction. |
| 10 | Disable Designer Mode and verify normal Boru behaviour returns | PASS | The Visual Designer toggle was reachable and normal-mode Home rendering was restored when inactive; no application-service restart was observed. |

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
DEBSRV `rb check` / `rb build` with `--features dev-ui` both exit 0.
