# BORU-TWEMOJI-19 — Leave the Editable Composer Simple for Phase 1

- **Task**: t_f3af1fc9 (BORU-TWEMOJI-19, PDF Task 19 of
  `Boru_Twemoji_Migration_Coding_Agent_Plan.pdf`)
- **Date**: 2026-08-16
- **Branch**: `wt/t_f3af1fc9`
- **Type**: restraint-gate — verify the composer stayed a normal Unicode
  text input through the whole Twemoji chain, and document that inline
  Twemoji inside the *editable* composer is a separate future enhancement.

## Objective restated

The Twemoji migration is a presentation-layer change for the emoji picker
and *rendered* messages. The editable message composer must remain a normal
Unicode text input/editor:

- The operating system / font fallback displays whatever emoji it can
  while the user is typing.
- Twemoji SVGs appear in the picker and in rendered messages first.
- Inline Twemoji inside the editable composer is explicitly deferred to a
  future enhancement (see below).

## Verification — composer is still a plain Unicode text input ✅

### 1. The widget is iced's standard `text_input`, not a rich-text control

`IcedChat::view_composer` (`src/bin/boru/app/chat.rs`) builds the
message input with iced's plain `text_input` widget:

- `text_input(&t("chat.composer.placeholder"), &self.composer_text)`
  (chat.rs:4376) with `.id(COMPOSER_INPUT)`, `.on_input(InputChanged)`,
  `.on_submit(SendPressed)`, font/size/padding styling.
- The backing buffer is `composer_text: String` — ordinary UTF-8, no
  fragment list, no spans, no SVG handles, no rich-text model.
- No custom widget, no embedded-image renderer, no `emoji_text`-style
  paragraph layout is used inside the composer. The only widget in the
  chain that renders Twemoji SVG artwork is the picker grid cell
  (`emoji/picker.rs`) and the *message-log* renderer
  (`emoji/emoji_text.rs`, wired into `view_chat_log`, not the composer).

### 2. Every BORU-TWEMOJI commit touching `app/chat.rs` was audited

`git log 4d04f74a..HEAD -- src/bin/boru/app/chat.rs` lists exactly the
BORU-TWEMOJI-10/12/13/14/17 commits (plus the pre-chain BORU-RESP-04/07).
Their `app/chat.rs` diffs, inspected one by one:

| Commit | What changed in chat.rs | Composer widget touched? |
|---|---|---|
| BORU-TWEMOJI-10 | `InsertEmoji` handler: `push(ch)` → `push_str(&emoji)` — still plain Unicode; multi-codepoint graphemes now inserted whole | No |
| BORU-TWEMOJI-12 | Added `SelectEmojiCategory` handler + picker view arg (category state) | No |
| BORU-TWEMOJI-13 | Added `EmojiSearchChanged` handler + picker view arg (query) | No |
| BORU-TWEMOJI-14 | `InsertEmoji` handler additionally records the Unicode in recently-used settings | No |
| BORU-TWEMOJI-17 | Message-log body rendering switched from `text()` to `emoji_text()` — rendered messages only | No |

The `view_composer` function itself (the `text_input` widget, its id, its
styling) was never modified by any commit in the chain. The picker is an
overlay (`Stack::new()` in the chat view) that never replaces the input
widget.

### 3. Composer mutation sites are all plain `String` operations

- `InputChanged(text)` → `self.composer_text = text` (chat.rs:4687) — the
  user typing/editing path, unchanged from before the migration.
- `InsertEmoji(emoji)` → `self.composer_text.push_str(&emoji)`
  (chat.rs:6491) — appends the full Unicode grapheme at the cursor/end.
- `SendPressed` → trims, clears the buffer, sends
  `Message::Message { text }` — protocol path untouched (see
  `docs/emoji/unicode-semantics.md` for the byte-level guarantee).
- Conversation switching moves the plain `String` between per-conversation
  state; no transformation.

### 4. Picker insertion works inside the existing composer ✅

- Picker cells (`emoji/picker.rs::insert_message`) always emit
  `AppMessage::InsertEmoji(emoji.unicode.to_string())` — the full Unicode
  grapheme, never an asset key or SVG path (locked by
  `insert_message_carries_unicode_never_asset` and
  `insert_message_keeps_multicodepoint_graphemes`).
- The handler's `push_str` appends straight into `composer_text`, which is
  exactly the buffer the `text_input` displays. No re-focus, no re-parse,
  no widget swap required.

## Behavioural parity — typing/editing no worse than before ✅

- **Typing**: `InputChanged` replaces the buffer — identical code path to
  pre-migration; the chain never touched it.
- **Editing** (cursor, selection, paste, IME): owned entirely by iced's
  `text_input`; the widget and its configuration are unchanged.
- **Picker insertion**: pre-chain `push(char)` could only insert one code
  point (splitting multi-codepoint emoji like ❤️ / 👍🏽 / 👨‍👩‍👧‍👦);
  the chain's `push_str(&emoji)` inserts the whole grapheme — strictly
  better, still plain Unicode text in the buffer.
- **New regression test** `picker_insert_into_composer_buffer_is_plain_text_parity`
  (in `emoji/picker.rs`) models the handler exactly: for every catalog
  emoji, applying `buffer.push_str(&emoji)` to a prefixed buffer yields
  byte-identical `prefix + emoji.unicode` — the same result as typing the
  characters — and a run of inserts preserves typing order. This locks in
  "no custom rich-text editor" at the message level.

## Future enhancement — inline Twemoji in the editable composer (out of scope)

Displaying Twemoji SVGs *inside* the editable composer (while typing) is
**explicitly out of scope for this chain**, per PDF Task 19:

- Phase 1 behaviour: the composer renders whatever the OS/font fallback can
  display while editing (usually system-color emoji glyphs or monochrome
  fallback); Twemoji is used in the picker and in rendered messages.
- A future enhancement would need a custom rich-text / embedded-image
  editor widget (e.g. inline SVG runs inside the input), which contradicts
  Task 19's direction to keep the editor simple. If ever implemented it
  must remain presentation-layer only: the stored, encrypted, transmitted
  and copied text must stay the original Unicode grapheme string, reusing
  the existing `emoji::renderer` cache and grapheme-safe parser from this
  chain — no image-based emoji in the message protocol.

## Guardrails honored

- Presentation layer only — no image-based emoji in the message protocol.
- No networking, encryption or persistence format changes.
- No custom rich-text editor introduced (verified by commit audit above).
- Grapheme-safe insertion (`push_str` whole grapheme, not per `char`).
- Unsupported emoji still fall back to original Unicode (BORU-TWEMOJI-20).

## Files changed

- `docs/emoji/composer-simple.md` — this note.
- `src/bin/boru/emoji/picker.rs` — new behavioural-parity test
  `picker_insert_into_composer_buffer_is_plain_text_parity`.

## Verification

- `rb check --bin boru --features gui,video-playback,terminal` — exit 0.
- `rb test --bin boru --features gui,video-playback,terminal -- emoji::picker`
  — all picker tests pass (incl. the new parity test).
