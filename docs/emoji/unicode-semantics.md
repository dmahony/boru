# BORU-TWEMOJI-15 — Preserve Unicode Message Semantics

- **Task**: t_b88ac98a (BORU-TWEMOJI-15, PDF Task 15 of `Boru_Twemoji_Migration_Coding_Agent_Plan.pdf`)
- **Date**: 2026-08-16
- **Branch**: `wt/t_b88ac98a`
- **Type**: guardrail-gate — verify and lock in that the Twemoji chain changed
  no message serialization/protocol/storage format, and fix any discovered
  deviation.

## Objective restated

Guarantee that the Twemoji migration does not change the meaning or
representation of Boru messages:

- Composer text stays ordinary Unicode.
- Message serialization stays exactly as before the chain.
- No message type like `{ type: emoji, asset: ... }` for standard emoji.
- Old stored messages benefit from new rendering without migration.
- Copy/paste returns Unicode text, never SVG/asset references.

## Audit results

### 1. Composer insert path — plain Unicode, no asset references ✅

After the BORU-TWEMOJI-10 picker swap, the insert path is unchanged in
meaning:

- `AppMessage::InsertEmoji(String)` is the only message emitted by the picker
  for both catalog and recents cells (`emoji/picker.rs`:
  `insert_message()` → `AppMessage::InsertEmoji(emoji.unicode.to_string())`).
- The handler in `app/chat.rs` does `self.composer_text.push_str(&emoji)` —
  a plain Unicode string appended to the composer buffer. No SVG path, asset
  key, or markdown wrapper is ever inserted.
- Locked by existing tests: `insert_message_carries_unicode_never_asset` and
  `insert_message_keeps_multicodepoint_graphemes` (picker.rs test module),
  plus the send path builds `Message::Message { text }` from the composer
  string untouched.

### 2. Message serialization — zero protocol/storage change in the chain ✅

Every commit in the BORU-TWEMOJI chain was inspected with
`git diff-tree --name-only`:

| Commit | src/ files touched |
|---|---|
| BORU-TWEMOJI-01..14 (4d04f74a..7b2bb39d) | **0** |

- The `Message` enum in `src/chat_core/protocol.rs` has no emoji-specific
  variant. Chat text is still `Message::Message { text: String }`; reactions
  still carry a plain Unicode `emoji: String`.
- No new wire-format field, no new message type, no networking/encryption/
  persistence change anywhere in the chain.
- The only Cargo.toml change in the chain was a dev-dependency `resvg` for
  the BORU-TWEMOJI-03 render-proof test — not a runtime dependency and not
  used by the message path.
- All Twemoji work lives in `src/bin/boru/emoji/`, the GUI (`app.rs`,
  `app/chat.rs`, `theme.rs`), locales, vendored assets, and docs — i.e. the
  presentation layer only.

### 3. Protocol/storage snapshot tests — unchanged for emoji messages ✅

Existing round-trip coverage in `src/chat_core/tests.rs`:

- `message_serialization_roundtrip_text` — plain postcard round-trip.
- `compressed_roundtrip_all_message_variants` — every message variant
  survives sign/verify.
- `compressed_roundtrip_edge_cases` — includes `emoji_only`
  (`👍🏽🎉🔥🚀😀`) and `mixed_emoji_text` (`Great job! 🎉✨ Well done 👏`).
- `signed_message_roundtrip_reaction_various_emoji` — reactions with
  skin-tone, ZWJ family, flag, keycap and VS16 emoji.

New regression test added by this task:

- `message_emoji_roundtrips_serialization_unchanged` — for 8 emoji-bearing
  strings (simple, skin-tone, flag pair, ZWJ family, keycap, VS16 heart,
  mixed, RTL+emoji): plain postcard round-trip is byte-identical, signed
  wire round-trip preserves the exact Unicode, `message_hash` is stable, and
  the wire bytes contain no `assets`/`.svg` references.

### 4. Copy/paste — returns the original Unicode string ✅ (bug fixed)

Copy paths (`CopyMessage`, `ContextCopyText` in `app/chat.rs`) write
`entry.body` — the message's display string — to the system clipboard via
`iced::clipboard::write`. The body is the original Unicode message text
(through `sanitize_display_text`), never an asset reference.

**Bug found and fixed**: `sanitize_display_text` (and
`sanitize_single_line`) stripped U+200D (ZERO WIDTH JOINER). Any ZWJ emoji
sequence — family 👨‍👩‍👧‍👦, professions, gender variants — was silently split
into separate emoji in the display/copy body. That directly violated this
task's acceptance criteria:

- "Copying a rendered message yields the original Unicode string."
- Guardrail: "Do not suppress unsupported emoji; preserve and render their
  original Unicode as fallback."

Fix: removed U+200D from `is_stripped_unicode_format` in
`src/abuse_controls.rs` (ZWNJ U+200C remains stripped for anti-obfuscation).
The module doc now records the exception. New tests:

- `test_preserves_zwj_family_emoji` (abuse_controls.rs) — display and
  single-line sanitization both preserve the full family sequence.
- `test_strips_zero_width_non_joiner_but_keeps_zwj` — ZWNJ stripped, ZWJ kept.
- `metadata_zmj_zwj_sequences_safe` (tests/test_metadata_security.rs)
  strengthened — previously it asserted only that the first emoji survived
  (a misleadingly weak check whose comment claimed ZWJ was "NOT stripped"
  while the code stripped it); now it asserts the full family sequence with
  joiners round-trips unchanged.

This fix is presentation-layer only: it changes what text the UI displays and
copies, not what is stored, signed, or transmitted (which already preserved
ZWJ). Old stored messages with ZWJ emoji now display and copy correctly.

### 5. Old stored messages benefit without migration ✅

Rendering is presentation-layer only by construction: `EmojiRenderer` maps
Unicode graphemes to vendored Twemoji SVG assets at view time; the stored
message, its signed wire bytes, and its content hash are untouched. A message
stored before the chain renders through the exact same code path after the
chain — no migration, no rewrite of old rows. The new
`message_emoji_roundtrips_serialization_unchanged` test locks in the
byte-level guarantee that makes this true.

## Guardrails honored

- Presentation layer only — no image-based emoji in the message protocol.
- No networking, encryption or persistence format changes.
- No runtime dependency on an external emoji CDN/API (assets vendored).
- Grapheme-safe — the fix keeps ZWJ sequences whole for the upcoming
  BORU-TWEMOJI-16/17 grapheme parsing and EmojiText renderer.
- Unsupported emoji still fall back to their original Unicode text
  (BORU-TWEMOJI-20) — nothing is suppressed.

## Files changed

- `src/abuse_controls.rs` — stop stripping U+200D (ZWJ); doc note; 2 new tests.
- `src/chat_core/tests.rs` — new `message_emoji_roundtrips_serialization_unchanged`.
- `tests/test_metadata_security.rs` — strengthen `metadata_zmj_zwj_sequences_safe`.
- `docs/emoji/unicode-semantics.md` — this note.

## Verification

- `rb check --all-targets --features gui,video-playback,terminal` — passes
  except the documented pre-existing E0061 integration-test family (same as
  BORU-TWEMOJI-13/14); zero emoji-related errors.
- Targeted tests on debsrv: `chat_core` (incl. new emoji round-trip),
  `abuse_controls`, `test_metadata_security`.

## Follow-ups

- None blocking. BORU-TWEMOJI-16/17 (grapheme parsing, EmojiText renderer)
  can assume `entry.body` retains ZWJ sequences.
