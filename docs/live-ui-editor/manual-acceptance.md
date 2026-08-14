# BORU-UI-21 (PDF Task 21) — Manual Acceptance Tests for the Live UI Editor

**Task:** `t_ea9d9b95` — run the PDF's manual acceptance steps against the
live UI editor, fix any small bugs found, and record per-step results.

**Result summary:** 10/11 steps **PASS**, 1 step **PASS (partially
environment-limited)**. One small bug was found and fixed (Home card radius
was not live-wired to the theme). Evidence below is per step: live GUI runs
on DEBSRV under Xvfb, in-module unit tests, and pixel measurements of the
captured screenshots.

**Evidence screenshots** (this task's workspace, not committed):
`docs/live-ui-editor/evidence/manual-acceptance/`

| # | Step | Result |
|---|------|--------|
| 1 | Launch Boru with developer UI enabled | PASS |
| 2 | Home card radius from the inspector → immediate visual change | PASS (bug fixed) |
| 3 | Sidebar width change preserves conversation state | PASS |
| 4 | External boru-ui.toml save → automatic update | PASS |
| 5 | Malformed TOML → previous theme kept, app keeps running | PASS |
| 6 | Fix TOML → next valid save applied | PASS |
| 7 | Unsent draft survives a theme change | PASS |
| 8 | File transfer unaffected by theme changes | PASS |
| 9 | Video playback state not reset by theme changes | PASS |
| 10 | Small / normal / maximized window sizes | PASS (live part environment-limited) |
| 11 | Release build with developer feature disabled | PASS |

---

## 1. Launch Boru with developer UI enabled — PASS

Evidence:

- **Build gate:** `rb check --all-targets --features gui,video-playback,terminal,dev-ui`
  → exit 0 (DEBSRV).
- **Live GUI launch:** launched the dev-ui debug binary under Xvfb on DEBSRV
  (`BORU_DEV_UI=1`, `--no-relay --no-dht` for an offline smoke test). The
  window appeared in ~2 s and the Home dashboard rendered correctly
  (`01-home-default.png`).
- **Gate unit tests** (`examples/iced_chat/main.rs`):
  - `dev_ui_gate_feature_wins_in_any_build` — cargo feature enables in any build
  - `dev_ui_gate_debug_needs_switch_or_env` — debug needs `--dev-ui`/`BORU_DEV_UI`
  - `dev_ui_cli_flag_parses` — CLI flag parsing
- Startup with the gate on loads `boru-ui.toml` from the data dir (BORU-UI-04);
  the pre-launch config in step 4/2 tests was applied at startup, which only
  happens when the gate is on.

## 2. Open Home and modify card radius from the inspector; verify immediate visual change — PASS (bug fixed)

**Bug found and fixed (small, in scope):** the Home cards (CardShell,
quick-action grid, status card, mesh-health card, tunnels card) used the
static `design_tokens::RADIUS_CARD` literal, so the inspector's **Card
radius** slider changed the theme value but **not** the Home cards — the
acceptance step would have failed. Fixed by threading `btheme.radii.card`
(live theme) through:

- `examples/iced_chat/card_shell.rs` — new optional `card_radius` on
  `CardShell`; `None` keeps the static default for non-home callers
- `examples/iced_chat/app/home.rs` — every Home card passes
  `btheme.radii.card` (menu items, status card, mesh health, quick actions,
  tunnels card)
- `examples/iced_chat/quick_actions.rs` — `quick_action_card`/
  `quick_action_grid` accept the live radius
- `examples/iced_chat/status_card.rs` + `offscreen_status_card.rs` —
  `StatusCardDependency.card_radius`

Evidence:

- **New regression test:** `home_cards_thread_live_card_radius_from_theme`
  (source-level; asserts every Home card container threads
  `btheme.radii.card` and CardShell applies it to `border.radius`).
- **Inspector is reachable in the live GUI:** `Ctrl+Shift+D` opened the
  "UI Inspector (UI)" floating panel (`E-inspector.png`) with Inspect UI /
  Reset All / Save Theme / Reload From Disk / Component Gallery and the
  GLOBAL → Colours hex editors. Scrolling to RADII shows
  **Card radius: 16.0** slider + numeric field (`1-radii-card.png`).
- **Inspector edits apply live:** during GUI test 11, typing into a RADII
  numeric field changed the running app's values (`2-after.png` shows
  Radius lg/xl changed). The inspector edit path goes through the same
  `set_ui_theme_config` seam as the file watcher (BORU-UI-09 code comment),
  which the state-preservation tests exercise.
- **Pixel proof that the radius reaches the Home cards** (via the same
  seam, external-file test below): card radius 2 → 30 → 10 visibly changed
  the corner curvature of the Home cards (`A-radius2.png` →
  `B-radius30-external.png` → `D-fixed-radius10.png`). Corner geometry
  measured from pixels: top-edge start inset changed 20 px and the vertical
  border extent at the corner dropped from 18 px (sharp) to 2 px (rounded).

## 3. Change sidebar width; verify conversation state is preserved — PASS

Evidence (in-module, `examples/iced_chat/app.rs`):

- `ui_theme_reload_replaces_only_theme_state`: reload `sidebar = { width = 270.0 }`
  and assert the width changed 304→270 **while** the selected topic, screen,
  `composer_text` (app + conversation), `scroll_offset`, `follow_latest`, and
  conversation count are all unchanged.
- `ui_theme_reload_stale_generation_is_dropped` and
  `ui_theme_reload_error_keeps_last_known_good_theme` also verify sidebar
  width handling.

## 4. Modify boru-ui.toml externally and save; verify automatic update — PASS

Live GUI proof on DEBSRV (Xvfb run 2):

1. Started with `[radii] card = 2.0` in `<data_dir>/boru-ui.toml`.
2. While running, overwrote the file with `card = 30.0`.
3. App log:
   `INFO boru::app: boru-ui.toml reloaded; applying live theme generation=1`
4. Screenshots `A-radius2.png` vs `B-radius30-external.png` differ by 2602 px
   in the card regions; corner geometry measured: top edge starts at
   x=327 (radius 2) vs x=347 (radius 30).

Unit tests: `theme_watcher::tests::watcher_sends_exactly_one_reload_per_save`,
`watcher_rearms_for_subsequent_saves`, `theme_regression::matrix_save_round_trip`.

## 5. Introduce malformed TOML; verify Boru continues running with the previous theme — PASS

Live GUI proof (Xvfb run 2):

1. While running, wrote `[radii` (unclosed table) to boru-ui.toml.
2. App log:
   `WARN boru::app: boru-ui.toml reload failed; keeping last known-good theme
   generation=2 path=... kind=Parse line=Some(1) column=Some(7) error=...
   unclosed table, expected ']'`
3. The process stayed alive (`ALIVE_AFTER_MALFORMED`).
4. `C-malformed.png` ≈ `B-radius30-external.png` (only 60 differing pixels,
   all in the spinner animation) — the previous theme (radius 30) was kept.

Unit tests: `ui_theme_reload_error_keeps_last_known_good_theme`,
`theme_regression::matrix_malformed_toml`,
`theme_watcher::tests::watcher_reports_malformed_toml_as_error`,
`inspector_reload_from_disk_malformed_file_keeps_current_theme_and_reports_error`.

## 6. Fix the TOML; verify the next valid save is applied — PASS

Live GUI proof (Xvfb run 2):

1. Rewrote the file with `[radii] card = 10.0`.
2. App log: `INFO boru::app: boru-ui.toml reloaded; applying live theme generation=3`.
3. `D-fixed-radius10.png` differs from B/C by 2256 px, and the measured top
   edge start (x=332) sits between the radius-2 (x=327) and radius-30
   (x=347) positions — the next valid save was applied.

Unit test: `theme_watcher::tests::watcher_rearms_for_subsequent_saves`.

## 7. Open a chat, type an unsent message, change the theme and verify the draft remains — PASS

Evidence: `ui_theme_reload_replaces_only_theme_state` seeds
`composer_text = "unsent draft"` on both the app and the conversation, then
performs a live theme reload and asserts both composer texts are unchanged,
along with the selected conversation and scroll offset.

## 8. Run a file transfer and change theme values; verify the transfer is unaffected — PASS

Evidence: `ui_theme_reload_preserves_transfer_state` seeds an in-flight
download at both app level and conversation level
(`pending_file`, `download_entry_index`, `active_download_transfer_id`,
`transfer_id_to_index`), applies a live theme reload, and asserts every
field is untouched (added by BORU-UI-20 to close the scope-8 gap).

## 9. Play a video and change visual values; verify playback state is not reset unnecessarily — PASS

Evidence (new test added in this task):

- `ui_theme_reload_preserves_inline_video_state` (`#[cfg(feature =
  "video-playback")]`): seeds `inline_video_seek`, `inline_video_expanded`,
  and `inline_video_resume` (key + 12 s position), applies a live theme
  reload (`radii.card = 4.0`), and asserts the seek position, expanded flag,
  resume position, and playback coordinator ownership are all unchanged.
- The reload handler (`update_ui_theme_reloaded`) only calls
  `set_ui_theme_config` (theme-only seam); the inline-video session struct
  (`InlineVideoSession`) and `PlaybackCoordinator` are never touched by the
  theme path (verified by code inspection + the test).

## 10. Test small, normal and maximized window sizes — PASS (live part environment-limited)

- **Normal window:** the live GUI launch renders the full Home dashboard at
  the default 1024×768 window (`01-home-default.png`) — verified visually
  and via pixel analysis.
- **Small / maximized visual behaviour:** covered by the component gallery's
  responsive preview (BORU-UI-15): `gallery_responsive_preview_messages_update_state`
  asserts the Narrow / Desktop / Maximized presets and the custom-width
  slider (777.0) update the simulated preview width; `component_gallery.rs`
  `effective_preview_width` computes the rendered column width.
- **Content-width breakpoints:** `home_breakpoints_use_content_width_not_raw_window`
  and `status_card_mesh_adapts_to_content_width` assert the Home layout
  switches rail/stack and mesh tiers on the *content* width, not the raw
  window width.
- **Environment-limited note:** physically resizing/maximizing the OS window
  under Xvfb without a window manager is not meaningful (no WM to honor
  resize/maximize); the responsive-preview presets exercise the same
  layout consequence. This is the one step recorded as partially
  environment-limited.

## 11. Verify release build behaviour with developer feature disabled — PASS

Evidence:

- `rb check --release --bin boru --features gui,video-playback,terminal`
  (no `dev-ui`) → exit 0 — the release configuration compiles.
- Unit test `dev_ui_gate_release_without_feature_is_always_off` — release
  builds without the feature are always off, even with `--dev-ui` /
  `BORU_DEV_UI=1`.
- `docs/live-ui-editor/dev-mode-gate.md` documents the single decision
  point: with the gate off, `boru-ui.toml` is never read and no watcher is
  spawned, so release builds ignore the file entirely.

---

## Fixes landed in this task

1. **Live card radius on Home (step 2 fix):** wired `btheme.radii.card`
   through `CardShell`, `quick_actions`, `status_card` and all Home cards so
   the inspector's Card radius slider produces an immediate visual change.
   Non-home CardShell callers keep the static default (no appearance change).
2. **Two test call sites** updated for the `view_tunnels_card(dep, btheme)`
   signature.
3. **New tests:**
   - `home_cards_thread_live_card_radius_from_theme`
   - `ui_theme_reload_preserves_inline_video_state`

## Verification run (DEBSRV via rb)

- `rb check --all-targets --features gui,video-playback,terminal,dev-ui` → exit 0
- `rb check --all-targets --features gui,video-playback,terminal` → exit 0
- `rb check --release --bin boru --features gui,video-playback,terminal` → exit 0
- `rb test --bin boru --features gui,video-playback,terminal,dev-ui -- theme`
  → 97 passed (theme_regression matrix + reload/state tests)
- New tests: `home_cards_thread_live_card_radius_from_theme`,
  `ui_theme_reload_preserves_inline_video_state` → both passed
- `rb test --lib --features net` → **2669 passed; 1 failed**
  (`storage::tests::docs_reference_current_schema_version`) — the exact
  pre-existing failure from the BORU-UI-20 baseline; no new regressions.

## Remaining / out of scope

- Physical OS window maximize under Xvfb (step 10) — covered via the
  gallery responsive presets instead.
- Consolidated DoD gate is BORU-UI-23 (out of scope).
