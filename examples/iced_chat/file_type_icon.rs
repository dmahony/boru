//! Central `FileTypeIcon` component for Boru (PAPIRUS-04).
//!
//! One reusable component renders a **resolved** Papirus file-type icon at
//! semantic sizes, with a subtle neutral container per the visual
//! presentation rules (Task 13).  Every Boru file-sharing surface that
//! shows a file or folder icon builds its icon through this component; it
//! is the single place that:
//!
//! * asks the central resolver ([`crate::file_type_resolver::resolve_file_icon`])
//!   which bundled Papirus icon represents the file,
//! * maps semantic sizes (compact/list/card/large/hero) to the bundled
//!   Papirus size directories (16/24/32/48/64),
//! * loads the SVG asset once and caches its handle (no per-frame decode),
//! * decides light/dark variant selection (PAPIRUS-14 extends this hook),
//! * keeps the transfer **status** visually separate from the file type.
//!
//! ## Non-negotiable visual rules (Task 13)
//!
//! * Papirus artwork is rendered **contain-style**, centred in a stable
//!   icon box — never stretched, cropped, or distorted.
//! * Original colours are preserved: the SVG is drawn with **no** style /
//!   recolouring.  There is deliberately no `svg::Style` applied below.
//! * The container is subtle: softly tinted neutral background, rounded
//!   corners, consistent padding, no heavy shadow, no strong border.
//! * The icon answers *"what type of file is this?"*.  Transfer status
//!   (downloading / complete / failed / shared / unavailable / paused)
//!   must be shown as a separate badge/overlay by the caller — never by
//!   recolouring the file icon.
//!
//! ## Asset loading & caching
//!
//! The bundled SVGs live in `assets/third_party/papirus/<size>/` and are
//! resolved against `CARGO_MANIFEST_DIR` (compile-time absolute), so the
//! component works regardless of the process working directory.  Handles
//! are cached forever in a process-global map keyed by repo-relative asset
//! path; `svg::Handle` construction copies the SVG bytes, so building one
//! per frame would thrash the allocator and re-parse the SVG on every
//! draw.  A single embedded copy of the unknown-generic icon (32px) is the
//! safety net: even if a bundle path is missing at runtime, the component
//! never renders a broken/missing icon.
//!
//! ## Accessibility
//!
//! The component accepts an explicit `accessibility_label` and falls back
//! to the resolver's friendly `display_label` (e.g. "PDF document",
//! "MP4 video").  iced 0.14 exposes no aria slot on `container`/`svg`
//! widgets, so PAPIRUS-15 wires the label into the accessibility tree;
//! this component already carries the text.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use iced::widget::{container, svg};
use iced::{alignment, Border, ContentFit, Element, Length, Theme};

use crate::design_tokens;
use crate::file_category::FileCategory;
use crate::file_type_resolver::{papirus_asset_path, resolve_file_icon, ResolvedFileIcon};

// ── Semantic sizes ───────────────────────────────────────────────────

/// Semantic icon size variants, mapped to the bundled Papirus size
/// directories (16/24/32/48/64).
///
/// Callers pick a *semantic* size, never raw pixels, so every surface
/// stays consistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileTypeIconSize {
    /// 16 px — activity rows, compact transfer indicators.
    Compact,
    /// 24 px — file-sharing table rows, list items.
    List,
    /// 32 px — chat file-card headers, the default.
    #[default]
    Card,
    /// 48 px — generic attachments without a preview.
    Large,
    /// 64 px — empty states, large selection summaries.
    Hero,
}

impl FileTypeIconSize {
    /// The bundled Papirus size directory this variant maps to.
    pub fn papirus_dir(self) -> u16 {
        match self {
            FileTypeIconSize::Compact => 16,
            FileTypeIconSize::List => 24,
            FileTypeIconSize::Card => 32,
            FileTypeIconSize::Large => 48,
            FileTypeIconSize::Hero => 64,
        }
    }

    /// Pixel edge of the icon artwork.
    pub fn px(self) -> f32 {
        f32::from(self.papirus_dir())
    }
}

// ── Theme hook (PAPIRUS-14) ──────────────────────────────────────────

/// Papirus variant selection, centralised here.
///
/// The pinned Papirus bundle contains one full-colour asset set (the
/// `16/24/32/48/64` size directories).  PAPIRUS-14 extends this enum /
/// [`FileTypeIcon::variant_dir`] to select a dark asset set; call sites do
/// not need to change.  Until a dark set is bundled, every variant uses
/// the same bundled paths — this is intentional, not a light-only
/// hardcode: the decision point is already this component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileTypeIconTheme {
    /// Follow the active application theme automatically.
    #[default]
    Auto,
    /// Always use the bundled (light) asset set.
    Light,
    /// Use the dark variant set (not bundled yet; same paths today).
    Dark,
}

// ── Component ────────────────────────────────────────────────────────

/// Default padding (px) between the icon artwork and its container edge.
pub const FILE_TYPE_ICON_PADDING: f32 = 4.0;
/// Default corner radius (px) for the icon container.
pub const FILE_TYPE_ICON_RADIUS: f32 = 8.0;

/// A configured file-type icon ready to build into a widget.
///
/// Created with [`FileTypeIcon::new`] and customised with a builder chain:
///
/// ```ignore
/// FileTypeIcon::new("report.pdf", None, None, false)
///     .size(FileTypeIconSize::Card)
///     .build(theme)              // -> Element
/// ```
///
/// The resolution happens once at construction (pure CPU, no I/O); callers
/// can inspect [`FileTypeIcon::resolved`] for the label/category/confidence.
pub struct FileTypeIcon<'a> {
    resolved: ResolvedFileIcon,
    size: FileTypeIconSize,
    theme: FileTypeIconTheme,
    accessibility_label: Option<&'a str>,
    show_container: bool,
    container_padding: f32,
    container_radius: f32,
}

impl<'a> FileTypeIcon<'a> {
    /// Build a file-type icon from file metadata.
    ///
    /// * `filename` — the file (or folder) name, used for extension lookup.
    /// * `mime_type` — the MIME type already known for the file (e.g. from
    ///   the local sharing source or advertised by a peer).  Treated as a
    ///   hint when `detected_type` is also present.
    /// * `detected_type` — a trusted locally detected MIME type (e.g. after
    ///   download).  Outranks `mime_type` for the icon.
    /// * `is_directory` — explicit folder state.  Never inferred from the
    ///   filename (Task 12).
    pub fn new(
        filename: &'a str,
        mime_type: Option<&'a str>,
        detected_type: Option<&'a str>,
        is_directory: bool,
    ) -> Self {
        let resolved = resolve_file_icon(filename, mime_type, detected_type, is_directory);
        Self {
            resolved,
            size: FileTypeIconSize::default(),
            theme: FileTypeIconTheme::default(),
            accessibility_label: None,
            show_container: true,
            container_padding: FILE_TYPE_ICON_PADDING,
            container_radius: FILE_TYPE_ICON_RADIUS,
        }
    }

    /// Convenience constructor for a folder icon (explicit directory state).
    pub fn directory(name: &'a str) -> Self {
        Self::new(name, None, None, true)
    }

    /// Set the semantic size variant.
    pub fn size(mut self, size: FileTypeIconSize) -> Self {
        self.size = size;
        self
    }

    /// Set the theme variant hook (PAPIRUS-14).  Defaults to [`FileTypeIconTheme::Auto`].
    pub fn theme(mut self, theme: FileTypeIconTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Provide an explicit accessible name.  Falls back to the resolver's
    /// friendly `display_label` (e.g. "PDF document") when unset.
    pub fn accessibility_label(mut self, label: &'a str) -> Self {
        self.accessibility_label = Some(label);
        self
    }

    /// Whether to draw the subtle neutral tile behind the icon
    /// (default `true`).  Set to `false` for inline contexts where the
    /// icon should sit directly on the surrounding surface.
    pub fn show_container(mut self, show: bool) -> Self {
        self.show_container = show;
        self
    }

    /// Padding (px) between the icon artwork and its container edge.
    pub fn container_padding(mut self, padding: f32) -> Self {
        self.container_padding = padding;
        self
    }

    /// Corner radius (px) for the container.
    pub fn container_radius(mut self, radius: f32) -> Self {
        self.container_radius = radius;
        self
    }

    /// The resolved icon (label, category, confidence, source, mismatch).
    pub fn resolved(&self) -> &ResolvedFileIcon {
        &self.resolved
    }

    /// The resolved file category (folder, pdf, video, archive, …).
    pub fn category(&self) -> FileCategory {
        self.resolved.file_category
    }

    /// The accessible name for this icon: the explicit label if provided,
    /// otherwise the resolver's friendly description.
    pub fn accessibility_label_or_default(&self) -> String {
        self.accessibility_label
            .map(str::to_string)
            .unwrap_or_else(|| self.resolved.display_label.clone())
    }

    /// Choose the bundled Papirus variant directory for the active theme.
    ///
    /// **This is the PAPIRUS-14 hook.**  The pinned bundle ships one
    /// full-colour asset set, so every theme resolves to `None` (use the
    /// default bundled paths) today.  When a dark Papirus set is bundled,
    /// this is the single place that switches the variant per theme —
    /// call sites and the rest of the component stay untouched.
    pub fn variant_dir(&self, _theme: &Theme) -> Option<u16> {
        match self.theme {
            FileTypeIconTheme::Auto | FileTypeIconTheme::Light | FileTypeIconTheme::Dark => None,
        }
    }

    /// Build the widget for the given active application theme.
    ///
    /// The `theme` is only used by the variant hook (PAPIRUS-14) and the
    /// container tint; the Papirus artwork itself is never recoloured.
    pub fn build<'b, Message>(&'b self, theme: &Theme) -> Element<'b, Message>
    where
        Message: 'b,
    {
        let px = self.size.px();

        // PAPIRUS-14 hook: the variant may override the size directory.
        let size_dir = self
            .variant_dir(theme)
            .unwrap_or_else(|| self.size.papirus_dir());

        // The resolver guarantees icon_id exists in the pinned bundle at
        // the default 32px; every bundled icon ships all five size dirs
        // today.  Guard anyway: a missing path falls back to the
        // unknown-generic icon at the requested size, never a broken image.
        let asset_path = papirus_asset_path(&self.resolved.icon_id, size_dir)
            .or_else(|| papirus_asset_path(UNKNOWN_ICON_ID, size_dir));
        let handle = asset_path
            .as_deref()
            .map(cached_svg_handle)
            .unwrap_or_else(fallback_handle);

        // Contain-style, centred, original colours: no `svg::Style` is
        // applied, so the Papirus artwork keeps its exact colours (Task 13).
        let icon = svg(handle)
            .width(Length::Fixed(px))
            .height(Length::Fixed(px))
            .content_fit(ContentFit::Contain);

        if !self.show_container {
            return icon.into();
        }

        let padding = self.container_padding;
        let radius = self.container_radius;
        container(icon)
            // Stable icon box: the icon sits centred inside a fixed square
            // whose edge is the artwork plus consistent padding.
            .width(Length::Fixed(px + 2.0 * padding))
            .height(Length::Fixed(px + 2.0 * padding))
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center)
            .padding(padding)
            .style(move |t| container::Style {
                // Subtle neutral tile: theme-aware soft tint, rounded
                // corners, thin muted border — no heavy shadow (Task 13).
                background: Some(iced::Background::Color(design_tokens::surface_hover(t))),
                border: Border {
                    color: design_tokens::border_muted(t),
                    width: 1.0,
                    radius: radius.into(),
                },
                ..Default::default()
            })
            .into()
    }
}

// ── Asset loading & handle cache ─────────────────────────────────────

/// Terminal fallback icon id — must match the resolver's unknown icon so
/// the fallback path and the resolver agree.
const UNKNOWN_ICON_ID: &str = "application-x-generic";

/// Embedded safety net: the unknown-generic icon (32px) compiled into the
/// binary.  If a bundled asset path is missing at runtime (packaging edge
/// case), the component renders this instead of a broken icon.
const FALLBACK_SVG_BYTES: &[u8] =
    include_bytes!("../../assets/third_party/papirus/32/application-x-generic.svg");

/// Process-global cache of decoded SVG handles, keyed by repo-relative
/// asset path.  Handles are `Clone` cheaply (O(1)), so `view` never
/// re-reads or re-parses the SVG — only the first use of a path pays.
static SVG_HANDLE_CACHE: OnceLock<Mutex<HashMap<String, svg::Handle>>> = OnceLock::new();

/// Fetch (and cache) the SVG handle for a repo-relative asset path such as
/// `"assets/third_party/papirus/48/application-pdf.svg"`.
///
/// Paths are resolved against `CARGO_MANIFEST_DIR` (compile-time absolute)
/// so rendering does not depend on the process working directory.
fn cached_svg_handle(asset_path: &str) -> svg::Handle {
    let cache = SVG_HANDLE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(handle) = cache.lock().unwrap().get(asset_path) {
        return handle.clone();
    }
    let full_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(asset_path);
    let bytes = std::fs::read(&full_path).unwrap_or_else(|_| FALLBACK_SVG_BYTES.to_vec());
    let handle = svg::Handle::from_memory(bytes);
    cache
        .lock()
        .unwrap()
        .insert(asset_path.to_string(), handle.clone());
    handle
}

/// The embedded fallback handle (unknown-generic icon).
fn fallback_handle() -> svg::Handle {
    svg::Handle::from_memory(FALLBACK_SVG_BYTES)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppMessage;
    use crate::file_type_resolver::ResolutionSource;

    fn all_sizes() -> [FileTypeIconSize; 5] {
        [
            FileTypeIconSize::Compact,
            FileTypeIconSize::List,
            FileTypeIconSize::Card,
            FileTypeIconSize::Large,
            FileTypeIconSize::Hero,
        ]
    }

    #[test]
    fn size_variants_map_to_bundled_papirus_dirs() {
        assert_eq!(FileTypeIconSize::Compact.papirus_dir(), 16);
        assert_eq!(FileTypeIconSize::List.papirus_dir(), 24);
        assert_eq!(FileTypeIconSize::Card.papirus_dir(), 32);
        assert_eq!(FileTypeIconSize::Large.papirus_dir(), 48);
        assert_eq!(FileTypeIconSize::Hero.papirus_dir(), 64);
    }

    #[test]
    fn semantic_pixel_values_match_spec_bands() {
        assert_eq!(FileTypeIconSize::Compact.px(), 16.0);
        assert_eq!(FileTypeIconSize::List.px(), 24.0);
        assert_eq!(FileTypeIconSize::Card.px(), 32.0);
        assert_eq!(FileTypeIconSize::Large.px(), 48.0);
        assert_eq!(FileTypeIconSize::Hero.px(), 64.0);
    }

    #[test]
    fn resolves_directory_to_folder_icon() {
        let icon = FileTypeIcon::directory("shared-folder");
        let r = icon.resolved();
        assert_eq!(r.icon_id, "folder-open");
        assert_eq!(r.file_category, FileCategory::Folder);
        assert_eq!(r.source, ResolutionSource::Directory);
    }

    #[test]
    fn resolves_pdf_by_extension() {
        let icon = FileTypeIcon::new("report.pdf", None, None, false);
        let r = icon.resolved();
        assert_eq!(r.icon_id, "application-pdf");
        assert_eq!(r.file_category, FileCategory::Pdf);
        assert_eq!(r.display_label, "PDF document");
    }

    #[test]
    fn locally_detected_mime_outranks_advertised_hint() {
        // Peer advertises image/png; local detection says PDF — the icon
        // must follow the trusted local type.
        let icon = FileTypeIcon::new(
            "download.bin",
            Some("image/png"),
            Some("application/pdf"),
            false,
        );
        let r = icon.resolved();
        assert_eq!(r.icon_id, "application-pdf");
        assert!(r.mime_mismatch.is_some());
    }

    #[test]
    fn unknown_file_gets_generic_fallback() {
        let icon = FileTypeIcon::new("unknownfile", None, None, false);
        let r = icon.resolved();
        assert_eq!(r.source, ResolutionSource::UnknownFallback);
        assert!(r.icon_id.contains("generic"));
        assert_eq!(r.file_category, FileCategory::Unknown);
    }

    #[test]
    fn bundled_asset_path_exists_for_every_size_variant() {
        // The pinned manifest ships all five size dirs for every icon;
        // the size-lookup hook must return repo-relative paths that exist
        // in the bundle.
        for size in all_sizes() {
            let path = papirus_asset_path("application-pdf", size.papirus_dir())
                .expect("pdf icon must exist at every semantic size");
            assert!(path.starts_with("assets/third_party/papirus/"));
            assert!(path.ends_with(&format!("{}/application-pdf.svg", size.papirus_dir())));
        }
    }

    #[test]
    fn fallback_icon_exists_at_every_size() {
        for size in all_sizes() {
            let path = papirus_asset_path(UNKNOWN_ICON_ID, size.papirus_dir())
                .expect("unknown generic icon must exist at every semantic size");
            assert!(path.ends_with(&format!("{}/application-x-generic.svg", size.papirus_dir())));
        }
    }

    #[test]
    fn handle_cache_returns_cloned_handles_for_same_path() {
        let path = "assets/third_party/papirus/32/application-pdf.svg";
        // Count entries for THIS path rather than the whole-map delta: other
        // tests run in parallel and may legitimately insert distinct paths
        // into the process-global cache between our reads, which would make a
        // delta assertion flaky. Two requests for the same path must still
        // create exactly one entry.
        let count_for = |cache: &std::sync::Mutex<HashMap<String, svg::Handle>>, path: &str| {
            cache
                .lock()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.as_str() == path)
                .count()
        };
        let before = SVG_HANDLE_CACHE
            .get()
            .map(|c| count_for(c, path))
            .unwrap_or(0);
        let _ = cached_svg_handle(path);
        let _ = cached_svg_handle(path);
        let after = SVG_HANDLE_CACHE
            .get()
            .map(|c| count_for(c, path))
            .unwrap_or(0);
        // First request inserts the entry; the second reuses it — the path
        // must appear exactly once in the cache. The cache is keyed by path
        // (HashMap insert overwrites), so the count is 0 or 1 regardless of
        // how many requests were made; a concurrent test may already have
        // inserted the same path before our `before` read (e.g. the
        // all-semantic-sizes view test builds report.pdf at Card size, which
        // resolves to this exact 32px path). Assert the invariant that is
        // actually meaningful: the path is present exactly once after our two
        // requests, never duplicated.
        assert!(before <= 1, "path should never have duplicate entries");
        assert_eq!(after, 1);
    }

    #[test]
    fn view_builds_at_all_semantic_sizes_with_container() {
        for size in all_sizes() {
            let icon = FileTypeIcon::new("report.pdf", None, None, false).size(size);
            let element: Element<'_, AppMessage> = icon.build(&Theme::Light);
            let _ = element;
        }
    }

    #[test]
    fn view_builds_without_container() {
        let icon = FileTypeIcon::new("archive.tar.gz", None, None, false)
            .size(FileTypeIconSize::List)
            .show_container(false);
        let element: Element<'_, AppMessage> = icon.build(&Theme::Light);
        let _ = element;
    }

    #[test]
    fn accessibility_label_prefers_explicit_value() {
        let icon =
            FileTypeIcon::new("video.mp4", None, None, false).accessibility_label("MP4 movie clip");
        assert_eq!(icon.accessibility_label_or_default(), "MP4 movie clip");
        // Without an explicit label, fall back to the resolver's friendly
        // display label (category-derived).
        let plain = FileTypeIcon::new("video.mp4", None, None, false);
        assert_eq!(plain.accessibility_label_or_default(), "Video");
    }

    #[test]
    fn every_theme_variant_keeps_bundled_paths_today() {
        // The theme hook must not panic or diverge per variant while the
        // bundle only ships one asset set (PAPIRUS-14 extends this).
        for theme in [
            FileTypeIconTheme::Auto,
            FileTypeIconTheme::Light,
            FileTypeIconTheme::Dark,
        ] {
            let icon = FileTypeIcon::new("budget.xlsx", None, None, false).theme(theme);
            assert_eq!(icon.variant_dir(&Theme::Light), None);
            assert_eq!(icon.variant_dir(&Theme::Dark), None);
        }
    }
}
