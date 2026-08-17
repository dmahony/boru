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

## BORU-SSUI-14 (PDF Task 14): Final cleanup

- **Dead code:** the superseded raw blue-button sender layout was already
  replaced inline by SSUI-02..12 (source text buttons → source cards, preset
  buttons → segmented control, Audio On/Off button → switch, raw stop button →
  destructive action). `rb clippy` finds ZERO findings in every SSUI-touched
  file (chat.rs, screen_share_ui.rs, screen_share_surface.rs, theme.rs,
  theme_config.rs, theme_merge.rs, ui_components.rs, form_components.rs,
  icon_system.rs, app.rs) with both the desktop feature set
  (`gui,video-playback,terminal`) and `+screen-sharing`. All `ScreenShareTheme`
  / `ScreenShareConfig` / merge fields are consumed by view code; the old
  `screen_share_w/h` viewer geometry tokens remain in use by the viewer branch.
- **cargo fmt:** chain-added regions are fmt-clean (applied targeted rustfmt
  output to the ~20 hunks that fell in SSUI-added lines). The remaining
  repo-wide drift is pre-existing.
- **clippy:** passes with pre-existing warnings only once
  `clippy::redundant_comparisons` is allowed. That single deny-by-default error
  is in `src/screen_share/adaptation.rs:190` (`stats.rtt_us > 0 && ...`), added
  by BORU-SS-39 on 2026-08-15 and present on origin/main at d65de253 — NOT a
  regression from this chain and inside the task's explicitly out-of-scope
  `src/screen_share/` directory; recorded as a follow-up (clippy 1.97 lint,
  trivially fixable as `stats.rtt_us > RTT_PRESSURE_US`).
- **Tests/build:** `rb test --bin boru --features gui,video-playback,terminal,screen-sharing -- screen_share_`
  → 36 passed, 0 failed (same suite as SSUI-13). `rb build --bin boru --features
  gui,video-playback,terminal,screen-sharing` passes.
- **Before/after screenshots (1280×800 each):** `evidence/screen-share-ui-before.png`
  renders the pre-SSUI raw blue-button sender panel from commit d65de253 (built
  in a throwaway worktree with a temporary capture test, then removed); the
  before panel shows blue pill-buttons, a "Source:" text row, preset buttons and
  an "Audio Off" text button. `evidence/screen-share-ui-after.png` is the
  SSUI-13 capture at the same 1280×800: source cards with icons + dimensions,
  a segmented quality control, remote-control status and audio switch, and a
  right-aligned destructive Stop Sharing inside the shared card shell.
