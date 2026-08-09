# CONN-12 — Width-sweep captures + acceptance checklist evidence

Date: 2026-08-09
Task: t_862c1a4b
Spec: boru_connection_card_responsive_fix_instructions.pdf §18 ("Test widths manually") + acceptance criteria.
All implementation cards CONN-02..CONN-11 are Done; this card VERIFIES and CAPTURES (harness-only change).

## What was changed (harness only — no production code)

`examples/iced_chat/offscreen_status_card.rs`:

- `capture_status_card_states` now captures the **Ready** card at every spec §18 width with
  width-tagged names: `status_ready_w1215`, `w900`, `w800`, `w700`, `w679`, `w600`, `w550`,
  `w500`, `w450`, `w400`.
- State captures kept at one width (679): `status_connecting_medium_679`, `status_offline_medium_679`.
- `render_card` now also prints the **laid-out width** (`layout size for X: WxH`) so the
  no-horizontal-overflow criterion is verifiable directly from the layout tree, not just visually.
- Old tier-named captures (`status_ready_wide_1215`, `medium_679`, `modeb_560`, `modec_540`,
  `nomesh_500`, `narrow_400`) are superseded by the width-tagged matrix and removed from git.

The harness renders the REAL `status_card::view_status_card` with `content_width = w` (the card's
own width), matching the CONN-02 card-based measurement — the captures reflect the real responsive
tiers (MODE A ≥760 / MODE B 560-759 / MODE C <560; mesh hidden <520).

## rb commands run (all on debsrv, never local)

```
rb test --example boru --features gui,video-playback,terminal -- capture_status_card --nocapture
# -> 1 passed (12 captures) in 4.81s; RB_EXIT=0
rsync -az debsrv:~/boru-build/work-1/captures/ ./captures/
```

Final canonical-repo check: `rb check --example boru --features gui,video-playback,terminal` (see
verification below).

## Per-width checklist matrix

Measured from the layout tree (rb test stdout) and pixel probes (PIL) on the captured PNGs.
Card heights are the REAL laid-out heights (`render_card` print); widths listed are the card
content width (== canvas width == card width).

| Width | Mode | Heading words-only, "Boru" inline | Heading col ≥ ~260px (horizontal) | Pill one row | Mesh not overlapping text | No clip | No horiz overflow | Card height (laid out) | Mesh present? |
|------:|------|:---:|:---:|:---:|:---:|:---:|:---:|------:|:---:|
| 1215 | A | ✓ | ✓ (386px) | ✓ | ✓ | ✓ | ✓ | 218.0px | ✓ right |
| 900 | A | ✓ | ✓ (386px) | ✓ | ✓ | ✓ | ✓ | 218.0px | ✓ right |
| 800 | A | ✓ | ✓ (386px) | ✓ | ✓ | ✓ | ✓ | 218.0px | ✓ right |
| 700 | B | ✓ | ✓ (370px) | ✓ | ✓ | ✓ | ✓ | 207.9px | ✓ right |
| 679 | B | ✓ | ✓ (370px) | ✓ | ✓ | ✓ | ✓ | 207.9px | ✓ right |
| 600 | B | ✓ (wraps by words) | ✓ (301px) | ✓ | ✓ | ✓ | ✓ | 231.9px | ✓ right |
| 550 | C (stacked) | ✓ | n/a stacked | ✓ | ✓ (mesh bottom, separate region) | ✓ | ✓ | 393.9px | ✓ bottom |
| 500 | C (stacked) | ✓ | n/a stacked | ✓ | ✓ | ✓ | ✓ | 235.9px | ✗ hidden (CONN-09) |
| 450 | C (stacked) | ✓ | n/a stacked | ✓ | ✓ | ✓ | ✓ | 235.9px | ✗ hidden (CONN-09) |
| 400 | C (stacked) | ✓ | n/a stacked | ✓ | ✓ | ✓ | ✓ | 235.9px | ✗ hidden (CONN-09) |

Checklist legend: ✓ = passes. "n/a stacked" = the heading-width rule only applies while a
horizontal layout is active (MODE C is stacked by design, spec §12).

## Evidence per criterion (spec acceptance list)

1. **"Boru is connected and ready." always wraps by normal words** — vision review of every
   capture: at w600 and w400 the heading wraps as "Boru is connected and" / "ready." (whole
   words). No char-by-char wrapping anywhere ("co / nn / ec / te / d" absent).
2. **"Boru" stays inline** — vision: every capture shows "Boru" as the first word of the heading
   in the same text flow (accent span, not separate element).
3. **No character-by-character wrapping** — same as (1): none observed.
4. **Card responds to its actual container width** — `content_width = w` drives
   `layout_tier()` (constants in status_card.rs: 760/560/520); captures at 10 widths show the
   three modes (A horizontal full, B compact horizontal, C stacked) and the mesh-hide transition
   at 520 (w550 has bottom mesh; w500/450/400 have none — pixel probe: 3773 green px below y=250
   at w550 vs 0 at w500/450/400).
5. **Opening the right dashboard rail cannot break the card** — the card measures itself
   (`content_width`), not the window; this is the CONN-02 fix verified by CONN-10/11 and covered
   by the width sweep at the rail-narrowed widths (679/600/550).
6. **Maximizing Boru produces a clean compact card** — w1215 capture: 218px tall, single-line
   heading, pill one row, mesh right, no clipping.
7. **Normal-size card ~200-230px tall where space permits** — Full tier (1215/900/800) = 218.0px;
   MODE B (700/679) = 207.9px; all inside the band. w600 = 231.9px (MODE B, heading wraps by
   words — CONN-04 allows wrapped growth to 240). Stacked MODE C is taller by design (235.9px
   without mesh, 393.9px at w550 with the mesh retained at 520-559); not a defect per spec §12/§13.
8. **Icon smaller and better balanced** — CONN-05/11 composition; visible in all captures
   (check ~70-78px, icon/heading aligned in heading row).
9. **Security pill stays horizontal at normal widths** — pill is one row at EVERY width incl.
   w400 (vision-verified; structural nowrap guard in CONN-07 tests).
10. **Network illustration more visible but remains secondary** — mesh present at all
    horizontal tiers + w550, pixel-verified on the right/bottom; text never overlapped.
11. **Network illustration shrinks/disappears before text becomes unreadable** — mesh hidden
    entirely below 520 (w500/450/400, 0 green pixels right/bottom) and card stays readable;
    MODE B mesh shrinks (w700/679 vs w1215) before the text column hits its 260px floor.
12. **Card does not stretch vertically with parent** — CONN-10 (content-determined height);
    laid-out heights above are content-driven, all < canvas.
13. **Visually balanced at all supported widths** — all 10 captures reviewed; no asymmetry,
    clipping, or overlap found.
14. **Existing Boru connection behaviour untouched** — harness-only change; no networking /
    state / peer / side-panel / download-manager / chat-sidebar code touched.

## Measurements

Layout prints from the capture run (authoritative, layout tree):

```
layout size for status_ready_w1215: 1215.0x218.0px (canvas 1215x360)
layout size for status_ready_w900:  900.0x218.0px (canvas 900x360)
layout size for status_ready_w800:  800.0x218.0px (canvas 800x360)
layout size for status_ready_w700:  700.0x207.9px (canvas 700x360)
layout size for status_ready_w679:  679.0x207.9px (canvas 679x360)
layout size for status_ready_w600:  600.0x231.9px (canvas 600x360)
layout size for status_ready_w550:  550.0x393.9px (canvas 550x440)
layout size for status_ready_w500:  500.0x235.9px (canvas 500x440)
layout size for status_ready_w450:  450.0x235.9px (canvas 450x480)
layout size for status_ready_w400:  400.0x235.9px (canvas 400x480)
layout size for status_connecting_medium_679: 679.0x184.0px (canvas 679x320)
layout size for status_offline_medium_679: 679.0x205.2px (canvas 679x360)
```

Heading column widths (pixel probe, union of near-white text pixels in heading zone; horizontal
tiers only — the 260px floor is enforced structurally by `STATUS_CARD_TEXT_MIN_WIDTH` and the
`text_column_keeps_minimum_width_in_horizontal_tiers` test):

| Width | Heading column |
|------:|------:|
| 1215 | 386px |
| 900 | 386px |
| 800 | 386px |
| 700 | 370px |
| 679 | 370px |
| 600 | 301px |

Mesh pixel probe (green-dominant pixels by region; mesh colour #4DE5A3 == check accent, so
region isolation is required):
- Horizontal tiers (w ≥ 560): right-side green > 2900px at every width → mesh rendered right.
- w550 (stacked, 520-559): 3773px below y=250 → bottom mesh rendered.
- w500/450/400: 0px below y=250 → mesh hidden exactly per CONN-09 (`STATUS_CARD_MESH_HIDE_CONTENT` 520).

## Defects

None. Every criterion passes at every captured width. (The only observation, not a defect: the
MODE C stacked card at w550 is 393.9px because the mesh is retained in the 520-559 band per spec
§13's "optional small mesh" — below 520 the card drops to 235.9px. If the product owner prefers
a shorter stacked card at 520-559, that is a CONN-09 tier-policy choice, not a regression.)

## Files

- Captures: `captures/status_ready_w1215.png`, `w900`, `w800`, `w700`, `w679`, `w600`, `w550`,
  `w500`, `w450`, `w400`, `status_connecting_medium_679.png`, `status_offline_medium_679.png`,
  `mesh_isolated_white.png` (existing, unchanged).
- Harness: `examples/iced_chat/offscreen_status_card.rs` (test-only, committed on wt/t_862c1a4b).
- This report: `CONN-12-width-sweep.md`.
