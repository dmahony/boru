# Emoji search (BORU-TWEMOJI-13)

Task 13 of the Boru Twemoji migration: let users find emoji by name or
common keyword rather than manually browsing categories.

## What changed

- `src/bin/boru/emoji/catalog.rs`
  - Every `COMMON_EMOJIS` entry now carries curated search `keywords` (the
    search index). Keywords are lowercase and cover common words that are
    NOT part of the display name (e.g. `laugh` on "face with tears of joy",
    `rofl` on "rolling on the floor laughing", `coffee` on "hot beverage").
  - New `search_emojis(query) -> Vec<&'static Emoji>` — filters the SAME
    shared catalog the category views use; case-insensitive substring match
    on display name OR any keyword. Empty/whitespace query → empty (the
    picker restores the category view). Local-only; no network, no separate
    search dataset.
- `src/bin/boru/emoji/picker.rs`
  - `view_emoji_picker(theme, active, search_query)` — a search input sits
    above the category tab row using Boru's standard text-input styling
    (`crate::ui_components::text_input_style`) and a localized placeholder.
    A non-empty trimmed query replaces the grid with `search_emojis` results
    (spanning all categories) and hides the tab row; clearing the input
    restores the category view. Every keystroke emits
    `AppMessage::EmojiSearchChanged`, so results update immediately.
  - `picker_entries(query, active)` — the single entry-selection helper
    (search results when the query is non-empty, else the active category's
    entries), used by the view and tested directly.
  - Empty search results show a muted "no emoji found" hint
    (`emoji.search_no_results`) instead of a blank grid.
  - Geometry: `SEARCH_ROW_CHROME` (34 px input + 8 px spacing) is subtracted
    from the scroll fit; `picker_scroll_height` takes a `searching` flag so
    search mode reclaims the hidden tab row's chrome. The responsive
    invariant sweep now covers both category and search modes.
- `src/bin/boru/app.rs` + `app/chat.rs`
  - New `IcedChat::emoji_search_query: String` state (default empty),
    `AppMessage::EmojiSearchChanged(String)` routed through the normal state
    layer, and the picker view passes the live query.
- `locales/en.json` + `locales/fr.json` — `emoji.search_placeholder`
  ("Search emoji…" / "Rechercher un émoji…") and
  `emoji.search_no_results` ("No emoji found" / "Aucun émoji trouvé").

## Design notes

- Search filters `COMMON_EMOJIS` — the exact catalog the category views
  filter. There is no separate search dataset; the "index" is the static
  `keywords` field on each entry, so search and category views can never
  drift apart.
- Case-insensitivity is implemented by lowercasing the trimmed query once
  and comparing against lowercase names/keywords, so "LAUGH", "Laugh" and
  "laugh" all return the same entries.
- Search results are catalog entries, so the same `emoji_cell` /
  `insert_message` path applies: they insert the Unicode grapheme and render
  through the shared SVG renderer/cache (BORU-TWEMOJI-20 fallback included).
- While a query is active the category tab row hides (results span all
  categories); the scroll region reclaims that chrome. Clearing the query
  restores the active category's tab row and grid.

## Test evidence

`cargo test --bin boru --features gui,video-playback,terminal -- emoji`
→ passed, 0 failed (catalog + picker tests, including the new search tests):

- catalog: `search_laugh_returns_laughing_emoji`,
  `search_is_case_insensitive`, `search_empty_query_returns_nothing`,
  `search_filters_the_shared_catalog_only`, `search_matches_keywords_not_just_names`
- picker: `empty_search_restores_category_view`,
  `search_query_replaces_category_grid_with_results`,
  `search_results_insert_unicode_and_render_svg`,
  `search_no_match_yields_empty_grid`,
  `search_mode_reclaims_category_row_chrome`
- updated: `scroll_height_grows_with_content_and_respects_window`,
  `responsive_invariants_hold_across_window_sizes` (search-aware chrome,
  category + search modes).

`cargo check --bin boru --features gui,video-playback,terminal` → exit 0.

`cargo check --all-targets --features gui,video-playback,terminal` fails ONLY
on the documented pre-existing E0061 set (`DiscoveryService::join` 5-arg
breakage in stale integration tests) — zero emoji-related errors, same set
as BORU-TWEMOJI-12 and earlier parents.

## Acceptance criteria

- A query such as "laugh" returns relevant laughing emoji entries →
  `search_laugh_returns_laughing_emoji` (face with tears of joy via keyword,
  rolling on the floor laughing via name; every result is laugh-related).
- Empty search restores the category/recent view →
  `empty_search_restores_category_view` (empty/whitespace query yields
  exactly the active category's entries; `search_empty_query_returns_nothing`).
- Search results still insert Unicode and render with the shared SVG
  renderer → `search_results_insert_unicode_and_render_svg` (every result
  goes through `insert_message` → `InsertEmoji(unicode)` and
  `cell_artwork` → SVG).

## Notes / follow-ups

- Keywords are curated for the 61 common emojis; the full manifest-driven
  catalog (BORU-TWEMOJI-05/06) should carry keywords alongside names when
  generated. `search_emojis` reads `name` + `keywords`, so it will keep
  working unchanged.
- Fuzzy/pinyin/advanced matching is intentionally out of scope per the task.
- Recents (BORU-TWEMOJI-14) will slot into the same picker layout; the
  search input already restores to the category view when cleared.
- `cargo fmt` on the whole workspace still reformats hundreds of legacy
  files with the local rustfmt 1.9; this task formatted only its own files.
