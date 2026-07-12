# Typography & Text Layout Audit — Boru Chat GUI

**Date:** 2026-07-09
**Scope:** `examples/iced_chat/app.rs` (2609 lines) — chat list screen + chat room screen
**Status:** Review only — no code changes in this task

---

## 1. No Typographic Scale

**Severity:** P1

Every text node uses an ad-hoc `.size(N)` with no systematic ratio or semantic naming:

| Size | Used For | Notes |
|------|----------|-------|
| 24 | Chat list title | Correct as primary heading |
| 20 | Help overlay heading | OK for modal title |
| 18 | Chat screen room name | Heading-level but smaller than chat list |
| 16 | "Recent Chats" / "Online Friends" section headers + button emoji | Mixed use |
| 14 | Room names, online friend labels, empty-state text, button labels | Body text default |
| 13 | Error text | Single-use, no scale slot |
| 12 | Online indicator emoji (🟢) | Emoji sizing |
| 11 | Identity info, preview text, topic/status info, secondary labels | Primary metadata size |
| 10 | Instruction text, ticket, relay/transport info | Secondary metadata |

No defined type ramp (minor second, major second, perfect fourth, etc.). Sizes are chosen per-location rather than composed from a system.

**Fix:** Define a 5-step type ramp: `xs` (10–11), `sm` (12–13), `md` (14–16), `lg` (18–20), `xl` (24). Map every surface to a step, never use a raw integer.

---

## 2. No Custom Typeface

**Severity:** P1

Iced's default system font is used throughout (Noto Sans on Linux, Segoe UI on Windows, SF Pro on macOS). This means:

- No consistent brand identity across platforms
- Possible metrics mismatch (line-height, letter-spacing) between OS defaults
- No weight variation — all text at default `Normal` (400)

**Fix:** Load a single custom typeface with at least two weights (Regular 400 + Medium 500 or Semibold 600). Recommended: Inter (good Latin coverage, multiple weights, works at small sizes). Alternative: JetBrains Mono for a terminal/P2P-adjacent vibe.

---

## 3. Message Body Uses `text_editor` Widget

**Severity:** P1

Lines 2391–2421: each `ChatEntry` body is rendered with `iced::widget::text_editor`, which is designed for **editable rich text**. The extra features (caret blinking, input handling, cursor movement, selection) are entirely wasted on read-only chat messages. This adds:

- Per-message `text_editor::Content` objects stored in every `ChatEntry` (struct padding, clone cost)
- `TextEditorAction` messages dispatched for every click/selection on any message
- Syntax/key-binding handling for something the user can't edit
- A transparent text editor that's still a text editor underneath

**Fix:** Replace `text_editor` with `iced::widget::text` with selectable text support (`iced::widget::text::selectable = true` if available in iced 0.14, or a simple `Text` widget). The `content` field in `ChatEntry` and the `TextEditorAction` message can be removed entirely.

---

## 4. Color Contrast Failures

**Severity:** P0 (accessibility)

All colors in the RGB sRGB gamut. WCAG AA requires ≥4.5:1 for body text, ≥3:1 for large text (≥18px or bold ≥14px).

| Location | Color | Ratio on White | Verdict |
|----------|-------|:-:|:-:|
| System message body | `rgb(0.45, 0.45, 0.45)` #737373 | ~4.0:1 | **FAIL AA** |
| System message label | `rgb(0.5, 0.5, 0.5)` #808080 | ~3.5:1 | **FAIL AA** |
| Remote message label | `rgb(0.0, 0.4, 0.8)` #0066CC | ~4.1:1 | **FAIL AA** |
| Remote message body | `rgb(0.2, 0.2, 0.2)` #333333 | ~6.8:1 | PASS |
| Local message body | `rgb(0.0, 0.55, 0.0)` #008C00 | ~5.2:1 | Borderline |
| All secondary/gray text (identity info, previews, empty-state, tips, etc.) | `rgb(0.5)` #808080 | ~3.5:1 | **FAIL AA** |
| Placeholder text | Default text_input | Depends on theme, likely <3:1 | Suspect |

**Fix:** Bump gray text to at least `#5C5C5C` for ~4.5:1. Bump remote label/title color to a darker blue (e.g. `#005299`). Push system body to `#595959` or darker.

---

## 5. No Message Bubbles / Visual Containers

**Severity:** P1

Messages appear as bare text lines with no background, border, or container. This makes it hard to visually parse who said what, especially in a fast-moving conversation. The only differentiation is:
- Label text: `[nickname]` in the message type's color
- Body text: in the message type's color
- System messages: no `[...]` prefix but same layout

No visual grouping, no alignment (left for remote, right for local), no timestamp.

**Fix:** Add a subtle chat bubble container per message — light tinted background for remote, slightly different tint for local, plain text for system. Different from the AI-cliché gradient-card approach — use a thin solid border at 0.5 opacity of the type color, or a minimal filled rectangle at very low opacity (0.04–0.06).

---

## 6. No Maximum Line Width

**Severity:** P2

Chat log content fills `Length::Fill` on width, meaning on a wide monitor a message could span 1500+ pixels. WCAG and readability research recommends 65–75 characters per line.

**Fix:** Cap reading width on the chat log to ~70ch. Center or left-align the constrained container.

---

## 7. Spacing Inconsistencies

**Severity:** P2

| Surface | Container Padding | Entry Spacing |
|---------|:-:|:-:|
| Chat list | 16 | 8 |
| Chat screen | 8 | 4 (overall), 2 (log entries) |
| Help overlay | 16 | 4 |
| Room row | 8 (inner) | 2 |

Different surfaces use different base units. This gives an inconsistent feel when switching between screens.

**Fix:** Pick a single spacing unit (4px or 8px) and use a consistent multiplier system (e.g. 4, 8, 12, 16, 24). Map all spacing calls to named constants.

---

## 8. No Dark Mode Customization

**Severity:** P1

Dark mode is handled by `iced::Theme::Dark` (main.rs:489) — the framework default. This means:
- Hardcoded RGB colors in `view_chat_log()` don't adapt for dark backgrounds (e.g. `(0.0, 0.4, 0.8)` blue may be too bright on dark)
- Gray `(0.5, 0.5, 0.5)` text on dark backgrounds may be too low contrast too
- No custom dark palette

**Fix:** Extract all message colors into theme-aware functions that return different values for light vs dark. Iced's `theme` parameter in `.style()` closures provides the current theme to choose from.

---

## 9. Emoji as UI Icons

**Severity:** P2

Emoji characters are used as icons: `➕`, `🔗`, `🟢`, `💬`, `◀`, `☀`/`🌙`, `📎`, `➤`, `❓`, `❌`, `✕`. This means:
- Rendering varies by OS/system emoji font
- Emoji and UI text fonts may not align vertically
- No color control — emoji has baked-in colors that may clash with theme
- No hover/active state changes

**Fix:** Replace with simple text characters (Unicode geometric shapes, arrows) or draw minimal icon equivalents as `iced::widget::text` with the UI font. Reserve emoji for user content (reactions, status).

---

## 10. Topic/Ticket Visual Overload in Header

**Severity:** P2

The chat header (lines 2323–2347) packs:
- Back button + room name + theme toggle
- Topic hex string + identity + direct/relay counts
- Relay mode + transport notice
- Optional ticket string (potentially 100+ chars)

This creates information overload with no visual hierarchy. The topic hex string is 64 hex characters — it dominates the second line and is useless for identification.

**Fix:** Move purely diagnostic info (topic hex, relay mode, ticket) behind a collapsible "details" expander or into a debug-only panel. Keep the header showing only: room name, back button, connection status dot, theme toggle.

---

## Summary of Required Changes

| # | Area | Component | Fix |
|---|------|-----------|-----|
| P0 | Accessibility | Message colors | Fix contrast ratios (system, remote label, gray text) |
| P1 | Typography | All | Define and apply a 5-step type scale |
| P1 | Typography | All | Load a custom typeface with 2+ weights |
| P1 | Widget choice | Message body | Replace `text_editor` with `text` widget |
| P1 | Layout | All | Extract colors into theme-aware functions |
| P1 | Layout | Chat log | Add message bubble containers |
| P2 | Layout | All | Cap reading width at ~70ch |
| P2 | Layout | All | Normalize spacing to a consistent unit system |
| P2 | Icons | All | Replace emoji icons with text/simple symbols |
| P2 | Information | Chat header | Consolidate debug/diagnostic info behind expander |
| P2 | Font | Emoji reactions | Ensure consistent vertical alignment |
