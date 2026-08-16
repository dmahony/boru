//! SVG handles, caching and the rendering abstraction (BORU-TWEMOJI-04
//! skeleton).
//!
//! This is the only module that knows Twemoji SVG filenames. Chat/network
//! code never references asset paths; it requests rendering through the
//! [`EmojiRenderer`] trait, which resolves a Unicode grapheme to an
//! [`EmojiAsset`] (key + repo-relative path).
//!
//! BORU-TWEMOJI-08 fills in the SVG handle production and BORU-TWEMOJI-09
//! adds the handle cache (mirroring `file_type_icon::SVG_HANDLE_CACHE`).

use std::path::PathBuf;

/// A resolved Twemoji presentation asset.
///
/// Guardrail: this is presentation metadata only. It never enters a chat
/// message, the wire format, or persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmojiAsset {
    /// Normalized Twemoji asset key, e.g. `"1f600"`.
    pub key: &'static str,
    /// Repo-relative path to the bundled SVG, e.g.
    /// `assets/emoji/twemoji/svg/1f600.svg`.
    pub path: PathBuf,
}

impl EmojiAsset {
    /// Build an asset for a catalog key using the vendored layout convention.
    pub fn from_key(key: &'static str) -> Self {
        Self {
            key,
            path: PathBuf::from(format!("assets/emoji/twemoji/svg/{key}.svg")),
        }
    }
}

/// Rendering abstraction shared by the picker and the message renderer.
///
/// Both surfaces request artwork through this trait (or the free
/// [`crate::emoji::parser::emoji_asset`] helper) rather than touching SVG
/// paths directly. Swapping the artwork set later only changes the
/// implementation behind this trait.
pub trait EmojiRenderer {
    /// Resolve a Unicode grapheme to a bundled asset, or `None` for fallback
    /// to the original Unicode text.
    fn resolve(&self, grapheme: &str) -> Option<EmojiAsset>;
}

/// Twemoji-backed renderer using the vendored SVG set.
///
/// Resolution delegates to the single central resolver
/// [`crate::emoji::parser::emoji_asset`] (BORU-TWEMOJI-07), so the emoji
/// module has exactly one source of Unicode→key conversion. BORU-TWEMOJI-08
/// fills in SVG handle production behind this trait.
#[derive(Debug, Clone, Copy, Default)]
pub struct TwemojiRenderer;

impl EmojiRenderer for TwemojiRenderer {
    fn resolve(&self, grapheme: &str) -> Option<EmojiAsset> {
        super::parser::emoji_asset(grapheme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twemoji_renderer_resolves_catalog_entries() {
        let r = TwemojiRenderer;
        let asset = r.resolve("😀").expect("grinning face is in the catalog");
        assert_eq!(asset.key, "1f600");
        assert_eq!(
            asset.path,
            PathBuf::from("assets/emoji/twemoji/svg/1f600.svg")
        );
    }

    #[test]
    fn twemoji_renderer_resolves_multicodepoint_graphemes() {
        let r = TwemojiRenderer;
        // Flag pair, skin tone, and a ZWJ sequence — one grapheme each.
        assert_eq!(
            r.resolve("\u{1f1fa}\u{1f1f8}").map(|a| a.key),
            Some("1f1fa-1f1f8")
        );
        assert_eq!(
            r.resolve("\u{1f44d}\u{1f3fd}").map(|a| a.key),
            Some("1f44d-1f3fd")
        );
        assert_eq!(
            r.resolve("\u{1f469}\u{200d}\u{1f4bb}").map(|a| a.key),
            Some("1f469-200d-1f4bb")
        );
        // VS16 is stripped for hearts (vendored as "2764").
        assert_eq!(r.resolve("\u{2764}\u{fe0f}").map(|a| a.key), Some("2764"));
    }

    #[test]
    fn twemoji_renderer_falls_back_to_none_for_unknown() {
        let r = TwemojiRenderer;
        assert_eq!(r.resolve("plain text"), None);
        // 🫩 face with bags under eyes — Unicode 16.0, not vendored.
        assert_eq!(r.resolve("🫩"), None);
    }

    #[test]
    fn asset_path_follows_vendored_layout() {
        let asset = EmojiAsset::from_key("2764");
        assert_eq!(
            asset.path.to_string_lossy(),
            "assets/emoji/twemoji/svg/2764.svg"
        );
    }
}
