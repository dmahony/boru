# Final Boru home-screen verification

Task: `t_a8f11aa3`

## Build

Verified the GUI release target on debsrv through `rb`:

```text
rb build --release --bin boru --features gui,video-playback,terminal
Finished `release` profile [optimized] target(s) in 13m 52s
exit code: 0
```

The repository target is the `boru` example at `examples/iced_chat/main.rs`; there is no `iced_chat` package/example target in the manifest.

## Runtime capture

The release binary was copied from debsrv slot 1 and launched locally under Xvfb with a fresh temporary data directory, `--no-dht --no-relay`, and the loopback MCP test interface. Each capture navigated to `chat_list` before importing the window surface.

| Capture | Surface |
|---|---:|
| `final_home_1600x900.png` | 1600 × 900 |
| `final_home_maximized_1920x1080.png` | 1920 × 1080 |
| `final_home_1024x720.png` | 1024 × 720 |
| `final_home_800x600.png` | 800 × 600 |

All four PNG dimensions were verified with ImageMagick. OCR spot checks show the expected Boru branding, greeting, hero/connection state, Mesh Health card, sidebar, and (at wide sizes) right-hand status rail.

## Visual QA

- The screen remains recognizably the Boru home screen with the existing sidebar, greeting, connection hero, Mesh Health, quick actions, and right rail.
- The hero is compact at desktop widths and retains its connection message, explanatory subtitle, and mesh illustration. At 800 px it wraps the status title naturally without clipping.
- The Mesh Health card places the current connection state before the counters and recent events.
- Desktop and maximized captures have balanced two-column layout, consistent 16 px card geometry, subdued borders, and readable secondary actions.
- The 1024 px capture transitions to the single-column main flow without overlap; the lower quick-action cards continue below the viewport as expected for a scrollable page.
- The 800 px capture keeps the sidebar and main content usable, wraps the hero title/subtitle cleanly, and keeps toolbar controls visible. Lower content is below the viewport rather than clipped horizontally.
- No obvious rendering failure, horizontal overflow, or malformed long filename/activity wrapping was observed.

The captures are intentionally evidence of a fresh, offline startup state (`Connecting — waiting for peers...`); no network peers or persistent user data were used.
