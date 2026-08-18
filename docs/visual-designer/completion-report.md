# Boru Visual Drag-and-Drop Designer — Completion Report

**Gate:** BORU-DESIGN-29 (PDF Definition of Done)
**Verification date:** 2026-08-16
**Repository:** Boru / iroh-gossip-chat
**Verification host:** DEBSRV via `rb`; 76G free at start

## Verdict

The visual-designer Definition of Done is satisfied for the supported Home-screen scope. Boru remains an Iced application and the developer-only designer edits real Home components through the normal Iced update path. The manual acceptance record confirms a single developer-mode launch, component selection, drag reorder, semantic resize, breakpoint/grid editing, save/restart persistence, responsive layout validity, and restoration of normal application behavior when Designer Mode is disabled.

The repository-wide all-targets check is not green because four unrelated discovery/public-room test call sites still use the obsolete four-argument `DiscoveryService::join` API; the current API requires a fifth `SecretKey`. One broad `layout` test filter also has a pre-existing baseline failure in `app::tests::inspector_reset_layout_section_and_all_restore_defaults` (the test expects `Vertical`, while the current default is `Horizontal`). Neither failure is in the designer DoD path. They are recorded rather than changed under this gate.

## PDF task coverage (Tasks 1–28)

| PDF task | Delivered evidence | Result |
|---|---|---|
| 1. Designer architecture | `src/bin/boru/designer.rs`; `docs/visual-designer/designer-architecture.md`; transient `DesignerState` is separate from application/network state | PASS |
| 2. Stable component IDs | `designer.rs::ComponentId`; `docs/visual-designer/component-ids.md`; round-trip stability test | PASS |
| 3. Designer mode / developer gate | `main.rs` runtime gate and `dev-ui` feature; `docs/visual-designer/manual-acceptance.md` launch instructions | PASS |
| 4. Hover overlay | `designer.rs::overlay` and Home overlay integration in `app/home.rs`; manual active banner/handles | PASS |
| 5. Component selection | Overlay selection messages and inspector mapping; manual Quick Actions selection/inspector synchronization | PASS |
| 6. Layout metadata | `layout_metadata.rs`; stable component registry and transient bounds tests | PASS |
| 7. Drag-to-reorder | `app.rs::reorder_home_from_tree`; semantic `home.section_order` overrides; `designer_reorder_preserves_sections_and_visibility` | PASS |
| 8. Drag handles | Dedicated designer grip controls in `designer.rs`; manual drag did not trigger card actions | PASS |
| 9. Resize handles | `app.rs::update_resize`; typed sidebar/chat dimensions with constraints; `designer_resize_clamps_to_layout_constraints` | PASS |
| 10. Grid editing | `designer.rs` grid controls; `adjust_selected_grid_columns`; breakpoint-specific regression and manual Quick Actions grid verification | PASS |
| 11. Gap and padding editing | `layout_inspector.rs` fields for home/grid/card/page gaps and padding; typed layout override merge tests | PASS |
| 12. Alignment/orientation | `layout_inspector.rs` choice fields and typed enum merge; layout choice tests | PASS |
| 13. Visibility controls | Home section visibility/order controls; hidden-component recovery and visibility-preservation tests | PASS |
| 14. Component tree panel | `designer.rs` Home component tree and reorder controls; manual tree reorder verification | PASS |
| 15. Inspector synchronization | `layout_inspector.rs` read/apply functions and app inspector routing; manual inspector sync | PASS |
| 16. Snap/constraints | `designer::snap_layout_slot`, `snap_layout_dimension`; invalid-drop and clamp tests | PASS |
| 17. Responsive breakpoint designer | Breakpoint/custom-width controls and tiered `ByTier` overrides; breakpoint-specific regression | PASS |
| 18. Live TOML synchronization | `layout_watcher.rs`; external edit reload recorded in manual acceptance; typed reload tests | PASS |
| 19. Undo/redo | `DesignerHistory` and keyboard routing; manual reorder/resize/grid round-trips and bounded-history tests | PASS |
| 20. Dirty state/save workflow | `set_layout_overrides`, inspector Save/Reload, atomic `boru-layout.toml` writes; save round-trip tests | PASS |
| 21. Responsive semantics | `docs/visual-designer/responsive-semantics-audit.md`; no persisted desktop coordinates; semantic values only | PASS |
| 22. Component gallery support | `component_gallery.rs::view_gallery_with_designer`; production components, not mocks; gallery tests | PASS |
| 23. Final live-editor infrastructure | Reused typed config/theme/inspector/watcher seams; existing live-editor DoD report | PASS |
| 24. Error/constraint feedback | Designer validation errors, rejected-operation feedback, structured layout warnings; invalid-drop tests | PASS |
| 25. Home-screen-first scope | `docs/visual-designer/home-scope.md`; Home sections are the supported editing surface | PASS |
| 26. Automated tests | DEBSRV targeted designer/layout/theme runs below | PASS WITH BASELINE NOTE |
| 27. Manual acceptance | `docs/visual-designer/manual-acceptance.md`; all ten checks marked PASS, including final VM sweep | PASS |
| 28. Coding-agent guardrails | `docs/visual-designer/guardrails-audit.md`; Iced, single TOML layout store, no raw coordinates, no service restart, production widgets reused | PASS |

## Definition of Done evidence

| DoD clause | Evidence | Verdict |
|---|---|---|
| Launch once in developer mode and edit Home visually | `cargo run --features dev-ui` / debug `--dev-ui` / `BORU_DEV_UI=1` paths documented in `manual-acceptance.md`; fresh dev-ui binary rendered Home under Xvfb/Openbox | PASS |
| Select real components and synchronize inspector | Production Home overlay emits stable `ComponentId`; inspector reads the selected component; Quick Actions selection manually verified | PASS |
| Drag sections to reorder | Dedicated handles/tree controls commit semantic `home.section_order`; live manual reorder and regression test pass | PASS |
| Resize supported elements | Resize gesture commits typed sidebar/chat/card dimensions; pointer coordinates remain transient; constraint test passes | PASS |
| Change grid/layout properties | Grid columns, alignment/orientation, visibility, gap, padding, spacing and breakpoint fields are typed inspector/layout fields; grid and layout regression tests pass | PASS |
| Save edits to existing TOML configuration | Inspector Save calls the existing atomic `boru-layout.toml` writer; typed transaction and save round-trip tests pass | PASS |
| Restart reproduces saved design | Manual acceptance records Ctrl+S, process restart, and restored Home order; loader merges the same TOML overrides | PASS |
| Normal chat, room, tunnel, media and transfer behavior unaffected | Designer is feature/runtime gated; disabled overlay returns normal content; guardrail tests/documentation show designer update arms do not touch services; manual Designer Mode-off check restored normal behavior | PASS |

## Verification matrix

All compilation and test commands below ran on DEBSRV through `rb` from this worktree.

| Command | Result |
|---|---|
| `rb check --features dev-ui` | PASS, exit 0; warnings only |
| `rb check --all-targets` | BLOCKED by unrelated stale `DiscoveryService::join` calls in `tests/test_public_room_directory.rs:175` and `tests/test_discovery_two_node.rs:192,199` |
| `rb test --features dev-ui --bin boru -- designer` | **16 passed, 0 failed** |
| `rb test --features dev-ui --bin boru -- layout` | **136 passed, 1 failed**; failure is the pre-existing default-orientation assertion in `app::tests::inspector_reset_layout_section_and_all_restore_defaults` |
| `rb test --features dev-ui --bin boru -- theme` | **98 passed, 0 failed** |

The designer-specific suite is fully green. The layout failure is outside the designer DoD behavior and was not modified. The all-targets compile failures are outside the designer chain and are already documented by the Task 28 guardrail audit.

## Guardrail confirmation

- UI remains Iced (`src/bin/boru/main.rs`, `designer.rs`, `app/home.rs`).
- Layout persistence reuses typed `LayoutConfig`/`LayoutOverrides` and existing `boru-layout.toml`; `boru-ui.toml` remains the theme file rather than a second layout store.
- Pointer points are transient gesture state; persisted edits are semantic order, dimensions, tier values, visibility, spacing, and other typed leaves.
- Designer operations do not construct/restart network, chat, tunnel, room, media, or transfer services.
- Home overlays wrap existing production widgets; the gallery uses production components.
- `dev-ui` is opt-in and the runtime gate keeps the production path unchanged.

## Known gaps / out of scope

- Editing screens outside Home is intentionally out of scope for the initial designer scope.
- The unrelated discovery/public-room API call-site drift and the unrelated layout reset baseline failure remain for their owning tasks.
- No new persistence system, UI framework, or raw-coordinate layout format was introduced.

## Conclusion

The PDF Definition of Done is demonstrated end-to-end for Boru's supported Home visual-designer scope: one developer-mode launch is sufficient to select and edit real components, reorder and resize semantically, change responsive/grid/layout properties, persist through the existing TOML configuration, restart with the saved design, and return to normal chat/room/tunnel/media/transfer behavior with Designer Mode disabled.
