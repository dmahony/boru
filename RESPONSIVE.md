# Inline video controls: responsive layout

`BoruVideoFileCard` keeps one player/control component for every video shape. The
media frame is sized first from intrinsic dimensions with `ContentFit::Contain`;
control visibility and density never participate in that calculation, so showing
or hiding controls cannot change the frame dimensions.

The control row selects `Compact` when the intrinsic video is portrait or the
resolved frame is under 360 px wide. Compact mode keeps the seek bar above an
icon-only play/pause, timing, volume/mute, and overflow row and omits the
permanent volume slider. Landscape and square frames use the same row in
`Regular` mode; square spacing contracts naturally with the frame width.

All icon actions remain focusable and retain the existing `AppMessage` playback,
seek, mute, volume, and expand handlers. Aspect ratio remains exact; no crop or
stretch is introduced.
