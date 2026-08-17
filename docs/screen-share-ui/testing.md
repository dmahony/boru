# Sender screen-sharing UI testing (BORU-SSUI-13)

## Automated verification

- `rb test --bin boru --features gui,video-playback,terminal,screen-sharing -- screen_share_`
  - 36 passed, 0 failed.
  - Covers sender state-to-view mappings, source selection, quality presets,
    audio toggling, stop/idempotence, terminal-state inertness, viewer surface,
    shared primitives, and offscreen sender-card captures.
- `rb check --bin boru --features gui,video-playback,terminal,screen-sharing`
  - Passed. Existing warnings remain in unrelated screen-sharing and GUI code.
- `git diff --check`
  - Passed.

## Layout evidence

The existing offscreen capture tests passed for the sender card at the narrow
split, 1280 reference, 1920 maximized, light/dark, stopped-disabled, and long
peer-name variants. Source-card titles use the existing long-title ellipsis
coverage. These are deterministic render checks rather than a live desktop
session.

## Two-peer verification

Not run in this task workspace: no live Boru sender/viewer processes or
session-specific launch configuration were provided. The implementation keeps
the existing `ScreenShareSelectSource`, `ScreenShareSetPreset`,
`ScreenShareToggleAudio`, and `StopScreenShare` dispatch paths unchanged; a
manual two-peer run remains a release/QA follow-up before marking the complete
redesign acceptance checklist closed.

## Formatting note

`cargo fmt --all -- --check` reports a large pre-existing repository-wide
formatting drift (including files unrelated to this task). Formatting the whole
repository would create a broad unrelated diff, so this task keeps the focused
changes only.
