# Emoji Subsystem Architecture (BORU-TWEMOJI-24 final)

- **Chain**: BORU-TWEMOJI-01..24 (`Boru_Twemoji_Migration_Coding_Agent_Plan.pdf`)
- **Finalized**: 2026-08-17 (BORU-TWEMOJI-24, task t_9a5c9aa5)
- **Scope**: presentation layer only. Boru continues to store, encrypt and
  transmit normal Unicode emoji; Twemoji artwork is a rendering decision made
  locally at draw time.

## One authoritative implementation

As of BORU-TWEMOJI-24 there is exactly one emoji picker implementation and one
message renderer path:

- Picker: `src/bin/boru/emoji/picker.rs` (`view_emoji_picker`), invoked
  from `IcedChat::view_emoji_picker()` in `app/chat.rs`.
- Message rendering: `src/bin/boru/emoji/emoji_text.rs` (`emoji_text`),
  invoked from the chat-log body path in `app/chat.rs` (~line 3742).

The old hardcoded `const EMOJIS` grid + `text()` cells were removed in
BORU-TWEMOJI-04 (commit 5ba2e300); BORU-TWEMOJI-24 removed the last
chain-scaffolding artifacts (module-level `#![allow(dead_code, unused_imports)]`
and the unused `pub use` re-exports) and gated the remaining test-only helpers
with `#[cfg(test)]`. No dead font workaround or asset-loading code remains.

## Module map

```
src/bin/boru/emoji/
├── mod.rs            module docs, module map, narrow re-export (EmojiCategory)
├── catalog.rs        emoji metadata + categories (BORU-TWEMOJI-05, 12, 13)
├── asset_manifest.rs vendored asset index + lookup (BORU-TWEMOJI-06)
├── manifest_data.rs  generated sorted asset-key table (include!-ed, 3,838 entries)
├── parser.rs         grapheme-safe emoji detection + asset-key resolution (07, 16)
├── renderer.rs       SVG handle production + cache + EmojiRenderer trait (08, 09, 20)
├── emoji_text.rs     mixed text + Twemoji message renderer (17, 18)
├── picker.rs         the emoji picker panel (04, 10, 11, 12, 13, 14)
└── recents.rs        recently-used emoji list (14)
```

External consumers reach the module through narrow paths — see the
"Small stable interfaces" block at the top of `emoji/mod.rs`. Only
`EmojiCategory` is additionally re-exported at `emoji::` (used by `AppMessage`
and the chat panel).

## Rendering pipeline

```
Unicode grapheme (message text / picker cell / recent)
  → parser::emoji_asset(grapheme)          resolver: grapheme → normalized key
      ├─ Some(key) → asset_manifest::lookup validates against vendored set
      │     → renderer::EmojiRenderer::artwork()
      │         → cached_svg_handle(asset)  reads vendored SVG once, caches handle
      │         → Some(svg::Handle)         render Twemoji SVG
      └─ None (unsupported/newer)           → fall back to original Unicode text
```

- **Grapheme-safe**: `parser.rs` segments by grapheme cluster, never by single
  Rust `char` (BORU-TWEMOJI-07/16). ZWJ sequences, flags, skin tones and
  variation selectors resolve as one visual emoji.
- **Unicode-preserving**: message content on the wire, in storage and in the
  composer is always the original Unicode text. The SVG handle is a local
  rendering artifact only — it never enters a `Message`, a filename, an asset
  ID or any persistence format (enforced by tests like
  `message_wire_format_never_carries_asset_paths`).
- **Fallback (BORU-TWEMOJI-20)**: `EmojiRenderer::artwork` is the single shared
  fallback decision — SVG when the grapheme resolves AND the vendored file
  loads, otherwise the original Unicode text rendered with normal text
  rendering. Unsupported emoji are never hidden, dropped or replaced.
- **Caching (BORU-TWEMOJI-09)**: `renderer.rs` caches decoded `svg::Handle`s
  per asset key in a process-global cache; scrolling a chat or browsing the
  picker never re-reads an SVG per frame.
- **Generic font fallback kept**: the message/picker fallback uses Boru's
  normal font stack (`fonts.rs`), which delegates unknown glyphs to the OS
  system font — this is the generic fallback for composer text / unsupported
  Unicode, and is intentionally retained.

## Picker behaviour (BORU-TWEMOJI-10..14)

- Category tabs (BORU-TWEMOJI-12), search (BORU-TWEMOJI-13), recents
  (BORU-TWEMOJI-14), responsive layout (BORU-TWEMOJI-11), anchored above the
  composer with backdrop + Escape close (BORU-TWEMOJI-22).
- Selecting any cell emits `AppMessage::InsertEmoji(unicode_string)` — full
  grapheme string, never a single char, never an asset key. Composer insertion
  is unchanged plain-Unicode text.
- Recents persist via `AppSettings::recent_emojis` (`settings.json`), local
  only, never transmitted.

## Guardrail compliance (from the PDF)

| Guardrail | Where enforced |
|---|---|
| Presentation layer only; Unicode on the wire | `Message::Message { text }` untouched; asset paths never serialized |
| No external CDN/API at runtime | assets vendored in `assets/emoji/twemoji/`, loaded from disk |
| No Iced/large dependency upgrades | no dependency changes for the migration |
| Grapheme-safe parsing | `parser.rs` segmentation; VS16/ZWJ/flags tested (BORU-TWEMOJI-21) |
| Don't suppress unsupported emoji | `artwork()` Unicode fallback (BORU-TWEMOJI-20) |
| No new emoji message type / wire field | protocol format snapshots unchanged (BORU-TWEMOJI-15/24) |

## Tests

The emoji subsystem carries ~129 unit tests in the `boru` bin test module
(catalog, manifest drift, parser, renderer/cache, emoji_text metrics, picker,
recents) plus cross-cutting app tests (`escape_closes_emoji_and_gif_pickers`).
Run them with:

```
rb test --bin boru --features gui,video-playback,terminal -- emoji
```

Full-suite integration gate procedure: see the `iroh-gossip-chat-workflows`
skill reference `debsrv-integration-test-gate.md` (one `--test` per invocation
with `timeout 240`; known relay-hang suites are environment, not code).
