# UI-14 evidence: Final visual polish and consistency (t_5a3e7ac0)

Final visual-consistency pass over the home screen against Figure 3. The
changes are deliberately small, incremental component tweaks — no layout
restructure, no new data, no production-path changes:

## What changed

- **Quick-action icon semantics (Figure 3 alignment)** — `quick_actions.rs`:
  - "Create Group Chat" now uses a two-people (`users`) icon instead of the
    notification bell that previously suggested alerts.
  - "Share Files" now uses the Lucide `upload` arrow (arrow rising from a
    tray) instead of the folder metaphor.
  - Two new Lucide SVG assets (`assets/icons/lucide/users.svg`,
    `upload.svg`) and two new `Icon` variants (`Icon::Users`,
    `Icon::Upload`) in `icon_system.rs`, with a unit test pinning the four
    action-card icons to the Figure 3 semantics.
- **Action-card typography** — card titles now use the Source Sans 3
  Semibold weight (matching the Figure 3 bold title treatment) instead of
  the regular weight.
- **Card radius consistency** — the quick-action cards now use `RADIUS_LG`
  (12 px) via a local card-style (`quick_action_card_style`) instead of the
  generic 8 px `BUTTON_CARD`, and the home Mesh Activity card now uses
  `design_tokens::card_style` instead of the 8 px `container_card`. Every
  home card now shares one radius system: hero `RADIUS_XL` (16 px), body
  cards `RADIUS_LG` (12 px).
- **Icon wells are circular** — `icon_tile` now renders a perfect circle
  (radius = half the tile diameter) matching Figure 3's soft green circular
  icon containers instead of a 10 px-radius rounded square.

## Verification

- **601 tests pass** (`cargo test --features gui --example boru`),
  `cargo fmt --check` clean, `git diff --check` clean.
- Pixel-verified on the fresh 1280×800 capture:
  - All four icon tiles are circular: the tile bounding-box corners are
    white card background while the inner region is `primary_soft` green.
  - Icon glyph fingerprints changed as intended: "Create Group Chat" stroke
    pixels 11 → 21 (bell → two people), "Share Files" 7 → 10 (folder →
    upload arrow).
  - Labels render darker (Semibold) than the parent capture.
- No horizontal scroll or clipped cards at any required viewport. Content
  respects the responsive canvas padding at every size:
  - 1024×720 — content ends ~16 px from the right edge (only the page
    scrollbar touches the edge).
  - 1280×800 — content ends ~24 px from the edge; footer fully visible.
  - 1440×900 — content ends ~33 px from the edge.
  - 1920×1080 — content ends ~33 px from the edge.

## Files

- `t_5a3e7ac0_home_{1024x720,1280x800,1440x900,1920x1080}.png` — fresh
  captures at all four required viewports after the polish pass.
- `t_5a3e7ac0_side_by_side_1280x800.png` — Figure 3 target beside the final
  implementation capture.
- `verification.json` — machine-readable pixel checks.
