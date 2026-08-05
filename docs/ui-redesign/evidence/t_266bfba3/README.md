# UI-HOME-18 accessibility / visual / regression QA evidence

Captures from the running Boru GUI under Xvfb (fresh data dir,
--no-dht --no-relay, MCP + GUI test actions enabled). Four supported
widths: wide 1600x900, medium 1280x800, narrow 1024x720, minimum 800x600,
plus scrolled captures where content sits below the fold.

## Files

- `t_266bfba3_home_{1600x900,1280x800,1024x720,800x600}.png` — top-of-page
  captures at all four supported widths
- `t_266bfba3_home_{w}x{h}_scrolled.png` — full-page-bottom captures
- `t_266bfba3_home_{w}x_grid.png` — capture where the quick-action grid is
  in view (located by OCR "Create Public" heading)
- `t_266bfba3_home_{w}x{h}_scrolled_series/` — 8-step scroll series per
  width (covers the fold for the clip gate)
- `t_266bfba3_side_by_side_mockup_vs_current_1280x800.png` — approved
  Figure 3 mockup (left) vs current 1280x800 capture (right)
- `t_266bfba3_font_gallery_top.png` / `t_266bfba3_font_gallery_fallback.png`
  — controlled font-fallback scenario (developer gallery, Ctrl+Shift+G):
  primary TypeRole samples + live fallback demo (Source Sans 3 / platform
  monospace)
- `geometry.txt` — OCR word-box overflow check (words_past_right_edge=0)
- `quick_action_clip_check.txt` — HARD GATE: all four approved quick-action
  descriptions fully visible at every width (RESULT: PASS)
- `app_log_scan.txt` — panic scan across all launches (empty = none)
- `accessibility_checklist.md` — full a11y checklist (focus, keyboard
  order, contrast, target sizes, labels, typography/glyphs) with evidence
  and follow-up tickets
- `build.log` — cargo build --example boru --features gui (BUILD_EXIT=0)
- `gui_test.log` — cargo test --example boru --features gui (896/896 pass)
- `lib_test.log` — cargo test --lib (1824 pass / 20 pre-existing fail on
  origin/main, zero src/ diff; see UI-HOME-18-report.md §7)

Full report: docs/ui-redesign/UI-HOME-18-report.md
