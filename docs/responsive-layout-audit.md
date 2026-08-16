# Responsive layout regression matrix

This document defines the viewport matrix for automated and manual responsive
checks. The dimensions are intentional test fixtures, not additional
breakpoints; tier resolution remains centralized in `ResponsiveLayout`.

## Viewport matrix

| Width | Height | Width tier | Height tier | Primary use |
|---:|---:|---|---|---|
| 1024 | 720 | Desktop | Short | Minimum supported viewport; compact-height actions and dialogs |
| 1280 | 720 | Desktop | Short | Reference-width short-window regression |
| 1280 | 800 | Desktop | Normal | Reference baseline |
| 1366 | 768 | Desktop | Short | Common laptop viewport |
| 1440 | 900 | UltraWide | Tall | First ultra-wide boundary and tall layout |
| 1920 | 1080 | UltraWide | Tall | Full-HD desktop |
| 2560 | 1440 | UltraWide | Tall | High-resolution desktop |
| 3840 | 2160 | UltraWide | Tall | 4K max-width/centering regression |

## Automated coverage

The layout unit suite pins the exact width boundaries immediately below and at
both configured width thresholds (`359.99`/`360.0` and `1439.99`/`1440.0`),
per-tier values, content-width accounting, sidebar width clamping, and the
short-window height rules used by the matrix. TOML parsing, semantic
validation, merge-with-defaults, malformed-input handling, and last-known-good
reload behavior are covered by `layout_regression.rs`.

Key-screen baseline models are covered by the existing layout regression tests:

- Home: section order, hidden sections, grid/list defaults, content sizing and
  quick-action column thresholds.
- Chat: bubble, message, preview and composer defaults.
- Files: component placement and table column defaults.
- Sidebar: width bounds, mode resolution, section order and row sizing.

The default `LayoutConfig` must continue to reproduce the pre-responsive
appearance. Any intentional baseline change should update the corresponding
assertion and this matrix together.