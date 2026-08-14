# Boru Live Layout (TOML) — Definition of Done Gate + Completion Report (BORU-LAYOUT-12)

Source: `boru_live_layout_toml_tasks.pdf` (attached to parent `t_dd962986`). This is
the FINAL gate of the BORU-LAYOUT-01..11 chain. It verifies the PDF's Definition of
Done against the merged codebase with code/test/docs evidence, checks the cross-cutting
guardrails, and runs the full regression matrix on DEBSRV via `rb`.

Verification date: 2026-08-14. HEAD under test: BORU-LAYOUT-11 (`137aadc1`) — the
last chain task — plus this task's commit. Tree: `git fetch origin && git merge
origin/main` (clean fast-forward). Worktree branch `wt/t_41f8e1fa`.

## Definition of Done — clause-by-clause evidence

> **DoD:** *"Editing boru-layout.toml while Boru is running can instantly change the
> home screen structure, section order, visibility, columns, spacing, alignment and
> responsive behaviour without recompiling."*

Every clause is decomposed below with (a) the code path, (b) the acceptance/unit test,
(c) the verdict.

| DoD clause | Code evidence | Test evidence | Verdict |
|---|---|---|---|
| **Editing boru-layout.toml while Boru is running** | `spawn_layout_watcher` watches `<data_dir>` non-recursively, reusing the theme watcher's `Debouncer`/`ReloadTracker`/`is_dev_config_event` machinery (`layout_watcher.rs:72-152`, BORU-LAYOUT-06). Every save burst is debounced (300 ms trailing edge), parsed on a **background thread away from the render path**, and delivered as `LayoutReloadMsg` → `AppMessage::LayoutReloaded` (`main.rs:1913-1924`, `app.rs:16843`). The app-side seam `update_layout_reloaded` (`app.rs:19243-19301`) validates and applies via `set_layout_overrides` → `set_layout_config`, which bumps `layout_revision` and marks the prewarm cache stale so lazy/prewarm trees rebuild (`app.rs:19016-19024, 19036-19050`). Stale generations are dropped by the generation watermark (`ReloadTracker`). | `watcher_sends_exactly_one_reload_per_save`, `watcher_rearms_for_subsequent_saves`, `watcher_reports_malformed_toml_as_error`, `watcher_reports_duplicate_sections_as_validation_error`, `watcher_missing_file_yields_empty_overrides` (`layout_watcher.rs` tests, BORU-LAYOUT-06/07). App seam: `layout_reload_replaces_only_layout_state`, `layout_reload_stale_generation_is_dropped` (`app.rs:36672, 36773`). Live-GUI evidence: BORU-LAYOUT-06 manual step (external save → log `boru-layout.toml reloaded; merging + applying live layout generation=N`, home re-layouts within ~300 ms). | **PASS** |
| **Instantly change … without recompiling** | The file is a runtime override: no code change, no `cargo` run. Reload is a normal Iced message in the update loop; the same running binary redraws on the next frame. The default path is the **identity merge** (`LayoutConfig::default()` + overrides), so an absent file reproduces today's layout (`layout_merge.rs`, `app.rs:19036-19050`; startup `main.rs:615-637`). | The watcher tests prove file→message→apply in the live process; `matrix_missing_file` proves absent file → default layout (`layout_regression.rs:272-290`); `matrix_merge_with_defaults` proves partial overrides merge onto defaults (`:296-328`). | **PASS** |
| **Home screen structure** | `LayoutConfig` root + typed groups (`layout.rs:52-90`): `home.*` drives section order/visibility, grid/list mode, column counts, max content width, padding, gaps, card sizing. Home view consumes `layout.visible_sections()` and the grid/list split from the live model (`app/home.rs:1072-1090, 1148-1182, 1553-1571`). | `matrix_rearranges_home_screen` (pure seam, `layout_regression.rs:337-411`); `layout_reload_rearranges_home_screen` (app seam, `app.rs:36919-37008`). 125 layout-filter tests green. | **PASS** |
| **Section order** | `home.section_order` (typed `HomeSection` list) + `HomeLayout::visible_sections()` (`layout.rs:112-170`) — the view renders exactly that list in that order (`app/home.rs:1181, 1553-1571`). | `matrix_rearranges_home_screen` reorders all five sections and asserts `visible_sections() == section_order` (`layout_regression.rs:363-388`); app-seam asserts the same after a live reload (`app.rs:36963-36979`). | **PASS** |
| **Visibility** | `home.hidden_sections` removes sections from the rendered list while keeping the default order (`layout.rs:160-170`); BORU-LAYOUT-07 rejects an id listed in both order and hidden (`validate_layout_overrides`, `layout_config.rs`). | `matrix_rearranges_home_screen` hidden-section case (`layout_regression.rs:391-410`); `matrix_invalid_toml_never_crashes` duplicate-id rejection (`:419-463`); `watcher_reports_duplicate_sections_as_validation_error`. | **PASS** |
| **Columns** | `home.grid.main_portion/rail_portion/column_gap` + `home.quick_actions.columns_wide/mid/narrow` (`layout.rs:182-237`); quick-action grid reads `grid_columns_for` from the model (`app/home.rs:1451-1452`). | `matrix_rearranges_home_screen` asserts grid split 3:1, gap 16, columns 6 (`layout_regression.rs:376-383`); `matrix_out_of_range_values` clamps absurd column counts (0 → default, 999 → 12) (`:196-266`). | **PASS** |
| **Spacing** | `home.gaps.*` (card/hero/header/footer/compact-header gaps) and `home.padding.*` consumed by the home view (`app/home.rs:823, 1531, 1565-1594`). | `matrix_rearranges_home_screen` changes `card_gap` 20→12 and asserts it (`layout_regression.rs:383`); `layout_reload_preserves_inline_video_state` exercises `home.gaps.card_gap` (`app.rs:37094-37125`). | **PASS** |
| **Alignment** | `component.*` placement groups — `thumbnail_position`, `metadata_alignment`, `button_placement`, `card_orientation` (`layout.rs` ComponentPlacement, BORU-LAYOUT-05) consumed by `video_file_card.rs:673` and `shared_by_me_table.rs`; gallery previews every component under different layout configs (BORU-LAYOUT-09). | `gallery_layout_presets_map_to_expected_configs`, `gallery_presets_map_to_required_widths` (`component_gallery.rs:3023, 3120`); app.rs `:37668` asserts `component.card_orientation` lives in the active layout. 125 layout-filter tests include the component-layout unit suite. | **PASS** |
| **Responsive behaviour** | `responsive.*` group (`layout.rs:998-1106`, BORU-LAYOUT-04): viewport tiers (`narrow_max_width`/`ultra_wide_min_width`), `tier_for_width`, per-tier `home_columns` and `home_padding_x`. Home view derives columns/padding from `responsive` (`app/home.rs:1080, 1066-1074`). | `responsive_tier_resolution_matches_gallery_vocabulary`, `responsive_home_columns_switch_by_tier`, `responsive_home_padding_reproduces_previous_two_tier_rule`, `responsive_tier_thresholds_and_tables_are_overridable`, `responsive_overrides_expose_new_tier_fields` (`layout.rs` tests, BORU-LAYOUT-04); `matrix_parse_complete_config` asserts `responsive.narrow_max_width` (`layout_regression.rs:86`). | **PASS** |

**All DoD clauses PASS.** The clause "home screen structure … alignment" is covered by the
home + component groups; the clause "visibility" by `hidden_sections`; "columns" by the grid
portions and quick-action tiers; "spacing" by `home.gaps`/`home.padding`; "responsive behaviour"
by the `responsive.*` breakpoint tables. Live reload is proven at the watcher and app-seam
levels, and the runtime file-watch + revision-bump redraw path is the same one demonstrated
live in BORU-LAYOUT-06.

## What was delivered per PDF task (1..11)

| PDF task | BORU task | Commit(s) | Delivered |
|---|---|---|---|
| 1. Separate Style from Layout | BORU-LAYOUT-01 | `f8c30f1c` | `LayoutConfig` model split from `BoruTheme`; layout-audit map `docs/live-layout/layout-audit.md`; theme stays purely visual. |
| 2. Design Layout Schema | BORU-LAYOUT-02 | `d28182d3` | Typed structs for Home/Sidebar/Chat/Component/Tables/Responsive + future `screens` extension point; `LayoutOverrides` partial-override mirror with `#[serde(default)]`; defaults reproduce current appearance. |
| 3. Home Layout | BORU-LAYOUT-03 | `836a72ab` | `home.*` wired into `app/home.rs`: section order, visibility, grid/list mode, column counts, max width, padding, gaps, card sizing. |
| 4. Responsive Layouts | BORU-LAYOUT-04 | `7dd2d5cf` | `responsive.*` breakpoints: viewport tiers, per-tier home column counts + horizontal padding, `tier_for_width` resolution. |
| 5. Component Layout | BORU-LAYOUT-05 | `d09d8dc9` | `component.*` placement (thumbnail position, metadata alignment, button placement, card orientation) into `video_file_card.rs` + `shared_by_me_table.rs`. |
| 6. Live Reload | BORU-LAYOUT-06 | `bad66166` | `layout_watcher.rs` reusing the theme-watcher machinery: directory watch, debounce, background parse, `LayoutReloaded` message, generation watermark, revision bump + prewarm invalidation. |
| 7. Validation | BORU-LAYOUT-07 | `ff0a4d6b` | `validate_layout_overrides`: duplicate section ids rejected (`LayoutConfigError::Validation`); out-of-range values clamped with warnings; last known-good layout retained on any failure. |
| 8. Inspector | BORU-LAYOUT-08 | `0b104e6a` | Dev UI Inspector Layout section exposing the editable `LayoutConfig` properties with live apply + atomic Save Layout → `boru-layout.toml`. |
| 9. Component Gallery | BORU-LAYOUT-09 | `a28446b4` | Gallery layout preset row (Default/Narrow/Desktop/Maximized) previewing every reusable component under different layout configs. |
| 10. Example TOML | BORU-LAYOUT-10 | `925874df` | `boru-layout.example.toml` (repo root): documented, active baseline values for section order, columns, visibility, spacing, responsive settings. |
| 11. Acceptance Tests | BORU-LAYOUT-11 | `137aadc1` | `layout_regression.rs` 9-test matrix + 4 app-seam tests (`layout_reload_*`) covering rearrange, state preservation, invalid-TOML robustness. |
| DoD gate | BORU-LAYOUT-12 | this task | This report + full regression matrix below. |

## Guardrail compliance

| Guardrail | Verification | Result |
|---|---|---|
| Keep structural layout separate from behaviour; hot-reload only affects arrangement/presentation | `layout_config.rs`/`layout.rs` contain zero protocol/network references; reload seam touches only `active_layout` + `layout_revision` + prewarm flag — never networking, gossip, rooms, tunnels, media, chat history, conversation, scroll or composer (`app.rs:19016-19050`) | PASS |
| BoruTheme for visual styling only; never mix layout into theme tokens | `layout.rs:1-24` module contract: "Nothing in this module reads BoruTheme"; theme modules contain no layout geometry | PASS |
| Do not rewrite Boru from Iced | Only iced-family deps; layout implemented as plain Iced widgets | PASS |
| Do not alter protocol/network behaviour | `net` regression gate green (§gate); layout modules have no network code | PASS |
| Preserve existing behaviour | `layout_reload_preserves_transfer_state` / `layout_reload_preserves_inline_video_state` (`app.rs:37011, 37094`); `layout_reload_replaces_only_layout_state` asserts conversation/composer/scroll untouched | PASS |
| Small, reviewable commits; compile/test after each migrated area | 11 chain commits on origin/main, one per PDF task; every task ran `rb check` + targeted tests (per-task summaries) | PASS |
| When uncertain, leave out of live layout system | Behavioural constants (protocol limits, timeouts) stay in code; audit flags borderline entries (`layout-audit.md`) | PASS |
| Not a mandatory runtime dependency; dev-only like boru-ui.toml | `boru-layout.toml` only read under the `dev-ui` gate (`main.rs:615-637, 1904-1924`); release without the feature never reads it; missing file → empty config → defaults; startup never fails | PASS |
| Layout defaults reproduce current appearance when config absent | `LayoutConfig::default()` reproduces baseline byte-for-byte; `matrix_parse_complete_config` asserts the example TOML (current baseline) merges to exactly `LayoutConfig::default()` (`layout_regression.rs:96-100`); `matrix_missing_file` (`:272-290`) | PASS |

## Full regression matrix (DEBSRV via `rb`)

DEBSRV root disk at gate start: **127G free** — well above the 5G threshold; no cleanup
required, nothing freed. All checks/tests were warm-slot (BORU-LAYOUT-11 built this
worktree's slot).

| Step | Command | Result |
|---|---|---|
| 1 | `rb check --bin boru --all-targets --features gui,video-playback,terminal,dev-ui` (dev-ui ON) | **exit 0** (1m13s) |
| 2 | `rb check --bin boru --all-targets --features gui,video-playback,terminal` (dev-ui OFF — proves layout system compiles with the dev feature disabled) | **exit 0** (48.8s) |
| 3 | `rb test --bin boru --features gui,video-playback,terminal,dev-ui -- layout` (layout matrix + app-seam reload/state tests) | **125 passed; 0 failed** (42.5s) |
| 4 | `rb test --bin boru --features gui,video-playback,terminal,dev-ui -- theme` (theme matrix, regression guard) | **99 passed; 0 failed** (19.3s) |
| 5 | `rb test --features net --bin boru -- layout` (layout matrix without dev-ui) | **93 passed; 0 failed** (17.4s) |
| 6 | `rb test --features net --lib` | **2669 passed; 1 failed; 2 ignored** (359s) |
| 7 | Integration sample, one-per-invocation with `timeout 240`: `rb test --features net --test test_serde_format` / `test_hostile_input` / `test_branding_rename` | **1/1, 41/41, 28/28 — all pass** |

### The single lib failure is pre-existing, not a layout-chain regression

`storage::tests::docs_reference_current_schema_version` asserts
`docs/message-storage-design.md` states `CURRENT_SCHEMA_VERSION: u32 = 20`. Both
`src/storage.rs` and `docs/message-storage-design.md` are byte-identical across the
whole BORU-LAYOUT chain (BORU-LAYOUT-11 recorded the identical failure with
0-line-diff proof), and BORU-UI-23/BORU-CARGO-08 recorded it before that. It is a
docs-reference drift in the storage layer, untouched by this chain, and **not fixed**
per task scope ("do NOT fix unrelated tests").

### Known relay-hang suites (documented pre-existing, NOT run)

Per `references/debsrv-integration-test-gate.md`, ~12 integration suites
(`repro_two_iced_instances`, `test_full_chat_list_flow`, `test_image_iced_gui_flow`,
`test_message_transfer`, `test_performance_regression`, etc.) hang on debsrv because
`RelayMode::Default` + `endpoint.online().await` resolves prod relay hostnames
IPv6-first and debsrv has no IPv6 route. This is an infrastructure limitation
independent of the layout chain (BORU-LAYOUT-11 documented the same); the layout
modules have zero networking code, so they cannot affect these suites.

## Regression conclusion

Zero regressions attributable to the BORU-LAYOUT chain. Every gate check the chain
touches is green: `check --all-targets` with dev-ui on and off, the layout matrix
(125/125 dev-ui, 93/93 net), the theme matrix (99/99), the full net lib suite
(2669 pass; the single failure is the documented pre-existing storage doc-drift),
and the integration sample (70/70). The Definition of Done is demonstrated end-to-end
at the watcher, merge and app seams with live-reload tests, and the layout defaults
reproduce the pre-chain appearance when `boru-layout.toml` is absent.

## Known gaps / out of scope

- Sidebar/chat/tables groups are schema-complete and covered by the regression
  matrix but not wired into their views — they were delivered as schema (PDF Task 2)
  and the PDF's DoD targets the **home screen**; wiring them is future work (the
  `#![allow(dead_code)]` guard in `layout.rs` documents this).
- The `screens` extension point is empty by design (future screens register per-screen
  layout groups).
- No GUI-pixel-level manual acceptance video was produced for this gate; the live
  reload demonstration evidence is the BORU-LAYOUT-06 manual step recorded in that
  task's handoff plus the watcher/app-seam tests above.

## Files

- `docs/live-layout/completion-report.md` (THIS DOCUMENT)
- Prior chain docs: `docs/live-layout/layout-audit.md` (BORU-LAYOUT-01)
