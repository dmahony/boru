# Timeline items evidence — t_ead7de5f

Reusable presentational components for the chat timeline (Figure 4 of the
Boru Modern UI implementation plan):

- `date_separator(label, theme)` — centered, muted, 12 px date divider.
  Accepts borrowed `&str` or owned `String` labels so freshly formatted
  dates can be passed without lifetime gymnastics.
- `system_event_chip(label, accent, body, theme)` — centered chip on a muted
  secondary surface with a 1 px accent border (accent at 45% alpha), compact
  12 px typography. The caller supplies the label + accent pair; the chip
  contains no classification logic.

Both live in `examples/iced_chat/ui_components.rs` (section 14) and are used
by the real chat log in `examples/iced_chat/app.rs` (date divider at
`view_chat_log`, system-event chip at `view_chat_log`). The developer
component gallery (`Ctrl+Shift+G`) shows a "Timeline (Figure 4)" section
rendering both components with sample data.

## Captures

- `t_ead7de5f_timeline_1280x800.png` — gallery scrolled to the Timeline
  section at 1280x800: "Today"/"Yesterday"/"Sunday, August 2, 2026"
  separators, MEMBER/NAME/HELP/NOTICE/INFO chips with original event text,
  plus the accent-input demo chips.
- `t_ead7de5f_timeline_1024x720.png` — same section at the alternate
  viewport.
- `t_ead7de5f_timeline_zoom_1280x800.png` — zoomed crop of the sample
  timeline.

## Harness notes (bare Xvfb, no window manager)

- `xdotool click --window` (synthesized window-relative wheel events) stops
  being processed by winit after a few events; the real pointer position +
  plain `xdotool click 5` scrolls reliably.
- iced's scrollable does not handle Page_Down synthetic keys.
- OCR detection for the section uses distinctive sample strings
  ("Kitchen", "Chat joined", "Invite sent") because "Figure 3 rail" is
  frequently misread as "Figure 4".
