# Inline video geometry verification

BORU-PLAYER-06 (PDF tasks 15–16) audit, 2026-08-08.

## Geometry contract

`BoruVideoFileCard::media_frame` computes one `MediaFrameSizing` from the
attachment's intrinsic dimensions and the responsive card band. The poster
stack and the active `VideoPlayer` both use that same fixed width and height.
The media itself uses `ContentFit::Contain`, so the source aspect ratio is not
stretched, squashed, or cropped. The control stack is an overlay with
`Length::Fill` inside the already-sized frame; it never participates in frame
measurement.

Controls are rendered in the same full-frame `Stack` whether visible or hidden.
When hidden, only the control content is replaced by an empty spacer. The
outer frame, video element, and overlay therefore retain identical dimensions
for poster/playing, controls shown/hidden, and paused/playing states. The
bottom gradient is part of the control overlay and disappears with it.

## Aspect-ratio matrix checked

The ratio-preserving sizing tests cover:

- 1920×1080 (16:9)
- 1280×720 (16:9)
- 640×480 (4:3)
- 1080×1080 (1:1)
- 1080×1920 (9:16)
- 720×1600 (tall portrait)
- 2560×1080 (ultrawide)
- 6720×2880 (21:9)
- 7680×2160 (32:9 panorama)
- 1080×1200 and 1200×1080 (near-square unusual ratios)
- unknown metadata (safe 16:9 placeholder fallback)

The matrix is exercised at both wide and narrow chat widths. Each case asserts
an intrinsic-ratio match and bounded dimensions; narrow cases additionally
assert no horizontal overflow. The same deterministic `MediaFrameSizing` value
is used for poster and player, proving the control redesign does not alter
media geometry.

## Container styling audit

The shared media boundary uses:

- 13 px radius (within the requested 12–14 px range)
- fixed near-black `MEDIA_FRAME_BACKGROUND`
- one-pixel, low-alpha boundary only (not a thick border)
- `.clip(true)` only on the media-frame boundary, to keep rounded media edges
  contained
- no shadow, nested media frame, or permanent bottom toolbar

Verification command:

```text
rb check --bin boru --features gui,video-playback,terminal
```

Result: passed. Existing compiler warnings remain unrelated to this player
geometry audit.
