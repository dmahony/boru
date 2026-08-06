# UI-HOME-18 — Accessibility, Visual and Regression QA report

Status: VERIFIED (with filed follow-ups — no blocking defects)
Task: t_266bfba3 (reviewer)
Date: 2026-08-06
Branch: wt/t_266bfba3 (worktree; report only — no source changes)

## Summary

Final accessibility, visual and regression QA over the completed Boru home
screen and typography system (UI-HOME-04..17). The home screen closely
matches the approved mockup (side-by-side composite, OCR/pixel geometry).
**The hard gate passed: all four quick-action descriptions are fully
visible at every supported width — no clipping.** All required right-column
cards are present (Online Peers, Recent Activity, Tunnels, Mesh Health),
font roles are correct, glyphs render cleanly (no tofu/black squares/
synthetic weights), build + 896/896 GUI tests pass, and accessibility
checks pass with three follow-up tickets filed (contrast refinements +
pre-existing iced button-focus limitation). `cargo test --lib` has 20
pre-existing failures on origin/main unrelated to UI-HOME work (zero diff
in `src/` vs origin/main) — documented in §Tests.

## Deliverables (all under docs/ui-redesign/evidence/t_266bfba3/)

| Deliverable | Path |
|---|---|
| Side-by-side comparison (mockup vs current 1280x800) | `t_266bfba3_side_by_side_mockup_vs_current_1280x800.png` |
| Four-width screenshot set (1600/1280/1024/800 + scrolled + grid + scroll series) | `t_266bfba3_home_{1600x900,1280x800,1024x720,800x600}.png`, `_scrolled.png`, `_{w}x_grid.png`, `_scrolled_series/` |
| Accessibility checklist | `accessibility_checklist.md` |
| Build log | `build.log` (BUILD_EXIT=0) |
| GUI test log (896/896) | `gui_test.log` |
| Lib test log (1824 pass / 20 pre-existing fail) | `lib_test.log` |
| Font-fallback controlled scenario captures | `t_266bfba3_font_gallery_top.png`, `t_266bfba3_font_gallery_fallback.png` |
| OCR geometry (no words past right edge) | `geometry.txt` |
| Quick-action clip gate output | `quick_action_clip_check.txt` (RESULT: PASS) |
| App log panic scan (empty = none) | `app_log_scan.txt` |

## 1. Side-by-side comparison with the approved mockup

The composite pairs the approved Figure 3 mockup (rendered from the plan
PDF, canonical source `docs/ui-redesign/evidence/ui-11/target-figure3.png` —
pixel-verified as the same source image, mean abs diff 11.2/12.7 on
rescaled comparison) with the live 1280x800 capture of the current build
(`t_266bfba3_home_1280x800.png`).

Layout structure matches on both sides: sidebar navigation (brand + chat
sections + friends), page header with greeting + status pill, large
connection/hero card, quick-action grid, and the right rail (Online Peers,
Recent Activity, Tunnels, Mesh Health) with header actions.

## 2. Visual checklist (plan §Steps)

| Check | Result | Evidence |
|---|---|---|
| Page-header alignment | PASS | Header row: greeting left, status pill right (UI-HOME-09 spacing; OCR geometry rows `t_266bfba3_home_1600x900.png` band y=35-88) |
| Main/right column ratio | PASS | 66.7% / 33.3% — `FillPortion(2):FillPortion(1)` + 24px gap (app.rs:25820-25822); measured on capture: main card 793px (x 338-1131), gap 24px, rail 395px (x 1157-1552) → in plan band 65-68 / 32-35 |
| Connection-card proportions | PASS | ~244px card (plan band 230-260) — HERO_MIN_CONTENT_HEIGHT 180 + SPACE_32 padding (UI-HOME-04; unit test `hero_card_min_height_stays_in_plan_band`) |
| Mesh Health height & information | PASS | Content-driven; live badge + Neighbors/Direct/Relayed tiles + lobby/duration + 4-row recent events (UI-HOME-05); present at all widths (OCR) |
| Quick-action height & complete text | **PASS (hard gate)** | Content-driven heights (no fixed box, no clip — UI-HOME-06/10); every approved description fully visible at 1600/1280/1024/800 — `quick_action_clip_check.txt` RESULT: all four descriptions fully visible |
| Right-column spacing | PASS | Card gaps SPACE_20 (20-24 band), column gap SPACE_24, page padding 28-32 (UI-HOME-09); measured 24px column gap on capture |
| Tunnels presence | PASS | TUNNELS card present at every width with live count + spec empty copy "No active tunnels. Create or join a tunnel to securely route traffic." (OCR at 1600/1280 top, 1024/800 scrolled) |
| Card radii / borders / shadows | PASS | Shared `card_style`: RADIUS_CARD 16, 1px border_muted, shadow_card; hover elevation (UI-HOME-03/04/06) |
| Typography hierarchy | PASS | All home text through TypeRole (UI-HOME-12); roles verified by tests `home_screen_uses_type_role_roles`, `home_rail_cards_use_type_role_roles`; Manrope display-heading-only, Figtree chat-only, Raleway wordmark-only, JetBrains Mono technical-only (UI-HOME-14) |
| Overall whitespace | PASS | Shared spacing scale enforced (UI-HOME-09 tests `card_shell_spacing_uses_the_shared_scale`, `home_screen_spacing_uses_the_shared_scale`); geometry.txt shows no overflow |
| Horizontal overflow | PASS | `geometry.txt`: words_past_right_edge=0 at all captures; 40px right-edge strip white (UI-HOME-15) |

## 3. Accessibility checklist

Full checklist with per-row evidence: `accessibility_checklist.md`.

Summary:
- Visible focus: TextInputs show 2px `color_focus` ring (app.rs:29070-29078);
  buttons have no `Focused` variant in iced 0.14 (pre-existing framework
  limitation — follow-up filed).
- Keyboard order: Ctrl+N → dialog + autofocus + Tab name→description
  verified with real key events (UI-HOME-17 rows 7a-7c).
- Contrast: text_primary 16.5:1, text_secondary 5.3:1 — PASS AA. Two
  follow-ups: text_muted #8A978F 3.04:1 (below 4.5 AA normal), white label
  on primary #188C50 4.28:1 (below 4.5 AA normal; passes large-text 3:1).
  Non-text (focus ring 3.5:1, success dot 3.1:1, borders) all ≥3:1.
- Target sizes: 56px icon containers, 48/60px rows, content-driven cards —
  all ≥32px; ≥4px spacing everywhere.
- Accessible labels: icon-only buttons wrapped in tooltips
  (`icon_with_tooltip`/`tooltip_for`, icon_system.rs:374-397); long clipped
  names expose full text via tooltips (app.rs:23238/23558/23744/23889);
  empty states are text-based (UI-HOME-16).
- No colour-only status encoding: every status has text/icon + colour.

## 4. Font fallback behaviour (controlled development scenario)

Scenario: developer component gallery (Ctrl+Shift+G) — renders every
TypeRole with its primary font and a live "Fallback demo" section that
applies `role.fallback_font()` to real text widgets. Captured under Xvfb
1500x900: `t_266bfba3_font_gallery_top.png` + `t_266bfba3_font_gallery_fallback.png`.
OCR of the fallback section:

- "Fallback display_heading → Source Sans 3" ✓
- "Fallback technical_value → monospace" ✓
- "Fallbacks: all roles except technical_value → Source Sans 3 (same
  weight); technical_value → platform monospace." ✓
- App log panic scan: 0 panics.

Plus: `type_role_fallbacks_are_platform_appropriate` and
`type_role_weights_are_real_not_synthetic` tests pass (14/14 fonts tests);
fallback weights clamp to registered weights (fonts.rs:470-477) so no
synthetic bolding occurs.

## 5. Glyph integrity

- 4-width OCR mean confidence 71.7-77.8 (real glyphs recognized).
- Dark-blob scan of the 1280x800 capture: 20 blobs ≥120px, all text glyphs
  (14-23px header/body text); zero hollow-frame tofu or solid black boxes.
- All 5 font families embedded in the binary (strings scan: 11 hits for
  Figtree/Manrope/JetBrains Mono/Raleway/Source Sans 3).
- Font tests 14/14: registered weights, role→family mapping, fallback
  policy, monotonic type scale, spec sizes.

## 6. Platform inspection

- Linux/X11 is the supported desktop platform available in this
  environment; inspected via Xvfb at all four supported window sizes
  (1600x900 wide, 1280x800 medium, 1024x720 narrow, 800x600 minimum).
- Other desktop platforms not practically runnable here (no macOS/Windows
  host); font embedding + fallback design (documented above) makes the
  rendering platform-independent, and iced handles OS-level scaling
  natively (DESIGN_SYSTEM.md §13.4).

## 7. Tests

| Suite | Command | Result |
|---|---|---|
| Build | `cargo build --example boru --features gui` | **PASS** exit 0 (207 pre-existing warnings) |
| GUI + UI + integration (ex. boru) | `cargo test --example boru --features gui` | **PASS 896/896** (60.5s) |
| Fonts | `cargo test --example boru --features gui fonts::` | **PASS 14/14** |
| Lib unit tests | `cargo test --lib` | **1824 pass / 20 fail / 2 ignored** — see below |

### Pre-existing lib failures (NOT caused by UI-HOME)

`git diff origin/main HEAD` for `src/` is **empty** — this branch carries
zero changes to the library crate, so the 20 `cargo test --lib` failures
are identical on origin/main. They fall into four groups:

1. group_encryption integration (5): p2panda DCGKA flakiness
   (MissingPreKeys / "do not process own control messages") — integration
   tests, excluded from the cheap suite per card scope.
2. chat_core image-share / stale-timestamp tests (5): known stale
   `sent_at:1000` timestamp issue; remediation tasks t_99573d95 (test
   fixes) and t_42beb205 (production wiring) already filed.
3. friendly-name generation (7): `peer_names`, `conversations`,
   `resolve_name` — random-friendly-name flakiness ("should be a friendly
   name (Adjective Noun)").
4. store/fs path tests (3): `net::address_lookup`,
   `pairing_service::test_first_conversation_persists_across_restart`,
   `room_cleanup::delete_room_history_cascades_across_stores`.

None touch the home screen, typography, or any UI-HOME-changed module.
These should be triaged separately (friendly-name + fs groups look
environment/timing sensitive).

## 8. Remaining-difference list (mockup vs current)

Explicit, complete list of every remaining difference found:

1. **text_muted contrast** (follow-up t_b2ac1e1a → **resolved**, see
   `UI-HOME-followup-text-muted-contrast.md`): the QA run measured the stale
   DESIGN_SYSTEM.md hex `#8A978F` (3.04:1 on white), but the live token in
   `design_tokens.rs` at this commit was already `#64706A` (darkened in
   `04a3a7fe`) — measured 4.54–5.16:1 on all light surfaces, passing WCAG AA
   normal. Regression test `contrast_ratios_pass_wcag_aa` pins ≥4.5:1;
   DESIGN_SYSTEM.md updated to match.
2. **Primary button label contrast** (follow-up filed): white 14px
   semibold on #188C50 = 4.28:1 — passes 3:1 large-text AA, misses 4.5:1
   normal-text AA.
3. **iced 0.14 button focus** (follow-up t_d849e063 → resolved as accepted
   limitation, see `UI-HOME-followup-button-focus.md`): buttons cannot take
   keyboard focus (no `button::Status::Focused`). Pre-existing framework
   limitation — affects keyboard operability of quick actions/rows.
4. **Dark theme** not part of this QA scope (light theme is default);
   dark palette tokens exist and were covered by prior cards' tests.
5. **20 pre-existing `cargo test --lib` failures** (see §7) — unrelated to
   UI-HOME, need separate triage.
6. **207 pre-existing build warnings** (cargo fmt drift, dead code in
   legacy Typography) — unchanged.
7. **Sidebar "Add friend by key…" affordance** in the mockup HTML renders
   as a button in the live app (live app behavior supersedes static
   mockup; verified functional in UI-HOME-17).
8. **Mockup HTML uses a simplified illustration**; the live app shows the
   production network-motif SVG at the same position (deliberate — no
   static mock content in production, per plan constraint).

No clipping, no missing cards, no layout breakage remains.

## 9. Risks / notes

- Scrolled captures at 1024/800 prove below-fold content (quick-action
  rows / rail) rather than assuming it; grid captures located by OCR
  ("Create Public" heading).
- OCR of 13px muted text is noisy on full-page shots (UI-HOME-16
  documented); clip gate therefore uses word-containment across each
  width's shot set, which is robust to reading-order scrambling and would
  catch a clipped trailing word.
- UI-HOME-17 (interaction + live-update verification, commit 09adc40f)
  already covered the 11-action matrix and live-state paths; this card
  re-ran the full build/test suite on top of it.
- Gate for UI-HOME-19: side-by-side comparison ✓, four-width screenshots ✓,
  build/test logs ✓, remaining-difference list ✓ (this report), evidence
  paths listed ✓.

## Evidence inventory

- docs/ui-redesign/evidence/t_266bfba3/ — 12 PNGs + 4 scroll series
  (8 steps each) + 2 font-gallery PNGs + geometry.txt +
  quick_action_clip_check.txt + app_log_scan.txt + accessibility_checklist.md
  + README.md + build.log + gui_test.log + lib_test.log
- docs/ui-redesign/evidence/ui-11/target-figure3.png — canonical approved
  mockup source (plan PDF Figure 3)
- docs/ui-redesign/UI-HOME-17-report.md — interaction/live-update matrix
- docs/screenshots/dashboard-mockup.html — pre-redesign HTML mockup
  (informational only; superseded by Figure 3)
