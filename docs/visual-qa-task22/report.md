# Boru PDF Task 22 visual QA

Date: 2026-09-11
Revision: `9425f905` (integrated Home dashboard)
Target: `boru` debug binary, `gui,video-playback,terminal`

## Evidence and environment

The integrated binary was built on DEBSRV with:

```text
rb build --bin boru --features gui,video-playback,terminal
```

The runtime captures were made against the real Boru Home application, not a
mock or component preview. Each run used a fresh remote Xvfb display with
software rendering and:

```text
boru --no-relay --no-dht --bind-port 0 --data-dir <fresh-dir>
```

The no-relay/no-DHT setup intentionally provides a deterministic offline
connection state. mDNS discovery still exposed two discovered peers in the
sidebar during the captures; no real chat room, file catalogue, or active
tunnel was available.

## Viewport matrix

| Viewport | Evidence | Result | Notes |
|---|---|---|---|
| 1920x1080 | `evidence/home-1920x1080.png` | PASS | Wide layout retains the protected sidebar/hero/status hierarchy and does not stretch cards to the full display. |
| 1672x941 | `evidence/home-1672x941.png` | PASS | Intermediate wide layout remains balanced and readable. |
| 1440x900 | `evidence/home-1440x900.png` | PASS | Desktop layout preserves aligned hero and lower-card columns. |
| 1280x800 | `evidence/home-1280x800.png` | PASS | Reference desktop viewport; all primary Home controls render and remain legible. |
| 1024x720 | `evidence/home-1024x720.png` | PASS | Smallest supported matrix viewport; content uses vertical flow/scrolling rather than a new horizontal layout. |
| 800x600 | `evidence/home-800x600.png` | UNSUPPORTED | Captured to document behavior below the documented 1024x720 minimum. The narrow surface shows pressure/clipping and is not an acceptance viewport. |

The PNG dimensions were verified with `file`; they exactly match the requested
viewport dimensions. Xvfb has no physical compositor/DPI scaling, so 125%,
150%, and 200% OS scaling could not be exercised. Logical viewport coverage is
provided above; native DPI scaling remains environment-limited.

## Visual inspection

- Hierarchy is recognizable at every supported viewport: BORU sidebar, hero,
  Quick Actions, and connection/status card remain the dominant regions.
- Sidebar width, hero treatment, BORU branding, and status-card structure remain
  consistent across the supported captures. No protected-region theme or
  geometry change was observed relative to the integrated foundation.
- At 1280x800 and 1440x900, panel headers and tile columns are aligned and tile
  heights are consistent. Avatars and timestamps/metadata present in the
  discovered-peer rows remain readable.
- At 1024x720, primary text and actions remain readable. The lower Home content
  is under height pressure but is handled by the main vertical flow instead of
  a fixed-height nested panel.
- No horizontal overflow or clipped labels were observed in the supported
  1024px-and-wider captures. The 800x600 capture is explicitly below the
  documented minimum and is recorded as unsupported rather than treated as a
  regression.
- A keyboard-Tab capture is retained at
  `evidence/home-1024x720-focus.png`. The run completed without a crash; a
  strong focus ring was not consistently discernible in the software-rendered
  screenshot, so focus contrast is recorded as environment-limited rather than
  claimed as a full PASS.

## State coverage and limitations

| Surface/state | Result |
|---|---|
| Empty/connecting Home | PASS; captured in all supported viewports |
| Discovered-peer sidebar rows | PASS; two mDNS-discovered rows were visible |
| Populated Chat messages | NOT RUN; no peer-seeded room in isolated capture |
| Long filenames / file table | NOT RUN; no populated file catalogue |
| Active tunnel actions | NOT RUN; no live tunnel |
| Scrolling | PASS for supported short-height Home flow; no clipped primary action observed |
| Keyboard focus | PARTIAL; Tab run captured, focus styling not reliably visible under Xvfb |
| 100/125/150/200% scaling | 100% logical scale only; physical scaling unavailable under Xvfb |

The missing populated Chat/Files/Tunnels states require a seeded live desktop
session and are not substituted with mocked data. Existing responsive layout
regression coverage was run as the structural companion check:

```text
rb test --bin boru --features gui,video-playback,terminal -- layout_regression
12 passed, 0 failed
```

## Conclusion

The integrated Home dashboard passes the supported logical viewport visual
matrix with recognizable hierarchy, readable controls, and no P1/P2 responsive
defect found in the available real-app evidence. The only visible pressure is
below the documented minimum at 800x600. Physical DPI scaling and populated
Chat/Files/Tunnel interactions remain follow-up evidence gaps, not verified
claims.
