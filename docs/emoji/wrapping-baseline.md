# BORU-TWEMOJI-18 — Solve Line Wrapping and Baseline Alignment

- **Task**: t_b49dc7da (BORU-TWEMOJI-18, PDF Task 18 of `Boru_Twemoji_Migration_Coding_Agent_Plan.pdf`)
- **Date**: 2026-08-16
- **Branch**: `wt/t_b49dc7da`
- **Type**: widget hardening — replace the fragment-row renderer with a
  focused custom widget so mixed text/SVG messages wrap and align like
  ordinary wrapped chat text.

## Problem

BORU-TWEMOJI-17 rendered a message body as a `Row::wrap()` of independent
text/SVG children. Rows cannot provide *inline* wrapping: each child is a
block, so a line that fills up wraps the whole row of widgets instead of
breaking at word boundaries inside the text, and every emoji can be pushed
onto its own row. The PDF's Task 18 explicitly allowed replacing that
approach with a focused custom widget if nested rows cannot give stable
inline wrapping.

## Solution — one cosmic-text paragraph per message

`emoji/emoji_text.rs` now renders the whole message as **one span-based
cosmic-text paragraph** — the same layout engine the plain `text()` widget
uses — inside a custom `EmojiText` widget (`iced::advanced::widget::Widget`,
`Paragraph::with_spans`):

- **Text runs** become text spans carrying Boru's message typography
  (`EmojiTextStyle`: size, font, line height, `Wrapping::WordOrGlyph`,
  color).
- **Each emoji** becomes an *invisible placeholder span* whose advance
  width equals the emoji box size (`EMOJI_TEXT_SCALE` × text size) but
  whose font metrics stay at 1.0×.
- After layout, `Paragraph::span_bounds(index)` reports exactly where each
  placeholder landed (after wrapping), and the Twemoji SVG is drawn into
  that rectangle, centered on the line box.

### The placeholder

```
EM SPACE (U+2003)   = 1.00em advance
WORD JOINER (U+2060) = 0.00em advance  (keeps the box atomic, no line break)
FOUR-PER-EM SPACE (U+2005) = 0.25em advance
```

Total = exactly `EMOJI_TEXT_SCALE` (1.25) × text size. Because the span
keeps the message font size (1.0× metrics), cosmic-text computes line
height and baseline identically to a plain text run — an emoji never
stretches a line (no blank lines) and never shifts the baseline (no
vertical jitter). A transparent span color hides the space glyphs; only
the reserved advance remains for the SVG overlay.

### Fast path preserved

Emoji-free messages take the fast path: a single plain `text()` element
with the message typography, byte-for-byte the pre-Twemoji render. A
message whose emoji all fall back to Unicode (missing SVG, unknown emoji)
also degrades to one plain `text()` element.

### Caching / performance

The custom widget mirrors iced's own `Rich` text widget: the shaped
paragraph plus a `content_key` fingerprint live in tree state. Scrolling a
long conversation reuses the shaped paragraph instead of re-shaping on
every frame; only a content change (`content_key` differs) or a format
change (`Paragraph::compare` → `Difference::Shape`) triggers a re-shape.
`Difference::Bounds` just resizes the cached paragraph.

### Preserved behavior

- Maximum message width / responsive bubble: the widget is
  `Length::Shrink` and lays out against the bubble's available width
  (`layout::sized` with `limits.max()`), so the existing max-width and
  responsive chat-bubble behavior in `app/chat.rs` is untouched.
- Copy/selection/accessibility: `Widget::operate` reports the original
  full Unicode string (`self.input`), never SVG paths or asset keys.
- Presentation only: `emoji_text` borrows `&str` and never rewrites the
  message; the wire/storage format is untouched (see BORU-TWEMOJI-15).

## Test cases (wrapping near wrap boundaries)

All in `emoji/emoji_text.rs` tests, run with the real cosmic-text/iced
renderer:

| Test | What it locks in |
|---|---|
| `long_mixed_message_wraps_naturally_at_narrow_width` | A long message with emoji near wrap boundaries flows as ONE paragraph; line count differs from the same text without emoji by at most 1 (never one line per emoji). Height = line count × the plain-text line height (no blank lines). |
| `every_emoji_placeholder_lands_on_one_line` | After wrapping at a narrow width, each placeholder span reports exactly one rectangle — the emoji box is atomic and never splits across lines. |
| `placeholder_advance_reserves_exactly_one_em_quarter_more` | The placeholder's measured advance ≈ 1.25 × text size, and its line height matches a plain run (21.75 for 15px @ 1.45) — no inflation. |
| `placeholder_line_metrics_match_plain_text` | A message with an emoji placeholder produces the same total height AND the same per-line baseline positions (`layout_runs().line_y`) as the same text without the placeholder — no vertical jitter. |
| `emoji_scale_is_sane_relative_to_text` | `EMOJI_TEXT_SCALE` is 1.25 and the emoji box is larger than the text size. |
| T17 plan tests (6) | Order, no-gap adjacency, Unicode round-trip, fallback, fast path. |

## Files changed

- `src/bin/boru/emoji/emoji_text.rs` — custom `EmojiText` widget,
  span-based paragraph layout, placeholder, paragraph cache, 11 tests
  (5 new T18 tests + 6 T17 plan tests kept).
- `src/bin/boru/emoji/mod.rs` — module map notes T18 hardening.
- `docs/emoji/wrapping-baseline.md` — this note.

## Verification

- `rb check --bin boru --features gui,video-playback,terminal` — exit 0.
- `rb check --lib --features gui,video-playback,terminal` — exit 0.
- `rb check --all-targets --features gui,video-playback,terminal` — fails
  ONLY on the documented pre-existing E0061 `DiscoveryService::join`
  5-arg family (`test_discovery_*`, `test_extensions_metadata`,
  `test_public_room_directory`); zero emoji-related errors.
- Targeted tests on debsrv: `emoji::emoji_text` (11 pass) and the full
  `emoji::` filter (104 pass, 0.06s).

## Follow-ups

- Composer inline rendering is BORU-TWEMOJI-19 (out of scope here).
- URL-segment rows in chat bubbles still use the pre-T18 `Row::wrap()`
  path for text segments; if mixed URL+emoji messages need the same
  inline-wrapping treatment, that is a follow-up (noted, not done here —
  the single-segment body path is the common case and is fully hardened).
