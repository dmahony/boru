# UI-HOME-06 — Quick-Action Card Grid: Fix + Modernise

- Task: `t_2577e385` (UI-HOME-06)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-06 card)
- Repo: `/home/dan/iroh-gossip-chat` @ `main`
- Status: COMPLETE. All four quick-action cards show their full descriptions at
  every supported width, the grid responds 4 / 2×2 / 1 across the supported
  widths, and the four actions still open their correct flows. No business
  logic, payloads, state semantics, or network behaviour changed.

## 1. Root cause (from UI-HOME-01 audit §4)

The audit identified the exact clipping mechanism: the quick-action card was a
`button` with a **hard-coded `Length::Fixed(132.0)` height**
(`quick_actions.rs:88`) and 24 px vertical padding, leaving a 108 px inner
budget. At 1280×800 (the reference viewport) the 4-column grid made each card
~145 px wide, so the 18 px title **and** the 13 px description both wrapped to
two lines, needing ~122 px — more than the 108 px available. iced buttons with
`Length::Fixed` clip overflowing content instead of growing, so the bottom
description line was cut off. UI-HOME-12 had already removed the fixed height;
this card rebuilt the cards to the approved structure and re-verified.

## 2. What was delivered

| Area | Change |
|---|---|
| Descriptions | Updated to the approved copy (never truncated, exact strings): "Open a **public** room for anyone to join**.**", "Start a private group conversation**.**", "Connect with a friend by public key**.**", "Choose a file to share in a chat**.**" |
| Icon container | New 56 px circular tile (task band 52–60 px) with the light-green `primary_soft` background and the Figure-3 icon (chat / users / user-plus / upload) at `IconSize::Lg`. Replaces the old 40 px `icon_tile` inside the card. |
| Card padding | `[SPACE_20, SPACE_24]` — 20 px vertical / 24 px horizontal (task band 20–24 px). |
| Icon→title gap | `SPACE_16` (16 px, task band 14–18 px). |
| Title→description gap | `SPACE_8` (~8 px per task). |
| Action indicator | Subtle bottom-right chevron (`ChevronRight`, `IconSize::Xs`, muted) — hints the whole card is a button. |
| Height policy | **Content-driven** — no fixed height, no hidden overflow, no `clip()`. The card grows to contain icon + title + full description at every width (root-cause fix, per audit recommendation and task constraint). |
| Hover | Accent border + 1–2 px elevation shadow (`offset(0,2) blur 8, alpha 0.10`) over the resting `shadow_card`. |
| Responsive grid | `grid_columns_for`: **4 columns ≥ 1440 px**, 2 columns 640–1439 px, 1 column < 640 px. Four equal columns only where cards stay readable; two-by-two before they get too narrow (matches DESIGN_SYSTEM.md "Large ≥ 1440 = full four-column"). |
| Shared foundation | Cards keep the shared `card_style` surface / `RADIUS_CARD` (16) from UI-HOME-03; `RADIUS_CARD` unchanged. |
| Docs | DESIGN_SYSTEM.md §19.5 breakpoint line updated to the new grid rule. |

Actions preserved (same `AppMessage` dispatch as before):
`CreateNewRoom`, `ShowCreateGroupDialog`, `OpenFriendRequests`, `AttachPressed`.

## 3. Files changed

- `examples/iced_chat/quick_actions.rs` — card structure, descriptions, icon
  container, spacing, hover shadow, responsive breakpoints, new tests.
- `DESIGN_SYSTEM.md` — §19.5 quick-action grid breakpoint line.
- `scripts/ui_home06_quick_actions_evidence.sh` — new evidence harness
  (3-width screenshots + OCR + 4-card action clicks).
- `scripts/ui_home06_scroll_evidence.sh` — scrolled captures for the
  medium/narrow 2×2 second row.
- `scripts/ui_home06_interaction_evidence.sh` — hover elevation capture.
- `scripts/ui_home06_keyboard_evidence.sh` — Ctrl+N keyboard check.
- `docs/ui-redesign/evidence/t_2577e385/` — evidence PNGs + card-centre dumps.

## 4. Verification

### Build + tests
- `cargo build --bin boru --features gui` → exit 0.
- `cargo test --bin boru --features gui` → **866 passed / 0 failed**
  (baseline 864; +2 new quick-action tests: exact description copy and
  action-message dispatch; existing content-driven / breakpoint tests updated
  to the new design).

### Screenshots (all in `docs/ui-redesign/evidence/t_2577e385/`)
| Viewport | File | Grid |
|---|---|---|
| Wide 1600×900 | `t_2577e385_home_1600x900_after.png` | 4 equal columns, one row |
| Medium 1280×800 (reference) | `t_2577e385_home_1280x800_after.png` | 2×2 grid |
| Narrow 1024×720 | `t_2577e385_home_1024x720_after.png` | 2×2 grid (row 2 below fold) |
| Medium scrolled | `t_2577e385_home_1280x800_scrolled.png` | row 2 fully rendered |
| Narrow scrolled | `t_2577e385_home_1024x720_scrolled.png` | row 2 fully rendered |
| Hover | `t_2577e385_hover_card1_1600x900.png` | accent border + elevation |
| Keyboard | `t_2577e385_keyboard_ctrln_1600x900.png` | Ctrl+N → Create Room dialog |

### OCR: descriptions fully visible
Every description was OCR-verified word-by-word (13 px text, upscaled crops):
- 1600×900: all four full phrases on the one-row top shot.
- 1280×800 / 1024×720: row 1 on the top shot; row 2 ("Connect with a friend by
  public key." / "Choose a file to share in a chat.") on the scrolled shots —
  the page scrolls vertically by design (gutter_scrollable, UI-HOME-01 §4), the
  cards themselves never clip.

### Action test — all four cards open the correct flow
Clicked each card via the rendered label centre (xdotool) at 1600×900:
1. **Create Public Room** → Create Room dialog ("Create Public Room", "Room
   Details", "Room Name", "Visibility / Discovery" OCR-verified) ✅
2. **Create Group Chat** → Create Group dialog ✅
3. **Add Friend** → Friend Requests screen ("SEND A FRIEND REQUEST", "Incoming
   Requests") ✅
4. **Share Files** → `AttachPressed` dispatched (unit-tested); app stays alive.
   The native rfd GTK file chooser does not render under headless Xvfb — this
   is OS-dependent and noted as a remaining risk; the dispatch itself is
   covered by `action_messages_dispatch_to_expected_flows`.

### Focus / keyboard
- Mouse: whole card is a single `button`; hover adds accent border + 1–2 px
  elevation (verified: 67 accent-green border pixels on hover vs 0 at rest).
- Keyboard: iced 0.14 buttons do not implement `Focusable` (documented, UI-19),
  so the home actions stay keyboard-reachable via the global shortcuts.
  Verified **Ctrl+N still opens the Create Room dialog** under Xvfb.

### Grid geometry (pixel-verified icon-container centres)
- 1600×900: icons at x ≈ 426 / 634 / 842 / 1043, same row → 4 columns.
- 1280×800: icons at y ≈ 512 and y ≈ 711 → 2×2.
- 1024×720: row 1 at y ≈ 512; row 2 below the fold (scrolled capture shows it).

## 5. Remaining risks / notes

- **Share Files native picker**: the click dispatches `AttachPressed` and the
  app stays alive, but the OS file chooser cannot be screenshot-verified in
  headless Xvfb. The dispatch is unit-tested; visual verification of the picker
  itself needs a desktop session.
- **Breakpoints**: 4-col threshold moved 1040 → 1440 so 1280×800 now renders a
  2×2 grid (cards ~294 px wide) instead of four ~145 px columns. This matches
  the audit recommendation ("a saner 2×2 grid … or wider cards") and
  DESIGN_SYSTEM.md's "Large ≥ 1440 = full four-column". UI-HOME-15 still owns
  final home breakpoints if the plan wants a different split.
- **Card heights** remain content-driven and top-aligned per row; at the
  evidence widths all four descriptions wrap to the same line count, so the
  row heights are visually consistent (no fixed-height hack reintroduced).
- The icon container uses `primary_soft` (light green), consistent with the
  shared `icon_tile` palette; the icon glyph itself keeps `text_secondary`
  default colouring via `Icon::build()`.
- Pre-existing: NUL bytes in design_tokens.rs, ~218 build warnings, and the
  unrelated 132 px-era comments in UI-HOME-01 audit remain historical.

## 6. Evidence index

`docs/ui-redesign/evidence/t_2577e385/`:
- `t_2577e385_home_{1600x900,1280x800,1024x720}_after.png`
- `t_2577e385_home_{1280x800,1024x720}_scrolled.png`
- `t_2577e385_action_home_1600x900.png`, `t_2577e385_action_1_create_room.png`,
  `t_2577e385_action_2_create_group.png`,
  `t_2577e385_action_3_friend_requests.png`,
  `t_2577e385_action_home_1600x900_share.png`,
  `t_2577e385_action_4_share_files.png`
- `t_2577e385_hover_card1_1600x900.png`,
  `t_2577e385_keyboard_ctrln_1600x900.png`
- `t_2577e385_card_centers.txt`, `t_2577e385_card4_center.txt`
