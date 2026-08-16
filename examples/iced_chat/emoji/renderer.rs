//! SVG handles, caching and the rendering abstraction (BORU-TWEMOJI-04
//! skeleton, filled in by BORU-TWEMOJI-08).
//!
//! This is the only module that knows Twemoji SVG filenames. Chat/network
//! code never references asset paths; it requests rendering through the
//! [`EmojiRenderer`] trait, which resolves a Unicode grapheme to an
//! [`EmojiAsset`] (key + repo-relative path) and produces the Iced SVG
//! handle for that asset.
//!
//! Handle *production* (reading the vendored SVG bytes) lives here;
//! BORU-TWEMOJI-09 adds the handle *cache* on top (mirroring
//! `file_type_icon::SVG_HANDLE_CACHE`) so scrolling a chat or browsing the
//! picker does not re-read the same SVG on every frame.

use std::path::{Path, PathBuf};

use iced::widget::svg;

/// Repo-relative root of the vendored Twemoji bundle
/// (`assets/emoji/twemoji/svg/<key>.svg` layout).
pub const EMOJI_ASSET_ROOT: &str = "assets/emoji/twemoji";

/// Environment variable that overrides where the bundled Twemoji asset root
/// is loaded from at runtime (packaging), mirroring `BORU_PAPIRUS_ASSETS`.
const EMOJI_ASSETS_ENV: &str = "BORU_EMOJI_ASSETS";

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
            path: PathBuf::from(format!("{EMOJI_ASSET_ROOT}/svg/{key}.svg")),
        }
    }
}

/// Rendering abstraction shared by the picker and the message renderer.
///
/// Both surfaces request artwork through this trait rather than touching SVG
/// paths directly. Swapping the artwork set later only changes the
/// implementation behind this trait — chat/network code and the shared
/// consumer shape stay untouched.
pub trait EmojiRenderer {
    /// Resolve a Unicode grapheme to a bundled asset, or `None` for fallback
    /// to the original Unicode text.
    fn resolve(&self, grapheme: &str) -> Option<EmojiAsset>;

    /// Produce the Iced SVG handle for a resolved asset by loading the
    /// vendored SVG bytes.
    ///
    /// Returns `None` when the bundled file cannot be read (packaging edge
    /// case) — the caller then falls back to the original Unicode text,
    /// never a broken image. Handle *caching* is BORU-TWEMOJI-09; this
    /// method performs the load, so hot paths should cache the result.
    fn svg_handle(&self, asset: &EmojiAsset) -> Option<svg::Handle>;
}

/// Twemoji-backed renderer using the vendored SVG set.
///
/// Resolution delegates to the single central resolver
/// [`crate::emoji::parser::emoji_asset`] (BORU-TWEMOJI-07), so the emoji
/// module has exactly one source of Unicode→key conversion; handle
/// production reads the vendored SVG from the runtime asset root.
#[derive(Debug, Clone, Copy, Default)]
pub struct TwemojiRenderer;

impl EmojiRenderer for TwemojiRenderer {
    fn resolve(&self, grapheme: &str) -> Option<EmojiAsset> {
        super::parser::emoji_asset(grapheme)
    }

    fn svg_handle(&self, asset: &EmojiAsset) -> Option<svg::Handle> {
        read_vendored_svg(asset).map(svg::Handle::from_memory)
    }
}

/// Resolve the on-disk Twemoji bundle root for this process.
///
/// Priority mirrors the Papirus pattern (`file_type_icon.rs`, PAPIRUS-17):
/// 1. `BORU_EMOJI_ASSETS` env var — explicit override for packagers.
/// 2. `<exe_dir>/assets/emoji/twemoji` — release packages ship the binary
///    and the asset bundle side by side under this layout.
/// 3. `<exe_dir>/../assets/emoji/twemoji` — binary in a `bin/`-style
///    subdirectory beside a shared assets tree.
/// 4. `<cwd>/assets/emoji/twemoji` — ad-hoc "copy the binary into a folder
///    that also has assets" layout.
/// 5. `CARGO_MANIFEST_DIR/assets/emoji/twemoji` — dev builds and source
///    checkouts.
fn emoji_asset_root() -> Option<PathBuf> {
    let env_override = std::env::var_os(EMOJI_ASSETS_ENV).map(PathBuf::from);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let cwd = std::env::current_dir().ok();
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    resolve_asset_root(
        env_override.as_deref(),
        exe_dir.as_deref(),
        cwd.as_deref(),
        manifest_dir,
    )
}

/// Pure core of [`emoji_asset_root`]: pick the first candidate root that
/// actually contains the bundled SVGs (testable without touching process
/// state).
fn resolve_asset_root(
    env_override: Option<&Path>,
    exe_dir: Option<&Path>,
    cwd: Option<&Path>,
    manifest_dir: &Path,
) -> Option<PathBuf> {
    // The `svg/1f600.svg` file is a stable member of the vendored bundle.
    let probe = |root: &Path| root.join("svg").join("1f600.svg").is_file();
    if let Some(dir) = env_override {
        if probe(dir) {
            return Some(dir.to_path_buf());
        }
    }
    if let Some(dir) = exe_dir {
        let p = dir.join("assets").join("emoji").join("twemoji");
        if probe(&p) {
            return Some(p);
        }
        if let Some(parent) = dir.parent() {
            let p = parent.join("assets").join("emoji").join("twemoji");
            if probe(&p) {
                return Some(p);
            }
        }
    }
    if let Some(dir) = cwd {
        let p = dir.join("assets").join("emoji").join("twemoji");
        if probe(&p) {
            return Some(p);
        }
    }
    let p = manifest_dir.join(EMOJI_ASSET_ROOT);
    if probe(&p) {
        Some(p)
    } else {
        None
    }
}

/// Read the vendored SVG bytes for a resolved asset from an already-resolved
/// runtime root. `None` means the bundle root could not be resolved or the
/// file is missing — callers fall back to the original Unicode text.
fn read_vendored_svg(asset: &EmojiAsset) -> Option<Vec<u8>> {
    let root = emoji_asset_root()?;
    // `asset.path` is repo-relative (`assets/emoji/twemoji/svg/<key>.svg`);
    // strip the root prefix and join the remainder onto the resolved root.
    let root_prefix = format!("{EMOJI_ASSET_ROOT}/");
    let relative = asset
        .path
        .strip_prefix(root_prefix.as_str())
        .unwrap_or(&asset.path);
    std::fs::read(root.join(relative)).ok()
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

    #[test]
    fn twemoji_renderer_produces_svg_handle_from_vendored_asset() {
        let r = TwemojiRenderer;
        let asset = r.resolve("😀").expect("grinning face is in the catalog");
        let handle = r
            .svg_handle(&asset)
            .expect("vendored SVG exists in a source checkout");
        // The handle must carry the in-memory SVG bytes (not a filesystem
        // path): identical to a handle built directly from the vendored
        // file contents.
        let vendored = read_vendored_svg(&asset).expect("vendored SVG readable");
        if let Some(text) = std::str::from_utf8(&vendored).ok() {
            assert!(
                text.contains("<svg"),
                "expected SVG markup, got: {text:.60}"
            );
        }
        let expected = svg::Handle::from_memory(vendored);
        assert_eq!(handle.data(), expected.data());
    }

    #[test]
    fn svg_handle_none_when_vendored_file_unreadable() {
        let r = TwemojiRenderer;
        // A well-formed key with no vendored file behind it.
        let asset = EmojiAsset::from_key("zzzz-not-vendored");
        assert!(r.svg_handle(&asset).is_none());
    }

    #[test]
    fn asset_root_resolves_for_dev_checkout() {
        let root = emoji_asset_root().expect("dev checkout has vendored twemoji");
        assert!(root.join("svg").join("1f600.svg").is_file());
    }

    #[test]
    fn asset_root_candidates_are_checked_in_priority_order() {
        let real_root = emoji_asset_root().expect("dev checkout has vendored twemoji");
        // Env override wins when it points at a real bundle.
        let root = resolve_asset_root(
            Some(real_root.as_path()),
            None,
            None,
            Path::new("/nonexistent"),
        );
        assert_eq!(root, Some(real_root));
        // A bogus override is skipped, falling through to later candidates.
        assert!(resolve_asset_root(
            Some(Path::new("/nonexistent/override")),
            None,
            None,
            Path::new("/nonexistent"),
        )
        .is_none());
    }

    /// A stub renderer with a *different* asset layout. The consumer shape
    /// below proves the trait boundary: swapping the implementation does not
    /// require changes to the consumer, networking, or message data.
    struct StubRenderer;

    impl EmojiRenderer for StubRenderer {
        fn resolve(&self, grapheme: &str) -> Option<EmojiAsset> {
            if grapheme == "😀" {
                Some(EmojiAsset {
                    key: "stub-key",
                    path: PathBuf::from("stub/1f600.svg"),
                })
            } else {
                None
            }
        }

        fn svg_handle(&self, asset: &EmojiAsset) -> Option<svg::Handle> {
            let _ = asset;
            Some(svg::Handle::from_memory(
                b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
            ))
        }
    }

    /// The shared consumer shape both the picker and the message renderer
    /// use: resolve a grapheme through the trait, then request the render
    /// surface. It only knows [`EmojiRenderer`], never a concrete impl.
    fn consumer_shape(renderer: &dyn EmojiRenderer, grapheme: &str) -> Option<EmojiAsset> {
        renderer.resolve(grapheme)
    }

    #[test]
    fn swapping_renderer_impl_requires_no_consumer_changes() {
        let twemoji = TwemojiRenderer;
        let stub = StubRenderer;

        // The same consumer shape works for both implementations...
        let a = consumer_shape(&twemoji, "😀").expect("twemoji resolves grinning face");
        assert_eq!(a.key, "1f600");
        let b = consumer_shape(&stub, "😀").expect("stub resolves grinning face");
        assert_eq!(b.key, "stub-key");

        // ...and both agree on fallback behaviour for unknown graphemes.
        assert_eq!(consumer_shape(&twemoji, "🫩"), None);
        assert_eq!(consumer_shape(&stub, "🫩"), None);
    }

    #[test]
    fn message_wire_format_never_carries_asset_paths() {
        // A chat message stores and transmits the raw Unicode emoji. Even
        // after swapping the renderer, the wire format is unchanged — no
        // asset key, filename, or SVG path ever enters message data.
        let msg = crate::Message::Message {
            text: "hi 😀".to_string(),
        };
        let bytes = postcard::to_stdvec(&msg).expect("message serializes");
        let encoded = String::from_utf8_lossy(&bytes);
        assert!(encoded.contains("hi 😀"), "raw Unicode text is preserved");
        assert!(
            !encoded.contains("assets/emoji"),
            "no asset path in wire format"
        );
        assert!(!encoded.contains(".svg"), "no SVG filename in wire format");
        assert!(!encoded.contains("1f600"), "no asset key in wire format");
    }
}
