# FS-24 Visual QA Handoff

## CARD
FS-24 — Perform visual QA and pixel-level refinement

## STATUS
Complete — ready for orchestrator visual approval

## SUMMARY
Performed systematic visual QA pass against the FS-02 File Sharing Dashboard
spec. Identified and fixed three token-level issues: added `destructive_soft()`
theme-aware token for the destructive confirmation banner, replaced a raw
`Color::from_rgba` call in `shared_by_me_table.rs` with the new token, and
replaced an inline `size(28.0)` in `sharing_summary.rs` with
`Typography::PageTitle`. All other visual attributes (padding, tab spacing,
column proportions, card styling, empty states, responsive layout, color
contrast, hover/focus/disabled states) already conform to the spec. Captured
fresh screenshots at 1440x900, 1280x800, and 1024x720 confirming no visual
regression.

## CHANGED FILES (verified paths)
- `examples/iced_chat/design_tokens.rs` — added `destructive_soft()` function
- `examples/iced_chat/shared_by_me_table.rs` — replaced inline `Color::from_rgba` with `destructive_soft()`
- `examples/iced_chat/sharing_summary.rs` — replaced inline `size(28.0)` with `Typography::PageTitle`

## DESIGN/ARCHITECTURE DECISIONS

1. **destructive_soft(theme)**: Added to design_tokens.rs as a first-class
   theme-aware token. Uses `color_danger` at 8% opacity (light) / 12% opacity
   (dark). This eliminates the last `Color::from_rgba` call in the file-sharing
   view code, satisfying the FS-02 spec prohibition (§2.3).

2. **Typography-driven metric values**: The Sharing Summary card's large metric
   numbers now route through `Typography::PageTitle` instead of inline
   `size(28.0)`. This means any future change to the page-title size
   propagates automatically.

## COMMANDS RUN (exact + result)

```bash
# Build verification
cargo check --bin boru --features gui
# → 0 errors, 211 pre-existing warnings (unchanged)

# Token tests
cargo test --bin boru --features gui -- design_tokens -- --nocapture
# → 18 passed

# Component tests
cargo test --bin boru --features gui -- sharing_summary shared_by_me_table -- --nocapture
# → 21 passed

# Screenshot capture
bash scripts/fs24_screenshots.sh
# → 3 screenshots captured successfully
```

## TESTS
- design_tokens: 18/18 passed
- sharing_summary: 4/4 passed
- shared_by_me_table: 17/17 passed
- No test regressions

## VISUAL EVIDENCE (screenshots)
- `docs/ui-redesign/evidence/fs-24/t_f4f6f34d_file_sharing_1440x900.png` (wide)
- `docs/ui-redesign/evidence/fs-24/t_f4f6f34d_file_sharing_1280x800.png` (reference)
- `docs/ui-redesign/evidence/fs-24/t_f4f6f34d_file_sharing_1024x720.png` (narrow)

## VISUAL-DIFFERENCE CHECKLIST

| Check | Status | Notes |
|-------|--------|-------|
| Page padding (24px) | ✓ PASS | `padding([SPACE_24, SPACE_24])` on page Column |
| Content width | ✓ PASS | `Length::Fill` with FillPortion columns |
| Sidebar continuity | ✓ PASS | Sidebar intact, Files item present with active state |
| Title/subtitle/search/action alignment | ✓ PASS | Header Row: title left, search+button right |
| Tab spacing (SPACE_16) | ✓ PASS | `Row::new().spacing(SPACE_16)` |
| Active tab underline (2px primary) | ✓ PASS | 2px `primary(theme)` container below active tab |
| Tab bar padding (SPACE_8 top/bottom, SPACE_24 left/right) | ✓ PASS | `padding([SPACE_8, SPACE_24])` |
| Card radii (RADIUS_LG 12px) | ✓ PASS | `card_style` uses `RADIUS_LG.into()` |
| Card borders (1px border_muted) | ✓ PASS | `card_style` uses `border_muted(theme)` |
| Card shadows (subtle drop) | ✓ PASS | `card_style` applies `shadow_card(theme)` |
| Card internal padding (SPACE_16) | ✓ PASS | All card containers use `SPACE_16` |
| 2/3 + 1/3 column proportions | ✓ PASS | `FillPortion(63)` / `FillPortion(34)` |
| Table row height (56px) | ✓ PASS | `TABLE_ROW_HEIGHT` constant |
| Table row typography | ✓ PASS | Body 14px / SecondaryText 12px |
| Color tokens (no from_rgb in view code) | ✓ PASS | All colors via `design_tokens` functions |
| Typography tokens (no inline sizes) | ✓ PASS | Only pre-existing inline in app.rs (out of scope) |
| Progress bar weight (4px) | ✓ PASS | `PROGRESS_BAR_HEIGHT` constant |
| Sharing Summary metric sizing | ✓ PASS | Now uses `Typography::PageTitle` (was inline) |
| Empty states per tab | ✓ PASS | Spec-compliant messages for all tabs |
| Responsive: narrow ≤1024px | ✓ PASS | Single column, scrollable tabs |
| Responsive: medium 1024-1279px | ✓ PASS | Two columns, reduced search |
| Responsive: wide ≥1280px | ✓ PASS | Full two-column layout |
| No clipping or overlap | ✓ PASS | All elements fit at all breakpoints |
| No screenshot-specific hacks | ✓ PASS | All changes are token-level |
| Keyboard/accessibility preserved | ✓ PASS | No interactive element changes |
| Destructive soft bg token-driven | ✓ PASS | `destructive_soft(theme)` replaces inline rgba |

## SECURITY/PRIVACY IMPACT
None. All changes are internal refactoring of color/spacing tokens. No data
handling, file paths, or protocol behavior changed.

## KNOWN LIMITATIONS
- The Peers panel shows a placeholder card when no peers are downloading. This
  is data-dependent — the panel renders correctly once transfers begin.
- 211 pre-existing compiler warnings in the codebase (unchanged).
- Iced v0.14 lacks CSS-style animation — progress bar transitions are instant.

## FOLLOW-UPS
- None required. Token-level changes are complete.
