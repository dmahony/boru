# CONN-01 — Connection Status Card Audit + Baseline Captures

Status: COMPLETE (baseline evidence for the CONN responsive-layout batch)
Date: 2026-08-09
Author: kanban task `t_cf5500f6`
Spec: `boru_connection_card_responsive_fix_instructions.pdf` (attached to `t_bef6bbf9`)

This note documents Boru's home-screen connection status card ("Boru is
connected and ready." dark privacy panel) **as implemented before any
responsive-layout work**, with file:line references, the root cause of the
responsive failure, baseline offscreen captures, and the target fixes the
later CONN cards implement. No production code was modified.

---

## 1. What the card is

- The card is the dark privacy panel at the top of the home dashboard left
  column: dark green gradient background, outlined circular status
  indicator, two-tone heading, accent divider, supporting text, a
  `Secure • Decentralized • Private` pill, and a native `canvas`
  peer-to-peer mesh on the right.
- Introduced by commit `9fcaa21c` ("feat(ui): redesign home connection
  status card as dark privacy panel"), replacing the old pale-green hero
  card. Module doc: `examples/iced_chat/status_card.rs:1-27`.
- It is deliberately theme-independent — all colours come from
  `design_tokens::STATUS_*` constants (`status_card.rs:20-22`).
- All connection-state inputs come from the caller's
  `StatusCardDependency` snapshot; the module never reads live networking
  state (`status_card.rs:24-27`). Truthfulness mapping lives in
  `app.rs` (`home_connection_variant`).

## 2. Where it lives / call chain

```
app.rs:29909   content_width = home_content_width(window_width)   // window-derived
app.rs:30050-30062  view_status_card(&StatusCardDependency { content_width, ... })
status_card.rs:103  view_status_card(dep)
status_card.rs:108  tier = layout_tier(dep.content_width)          // ← measures WINDOW-derived width
status_card.rs:192-212  container(body).padding(SPACE_32).width(Fill)
```

- The card is placed in the home grid's left column:
  - `app.rs:30527-30534` — `left_col = hero_card + mesh_card + action_grid`
    (hero_card is the status card, pushed first).
  - `app.rs:30547-30554` — when `!rail_stacked`, a two-column Row:
    `left_col FillPortion(2)` + 24 px gap (`SPACE_24`) + `right_col
    FillPortion(1)` (Online Peers / Recent Activity / Tunnels).
  - `app.rs:30523-30524` — `rail_stacked = content_width <
    HOME_TWO_COL_CONTENT` (720.0, `design_tokens.rs:269`).
  - So with the rail open, the card's ACTUAL container is
    `(content_width − 24) × 2/3` — roughly two-thirds of the window-derived
    dashboard width.

## 3. Design tokens (`examples/iced_chat/design_tokens.rs`)

| Token | Value | Line |
|---|---|---|
| `STATUS_CARD_BG_TOP` | `#10201C` | 801-805 |
| `STATUS_CARD_BG_MID` | `#091714` | 807-811 |
| `STATUS_CARD_BG_BOTTOM` | `#06100E` | 813-817 |
| `STATUS_CARD_BORDER` | `#4DE5A3` @ 0.22 alpha | 820-825 |
| `STATUS_CARD_RADIUS` | 22.0 px | 828 |
| `status_card_shadow()` | black 0.22, offset (0,6), blur 16 | 832-838 |
| `STATUS_CONNECTED` | `#4DE5A3` | 842-846 |
| `STATUS_PRIMARY_TEXT` | `#F3F7F5` (near-white) | 849-853 |
| `STATUS_SECONDARY_TEXT` | `#9FB3AA` (grey-green) | 857-861 |
| `STATUS_NETWORK_LINE` | `#4DE5A3` | 865-869 |
| `STATUS_NETWORK_NODE` | `#4DE5A3` | 873-877 |
| `STATUS_INDICATOR_SIZE` | 100.0 (outer outline diameter) | 881 |
| `STATUS_INDICATOR_RING` | 82.0 (inner ring diameter) | 884 |
| `STATUS_INDICATOR_GLYPH` | 36.0 (check glyph) | 886 |
| `home_content_width(window)` | window − sidebar − 1 − 2×h_padding | 257-261 |
| `HOME_TWO_COL_CONTENT` | 720.0 (rail stack breakpoint) | 269 |

`home_content_width` (`design_tokens.rs:250-261`): the sidebar is 288-320 px
(288 below min width, 304 at reference 1280, 320 at ≥1440; constants
`design_tokens.rs:165-168`, `VIEWPORT_LG_WIDTH = 1440` at 213); horizontal
page padding is `SPACE_32` (32) when `is_large(width)` (≥1440) else
`SPACE_28` (28) (`design_tokens.rs:245-248, 257-261`).

Examples used by the baseline captures:

| Window | content_width | card real width (rail open) |
|---|---|---|
| 1024 (min) | 679 | rail stacked → card = 679 |
| 1280 (ref) | 919 | (919−24)×2/3 = 596.7 |
| 1366 (maximized laptop) | ~1005 | ~654 |
| 1600 (capture) | 1215 | (1215−24)×2/3 = 794 |

## 4. Layout tiers + thresholds (`status_card.rs`)

- `Tier` enum: `Full | Medium | Narrow` (`status_card.rs:215-221`).
- `layout_tier(content_width)` (`status_card.rs:223-231`):
  - `≥ 760.0` (`STATUS_CARD_MEDIUM_CONTENT`, L51) → `Full`
  - `≥ 520.0` (`STATUS_CARD_NARROW_CONTENT`, L55) → `Medium`
  - else → `Narrow`
- Heading/support sizes per tier (`status_card.rs:110-114`):
  Full (30, 17), Medium (28, 16), Narrow (26, 16).
- Gaps (`status_card.rs:135-138`):
  - Full: icon-text 32 / text-graph 40 / heading-divider 20 /
    divider-description 16 / description-pill 24
  - Medium: 28 / 32 / 16 / 12 / 20
- Row layout (Full/Medium) (`status_card.rs:132-163`):
  `[0-width × 200 spacer][indicator 100][gap][info Column Fill][gap][mesh]`
  — `align_y(Center)`.
- Narrow stacked layout (`status_card.rs:165-187`):
  header row `[indicator][16 gap][heading]`, then 18 gap, divider, 14 gap,
  supporting, 22 gap, footer, 28 gap, mesh below.
- Container padding `SPACE_32` = 32 (`status_card.rs:193`);
  `STATUS_CARD_MIN_CONTENT_HEIGHT = 200.0` zero-width spacer
  (`status_card.rs:47`); module comment says the Ready card lands ~280 px
  tall (200 + 2×32 padding + content).

## 5. Status indicator (`status_card.rs:252-306`)

- Outer outlined container: `STATUS_INDICATOR_SIZE` (100), outline border
  1.0 px @ accent 0.18 alpha (`status_card.rs:291-304`).
- Inner ring container: `STATUS_INDICATOR_RING` (82), ring border 2.0 px
  full accent, background glow `accent @ 0.10` (Ready) or `0.07`
  (others) (`status_card.rs:272-289`, `266-270`).
- Glyph: `STATUS_INDICATOR_GLYPH` (36) white check (Ready), accent retry
  (Starting/Connecting), accent mesh (Degraded), accent offline (Offline)
  (`status_card.rs:254-261`).
- Accent per variant: green `STATUS_CONNECTED` (Ready), amber
  `STATUS_WARNING #E8A33D` (Starting/Connecting/Degraded), red
  `STATUS_DANGER #E55B5B` (Offline) (`status_card.rs:57-69, 235-243`).

## 6. Heading (`status_card.rs:310-349`)

- Two spans in a Row for Ready: `"Boru "` in `STATUS_CONNECTED` +
  `"is connected and ready."` in `STATUS_PRIMARY_TEXT`
  (`status_card.rs:315-336`).
- `HEADING_LH = 1.15` (`status_card.rs:314`).
- Both spans use `.wrapping(iced::widget::text::Wrapping::WordOrGlyph)`
  (`status_card.rs:331, 346`) — this permits glyph-level breaks when a
  word is wider than the column (the char-by-char collapse risk).
- Other variants render `dep.headline` in the variant accent
  (`status_card.rs:337-348`); headlines are composed in `app.rs:29925-29966`.

## 7. Divider, supporting text, footer (`status_card.rs`)

- `status_divider` (`status_card.rs:352-365`): 32 × 3 px rounded bar,
  accent @ 0.55 alpha.
- Supporting text: `TypeRole::Body`, `"Private communication, peer to
  peer."`, line height 1.5, size per tier, `STATUS_SECONDARY_TEXT`
  (`status_card.rs:118-120`).
- Footer: Ready → `security_pill()`; Offline/Degraded → actions row
  (Retry primary / Details outline buttons, `status_card.rs:405-425`);
  else 0-height spacer (`status_card.rs:122-128`).

## 8. Security pill (`status_card.rs:369-403`)

- Lock glyph 14 px + 8 px gap + SupportingText
  `"Secure  •  Decentralized  •  Private"` (`status_card.rs:373-385`).
- Padding `[8, 14]`, radius 14, bg `STATUS_CONNECTED @ 0.10`, border
  `@ 0.25` (`status_card.rs:389-401`).
- **No `nowrap` / min-width protection** — in a narrow column it wraps
  (observed stacking vertically on the maximized machine).

## 9. Network mesh (`status_card.rs:427-613`)

- `network_size(tier)` (`status_card.rs:428-434`): Full (250,170),
  Medium (200,136), Narrow (190,130).
- Canvas widget `NetworkMesh` (`status_card.rs:501-598`): 7 nodes in an
  irregular mesh (`MESH_NODES`, L467-475), 11 edges (`MESH_EDGES`,
  L478-490), two larger hubs (`hub: true` at indices 1 and 3).
- Pulse: 6 phases (`STATUS_CARD_PULSE_PHASES`, L41), one phase per
  app `ActivityTick`.
- Alpha modulation (`status_card.rs:531-542`):
  - dimmed (non-Ready): nodes 0.35/0.35, others 0.30, lines 0.12
  - animate (Ready): nodes 0.55+0.25·sin / 0.55+0.20·cos, others
    0.42+0.08·sin, lines 0.18+0.07·cos
  - idle non-dimmed: nodes 0.65, others 0.50, lines 0.22
- Line stroke width 1.0 (`status_card.rs:555`); hub halo `alpha × 0.16`
  (`status_card.rs:573`).
- Debug helper `network_mesh_for_debug` (`status_card.rs:600-613`).
- Tests: `layout_tiers_are_ordered_and_consistent` (L621-638, asserts
  `layout_tier(home_content_width(1024.0)) == Tier::Medium` at L634-637),
  `mesh_is_decentralized_not_star_shaped` (L640-674),
  `min_height_and_network_sizes_are_positive` (L676-683),
  `variant_accent_covers_every_state` (L685-697).

## 10. Parent grid + why the responsive rule is WRONG

The card's tier is chosen from the **window-derived** dashboard content
width, not from the card's **actual container width**:

- `app.rs:29909` — `content_width = home_content_width(window_width)`
  (window minus sidebar/divider/padding).
- `status_card.rs:108` — `tier = layout_tier(dep.content_width)`.
- When the right rail is visible (`!rail_stacked`, content ≥ 720), the
  card container is `FillPortion(2)` of `(content_width − 24)`
  (`app.rs:30547-30554`) — the card is only ~2/3 of `content_width`.
- The card itself is `width(Fill)` (`status_card.rs:194`), so it stretches
  to whatever the grid gives it — but its *tier* was already decided from
  the larger window-derived number.

**Failure band math.** Full tier requires `content_width ≥ 760`, but the
card's real width only reaches 760 when
`(content_width − 24) × 2/3 ≥ 760` → `content_width ≥ 1164`.
So for **content widths in [760, 1164)** the card selects `Tier::Full`
while its actual container is only **490.7–760 px**:

| Window | content | card real width | tier selected | text column (real − fixed 486) |
|---|---|---|---|---|
| 1280 | 919 | 596.7 | Full | **110.7 px** |
| 1366 | ~1005 | ~654 | Full | **~168 px** |
| 1600 | 1215 | 794 | Full | 308 px |

Full-tier fixed horizontal budget: 2×32 padding + 100 indicator + 32
icon-text gap + 40 text-graph gap + 250 mesh = **486 px**. The info column
gets whatever is left. At a 1280/1366 maximized window the text column is
~110–168 px: `"Boru is connected and ready."` at 30 px breaks
word-or-glyph (`WordOrGlyph`, `status_card.rs:331`) — words wrap nearly
one character at a time, the card becomes extremely tall, and the pill
(`"Secure • Decentralized • Private"`, ~240 px wide) stacks vertically.
This is exactly the maximized-machine failure in the spec (§"Current
problems").

The 1024 px minimum window does NOT hit the bug (content 679 → rail
stacked → card is full 679 → Medium tier is correct), which is why the
existing capture matrix at 679 looks fine while the maximized machine
fails.

## 11. Baseline captures (evidence at current code state)

The existing test-only offscreen harness
(`examples/iced_chat/offscreen_status_card.rs`, registered in
`main.rs:39-40`) renders the real `view_status_card` widget with the
tiny-skia headless renderer (no GPU/display/network). Run:

```text
rb test --example boru --features gui,video-playback,terminal -- capture_status_card --nocapture
rsync -az debsrv:~/boru-build/work-<slot>/captures/ ./captures/
```

Harness details: `dep()` fixture L44-70, `render_card` L74-111, capture
matrix L157-174:

- `captures/status_ready_wide_1215.png` — Ready, content 1215 (1600
  window), 1215×320 canvas → Full tier. In the real app with the rail open
  the card would be 794 px wide; the harness renders at the raw content
  width, so this is the *window-derived* tier, not the real container.
- `captures/status_ready_medium_679.png` — Ready, content 679 (1024
  window), 679×320 → Medium tier, rail stacked in the real app.
- `captures/status_connecting_medium_679.png` — Connecting, 679×320.
- `captures/status_offline_medium_679.png` — Offline (Retry/Details),
  679×360.
- `captures/status_ready_narrow_400.png` — Ready, 400×480 → Narrow stacked.
- `captures/mesh_isolated_white.png` — mesh canvas isolated, 200×136.

Captures are attached to this task and committed under `captures/`.

## 12. Target fixes the later CONN cards implement (from the spec)

1. **Respond to CARD width, not window width** (spec §1, CONN-02): derive
   the card's real width at the call site — `(content_width − 24) × 2/3`
   when `!rail_stacked`, else `content_width` — and pass that as
   `StatusCardDependency.content_width` (`app.rs:30050-30062`).
   iced has no CSS container queries; measuring the component width is the
   spec-permitted alternative.
2. **Minmax text column** (spec §2-3): conceptually
   `auto | minmax(260px, 1fr) | minmax(150px, 190px)` — the text column
   must never be squeezed; words must wrap by whole words only (no
   `break-all` / glyph wrapping).
3. **Compact 200–230 px height** (spec §4): reduce
   `STATUS_CARD_MIN_CONTENT_HEIGHT`/padding-driven height; card grows only
   with content; no parent stretching.
4. **Smaller icon** (spec §5-6): outer diameter 70–78 px (currently 100),
   thinner ring; a status indicator, not the focal point.
5. **Inline, left-aligned heading** (spec §7-8): 24–27 px, weight ~650-700,
   LH ~1.15-1.2; `"Boru"` stays in the same text flow as the sentence
   (one styled-span text, not a separately positioned span).
6. **Nowrap security pill** (spec §9): one compact inline row
   (`nowrap`, `fit-content`, gap ~7, padding ~8×12, 12-13 px).
7. **Brighter mesh** (spec §10): nodes ~70-90% opacity, lines ~25-35%
   (currently idle 0.65/0.22 max), subtle halo; still secondary.
8. **Three explicit modes** (spec §11-13): Wide ≥760 (graph 170-190),
   Compact 560-759 (smaller icon, graph shrinks/moves, text keeps ≥260),
   Narrow <560 (stacked; graph may disappear ≤520 — decorative, never
   allowed to destroy readability).
9. **No parent stretch** (spec §14): `align-self: start` /
   `height: fit-content`; the card's vertical size is its own content.
10. **Composition** (spec §15-17): shared alignment lines, text block as
    the visual anchor, decorative elements demoted.

Priority order preserved from the spec: heading → description → security
status → decorative graph (spec §11).

## 13. Regression protection

This audit modified **no production code** — only this note and the
baseline captures. Later CONN cards must not touch networking logic,
connected-state detection, Mesh Health logic, peer counting, side panels,
Download Manager, or chat sidebar behaviour (spec §19).
