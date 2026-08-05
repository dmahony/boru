# UI-HOME-09 — Standardise spacing, hierarchy and vertical rhythm

- Task: `t_a24fbc67` (UI-HOME-09)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-09 card, page 15)
- Repo: `/home/dan/iroh-gossip-chat` @ `main` (based on `0cd8ebba` UI-HOME-06, on top of UI-HOME-04/05/07/08)
- Status: DONE (build green, 880/880 tests pass, before/after screenshots, removed one-off rules listed, pushed)
- Labels: ui-home, design-system, visual-qa, regression-risk
- This card gates UI-HOME-10.

## Summary

Visual-consistency pass over the whole Figure 3 home dashboard. Every
structural gap now comes from the plan's shared spacing scale
(4, 8, 12, 16, 20, 24, 32 px): page header → dashboard is 28 px, card
title → subtitle is 4 px, card header → content is 16 px, and all
one-off margins/padding (raw `2.0` badge padding, raw `48.0`/`24.0`/`22.0`
hero-badge literals, off-scale `SPACE_6`/`SPACE_10` structural gaps) are
gone. No content, business logic, or card structure changed — this is a
token-level standardisation across the shared CardShell foundation and the
home view.

## What changed

### `examples/iced_chat/card_shell.rs` — shared dashboard-card foundation

The shell every home card (Mesh Health, Online Peers, Recent Activity,
Tunnels) is built from. One change here standardises all of them:

| Gap | Before | After | Plan band |
|---|---|---|---|
| Card title → subtitle | `SPACE_2` (2 px, off-scale) | **`SPACE_4`** | 4–8 px |
| Card header → content | `SPACE_6` (6 px, off-scale) | **`SPACE_16`** | 16–20 px |
| Body → footer | `SPACE_6` (6 px, off-scale) | **`SPACE_8`** | shared scale |
| Header element horizontal gap | `SPACE_6` | **`SPACE_8`** | shared scale |
| Empty-state vertical padding | `SPACE_6` | **`SPACE_8`** | shared scale |
| Count / status badge padding | raw `2.0` literal | **`SPACE_2`** token | no raw literals |

### `examples/iced_chat/app.rs` — home view (`view_chat_list_content`)

| Gap | Before | After | Plan band |
|---|---|---|---|
| Page header → dashboard | `SPACE_20` (20 px) | **`SPACE_28`** | 28–32 px |
| Greeting → welcome line | `SPACE_2` (2 px) | **`SPACE_4`** | 4–8 px |
| Status pill vertical padding | `SPACE_10` (36 px pill) | **`SPACE_12`** (~40 px pill, still in the UI-HOME-02 36–40 px band) | shared scale |
| Status pill icon gap | `SPACE_6` | **`SPACE_8`** | shared scale |
| Hero badge container | raw `48.0` ×2 + raw radius `24.0` | **`AVATAR_MD`** + `AVATAR_MD/2` | icon-container tokenised |
| Hero badge glyph | raw `22.0` | **`IconSize::Lg.px()`** (24 px) | standard icon size |
| Mesh stat-tile gaps | `SPACE_6` ×2 | **`SPACE_8`** ×2 | shared scale |
| Mesh body block gaps | `SPACE_10` ×2 | **`SPACE_12`** ×2 | shared scale |

Card gaps between major cards stay `SPACE_20` (already in the 20–24 px
band, set by UI-HOME-02), the 24 px column gap and the 28/32 px page
padding are unchanged. Quick actions were already standardised by
UI-HOME-06 (56 px tile, `IconSize::Lg`, SPACE_16/8/12 rhythm) and needed no
changes.

### `DESIGN_SYSTEM.md`

Added a "Home Dashboard Rhythm (UI-HOME-09)" table under §2 Spacing that
records the standard bands and token sources so later visual work (UI-HOME-10
onwards) preserves them.

## Removed one-off rules (required evidence)

1. `card_shell.rs` count badge padding `[2.0, SPACE_8]` → `[SPACE_2, SPACE_8]` — raw `2.0` literal tokenised.
2. `card_shell.rs` status badge padding `[2.0, SPACE_8]` → `[SPACE_2, SPACE_8]` — raw `2.0` literal tokenised.
3. `card_shell.rs` title_col `.spacing(SPACE_2)` → `.spacing(SPACE_4)` — off-scale title→subtitle gap.
4. `card_shell.rs` header→body `SPACE_6` gap → `SPACE_16` — off-scale header→content gap.
5. `card_shell.rs` header row `.spacing(SPACE_6)` → `.spacing(SPACE_8)` — off-scale header element gap.
6. `card_shell.rs` empty-state `.padding([SPACE_6, 0.0])` → `.padding([SPACE_8, 0.0])` — off-scale padding.
7. `card_shell.rs` body→footer `SPACE_6` gap → `SPACE_8` — off-scale footer gap.
8. `app.rs` greeting→welcome `SPACE_2` gap → `SPACE_4` — off-scale title/subtitle gap.
9. `app.rs` status pill `.padding([SPACE_10, SPACE_12])` → `.padding([SPACE_12, SPACE_12])` — off-scale vertical padding (pill stays in the approved 36–40 px band at ~40 px).
10. `app.rs` status pill icon gap `SPACE_6` → `SPACE_8`.
11. `app.rs` hero badge `.width/.height(Length::Fixed(48.0))` → `Length::Fixed(AVATAR_MD)` — raw literal → token (identical 48 px value).
12. `app.rs` hero badge border radius `24.0.into()` → `(AVATAR_MD / 2.0).into()` — raw literal derived from the token.
13. `app.rs` hero badge glyph `icon_svg(hero_icon, 22.0)` → `icon_svg(hero_icon, IconSize::Lg.px())` — raw icon size → standard 24 px home-card icon size.
14. `app.rs` mesh stat-tile gaps `SPACE_6` → `SPACE_8` (×2).
15. `app.rs` mesh body block gaps `SPACE_10` → `SPACE_12` (×2).

Retained with justification (not one-offs — approved bands):

- Card gaps `SPACE_20`, column gap `SPACE_24`, page padding 28/32 px (UI-HOME-02 band).
- Card padding `[SPACE_24, SPACE_24]` (UI-HOME-03 band 22–28 px) and hero padding `SPACE_32` (UI-HOME-04 band 30–36 px).
- Row-level icon gaps inside rows (avatar→text `SPACE_8`, row internal `SPACE_6`) stay as-is — they are intra-row micro-rhythm, not structural card gaps, and changing them would alter the approved UI-HOME-05/07/08 rows.

## Approved-mockup comparison notes

The plan (page 15) asks for: shared scale 4/8/12/16/20/24/32; 28–32 px
page header → dashboard; 20–24 px card gaps; 4–8 px title → subtitle;
16–20 px header → content; aligned title baselines / header actions /
status badges; aligned card edges; no one-off margins; standard icon
sizes; balanced wide and medium widths.

| Mockup requirement | Before | After | Match |
|---|---|---|---|
| Shared spacing scale only | SPACE_2/6/10 structural gaps present | all structural gaps on 4/8/12/16/20/24/28/32 | ✅ |
| 28–32 px page header → dashboard | 20 px | **28 px** (`SPACE_28`) | ✅ |
| 20–24 px between major cards | 20 px | 20 px (`SPACE_20`) | ✅ |
| 4–8 px card title → subtitle | 2 px | **4 px** (`SPACE_4`) | ✅ |
| 16–20 px card header → content | 6 px | **16 px** (`SPACE_16`) | ✅ |
| Align title baselines / header actions / badges | CardShell header row centres all elements | unchanged (already aligned) | ✅ |
| Align card edges across columns | FillPortion 2/1, top-aligned, width-Fill cards | unchanged | ✅ |
| Standard icon-container sizes | hero badge raw 48.0 + 22.0 glyph | `AVATAR_MD` + `IconSize::Lg` | ✅ |
| Balanced wide + medium | 20 px header gap felt tight | 28 px gap + 16 px in-card rhythm | ✅ |

Verified numerically on the 1280×800 captures: rail card top edges moved
109 → 119 px (page-header shift), ONLINE PEERS header 138 → 148,
RECENT ACTIVITY 363 → 383, TUNNELS 489 → 523 — each rail card grew by the
+10 px header→content band, and the mesh card grew by the +10 px header
gap plus the +4 px body-block/stat-tile adjustments. Card tops in both
columns sit at the same y (109/119), so edges align across columns.

## Tests

- `cargo build --example boru --features gui` — OK (exit 0; 207 pre-existing warnings unchanged).
- `cargo test --example boru --features gui` — **880 passed / 0 failed** (prior 876; +4 net: 2 new + existing suite re-run green).
- New tests:
  - `card_shell_spacing_uses_the_shared_scale` — pins title→subtitle `SPACE_4`, header→content `SPACE_16`, body→footer `SPACE_8`, badge padding via `SPACE_2` token, and forbids off-scale `SPACE_6` structural gaps / raw `2.0` padding.
  - `home_screen_spacing_uses_the_shared_scale` — pins page-header→dashboard `SPACE_28`, greeting→welcome `SPACE_4`, hero badge `AVATAR_MD` + `IconSize::Lg`, pill `SPACE_12` padding, and forbids raw `48.0`/`22.0`/`SPACE_10` remnants.

## Evidence

`docs/ui-redesign/evidence/t_a24fbc67/`

- `before/home_1600x900_before.png`, `before/home_1280x800_before.png`, `before/home_populated_1280x800_before.png` — full-page captures before the change (fresh-launch empty state + seeded populated state).
- `after/home_1600x900_after.png`, `after/home_1280x800_after.png`, `after/home_populated_1280x800_after.png` — same three states after the change.
- `side_by_side_1280x800.png` — before|after composite at the reference width.
- `before/geometry_before.txt`, `after/geometry_after.txt` — OCR word-box geometry (card header y positions) used for the numeric comparison above.

Capture harness: `scripts/ui_home09_evidence.sh <label>` (Xvfb, MCP
`boru_gui_navigate`, xdotool window sizing, tesseract TSV geometry).

## Remaining risks / notes

- Status pill is now ~40 px tall (top of the UI-HOME-02 36–40 px band). If
  a later pass wants the pill back at 36 px, that requires an off-scale
  `SPACE_10` vertical padding again — the band is the authority, and 40 px
  is inside it.
- The hero badge glyph moved 22 → 24 px (`IconSize::Lg`), which is the
  standard home-card icon size; it is a 2 px visual change inside the
  approved 48 px circle.
- `cargo fmt` repo-wide still shows pre-existing drift; this card's edited
  regions are rustfmt-clean by inspection (existing standing drift at
  shifted line numbers is pre-existing).
- Pre-existing NUL-byte report for `design_tokens.rs` from earlier cards is
  stale — the file is clean UTF-8 in this worktree; no NULs found.
- 207 pre-existing build warnings untouched (UI-HOME-01 baseline).
