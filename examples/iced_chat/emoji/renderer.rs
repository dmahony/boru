//! SVG handles, caching and the rendering abstraction (BORU-TWEMOJI-04
//! skeleton, filled in by BORU-TWEMOJI-08).
//!
//! This is the only module that knows Twemoji SVG filenames. Chat/network
//! code never references asset paths; it requests rendering through the
//! [`EmojiRenderer`] trait, which resolves a Unicode grapheme to an
//! [`EmojiAsset`] (key + repo-relative path) and produces the Iced SVG
//! handle for that asset.
//!
//! Handle *production* (reading the vendored SVG bytes) and the handle
//! *cache* live here (BORU-TWEMOJI-09, mirroring
//! `file_type_icon::SVG_HANDLE_CACHE`): scrolling a chat or browsing the
//! picker re-reads each vendored SVG at most once per process, never on
//! every frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
    /// never a broken image. Implementations should route hot paths through
    /// the [`EmojiAssetCache`] (BORU-TWEMOJI-09) so repeated rendering of
    /// the same emoji reuses the cached handle instead of re-reading the
    /// file on every frame.
    fn svg_handle(&self, asset: &EmojiAsset) -> Option<svg::Handle>;

    /// One-step resolve + handle production — the shared fallback decision
    /// every emoji surface makes (picker BORU-TWEMOJI-10, message renderer
    /// BORU-TWEMOJI-17, recents BORU-TWEMOJI-14, any future surface).
    ///
    /// Returns `Some(handle)` when the grapheme resolves to a vendored
    /// asset whose SVG can be loaded (render the Twemoji SVG), `None`
    /// otherwise (render the original Unicode grapheme with normal text
    /// rendering — BORU-TWEMOJI-20: never hide, drop or replace an
    /// unsupported emoji with an empty widget).
    ///
    /// The default implementation composes [`Self::resolve`] and
    /// [`Self::svg_handle`]; implementations that already hold a resolved
    /// asset (e.g. the fragment-based message plan) may call
    /// [`Self::svg_handle`] directly — the decision rule is identical.
    fn artwork(&self, grapheme: &str) -> Option<svg::Handle> {
        self.resolve(grapheme)
            .and_then(|asset| self.svg_handle(&asset))
    }
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
        cached_svg_handle(asset)
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
///
/// Missing assets are logged at DEBUG level only (BORU-TWEMOJI-20): this is
/// a packaging edge case, not an expected production event, and the hot
/// path (message rendering) must not produce noisy logs. The rate is
/// naturally bounded because callers route through [`cached_svg_handle`],
/// which caches misses, so a given missing asset is logged at most once per
/// process.
fn read_vendored_svg(asset: &EmojiAsset) -> Option<Vec<u8>> {
    let Some(root) = emoji_asset_root() else {
        tracing::debug!(
            asset = %asset.key,
            "emoji: Twemoji asset root not found; \
             falling back to original Unicode text"
        );
        return None;
    };
    // `asset.path` is repo-relative (`assets/emoji/twemoji/svg/<key>.svg`);
    // strip the root prefix and join the remainder onto the resolved root.
    let root_prefix = format!("{EMOJI_ASSET_ROOT}/");
    let relative = asset
        .path
        .strip_prefix(root_prefix.as_str())
        .unwrap_or(&asset.path);
    match std::fs::read(root.join(relative)) {
        Ok(bytes) => Some(bytes),
        Err(err) => {
            tracing::debug!(
                asset = %asset.key,
                path = %root.join(relative).display(),
                error = %err,
                "emoji: vendored Twemoji SVG missing or unreadable; \
                 falling back to original Unicode text"
            );
            None
        }
    }
}

/// App-lifetime cache of decoded Twemoji SVG handles, keyed by normalized
/// asset ID (BORU-TWEMOJI-09).
///
/// Mirrors `file_type_icon::SVG_HANDLE_CACHE` (PAPIRUS-17): the first
/// render of a given emoji pays the file read + handle construction, and
/// every later render clones the handle (O(1) — `svg::Handle` wraps an
/// `Arc`) instead of re-reading the SVG. Scrolling a chat or browsing the
/// picker therefore causes at most one disk read per distinct emoji.
///
/// ## Lifetime
///
/// Process-global, app lifetime. The vendored Twemoji asset set is
/// immutable while the app runs (pinned at BORU-TWEMOJI-02 and only
/// regenerated by a checked-in repository script, never at runtime), so a
/// handle — or a recorded miss — loaded once can never go stale. There is
/// no asset hot-reload or re-resolution that could invalidate an entry,
/// which is exactly why the cache is safe to key by the bare normalized
/// asset ID rather than re-validating paths on every lookup.
#[derive(Debug, Default)]
pub struct EmojiAssetCache {
    handles: Mutex<HashMap<String, Option<svg::Handle>>>,
}

impl EmojiAssetCache {
    /// Create an empty cache (used by the process-global instance and by
    /// tests that need a fresh, isolated seam).
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct asset IDs currently in the cache (hits and
    /// recorded misses).
    ///
    /// Test-only (BORU-TWEMOJI-24): production code only inserts and reads
    /// cache entries; the size queries are compiled only in test builds.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.handles.lock().unwrap().len()
    }

    /// True when no asset ID has been resolved yet.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the cached handle for `asset_id`, or load it via `load`,
    /// store it, and return it.
    ///
    /// This is the testable seam: callers (or tests) pass a loader that may
    /// count or intercept file reads, and a second request for the same
    /// asset ID must not invoke `load` again. Both successful handles and
    /// misses (`None`) are cached: the asset set is immutable, so a missing
    /// file now will still be missing on the next frame — caching the miss
    /// prevents repeated failed reads in the broken-packaging case.
    pub fn get_or_load(
        &self,
        asset_id: &str,
        load: impl FnOnce() -> Option<svg::Handle>,
    ) -> Option<svg::Handle> {
        if let Some(cached) = self.handles.lock().unwrap().get(asset_id) {
            return cached.clone();
        }
        let handle = load();
        self.handles
            .lock()
            .unwrap()
            .insert(asset_id.to_string(), handle.clone());
        handle
    }
}

/// The process-global emoji cache instance, shared by the picker and the
/// message renderer for the whole application lifetime.
static EMOJI_ASSET_CACHE: OnceLock<EmojiAssetCache> = OnceLock::new();

/// Fetch (and cache) the SVG handle for a resolved emoji asset.
///
/// Keyed by the normalized asset ID (`asset.key`, e.g. `"1f600"`), so the
/// same emoji appearing many times in a chat or picker reuses one handle.
/// Returns `None` for a missing/unreadable vendored file — callers fall
/// back to the original Unicode text, never a broken image.
pub fn cached_svg_handle(asset: &EmojiAsset) -> Option<svg::Handle> {
    let cache = EMOJI_ASSET_CACHE.get_or_init(EmojiAssetCache::new);
    cache.get_or_load(asset.key, || {
        read_vendored_svg(asset).map(svg::Handle::from_memory)
    })
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

    /// The shared fallback helper (`artwork`) resolves AND loads in one
    /// step: a supported grapheme yields the SVG handle exactly like the
    /// two-step resolve + svg_handle path.
    #[test]
    fn artwork_resolves_and_loads_supported_emoji() {
        let r = TwemojiRenderer;
        let handle = r.artwork("😀").expect("grinning face resolves + loads");
        let expected = r
            .svg_handle(&r.resolve("😀").expect("grinning face resolves"))
            .expect("vendored SVG exists");
        assert_eq!(handle.data(), expected.data());
    }

    /// The shared fallback helper returns `None` for unknown/newer emoji —
    /// the caller then renders the original Unicode text (BORU-TWEMOJI-20).
    #[test]
    fn artwork_returns_none_for_unknown_emoji() {
        let r = TwemojiRenderer;
        assert_eq!(r.artwork("🫩"), None);
        assert_eq!(r.artwork("plain text"), None);
    }

    /// The shared fallback helper returns `None` when the vendored file is
    /// missing — the caller falls back to text instead of a broken image.
    #[test]
    fn artwork_returns_none_when_svg_file_missing() {
        let r = TwemojiRenderer;
        let missing = EmojiAsset {
            key: "zzzz-missing-file-test",
            path: PathBuf::from("assets/emoji/twemoji/svg/definitely-missing.svg"),
        };
        // `artwork` composes resolve + svg_handle; for a *resolved* key
        // whose file is absent, emulate the resolved-asset path through
        // svg_handle directly (the trait contract both surfaces share).
        assert!(r.svg_handle(&missing).is_none());
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

    /// Repeated rendering of the same emoji reuses the cached handle: a
    /// fresh cache with a counting loader performs exactly one load for two
    /// requests of the same normalized asset ID, and both requests return
    /// the identical handle bytes.
    #[test]
    fn cache_reuses_handle_without_reloading() {
        let cache = EmojiAssetCache::new();
        let loads = std::cell::Cell::new(0usize);
        let load = || {
            loads.set(loads.get() + 1);
            Some(svg::Handle::from_memory(b"<svg/>".to_vec()))
        };
        let a = cache.get_or_load("1f600", load);
        let b = cache.get_or_load("1f600", load);
        assert!(a.is_some() && b.is_some());
        assert_eq!(a.unwrap().data(), b.unwrap().data());
        assert_eq!(loads.get(), 1, "loader must run exactly once per asset ID");
        assert_eq!(cache.len(), 1);
    }

    /// Different asset IDs are independent cache entries; each pays its own
    /// single load.
    #[test]
    fn cache_keys_are_per_asset_id() {
        let cache = EmojiAssetCache::new();
        let loads = std::cell::Cell::new(0usize);
        let load = || {
            loads.set(loads.get() + 1);
            Some(svg::Handle::from_memory(b"<svg/>".to_vec()))
        };
        let _ = cache.get_or_load("1f600", load);
        let _ = cache.get_or_load("1f600", load);
        let _ = cache.get_or_load("1f602", load);
        assert_eq!(loads.get(), 2, "two distinct asset IDs -> two loads");
        assert_eq!(cache.len(), 2);
    }

    /// A miss (missing/unreadable file) is recorded so the broken-packaging
    /// case does not re-read the file on every frame; the caller still gets
    /// `None` and falls back to the original Unicode text.
    #[test]
    fn cache_records_miss_and_still_returns_none() {
        let cache = EmojiAssetCache::new();
        let loads = std::cell::Cell::new(0usize);
        let load = || {
            loads.set(loads.get() + 1);
            None
        };
        assert_eq!(cache.get_or_load("zzzz-not-vendored", load), None);
        assert_eq!(cache.get_or_load("zzzz-not-vendored", load), None);
        assert_eq!(loads.get(), 1, "miss is cached; no repeated failed reads");
        assert_eq!(cache.len(), 1);
    }

    /// Through the renderer, scrolling a chat with a repeated emoji does not
    /// re-read the SVG: two `svg_handle` requests for the same asset reuse
    /// the cached handle (identical bytes) and the process-global cache
    /// holds exactly one entry for that asset ID.
    #[test]
    fn twemoji_renderer_reuses_cached_handle_across_calls() {
        let r = TwemojiRenderer;
        let asset = r.resolve("😀").expect("grinning face is in the catalog");
        let a = r.svg_handle(&asset).expect("vendored SVG exists");
        let b = r.svg_handle(&asset).expect("vendored SVG exists");
        assert_eq!(a.data(), b.data());
        // Count entries for THIS key rather than the whole-map delta: other
        // tests run in parallel and may legitimately insert distinct asset
        // IDs into the process-global cache.
        let count_for = |cache: &EmojiAssetCache, key: &str| {
            cache
                .handles
                .lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| k.as_str() == key)
                .count()
        };
        let cache = EMOJI_ASSET_CACHE.get().expect("global cache initialized");
        assert_eq!(count_for(cache, "1f600"), 1, "one entry per asset ID");
    }

    /// The cache is keyed by the normalized asset ID, not the filesystem
    /// path: two assets carrying the same key but different paths (a
    /// defensive caller error) still resolve to one cached handle, so a
    /// stale path can never be produced — the key is the identity.
    #[test]
    fn cache_keyed_by_asset_id_not_path() {
        let cache = EmojiAssetCache::new();
        let loads = std::cell::Cell::new(0usize);
        let load = || {
            loads.set(loads.get() + 1);
            Some(svg::Handle::from_memory(b"<svg/>".to_vec()))
        };
        let asset_a = EmojiAsset {
            key: "1f600",
            path: PathBuf::from("assets/emoji/twemoji/svg/1f600.svg"),
        };
        let asset_b = EmojiAsset {
            key: "1f600",
            path: PathBuf::from("some/other/1f600.svg"),
        };
        let _ = cache.get_or_load(asset_a.key, load);
        let _ = cache.get_or_load(asset_b.key, load);
        assert_eq!(loads.get(), 1, "same asset ID, different path -> one load");
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

    /// A missing vendored SVG logs at DEBUG level (BORU-TWEMOJI-20) — never
    /// at a production-visible level — and the caller still receives `None`,
    /// so a missing file can neither crash rendering nor produce noisy logs.
    ///
    /// Uses a unique key so the process-global cache miss is fresh (the
    /// loader — and therefore the log — runs exactly once per asset ID).
    #[test]
    #[n0_tracing_test::traced_test]
    fn missing_svg_logs_at_debug_level() {
        let r = TwemojiRenderer;
        let missing = EmojiAsset {
            key: "zzzz-missing-log-test",
            path: PathBuf::from("assets/emoji/twemoji/svg/zzzz-missing-log-test.svg"),
        };
        assert!(r.svg_handle(&missing).is_none());
        assert!(
            logs_contain("missing or unreadable"),
            "missing asset must log at debug level"
        );
    }
}
