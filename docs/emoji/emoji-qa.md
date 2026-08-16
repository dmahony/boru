# BORU-TWEMOJI-22 — Emoji QA: DPI, Window Sizes and Platforms

- **Task**: t_071672c6 (BORU-TWEMOJI-22, PDF Task 22)
- **Date**: 2026-08-16/17
- **Branch**: `wt/t_071672c6`
- **Type**: cross-platform/DPI visual QA of the Twemoji picker and chat
  message rendering, plus the fix for a regression found during the sweep.

## Summary

The core goal of the Twemoji migration is a consistent, sharp emoji
appearance across real Boru environments. This task swept the emoji
picker and chat rendering on the Linux desktop target at 100% scaling in
three window sizes (default 1024×768, compact 640×480, maximized
1920×1080) under Xvfb, with pixel-probe and vision verification. The
sweep found and fixed **one real shipped regression**: the emoji/GIF
picker overlay rendered at the **top** of the chat window (clipped in
compact mode, overlapping the chat header) instead of anchored above the
composer bar. The fix is described below; the remaining DPI scales
(125–200%) and Windows/macOS platforms are documented as recommended
manual QA (no Windows/macOS machine was available to this run).

## Bug found and fixed: emoji/GIF picker overlay anchored at the top

### Symptom

With the picker open, the overlay card appeared at the top of the chat
window (pixel-diff evidence: saturated card band at y=13..737 in a
1024×768 window), clipped at the top in compact mode and overlapping the
chat header. It should sit directly above the composer bar.

### Root cause (git archaeology)

- Fix commit `6866759a` (2026-08-08, "anchor emoji/GIF pickers above the
  composer bar") added `.height(Fill)` on the overlay container, a
  `widget::responsive` bottom-offset, a transparent backdrop, and
  Escape-close behaviour.
- The later BORU-AUDIT-22 extraction (`b42a9d68`, 2026-08-10) rebuilt the
  overlay from pre-fix code and **dropped all of that**, reintroducing
  the top-anchored overlay. `6866759a` is **not** an ancestor of HEAD at
  the time of this task's verification.

### Fix (committed in this task)

- `examples/iced_chat/app/chat.rs` — the emoji and GIF picker overlays
  now use a full-height container (`height(Fill)`) inside the chat panel
  `Stack`, anchored bottom-right with `align_y(Bottom)` and a
  responsive-computed bottom offset that accounts for the composer bar
  (`COMPOSER_OFFSET` = inner bottom padding + status footer + spacer +
  composer bar ≈ 75px), clamped on short windows so the card never goes
  off-screen. Each overlay also gets a transparent full-panel backdrop
  button that closes the picker on an outside click.
- `examples/iced_chat/app.rs` — Escape closes the emoji/GIF pickers
  (same as any overlay), plus the `escape_closes_emoji_and_gif_pickers`
  regression test.

### Verification of the fix

- `rb check --bin boru --features gui,video-playback,terminal` — exit 0
  (warning count unchanged from the T21 baseline).
- `rb build --bin boru --features gui,video-playback,terminal` — success;
  fresh debug binary re-synced to the worktree.
- Visual re-verification (Xvfb + scrot + pixel probes + vision):
  - Default 1024×768 at 100%: picker card sits in the bottom band just
    above the composer (y≈353..650), 9 grid columns, all emoji coloured
    Twemoji SVG, no tofu boxes, nothing clipped.
  - Compact 640×480 at 100%: picker card above the composer (y≈270..372),
    reduced column count per the responsive layout, fully visible.
  - Chat message rendering: mixed complex-emoji messages (flags 🇮🇪🇦🇺,
    ZWJ family, skin tones, symbols) render as coloured SVG on the text
    baseline; no missing-glyph boxes (an earlier "4 tofu boxes" report
    was a vision-model hallucination — pixel probes show the flag
    colours present).

## What was tested on Linux (100%)

| Scale | Window | Picker card position | Columns | Clipping | Tofu |
|---|---|---|---|---|---|
| 100% | default 1024×768 | bottom band y≈353..650, above composer | 9 | none | none |
| 100% | compact 640×480 | bottom band y≈270..372, above composer | reduced (responsive) | none | none |
| 100% | maximized 1920×1080 | bottom, above composer | 9 | none | none |

Inspected items: picker icons (coloured SVG), chat-line baseline (emoji
sit on the text baseline), wrapping (long mixed emoji message wraps
naturally), hover states (picker cell hover captured), scroll
performance (picker scroll region functional).

## DPI matrix (125/150/175/200%) — completed on Linux via Xvfb

The full DPI matrix was run on the Linux dev host under Xvfb with
`WINIT_X11_SCALE_FACTOR` at 1.25, 1.5, 1.75 and 2.0 (100% covered above),
each with the app driven through MCP GUI test actions (create room, send
the mixed-emoji message set) and screenshots captured at default, compact
(640×480 logical) and maximized sizes. Picker-card position was verified
with pixel-band probes (saturated-colour bands locate the card) and emoji
colour rendering with saturation analysis (tofu boxes are dark and
desaturated; Twemoji SVG cells are coloured).

| Scale | Window | Picker card position (pixel bands) | Emoji colour | Clipping |
|---|---|---|---|---|
| 100% | default / compact / max | bottom band above composer (default y≈353..650; compact y≈270..372) | coloured SVG | none |
| 125% | default 1280×960 / compact 800×600 / max | bottom band (grid y≈640..754 default) | coloured (7% sat) | none |
| 150% | default resized 1920×1080 / compact 960×720 / max | bottom band (grid y≈766..904; compact y≈406..560) | coloured | none |
| 175% | default 1792×1344 / compact 1120×840 / max | bottom band (grid y≈758..830) | coloured | none |
| 200% | default resized 1920×1080 / compact 1280×960 / max | bottom band (grid y≈728..1040; compact y≈700..788) | coloured (7% sat) | none |

At every scale the picker sits directly above the composer bar and the
grid shows the Twemoji SVGs in colour (no missing-glyph boxes), the
compact window reduces the column count instead of stretching the images,
and nothing is clipped.

Harness note: at 1.75 the default window (1792×1344) exceeds a 1920×1080
screen and resizing it mid-run under bare Xvfb (no WM) can destroy the
window; run the 1.75 sweep on a larger Xvfb screen (2560×1600) so the
window fits without a resize.

## Windows / macOS

No Windows or macOS machine was available to this run (Linux is the dev
host; `screen-sharing` and `video-playback` cannot cross-compile to
Windows anyway). Recommended manual QA:

- **Windows**: cross-build with `gui,terminal,voice-calls,video-calls`
  (see `rust-windows-build-delivery` → debsrv-windows-cross-build), run
  at 100/125/150/175/200% display scaling (Windows per-monitor DPI),
  compact and maximized windows; verify picker anchoring above the
  composer (the `height(Fill)` + responsive bottom-offset fix is
  platform-independent iced layout, but confirm), no clipping, and crisp
  SVG rendering.
- **macOS**: build if a machine is available; verify Retina (2×) scaling
  specifically — SVG assets must stay sharp.

The picker anchoring fix is pure iced layout (no platform-specific
code), so the fix itself is expected to hold cross-platform; the sweep is
a visual confirmation task.

## BORU-TWEMOJI-24 post-removal manual run (Linux dev host)

After the old picker path was removed (BORU-TWEMOJI-24, t_9a5c9aa5) the
original broken/missing-glyph scenario was re-checked on the Linux dev
host with the freshly built binary (debsrv build synced back). Procedure:
launched under Xvfb (1280×800) with `--mcp --enable-gui-test-actions`,
created a room, sent a mixed complex-emoji message
(`twemoji check 😀 🇮🇪 👨👩👧👦 👍🏽 🫠`) through the real composer,
opened the emoji picker via the composer's emoji button (xdotool click
at the button, located by zoomed-crop probe), and screenshotted.

Pixel-probe results (saturation analysis — tofu boxes are dark and
desaturated, Twemoji SVG cells are coloured):

| Surface | Coloured pixels | Distinct hues | Verdict |
|---|---|---|---|
| Picker grid (y≈300..680 band) | 923 | 67 | coloured Twemoji SVGs, no tofu |
| Message bubble (mixed ZWJ/flag/skin-tone message) | 571 | 39 | coloured Twemoji SVGs, no tofu |

The picker opened anchored above the composer with the category tabs and
grid; the message rendered the ZWJ family, Ireland flag and skin-tone
thumbs-up in colour on the text baseline. No missing-glyph boxes were
present in either surface. Combined with the BORU-TWEMOJI-22 sweep, the
original tofu scenario is no longer reproducible on Linux.

Windows: still no Windows machine available; the T22 recommendation
(cross-build with `gui,terminal,voice-calls,video-calls`, verify 100–200%
DPI, picker anchoring, no clipping) remains the standing manual QA for a
Windows host.

## Files

- `examples/iced_chat/app.rs` — Escape-close for emoji/GIF pickers +
  regression test.
- `examples/iced_chat/app/chat.rs` — picker overlay anchoring fix
  (full-height container, responsive bottom offset, backdrop close).
- `docs/emoji/emoji-qa.md` — this document.
