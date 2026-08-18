# Responsive semantics audit

Status: complete for BORU-DESIGN-21 (PDF Task 21).

## Scope and source of truth

The designer was audited against the Task 21 requirements and the typed
`LayoutConfig` model in `src/bin/boru/layout.rs`. Persisted edits continue
to use the existing `boru-layout.toml` override model; pointer coordinates are
interaction-session state only.

## Operation audit

| Operation | Persisted result | Responsive assessment |
| --- | --- | --- |
| Home drag handle | `home.section_order` insertion index | Semantic reorder; no x/y field exists in the model. |
| Component-tree reorder | `home.section_order` | Same semantic reorder path as the drag handle. |
| Resize: Sidebar | `sidebar.width`, constrained by `width_min`/`width_max` | Reference width with min/max constraints; it is not a desktop coordinate. |
| Resize: message list | `chat.message_max_width` | Preferred content width, clamped to the bubble limit. |
| Resize: composer | `chat.bubble_max_width` | Responsive content width, bounded to a safe range. |
| Quick Actions grid | Per-breakpoint `home.quick_actions.columns_*` override | Changes column count for the active responsive tier. |
| Spacing/alignment/orientation | Existing inspector `LayoutField` values | Uses typed padding, gaps, placement, alignment, and orientation fields. |
| Visibility | `home.hidden_sections` / corresponding typed lists | Structural visibility, not canvas removal or coordinate movement. |
| Breakpoint preview | Transient preview band/custom width | Selects the responsive tier being edited; the preview width itself is not persisted. |

Drag deltas and resize points are deliberately retained in `DragOperation` and
`ResizeOperation` only while a gesture is active. `update_home_drag` converts
the delta to an insertion index, and `update_resize` converts the delta to a
typed width before calling `set_layout_config`.

## Absolute positioning check

There are no persisted `x`, `y`, `left`, or `top` component-position fields in
`LayoutConfig` or its TOML override types. The designer overlay uses Iced
`Stack` widgets only for transient outlines, labels, and handles; it does not
move production content with an absolute-position API. The only
`AbsoluteOffset` uses found in the app are inspector scroll operations, not
layout persistence or component placement.

If a future overlay/floating component needs absolute placement, it must be
introduced as an explicitly typed overlay model and kept separate from the
responsive content fields. It must not add desktop coordinates to the general
component layout model.

## Cross-platform and persistence assessment

- Saved values are TOML scalar/enumeration/list values consumed by the same
  merge/validation path on Linux and Windows.
- Widths, padding, gaps, row heights, column counts, and breakpoints are
  validated/clamped by `layout_merge`; invalid structural lists are rejected by
  `layout_config` before application.
- Atomic writes and last-known-good reload behavior remain in the existing
  `layout_config` path; this task does not add a competing persistence system.
- Designer changes only replace `active_layout` and invalidate layout caches;
  they do not restart networking, chat, room, tunnel, media, transfer, or
  persistence services.

## Guard added in this task

Cancelling a resize now cancels the matching pending `DesignerHistory`
transaction. Without that cleanup, a later resize could commit a stale
pre-gesture snapshot and make undo history span unrelated operations. The
designer unit suite now covers that invariant.

## Verification commands

The required remote checks for this task are:

```text
rb check --features dev-ui
rb check --all-targets
```
