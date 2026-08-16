# BORU-TWEMOJI-21 — Manual Test: Chat Bubble with Mixed Complex Emoji

- **Task**: t_ca9adbe1 (BORU-TWEMOJI-21, PDF Task 21)
- **Date**: 2026-08-16
- **Branch**: `wt/t_ca9adbe1`
- **Type**: manual/UI verification note — how to visually verify a chat
  bubble containing the full range of complex Unicode emoji that the
  resolver/parser suite (this task) now pins down in code.

## What the automated suite covers (run via `rb test ... -- emoji::`)

The unit tests added in this task protect the resolver
(`emoji_asset`) and the grapheme parser (`split_fragments`) against the
Unicode sequences most likely to break a naive implementation:

| Sequence class | Examples asserted | Test (module) |
|---|---|---|
| Single codepoint | 😀 🍕 🚀 😂 ✅ ❌ 🔥 🎉 | parser.rs resolver + boundary tests |
| Variation selectors | ❤️ (U+2764 U+FE0F), ⚠️, ✔️, ☺︎ ☠︎ (U+FE0E text forms) | `resolves_symbol_sequences_with_variation_selectors`, `resolves_text_presentation_variation_selector_forms`, `split_fragments_variation_selector_emoji_is_one_fragment` |
| Fitzpatrick skin tones | 👍🏻👍🏼👍🏽👍🏾👍🏿 (all five) | `resolves_all_five_fitzpatrick_skin_tone_modifiers`, `split_fragments_each_skin_tone_is_one_fragment` |
| Regional-indicator flags | 🇮🇪 🇦🇺 (also 🇺🇸 🇬🇧 from T07) | `resolves_ireland_and_australia_flags`, `split_fragments_ireland_and_australia_flags_are_single_fragments` |
| ZWJ sequences | 👩💻 👨💻, family 👨👩👧👦, 🏳️🌈 🏴☠️ (VS16 kept inside ZWJ) | `resolves_profession_gender_and_flag_zwj_sequences`, `split_fragments_profession_and_flag_zwj_are_single_fragments`, `plan_family_zwj_sequence_is_one_svg_artwork` |
| Symbol sequences | ✅ ✔️ ❌ ⚠️ 🔥 🎉 | `resolves_representative_single_codepoint_emoji`, `split_fragments_symbol_sequences_each_one_fragment` |
| Mixed strings | "Ship it! ✅🚀 See you at 9am 🇮🇪🇦🇺 — keep 👍🏻👍🏿 and ❤️. 🎉🔥" | `split_fragments_mixed_complex_string_with_punctuation`, `plan_mixed_complex_emoji_with_punctuation` |
| Graceful fallback | 🇽🇽 (unassigned flag pair), 😀‍😀 (unregistered ZWJ), 🫩 (Unicode 16.0), U+10FFFF | `returns_none_for_sequence_classes_without_vendored_assets`, `returns_none_for_unknown_or_newer_emoji`, `split_fragments_unvendored_sequences_stay_in_text_run` |

Every multi-codepoint case asserts **one visual emoji = one fragment /
one Svg artwork** (never split into its codepoints), and every mixed
string asserts the **byte-for-byte roundtrip** of the original Unicode —
the message content the user sent/stored is untouched by presentation.

## Manual GUI procedure

The automated tests verify the plan (what the renderer draws), but a
visual pass confirms the widget layer draws those plans correctly inside
a real bubble. To run it:

1. `cargo run` from the repo root (default features include `gui`; the
   binary target is `boru`).
2. Open or join any chat conversation.
3. Send the mixed test message below (paste works — the composer is a
   plain Unicode text input, BORU-TWEMOJI-19):

   ```
   Ship it! ✅🚀 See you at 9am 🇮🇪🇦🇺 — keep 👍🏻👍🏿 and ❤️. 🎉🔥
   👨💻 and 👩💻 ship together; family 👨👩👧👦 + 🏳️🌈 🏴☠️ 🫩 ❤
   ```

4. Verify on the sender **and a second peer** (run two instances, join
   the same topic):

   - Every emoji renders as a Twemoji SVG sitting on the text baseline —
     flags 🇮🇪🇦🇺 as one image each, skin tones as one image each, the
     family ZWJ as ONE image (never four separate faces), ✅ and ❌ as
     symbols, ❤️ as one heart.
   - The bubble wraps naturally and the emoji scale matches the
     surrounding text (no oversized boxes, no blank lines).
   - The unknown 🫩 (Unicode 16.0, no vendored asset) shows its original
     Unicode glyph — it must NOT be a blank box, a broken image, or an
     asset filename.
   - Copying the message text from the bubble reproduces the original
     Unicode string exactly (no asset keys/paths leak into the message).
   - Message history / restart shows the same Unicode content
     (presentation is render-time only).

5. Send a message containing ONLY an unsupported sequence (e.g. 🫩) to
   confirm the whole bubble falls back to a plain text render — never an
   empty widget.

## Result

- Automated: the full emoji test suite passes on DEBSRV (111 prior tests
  plus this task's ~16 new resolver/parser/plan tests; exact counts in
  the task completion metadata) — resolver success + graceful fallback,
  parser fragment boundaries, and mixed-string roundtrips all green.
- Manual GUI pass: to be performed by a human operator on the desktop
  target (this task's scope is the test suite + procedure; DPI/platform
  sweeps are BORU-TWEMOJI-22).
