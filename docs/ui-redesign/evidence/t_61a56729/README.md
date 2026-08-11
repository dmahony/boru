# UI-HOME-19 release-gate evidence (task t_61a56729)

Independent verification performed by the gate reviewer at gate time
(origin/main HEAD 9d34a33c). The authoritative evidence for each card lives in
its own `docs/ui-redesign/evidence/t_<card-id>/` directory; this folder holds
the gate's own re-verification outputs.

## Files

- `gate_build_test.log` — fresh `cargo build --bin boru --features gui`
  (BUILD_EXIT=0) + `cargo test --bin boru --features gui`
  (896 passed / 0 failed, 66.5s) at the exact HEAD being gated.
- `verify_mockup_composite.txt` — side-by-side composite check: 2200x808
  composite, halves split at midpoint, mean abs RGB diff 13.42 (UI-HOME-18
  reported 11.2–12.7; difference is live-state content, e.g. Connecting vs
  Ready).
- `verify_1280_wordmap.txt` — full-page OCR word map of
  `t_266bfba3_home_1280x800.png` grouped into sidebar/main/rail bands:
  hero → Mesh Health → quick actions in the main column, rail = Online Peers /
  Recent Activity / Tunnels; `words_past_right_edge = 0`.
- `verify_1600_layout.txt` — OCR of `t_266bfba3_home_1600x900.png` main column
  (hero → Mesh Health → 4-col quick actions) and rail (Online Peers / Recent
  Activity / Tunnels), matching the approved Figure 3 structure.
- `verify_qa_grids.txt` — OCR of `t_266bfba3/crops/qa_1280_grid.png` (2×2) and
  `qa_1600_grid.png` (4-col): all four approved quick-action descriptions fully
  present at both widths.
- `verify_min_and_scrolled.txt` — OCR of the 800x600 minimum-width capture
  (hero + mesh card, one-column) and scrolled captures (quick actions fully
  rendered below the fold, rail bottom shows mesh summary).

## Approved screenshot set (canonical, committed by prior cards)

- `../ui-11/target-figure3.png` — approved Figure 3 mockup (plan PDF render)
- `../t_266bfba3/t_266bfba3_side_by_side_mockup_vs_current_1280x800.png` — mockup vs current composite
- `../t_266bfba3/t_266bfba3_home_{1600x900,1280x800,1024x720,800x600}.png` (+ `_scrolled`, `_{w}x_grid`, `_scrolled_series/`)
- `../t_dfe40e9f/t_dfe40e9f_home_{1600x900,1280x800,1024x720,800x600}.png` (+ scrolled)
- `../t_266bfba3/crops/qa_{1280,1600}_grid.png` — quick-action grid crops

No font files are distributed in this evidence set (plan constraint).
