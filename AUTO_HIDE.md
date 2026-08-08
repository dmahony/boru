# Boru inline video controls — auto-hide state machine

Implements PDF task 10 (auto-hide), task 11 (click-to-toggle, no control
propagation), task 12 (centre play overlay), and the PDF task 18 /
AC9 keyboard-focus exception for the Boru inline video player
(BORU-PLAYER-04).

## State

Each `InlineVideoSession` (examples/iced_chat/app.rs) carries:

- `controls_visible: bool` — whether the on-media controls bar is drawn.
- `controls_last_interaction: Instant` — last pointer/control interaction.
- `controls_focused: bool` — keyboard focus is currently inside the
  controls (the AC9 / PDF task 18 exception).

## Show triggers (all reset the idle deadline)

- Playback starts (session creation sets `controls_visible = true`).
- Pointer moves over / enters the video surface (`mouse_area::on_enter` /
  `on_move` -> `AppMessage::InlineVideoShowControls`).
- Seek slider changes / is released (`InlineVideoSeekChanged` /
  `InlineVideoSeekReleased`).
- Volume adjusted (`InlineVideoSetVolume`), mute toggled
  (`InlineVideoToggleMute`), expand toggled (`InlineVideoToggleExpanded`).
- Keyboard focus enters the controls (`InlineVideoControlsFocused(true)`),
  which also resets the idle deadline so the controls stay visible.

## Hide logic

In `AppMessage::InlineVideoTick` (app.rs), while the video is PLAYING
(paused playback never auto-hides):

```
if !video.paused() && !session.controls_focused
    && now - controls_last_interaction >= 2800 ms
{
    session.controls_visible = false;
}
```

- Hidden only after ~2.8 s of no interaction, while playing.
- Never hidden while paused (`!video.paused()` gate).
- Never hidden while keyboard focus is inside the controls
  (`!session.controls_focused` gate — PDF task 18 / AC9).
- The bottom gradient travels with the controls: the whole controls bar
  (including its translucent background) is replaced by `Space::new()`
  when hidden, so the gradient disappears and reappears with the controls.

## Keyboard-focus exception (AC9 / PDF task 18)

`FocusableButton` (examples/iced_chat/focusable_button.rs) gained an
optional `on_focus_change(focused)` callback. On every `RedrawRequested`
event it compares its tree `State::is_focused` against the last reported
value and publishes the callback exactly once per transition — the same
focus-tracking pattern iced's own `Stack` widget uses for
`is_top_focused`.

The three media control buttons (play/pause, mute, more) are built by
`media_icon_button` in video_file_card.rs, which wires
`.on_focus_change(|focused| AppMessage::InlineVideoControlsFocused(focused))`.
The app handler updates `session.controls_focused`, and when focus ENTERS
the controls it also forces `controls_visible = true` and resets the idle
deadline. While `controls_focused` is true the tick never hides the
controls, so keyboard users are never stranded with their focus target
removed from the tree.

Note: iced 0.14 `Slider` widgets are not keyboard-focusable, so the seek
and volume sliders cannot hold focus; the focusable surface is exactly the
three `FocusableButton` controls. Dragging a slider is still a pointer
interaction and resets the deadline via its change message.

## Interaction layering (task 11)

The playing surface is a stack:

1. `video_element` — `mouse_area` wrapping the `VideoPlayer`:
   - `on_press(PlayInlineVideo)` toggles play/pause (click video surface).
   - `on_enter` / `on_move(InlineVideoShowControls)` restores controls.
2. `controls_overlay` — the controls bar, drawn only when
   `controls_visible`; `Space` otherwise. Because it sits ABOVE the video
   element in the stack, clicks on the buttons/sliders hit the controls
   and do not propagate back to the video's `mouse_area` (no double
   toggle). When hidden it is an inert spacer, so clicks fall through to
   the video surface as intended.

## Centre play overlay (task 12, AC13)

The pre-playback centre play button (the redesigned `FocusableButton`
wrapped circular play glyph with tooltip label "Play video") is retained
in the poster branch. Once an inline session exists
(`self.player = Some`), the preview switches to the video + controls
stack, so the large centre control disappears when playback begins and
the small bottom controls take over.
