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
/// Skeleton implementation: resolves through the curated catalog
/// (`catalog::common_emojis`). BORU-TWEMOJI-07/08 replace the resolution and
/// add real SVG handle production.
#[derive(Debug, Clone, Copy, Default)]
pub struct TwemojiRenderer;

impl EmojiRenderer for TwemojiRenderer {
    fn resolve(&self, grapheme: &str) -> Option<EmojiAsset> {
        super::parser::catalog_lookup(grapheme).map(|e| EmojiAsset::from_key(e.asset))
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
    fn twemoji_renderer_falls_back_to_none_for_unknown() {
        let r = TwemojiRenderer;
        assert_eq!(r.resolve("plain text"), None);
        assert_eq!(r.resolve("🦤"), None);
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
