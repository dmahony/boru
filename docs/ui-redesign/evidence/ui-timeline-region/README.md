# t_232df918 — Message timeline as the sole expanding scrollable region

Refactor goal: the message timeline is the only element that expands and
scrolls vertically between the fixed conversation header and the pinned
composer. No giant dead areas.

## Behavior

- The timeline scrollable keeps a Fill height inside the chat panel column:
  `header (fixed 60px) -> divider -> timeline (Fill) -> composer (pinned)`.
- When the message content is shorter than the viewport, a leading spacer
  pushes the content to the bottom of the timeline so it hugs the composer
  (chat convention — Telegram/Signal/WhatsApp). Whitespace sits ABOVE the
  messages where it reads as balanced, instead of a giant dead area below the
  last message.
- When content overflows the viewport the spacer is zero and the existing
  anchored-to-bottom virtualized scrolling takes over unchanged.
- The spacer derives from the existing incremental layout cache
  (`LayoutCache::total_height`) and the live scrollable viewport height
  (`Scrolled` event), so it tracks message growth exactly and never interferes
  with reading position once the timeline overflows.

## Files

- `t_232df918_empty_1280x800.png` — empty conversation; region stretches,
  composer pinned.
- `t_232df918_short_1280x800.png` — 3 real messages, bottom-aligned.
- `t_232df918_short_1024x720.png` — same at the alternate viewport.
- `t_232df918_long_1280x800.png` — 40 real messages, scrollbar present.
- `t_232df918_scrolled_1280x800.png` — scrolled up; header/composer fixed.
- `verification.json` — pixel-analysis geometry for every capture.
