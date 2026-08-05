# UI-HOME-18 accessibility checklist

Scope: final accessibility pass over the completed Boru home screen and
typography system (UI-HOME-04..17), light theme (default). Each row states
the check, the result, the evidence, and whether a follow-up ticket is
needed. Contrast ratios computed this run from the actual token hex values
in `design_tokens.rs` (WCAG 2.1 relative-luminance formula); the token
table is in DESIGN_SYSTEM.md §3.1.

## 1. Visible focus

| Check | Result | Evidence |
|---|---|---|
| Focus ring present on focused TextInputs | PASS | `app.rs:29070-29078` — `Status::Focused` renders a 2px `color_focus()` (#2B9B67) border; UI-HOME-17 verified autofocus (Ctrl+N → name input) and Tab focus (name → description) with OCR captures `kb_typed_name_autofocus.png`, `kb_tab_focus_order_group.png` |
| Focus ring on buttons | KNOWN LIMITATION (pre-existing) | iced 0.14 `button::Status` has no `Focused` variant (`app.rs:24717` comment); buttons are not keyboard-focusable. Pre-existing framework limitation, unchanged by UI-HOME work; hover is the interactive affordance. Already documented by UI-HOME-07/17 |
| Focus ring contrast (non-text ≥ 3:1) | PASS | `color_focus` #2B9B67 vs surface 3.51:1, vs input 3.09:1, vs primary_soft 3.14:1 — all ≥ 3:1 |

## 2. Keyboard order / operability

| Check | Result | Evidence |
|---|---|---|
| Global shortcut Ctrl+N opens Create Room dialog | PASS | UI-HOME-17 matrix row 7a (OCR-verified) |
| Dialog auto-focuses first field | PASS | UI-HOME-17 row 7b — typed text lands in "Room name" |
| Tab moves name → description | PASS | UI-HOME-17 row 7c — OCR "Focus Group" → "Focus Description" |
| Every interactive element reachable by Tab | PARTIAL (see buttons above) | TextInputs, dialogs, and shortcut paths verified; iced 0.14 buttons not keyboard-focusable (framework limitation) |

## 3. Contrast (computed this run, light theme)

| Token | Hex | On surface | On input | On primary_soft | Verdict |
|---|---|---|---|---|---|
| text_primary | #17211B | 16.54:1 | 14.55:1 | 14.80:1 | PASS AA normal |
| text_secondary | #5F6F66 | 5.31:1 | 4.67:1 | 4.76:1 | PASS AA normal |
| text_muted | #8A978F | 3.04:1 | 2.68:1 | 2.72:1 | **FAIL AA normal** (below 4.5:1) — see follow-up |
| primary (bg) | #188C50 | — | — | — | white label on it = 4.28:1 → **FAIL AA normal text** (≥4.5 needed; passes large-text 3:1) — see follow-up |
| primary_hover | #147643 | white on it 5.67:1 | | | PASS |
| color_success (online dot) | #20A661 | 3.14:1 | | | PASS non-text ≥3:1 (indicator only; label text also present) |
| color_danger | #C84E4E | 4.51:1 | | | PASS non-text; white text on it 4.51:1 |
| color_focus | #2B9B67 | 3.51:1 | 3.09:1 | 3.14:1 | PASS non-text ≥3:1 |
| color_warning | #B3730D | 3.91:1 | | | PASS non-text ≥3:1 |

Status encoding is never colour-only: every status has text/icon in
addition to colour (badges, secondary lines, `MeshEventTone` content
classifier — UI-HOME-05), satisfying DESIGN_SYSTEM.md §13.1.

## 4. Target sizes

| Element | Size | Requirement | Result |
|---|---|---|---|
| Quick-action icon container | 56×56 px (`QUICK_ACTION_ICON_SIZE`) | ≥32×32 | PASS |
| Quick-action card (whole card is a button) | content-driven, ~100+ px tall | ≥32 | PASS |
| Online Peers row | 60 px (`PEER_ROW_HEIGHT`, band 58–68) | ≥32 | PASS |
| Recent Activity row | 48 px (`CARD_ROW_HEIGHT`) | ≥32 | PASS |
| Tunnels row | 48 px | ≥32 | PASS |
| Hero actions (Retry / View details) | standard button height ≥32 | ≥32 | PASS |
| Spacing between tappable targets | ≥8 px within cards, 20 px card gaps | ≥4 px | PASS |

## 5. Accessible labels where supported

| Check | Result | Evidence |
|---|---|---|
| Icon-only buttons carry tooltips | PASS | `icon_with_tooltip()` / `tooltip_for()` in `icon_system.rs:374-397`; tooltip text uses `TypeRole::Metadata` (12px) |
| Long clipped names expose full text via tooltip | PASS | `app.rs:23238/23558/23744/23889` — group names, display names, room names wrapped in `Tooltip::new(..., Position::Right)` |
| Empty states are text-based, not blank | PASS | UI-HOME-16 empty copy (Online Peers "No peers discovered yet", Recent Activity, Tunnels, Mesh Health) — satisfies §13.6 "empty lists must communicate with text" |
| Iced aria-label support | N/A | iced 0.14 exposes no stable screen-reader `aria_label` API; tooltips are the supported label mechanism |

## 6. Typography / glyph integrity (accessibility-relevant)

| Check | Result | Evidence |
|---|---|---|
| Minimum body text ≥13px | PASS | `TypeRole::Body/SupportingText` = 15/13px; Metadata 12px; captions 12px — never below the 10px floor |
| No missing glyphs / tofu / black squares | PASS | 4-width OCR mean confidence 71.7–77.8; dark-blob scan found only text glyphs (14–23px), zero hollow-frame/black-box patterns |
| No synthetic weights | PASS | `type_role_weights_are_real_not_synthetic` test; fallback clamps to registered weights (fonts.rs:470-477) |
| Fallback degrades to Source Sans 3 / platform mono | PASS | `type_role_fallbacks_are_platform_appropriate` + `fallback_font()` (fonts.rs:461-487); font tests 14/14 |

## Follow-up tickets filed (do NOT fix in this card)

1. **text_muted contrast** — #8A978F at 3.04:1 on white fails WCAG AA for
   normal-size text; used for timestamps/metadata (12px). Consider darkening
   to ≥4.5:1 (#5F6F66-class) or restricting to large/incidental text only.
2. **White label on primary buttons** — 4.28:1 (14px semibold
   `ButtonLabel`) just misses AA normal-text 4.5:1. Darken primary
   (#147643-class) or bump button labels to ≥18px semibold.
3. **iced 0.14 button focus** — no `Focused` status variant; buttons can't
   take keyboard focus. Pre-existing framework limitation; re-evaluate when
   the iced version supports focus styling.

## Pre-existing failures NOT caused by UI-HOME (see report §tests)

- 20 `cargo test --lib` failures on origin/main (zero diff in `src/`
  between this branch and origin/main): group_encryption integration
  flakiness (5), stale-timestamp chat_core tests (memory notes: remediation
  t_99573d95 / t_42beb205 already filed), friendly-name generation
  flakiness (peer_names/conversations/resolve_name), store/fs path tests.
