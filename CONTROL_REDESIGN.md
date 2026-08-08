# Boru inline video controls redesign

The active player now uses a compact in-frame overlay: the seek control sits above one timing label and icon-only media actions. The previous duplicate duration badge and bordered Play/Mute text buttons are gone.

Design decisions:

- Existing `MediaFrameSizing`, `ContentFit::Contain`, and `VideoPlayer` paths are unchanged, so control visibility does not alter the media dimensions or aspect ratio.
- Controls reuse Boru's `Icon` and `IconSize` system, the existing focusable-button wrapper, spacing tokens, and the green accent through the native seek slider's existing interaction path.
- The control surface uses a limited transparent-to-dark gradient rather than an opaque bar; it is confined to the bottom overlay footprint.
- Volume remains available through the speaker icon's hover tooltip, keeping the horizontal slider out of the persistent portrait layout.
- Fullscreen and a true overflow action menu remain separate enhancements because the existing inline player has no fullscreen/action API to expose.
