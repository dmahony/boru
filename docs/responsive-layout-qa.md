# Responsive layout QA: DPI and display scaling

This is the DPI/scaling validation record for BORU-RESP-13 (PDF Task 13).
The application uses Iced's logical layout coordinates; OS display scaling changes
physical pixels per logical pixel rather than introducing a second breakpoint
system. The existing `ResponsiveLayout` tier resolver therefore remains the
single source of truth. High-DPI validation focuses on the effective logical
viewport and on controls that must remain reachable when physical space is
consumed by scaling.

## Environment and testability

The worker environment exposes an Xvfb display (`:144`) at 1440x1000 with a
single 1440x1000 mode and no desktop compositor. Its GNOME text scaling factor
is 1.0, and there is no physical monitor or OS display-settings session on which
to switch 100/125/150/175/200% scaling. Consequently, real compositor/DPI
runs are recorded as not available rather than being inferred from a headless
run.

The logical-viewport equivalents are testable and are covered by the existing
responsive regression suite. In particular, the 1024x720 and 1280x720 fixtures
exercise short logical windows, where dialog bodies are capped and scrollable,
while the wider/taller fixtures exercise normal and tall layouts.

## Scaling matrix

| OS scaling | Physical/compositor run | Logical-layout equivalent | Result | Notes |
|---:|---|---|---|---|
| 100% | Not available in Xvfb | 1024x720, 1280x800, 1920x1080 | PASS | Baseline tiers and default appearance are covered. |
| 125% | Not available in Xvfb | 1024x720 short-window fixtures | PASS | Dialog body cap keeps the footer outside the scroll viewport. |
| 150% | Not available in Xvfb | 1024x720 and 1280x720 short-window fixtures | PASS | Layout uses flow/scrolling; typography is not reduced. |
| 175% | Not available in Xvfb | 1024x720 minimum supported fixture | PASS | Sidebar width is clamped and primary actions remain in the dialog footer. |
| 200% | Not available in Xvfb | 1024x720 minimum supported fixture | PASS | Same safe logical layout; no additional breakpoint framework is used. |

"PASS" in the logical-equivalent column means the layout invariants are
verified by unit tests, not that a physical 125--200% compositor session was
observed. A follow-up desktop QA run should repeat this table on a real Linux
or Windows desktop with the OS scaling control changed at each value.

## Fixed-pressure surfaces checked

- **Dialogs:** all creation/settings/tunnel dialogs route their body through
  `IcedChat::dialog_body_max_height()`. The body is scrollable and the footer
  remains outside it, so the primary action cannot be pushed below a short
  scaled window.
- **Composer:** chat layout regression coverage pins the composer structure;
  controls use Iced flow layout and fill/shrink sizing rather than absolute
  coordinates.
- **Sidebar rows:** sidebar width is resolved and clamped by `SidebarLayout`,
  while rows remain in a scrollable column. The regression suite covers the
  supported viewport extremes.
- **File cards:** file/download cards use flow layout and content-fit media
  sizing. Their fixed dimensions are component-level media bounds, not window
  assumptions; text and actions remain in the surrounding flow.
- **Tables:** file-library table columns are part of the layout configuration;
  the files regression coverage checks the configured column structure rather
  than relying on raw window width.

No font sizes were reduced and no new breakpoint framework or view-local
viewport thresholds were added for this validation task.

## Verification commands

Run from the repository root:

```text
cargo fmt --all -- --check
rb test --example boru --features gui,video-playback,terminal -- layout_regression
rb check --example boru --features gui,video-playback,terminal
```

The targeted test and desktop-target check are the required automated gates;
real OS scaling remains a manual follow-up because the available display is
headless Xvfb.
