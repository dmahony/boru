# EPIC-CONN — Connection Card Responsive Fix: Close-out Report

Date: 2026-08-09
Task: t_3cd163a0 (EPIC-CONN close-out)
Parent epic origin: t_bef6bbf9 (boru_connection_card_responsive_fix_instructions.pdf)
Repo: iroh-gossip-chat (Rust), remote origin = https://github.com/dmahony/boru.git

---

## 1. Summary of the previous card implementation (from CONN-01)

CONN-01 (t_cf5500f6) audited the "Boru is connected and ready." dark privacy panel
before any responsive work began. The audit (CONN-01-status-card-audit.md, committed
1bcda170) produced a full file:line map of the card and identified the root cause:

> The card's responsive tier was chosen from the **window-derived** `content_width`
> (app.rs home_content_width), while the card's REAL container is only FillPortion(2)
> of `(content_width − 24)` whenever the right dashboard rail (Online Peers / Recent
> Activity / Tunnels) is open. On maximized 1280/1366 windows the card (~490-654px)
> selected Tier::Full (≥760), squeezing the heading into a near-char-wide column —
> the exact "maximized machine" failure from the spec.

Additional documented problems: `Wrapping::WordOrGlyph` allowed char-by-char wrapping,
the `info` column had no minimum width, the mesh was fixed-width and did not yield,
the card was ~280px tall (min-height spacer 200 + padding 32), the icon was 100px
(dominant), the pill could wrap/stack, the mesh was too faint, and no parent-stretch
guard existed. Baseline captures (Ready 1215/679/400 + Connecting + Offline) were
generated via the offscreen tiny-skia harness and attached.

The spec's "MOST IMPORTANT" directive — *measure the card itself, reserve space for
readable text first, and remove or reposition the decorative mesh when necessary* —
drove the whole chain.

## 2. Files and modules changed per CONN card (with constants/values landed)

All implementation was confined to the presentation layer under examples/iced_chat/,
the test-only offscreen harness, captures, and docs. Commits are on origin/main.

| Card | Commit | File(s) changed | Key constants / values landed |
|------|--------|-----------------|-------------------------------|
| CONN-01 | 1bcda170 | CONN-01-status-card-audit.md, captures/ | Audit note + baseline captures. No production code. |
| CONN-02 | a167c17c | design_tokens.rs, app.rs, status_card.rs | `status_card_content_width()` helper; card width = full content when rail stacked, `(content_width − 24) * 2/3` when not; passed as `StatusCardDependency.content_width` |
| CONN-03 | a1110f9f | status_card.rs, captures/ | `STATUS_CARD_TEXT_MIN_WIDTH = 260` (Full), `STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM = 240`, `STATUS_CARD_MESH_MAX_WIDTH = 190`; `horizontal_mesh_width()` (mesh = leftover after text minimum, bounded [0,190]); heading spans `Wrapping::Word` |
| CONN-04 | 80efd989 | status_card.rs, offscreen_status_card.rs, captures/ | `STATUS_CARD_MIN_CONTENT_HEIGHT = 150` (was 200, now floor-only); vertical padding 24 (was 32); info-column gaps trimmed (Full 16/12/20, Medium 12/10/16) |
| CONN-05 | db827280 | design_tokens.rs, status_card.rs, captures/ | `STATUS_INDICATOR_SIZE 100→74`, `RING 82→60`, `GLYPH 36→26`; padding `SPACE_24` uniform; `STATUS_ICON_TEXT_GAP_FULL=24, MEDIUM=20`; `STATUS_TEXT_GRAPH_GAP_FULL=24, MEDIUM=24`; `STATUS_CARD_MESH_MAX_WIDTH 190→170` |
| CONN-06 | f820eb40 | status_card.rs, offscreen_status_card.rs, captures/ | Heading = ONE iced Rich text flow (green "Boru" run + near-white remainder, NBSP glue); sizes Full 26 / Medium 25 / Narrow 24; weight Archivo SemiCondensed Bold 700; line height 1.15 |
| CONN-07 | 284bf482 | status_card.rs, offscreen_status_card.rs, captures/ | Pill: `Wrapping::None` (nowrap), Row + container Shrink/fit-content, padding [8,12], SupportingText 13px, gap 8px |
| CONN-08 | af63b643 | status_card.rs, captures/ | Mesh alphas: Ready idle nodes 0.72-0.85 / lines 0.30; animated hubs 0.80-0.90, others 0.70-0.76, lines 0.26-0.32; dimmed nodes 0.46-0.52 / lines 0.18; halo multiplier 0.16 (no glow bomb); topology untouched |
| CONN-09 | 7435c0d6 | status_card.rs, offscreen_status_card.rs, captures/ | `STATUS_CARD_MEDIUM_CONTENT = 760`, `STATUS_CARD_NARROW_CONTENT = 560` (was 520), `STATUS_CARD_MESH_HIDE_CONTENT = 520`, `STATUS_CARD_TEXT_MIN_WIDTH_MEDIUM = 260` |
| CONN-10 | 2e3aa196 | status_card.rs, app.rs, offscreen_status_card.rs | Card container `height(Length::Shrink)` + guard comment; wide-mode column wrappers explicit `height(Length::Shrink)` |
| CONN-11 | 12a2262d | status_card.rs, offscreen_status_card.rs, captures/ | Check indicator in HEADING ROW (icon centre == heading centre); divider 32→44px wide @ alpha 0.55→0.45; gaps dd 12→10, dp 20→18 |
| CONN-12 | d793ee0f | offscreen_status_card.rs, captures/, CONN-12-width-sweep.md | Harness width sweep (400-1215, width-tagged PNGs) + per-width acceptance evidence; harness-only change |

## 3. The width-measurement fix (CONN-02) — what changed at the call site

**Before:** `app.rs` passed the window-derived dashboard width
(`content_width = home_content_width(window_width)`) as
`StatusCardDependency.content_width`. The card's tier came from a measurement that
did not reflect the card's real container when the right rail was open.

**After:** a new `design_tokens::status_card_content_width(content_width)` helper
derives the card's actual width from the same grid rules the home layout builds:

```rust
// design_tokens.rs (a167c17c)
pub fn status_card_content_width(content_width: f32) -> f32 {
    if content_width < HOME_TWO_COL_CONTENT {          // rail stacked
        content_width
    } else {                                           // rail open: FillPortion(2) of (width − 24 gap)
        (content_width - SPACE_24) * 2.0 / 3.0
    }
}
```

**At the call site (app.rs, status card construction):**
```rust
let card_width = crate::design_tokens::status_card_content_width(content_width);
let hero_card = crate::status_card::view_status_card(&StatusCardDependency {
    variant,
    content_width: card_width,   // was: content_width
    ...
});
```

Result (verified in CONN-02 metadata): maximized 1280 → card 596.7px → Medium (was
Full); 1366 → 654px → Medium; 1600 → 804.7px → Full; 1920 → 1018px → Full. Tier
thresholds were left unchanged (CONN-09 tuned boundaries). Regression tests added:
`design_tokens::status_card_width_tracks_the_real_container_not_the_window` and
`status_card::card_tier_uses_card_width_not_window_width`.

## 4. The three responsive modes and their boundaries (CONN-09)

| Mode | Card width | Layout | Mesh |
|------|-----------|--------|------|
| MODE A (Wide) | ≥ 760px | Full horizontal `[icon][text/pill][network]`, graph capped 170px | Rendered right |
| MODE B (Compact) | 560–759px | Compact horizontal; graph shrinks first (134px at 560) before the text column hits its 260px floor | Rendered right, shrinks |
| MODE C (Narrow) | < 560px | Stacked compact: `[check] Boru is connected and ready.` / divider / description / pill / optional small mesh | **Hidden below 520px** (`STATUS_CARD_MESH_HIDE_CONTENT`) |

Priority when space runs out: heading > description > security status (pill) >
decorative graph. The graph sacrifices space first, and below 520px the network
canvas is not rendered at all (`mesh_rendered()` false), exactly per spec §11/§12/§13.

## 5. Per-width checklist summary (CONN-12) — final table

Copied from the CONN-12-width-sweep.md attachment (t_862c1a4b). Card heights are the
REAL laid-out heights from the offscreen harness; widths are card content width.

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

Also captured: Connecting @ 679 = 184.0px, Offline @ 679 = 205.2px.

## 6. Spec acceptance criteria — verification

| Spec criterion | Evidence |
|----------------|----------|
| "Boru is connected and ready." always wraps by normal words | CONN-03 `Wrapping::Word` at both heading sites + CONN-12 vision review of every capture (w600/w400 wrap "…connected and" / "ready." by words) |
| "Boru" stays inline with the rest of the heading | CONN-06 single Rich text flow with NBSP glue; "is connected Boru and ready" split impossible; CONN-12 vision: Boru first word of the heading everywhere |
| No character-by-character wrapping | CONN-03 wrapping change; none observed at any width (CONN-12) |
| Card responds to its actual container width | CONN-02 `status_card_content_width()` + width sweep shows all three modes and mesh-hide transition at 520 |
| Opening the right dashboard rail cannot break the card | CONN-02 (card self-measures) + CONN-10 (no parent stretch) + CONN-12 rail-narrowed widths (679/600/550) |
| Maximizing Boru produces a clean compact card | CONN-12 w1215: 218px, one-line heading, one-row pill, mesh right, no clipping |
| Normal-size card ~200-230px tall where space permits | CONN-04 + CONN-12: Full 218.0px, MODE B 207.9px; w600 231.9px (wrapped heading growth, sanctioned); stacked MODE C taller by design (spec §12/§13) |
| Icon smaller and better balanced | CONN-05: 100→74px outer (ring 60, glyph 26); CONN-11 icon-in-heading-row composition |
| Security pill stays horizontal at normal widths | CONN-07 `Wrapping::None` + Shrink; one row at EVERY width incl. w400 (vision + regression tests) |
| Network illustration more visible but remains secondary | CONN-08 alpha bands (nodes 0.70-0.90 Ready, lines 0.25-0.35); pixel + vision verified; text never overlapped |
| Network illustration shrinks or disappears before text becomes unreadable | CONN-09: MODE B shrinks mesh first; mesh hidden below 520 (0 green px at w500/450/400) |
| Card does not stretch vertically with its parent | CONN-10: `height(Length::Shrink)` on card + column wrappers; rail-open == rail-closed == standalone height (regression test) |
| Layout visually balanced at all supported window sizes | CONN-12: all 10 captures reviewed; no asymmetry/clipping/overlap |
| Existing Boru connection behaviour untouched | All 12 CONN commits touch ONLY presentation files (see §7); no networking/state/persistence changes |

## 7. No networking / protocol / state logic changed (grep evidence)

`git show --stat` for each of the 12 CONN commits (1bcda170 … d793ee0f) shows only:

- examples/iced_chat/status_card.rs (card widget layout + draw)
- examples/iced_chat/design_tokens.rs (STATUS_* tokens + width helper)
- examples/iced_chat/app.rs — **only** the home-dashboard status-card call site
  (CONN-02 width derivation, ~11 lines) and the wide-mode column wrappers
  (CONN-10 Shrink height, ~19 lines). No networking/state/peer/side-panel/
  download-manager/chat-sidebar code touched.
- examples/iced_chat/offscreen_status_card.rs (#[cfg(test)] harness only)
- captures/*.png and the two markdown reports

Confirmed: `git log 9fcaa21c..d793ee0f -- src/ Cargo.toml Cargo.lock` contains NO
CONN commits (only the unrelated version-bump chore commits and the pre-existing
terminal fix 9cb90217 that CONN-04 rebased through).

## 8. Tests added/updated and commands run

- **rb check** (debsrv, never local): `rb check --example boru --features gui,video-playback,terminal`
  → exit 0 from the final tree (origin/main content, d793ee0f).
- **rb test** (targeted, once): `rb test --example boru --features gui,video-playback,terminal -- status_card`
  → **20 passed / 0 failed** (5.07s), including: layout_tiers_are_ordered_and_consistent,
  card_tier_uses_card_width_not_window_width, status_card_width_tracks_the_real_container_not_the_window,
  text_column_keeps_minimum_width_in_horizontal_tiers, mesh_yields_before_text_when_space_is_tight,
  mesh_is_hidden_below_mesh_hide_width, heading_sizes_land_in_the_conn06_band,
  security_pill_stays_one_compact_row_at_every_width, security_pill_uses_nowrap_and_fit_content,
  ready_card_lands_in_compact_band, hero_card_height_is_content_determined_in_dashboard_grid,
  conn11_icon_heading_row_and_text_block_align, capture_status_card_states, capture_mesh_isolated_on_white,
  status_card_is_wired_into_home_screen, status_card_mesh_adapts_to_content_width, and more.
- Per-card targeted runs during the chain: CONN-02 3/3, CONN-03 5/5, CONN-04 14/14,
  CONN-05 14/14, CONN-06 15/15, CONN-07 3/3, CONN-08 1/1, CONN-09 layout_tier +
  status_card::tests 12/12 + capture tests, CONN-10 dashboard 41/41 + home_rail 5/5 +
  status_card 15/15, CONN-11 status_card 9/9 + offscreen_status_card 7/7.

## 9. Confirmation: no business-logic/protocol/state changes

Confirmed by the commit-scope grep in §7. All CONN-* work is presentation-only:
layout, spacing, typography, colour/alpha, responsive tier selection (from the card's
own width), and the test-only capture harness. Networking logic, connected-state
detection, Mesh Health, peer counting, side panels, Download Manager, and chat
sidebar behaviour are untouched.

## 10. Remaining limitations + items needing human visual review

- **Human visual review of the captures (recommended).** The user should review the
  CONN-12 PNG matrix (`status_ready_w400 … w1215` + Connecting/Offline @ 679) for
  final aesthetic sign-off. Automated/pixel checks all pass.
- **MODE C at 520-559px is taller (393.9px at w550)** because the mesh is retained in
  the 520-559 band per spec §13's "optional small mesh". Below 520 the card drops to
  235.9px (mesh hidden). If a shorter stacked card is preferred at 520-559, that is a
  CONN-09 tier-policy choice, not a regression — no defect filed.
- **w600 card is 231.9px** (a hair over 230): MODE B, heading wraps by whole words —
  sanctioned content growth per CONN-04 requirement 3 ("grow, don't clip").
- **Other workers' unpushed commits.** The canonical repo's local `main` holds three
  unpushed commits from a separate chat/forwarder workstream (e010c26f, 822e07d3,
  129368f6 — chat neighbor sync, BackgroundSubscribe dedupe, debug logging). Per the
  CONN-09…CONN-12 precedent these were deliberately left untouched and NOT pushed by
  this epic; they will land with their owning task. Nothing CONN-related is unpushed.

## 11. Final push confirmation

- All 12 CONN commits are on **origin/main** (verified 2026-08-09 after
  `git fetch origin`):
  `git log origin/main --oneline --grep=CONN` →
  1bcda170 (CONN-01), a167c17c (CONN-02), a1110f9f (CONN-03), 80efd989 (CONN-04),
  db827280 (CONN-05), f820eb40 (CONN-06), 284bf482 (CONN-07), af63b643 (CONN-08),
  7435c0d6 (CONN-09), 2e3aa196 (CONN-10), 12a2262d (CONN-11), d793ee0f (CONN-12).
- origin/main HEAD == d793ee0f (CONN-12). All epic work is on GitHub; no CONN commit
  is local-only.
- `git push origin main` from the canonical repo is intentionally NOT run for the
  three unrelated chat-fix commits (they belong to another in-flight workstream; see
  §10). Pushing them would publish another worker's unreviewed work ahead of its task.
- This close-out report is committed on wt/t_3cd163a0 (at origin/main) and pushed to
  origin/main as a documentation commit.
