# Recently Used Emoji (BORU-TWEMOJI-14)

## Objective

Make frequently selected emoji quick to reach. Selecting an emoji moves it to
the front of a recently-used list that persists across app restarts through
Boru's normal local settings system. No message semantics change: recents are
plain Unicode strings, never image payloads, and the list is never transmitted
on the wire.

## Design

### Storage — `AppSettings::recent_emojis` (`app.rs`)

A `Vec<String>` field on `AppSettings`, persisted as `settings.json` in the
application data dir — the same local settings system every other Boru option
uses (`#[serde(default)]`, so old files load fine with an empty list). Only
Unicode strings are stored: no SVG paths, no asset keys, no image bytes.

- Runtime copy: `IcedChat.recent_emojis`, initialized in the constructor via
  `recents::sanitize_recents(&app_settings.recent_emojis)`.
- Persisted on every selection from the `InsertEmoji` handler
  (`app/chat.rs`), which calls the existing `save_settings()` (made live;
  previously `#[expect(dead_code)]`).

### List logic — `emoji/recents.rs`

Pure, unit-tested helpers:

- `record_recent(current, selected)` — move-to-front, deduplicate (a
  re-selected grapheme is removed from its old position), cap at
  `RECENT_LIMIT` (32, within the plan's 24–32 range).
- `sanitize_recents(entries)` — load-time/view-time normalization: skip
  empty/whitespace entries (corrupt storage), deduplicate, cap.

### Picker — `emoji/picker.rs`

- The category tab row now leads with a `Recent` tab
  (`EmojiCategory::Recent` + `EmojiCategory::ALL`); the Recent tab renders a
  themed clock glyph (the pseudo-category has no Twemoji icon, by design).
- With `Recent` active and an empty search query, `picker_entries` returns the
  sanitized recently-used list as `PickerEntry::Recent(&str)` entries.
- Recents render through the SAME resolver/fallback pipeline as catalog cells
  (`cell_artwork_unicode` → `renderer.resolve` → `svg_handle` → SVG, else the
  original Unicode text). An unknown/corrupt stored grapheme falls back to
  text and can never render an empty or broken cell; empty/whitespace entries
  are skipped.
- Selecting a recent cell emits `AppMessage::InsertEmoji(unicode)` — the same
  message as catalog/search cells, so composer insertion stays plain Unicode.
- Search overrides recents: a non-empty query shows catalog search results
  even when `Recent` is active; an empty recents list shows the muted
  "No recently used emoji yet" hint (`emoji.no_recents_yet`, en + fr).
- The tab-row natural width was updated for 9 tabs (Recent + 8 content), and
  the geometry invariant sweep still passes.

## Guardrails honored

- **Presentation layer only** — recents store and insert Unicode strings; no
  image-based emoji enters the message protocol.
- **No new persistence format** — reuse of `AppSettings`/`settings.json`.
- **No wire sync** — recents are local settings only; no message-protocol
  change.
- **Grapheme-safe** — recents are whole `String`s (multi-codepoint
  graphemes like ❤️, 🇬🇧, 👍🏽 survive selection, storage and rendering).
- **Fallback never suppresses** — unsupported graphemes render as their
  original Unicode text (BORU-TWEMOJI-20).

## Tests

`emoji/recents.rs` (7):

- `record_recent_moves_selected_to_front`
- `record_recent_deduplicates`
- `record_recent_caps_at_limit`
- `record_recent_ignores_empty_selection`
- `sanitize_recents_skips_empty_entries`
- `sanitize_recents_deduplicates`
- `sanitize_recents_caps_at_limit`
- `sanitize_recents_keeps_unknown_unicode`

`emoji/picker.rs` (7 new + updated search/category tests):

- `recent_category_shows_recents_in_order` — recents shown in order; values
  are raw Unicode, never asset/path
- `empty_recents_yield_empty_grid`
- `recent_entries_skip_corrupt_whitespace_only` — corrupt entries don't break
  the picker
- `search_overrides_recents`
- `recent_known_grapheme_renders_svg` — same resolver as catalog cells
- `recent_unknown_grapheme_falls_back_to_text` — fallback, never suppress
- updated `category_row_fits_wide_and_scrolls_narrow` for the 9-tab row
- updated `empty_search_restores_category_view` /
  `search_query_replaces_category_grid_with_results` /
  `search_results_insert_unicode_and_render_svg` /
  `search_no_match_yields_empty_grid` for the new `PickerEntry` shape

`app.rs` (1 new + updated literals):

- `recent_emojis_round_trip_in_settings` — settings.json round-trip
  preserves the Unicode list (persistence across restart)
- all `AppSettings` construction sites carry `recent_emojis`

## Verification

- `rb check --bin boru --features gui,video-playback,terminal` — clean.
- `rb test --bin boru --features gui,video-playback,terminal -- emoji` —
  all picker/recents/catalog tests pass.
- Targeted settings round-trip test passes.

## Follow-ups

- A settings-screen "Clear recent emoji" affordance was not requested by the
  PDF task; the picker's Recent tab has no per-item removal. If desired, a
  future task can add a clear action to the settings screen (same
  `AppSettings` field, no new store).
