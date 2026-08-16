# BORU-TWEMOJI-11 — Responsive Picker Layout (verification notes)

- **Task**: t_0f03ccc7 (BORU-TWEMOJI-11, PDF Task 11 of `Boru_Twemoji_Migration_Coding_Agent_Plan.pdf`)
- **Date**: 2026-08-16
- **Branch**: `wt/t_0f03ccc7`
- **Scope**: make the emoji picker adapt cleanly to Boru window sizes and DPI settings — replace the fixed 8-per-row grid with a responsive column layout that never stretches the emoji artwork.

## What changed

`examples/iced_chat/emoji/picker.rs` — the picker Card is now wrapped in
`iced::widget::Responsive` (iced 0.14.2, no Iced upgrade). The `Responsive`
closure receives the actual available `Size` from the overlay and computes:

- **`picker_columns(available_width)`** — how many 36 px cells + 4 px gaps fit
  the available width (minus the Card's ~18 px horizontal chrome), clamped to
  1–9 columns. 9 columns ⇒ card ≈ 374 px (within the 340–400 px target);
  10 would be 414 px so 9 is the cap.
- **`picker_card_width(columns, available_width)`** — the natural grid width
  (never stretched), capped at the available width so the card never overflows
  the overlay (no horizontal scrolling / clipping).
- **`picker_scroll_height(columns, available_height, token)`** — at least the
  grid content height, at least the theme token (200 px), never above
  [`PICKER_MAX_SCROLL`] (340 px ⇒ card ≤ ~400 px), and never taller than what
  fits the window (short windows shrink the region instead of clipping).

The theme tokens `chat.emoji_picker_width` (336) / `chat.emoji_picker_scroll_height`
(200) remain the **baseline/reference** values (336 px ↔ the reference 8-column
grid; 200 px ↔ 5 visible rows) while the live card adapts around them.

### Why `Responsive` + manual rows, not `Grid::fluid`

iced 0.14 has no `wrap` widget; `iced::widget::Grid::fluid(max_width)` exists
but computes `cells_per_row` with `ceil`, which can make cells *narrower* than
the fixed 36 px button (the grid stretches/shrinks cells to fill). The task
requires cells stay exactly 36 px and the artwork never stretches, so the
picker builds manual rows of fixed-size cells inside `Responsive`'s closure.

### Why `Shrink` width/height on the Responsive wrapper

`Responsive` fills its parent by default, which would pin the Card to the
top-left of the overlay. `.width(Length::Shrink).height(Length::Shrink)` makes
the Responsive node hug the Card while the closure still receives the full
available `Size` (Shrink only marks compression; `limits.max()` is unchanged),
so the existing bottom-right overlay alignment is preserved.

## Geometry table (reference values)

| available width | columns | card width |
|---|---|---|
| 60 px | 1 | 54 px |
| 120 px | 2 | 94 px |
| 200 px | 4 | 174 px |
| 250 px | 5 | 214 px |
| 336 px (token) | 8 | 334 px |
| 400 px | 9 | 374 px |
| 500–1920 px | 9 (cap) | 374 px |

Scroll region: at 9 columns the 40-entry list is 5 rows (196 px) so the region
stays at the 200 px token; at 4 columns it is 10 rows (396 px) and the region
grows to 340 px (card ≈ 398 px, still ≤ 400) when the window is tall.

## Verification

### Automated tests (`rb test --bin boru --features gui,video-playback,terminal -- picker`)

15 tests pass (10 picker + 5 related app tests), including the new BORU-TWEMOJI-11
geometry tests:

- `reference_width_shows_reference_columns` — 336 px ↔ 8 columns.
- `wide_windows_show_more_columns_capped` — 400/500/1920 px → 9 columns,
  card 374 px ≤ 400.
- `narrow_windows_show_fewer_columns_without_clipping` — 250/200/120/60 px →
  5/4/2/1 columns; card ≤ available width at every width.
- `card_width_is_natural_grid_width_never_stretched` — card == natural grid
  width + chrome for every column count; artwork stays 24 px in 36 px cells.
- `scroll_height_grows_with_content_and_respects_window` — 9 cols → token 200;
  4 cols → 340 cap; short window shrinks to fit.
- `responsive_invariants_hold_across_window_sizes` — sweep 60–1920 px width /
  120–1200 px height: card never exceeds available width, scroll + chrome
  never exceeds available height, scroll ≤ 340.

### Constrained vs maximized windows

- **Constrained**: `narrow_windows_show_fewer_columns_without_clipping` +
  the invariant sweep cover chat panels from 60 px up. Fewer columns render,
  the card shrinks to the natural grid width, and the card never exceeds the
  available width, so nothing clips and no horizontal scrollbar appears.
- **Maximized / wide**: `wide_windows_show_more_columns_capped` covers
  1920 px — the picker caps at 9 columns (374 px) instead of stretching cells
  across the full panel.

### Display scaling (100–200 %)

iced uses logical pixels for `Length::Fixed`; at 125/150/175/200 % DPI the
window's *logical* size shrinks, so `Responsive` receives a smaller available
width and the picker drops to fewer columns automatically — the same code path
covered by the narrow-window tests. Cells and artwork remain fixed logical
sizes (24 px art / 36 px hit area) and are never stretched at any scaling
factor, so the picker stays usable at every scaling level the platform
supports. (On this Linux DEBSRV build host there is no interactive display to
screenshot 200 % scaling; the geometry math is scale-invariant by construction
and unit-tested across the width sweep.)

### Build verification

- `rb test --bin boru --features gui,video-playback,terminal -- picker` → 15 passed.
- `rb check --bin boru --features gui,video-playback,terminal` → clean (exit 0).
- `rb check --all-targets --features gui,video-playback,terminal` → fails ONLY
  on the documented pre-existing E0061 (`DiscoveryService::join` 5-arg
  breakage in stale integration tests `test_reconnect_asymmetric`,
  `test_discovery_two_node`) — same set as all prior BORU-TWEMOJI parents; zero
  emoji-related errors.

## Follow-up notes (not scope creep)

- Category rows (T12), search (T13) and recents (T14) will grow the grid
  content; the scroll region's `content_h.max(token)` growth already
  accommodates taller content without further layout changes.
- The `emoji_picker_width` token remains user-tunable in the theme inspector;
  it now means "preferred reference width" rather than a hard card width.
