# Emoji category navigation (BORU-TWEMOJI-12)

Task 12 of the Boru Twemoji migration: replace the undifferentiated long
emoji list with predictable category navigation.

## What changed

- `examples/iced_chat/emoji/catalog.rs`
  - `EmojiCategory` already carried the 8 PDF content categories (Smileys &
    People, Animals & Nature, Food & Drink, Activities, Travel & Places,
    Objects, Symbols, Flags) plus the reserved `Recent` pseudo-category from
    BORU-TWEMOJI-05; unchanged.
  - `COMMON_EMOJIS` expanded from 40 → 61 entries so every content category
    has grid entries (Animals & Nature and Flags were empty before). Every
    new asset key was verified against the vendored
    `assets/emoji/twemoji/svg/` set before adding.
  - New `emojis_for_category(category)` — the single filtering surface the
    picker (and later search, BORU-TWEMOJI-13) use against the shared
    catalog. Exact and disjoint per category; `Recent` yields empty.
  - New `category_icon(category)` + `CATEGORY_ICONS` — a representative
    vendored Twemoji entry per content category for the tab row; `None` for
    `Recent` (the extension point BORU-TWEMOJI-14 uses).
- `examples/iced_chat/emoji/picker.rs`
  - `view_emoji_picker(theme, active: EmojiCategory)` — the grid now shows
    exactly `emojis_for_category(active)`; a category switch rebuilds the
    grid from the filtered catalog every frame, so stale items cannot
    survive.
  - Category tab row: one 32x32 button per content category above the grid,
    showing the category's Twemoji artwork (20 px) through the shared
    renderer/cache (Unicode text fallback per BORU-TWEMOJI-20). Active tab
    uses Boru's selection styling — `surface_selected` background + primary
    border — mirroring `TabStrip::tab_button_style`. The row scrolls
    horizontally when the card is too narrow to show all 8 tabs.
  - Geometry helpers: `category_row_natural_width()` (8×32 + 7×4 = 284 px),
    `picker_card_width_with_category_row()` (card is at least wide enough
    for the tab row when space permits, never wider than available),
    `picker_scroll_height()` is now category-aware (takes the visible entry
    count and subtracts `CATEGORY_ROW_CHROME` from the fit).
- `examples/iced_chat/app.rs` + `app/chat.rs`
  - New `IcedChat::emoji_category` state (default SmileysAndPeople),
    `AppMessage::SelectEmojiCategory(EmojiCategory)` routed through the
    normal state layer, and the picker view passes the active category.
- `locales/en.json` + `locales/fr.json` — `emoji.category.*` label keys for
  the 8 categories + Recent (used by BORU-TWEMOJI-14's tab label).

## Geometry (narrow and wide)

| Available width | Columns | Card width (with tab row) | Tab row usable width | Tab row behavior |
|---|---|---|---|---|
| 60 px   | 1 | 60 px (capped) | 42 px | scrolls horizontally |
| 120 px  | 2 | 120 px (capped) | 102 px | scrolls horizontally |
| 200 px  | 4 | 200 px (capped) | 182 px | scrolls horizontally |
| 250 px  | 5 | 250 px (capped) | 232 px | scrolls (near fit) |
| 336 px  | 8 | 302 px (row-driven) | 284 px | all 8 tabs visible, no scroll |
| 400 px  | 9 | 374 px (grid-driven) | 356 px | all 8 tabs visible, no scroll |
| 1920 px | 9 | 374 px (grid-driven) | 356 px | all 8 tabs visible, no scroll |

- Wide: card = grid natural width (374 px at 9 columns) ≥ tab row (302 px
  incl. chrome) — everything visible, no clipping.
- Narrow: card caps at the available width; the tab row scrolls horizontally
  so all 8 categories stay reachable and the grid never clips.
- Scroll region: `max(content rows of active category, token 200).min(340)
  .min(available − 58 chrome − 40 tab row)` — category-aware.

## Test evidence

`cargo test --bin boru --features gui,video-playback,terminal -- emoji`
→ 62 passed, 0 failed (includes all pre-existing parser/renderer/catalog/
picker tests plus the new ones):

- catalog: `every_entry_belongs_to_a_content_category`,
  `every_content_category_has_picker_entries`,
  `emojis_for_category_is_exact_and_disjoint`,
  `category_icons_cover_content_categories_only`,
  `category_icon_assets_match_vendored_files`
- picker: `every_category_icon_renders_svg`,
  `category_row_fits_wide_and_scrolls_narrow`,
  `card_width_accommodates_category_row`,
  `grid_entries_change_with_category_selection_without_stale_items`,
  plus updated `scroll_height_grows_with_content_and_respects_window` and
  `responsive_invariants_hold_across_window_sizes` (category-aware chrome).

`cargo check --bin boru --features gui,video-playback,terminal` → exit 0.

`cargo check --all-targets --features gui,video-playback,terminal` fails ONLY
on the documented pre-existing E0061 set (`DiscoveryService::join` 5-arg
breakage in stale integration tests: test_discovery_startup,
test_discovery_restart, test_discovery_group_isolation) — zero emoji-related
errors. Same set documented by BORU-TWEMOJI-11 and earlier parents.

## Acceptance criteria

- Every catalog entry belongs to a category → `every_entry_belongs_to_a_content_category` (all_emoji() = common + representative, each in `EmojiCategory::ALL`, never Recent; every category represented).
- Changing categories updates the visible grid without stale items → grid rebuilt from `emojis_for_category(active)` on every frame; `emojis_for_category_is_exact_and_disjoint` + `grid_entries_change_with_category_selection_without_stale_items` prove the sets are disjoint and the union equals the catalog.
- Category navigation works at narrow and wide picker widths → `category_row_fits_wide_and_scrolls_narrow`, `card_width_accommodates_category_row`, `responsive_invariants_hold_across_window_sizes` (60→1920 px sweep).

## Notes / follow-ups

- Full manifest-driven catalog population (BORU-TWEMOJI-05/06) will replace
  the curated 61-entry list; `emojis_for_category` keeps working because it
  filters the shared catalog.
- Search (BORU-TWEMOJI-13) should reuse `emojis_for_category`-style filtering
  over the same catalog.
- Recent tab (BORU-TWEMOJI-14): add `EmojiCategory::Recent` to the tab row
  iteration and give it a label via `emoji.category.recent`; `category_icon`
  returns `None` for it so the tab falls back to a themed glyph until a
  recents icon is decided.
- `cargo fmt` on the whole workspace reformats hundreds of legacy files with
  the local rustfmt 1.9 (no pinned toolchain); this task formatted only its
  own files.
