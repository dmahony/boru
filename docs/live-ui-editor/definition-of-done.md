# Boru Live UI Editor — Definition of Done (BORU-UI-23 / PDF Task 23)

Source: `Boru_Live_UI_Editor_Agent_Tasks.pdf` (Phase 9), sections **Goal**,
**Required End State**, **Task 23 (Recommended Implementation Order)**,
**Coding Agent Rules**, and **Definition of Done**. This is the final gate of
the BORU-UI-01..22 chain: every Required End State bullet is verified against
the merged codebase with code/docs/test evidence, the Coding Agent Rules are
checked for violations, and the full regression gate ran on DEBSRV.

Verification date: 2026-08-14. HEAD: BORU-UI-22 (`28a951d6`) + this task's
commit. Tree under test: `git fetch origin && git merge origin/main` (clean
fast-forward to `28a951d6`, the last UI-chain task). Worktree branch
`wt/t_69cc701c`.

## Implementation order (PDF Task 23) — chain reconstruction

The PDF's recommended order is "audit → theme → refactor → config → merge →
watcher → live redraw → dev gate → inspector → save/reset → gallery →
responsive → typography → colours → error reporting → perf → tests →
acceptance → layout". The chain landed in exactly that order, 32 commits on
origin/main:

| PDF Task 23 step | Task(s) | Commit(s) |
|---|---|---|
| 1. Audit visual constants | BORU-UI-01 | `417ea7cd` |
| 2. Introduce BoruTheme defaults | BORU-UI-02 | `90374e32` |
| 3. Refactor existing components | BORU-UI-03 (10 commits, one per visual area) | `a7deb558`..`a2c7885d` |
| 4. TOML parsing + default merging | BORU-UI-04, 05 | `3c3f5fcf`, `7af5281a` |
| 5. File watcher + live reload | BORU-UI-06, 07 | `adbf828c`, `d2c0e538` |
| 6. Developer feature gate | BORU-UI-08 | `05b21f14` |
| 7. Inspector basics | BORU-UI-09 | `207ea335` |
| 8. Save / Reload / Reset | BORU-UI-12, 13 | `2bd8dcac`, `1ce3dc96` |
| 9. Component Gallery | BORU-UI-14 | `f89c7ed5` |
| 10. Responsive preview + inspection mode | BORU-UI-15, 11 | `70798806`, `e5f4cc24` |
| 11. Tests and documentation | BORU-UI-16..22 | `c4d62a7c`..`28a951d6` |

The "no inspector before the typed theme layer" rule holds: the typed theme
layer (BORU-UI-02/03) and clean state boundaries (BORU-UI-04..07) landed
before any inspector work (BORU-UI-09+).

## Required End State — bullet-by-bullet evidence

| # | Required End State | Evidence (code/docs) | Evidence (tests) | Verdict |
|---|---|---|---|---|
| 1 | Boru continues to use Iced and does not require a UI framework rewrite | `Cargo.toml:183-187` — the only UI framework deps are `iced 0.14`, `iced_aw`, `iced_moving_picture`, `iced_video_player`, `iced_term`; no Dioxus/Slint/egui/tauri/yew anywhere in the manifest. All editor views are plain Iced widgets (`inspector.rs:38-41` imports `iced::widget::{button, container, pick_list, row, scrollable, slider, text, text_input, toggler}`; `component_gallery.rs:16-17` imports `iced::widget::{container, rule::horizontal, slider, text, Column, Row}`) | `theme_is_copy_clone` (`theme.rs:2189`) exercises `Copy` on the theme; the GUI bin compiles with `gui` feature (`rb check --bin boru` in §gate) | **PASS** |
| 2 | Visual constants are centralized in a typed BoruTheme / UiTheme model | `BoruTheme` root struct with 19 typed groups: `colors, typography, spacing, radii, icons, avatars, lists, borders, responsive, motion, sidebar, home, chat, attachments, rooms, tunnels, dialogs, calls, controls` (`theme.rs:1680-1719`); every group is a typed `Copy` struct with a `Default` matching the baseline UI (`theme.rs:620-623, 910-916, 970-975, 1076-1081, 1183-1186, 1263-1266`); `BoruTheme::dark()` / `for_theme()` select the mode (`theme.rs:1721-1738`); view code consumes `btheme` instead of raw literals (`app.rs:19613, 20883`; `app/home.rs:390-539` pass `btheme.spacing/radii/avatars/lists/home` tokens) | `light_palette_matches_design_tokens` (`theme.rs:1834`), `dark_palette_matches_design_tokens` (`:1870`), `semantic_colour_tokens_map_to_backing_fields` (`:1906`), `typography_matches_fonts` (`:1947`), `spacing_matches_design_tokens` (`:2045`), `radii_match_design_tokens` (`:2068`), `sidebar_geometry_matches_design_tokens` (`:2128`), `chat_geometry_matches_design_tokens` (`:2137`), `default_matches_audit_source_values` (`:2261`) | **PASS** |
| 3 | Development overrides can be loaded from a human-editable boru-ui.toml file | `boru-ui.example.toml` (repo root) documents the format — copy to `<data_dir>/boru-ui.toml`; every key optional; units/ranges documented (px floats, 0..=1 ratios, hex/rgba colours); all 24 config tables documented. `UiThemeConfig` mirrors `BoruTheme` 1:1 with `#[serde(default)]` + `Option` leaves (`theme_config.rs:695-717`); `load_ui_theme_config` reads the file, missing file → empty config (startup never fails) (`theme_config.rs:894-905`); `ColorValue` accepts hex `"#RRGGBB"`/`"#RRGGBBAA"` or float arrays (`theme_config.rs:104-147`) | `matrix_parse_complete_config`, `matrix_parse_partial_config` (`theme_regression.rs:126, 182`), `parse_full_config`, `parse_partial_config_missing_keys`, `color_value_hex_parsing`, `missing_file_returns_defaults` (`theme_config.rs:1091, 1155, 1266, 1249`), `older_partial_file_merges` (`theme_merge.rs`) | **PASS** |
| 4 | Saving boru-ui.toml updates the running UI automatically | `spawn_ui_theme_watcher` watches `<data_dir>` non-recursively (write/create/rename, catches atomic editor replacement) (`theme_watcher.rs:33-41`; main.rs:1841-1850); trailing-edge `Debouncer` (300 ms) collapses save storms (`theme_watcher.rs:41, 83-126`); parse happens off the render path; a normal `UiThemeReloadMsg` is sent into the update loop (`theme_watcher.rs:43-56`); app-side `update_ui_theme_reloaded` → `set_ui_theme_config` applies valid reloads (`app.rs:19054, 19023-19036`); stale generations dropped via `ReloadTracker` (`theme_watcher.rs:129-139`); never mutates state from the watcher callback | `watcher_sends_exactly_one_reload_per_save`, `watcher_rearms_for_subsequent_saves`, `watcher_reports_malformed_toml_as_error` (theme_watcher tests, BORU-UI-21 §4-6); `debouncer_fires_once_after_quiet_window`, `debouncer_generations_increase_per_burst`, `reload_tracker_drops_stale_results` (`theme_watcher.rs` tests); live-GUI evidence BORU-UI-21 step 4 (external save → `boru-ui.toml reloaded; applying live theme generation=1`, radius 2→30 pixel-measured) | **PASS** |
| 5 | A hidden developer UI Inspector can modify the same values with sliders, number fields, toggles and colour controls | `inspector.rs` toggled with Ctrl+Shift+D (`inspector.rs:1-9`); `FieldKind::{Float, Bool, Color, Choice}` drives control choice — sliders + numeric inputs for floats, togglers for bools, hex/RGBA text + swatch for colours, pick_lists for choices (`inspector.rs:50-62, 384-395`); field rows emit normal Iced `InspectorMsg`s (`SetFloat`/`SetBool`/`ColorTextChanged`/`SetChoice`) handled by `update_inspector` (`app.rs:19111-19112`); "the panel NEVER mutates view-local state" (`inspector.rs:11-15`); every exposed field maps to a real config leaf | `inspector_toggle_and_edit_updates_active_theme_via_messages` (`app.rs:36217`), `apply_float_sets_exact_config_leaf`, `apply_color_sets_exact_config_leaf`, `float_text_draft_applies_on_valid_parse`, `parse_hex_rgba_accepts_6_and_8_digit_and_rejects_garbage`, `every_exposed_field_maps_to_a_real_config_leaf`, `reads_match_writes_for_a_sample_field` (inspector tests); live-GUI: Ctrl+Shift+D opened the panel, RADII card radius slider + numeric field rendered (`manual-acceptance.md` step 2) | **PASS** |
| 6 | Inspector changes are reflected immediately in the live application | Every edit goes through `set_ui_theme_config` → `recompute_active_theme` → `theme_revision++` so the normal Iced state/update/view cycle redraws (`app.rs:19023-19036, 19005-19016`); `boru_theme()` is the single seam view code reads per frame (`app.rs:18909-18919`); live-GUI proof: typing into a RADII numeric field changed the running app's values immediately (`manual-acceptance.md` step 2, `2-after.png`) | `inspector_toggle_and_edit_updates_active_theme_via_messages` (asserts theme changed + revision bumped); `inspector_slider_edit_keeps_visual_feedback_immediate_and_defers_prewarm` (`app.rs:36278`); live pixel proof in `manual-acceptance.md` step 2 | **PASS** |
| 7 | The current theme can be saved back to boru-ui.toml | `InspectorMsg::SaveTheme` → `theme_config::save_ui_theme_config` serializes the current overrides to `<data_dir>/boru-ui.toml` (`app.rs:19163-19189`); write is **atomic** (temp sibling + fsync + rename via `atomic_write_bytes`) so the watcher never sees a partial file (`theme_config.rs:969-985`); stable field order keeps git diffs readable (`save_serialization_omits_none_and_keeps_stable_order`); success/failure shown in the panel status line | `inspector_save_theme_writes_boru_ui_toml_and_reports_status` (`app.rs:36450`), `inspector_save_theme_failure_sets_failed_status` (`:36503`), `save_round_trip_merge_reproduces_same_theme` (`theme_config.rs:1506`), `save_ui_theme_config_writes_file_atomically` (`:1585` — no `.tmp` leftover, reload round-trips), `matrix_save_round_trip` (`theme_regression.rs:376`) | **PASS** |
| 8 | A Component Gallery / UI Playground can display reusable Boru components in predictable states | `component_gallery.rs` renders production components: `CardShell`, `BoruDialog`, `Avatar`/`ListRow`/`PeerChipStack`, `view_download_progress`, `quick_action_card`, `badge`/`status_dot`/`primary_button` from `ui_components`, message bubbles, video cards (`component_gallery.rs:22-31, 128-132, 262-268, 1287-1302, 2023-2025, 2172-2175`); PDF Task 14 states covered by fixtures mapped to production `DownloadState` (Ready/Active/Completed/Error) + video aspect ratios 16:9/square/vertical; short/long names; empty/populated cards; narrow/normal/wide widths via responsive presets | `all_gallery_sections_build` (`component_gallery.rs:2566`), `attachment_fixture_states_cover_required_set`, `video_fixtures_cover_aspect_ratios`, `name_variants_gallery` renders short+long names (BORU-UI-14 tests); `gallery_presets_map_to_required_widths`, `gallery_custom_width_clamps_to_slider_range`, `gallery_effective_width_bounds_to_window` (`component_gallery.rs` tests) | **PASS** |
| 9 | The feature is disabled or excluded from normal release builds unless explicitly enabled | `dev-ui` feature is deliberately **not** in `default` (`Cargo.toml:258, 300-303`); gate precedence: feature wins in any build; debug build needs `--dev-ui` or `BORU_DEV_UI=1`; release without feature is always off (`main.rs:222-239, 247-254`; `docs/live-ui-editor/dev-mode-gate.md`); with the gate off, `boru-ui.toml` is never read and no watcher spawns (`main.rs:562-574, 1841-1850`); `inspector.rs` and `component_gallery.rs` are `#[cfg(feature = "dev-ui")]`-gated (`main.rs:12-13, 46-47`); no unauthenticated remote editing port exists (no network I/O in theme modules — see §Coding rules) | `dev_ui_gate_feature_wins_in_any_build`, `dev_ui_gate_release_without_feature_is_always_off`, `dev_ui_gate_debug_needs_switch_or_env` (`main.rs:2908, 2918, 2929`); release check without feature compiles (`rb check --release` in §gate) | **PASS** |
| 10 | No existing Boru functionality is coupled to the editor | Theme modules (`theme.rs`, `theme_config.rs`, `theme_merge.rs`, `theme_watcher.rs`) contain **zero** networking/protocol references (grep for gossip/iroh/subscribe/broadcast/Ticket/RelayMode in theme modules: empty); the reload seam `set_ui_theme_config` touches only `ui_theme_config` + `active_theme` + revision flags — never networking, gossip, rooms, tunnels, media, chat history, selected conversation, scroll or composer (`app.rs:19018-19036`); editor code is inert when gate is off; startup never depends on the dev file (missing → defaults, `theme_config.rs:894-900`); behavioural constants (protocol limits, timeouts) are explicitly excluded from the theme (`constants-audit.md` §4) | `ui_theme_reload_replaces_only_theme_state` (asserts selected topic, screen, composer_text, scroll_offset, follow_latest, conversation count all unchanged after a live reload) (`app.rs:35915`); `ui_theme_reload_preserves_transfer_state` (`:35972`); `ui_theme_reload_preserves_inline_video_state` (`:36050`); `ui_theme_reload_error_keeps_last_known_good_theme` (`:36090`); `ui_theme_reload_stale_generation_is_dropped` (`:36137`) | **PASS** |

All 10 Required End State bullets **PASS**.

## Definition of Done — demonstrated

> "a developer can run Boru once, open the UI Inspector or edit
> boru-ui.toml, change visual properties such as padding, card radius,
> sidebar width, typography and colours, and see those changes reflected in
> the running UI immediately — while chats, rooms, tunnels, transfers,
> drafts and other application state continue functioning normally."

| DoD element | How demonstrated | Evidence |
|---|---|---|
| Run Boru once, edit boru-ui.toml → live redraw without rebuild/restart | File watcher + `UiThemeReloaded` → `set_ui_theme_config` (BORU-UI-06/07) | `watcher_sends_exactly_one_reload_per_save`, `watcher_rearms_for_subsequent_saves`; live GUI: radius 2→30→10 with app log `boru-ui.toml reloaded; applying live theme generation=N` and pixel-measured corner change (`manual-acceptance.md` steps 4-6) |
| Open the UI Inspector → same values editable with sliders/number fields/toggles/colour controls | `FieldKind`-driven rows emit normal Iced messages (`inspector.rs:50-62`) | `inspector_toggle_and_edit_updates_active_theme_via_messages`, `apply_float_sets_exact_config_leaf`, `apply_color_sets_exact_config_leaf`; live GUI Ctrl+Shift+D panel (`manual-acceptance.md` step 2) |
| Padding | `spacing` token group live-editable | `inspector_slider_edit_keeps_visual_feedback_immediate_and_defers_prewarm`; `app/home.rs:419` consumes `btheme.spacing.space_2` |
| Card radius | `radii.card` threaded through every Home card (BORU-UI-21 fix) | `home_cards_thread_live_card_radius_from_theme` (`app.rs:21769`); live pixel proof `manual-acceptance.md` step 2 |
| Sidebar width | `sidebar.width` live-editable | `ui_theme_reload_replaces_only_theme_state` (width 304→270 while conversation state preserved) |
| Typography | `typography` token group + font family/weight choices (BORU-UI-16) | `typography_matches_fonts`, `typography_weights_match_type_role`, `valid_family_and_weight_are_applied` (theme_merge); `Choice` field rows in inspector (`inspector.rs:59-61`) |
| Colours | semantic colour tokens (BORU-UI-17) + hex/RGBA editor | `semantic_colour_tokens_map_to_backing_fields`, `color_to_hex_round_trips`, `apply_color_sets_exact_config_leaf` |
| State continues normally (chats, rooms, tunnels, transfers, drafts) | Reload seam replaces only theme state | `ui_theme_reload_replaces_only_theme_state` (selected conversation, composer draft, scroll preserved), `ui_theme_reload_preserves_transfer_state`, `ui_theme_reload_preserves_inline_video_state`, `ui_theme_reload_error_keeps_last_known_good_theme`; manual acceptance steps 7-9 (draft, file transfer, video playback unaffected) |

## Coding Agent Rules — verification

| Rule | Verification | Result |
|---|---|---|
| Do not rewrite Boru from Iced to another UI framework | Only iced-family deps in Cargo.toml; no Dioxus/Slint/egui/tauri/yew; editor built as plain Iced widgets | PASS |
| Do not alter protocol/network behaviour | Zero network/protocol references in theme modules; `set_ui_theme_config` touches only theme state; full `net` regression gate green (§gate) | PASS |
| Prefer small, reviewable commits; compile/test after each migrated visual area | 32 commits, one per PDF step / visual area; each UI task ran `rb check` + targeted tests (per-task summaries BORU-UI-01..22) | PASS |
| Reuse production widgets in the gallery | Gallery imports and renders `CardShell`, `BoruDialog`, `Avatar`, `ListRow`, `PeerChipStack`, `view_download_progress`, `ui_components` buttons/badges — no duplicated mock implementations (`component_gallery.rs:22-31`) | PASS |
| Keep live-editor code isolated enough to be disabled for production | `dev-ui` feature not in defaults; `#[cfg(feature="dev-ui")]` on inspector + gallery; runtime gate in `main.rs`; release always off without feature | PASS |
| Treat existing UI appearance as baseline; don't silently change fonts/colours/sizing/layouts | `BoruTheme::default()` reproduces the baseline byte-for-byte, asserted by `*_matches_design_tokens` + `default_matches_audit_source_values` tests; the only intentional appearance change in the chain was BORU-UI-21's *fix* (Home cards now follow the live radius) so acceptance step 2 works | PASS |
| Do not make theme config a mandatory runtime dependency | Missing file → empty config → defaults; startup never fails because of the dev file (`theme_config.rs:894-900`); gate-off builds never read it | PASS |
| When uncertain whether a value is visual or behavioural, leave it out | `MESSAGE_GROUP_WINDOW_MS`, protocol limits, timeouts, character counts explicitly excluded (`constants-audit.md` §4) | PASS |

## Full regression gate (DEBSRV via `rb`)

Method per `references/debsrv-integration-test-gate.md` + the task body's
explicit gate. Runner: `scripts/t23_run_dod_gate.sh` (this task), log:
`docs/live-ui-editor/evidence/t23-gate/integration-gate.log`. debsrv root
disk at gate start: **37G free** — above the 5G threshold; no cleanup
required, nothing freed.

| Step | Command | Result |
|---|---|---|
| 1 | `rb test --lib --features net` | **2669 passed; 1 failed; 2 ignored** (359.24s) |
| 2 | `rb check --all-targets --features gui,video-playback,terminal,dev-ui` (dev-ui ON) | **exit 0** |
| 3 | `rb check --all-targets --features gui,video-playback,terminal` (dev-ui OFF) | **exit 0** |
| 4 | `rb check --release --bin boru --features gui,video-playback,terminal` (release, dev disabled) | **exit 0** |
| 5 | `rb test --bin boru --features gui,video-playback,terminal,dev-ui -- theme` (theme matrix + reload/state tests) | **99 passed; 0 failed** (26.49s) |

### The single lib failure is pre-existing, not a UI-chain regression

`storage::tests::docs_reference_current_schema_version` asserts that
`docs/message-storage-design.md` states `CURRENT_SCHEMA_VERSION: u32 = 20`.
Both `src/storage.rs` and `docs/message-storage-design.md` are **byte-identical
across the whole BORU-UI chain** (`git diff 417ea7cd^..HEAD -- src/storage.rs
docs/message-storage-design.md` → 0 lines), and the identical failure was
recorded by BORU-UI-20 (`f5f4f6b4`), BORU-UI-21 (`7a5cf548`), and BORU-UI-22
(`28a951d6`) as the same pre-existing baseline. It is a docs-reference drift
in the storage layer that predates the live UI editor and is untouched by it.

### Regression conclusion

Zero regressions attributable to the BORU-UI chain. Every gate check that the
chain touches (theme/config/merge/watcher/inspector/gallery modules, the
dev-ui on/off compile surface, and the release build with the dev feature
disabled) is green; the one non-green lib test is the documented pre-existing
storage doc-reference failure with 0-line-diff proof.

## Conclusion

All 10 Required End State bullets PASS with code/test/docs evidence, the
Definition of Done is demonstrated end-to-end (boru-ui.toml external edit,
inspector sliders/number fields/toggles/colour controls, live redraw,
state preservation for chats/rooms/tunnels/transfers/drafts/video), and all
Coding Agent Rules hold. The BORU-UI chain (01..22) satisfies the PDF's
Definition of Done.

## Files

- `docs/live-ui-editor/definition-of-done.md` (THIS DOCUMENT)
- `scripts/t23_run_dod_gate.sh` (NEW — DoD gate runner)
- `docs/live-ui-editor/evidence/t23-gate/integration-gate.log` (NEW — gate evidence)
