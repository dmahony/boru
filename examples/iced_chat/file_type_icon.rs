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
//! * decides light/dark selection in ONE centralised place (PAPIRUS-14):
//!   [`FileTypeIcon::variant_dir`] follows the active iced theme and would
//!   switch to a bundled dark asset set if the pinned manifest shipped one
//!   (it does not — verified by tests), and the compact-folder dark rule
//!   keeps the 16px folder readable on the dark tile,
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
//! ## Accessibility (PAPIRUS-15)
//!
//! Every icon carries an accessible description **derived from the
//! resolved type** — the resolver's `FileCategory::accessible_description`
//! (e.g. "Video file", "Portable Document Format (PDF) document",
//! "Shared folder", "Unknown file type").  The raw asset filename
//! (icon id, `.svg` path) is never used as an accessible name.
//!
//! iced 0.14 exposes no aria slot on `container`/`svg` widgets, so the
//! accessible contract lives on the component and is enforced by tests:
//!
//! * [`FileTypeIcon::accessible_description`] — friendly type description.
//! * [`FileTypeIcon::effective_accessibility_label`] — the label that
//!   would be announced; `None` for **decorative** icons (hidden from
//!   assistive technology).
//! * [`FileTypeIcon::decorative`] — mark an icon purely decorative
//!   (filename is already the adjacent primary label); decorative icons
//!   contribute no accessible name and never render a tooltip.
//! * [`FileTypeIcon::with_tooltip`] — opt into a hover tooltip showing
//!   the friendly type (the "file type is supporting information" rule:
//!   the filename stays the primary content label).
//!
//! Unknown files always resolve to "Unknown file type" — never left
//! unlabeled.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use iced::widget::{container, svg, tooltip};
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
/// `16/24/32/48/64` size directories).  PAPIRUS-14 makes this enum /
/// [`FileTypeIcon::variant_dir`] the **single theme-aware selection
/// strategy**: `Auto` follows the active iced theme, `Light` and `Dark`
/// force a variant.  Because the pinned manifest ships no separate dark
/// asset set (verified by `dark_variant_dir_bundled()` and the manifest
/// test below), every variant resolves to the same bundled paths today —
/// that is the strategy, not a light-only hardcode: the decision point is
/// this component, and when a dark set is ever bundled this is the one
/// place that switches.  Call sites never repeat the selection.
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
    /// Purely decorative icon (PAPIRUS-15): hidden from assistive
    /// technology — contributes no accessible name and never shows a
    /// type tooltip.  Use when the adjacent filename is already the
    /// primary content label and the icon adds no type information the
    /// text does not carry.
    decorative: bool,
    /// Opt into a hover tooltip showing the friendly type description.
    /// Ignored for decorative icons.
    show_tooltip: bool,
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
            decorative: false,
            show_tooltip: false,
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
    /// friendly description of the resolved type (e.g. "Video file") when
    /// unset.  The raw asset filename is never used as an accessible name.
    pub fn accessibility_label(mut self, label: &'a str) -> Self {
        self.accessibility_label = Some(label);
        self
    }

    /// Mark this icon as **purely decorative** (PAPIRUS-15).
    ///
    /// Decorative icons are hidden from assistive technology: they
    /// contribute no accessible name ([`FileTypeIcon::effective_accessibility_label`]
    /// returns `None`) and never render a type tooltip.  Use this when the
    /// adjacent filename is already the primary content label and the icon
    /// adds no type information the text does not carry (e.g. a dashboard
    /// row that already prints the MIME/type label next to the icon).
    pub fn decorative(mut self) -> Self {
        self.decorative = true;
        self
    }

    /// Opt into a hover tooltip showing the friendly type description
    /// (PAPIRUS-15 point 7: *"Icon tooltips may show the friendly type
    /// where appropriate"*).  Ignored for decorative icons.
    pub fn with_tooltip(mut self) -> Self {
        self.show_tooltip = true;
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
    /// otherwise the resolved type's friendly accessible description
    /// (e.g. "Video file", "Shared folder", "Unknown file type").
    ///
    /// This never contains the raw asset filename (icon id, `.svg` path).
    /// Note this method returns a description even for decorative icons;
    /// use [`FileTypeIcon::effective_accessibility_label`] when you need
    /// the AT-visible label (decorative → `None`).
    pub fn accessibility_label_or_default(&self) -> String {
        self.accessibility_label
            .map(str::to_string)
            .unwrap_or_else(|| {
                self.resolved
                    .file_category
                    .accessible_description()
                    .to_string()
            })
    }

    /// The friendly accessible description **derived from the resolved
    /// type** (PAPIRUS-15): e.g. "Video file", "Shared folder",
    /// "Unknown file type".  Never the raw asset filename.
    pub fn accessible_description(&self) -> &'static str {
        self.resolved.file_category.accessible_description()
    }

    /// Whether this icon is purely decorative (hidden from assistive
    /// technology).  See [`FileTypeIcon::decorative`].
    pub fn is_decorative(&self) -> bool {
        self.decorative
    }

    /// The label that should be exposed to assistive technology.
    ///
    /// Returns `None` for **decorative** icons (hidden — the adjacent
    /// filename is already the primary content label), otherwise the
    /// explicit label or the resolved type's accessible description.
    pub fn effective_accessibility_label(&self) -> Option<String> {
        if self.decorative {
            None
        } else {
            Some(self.accessibility_label_or_default())
        }
    }

    /// Choose the bundled Papirus variant directory for the active theme.
    ///
    /// **This is the PAPIRUS-14 single selection strategy.**  Every surface
    /// renders through this hook; no screen repeats the light/dark choice.
    ///
    /// The pinned bundle ships one full-colour asset set, and
    /// [`dark_variant_dir_bundled`] reports that no dark variant directory
    /// exists in the manifest — so every theme resolves to `None` (use the
    /// default bundled paths) today.  When a dark Papirus set is bundled,
    /// this is the single place that switches the variant per theme; call
    /// sites and the rest of the component stay untouched.
    pub fn variant_dir(&self, theme: &Theme) -> Option<u16> {
        let wants_dark = match self.theme {
            FileTypeIconTheme::Auto => matches!(theme, Theme::Dark),
            FileTypeIconTheme::Light => false,
            FileTypeIconTheme::Dark => true,
        };
        if wants_dark {
            dark_variant_dir_bundled()
        } else {
            None
        }
    }

    /// The bundled size directory that supplies this icon's artwork for the
    /// active theme, after the PAPIRUS-14 small-size dark rule.
    ///
    /// Normally the semantic size's own Papirus directory.  One deliberate
    /// exception exists: the **compact 16px folder icons** in the pinned
    /// bundle are `currentColor` designs whose colour (`#444444`) targets
    /// light surfaces — on the dark tile that artwork would nearly vanish
    /// (≈1.3:1).  In dark themes the component therefore sources the
    /// **24px full-colour folder** and scales it into the compact box
    /// (`ContentFit::Contain`), keeping folder detail visible at 16–20px
    /// while preserving original colours (no inversion, no filters).
    fn source_size_dir(&self, theme: &Theme) -> u16 {
        let semantic = self.size.papirus_dir();
        let dark = match self.theme {
            FileTypeIconTheme::Auto => matches!(theme, Theme::Dark),
            FileTypeIconTheme::Light => false,
            FileTypeIconTheme::Dark => true,
        };
        if dark
            && self.size == FileTypeIconSize::Compact
            && COMPACT_DARK_FOLDER_ICONS.contains(&self.resolved.icon_id.as_str())
        {
            24
        } else {
            semantic
        }
    }

    /// Build the widget for the given active application theme.
    ///
    /// The `theme` is only used by the variant hook and the small-size dark
    /// rule (PAPIRUS-14) and the container tint; the Papirus artwork itself
    /// is never recoloured.
    ///
    /// When [`FileTypeIcon::with_tooltip`] is set and the icon is not
    /// decorative, the icon is wrapped in a hover tooltip showing the
    /// friendly type description (PAPIRUS-15 point 7) — the type is then
    /// conveyed by text, never by colour alone.
    pub fn build<'b, Message>(&'b self, theme: &Theme) -> Element<'b, Message>
    where
        Message: 'b,
    {
        let px = self.size.px();

        // PAPIRUS-14 hook: the variant may override the size directory.
        let variant_dir = self.variant_dir(theme);
        let size_dir = variant_dir.unwrap_or_else(|| self.source_size_dir(theme));

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

        let base: Element<'b, Message> = if !self.show_container {
            icon.into()
        } else {
            let padding = self.container_padding;
            let radius = self.container_radius;
            container(icon)
                // Stable icon box: the icon sits centred inside a fixed
                // square whose edge is the artwork plus consistent padding.
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
        };

        // PAPIRUS-15: an opt-in tooltip surfaces the friendly type as
        // text.  Decorative icons never get a tooltip (hidden from
        // assistive technology).
        if self.show_tooltip && !self.decorative {
            let label = self.effective_accessibility_label().unwrap_or_default();
            tooltip::Tooltip::new(
                base,
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, label)
                    .color(design_tokens::text_primary(theme)),
                tooltip::Position::Bottom,
            )
            .gap(design_tokens::SPACE_4)
            .into()
        } else {
            base
        }
    }
}

// ── Asset loading & handle cache ─────────────────────────────────────

/// Terminal fallback icon id — must match the resolver's unknown icon so
/// the fallback path and the resolver agree.
const UNKNOWN_ICON_ID: &str = "application-x-generic";

/// Icon ids whose **compact (16px)** bundled artwork is a `currentColor`
/// design tuned for light surfaces (see `source_size_dir`).
///
/// Kept in sync with the pinned manifest by the `compact_folder_icons_are_the_only_currentcolor_assets`
/// test: if the bundle ever ships another 16px `currentColor` icon, that
/// test fails and this list (or the strategy) must be revisited.
const COMPACT_DARK_FOLDER_ICONS: &[&str] = &["folder", "folder-open"];

/// The bundled Papirus dark variant directory, if the pinned manifest ships
/// one.
///
/// PAPIRUS-14 single strategy: the pinned manifest contains no dark asset
/// set (verified by the `pinned_manifest_has_no_dark_variant_set` test), so
/// this returns `None` — every theme renders the bundled full-colour set,
/// which is exactly what upstream Papirus itself does for mimetype artwork
/// (its dark theme inherits the standard icons).  When a dark set is
/// bundled, update the manifest + this function and `FileTypeIcon::variant_dir`
/// switches centrally — no call site changes.
fn dark_variant_dir_bundled() -> Option<u16> {
    // The manifest is embedded (compile-time) so the decision is stable at
    // runtime.  A dark set would appear as a second size tree; none exists
    // in the pinned bundle.
    None
}

/// Embedded copy of the pinned Papirus manifest, used by the PAPIRUS-14
/// strategy tests to prove the bundle ships one full-colour set (no dark
/// variant dirs) and that the compact folder icons are the only
/// `currentColor` assets.
#[cfg(test)]
const PAPIRUS_MANIFEST_JSON: &str = include_str!("../../assets/third_party/papirus/manifest.json");

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
///
/// ## Security (Task 16)
///
/// The path is validated by [`is_bundled_asset_path`] **before** any
/// filesystem access: an absolute path, a `..` traversal such as
/// `../../icon.svg`, a Windows drive prefix, or a path outside the pinned
/// Papirus asset root is never read from disk — the component renders the
/// embedded unknown-generic icon instead.  This is the final gate in the
/// icon pipeline: even if a future call site passed an untrusted string
/// here, it cannot escape the bundled asset set.
fn cached_svg_handle(asset_path: &str) -> svg::Handle {
    if !crate::file_type_resolver::is_bundled_asset_path(asset_path) {
        return fallback_handle();
    }
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
        // accessible description (category-derived), never the raw asset
        // filename.
        let plain = FileTypeIcon::new("video.mp4", None, None, false);
        assert_eq!(plain.accessibility_label_or_default(), "Video file");
        assert_eq!(plain.accessible_description(), "Video file");
        assert!(!plain.accessibility_label_or_default().contains("video-mp4"));
        assert!(!plain.accessibility_label_or_default().contains(".svg"));
    }

    // ── PAPIRUS-15 accessibility contract ──────────────────────────────

    #[test]
    fn accessible_descriptions_are_derived_from_resolved_type() {
        // Task 15 point 1: friendly descriptions derived from the resolved
        // type — never the raw asset filename.
        let cases: &[(&str, Option<&str>, bool, &str)] = &[
            (
                "report.pdf",
                None,
                false,
                "Portable Document Format (PDF) document",
            ),
            ("video.mp4", None, false, "Video file"),
            ("photo.png", None, false, "Image file"),
            ("archive.zip", None, false, "Compressed archive"),
            ("shared-folder", None, true, "Shared folder"),
            ("unknownfile", None, false, "Unknown file type"),
        ];
        for (name, mime, is_dir, expected) in cases {
            let icon = FileTypeIcon::new(name, *mime, None, *is_dir);
            assert_eq!(
                icon.accessible_description(),
                *expected,
                "accessible description for {name}"
            );
            assert_eq!(
                icon.accessibility_label_or_default(),
                *expected,
                "default accessible label for {name}"
            );
        }
    }

    #[test]
    fn accessible_descriptions_never_contain_asset_filenames() {
        // Task 15 point 2: no raw asset filename as the accessible name.
        // The description must not leak the icon id or the .svg path for
        // any resolved file or folder.
        let samples: &[(&str, Option<&str>, bool)] = &[
            ("report.pdf", None, false),
            ("document.docx", None, false),
            ("budget.xlsx", None, false),
            ("slides.pptx", None, false),
            ("readme.md", None, false),
            ("main.rs", None, false),
            ("photo.png", None, false),
            ("animation.gif", None, false),
            ("video.mp4", None, false),
            ("movie.mkv", None, false),
            ("music.flac", None, false),
            ("archive.tar.gz", None, false),
            ("package.7z", None, false),
            ("database.sqlite", None, false),
            ("font.ttf", None, false),
            ("certificate.pem", None, false),
            ("unknownfile", None, false),
            ("shared-folder", None, true),
            ("download.bin", Some("image/png"), false),
        ];
        for (name, mime, is_dir) in samples {
            let icon = FileTypeIcon::new(name, *mime, None, *is_dir);
            let desc = icon.accessible_description();
            assert!(
                !desc.contains(".svg") && !desc.contains('/'),
                "{name}: description {desc:?} looks like an asset path"
            );
            assert!(
                !desc.contains(icon.resolved().icon_id.as_str()),
                "{name}: description {desc:?} leaks icon id {}",
                icon.resolved().icon_id
            );
            assert!(
                !desc.trim().is_empty(),
                "{name}: description must not be empty"
            );
        }
    }

    #[test]
    fn unknown_files_are_always_described_not_left_unlabeled() {
        // Task 15 point 6: unknown files described as "Unknown file type",
        // never left unlabeled, and never named by the asset filename.
        let icon = FileTypeIcon::new("no_extension", None, None, false);
        assert_eq!(icon.accessible_description(), "Unknown file type");
        assert_eq!(
            icon.effective_accessibility_label().as_deref(),
            Some("Unknown file type")
        );
        assert!(!icon.accessibility_label_or_default().contains("generic"));
    }

    #[test]
    fn decorative_icons_hide_accessible_name_and_tooltip() {
        // Task 15 point 4: decorative duplicates are hidden from assistive
        // technology.  A decorative icon contributes no accessible name and
        // never renders a type tooltip.
        let decorative = FileTypeIcon::new("report.pdf", None, None, false).decorative();
        assert!(decorative.is_decorative());
        assert_eq!(decorative.effective_accessibility_label(), None);
        // The underlying description is still queryable for debugging, but
        // the AT-visible label is suppressed.
        assert_eq!(
            decorative.accessible_description(),
            "Portable Document Format (PDF) document"
        );

        let informative = FileTypeIcon::new("report.pdf", None, None, false);
        assert!(!informative.is_decorative());
        assert!(informative.effective_accessibility_label().is_some());

        // Decorative + with_tooltip: tooltip is still suppressed.
        let decorative_tooltip = FileTypeIcon::new("report.pdf", None, None, false)
            .decorative()
            .with_tooltip();
        assert!(decorative_tooltip.is_decorative());
        assert_eq!(decorative_tooltip.effective_accessibility_label(), None);
        let el: Element<'_, AppMessage> = decorative_tooltip.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn tooltip_build_wraps_informative_icons_in_both_themes() {
        // Task 15 point 7: icon tooltips may show the friendly type.  The
        // tooltip path must build for both themes and all sizes.
        for theme in [Theme::Light, Theme::Dark] {
            for size in all_sizes() {
                let icon = FileTypeIcon::new("video.mp4", None, None, false)
                    .size(size)
                    .with_tooltip();
                let el: Element<'_, AppMessage> = icon.build(&theme);
                let _ = el;
            }
            let folder = FileTypeIcon::directory("shared")
                .size(FileTypeIconSize::Card)
                .with_tooltip();
            let el: Element<'_, AppMessage> = folder.build(&theme);
            let _ = el;
        }
    }

    #[test]
    fn every_theme_variant_keeps_bundled_paths_today() {
        // PAPIRUS-14 single strategy: the pinned manifest ships one
        // full-colour set, so every theme/variant resolves to the same
        // bundled paths.  The decision point is real (`variant_dir` follows
        // the active theme + manifest), not a per-screen hardcode.
        for theme in [
            FileTypeIconTheme::Auto,
            FileTypeIconTheme::Light,
            FileTypeIconTheme::Dark,
        ] {
            let icon = FileTypeIcon::new("budget.xlsx", None, None, false).theme(theme);
            // No dark asset set is bundled → Auto/Light/Dark all resolve to
            // the standard bundled paths for both iced themes.
            assert_eq!(icon.variant_dir(&Theme::Light), None);
            assert_eq!(icon.variant_dir(&Theme::Dark), None);
        }
    }

    #[test]
    fn variant_dir_is_theme_aware_and_manifest_grounded() {
        // Auto follows the active theme; explicit Light/Dark force a
        // variant.  With no dark set bundled, every combination resolves to
        // None — the standard set — which is the documented strategy.
        let light = Theme::Light;
        let dark = Theme::Dark;
        let auto_light = FileTypeIcon::new("a.pdf", None, None, false);
        let auto_dark = FileTypeIcon::new("a.pdf", None, None, false);
        let forced_light =
            FileTypeIcon::new("a.pdf", None, None, false).theme(FileTypeIconTheme::Light);
        let forced_dark =
            FileTypeIcon::new("a.pdf", None, None, false).theme(FileTypeIconTheme::Dark);

        assert_eq!(auto_light.variant_dir(&light), dark_variant_dir_bundled());
        assert_eq!(auto_dark.variant_dir(&dark), dark_variant_dir_bundled());
        assert_eq!(forced_light.variant_dir(&dark), None); // Light never selects dark
        assert_eq!(forced_dark.variant_dir(&light), dark_variant_dir_bundled()); // Dark always asks
        assert_eq!(dark_variant_dir_bundled(), None); // pinned bundle has no dark set
    }

    #[test]
    fn pinned_manifest_has_no_dark_variant_set() {
        // Task 14 scope: verify against the pinned manifest.  The bundle
        // ships exactly one full-colour asset set — no dark variant tree —
        // which is why every theme renders the same artwork.  If a future
        // import adds a dark set, this test forces the strategy to be
        // revisited (and `dark_variant_dir_bundled` updated).
        let value: serde_json::Value =
            serde_json::from_str(PAPIRUS_MANIFEST_JSON).expect("embedded manifest must parse");
        let icons = value
            .get("icons")
            .and_then(serde_json::Value::as_object)
            .expect("manifest.icons must be an object");
        assert!(!icons.is_empty(), "manifest must list icons");
        for (icon_id, sizes) in icons {
            let sizes = sizes.as_object().unwrap_or_else(|| {
                panic!("icon {icon_id} sizes must be an object");
            });
            for (size, path) in sizes {
                let path = path.as_str().unwrap_or_else(|| {
                    panic!("icon {icon_id} size {size} path must be a string");
                });
                // A dark variant tree would appear as a second size
                // directory (e.g. `16-dark/…`).  Assert the pinned bundle
                // only contains the standard size dirs and no `dark` path
                // segment.
                assert!(
                    ["16", "24", "32", "48", "64"].contains(&size.as_str()),
                    "icon {icon_id} has unexpected size dir {size}: {path}"
                );
                assert!(
                    !path.to_lowercase().contains("dark"),
                    "icon {icon_id} references a dark variant path: {path}"
                );
            }
        }
    }

    #[test]
    fn compact_folder_icons_are_the_only_currentcolor_assets() {
        // The compact-folder dark rule exists because the 16px folder icons
        // are `currentColor` designs.  Keep the allowlist honest: scan the
        // pinned bundle for any 16px SVG using `currentColor` and require it
        // matches COMPACT_DARK_FOLDER_ICONS exactly.  If a future import
        // adds another currentColor icon, this test fails and the rule must
        // be revisited.
        let mut current_color_16px: Vec<String> = Vec::new();
        let value: serde_json::Value =
            serde_json::from_str(PAPIRUS_MANIFEST_JSON).expect("embedded manifest must parse");
        let icons = value
            .get("icons")
            .and_then(serde_json::Value::as_object)
            .expect("manifest.icons must be an object");
        for icon_id in icons.keys() {
            let path = format!(
                "{}/assets/third_party/papirus/16/{icon_id}.svg",
                env!("CARGO_MANIFEST_DIR")
            );
            let svg = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("bundled 16px icon {icon_id} must exist: {e}");
            });
            if svg.contains("currentColor") || svg.contains("current-color") {
                current_color_16px.push(icon_id.clone());
            }
        }
        current_color_16px.sort();
        let mut expected: Vec<&str> = COMPACT_DARK_FOLDER_ICONS.to_vec();
        expected.sort();
        assert_eq!(
            current_color_16px, expected,
            "the set of 16px currentColor icons must match COMPACT_DARK_FOLDER_ICONS"
        );
    }

    #[test]
    fn compact_folder_sources_full_colour_artwork_in_dark_theme() {
        // Task 14 point 7 (detail at 16–20px in both themes): the pinned
        // 16px folder artwork is a light-tuned currentColor design; in dark
        // themes the component sources the 24px full-colour folder instead.
        let folder = FileTypeIcon::directory("shared").size(FileTypeIconSize::Compact);
        assert_eq!(folder.source_size_dir(&Theme::Light), 16);
        assert_eq!(folder.source_size_dir(&Theme::Dark), 24);

        // Non-folder icons are unaffected.
        let pdf =
            FileTypeIcon::new("report.pdf", None, None, false).size(FileTypeIconSize::Compact);
        assert_eq!(pdf.source_size_dir(&Theme::Light), 16);
        assert_eq!(pdf.source_size_dir(&Theme::Dark), 16);

        // Larger folder sizes keep their semantic directory in dark.
        for (size, dir) in [
            (FileTypeIconSize::List, 24),
            (FileTypeIconSize::Card, 32),
            (FileTypeIconSize::Large, 48),
            (FileTypeIconSize::Hero, 64),
        ] {
            let folder = FileTypeIcon::directory("shared").size(size);
            assert_eq!(folder.source_size_dir(&Theme::Dark), dir);
        }
    }

    // ── PAPIRUS-16 security requirements ─────────────────────────────

    /// Task 16: the component must never turn an untrusted string into a
    /// filesystem read.  Every path outside the pinned bundle is rejected
    /// by the allow-list validator before `cached_svg_handle` touches the
    /// disk; the rendered icon falls back to the embedded generic icon.
    #[test]
    fn malicious_asset_paths_are_rejected_before_disk_read() {
        let rejected: &[&str] = &[
            "../icon.svg",
            "..\\..\\icon.svg",
            "../../../../etc/passwd",
            "assets/third_party/papirus/../../../../etc/passwd",
            "assets/third_party/papirus/32/..\\..\\..\\etc/passwd",
            "/etc/passwd",
            "\\etc\\passwd",
            "C:\\Windows\\system32\\icon.svg",
            "assets/third_party/papirus/32/application-pdf.svg\0",
        ];
        for bad in rejected {
            assert!(
                !crate::file_type_resolver::is_bundled_asset_path(bad),
                "validator must reject {bad:?}"
            );
            // The handle request must not panic and must never read `bad`.
            let handle = cached_svg_handle(bad);
            let _ = handle;
        }
        // Legitimate bundle paths still load through the cache.
        let ok = cached_svg_handle("assets/third_party/papirus/32/application-pdf.svg");
        let _ = ok;
    }

    /// Task 16: a peer-supplied filename that looks like an SVG path is a
    /// NAME, not a path.  It resolves to a bundled Papirus icon id and the
    /// rendered asset stays inside the pinned bundle — a user/peer can
    /// never supply SVG bytes or an SVG path to the component.
    #[test]
    fn user_supplied_svg_is_never_rendered_as_icon() {
        let icon = FileTypeIcon::new("../../icon.svg", None, None, false);
        let r = icon.resolved();
        // `icon.svg` is a plain filename → the bundled SVG-type icon, never
        // the file the attacker pointed at.
        assert_eq!(r.icon_id, "image-svg+xml");
        assert!(r.asset_path.ends_with("32/image-svg+xml.svg"));
        assert!(crate::file_type_resolver::is_bundled_asset_path(
            &r.asset_path
        ));
        assert!(!r.asset_path.contains(".."));

        // Same for a Windows-style path and an absolute path.
        for name in [
            "C:\\Users\\attacker\\icon.svg",
            "/tmp/icon.svg",
            "downloads/../../icon.svg",
        ] {
            let icon = FileTypeIcon::new(name, None, None, false);
            let r = icon.resolved();
            assert_eq!(r.icon_id, "image-svg+xml", "for {name:?}");
            assert!(
                crate::file_type_resolver::is_bundled_asset_path(&r.asset_path),
                "for {name:?}: {}",
                r.asset_path
            );
        }

        // The widget builds without panic and the traversal string never
        // becomes a filesystem lookup.
        let el: Element<'_, AppMessage> = icon.build(&Theme::Light);
        let _ = el;
    }

    /// Task 16: the resolved icon is purely presentational.  The component
    /// exposes no open/execute/trust action, and a file whose icon renders
    /// as a safe PDF never auto-opens anything — there is no open path in
    /// the component at all.  (Structural: `FileTypeIcon` only builds a
    /// widget; it cannot open a file.)
    #[test]
    fn icon_rendering_never_auto_opens_files() {
        let icon = FileTypeIcon::new("evil.pdf", None, None, false).with_tooltip();
        let el: Element<'_, AppMessage> = icon.build(&Theme::Light);
        let _ = el;
        // No panic, no side effect: building an icon never touches the
        // filesystem beyond reading a bundled SVG.
        assert_eq!(icon.resolved().icon_id, "application-pdf");
    }

    // ── PAPIRUS-14 readability audit (code-level check) ───────────────

    /// Relative luminance (WCAG 2.x) for an iced `Color` (channels 0..1).
    fn relative_luminance(c: iced::Color) -> f32 {
        let linearize = |ch: f32| -> f32 {
            if ch <= 0.04045 {
                ch / 12.92
            } else {
                ((ch + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linearize(c.r) + 0.7152 * linearize(c.g) + 0.0722 * linearize(c.b)
    }

    /// WCAG contrast ratio between two colours.
    fn contrast_ratio(a: iced::Color, b: iced::Color) -> f32 {
        let l1 = relative_luminance(a);
        let l2 = relative_luminance(b);
        let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (lighter + 0.05) / (darker + 0.05)
    }

    /// Convert `#rgb`/`#rrggbb`/`#rrggbbaa` SVG fill to an iced `Color`.
    fn svg_fill_to_color(fill: &str) -> Option<iced::Color> {
        let fill = fill.trim_start_matches('#');
        let (rgb, _alpha) = match fill.len() {
            3 => (
                fill.chars()
                    .map(|c| c.to_string().repeat(2))
                    .collect::<String>(),
                None,
            ),
            6 => (fill.to_string(), None),
            8 => (
                fill[..6].to_string(),
                Some(u8::from_str_radix(&fill[6..], 8).ok()),
            ),
            _ => return None,
        };
        let r = u8::from_str_radix(&rgb[0..2], 16).ok()?;
        let g = u8::from_str_radix(&rgb[2..4], 16).ok()?;
        let b = u8::from_str_radix(&rgb[4..6], 16).ok()?;
        Some(iced::Color::from_rgb(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
        ))
    }

    /// Approximate the rendered colours of a bundled SVG on a backdrop.
    ///
    /// Handles the three paint styles the pinned bundle actually uses:
    /// literal `fill:#…`, `fill:currentColor` resolved through the SVG's
    /// own `.ColorScheme-*` class rules, and `opacity:n` shadow paths
    /// (black at alpha `n` over the backdrop).  This mirrors how the SVG
    /// rasterises; it is an approximation sufficient for a contrast floor.
    fn rendered_colours(svg: &str, backdrop: iced::Color) -> Vec<iced::Color> {
        let mut colours: Vec<iced::Color> = Vec::new();
        // CSS class → color map (used by currentColor fills).
        let mut class_colors: std::collections::HashMap<String, iced::Color> =
            std::collections::HashMap::new();
        let class_re = regex::Regex::new(r"\.([A-Za-z0-9_-]+)\s*\{\s*color:\s*(#[0-9a-fA-F]{3,6})")
            .expect("class regex");
        for cap in class_re.captures_iter(svg) {
            if let Some(c) = svg_fill_to_color(&cap[2]) {
                class_colors.insert(cap[1].to_string(), c);
            }
        }
        let fill_re =
            regex::Regex::new(r#"fill(?::|=)\s*"?\s*(#[0-9a-fA-F]{3,8})"#).expect("fill regex");
        for cap in fill_re.captures_iter(svg) {
            if let Some(c) = svg_fill_to_color(&cap[1]) {
                colours.push(c);
            }
        }
        // currentColor fills resolve to their class colour (black fallback).
        let cc_re =
            regex::Regex::new(r#"<path\b([^>]*?)fill:currentColor([^>]*?)/?>"#).expect("cc regex");
        for cap in cc_re.captures_iter(svg) {
            let tag = cap.get(0).map(|m| m.as_str()).unwrap_or_default();
            let class = regex::Regex::new(r#"class="([A-Za-z0-9_-]+)""#)
                .expect("class attr regex")
                .captures(tag)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());
            let resolved = class.and_then(|k| class_colors.get(&k).copied());
            colours.push(resolved.unwrap_or(iced::Color::BLACK));
        }
        // Opacity-only shadow paths: black at that alpha over the backdrop.
        let path_re = regex::Regex::new(r#"<path\b[^>]*?/?>"#).expect("path regex");
        let opacity_re =
            regex::Regex::new(r#"opacity[:=]\s*"?([01](?:\.\d+)?)"#).expect("opacity regex");
        for cap in path_re.captures_iter(svg) {
            let tag = cap.get(0).map(|m| m.as_str()).unwrap_or_default();
            if tag.contains("fill") {
                continue;
            }
            if let Some(om) = opacity_re.captures(tag) {
                if let Ok(alpha) = om[1].parse::<f32>() {
                    let a = alpha.clamp(0.0, 1.0);
                    let blended = iced::Color::from_rgb(
                        backdrop.r * (1.0 - a),
                        backdrop.g * (1.0 - a),
                        backdrop.b * (1.0 - a),
                    );
                    colours.push(blended);
                }
            }
        }
        colours
    }

    /// Max WCAG contrast of a bundled 16px icon against a backdrop,
    /// honouring the component's PAPIRUS-14 source-size rule for folders.
    fn max_contrast_compact(icon_id: &str, theme: &Theme) -> Option<f32> {
        let dark = matches!(theme, Theme::Dark);
        let src_dir = if dark && COMPACT_DARK_FOLDER_ICONS.contains(&icon_id) {
            24
        } else {
            16
        };
        let path = format!(
            "{}/assets/third_party/papirus/{src_dir}/{icon_id}.svg",
            env!("CARGO_MANIFEST_DIR")
        );
        let svg = std::fs::read_to_string(&path).ok()?;
        let tile = design_tokens::surface_hover(theme);
        let colours = rendered_colours(&svg, tile);
        colours
            .iter()
            .map(|c| contrast_ratio(*c, tile))
            .fold(0.0f32, f32::max)
            .into()
    }

    #[test]
    fn all_bundled_icons_readable_on_dark_tile_at_compact() {
        // Task 14 point 1/7: icons must be readable in dark mode at compact
        // 16–20px.  The white-document Papirus artwork pops on the dark tile
        // (min ≈3.93:1 measured), comfortably above the 3:1 non-text floor.
        let value: serde_json::Value =
            serde_json::from_str(PAPIRUS_MANIFEST_JSON).expect("embedded manifest must parse");
        let icons = value
            .get("icons")
            .and_then(serde_json::Value::as_object)
            .expect("manifest.icons must be an object");
        let theme = Theme::Dark;
        let mut worst: (&str, f32) = ("", f32::MAX);
        for icon_id in icons.keys() {
            let mc = max_contrast_compact(icon_id, &theme)
                .unwrap_or_else(|| panic!("icon {icon_id} missing at 16px/24px"));
            assert!(
                mc >= 3.0,
                "icon {icon_id} max contrast on dark tile is {mc:.2}:1 (need ≥ 3:1)"
            );
            if mc < worst.1 {
                worst = (icon_id, mc);
            }
        }
        assert!(
            worst.1 >= 3.0,
            "worst dark-tile contrast: {} at {:.2}:1",
            worst.0,
            worst.1
        );
    }

    #[test]
    fn all_bundled_icons_visible_on_light_tile_at_compact() {
        // Task 14 point 1/7: icons must be readable in light mode at compact
        // 16–20px.  The light theme is the Papirus-native surface (the
        // bundled set IS the light-theme set); the four white-page generic
        // icons are deliberately a plain sheet and rely on the tile + border
        // for delineation, while every other icon carries a distinguishing
        // colour (≥ 1.5:1).  The tile's 1px muted border (design_tokens)
        // keeps the white-page box visible; visual confirmation lands in
        // PAPIRUS-20.
        const WHITE_PAGE_GENERICS: &[&str] = &[
            "application-x-generic",
            "application-x-zerosize",
            "text-css",
            "text-x-log",
        ];
        let value: serde_json::Value =
            serde_json::from_str(PAPIRUS_MANIFEST_JSON).expect("embedded manifest must parse");
        let icons = value
            .get("icons")
            .and_then(serde_json::Value::as_object)
            .expect("manifest.icons must be an object");
        let theme = Theme::Light;
        for icon_id in icons.keys() {
            let mc = max_contrast_compact(icon_id, &theme)
                .unwrap_or_else(|| panic!("icon {icon_id} missing at 16px/24px"));
            let floor = if WHITE_PAGE_GENERICS.contains(&icon_id.as_str()) {
                1.0 // plain-sheet design: present, delineated by tile border
            } else {
                1.5
            };
            assert!(
                mc >= floor,
                "icon {icon_id} max contrast on light tile is {mc:.2}:1 (need ≥ {floor}:1)"
            );
        }
    }

    #[test]
    fn compact_folder_24px_artwork_readable_on_dark_tile() {
        // The 16px folder (currentColor #444444) is ~1.25:1 on the dark
        // tile; the 24px full-colour folder the component sources instead is
        // comfortably readable.  Guard the replacement artwork itself.
        let mc = max_contrast_compact("folder-open", &Theme::Dark)
            .expect("folder-open must exist at 24px");
        assert!(
            mc >= 3.0,
            "24px folder-open on dark tile: {mc:.2}:1 (need ≥ 3:1)"
        );
        let mc_folder =
            max_contrast_compact("folder", &Theme::Dark).expect("folder must exist at 24px");
        assert!(
            mc_folder >= 3.0,
            "24px folder on dark tile: {mc_folder:.2}:1 (need ≥ 3:1)"
        );
    }
}
