# Boru inline video player accessibility

BORU-PLAYER-07 (PDF tasks 17–18), 2026-08-08.

## Keyboard controls

The three inline media controls are `FocusableButton` widgets and participate in
Tab / Shift+Tab traversal. While any of those controls owns focus, the widget
handles only unmodified local key events and captures handled events there:

- Space / Enter: activate the focused control (play/pause, mute, or expand).
- Left / Right: seek backward/forward by five seconds.
- Up / Down: increase/decrease volume by ten percent.
- M: mute/unmute.

The arrow, volume, and M handlers are attached to the player controls rather
than the global `keyboard::listen()` subscription. Chat composer input and the
existing global shortcuts therefore keep receiving their normal keys; no player
key is captured when focus is elsewhere.

## Checklist

- Play/Pause accessible name: satisfied by the dynamic `Play video` / `Pause
  video` tooltip and control label.
- Mute/Unmute accessible name: satisfied by the dynamic `Mute` / `Unmute`
  tooltip and control label.
- Seek minimum, maximum, current position: satisfied by the `0.0..=1.0` seek
  slider, with its current fraction derived from the media position and a
  persisted drag value while seeking.
- Volume current value: satisfied by the `0.0..=1.0` volume slider, bound to
  the backend volume; the icon also distinguishes muted, low, and high volume.
- Visible focus states: satisfied by `FocusableButton`'s two-pixel focus ring.
- Status not conveyed by colour alone: satisfied by the textual status line
  (for example `Ready to play`, `Paused`, or `Downloading video…`) and the
  textual time readout; colour is supplementary.
- Auto-hide and keyboard focus: satisfied by `controls_focused`; the 2.8-second
  playing-only hide path refuses to hide while a control has focus and focus
  entry restores the controls and resets the idle deadline.
- Hit targets: satisfied by the padded icon buttons (`SPACE_6`) and the visible
  seek/volume slider interaction surfaces.
- UI scaling above 100%: controls use Iced layout lengths, Fill sizing, and
  existing spacing tokens rather than fixed-position pixel coordinates; the
  responsive compact layout removes the persistent volume slider in narrow
  frames without removing volume access.

The required verification command is:

```text
rb check --bin boru --features gui,video-playback,terminal
```
