# Conditional player features (PDF tasks 13–14)

Date: 2026-08-08

## Evidence reviewed

- `PDF_SUMMARY.md` (tasks 13–14) requires the overflow menu and fullscreen
  only when existing functionality justifies exposing them; it explicitly
  forbids inventing new actions or a new playback architecture.
- `CONTROL_REDESIGN.md` records that the inline player has no fullscreen or
  action API to expose.
- `examples/iced_chat/video_file_card.rs` was inspected directly after
  merging the BORU-PLAYER-04 parent tip. The header already renders a
  focusable overflow control (`OverflowMenu::build`) and its existing menu
  contains real actions: Copy filename, Open downloads folder, Open file,
  Re-share, and Remove, selected according to transfer state.
- The inline player's far-right `Icon::More` control is already wired to
  `AppMessage::InlineVideoToggleExpanded` (`video_file_card.rs`, media-frame
  controls). It opens the existing expanded-player presentation; it is not a
  second action menu.
- A repository search found no fullscreen API, backend method, message, or
  window-mode action in the playback path. The only fullscreen hit was a
  documentation mention of Iced window events in an unrelated focus module.

## Decisions

### Task 13 — More menu: no additional menu added

The requested actions already exist in the video-card header overflow menu,
where they are available without duplicating controls inside the player. The
player-row `More` affordance is retained as the existing expand-player action,
not relabeled or repurposed as a menu. Adding a second menu in the control row
would duplicate the same actions and would invent no additional value. No code
change is required for this conditional feature.

### Task 14 — Fullscreen: not implemented

Fullscreen is not supported by the current `iced_video_player` playback
integration or Boru inline-player message path. This ticket therefore does not
introduce a new fullscreen architecture or icon. Fullscreen remains a separate
enhancement requiring an explicit backend/window integration design.

## Verification

The required debsrv check passed after the parent merge:

```text
rb check --example boru --features gui,video-playback,terminal
Finished `dev` profile [unoptimized + debuginfo] target(s) in 27.46s
```

The check emitted existing warnings but returned exit status 0.
