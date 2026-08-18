//! Unicode grapheme/emoji detection and asset-key resolution (BORU-TWEMOJI-07).
//!
//! The central resolver [`emoji_asset`] converts a single Unicode emoji
//! grapheme into a Twemoji asset key (lowercase hex codepoints joined with
//! `-`) and validates it against the generated vendored-asset manifest
//! (BORU-TWEMOJI-06). Grapheme *segmentation* of full message text — walking
//! clusters and coalescing plain runs into [`MessageFragment`]s — lives here
//! too ([`split_fragments`], BORU-TWEMOJI-16); the resolution half
//! ([`emoji_asset`]) accepts one grapheme at a time.
//!
//! Guardrails:
//! - Never assume one Rust `char` equals one visual emoji. The resolver
//!   processes the whole grapheme string: variation selectors, skin-tone
//!   modifiers, regional-indicator flag pairs and ZWJ sequences all map to a
//!   single asset key.
//! - Unknown/newer emoji return `None` → callers fall back to the original
//!   Unicode text (never a manufactured/broken asset path).

use crate::emoji::asset_manifest;
use crate::emoji::renderer::EmojiAsset;
use unicode_segmentation::UnicodeSegmentation;

/// Variation selector 16 (U+FE0F) — forces emoji presentation. Twemoji
/// vendors keys with FE0F kept inside ZWJ sequences (e.g.
/// `1f3c3-1f3fb-200d-2640-fe0f`) but strips it from most standalone keys
/// (`2764`, `23-20e3`), so the resolver validates both forms.
const VS16: char = '\u{FE0F}';
/// Variation selector 15 (U+FE0E) — forces text presentation. Twemoji
/// vendors no text-presentation keys, so FE0E is dropped when computing the
/// closest emoji key (the text-default base key, e.g. `2620` for `☠︎`).
const VS15: char = '\u{FE0E}';

/// A fragment of message text after emoji-aware splitting (PDF Task 16 shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageFragment<'a> {
    /// Plain text run (one or more graphemes with no bundled asset).
    Text(&'a str),
    /// A single emoji grapheme resolved to a bundled asset.
    Emoji {
        /// The original Unicode grapheme (what stays in the message).
        unicode: &'a str,
        /// Resolved presentation asset.
        asset: EmojiAsset,
    },
}

/// Resolve a single Unicode grapheme to a bundled Twemoji asset, or `None`
/// when no vendored asset exists.
///
/// Normalization (BORU-TWEMOJI-07): the grapheme is converted to the Twemoji
/// key format — lowercase hex codepoints joined with `-`, e.g. `1f600`,
/// `1f1fa-1f1f8` (flag pair), `1f44d-1f3fd` (skin tone),
/// `1f469-200d-1f4bb` (ZWJ). Variation selector handling is candidate-based
/// because Twemoji keeps FE0F in some keys (gender/profession ZWJ sequences)
/// and strips it in others (hearts, symbols, keycaps):
///
/// 1. the raw sequence (variation selectors kept), then
/// 2. the sequence with FE0F/FE0E removed.
///
/// Each candidate is validated against the generated asset manifest; the
/// first vendored key wins. `None` means "not bundled" — callers fall back
/// to rendering the original Unicode text.
///
/// Callers must pass a single grapheme (message segmentation is
/// BORU-TWEMOJI-16); a multi-grapheme string never matches a vendored key.
pub fn emoji_asset(grapheme: &str) -> Option<EmojiAsset> {
    for key in twemoji_key_candidates(grapheme) {
        if let Some(asset_key) = asset_manifest::lookup(&key) {
            return Some(EmojiAsset::from_key(asset_key));
        }
    }
    None
}

/// Candidate Twemoji keys for a grapheme, most specific first: the raw
/// lowercase-hex sequence, then the same sequence with FE0F/FE0E variation
/// selectors removed. The second candidate is omitted when stripping changes
/// nothing.
fn twemoji_key_candidates(grapheme: &str) -> Vec<String> {
    let mut keys = Vec::with_capacity(2);
    if let Some(raw) = hex_key(grapheme, false) {
        keys.push(raw);
    }
    if let Some(stripped) = hex_key(grapheme, true) {
        if keys.first() != Some(&stripped) {
            keys.push(stripped);
        }
    }
    keys
}

/// Lowercase-hex dash-joined key for a grapheme, optionally skipping
/// variation selectors. Returns `None` for empty input or a string made only
/// of stripped variation selectors.
fn hex_key(grapheme: &str, strip_variation_selectors: bool) -> Option<String> {
    use std::fmt::Write as _;
    if grapheme.is_empty() {
        return None;
    }
    let mut key = String::with_capacity(grapheme.len() * 2);
    for ch in grapheme.chars() {
        if strip_variation_selectors && (ch == VS16 || ch == VS15) {
            continue;
        }
        if !key.is_empty() {
            key.push('-');
        }
        write!(&mut key, "{:x}", ch as u32).ok()?;
    }
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// Split message text into text/emoji fragments.
///
/// Walks the input by Unicode grapheme cluster (never by Rust `char` — a
/// single visual emoji is frequently several codepoints: skin-tone
/// modifiers, regional-indicator flag pairs, ZWJ sequences), resolves each
/// cluster through [`emoji_asset`], and coalesces adjacent non-emoji
/// graphemes into a single `Text` run so the renderer pays per-run layout
/// cost instead of per-grapheme.
///
/// Guarantees:
/// - Output order matches input order; the concatenation of every fragment's
///   Unicode (Text payload + Emoji `unicode`) is exactly the input.
/// - An emoji cluster that resolves to a vendored asset becomes exactly one
///   `Emoji` fragment carrying the *whole* grapheme (e.g. `👍🏻` stays one
///   fragment, not `👍` + modifier).
/// - An emoji cluster with no vendored asset (newer/unknown emoji) is
///   preserved inside a `Text` run — original Unicode, never suppressed.
/// - Empty input yields no fragments.
pub fn split_fragments(input: &str) -> Vec<MessageFragment<'_>> {
    let mut fragments: Vec<MessageFragment<'_>> = Vec::new();
    // Byte offset where the current plain-text run starts. `None` when no
    // run is open. Runs are closed when an emoji cluster is emitted.
    let mut run_start: Option<usize> = None;

    let mut offset = 0usize;
    for grapheme in input.graphemes(true) {
        if let Some(asset) = emoji_asset(grapheme) {
            flush_text_run(&mut fragments, &mut run_start, input, offset);
            fragments.push(MessageFragment::Emoji {
                unicode: grapheme,
                asset,
            });
        } else if run_start.is_none() {
            run_start = Some(offset);
        }
        offset += grapheme.len();
    }
    flush_text_run(&mut fragments, &mut run_start, input, input.len());
    fragments
}

/// Push the accumulated plain-text run `input[start..end]` as a [`Text`]
/// fragment, if any run is open (`run_start` is `Some`). Takes `input`
/// explicitly (rather than capturing it in a closure) so the emitted slice
/// keeps the input's lifetime `'a`.
fn flush_text_run<'a>(
    fragments: &mut Vec<MessageFragment<'a>>,
    run_start: &mut Option<usize>,
    input: &'a str,
    end: usize,
) {
    if let Some(start) = run_start.take() {
        fragments.push(MessageFragment::Text(&input[start..end]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_basic_single_codepoint_emoji() {
        assert_eq!(emoji_asset("😀").map(|a| a.key), Some("1f600"));
        assert_eq!(emoji_asset("🍕").map(|a| a.key), Some("1f355"));
        assert_eq!(emoji_asset("🚀").map(|a| a.key), Some("1f680"));
    }

    #[test]
    fn resolves_heart_and_symbol_variation_selector_variants() {
        // ❤️ = U+2764 U+FE0F — the vendored key drops VS16 ("2764-fe0f" is
        // not bundled; only "2764" is), so resolution falls back to the
        // stripped candidate.
        assert_eq!(emoji_asset("\u{2764}\u{fe0f}").map(|a| a.key), Some("2764"));
        assert_eq!(emoji_asset("\u{26a0}\u{fe0f}").map(|a| a.key), Some("26a0"));
        // Bare text-default symbol (no VS16) resolves to the same key...
        assert_eq!(emoji_asset("☠").map(|a| a.key), Some("2620"));
        // ...and so does the emoji-presentation variant (VS16 stripped).
        assert_eq!(emoji_asset("\u{2620}\u{fe0f}").map(|a| a.key), Some("2620"));
    }

    #[test]
    fn resolves_regional_indicator_flags() {
        // 🇺🇸 = U+1F1FA U+1F1F8 — a flag pair is one grapheme, one key.
        assert_eq!(
            emoji_asset("\u{1f1fa}\u{1f1f8}").map(|a| a.key),
            Some("1f1fa-1f1f8")
        );
        // 🇬🇧 = U+1F1EC U+1F1E7
        assert_eq!(
            emoji_asset("\u{1f1ec}\u{1f1e7}").map(|a| a.key),
            Some("1f1ec-1f1e7")
        );
    }

    #[test]
    fn resolves_skin_tone_modifiers() {
        assert_eq!(emoji_asset("👍").map(|a| a.key), Some("1f44d"));
        // 👍🏻 U+1F44D U+1F3FB (light) / 👍🏽 U+1F44D U+1F3FD (medium)
        assert_eq!(
            emoji_asset("\u{1f44d}\u{1f3fb}").map(|a| a.key),
            Some("1f44d-1f3fb")
        );
        assert_eq!(
            emoji_asset("\u{1f44d}\u{1f3fd}").map(|a| a.key),
            Some("1f44d-1f3fd")
        );
        // 👋🏿 U+1F44B U+1F3FF (dark)
        assert_eq!(
            emoji_asset("\u{1f44b}\u{1f3ff}").map(|a| a.key),
            Some("1f44b-1f3ff")
        );
    }

    #[test]
    fn resolves_zwj_sequences() {
        // 👩💻 = U+1F469 ZWJ U+1F4BB (woman technologist).
        assert_eq!(
            emoji_asset("\u{1f469}\u{200d}\u{1f4bb}").map(|a| a.key),
            Some("1f469-200d-1f4bb")
        );
        // 👨👩👧👦 = 4-component ZWJ family sequence.
        assert_eq!(
            emoji_asset("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}")
                .map(|a| a.key),
            Some("1f468-200d-1f469-200d-1f467-200d-1f466")
        );
        // 🏃🏽‍♀️ = U+1F3C3 U+1F3FD ZWJ U+2640 U+FE0F — Twemoji KEEPS VS16 in
        // this gender sequence, so the raw candidate must win (the stripped
        // form "1f3c3-1f3fd-200d-2640" is not bundled).
        assert_eq!(
            emoji_asset("\u{1f3c3}\u{1f3fd}\u{200d}\u{2640}\u{fe0f}").map(|a| a.key),
            Some("1f3c3-1f3fd-200d-2640-fe0f")
        );
        // #️⃣ = U+0023 U+FE0F U+20E3 — the keycap key drops VS16 ("23-20e3"
        // is bundled, "23-fe0f-20e3" is not), so the stripped candidate wins.
        assert_eq!(
            emoji_asset("#\u{fe0f}\u{20e3}").map(|a| a.key),
            Some("23-20e3")
        );
    }

    #[test]
    fn returns_none_for_unknown_or_newer_emoji() {
        // 🫩 face with bags under eyes — Unicode 16.0, not in Twemoji 15.1.0.
        assert_eq!(emoji_asset("🫩"), None);
        // Private-use codepoint, never vendored.
        assert_eq!(emoji_asset("\u{10FFFF}"), None);
    }

    #[test]
    fn returns_none_for_non_emoji_input() {
        assert_eq!(emoji_asset(""), None);
        assert_eq!(emoji_asset("hello"), None);
        assert_eq!(emoji_asset("a"), None);
        // More than one grapheme: callers must segment first
        // (BORU-TWEMOJI-16); the joined sequence never matches a key.
        assert_eq!(emoji_asset("😀👍"), None);
    }

    #[test]
    fn resolved_asset_uses_vendored_key_and_path() {
        let asset = emoji_asset("😀").expect("grinning face resolves");
        assert_eq!(asset.key, "1f600");
        assert_eq!(
            asset.path.to_string_lossy(),
            "assets/emoji/twemoji/svg/1f600.svg"
        );
    }

    #[test]
    fn split_fragments_coalesces_plain_text_into_one_run() {
        let fragments = split_fragments("hello world");
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0], MessageFragment::Text("hello world"));
    }

    #[test]
    fn split_fragments_mixed_text_and_emoji_preserve_order() {
        let fragments = split_fragments("hi 😀 there 🍕 bye");
        assert_eq!(fragments.len(), 5);
        assert_eq!(fragments[0], MessageFragment::Text("hi "));
        assert!(matches!(
            &fragments[1],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == "😀" && asset.key == "1f600"
        ));
        assert_eq!(fragments[2], MessageFragment::Text(" there "));
        assert!(matches!(
            &fragments[3],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == "🍕" && asset.key == "1f355"
        ));
        assert_eq!(fragments[4], MessageFragment::Text(" bye"));
    }

    #[test]
    fn split_fragments_keeps_multicodepoint_emoji_as_single_fragment() {
        // 🇺🇸 = U+1F1FA U+1F1F8 — a two-codepoint flag pair is ONE fragment.
        let fragments = split_fragments("🇺🇸");
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == "🇺🇸" && asset.key == "1f1fa-1f1f8"
        ));

        // 👍🏻 = U+1F44D U+1F3FB — base + skin-tone modifier is ONE fragment.
        let fragments = split_fragments("👍🏻");
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == "👍🏻" && asset.key == "1f44d-1f3fb"
        ));

        // 👩💻 = U+1F469 ZWJ U+1F4BB — a ZWJ sequence is ONE fragment.
        // (Uses explicit escapes: the U+200D joiner is invisible in source
        // and easy to drop accidentally when composing the literal.)
        let fragments = split_fragments("\u{1f469}\u{200d}\u{1f4bb}");
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == "\u{1f469}\u{200d}\u{1f4bb}" && asset.key == "1f469-200d-1f4bb"
        ));

        // A ZWJ family sequence (4 components + 3 joiners) stays ONE fragment.
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let fragments = split_fragments(family);
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == family && asset.key == "1f468-200d-1f469-200d-1f467-200d-1f466"
        ));
    }

    #[test]
    fn split_fragments_adjacent_emoji_have_no_gap_runs() {
        // Two emoji back-to-back produce two Emoji fragments with no empty
        // Text fragment between them.
        let fragments = split_fragments("😀🍕");
        assert_eq!(fragments.len(), 2);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, .. } if *unicode == "😀"
        ));
        assert!(matches!(
            &fragments[1],
            MessageFragment::Emoji { unicode, .. } if *unicode == "🍕"
        ));
    }

    #[test]
    fn split_fragments_unknown_emoji_stays_in_text_run() {
        // 🫩 (Unicode 16.0, not vendored) must not be suppressed: it stays
        // inside the surrounding plain-text run as original Unicode.
        let fragments = split_fragments("hi 🫩 bye");
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0], MessageFragment::Text("hi 🫩 bye"));
    }

    #[test]
    fn split_fragments_empty_input_yields_no_fragments() {
        assert!(split_fragments("").is_empty());
    }

    #[test]
    fn split_fragments_roundtrips_full_unicode_text() {
        // Concatenating every fragment's Unicode reproduces the input
        // exactly — nothing lost, reordered, or replaced.
        let input = "hello 😀 world 🇺🇸 👍🏻 \u{1f469}\u{200d}\u{1f4bb} bye";
        let fragments = split_fragments(input);
        let joined: String = fragments
            .iter()
            .map(|f| match f {
                MessageFragment::Text(s) => *s,
                MessageFragment::Emoji { unicode, .. } => *unicode,
            })
            .collect();
        assert_eq!(joined, input);
    }

    #[test]
    fn split_fragments_is_deterministic() {
        let inputs = [
            "hello 😀 world",
            "🇺🇸 👍🏻 \u{1f469}\u{200d}\u{1f4bb}",
            "a😀b🍕c",
            "",
            "plain text",
        ];
        for input in inputs {
            assert_eq!(
                split_fragments(input),
                split_fragments(input),
                "split must be deterministic for {input:?}"
            );
        }
    }

    // ── BORU-TWEMOJI-21: comprehensive sequence coverage ──────────────
    //
    // The PDF Task 21 suite: representative single-codepoint emoji,
    // variation-selector forms, all Fitzpatrick skin tones, regional-
    // indicator flag pairs, ZWJ sequences, symbol sequences, and mixed
    // strings with plain text + punctuation. Resolver tests assert the
    // exact vendored key; parser tests assert fragment boundaries (one
    // visual emoji = one fragment, never split into its codepoints).

    #[test]
    fn resolves_representative_single_codepoint_emoji() {
        // 😂 U+1F602 face with tears of joy — a second single-codepoint
        // representative beyond the T07 basics.
        assert_eq!(emoji_asset("😂").map(|a| a.key), Some("1f602"));
        // ✅ U+2705 check / ❌ U+274C cross — symbol sequences.
        assert_eq!(emoji_asset("✅").map(|a| a.key), Some("2705"));
        assert_eq!(emoji_asset("❌").map(|a| a.key), Some("274c"));
        // 🔥 U+1F525 fire / 🎉 U+1F389 celebration.
        assert_eq!(emoji_asset("🔥").map(|a| a.key), Some("1f525"));
        assert_eq!(emoji_asset("🎉").map(|a| a.key), Some("1f389"));
    }

    #[test]
    fn resolves_symbol_sequences_with_variation_selectors() {
        // ✔️ U+2714 U+FE0F — emoji presentation strips VS16 to the
        // vendored base key "2714" (raw "2714-fe0f" is not bundled).
        assert_eq!(emoji_asset("\u{2714}\u{fe0f}").map(|a| a.key), Some("2714"));
        // ⚠️ U+26A0 U+FE0F — warning, same strip-to-base rule.
        assert_eq!(emoji_asset("\u{26a0}\u{fe0f}").map(|a| a.key), Some("26a0"));
    }

    #[test]
    fn resolves_text_presentation_variation_selector_forms() {
        // ☺︎ U+263A U+FE0E — text presentation. Twemoji vendors no
        // text-presentation keys, so FE0E is dropped and the text-default
        // base key resolves.
        assert_eq!(emoji_asset("\u{263a}\u{fe0e}").map(|a| a.key), Some("263a"));
        // ☠︎ U+2620 U+FE0E — same rule for the skull.
        assert_eq!(emoji_asset("\u{2620}\u{fe0e}").map(|a| a.key), Some("2620"));
    }

    #[test]
    fn resolves_ireland_and_australia_flags() {
        // 🇮🇪 = U+1F1EE U+1F1EA — one flag pair, one key.
        assert_eq!(
            emoji_asset("\u{1f1ee}\u{1f1ea}").map(|a| a.key),
            Some("1f1ee-1f1ea")
        );
        // 🇦🇺 = U+1F1E6 U+1F1FA.
        assert_eq!(
            emoji_asset("\u{1f1e6}\u{1f1fa}").map(|a| a.key),
            Some("1f1e6-1f1fa")
        );
    }

    #[test]
    fn resolves_all_five_fitzpatrick_skin_tone_modifiers() {
        // 👍 + U+1F3FB..U+1F3FF — every Fitzpatrick modifier resolves to a
        // distinct vendored key (never a base-with-modifier split).
        let tones = [
            ("\u{1f44d}\u{1f3fb}", "1f44d-1f3fb"), // light
            ("\u{1f44d}\u{1f3fc}", "1f44d-1f3fc"), // medium-light
            ("\u{1f44d}\u{1f3fd}", "1f44d-1f3fd"), // medium
            ("\u{1f44d}\u{1f3fe}", "1f44d-1f3fe"), // medium-dark
            ("\u{1f44d}\u{1f3ff}", "1f44d-1f3ff"), // dark
        ];
        for (grapheme, expected) in tones {
            assert_eq!(
                emoji_asset(grapheme).map(|a| a.key),
                Some(expected),
                "skin tone grapheme {grapheme:?} must resolve to {expected}"
            );
        }
    }

    #[test]
    fn resolves_profession_gender_and_flag_zwj_sequences() {
        // 👨💻 = U+1F468 ZWJ U+1F4BB (man technologist) — the male
        // counterpart of the existing woman-technologist case.
        assert_eq!(
            emoji_asset("\u{1f468}\u{200d}\u{1f4bb}").map(|a| a.key),
            Some("1f468-200d-1f4bb")
        );
        // 🏳️🌈 = U+1F3F3 U+FE0F ZWJ U+1F308 — rainbow flag: Twemoji KEEPS
        // VS16 inside this ZWJ sequence, so the raw candidate must win.
        assert_eq!(
            emoji_asset("\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}").map(|a| a.key),
            Some("1f3f3-fe0f-200d-1f308")
        );
        // 🏴☠️ = U+1F3F4 ZWJ U+2620 U+FE0F — pirate flag, VS16 kept.
        assert_eq!(
            emoji_asset("\u{1f3f4}\u{200d}\u{2620}\u{fe0f}").map(|a| a.key),
            Some("1f3f4-200d-2620-fe0f")
        );
    }

    #[test]
    fn returns_none_for_sequence_classes_without_vendored_assets() {
        // 🇽🇽 = U+1F1FD U+1F1FD — "XX" is not an assigned ISO 3166-1 pair
        // (the manifest vendors real pairs, incl. 1f1fd-1f1f0 "XK", but
        // not this one). A valid grapheme, an unvendored key.
        assert_eq!(emoji_asset("\u{1f1fd}\u{1f1fd}"), None);
        // 😀‍😀 = U+1F600 ZWJ U+1F600 — a valid ZWJ grapheme cluster that is
        // not a registered emoji; the joined key is not bundled.
        assert_eq!(emoji_asset("\u{1f600}\u{200d}\u{1f600}"), None);
        // Codepoint-level fallbacks (newer-than-vendored 🫩 U+1FAE9 and
        // private-use U+10FFFF) are covered by
        // `returns_none_for_unknown_or_newer_emoji` above.
    }

    #[test]
    fn split_fragments_variation_selector_emoji_is_one_fragment() {
        // ❤️ = U+2764 U+FE0F — VS16 is part of the grapheme; the whole
        // cluster is ONE Emoji fragment resolving to the stripped base key.
        let fragments = split_fragments("\u{2764}\u{fe0f}");
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == "\u{2764}\u{fe0f}" && asset.key == "2764"
        ));
    }

    #[test]
    fn split_fragments_each_skin_tone_is_one_fragment() {
        // All five Fitzpatrick tones back-to-back: five Emoji fragments,
        // the modifier never split off into its own Text run, no gap runs.
        let input = "\u{1f44d}\u{1f3fb}\u{1f44d}\u{1f3fc}\u{1f44d}\u{1f3fd}\
                     \u{1f44d}\u{1f3fe}\u{1f44d}\u{1f3ff}";
        let fragments = split_fragments(input);
        let expected_keys = [
            "1f44d-1f3fb",
            "1f44d-1f3fc",
            "1f44d-1f3fd",
            "1f44d-1f3fe",
            "1f44d-1f3ff",
        ];
        assert_eq!(fragments.len(), expected_keys.len());
        for (i, (fragment, key)) in fragments.iter().zip(expected_keys).enumerate() {
            assert!(
                matches!(fragment, MessageFragment::Emoji { asset, .. } if asset.key == key),
                "fragment {i} must be Emoji with key {key}, got {fragment:?}"
            );
        }
    }

    #[test]
    fn split_fragments_ireland_and_australia_flags_are_single_fragments() {
        let fragments = split_fragments("\u{1f1ee}\u{1f1ea} \u{1f1e6}\u{1f1fa}");
        assert_eq!(fragments.len(), 3);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == "\u{1f1ee}\u{1f1ea}" && asset.key == "1f1ee-1f1ea"
        ));
        assert_eq!(fragments[1], MessageFragment::Text(" "));
        assert!(matches!(
            &fragments[2],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == "\u{1f1e6}\u{1f1fa}" && asset.key == "1f1e6-1f1fa"
        ));
    }

    #[test]
    fn split_fragments_profession_and_flag_zwj_are_single_fragments() {
        // 👨💻 man technologist — ZWJ, one fragment.
        let man_tech = "\u{1f468}\u{200d}\u{1f4bb}";
        let fragments = split_fragments(man_tech);
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == man_tech && asset.key == "1f468-200d-1f4bb"
        ));

        // 🏳️🌈 rainbow flag (VS16 inside the ZWJ sequence) — one fragment.
        let rainbow = "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}";
        let fragments = split_fragments(rainbow);
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == rainbow && asset.key == "1f3f3-fe0f-200d-1f308"
        ));

        // 🏴☠️ pirate flag — one fragment.
        let pirate = "\u{1f3f4}\u{200d}\u{2620}\u{fe0f}";
        let fragments = split_fragments(pirate);
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            &fragments[0],
            MessageFragment::Emoji { unicode, asset }
                if *unicode == pirate && asset.key == "1f3f4-200d-2620-fe0f"
        ));

        // The 4-component family ZWJ is covered as a single fragment by
        // `split_fragments_keeps_multicodepoint_emoji_as_single_fragment`.
    }

    #[test]
    fn split_fragments_symbol_sequences_each_one_fragment() {
        // ✅ ❌ ⚠️ 🔥 🎉 — every symbol is its own Emoji fragment; adjacent
        // emoji produce no empty Text runs between them.
        let fragments = split_fragments("✅❌\u{26a0}\u{fe0f}🔥🎉");
        let expected = ["2705", "274c", "26a0", "1f525", "1f389"];
        assert_eq!(fragments.len(), expected.len());
        for (i, (fragment, key)) in fragments.iter().zip(expected).enumerate() {
            assert!(
                matches!(fragment, MessageFragment::Emoji { asset, .. } if asset.key == key),
                "fragment {i} must be Emoji with key {key}, got {fragment:?}"
            );
        }
    }

    #[test]
    fn split_fragments_mixed_complex_string_with_punctuation() {
        // A realistic chat bubble: plain text, punctuation, multi-codepoint
        // emoji (skin tones, flag pairs, VS16 heart), symbols and emoji
        // clusters — every fragment boundary and the full roundtrip must
        // hold. This is the PDF Task 21 mixed-string case.
        let input = "Ship it! ✅🚀 See you at 9am 🇮🇪🇦🇺 — keep 👍🏻👍🏿 and ❤️. 🎉🔥";
        let fragments = split_fragments(input);
        let expected: Vec<(Option<&str>, &str)> = vec![
            (None, "Ship it! "),
            (Some("2705"), "✅"),
            (Some("1f680"), "🚀"),
            (None, " See you at 9am "),
            (Some("1f1ee-1f1ea"), "🇮🇪"),
            (Some("1f1e6-1f1fa"), "🇦🇺"),
            (None, " — keep "),
            (Some("1f44d-1f3fb"), "👍🏻"),
            (Some("1f44d-1f3ff"), "👍🏿"),
            (None, " and "),
            (Some("2764"), "❤️"),
            (None, ". "),
            (Some("1f389"), "🎉"),
            (Some("1f525"), "🔥"),
        ];
        assert_eq!(
            fragments.len(),
            expected.len(),
            "fragment count mismatch: {fragments:?}"
        );
        for (i, (fragment, (expected_key, expected_unicode))) in
            fragments.iter().zip(expected).enumerate()
        {
            match (fragment, expected_key) {
                (MessageFragment::Text(text), None) => {
                    assert_eq!(*text, expected_unicode, "Text fragment {i}");
                }
                (MessageFragment::Emoji { unicode, asset }, Some(key)) => {
                    assert_eq!(*unicode, expected_unicode, "Emoji fragment {i} unicode");
                    assert_eq!(asset.key, key, "Emoji fragment {i} key");
                }
                (fragment, key) => {
                    panic!("fragment {i} mismatch: got {fragment:?} but expected key {key:?}")
                }
            }
        }
        // Roundtrip: concatenating all fragment Unicode reproduces the
        // input byte-for-byte — parsing never alters the message text.
        let joined: String = fragments
            .iter()
            .map(|f| match f {
                MessageFragment::Text(s) => *s,
                MessageFragment::Emoji { unicode, .. } => *unicode,
            })
            .collect();
        assert_eq!(joined, input);
    }

    #[test]
    fn split_fragments_unvendored_sequences_stay_in_text_run() {
        // 🇽🇽 (fake flag pair) and 😀‍😀 (unregistered ZWJ) are valid
        // grapheme clusters with no vendored asset: they must be preserved
        // verbatim inside the surrounding text run — never split into
        // their codepoints, never suppressed.
        let input = "go \u{1f1fd}\u{1f1fd} \u{1f600}\u{200d}\u{1f600} now";
        let fragments = split_fragments(input);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0], MessageFragment::Text(input));
    }
}
