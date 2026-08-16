//! Unicode grapheme/emoji detection and asset-key resolution (BORU-TWEMOJI-04
//! skeleton).
//!
//! The full grapheme-safe resolver lands in BORU-TWEMOJI-07 (which will add
//! `unicode-segmentation` as a direct dependency or a small grapheme walk —
//! see `docs/emoji/emoji-audit.md` §5). This skeleton exposes the stable
//! parsing surface the renderer and picker will share, with a minimal
//! catalog-backed lookup so the module compiles and is testable now.
//!
//! Guardrails:
//! - Never assume one Rust `char` equals one visual emoji.
//! - Unknown/newer emoji return `None` → callers fall back to the original
//!   Unicode text.

use crate::emoji::catalog;
use crate::emoji::renderer::{EmojiAsset, TwemojiRenderer};

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

/// Resolve a single Unicode grapheme (or plain text run) to a bundled Twemoji
/// asset, or `None` when no vendored asset exists.
///
/// Skeleton implementation: exact-match lookup against the curated catalog.
/// BORU-TWEMOJI-07 replaces this with full normalization (variation
/// selectors, skin tones, regional-indicator flags, ZWJ sequences) validated
/// against the generated asset manifest (BORU-TWEMOJI-06).
pub fn emoji_asset(grapheme: &str) -> Option<EmojiAsset> {
    let renderer = TwemojiRenderer;
    use crate::emoji::renderer::EmojiRenderer as _;
    renderer.resolve(grapheme)
}

/// Split message text into text/emoji fragments.
///
/// Skeleton implementation: returns the whole input as a single `Text`
/// fragment. BORU-TWEMOJI-16 walks grapheme clusters and coalesces adjacent
/// non-emoji runs; this keeps the interface stable until then.
pub fn split_fragments(input: &str) -> Vec<MessageFragment<'_>> {
    vec![MessageFragment::Text(input)]
}

/// Look up a catalog entry by its Unicode value (used by the renderer's
/// skeleton resolver).
pub(crate) fn catalog_lookup(unicode: &str) -> Option<&'static catalog::Emoji> {
    catalog::common_emojis()
        .iter()
        .find(|e| e.unicode == unicode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_asset_resolves_known_entries() {
        assert_eq!(emoji_asset("😀").map(|a| a.key), Some("1f600"));
        assert_eq!(emoji_asset("❤️").map(|a| a.key), Some("2764"));
        assert_eq!(emoji_asset("🍕").map(|a| a.key), Some("1f355"));
    }

    #[test]
    fn emoji_asset_returns_none_for_unknown_or_plain_text() {
        assert_eq!(emoji_asset("hello"), None);
        assert_eq!(emoji_asset("🦤"), None); // dodo: not in curated list
        assert_eq!(emoji_asset(""), None);
    }

    #[test]
    fn split_fragments_skeleton_is_single_text_run() {
        let fragments = split_fragments("hello 😀 world");
        assert_eq!(fragments.len(), 1);
        assert!(matches!(
            fragments[0],
            MessageFragment::Text("hello 😀 world")
        ));
    }
}
