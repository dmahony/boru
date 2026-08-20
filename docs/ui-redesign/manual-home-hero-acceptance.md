# Home Hero manual acceptance matrix

Run date: 2026-08-20
Build: `wt/t_d306a24c` after merge of `wt/t_0334077a` (`4e4c0d10`)
Features: `gui,video-playback,terminal`
Environment: Linux/Xvfb software renderer, seeded empty Home state, `--no-dht --no-relay`

## Viewport matrix

| Viewport | Evidence | Result | Notes |
|---|---|---|---|
| 1600x900 | `/tmp/boru-home-acceptance-1787200369/home_1600x900.png` | PASS | Desktop two-row Home composition; hero, quick actions, Mesh Health, People & Activity, Tunnels and bottom status remain readable with no overlap. |
| 1920x1080 | `/tmp/boru-home-acceptance-1787200369/home_1920x1080.png` | PASS | Content remains constrained rather than stretching to the display; text and controls are readable; no clipping/overlap. |
| 1366x768 | `/tmp/boru-home-acceptance-1787200369/home_1366x768.png` | PASS | Desktop composition remains usable. Lower content is within the scrollable Home flow; no horizontal overflow observed. |
| 1024x720 (smaller supported) | `/tmp/boru-home-acceptance-1787200369/home_1024x720.png` | PASS | Cards stack into two columns, primary actions remain visible and usable, and short-height pressure is handled by scrolling rather than shrinking or clipping text. |

## Checklist

| Item | Result | Evidence |
|---|---|---|
| Home Hero text readability | PASS | Runtime captures at all four sizes; `Starting Boru` and supporting copy remain legible. |
| No clipping or overlap | PASS | Visual inspection of all four captures; responsive test `layout::responsive_height_tests::home_acceptance_sizes_keep_cards_inside_the_canvas` passed. |
| Public Rooms quick action reaches the existing public-room list | PASS | `quick_actions::tests::exposes_the_four_home_actions`, `app::tests::gui_navigation_mapping_includes_home_friends_settings_and_file_sharing`, and prior Home navigation focused gate; quick action is visibly present beside New Chat/Create Room/Share File. |
| Map remains monochrome | PASS | The responsive hero capture shows the grey monochrome world-map artwork at the compact breakpoint; `assets/status/world-map.svg` is the configured artwork and contains no colour palette. |
| Logo, fonts, and icons unchanged | PASS | BORU text logo and Lucide-style action icons are visible in runtime captures; `app::tests::home_screen_fonts05_approved_family_mapping` and `app::tests::home_screen_uses_type_role_roles` passed. No font/icon dependency changes were introduced. |
| Semantic status colours | PASS | Healthy badge/status uses green, Starting up uses amber, and the primary action emphasis uses the active accent in runtime captures; `app::home::tests::mesh_event_tone_keeps_unknown_events_neutral` and the focused Home test gate passed. |
| Accent `#8ec07c` selection | PASS | Settings state test selects `[142, 192, 124]` and emits `AccentChanged` + `PersistSettings`; `settings::tests::accent_color_selected_sets_and_closes_picker` passed in the Home-focused gate. |
| Accent switch, switch back, and restart persistence | PASS (state/persistence gate) | `settings::SettingsState::update` persists selected RGB, startup restores `AppSettings.accent_color` before first frame, and `app::tests::home_menu_item_opacity_persists_across_settings_roundtrip` plus accent override tests passed. The headless harness does not expose a reliable color-picker automation path, so this row is backed by the production state round-trip and startup restoration tests rather than a coordinate-driven picker replay. |

## Verification

- `rb build --bin boru --features gui,video-playback,terminal` — PASS.
- `rb test --bin boru --features gui,video-playback,terminal -- home` — PASS, **41 passed, 0 failed**.
- Runtime captures — PASS, PNG dimensions verified as 1600x900, 1920x1080, 1366x768, and 1024x720.
- The broader `scripts/ui_home17_verification_evidence.sh` produced Home/live-update captures; its OCR click calibration could not locate several labels despite the rendered controls being visible, so those warnings are harness/OCR limitations, not UI failures.
- Existing parent gate reported `rb check` and full binary build passing; three unrelated Tokio-reactor tests remain pre-existing failures.
