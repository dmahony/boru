# BORU-TWEMOJI-01 — Audit of the Existing Emoji Implementation

- **Task**: t_6d9e56df (BORU-TWEMOJI-01, PDF Task 1 of `Boru_Twemoji_Migration_Coding_Agent_Plan.pdf`)
- **Date**: 2026-08-16
- **Branch**: `wt/t_6d9e56df`
- **HEAD at audit time**: `c03e12f6` (BORU-DESIGN-29: definition-of-done gate + completion report)
- **Scope**: read-only audit. No picker/renderer code was changed; this document records what exists so later BORU-TWEMOJI tasks can wire Twemoji rendering into it.

## Summary (implementation note)

The emoji picker is a hardcoded 40-entry list of Unicode emoji rendered as plain `text()` glyphs inside buttons, hosted in an `iced_aw::Card` overlay. Selecting an emoji emits `AppMessage::InsertEmoji(char)` where the payload is computed with `emoji.chars().next().unwrap()` — **a single-Rust-char truncation** that drops variation selectors (❤️ → ❤, ⚠️ → ⚠). The composer holds ordinary Unicode text and the whole send/receive pipeline (protocol `Message::Message { text }`, SQLite/JSON persistence, `SignedMessage` signing, gossip broadcast) treats emoji as plain text — **no networking, encryption or serialization change is needed** for the Twemoji presentation-layer migration. Chat message bodies render through a single `text()`/segmented-row path in `view_chat_log` using bundled fonts that contain no emoji glyphs, so emoji appearance depends entirely on OS system-font fallback (the tofu/inconsistency this chain fixes).

---

## 1. Emoji picker

### 1.1 Location

`src/bin/boru/app/chat.rs` — `IcedChat::view_emoji_picker()` (line 1092), called from the chat panel overlay in `view_chat_panel` (chat.rs ~337-344) when `self.show_emoji_picker` is true. The picker state flag lives on `IcedChat` (`show_emoji_picker: bool`, app.rs 4914; init false at app.rs 8176).

### 1.2 Emoji list definition

Hardcoded `const EMOJIS: &[&str]` **inside** `view_emoji_picker()` (chat.rs 1096-1100): 40 emojis laid out in fixed 8-per-row chunks (`EMOJIS.chunks(8)`, chat.rs 1106). There is no catalog module, no asset manifest, no per-platform variation — the same 40 strings are compiled into every build.

Entries are ordinary Unicode literals; two of the 40 are multi-codepoint sequences carrying U+FE0F variation selector 16: `"❤️"` and `"⚠️"`.

### 1.3 Widget used to render picker items

Plain system-font **`text()` glyphs**, not SVG, not an icon font, not a custom font run:

```rust
button(text(emoji).size(20.0))                 // chat.rs:1111
    .on_press(AppMessage::InsertEmoji(c))
    .padding([SPACE_2, SPACE_4])
    .style(|_t, _s| iced::widget::button::Style::default())
```

The `text()` call sets no `.font()`, so it uses iced's default font. The app-level default font is Public Sans (main.rs 2030-2035); none of the bundled fonts contain emoji glyphs (see §3.3), so the OS must supply the glyph via system-font fallback — the likely tofu/missing-glyph source and the cross-platform inconsistency this chain removes.

### 1.4 Panel chrome and theme tokens

- Hosted in `iced_aw::Card` (chat.rs 1125-1129) with title `i18n::t("emoji.title")` ("Emojis" / "Émojis") and `on_close(AppMessage::ToggleEmojiPicker)`.
- Card width: `chat.emoji_picker_width` (280.0), body height: `chat.emoji_picker_scroll_height` (160.0) via `gutter_scrollable(...)` (chat.rs 1120-1126).
- Theme tokens defined in `src/bin/boru/theme.rs` (fields 1344-1347, defaults 1377-1378) on the `BoruTheme.chat` group; overridable through `theme_config.rs` / `theme_merge.rs` (boru-ui.toml) and exposed to the visual designer via `inspector.rs` (`ChatEmojiPickerWidth` token). Layout overrides via `layout.rs` / `layout_merge.rs` (`PickerLayout` / `PickerOverrides`).

### 1.5 Composer toggle button

The 😊 button in the composer bar (chat.rs 4436-4443) is **not** a text glyph: it renders the bundled Lucide SVG `assets/icons/lucide/smile.svg` through the `Icon::Smile` icon-system enum (`src/bin/boru/icon_system.rs` 53, 222) and toggles `AppMessage::ToggleEmojiPicker` (handler chat.rs 6479-6482). Tooltip string: `i18n::t("chat.composer.emoji")`.

---

## 2. Selection → composer insertion path

### 2.1 Event trace

```
view_emoji_picker()                          chat.rs:1092
  └─ button(text(emoji).size(20.0))
       .on_press(AppMessage::InsertEmoji(c))
         where c = emoji.chars().next().unwrap()   chat.rs:1109
             → AppMessage::InsertEmoji(char)       app.rs:6441 (enum variant)
update(): AppMessage::InsertEmoji(ch)              chat.rs:6484-6488
  └─ self.composer_text.push(ch)                   chat.rs:6486
     (composer_text: String on IcedChat, app.rs 3817 / per-conversation 3220)
```

`InsertEmoji` is a pure state-layer event (no task); the picker stays open after insertion (the panel is closed only via `ToggleEmojiPicker`).

### 2.2 Single-Rust-char assumption (must fix in this chain)

`emoji.chars().next().unwrap()` (chat.rs:1109) takes only the **first Rust `char`** of the emoji string. For the two VS16 entries this silently drops the trailing variation selector:

| Picker entry | Codepoints | `chars().next()` inserts |
|---|---|---|
| `"❤️"` | U+2764 U+FE0F | `'❤'` (U+2764 only — VS16 lost) |
| `"⚠️"` | U+26A0 U+FE0F | `'⚠'` (U+26A0 only — VS16 lost) |

So the composed message text is already lossy for those two entries today. Additionally the `AppMessage::InsertEmoji(char)` payload type itself cannot carry a multi-codepoint emoji — the variant must become `InsertEmoji(String)` (or a grapheme-aware type) in BORU-TWEMOJI-07+.

Second finding: the handler comment says "Insert the emoji at the current cursor position" but it executes `self.composer_text.push(ch)` — **append at end**. There is no cursor-position tracking in the composer, so the comment is aspirational, not behavioral. Recording it here; changing cursor behavior is out of scope for this chain unless a later task explicitly asks.

---

## 3. Message rendering path (sent + received)

### 3.1 Entry model and caches

- `ChatEntry` (app.rs 2660-2735): fields include `body: String`, `kind: ChatKind` (Local/Remote/System), `image_handle: Option<iced::widget::image::Handle>` (decoded once), `parsed_segments: Option<Vec<link_preview::TextSegment>>` (cached URL/text runs), `label_text`, `reactions_text`, `formatted_time`.
- Constructors `ChatEntry::local` (2866), `remote` (2899), `system`; every entry passes through `entries_push` (app.rs 10079) which calls `update_cache()` (app.rs 3063-3086) once, computing `parsed_segments = link_preview::parse_url_segments(&self.body)` (app.rs 3081) — so per-message text analysis happens **once per entry, not per frame**.
- Send side: `SendPressed` (chat.rs 4719) → `persist_outgoing_message(topic, &trimmed)` (app.rs 10228-10234) → `crate::Message::Message { text: text.to_string() }` → sign/persist/broadcast → `ChatEntry::local` via `entries_push`.
- Receive side: `handle_net_event` → `ChatCallbacks::push_remote` (app.rs 18999) → `ChatEntry::remote` → `entries_push`; history replay in `RoomOpened` also lands in `entries_push`.

### 3.2 Bubble body rendering in `view_chat_log` (chat.rs 3383)

Per-entry body element (chat.rs 3716-3776):

- **No URLs** (`segments.len() == 1 && Text`): single `text(&entry.body)` widget, `.size(self.chat_text_size)`, `.font(btheme.type_font(TypeRole::ChatMessage))` → **Figtree** (fonts.rs 464-466), `.wrapping(Wrapping::WordOrGlyph)` (chat.rs 3725-3733).
- **Mixed text/URLs**: a `Row` of per-segment `text()`/`button(text(...))` elements from `entry.parsed_segments` (chat.rs 3735-3776), also `Wrapping::WordOrGlyph`.
- Bubble chrome: `container(body_el)` (chat.rs 3778-3797) wrapped in a copy-button + right-click `mouse_area` (chat.rs 3802-3809). Reactions render as a joined `reactions_text` string in the bubble metadata (app.rs 3076-3080).

This is the **single render site** that must become emoji-aware: `text(&entry.body)` and the per-segment `text(t)` calls (chat.rs 3725 and 3740-3748). The existing `parsed_segments` cache is the natural place to extend (see §5).

### 3.3 Fonts — why glyphs are inconsistent / tofu

Bundled fonts (all loaded at startup via `fonts::load_fonts()`, main.rs 2019): Figtree Regular/Medium/SemiBold, Raleway ExtraBold, JetBrains Mono family, Inter Tight Bold, Public Sans family (fonts.rs 39-75). **None contain emoji glyphs.** App default font is Public Sans (main.rs 2030-2035). Emoji therefore depend entirely on iced's system-font fallback (cosmic-text/fontdb): Segoe UI Emoji on Windows, Apple Color Emoji on macOS, Noto Color Emoji on typical Linux — and tofu boxes on minimal Linux installs or headless boxes with no emoji font. This is the cross-platform inconsistency the migration targets, and it is a pure presentation-layer concern.

### 3.4 Reactions path (second emoji surface)

`Message::Reaction { emoji: String }` (src/chat_core/protocol.rs 148-153) → `ChatCallbacks::add_reaction` (src/chat_core/state.rs 311-317) → `ChatEntry.reactions` joined into `reactions_text` (app.rs 3076-3080) and rendered inside the bubble. Reaction emoji are also plain Unicode strings and would benefit from the same Twemoji renderer (later tasks; not required by Task 1).

---

## 4. Iced version, enabled features, SVG/image caches

### 4.1 Iced 0.14 + features (svg confirmed usable)

Cargo.toml:196:

```toml
iced = { version = "0.14", default-features = false,
         features = ["tokio", "x11", "wayland", "tiny-skia", "image",
                     "lazy", "wgpu", "svg", "canvas", "advanced"], optional = true }
```

The `"svg"` feature **is listed and is genuinely usable in the GUI build**: it is compiled whenever the `gui` feature is enabled (`gui = ["net", "dep:iced", ...]`, Cargo.toml 304), which is Boru's default (default features include `gui`). Concrete proof of runtime SVG usage: `iced::widget::svg::Handle::from_memory` in `icon_svg()` (app.rs 1043-1051), `svg(handle)` in `file_type_icon.rs` (430), and every `Icon::*` Lucide icon (icon_system.rs). Related: `iced_aw = "0.14.1"` (badge/card/color_picker, Cargo.toml 197) is what hosts the picker `Card`; `iced_tiny_skia = "0.14"` is a dev-dependency (Cargo.toml 256). No iced version upgrade is required — the current stack is demonstrably capable (SVG rendering is already shipped and used).

### 4.2 Existing SVG/image cache abstractions (reuse, don't duplicate)

- **`SVG_HANDLE_CACHE`** — `OnceLock<Mutex<HashMap<String, svg::Handle>>>` in `src/bin/boru/file_type_icon.rs` (698-701), with `cached_svg_handle(asset_path)` (741) and a bundled-asset-path validation gate (`file_type_resolver::is_bundled_asset_path`, 742). Process-global, keyed by repo-relative asset path; `view` clones the O(1) handle instead of re-parsing SVG every frame. **This is the pattern to mirror (or extend) for Twemoji SVG handles.**
- **`icon_svg()`** (app.rs 1043-1051) — builds an SVG widget from embedded Lucide `include_bytes!` static data; no cache needed because bytes are `'static`.
- **`ChatEntry.image_handle`** (app.rs 2670ish, decoded once in the image path) — the "decode once, cache the handle, clone cheaply in view" pattern for raster images (see the iroh-gossip-chat skill's Iced image section).
- **GIF preview cache** (`gif_preview_cache`, chat.rs/`GifPreviewLoaded`), **link-preview thumbnail handles** — additional per-panel handle caches with the same shape.

---

## 5. Reusable Unicode segmentation / emoji parsing / text-run logic

- **`link_preview::TextSegment` + `parse_url_segments(body) -> Vec<TextSegment>`** (`src/bin/boru/link_preview.rs` 113-150): the closest existing "text-run" mechanism. Splits a message body into `Text(String)` / `Url(String)` segments, cached once per entry in `ChatEntry.parsed_segments` (app.rs 3081). The Twemoji renderer should extend this same per-entry cached-segments pattern (e.g. a `TextSegment::Emoji` variant or a parallel grapheme-run cache), not invent a new per-frame parser. Note: `parse_url_segments` is regex/byte-index based (link_preview.rs 123-150), **not** grapheme-safe — it must not be assumed safe for emoji splitting; BORU-TWEMOJI-07 needs a real grapheme walk.
- **`unicode-normalization = "0.1"`** — the only direct Unicode dependency (Cargo.toml 216), used for NFC normalization in `src/abuse_controls.rs` (39, 139, 182). It does **not** provide segmentation; reuse for normalization only.
- **`unicode-segmentation`** — present in Cargo.lock (multiple entries) **only as a transitive dependency** (cosmic-text/iced text stack); it is NOT a direct Cargo.toml dependency, so it cannot be `use`d without adding it. BORU-TWEMOJI-07 should either add `unicode-segmentation` as a direct dep (grapheme-safe splitting) or implement a small grapheme walk over `char_indices()`. No existing in-repo emoji-parsing/segmentation code was found.
- iced's `Wrapping::WordOrGlyph` (used on every message body) handles line breaking internally but is not emoji-aware rendering; it stays as-is.

---

## 6. Files/functions that will be changed (implementation note)

| Area | File | Function / item | What later tasks will touch |
|---|---|---|---|
| Picker list + render | `src/bin/boru/app/chat.rs` | `view_emoji_picker()` (1092), `const EMOJIS` (1096) | Replace `text()` glyphs with Twemoji SVG items; source list from catalog (BORU-TWEMOJI-02/04); grapheme-safe selection |
| Selection payload | `src/bin/boru/app/chat.rs` (1109) + `app.rs` `AppMessage::InsertEmoji` (6441) | `emoji.chars().next().unwrap()` → payload type | Change to full emoji string (`InsertEmoji(String)`) — fixes VS16 truncation |
| Insertion handler | `src/bin/boru/app/chat.rs` | `InsertEmoji(ch)` handler (6484-6488) | Insert full string into `composer_text` |
| Message body render | `src/bin/boru/app/chat.rs` | `view_chat_log` body element (3716-3776) | Route body through EmojiRenderer (inline SVG runs for supported emoji; Unicode fallback otherwise) |
| Entry text-run cache | `src/bin/boru/app.rs` | `ChatEntry.parsed_segments` (2734), `update_cache` (3081) | Extend segment model with emoji runs (grapheme-safe) |
| SVG handle cache | `src/bin/boru/file_type_icon.rs` | `SVG_HANDLE_CACHE` / `cached_svg_handle` (698-747) | Reuse pattern for Twemoji SVG handle cache (new `src/ui/emoji/renderer.rs` per PDF layout) |
| Asset manifest | new `assets/emoji/twemoji/` + catalog module | (BORU-TWEMOJI-02 vendors assets; BORU-TWEMOJI-04 creates `src/ui/emoji/` modules per PDF §3) | `mod.rs`, `catalog.rs`, `parser.rs`, `renderer.rs`, `picker.rs` |
| Theme tokens | `src/bin/boru/theme.rs` (1344-1347) + config/merge/inspector | `chat.emoji_picker_*` | Existing tokens reused; add grid/asset-size tokens only if needed |
| Locales | `locales/en.json` (40, 351), `fr.json` (39, 317) | `chat.composer.emoji`, `emoji.title` | Keep; add new emoji-picker strings in both locales |

Explicitly **unchanged**: protocol types (`src/chat_core/protocol.rs`), `handle_net_event`, storage/persistence (SQLite + JSON history/outbox), `SignedMessage` signing/compression, and all networking — emoji stay Unicode on the wire (see §7).

## 7. Networking / encryption / serialization — no changes needed

Confirmed by trace:

- **Message content** is `text: String` inside `crate::Message::Message` (persist_outgoing_message, app.rs 10233-10234; protocol.rs). Emoji are ordinary Unicode text; no image/filename/asset-id indirection exists or is planned.
- **Persistence**: outgoing goes through `Storage::queue_outgoing_message` (SQLite) or the legacy JSON `ChatHistoryStore`/`OutboxStore`; incoming through `handle_net_event` → `push_remote`. Both store the Unicode string verbatim.
- **Signing/transport**: `SignedMessage::sign_and_encode` + gossip broadcast operate on the encoded `Message` bytes; content is opaque to the transport.
- **Reactions** (`Message::Reaction { emoji: String }`, protocol.rs 153) are likewise plain Unicode strings on the wire.

The PDF guardrail "no chat protocol, encryption, persistence or wire-format changes" is satisfied: **no networking, encryption or serialization change is required** for the Twemoji migration. Any later task that proposes such a change is violating the plan.

## 8. Existing behavior coverage

- **Automated tests**: theme token defaults (theme.rs 2286-2287), and `ChatEntry::update_cache` caching behavior (app.rs 25501) indirectly cover entry caching. There are **no tests** referencing `view_emoji_picker`, `EMOJIS`, or `InsertEmoji`.
- **Reproducible manual test (baseline before modification)**:
  1. `cargo run` (default features, `gui` on) → open any room/conversation.
  2. Click the 😊 (Lucide SVG) button in the composer → the 280×160 `Card` picker opens with 40 emoji in 5 rows of 8.
  3. Click e.g. 😀 → `😀` appears at the **end** of the composer text; click ❤️ → only `❤` (no VS16) is appended; picker stays open.
  4. Send the message → it renders in a bubble in Figtree/system-fallback glyphs; received peers see the same Unicode text.
  5. On a Linux box with no emoji font installed, picker items and in-message emoji render as tofu boxes — reproduces the missing-glyph problem this chain fixes.
- This baseline is recorded here for BORU-TWEMOJI-24's cross-platform regression comparison.

## 9. Follow-up issues recorded (no scope creep)

1. `InsertEmoji(char)` drops U+FE0F for ❤️/⚠️ (chat.rs 1109 + payload type) — fix in the picker-rework task (BORU-TWEMOJI-07+).
2. "Insert at cursor position" comment vs `push()` append behavior (chat.rs 6484-6486) — composer has no cursor tracking; decide explicitly whether cursor insertion is wanted before claiming it.
3. `parse_url_segments` is byte/regex based, not grapheme-safe — the emoji-run splitter must not reuse it directly for segmentation (BORU-TWEMOJI-07).
4. `unicode-segmentation` is transitive-only today; adding it as a direct dependency (or a small grapheme walk) is required for grapheme-safe rendering.
5. Reactions (`reactions_text`) are a second emoji render surface; include them in the renderer's coverage once the picker/message path is done (not required by Task 1).
