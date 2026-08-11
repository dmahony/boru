# UI-HOME-15 — Responsive Behaviour for the Completed Home Screen

- Task: `t_dfe40e9f` (UI-HOME-15)
- Repo: `iroh-gossip-chat` @ worktree `wt/t_dfe40e9f`
- Status: IMPLEMENTED + VERIFIED (build, 891 tests, four-width screenshot set, OCR geometry)

## Summary

The home dashboard now drives every responsive decision from the available
**content width** — window width minus the sidebar (288–320 px), the 1 px
divider and both horizontal page paddings — instead of the raw window width.
Four intentional layout tiers (wide / medium / narrow / minimum) are defined
in `design_tokens.rs` and verified at 1600×900, 1280×800, 1024×720 and
800×600. No overlap, clipping or horizontal scrolling occurs at any of the
four widths; all cards, actions and full text are preserved.

## Content-width breakpoints (`design_tokens.rs`)

| Constant | Value | Meaning |
|---|---|---|
| `HOME_QUICK_FOUR_COL_CONTENT` | 1000 px | ≥ this → four quick-action columns |
| `HOME_TWO_COL_CONTENT` | 720 px | ≥ this → two dashboard columns |
| `HOME_ILLUSTRATION_FULL_CONTENT` | 720 px | ≥ this → full-size hero illustration |
| `HOME_COMPACT_HEADER_CONTENT` | 560 px | below → two-line card headers + pill under greeting |
| `HOME_QUICK_ONE_COL_CONTENT` | 520 px | below → one quick action per row (minimum) |
| `HOME_ILLUSTRATION_HIDE_CONTENT` | 520 px | below → hero illustration hidden |

`home_content_width(window_width)` subtracts `sidebar_width_for`, the 1 px
divider, and `2 × h_padding` so the fixed sidebar can never starve the grid.
At the four evidence windows the content widths are ~1231 / ~919 / ~679 /
~455 px.

## Evidence-width mapping (acceptance criteria)

| Window | Content | Layout (verified by screenshot OCR + pixel geometry) |
|---|---|---|
| 1600×900 | ~1231 px | **Wide** — two dashboard columns; four quick actions in one row; full hero illustration; pill top-right beside greeting |
| 1280×800 | ~919 px | **Medium** — two dashboard columns; 2×2 quick actions (row 2 below the fold, visible in scrolled capture); single-line card headers; pill top-right |
| 1024×720 | ~679 px | **Narrow** — one dashboard column (right rail moves below main cards); 2×2 quick actions; scaled 0.8× hero illustration; single-line headers; pill top-right |
| 800×600 | ~455 px | **Minimum** — one dashboard column; one quick action per row; compact two-line card headers (title line, then badge/action line); pill stacked under greeting; hero illustration hidden |

## What changed

### `examples/iced_chat/design_tokens.rs`
- Added `home_content_width(window_width)` — the available content width the
  dashboard actually renders in.
- Added the six content-width breakpoint constants above.
- Added tests: content width at evidence windows, breakpoint ordering,
  evidence-width → tier mapping.

### `examples/iced_chat/quick_actions.rs`
- `grid_columns_for` / `quick_action_grid` now take **content width**, not
  window width: 4 columns ≥ 1000, 2 columns 520–999, 1 column < 520.
- Tests updated to content-width semantics (e.g. 800×600 → 1 column, which
  the old window-based 640 threshold could not express).

### `examples/iced_chat/card_shell.rs`
- New `compact_header(bool)` mode: on narrow content the header becomes two
  lines — line one icon + title/subtitle (Fill, wraps), line two badges +
  action link — so titles never squeeze below a readable width.
- New test: compact header builds with every optional element present.

### `examples/iced_chat/app.rs` (`view_chat_list_content` + rail selectors)
- Rail stack now uses `content_width < HOME_TWO_COL_CONTENT` (removed the
  window-width `RAIL_STACK_BREAKPOINT = 1120`).
- Hero illustration tiers: full 205×140 ≥ 720 px content, scaled 164×112
  (0.8×) 520–719 px, hidden below 520 px.
- Page header: on compact content the status pill stacks under the greeting
  (left-aligned) instead of squeezing it; otherwise keeps top-right.
- Quick-action grid built from `content_width`.
- Mesh card + the three rail cards thread `compact_header` from the content
  width (`home_compact_headers()`), so Online Peers / Recent Activity /
  Tunnels / Mesh Health all switch to the two-line header at the minimum
  band.
- Three new regression tests: content-width breakpoints in use (no
  `RAIL_STACK_BREAKPOINT`), illustration tiers, compact-header wiring.

### `DESIGN_SYSTEM.md`
- §19.5 rewritten: content-width table, mapping to the four evidence widths,
  quick-action column rule, compact-header + pill-stacking + illustration
  tiers.

### Evidence
- `scripts/ui_home15_responsive_evidence.sh` — Xvfb harness capturing the
  four widths plus scrolled captures at 1280/1024/800 (mouse-wheel scroll,
  same pattern as UI-HOME-06) and an OCR geometry report.
- `docs/ui-redesign/evidence/t_dfe40e9f/` — 7 PNGs + `geometry.txt` +
  `README.md`.

## Verification

- `cargo build --bin boru --features gui` — OK (exit 0; only
  pre-existing warnings).
- `cargo test --bin boru --features gui` — **891 passed / 0 failed**
  (prior 884; +7 new: 3 design-token tests, 1 card-shell test, 3 app.rs
  regression guards).
- Screenshot pixel geometry:
  - 1600/1280 → two white-surface column segments; 1024/800 → one.
  - 1600 quick-action row has 4 card centers (x ≈ 432/633/834/1035); 1280
    and 1024 scrolled captures show the 2×2 grid; 800 scrolled shows one
    card per row.
  - No dark pixels within 40 px of any right window edge → no horizontal
    overflow at any width.
  - Pill: top-right at 1600/1280/1024; stacked under greeting at 800.
  - Compact header at 800: "ONLINE PEERS" title wraps to two lines with the
    "0/0" badge + "View all" on the second line.
  - OCR word-box report in `geometry.txt` (words_past_right_edge = 0).

## Accessibility text scaling

The home screen uses fixed TypeRole sizes (no user-facing global text scale
knob exists in iced 0.14 for this app), so a dedicated text-scaling test is
N/A. The layout is content-driven end-to-end (UI-HOME-10): every row and card
grows with wrapped text, so any future OS-level text scaling will reflow
instead of clipping. The four-width set above confirms typography stays
readable across the supported range.

## Remaining risks / notes

- The 1280/1024 top captures show the second quick-action row below the fold
  by design (the page scrolls vertically via `gutter_scrollable`); scrolled
  captures prove the row renders fully.
- `HOME_ILLUSTRATION_HIDE_CONTENT` equals `HOME_QUICK_ONE_COL_CONTENT` (520):
  below the minimum supported width the illustration would only crowd the
  hero text, so it is removed at the same point quick actions collapse to one
  per row.
- Pre-existing: ~207 build warnings, cargo fmt drift, `design_tokens.rs`
  NUL-byte history (file is valid UTF-8 in this worktree) — untouched.
- The evidence harness scrolls with the mouse wheel over the main panel; the
  exact pixel positions of the second quick-action row depend on the window
  manager, but the scrolled captures consistently show the full 2×2 grid.

## Files changed

- `examples/iced_chat/design_tokens.rs`
- `examples/iced_chat/quick_actions.rs`
- `examples/iced_chat/card_shell.rs`
- `examples/iced_chat/app.rs`
- `DESIGN_SYSTEM.md`
- `scripts/ui_home15_responsive_evidence.sh`
- `docs/ui-redesign/evidence/t_dfe40e9f/` (7 PNGs, `geometry.txt`, `README.md`)
- `docs/ui-redesign/UI-HOME-15-report.md` (this file)
