#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]

//! Central file-type resolver for Boru (PAPIRUS-05).
//!
//! Every Boru file-sharing surface that shows a file or folder icon must
//! select that icon through [`resolve_file_icon`].  Individual screens must
//! **not** keep their own extension maps: this module is the single source
//! of truth for "which bundled Papirus icon represents this file".
//!
//! ## Resolution priority
//!
//! The resolver evaluates inputs in this exact order:
//!
//! 1. Explicit directory / folder state
//! 2. Trusted MIME type detected locally after download
//! 3. Validated MIME type already known from the local sharing source
//! 4. Advertised MIME type received from a peer (treated as a **hint**, not truth)
//! 5. Compound filename extension (`.tar.gz`, `.d.ts`, ...)
//! 6. Ordinary filename extension (`.pdf`, `.png`, ...)
//! 7. Broad category fallback (e.g. `video/*` → `Video`)
//! 8. Generic unknown-file fallback
//!
//! The public API collapses priorities 2 and 3 into the single
//! `locally_detected_mime_type` parameter: the caller passes whichever
//! trusted local MIME it has (post-download detection or validated local
//! source).  Both outrank the peer-advertised hint.
//!
//! ## Never a missing asset
//!
//! The resolver embeds the pinned Papirus manifest (PAPIRUS-02/03) and
//! consults it before returning: every candidate icon is checked against
//! the bundled asset set, and the fallback chain walks
//! *exact icon → related extension icon → broad category icon → unknown
//! generic icon*, where the terminal unknown icon is one of the manifest's
//! guaranteed `required_fallbacks`.  A `ResolvedFileIcon::asset_path` is
//! therefore always a path that exists in the pinned bundle.
//!
//! ## Peer MIME is a hint
//!
//! A peer-provided MIME never renames, executes, or opens a file.  When it
//! conflicts strongly with a locally detected type, the resolver prefers
//! the locally detected type for the icon, records the mismatch through the
//! existing `tracing` diagnostics channel (`tracing::warn!`), and exposes
//! the mismatch on [`ResolvedFileIcon::mime_mismatch`] so the UI can show a
//! warning state.
//!
//! ## octet-stream is "no MIME info" (PAPIRUS-21)
//!
//! `application/octet-stream` is the MIME for "unknown binary data"; it
//! carries **no** type signal.  Boru's legacy extension→MIME map stamped
//! this value for every extension it did not recognise, and peers may
//! advertise it for the same reason.  The resolver therefore treats it as
//! an absent hint everywhere in the priority chain: a stored or advertised
//! octet-stream never outranks a real filename extension (priority 6), so
//! `budget.xlsx` with octet-stream resolves to the spreadsheet icon, and an
//! unknown `mystery.crypt` with octet-stream still ends on the generic
//! unknown icon (priority 8).  Because it carries no category signal either,
//! an octet-stream side never triggers a MIME mismatch record.  The
//! `application-octet-stream` icon itself stays reachable through the
//! extension table for explicit `.bin`-style names.
//!
//! ## Security (Task 16)
//!
//! The icon pipeline treats **all** input as untrusted:
//!
//! * A peer-advertised MIME type is a hint (priority 4), never truth; a
//!   locally detected type wins for the icon and the mismatch is recorded.
//! * MIME and filename strings are **names**, never filesystem paths: they
//!   are matched against the static tables below and the pinned manifest,
//!   and no path is ever built from them.  `resolve_file_icon` performs no
//!   file I/O at all — it cannot inspect or decode file contents, and it
//!   never opens or executes anything.
//! * Every repo-relative asset path returned to callers is validated by
//!   [`is_bundled_asset_path`] against the pinned asset root: absolute
//!   paths, `..` components, control bytes, and Windows drive prefixes are
//!   rejected, so a traversal string such as `../../icon.svg` can never
//!   escape the bundled Papirus set.  The `FileTypeIcon` component applies
//!   the same guard before reading SVG bytes from disk.
//!
//! ## Scope of this module
//!
//! The mapping tables below are the initial seed set.  PAPIRUS-08 extends
//! the MIME → icon mapping and PAPIRUS-09 extends coverage; the resolver
//! core, priority chain, and fallback structure are final.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::file_category::FileCategory;

// ── Constants ────────────────────────────────────────────────────────

/// Repo-relative root of the bundled Papirus asset set.
pub const PAPIRUS_ASSET_ROOT: &str = "assets/third_party/papirus";

/// Default icon size returned in `ResolvedFileIcon::asset_path`.
///
/// The `FileTypeIcon` component (PAPIRUS-04) is responsible for semantic
/// sizes; the resolver returns the canonical `card` size and the component
/// can re-query `PapirusCatalog::asset_path(icon_id, size)` for others.
pub const DEFAULT_ICON_SIZE: u16 = 32;

/// Embedded copy of the pinned manifest (generated by PAPIRUS-02/03).
const MANIFEST_JSON: &str = include_str!("../../../assets/third_party/papirus/manifest.json");

/// Terminal fallback icon; guaranteed present by the manifest's
/// `required_fallbacks` list.
const UNKNOWN_ICON: &str = "application-x-generic";

/// Icon used for explicit directory / folder state.
const DIRECTORY_ICON: &str = "folder-open";

/// MIME type that means "unknown binary data", not a concrete type.
///
/// `application/octet-stream` is the placeholder Boru's legacy
/// extension→MIME map (predating PAPIRUS) stamped for every extension it
/// did not recognise, and peers may advertise it for the same reason.  It
/// carries **no** type signal, so the resolver treats it as an absent hint
/// everywhere in the priority chain (PAPIRUS-21): a stored or advertised
/// octet-stream never outranks a real filename extension (priority 6).  The
/// `application-octet-stream` icon itself stays reachable through the
/// extension table for explicit `.bin`-style names.
const MIME_NO_INFO: &str = "application/octet-stream";

// ── Resolution result cache (PAPIRUS-17) ─────────────────────────────

/// Cache key for [`resolve_file_icon`] results: the **normalised** inputs
/// that fully determine the outcome.
///
/// Normalising on this side means `REPORT.PDF` and `report.pdf` (or a MIME
/// with stray case/whitespace/params) share one entry — exactly the
/// high-hit-rate path a file list or chat log exercises, where the same few
/// file types repeat hundreds of times per frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ResolveCacheKey {
    is_directory: bool,
    /// `normalise_mime` output of the peer-advertised MIME; `None` when
    /// absent / whitespace-only.
    advertised_mime: Option<String>,
    /// `normalise_mime` output of the locally detected MIME; `None` when
    /// absent / whitespace-only.
    local_mime: Option<String>,
    /// `normalised_extensions` output of the filename (compound first).
    extensions: Vec<String>,
}

/// Upper bound on the resolver result cache.
///
/// Each entry is a `ResolvedFileIcon` (a few small strings) and the key
/// space is the normalised type universe, so in practice the cache stays
/// tiny.  The cap protects against a pathological peer flooding the chat
/// with unique MIME strings (each unique normalised key would otherwise
/// create an entry); on overflow the whole cache is reset, which is cheap
/// and correct (results are deterministic, so a cold cache just
/// recomputes).
const RESOLVE_CACHE_MAX_ENTRIES: usize = 4096;

static RESOLVE_CACHE: OnceLock<Mutex<HashMap<ResolveCacheKey, ResolvedFileIcon>>> = OnceLock::new();

fn resolve_cache() -> &'static Mutex<HashMap<ResolveCacheKey, ResolvedFileIcon>> {
    RESOLVE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Insert `(key, value)` into `cache`, bounding the size at
/// [`RESOLVE_CACHE_MAX_ENTRIES`].  Exposed as a pure function so the bound
/// is testable without touching the process-global cache.
fn bounded_resolve_cache_insert(
    cache: &mut HashMap<ResolveCacheKey, ResolvedFileIcon>,
    key: ResolveCacheKey,
    value: ResolvedFileIcon,
) {
    if cache.len() >= RESOLVE_CACHE_MAX_ENTRIES {
        cache.clear();
    }
    cache.insert(key, value);
}

/// Normalise a MIME hint for cache keying; `None` when it carries no type
/// signal — absent, whitespace-only, or the `application/octet-stream`
/// "unknown binary" placeholder.  PAPIRUS-21 treats octet-stream as no
/// information, so it must not create a distinct cache entry from absent.
fn meaningful_mime(mime: Option<&str>) -> Option<String> {
    mime.map(normalise_mime)
        .filter(|m| !m.is_empty() && m != MIME_NO_INFO)
}

/// Build the normalised cache key for a resolution request.
///
/// `is_directory` and the **normalised** MIME strings and extension
/// candidates fully determine [`resolve_file_icon`]'s output: the original
/// filename only matters through its normalised extension candidates, and
/// MIME lookups normalise internally, so two requests that differ only in
/// case/whitespace/parameters share one entry.
fn resolve_cache_key(
    filename: &str,
    advertised_mime_type: Option<&str>,
    locally_detected_mime_type: Option<&str>,
    is_directory: bool,
) -> ResolveCacheKey {
    ResolveCacheKey {
        is_directory,
        advertised_mime: meaningful_mime(advertised_mime_type),
        local_mime: meaningful_mime(locally_detected_mime_type),
        extensions: normalised_extensions(filename),
    }
}

// ── Public types ─────────────────────────────────────────────────────

/// The resolved presentation of a file or folder.
///
/// This is a **local** presentation concern: the category is never
/// transmitted, and no file-transfer payload or message type is modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFileIcon {
    /// Papirus icon identifier, e.g. `"application-pdf"`.
    pub icon_id: String,
    /// Repo-relative asset path that exists in the pinned bundle, e.g.
    /// `"assets/third_party/papirus/32/application-pdf.svg"`.
    pub asset_path: String,
    /// Coarse presentation category derived during resolution.
    pub file_category: FileCategory,
    /// Human-readable label for tooltips / accessible text.
    pub display_label: String,
    /// How strongly the icon matches the actual file type.
    pub confidence: IconConfidence,
    /// Which priority level won the resolution.
    pub source: ResolutionSource,
    /// Present when a peer-advertised MIME strongly conflicted with a
    /// locally detected MIME (local won for the icon).
    pub mime_mismatch: Option<MimeMismatch>,
}

/// Ordered confidence ladder for a resolved icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IconConfidence {
    /// Explicit directory state or a trusted local MIME type.
    Exact,
    /// Validated local MIME or a compound filename extension.
    Strong,
    /// Advertised (peer-hinted) MIME or an ordinary extension.
    Medium,
    /// Broad category fallback.
    Weak,
    /// Generic unknown-file fallback.
    None,
}

/// Which input won the resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSource {
    /// Explicit directory / folder state (priority 1).
    Directory,
    /// Trusted local MIME: detected after download or validated from the
    /// local sharing source (priorities 2–3).
    LocalMime,
    /// Advertised MIME received from a peer (priority 4, a hint).
    AdvertisedMime,
    /// Compound filename extension (priority 5).
    CompoundExtension,
    /// Ordinary filename extension (priority 6).
    Extension,
    /// Broad category fallback (priority 7).
    CategoryFallback,
    /// Generic unknown-file fallback (priority 8).
    UnknownFallback,
}

/// A recorded conflict between a peer-advertised MIME and a locally
/// detected MIME.  The locally detected type wins for the icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeMismatch {
    /// MIME type advertised by the remote peer.
    pub advertised: String,
    /// MIME type detected or validated locally.
    pub locally_detected: String,
    /// Category derived from the advertised MIME.
    pub advertised_category: FileCategory,
    /// Category derived from the locally detected MIME.
    pub locally_detected_category: FileCategory,
}

// ── Manifest catalog ─────────────────────────────────────────────────

/// Parsed view of the pinned Papirus manifest.
///
/// `icons: icon_id -> (size -> manifest-relative path)`.
#[derive(Debug)]
struct PapirusCatalog {
    icons: HashMap<String, HashMap<u16, String>>,
    required_fallbacks: Vec<String>,
    /// Alias dedup (PAPIRUS-17): manifest-relative path -> canonical member
    /// of the same content-duplicate group (e.g. `32/audio-x-m4a.svg` ->
    /// `32/audio-flac.svg`).  Built from the manifest's `duplicates.groups`;
    /// the canonical member is the lexicographically smallest path in each
    /// group, so identical SVG content is loaded and cached **once** no
    /// matter which alias resolved it.
    canonical_paths: HashMap<String, String>,
}

static CATALOG: OnceLock<PapirusCatalog> = OnceLock::new();

impl PapirusCatalog {
    /// Access the shared catalog parsed from the embedded manifest.
    fn global() -> &'static PapirusCatalog {
        CATALOG.get_or_init(PapirusCatalog::load)
    }

    fn load() -> Self {
        let value: serde_json::Value =
            serde_json::from_str(MANIFEST_JSON).expect("embedded Papirus manifest must parse");

        let mut icons = HashMap::new();
        if let Some(icons_obj) = value.get("icons").and_then(serde_json::Value::as_object) {
            for (icon_id, sizes) in icons_obj {
                let mut size_map = HashMap::new();
                if let Some(sizes_obj) = sizes.as_object() {
                    for (size, path) in sizes_obj {
                        if let (Ok(size), Some(path)) = (size.parse::<u16>(), path.as_str()) {
                            size_map.insert(size, path.to_string());
                        }
                    }
                }
                icons.insert(icon_id.clone(), size_map);
            }
        }

        let required_fallbacks = value
            .get("required_fallbacks")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        // PAPIRUS-17 alias dedup: manifest `duplicates.groups` lists the
        // bundle paths that share identical content (keyed by content
        // hash).  For each group pick the lexicographically smallest path
        // as the canonical member and map every member to it, so aliases
        // (`audio-x-m4a` ≡ `audio-flac`) resolve to a single real file and
        // the SVG handle cache stores one entry per distinct content.
        let mut canonical_paths: HashMap<String, String> = HashMap::new();
        if let Some(groups) = value
            .get("duplicates")
            .and_then(|d| d.get("groups"))
            .and_then(serde_json::Value::as_object)
        {
            for members in groups.values() {
                let Some(members) = members.as_array() else {
                    continue;
                };
                let mut paths: Vec<&str> = members
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect();
                paths.sort_unstable();
                let Some(canonical) = paths.first().copied() else {
                    continue;
                };
                for member in paths {
                    canonical_paths
                        .entry(member.to_string())
                        .or_insert_with(|| canonical.to_string());
                }
            }
        }

        PapirusCatalog {
            icons,
            required_fallbacks,
            canonical_paths,
        }
    }

    /// Whether `icon_id` is present in the pinned bundle.
    fn has_icon(&self, icon_id: &str) -> bool {
        self.icons.contains_key(icon_id)
    }

    /// Manifest-relative path for `icon_id` at `size`, e.g.
    /// `"32/application-pdf.svg"`.
    fn manifest_path(&self, icon_id: &str, size: u16) -> Option<&str> {
        self.icons.get(icon_id)?.get(&size).map(String::as_str)
    }

    /// Repo-relative path for `icon_id` at `size`, e.g.
    /// `"assets/third_party/papirus/32/application-pdf.svg"`.
    ///
    /// **Alias dedup (PAPIRUS-17):** when the icon's manifest path is a
    /// member of a content-duplicate group, the returned path is the
    /// group's canonical member, so identical SVG content is read and
    /// cached once regardless of which alias resolved it (e.g.
    /// `audio-x-m4a` and `audio-flac` both return
    /// `.../32/audio-flac.svg`).
    ///
    /// The manifest is bundled at build time and therefore trusted, but
    /// the returned path is still passed through [`is_bundled_asset_path`]
    /// (Task 16 defense in depth): even a corrupted manifest entry that
    /// escaped the bundle would be rejected here instead of reaching a
    /// filesystem read.
    pub fn asset_path(&self, icon_id: &str, size: u16) -> Option<String> {
        let manifest_path = self.manifest_path(icon_id, size)?;
        let canonical = self
            .canonical_paths
            .get(manifest_path)
            .map(String::as_str)
            .unwrap_or(manifest_path);
        let repo_relative = format!("{PAPIRUS_ASSET_ROOT}/{canonical}");
        if !is_bundled_asset_path(&repo_relative) {
            return None;
        }
        Some(repo_relative)
    }
}

/// Repo-relative path for `icon_id` at a semantic Papirus size directory
/// (16/24/32/48/64), e.g. `"assets/third_party/papirus/48/application-pdf.svg"`.
///
/// This is the PAPIRUS-04 size-lookup hook: [`resolve_file_icon`] returns
/// the canonical `card` (32px) path on [`ResolvedFileIcon::asset_path`],
/// and the `FileTypeIcon` component re-queries the pinned bundle here for
/// the other semantic sizes (compact/list/large/hero).  `None` means the
/// icon does not exist at that size in the pinned bundle; the component
/// must fall back to the unknown-generic icon — never a missing asset.
pub fn papirus_asset_path(icon_id: &str, size: u16) -> Option<String> {
    PapirusCatalog::global().asset_path(icon_id, size)
}

/// Whether `path` is a safe **repo-relative** asset path inside the pinned
/// Papirus asset root (Task 16 path-traversal guard).
///
/// Only paths produced by the manifest (or code that independently
/// reconstructs a manifest path) may be passed to a filesystem read.  This
/// validator enforces the asset allow-list shape:
///
/// * non-empty,
/// * no control bytes (NUL, newline, ...),
/// * relative — never `/`-absolute and never a Windows drive prefix
///   (`C:`), and no leading `\` separator,
/// * no `..` component (checked on both `/` and `\` separators),
/// * starts with the pinned [`PAPIRUS_ASSET_ROOT`].
///
/// A `..`-free path that starts with the root cannot escape the bundle:
/// `join`-ing it onto `CARGO_MANIFEST_DIR` stays inside
/// `<repo>/assets/third_party/papirus/`.
pub fn is_bundled_asset_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    // Reject control bytes (NUL, newline, carriage return, tab, ...).
    if path.bytes().any(|b| b < 0x20) {
        return false;
    }
    // Reject absolute POSIX paths and backslash-absolute paths.
    if path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    // Reject Windows drive prefixes (`C:\...`, `C:/...`).
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        return false;
    }
    // Reject any `..` component, regardless of separator style.
    if path.split(['/', '\\']).any(|component| component == "..") {
        return false;
    }
    path.starts_with(PAPIRUS_ASSET_ROOT)
}

// ── Seed mapping tables (PAPIRUS-08/09 extend) ───────────────────────

/// (extension, icon_id, category) — ordinary extensions.
///
/// Extensions are matched case-insensitively after trimming; compound
/// extensions are checked before ordinary ones.
const EXTENSION_ICONS: &[(&str, &str, FileCategory)] = &[
    // Documents
    ("pdf", "application-pdf", FileCategory::Pdf),
    ("doc", "application-msword", FileCategory::Document),
    (
        "docx",
        "application-vnd.openxmlformats-officedocument.wordprocessingml.document",
        FileCategory::Document,
    ),
    (
        "odt",
        "application-vnd.oasis.opendocument.text",
        FileCategory::Document,
    ),
    ("rtf", "application-rtf", FileCategory::Document),
    // Spreadsheets
    ("xls", "application-vnd.ms-excel", FileCategory::Spreadsheet),
    (
        "xlsx",
        "application-vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        FileCategory::Spreadsheet,
    ),
    (
        "ods",
        "application-vnd.oasis.opendocument.spreadsheet",
        FileCategory::Spreadsheet,
    ),
    ("csv", "text-csv", FileCategory::Spreadsheet),
    (
        "tsv",
        "text-tab-separated-values",
        FileCategory::Spreadsheet,
    ),
    // Presentations
    (
        "ppt",
        "application-vnd.ms-powerpoint",
        FileCategory::Presentation,
    ),
    (
        "pptx",
        "application-vnd.openxmlformats-officedocument.presentationml.presentation",
        FileCategory::Presentation,
    ),
    (
        "odp",
        "application-vnd.oasis.opendocument.presentation",
        FileCategory::Presentation,
    ),
    // Text / markdown
    ("txt", "text-plain", FileCategory::Text),
    ("log", "text-x-log", FileCategory::Text),
    ("md", "text-markdown", FileCategory::Markdown),
    ("markdown", "text-markdown", FileCategory::Markdown),
    // Source code
    ("rs", "text-rust", FileCategory::SourceCode),
    ("py", "text-x-python", FileCategory::SourceCode),
    ("js", "application-javascript", FileCategory::SourceCode),
    ("ts", "text-javascript", FileCategory::SourceCode),
    ("html", "text-html", FileCategory::SourceCode),
    ("css", "text-css", FileCategory::SourceCode),
    ("json", "application-json", FileCategory::SourceCode),
    ("xml", "application-xml", FileCategory::SourceCode),
    ("yaml", "application-yaml", FileCategory::SourceCode),
    ("yml", "application-yaml", FileCategory::SourceCode),
    ("toml", "application-toml", FileCategory::SourceCode),
    ("sh", "text-x-shellscript", FileCategory::SourceCode),
    ("bash", "text-x-shellscript", FileCategory::SourceCode),
    ("zsh", "text-x-shellscript", FileCategory::SourceCode),
    ("fish", "text-x-shellscript", FileCategory::SourceCode),
    ("ps1", "text-x-script", FileCategory::SourceCode),
    ("psm1", "text-x-script", FileCategory::SourceCode),
    ("psd1", "text-x-script", FileCategory::SourceCode),
    ("java", "text-x-java", FileCategory::SourceCode),
    ("kt", "text-x-kotlin", FileCategory::SourceCode),
    ("kts", "text-x-kotlin", FileCategory::SourceCode),
    ("c", "text-x-csrc", FileCategory::SourceCode),
    ("h", "text-x-chdr", FileCategory::SourceCode),
    ("cpp", "text-x-c++src", FileCategory::SourceCode),
    ("cc", "text-x-c++src", FileCategory::SourceCode),
    ("cxx", "text-x-c++src", FileCategory::SourceCode),
    ("hpp", "text-x-chdr", FileCategory::SourceCode),
    ("cs", "text-x-csharp", FileCategory::SourceCode),
    ("go", "text-x-go", FileCategory::SourceCode),
    ("sql", "application-sql", FileCategory::SourceCode),
    // Images
    ("png", "image-png", FileCategory::Image),
    ("jpg", "image-jpeg", FileCategory::Image),
    ("jpeg", "image-jpeg", FileCategory::Image),
    ("gif", "image-gif", FileCategory::Image),
    ("bmp", "image-bmp", FileCategory::Image),
    ("svg", "image-svg+xml", FileCategory::Image),
    ("tiff", "image-tiff", FileCategory::Image),
    ("tif", "image-tiff", FileCategory::Image),
    // WEBP / HEIC / HEIF / RAW (no dedicated Papirus icons in the pinned
    // bundle; closest existing asset per the fallback chain).
    ("webp", "image", FileCategory::Image),
    ("heic", "image", FileCategory::Image),
    ("heif", "image", FileCategory::Image),
    ("dng", "image-x-adobe-dng", FileCategory::Image),
    ("cr2", "image-x-adobe-dng", FileCategory::Image),
    ("nef", "image-x-adobe-dng", FileCategory::Image),
    ("arw", "image-x-adobe-dng", FileCategory::Image),
    ("raw", "image-x-adobe-dng", FileCategory::Image),
    // Video
    ("mp4", "video-mp4", FileCategory::Video),
    ("webm", "video-webm", FileCategory::Video),
    ("mkv", "video-x-matroska", FileCategory::Video),
    ("avi", "video-x-msvideo", FileCategory::Video),
    ("ogv", "video-x-theora+ogg", FileCategory::Video),
    ("m4v", "video-mp4", FileCategory::Video),
    // MOV / MPEG (no dedicated icons in the pinned bundle; generic video).
    ("mov", "video", FileCategory::Video),
    ("mpeg", "video", FileCategory::Video),
    ("mpg", "video", FileCategory::Video),
    // Audio
    ("mp3", "audio-mp3", FileCategory::Audio),
    ("flac", "audio-flac", FileCategory::Audio),
    ("wav", "audio-x-wav", FileCategory::Audio),
    ("ogg", "audio-x-vorbis+ogg", FileCategory::Audio),
    ("m4a", "audio-x-m4a", FileCategory::Audio),
    ("wma", "audio-x-ms-wma", FileCategory::Audio),
    // OPUS / AAC (no dedicated icons in the pinned bundle; generic audio).
    ("opus", "audio-x-generic", FileCategory::Audio),
    ("aac", "audio-x-generic", FileCategory::Audio),
    // Archives
    ("zip", "application-zip", FileCategory::Archive),
    ("7z", "application-x-7z-compressed", FileCategory::Archive),
    ("rar", "application-vnd.rar", FileCategory::Archive),
    ("tar", "application-x-tar", FileCategory::Archive),
    ("gz", "application-gzip", FileCategory::Archive),
    ("bz2", "application-x-bzip2", FileCategory::Archive),
    (
        "xz",
        "application-x-xz-compressed-tar",
        FileCategory::Archive,
    ),
    ("zst", "application-zstd", FileCategory::Archive),
    // Executables / installers / images
    (
        "exe",
        "application-x-ms-dos-executable",
        FileCategory::Executable,
    ),
    (
        "deb",
        "application-vnd.debian.binary-package",
        FileCategory::Installer,
    ),
    ("rpm", "application-x-rpm", FileCategory::Installer),
    (
        "apk",
        "application-vnd.android.package-archive",
        FileCategory::Installer,
    ),
    ("iso", "application-x-cd-image", FileCategory::DiskImage),
    (
        "img",
        "application-x-raw-disk-image",
        FileCategory::DiskImage,
    ),
    (
        "dmg",
        "application-x-apple-diskimage",
        FileCategory::DiskImage,
    ),
    // Additional virtual/cloud disk images (closest existing asset).
    ("vhd", "application-x-cd-image", FileCategory::DiskImage),
    ("vmdk", "application-x-cd-image", FileCategory::DiskImage),
    ("vdi", "application-x-cd-image", FileCategory::DiskImage),
    ("qcow2", "application-x-cd-image", FileCategory::DiskImage),
    // Executables / shared libraries / batch scripts.
    ("elf", "application-x-executable", FileCategory::Executable),
    ("so", "application-x-executable", FileCategory::Executable),
    ("dll", "application-x-executable", FileCategory::Executable),
    (
        "dylib",
        "application-x-executable",
        FileCategory::Executable,
    ),
    (
        "bat",
        "application-x-ms-dos-executable",
        FileCategory::Executable,
    ),
    (
        "cmd",
        "application-x-ms-dos-executable",
        FileCategory::Executable,
    ),
    // Installers / packages.
    ("msi", "package-x-generic", FileCategory::Installer),
    ("pkg", "package-x-generic", FileCategory::Installer),
    (
        "appimage",
        "application-x-iso9660-appimage",
        FileCategory::Installer,
    ),
    ("flatpak", "package-x-generic", FileCategory::Installer),
    ("snap", "package-x-generic", FileCategory::Installer),
    // Databases / fonts / certificates / keys
    ("sqlite", "application-x-sqlite3", FileCategory::Database),
    ("sqlite3", "application-x-sqlite3", FileCategory::Database),
    ("db", "application-x-sqlite3", FileCategory::Database),
    ("dbf", "application-x-sqlite3", FileCategory::Database),
    ("ttf", "application-x-font-ttf", FileCategory::Font),
    ("otf", "application-x-font-otf", FileCategory::Font),
    ("ttc", "application-x-font-ttf", FileCategory::Font),
    ("woff", "font-x-generic", FileCategory::Font),
    ("woff2", "font-x-generic", FileCategory::Font),
    ("crt", "application-certificate", FileCategory::Certificate),
    ("cer", "application-certificate", FileCategory::Certificate),
    ("der", "application-pkix-cert", FileCategory::Certificate),
    ("p12", "application-pkix-cert", FileCategory::Certificate),
    ("pfx", "application-pkix-cert", FileCategory::Certificate),
    ("pem", "application-x-pem-key", FileCategory::Key),
    ("key", "application-pgp-keys", FileCategory::Key),
    ("pub", "application-pgp-keys", FileCategory::Key),
    ("asc", "application-pgp-keys", FileCategory::Key),
    ("gpg", "application-pgp-keys", FileCategory::Key),
    ("ppk", "application-pgp-keys", FileCategory::Key),
    // Ebooks / torrents / CAD / 3D
    ("epub", "application-epub+zip", FileCategory::Ebook),
    ("mobi", "application-epub+zip", FileCategory::Ebook),
    ("torrent", "application-x-bittorrent", FileCategory::Torrent),
    ("step", "application-x-step", FileCategory::Cad),
    ("stp", "application-x-step", FileCategory::Cad),
    ("dwg", "application-x-step", FileCategory::Cad),
    ("dxf", "application-x-step", FileCategory::Cad),
    ("stl", "model-stl", FileCategory::ThreeDimensional),
    ("obj", "model-stl", FileCategory::ThreeDimensional),
    ("fbx", "model-stl", FileCategory::ThreeDimensional),
    ("glb", "model-stl", FileCategory::ThreeDimensional),
    ("gltf", "model-stl", FileCategory::ThreeDimensional),
    ("blend", "model-stl", FileCategory::ThreeDimensional),
    ("3ds", "model-stl", FileCategory::ThreeDimensional),
    // Unknown binary (closest existing asset: generic octet-stream).
    ("bin", "application-octet-stream", FileCategory::Unknown),
];

/// Compound extensions checked before ordinary extensions.
///
/// The exact matching behaviour (`archive.tar.gz` → tar archive rather than
/// a generic `.gz`) is part of the priority chain; PAPIRUS-06 extends the
/// normalisation rules.
const COMPOUND_EXTENSIONS: &[(&str, &str, FileCategory)] = &[
    ("tar.gz", "application-x-tar", FileCategory::Archive),
    ("tar.bz2", "application-x-tar", FileCategory::Archive),
    (
        "tar.xz",
        "application-x-xz-compressed-tar",
        FileCategory::Archive,
    ),
    ("tar.zst", "application-zstd", FileCategory::Archive),
    (
        "user.js",
        "application-javascript",
        FileCategory::SourceCode,
    ),
    ("d.ts", "text-javascript", FileCategory::SourceCode),
    ("min.js", "application-javascript", FileCategory::SourceCode),
    ("min.css", "text-css", FileCategory::SourceCode),
];

/// (MIME type, icon_id, category) — seed MIME mapping (PAPIRUS-08 extends).
const MIME_ICONS: &[(&str, &str, FileCategory)] = &[
    ("application/pdf", "application-pdf", FileCategory::Pdf),
    (
        "application/msword",
        "application-msword",
        FileCategory::Document,
    ),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "application-vnd.openxmlformats-officedocument.wordprocessingml.document",
        FileCategory::Document,
    ),
    (
        "application/vnd.oasis.opendocument.text",
        "application-vnd.oasis.opendocument.text",
        FileCategory::Document,
    ),
    ("application/rtf", "application-rtf", FileCategory::Document),
    (
        "application/vnd.ms-excel",
        "application-vnd.ms-excel",
        FileCategory::Spreadsheet,
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "application-vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        FileCategory::Spreadsheet,
    ),
    (
        "application/vnd.oasis.opendocument.spreadsheet",
        "application-vnd.oasis.opendocument.spreadsheet",
        FileCategory::Spreadsheet,
    ),
    (
        "application/vnd.ms-powerpoint",
        "application-vnd.ms-powerpoint",
        FileCategory::Presentation,
    ),
    (
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "application-vnd.openxmlformats-officedocument.presentationml.presentation",
        FileCategory::Presentation,
    ),
    (
        "application/vnd.oasis.opendocument.presentation",
        "application-vnd.oasis.opendocument.presentation",
        FileCategory::Presentation,
    ),
    ("text/plain", "text-plain", FileCategory::Text),
    ("text/markdown", "text-markdown", FileCategory::Markdown),
    ("text/x-log", "text-x-log", FileCategory::Text),
    ("text/csv", "text-csv", FileCategory::Spreadsheet),
    ("text/html", "text-html", FileCategory::SourceCode),
    ("text/css", "text-css", FileCategory::SourceCode),
    (
        "application/json",
        "application-json",
        FileCategory::SourceCode,
    ),
    (
        "application/xml",
        "application-xml",
        FileCategory::SourceCode,
    ),
    (
        "application/yaml",
        "application-yaml",
        FileCategory::SourceCode,
    ),
    (
        "application/toml",
        "application-toml",
        FileCategory::SourceCode,
    ),
    (
        "application/sql",
        "application-sql",
        FileCategory::SourceCode,
    ),
    ("image/png", "image-png", FileCategory::Image),
    ("image/jpeg", "image-jpeg", FileCategory::Image),
    ("image/gif", "image-gif", FileCategory::Image),
    ("image/bmp", "image-bmp", FileCategory::Image),
    ("image/svg+xml", "image-svg+xml", FileCategory::Image),
    ("image/tiff", "image-tiff", FileCategory::Image),
    ("video/mp4", "video-mp4", FileCategory::Video),
    ("video/webm", "video-webm", FileCategory::Video),
    ("video/x-matroska", "video-x-matroska", FileCategory::Video),
    ("video/x-msvideo", "video-x-msvideo", FileCategory::Video),
    ("audio/mpeg", "audio-mpeg", FileCategory::Audio),
    ("audio/mp3", "audio-mp3", FileCategory::Audio),
    ("audio/flac", "audio-flac", FileCategory::Audio),
    ("audio/x-wav", "audio-x-wav", FileCategory::Audio),
    ("audio/ogg", "audio-x-vorbis+ogg", FileCategory::Audio),
    ("audio/x-m4a", "audio-x-m4a", FileCategory::Audio),
    ("application/zip", "application-zip", FileCategory::Archive),
    (
        "application/x-7z-compressed",
        "application-x-7z-compressed",
        FileCategory::Archive,
    ),
    (
        "application/vnd.rar",
        "application-vnd.rar",
        FileCategory::Archive,
    ),
    (
        "application/x-tar",
        "application-x-tar",
        FileCategory::Archive,
    ),
    (
        "application/gzip",
        "application-gzip",
        FileCategory::Archive,
    ),
    (
        "application/x-bzip2",
        "application-x-bzip2",
        FileCategory::Archive,
    ),
    (
        "application/x-xz-compressed-tar",
        "application-x-xz-compressed-tar",
        FileCategory::Archive,
    ),
    (
        "application/zstd",
        "application-zstd",
        FileCategory::Archive,
    ),
    (
        "application/x-ms-dos-executable",
        "application-x-ms-dos-executable",
        FileCategory::Executable,
    ),
    (
        "application/vnd.debian.binary-package",
        "application-vnd.debian.binary-package",
        FileCategory::Installer,
    ),
    (
        "application/x-rpm",
        "application-x-rpm",
        FileCategory::Installer,
    ),
    (
        "application/vnd.android.package-archive",
        "application-vnd.android.package-archive",
        FileCategory::Installer,
    ),
    (
        "application/x-cd-image",
        "application-x-cd-image",
        FileCategory::DiskImage,
    ),
    (
        "application/x-apple-diskimage",
        "application-x-apple-diskimage",
        FileCategory::DiskImage,
    ),
    (
        "application/x-raw-disk-image",
        "application-x-raw-disk-image",
        FileCategory::DiskImage,
    ),
    (
        "application/x-sqlite3",
        "application-x-sqlite3",
        FileCategory::Database,
    ),
    (
        "application/vnd.sqlite3",
        "application-vnd.sqlite3",
        FileCategory::Database,
    ),
    (
        "application/x-font-ttf",
        "application-x-font-ttf",
        FileCategory::Font,
    ),
    (
        "application/x-font-otf",
        "application-x-font-otf",
        FileCategory::Font,
    ),
    (
        "application/x-pem-key",
        "application-x-pem-key",
        FileCategory::Key,
    ),
    (
        "application/pgp-keys",
        "application-pgp-keys",
        FileCategory::Key,
    ),
    (
        "application/epub+zip",
        "application-epub+zip",
        FileCategory::Ebook,
    ),
    (
        "application/x-bittorrent",
        "application-x-bittorrent",
        FileCategory::Torrent,
    ),
    (
        "application/x-step",
        "application-x-step",
        FileCategory::Cad,
    ),
    ("model/stl", "model-stl", FileCategory::ThreeDimensional),
    // ── PAPIRUS-08: Task 9 MIME-class coverage ─────────────────────
    // Every icon below was verified against the pinned manifest
    // (assets/third_party/papirus/manifest.json).  MIME types whose exact
    // Papirus icon is absent from the bundle map to the closest existing
    // icon per the fallback chain (e.g. image/webp → generic `image`,
    // video/quicktime → generic `video`).
    //
    // Spreadsheets: TSV
    (
        "text/tab-separated-values",
        "text-tab-separated-values",
        FileCategory::Spreadsheet,
    ),
    // Images: WEBP / HEIC / HEIF / RAW / PSD / TGA
    ("image/webp", "image", FileCategory::Image),
    ("image/heic", "image", FileCategory::Image),
    ("image/heif", "image", FileCategory::Image),
    (
        "image/x-adobe-dng",
        "image-x-adobe-dng",
        FileCategory::Image,
    ),
    ("image/x-ms-bmp", "image-bmp", FileCategory::Image),
    (
        "image/vnd.adobe.photoshop",
        "image-vnd.adobe.photoshop",
        FileCategory::Image,
    ),
    ("image/x-tga", "image-x-tga", FileCategory::Image),
    // Video: MOV / MPEG / M4V / OGV / MPEG-TS
    ("video/quicktime", "video", FileCategory::Video),
    ("video/mpeg", "video", FileCategory::Video),
    ("video/x-m4v", "video-mp4", FileCategory::Video),
    ("video/ogg", "video-x-theora+ogg", FileCategory::Video),
    (
        "video/x-theora+ogg",
        "video-x-theora+ogg",
        FileCategory::Video,
    ),
    ("video/mp2t", "video-mp2t", FileCategory::Video),
    // Audio: WAV / M4A / AAC / OPUS / WMA
    ("audio/wav", "audio-x-wav", FileCategory::Audio),
    ("audio/mp4", "audio-x-m4a", FileCategory::Audio),
    ("audio/aac", "audio-x-generic", FileCategory::Audio),
    ("audio/opus", "audio-x-generic", FileCategory::Audio),
    ("audio/x-ms-wma", "audio-x-ms-wma", FileCategory::Audio),
    // Source code: Rust / JS / TS / Python / Java / Kotlin / C / C++ /
    // C# / Go / shell / PowerShell / SQL / XML
    ("text/x-rust", "text-x-rust", FileCategory::SourceCode),
    (
        "application/javascript",
        "application-javascript",
        FileCategory::SourceCode,
    ),
    (
        "text/javascript",
        "text-javascript",
        FileCategory::SourceCode,
    ),
    (
        "text/typescript",
        "text-javascript",
        FileCategory::SourceCode,
    ),
    ("text/x-python", "text-x-python", FileCategory::SourceCode),
    ("text/x-java", "text-x-java", FileCategory::SourceCode),
    ("text/x-kotlin", "text-x-kotlin", FileCategory::SourceCode),
    ("text/x-c", "text-x-csrc", FileCategory::SourceCode),
    ("text/x-c++", "text-x-c++src", FileCategory::SourceCode),
    ("text/x-csharp", "text-x-csharp", FileCategory::SourceCode),
    ("text/x-go", "text-x-go", FileCategory::SourceCode),
    (
        "text/x-shellscript",
        "text-x-shellscript",
        FileCategory::SourceCode,
    ),
    (
        "text/x-powershell",
        "text-x-script",
        FileCategory::SourceCode,
    ),
    ("text/x-sql", "text-x-sql", FileCategory::SourceCode),
    ("text/xml", "text-xml", FileCategory::SourceCode),
    // Executables / installers / disk images / archives / fonts /
    // certificates / keys / ebooks / 3D / unknown binaries
    (
        "application/x-executable",
        "application-x-executable",
        FileCategory::Executable,
    ),
    (
        "application/x-sharedlib",
        "application-x-executable",
        FileCategory::Executable,
    ),
    (
        "application/x-iso9660-appimage",
        "application-x-iso9660-appimage",
        FileCategory::Installer,
    ),
    (
        "application/x-iso9660-image",
        "application-x-cd-image",
        FileCategory::DiskImage,
    ),
    (
        "application/vnd.efi.iso",
        "application-vnd.efi.iso",
        FileCategory::DiskImage,
    ),
    (
        "application/x-archive",
        "application-x-archive",
        FileCategory::Archive,
    ),
    ("font/ttf", "application-x-font-ttf", FileCategory::Font),
    ("font/otf", "application-x-font-otf", FileCategory::Font),
    (
        "application/x-x509-ca-cert",
        "application-pkix-cert",
        FileCategory::Certificate,
    ),
    (
        "application/pkix-cert",
        "application-pkix-cert",
        FileCategory::Certificate,
    ),
    (
        "application/x-pem-file",
        "application-x-pem-key",
        FileCategory::Key,
    ),
    (
        "application/x-mobipocket-ebook",
        "application-epub+zip",
        FileCategory::Ebook,
    ),
    ("model/obj", "model-stl", FileCategory::ThreeDimensional),
    ("model/gltf", "model-stl", FileCategory::ThreeDimensional),
    // `application/octet-stream` maps to the generic binary icon, but the
    // entry is deliberately bypassed by `mime_lookup` (PAPIRUS-21: octet-
    // stream is "no MIME info" — see module docs).  The icon stays in this
    // table so the manifest-verification test confirms it is bundled, and
    // it remains reachable via the `.bin` extension entry in
    // `EXTENSION_ICONS`.
    (
        "application/octet-stream",
        "application-octet-stream",
        FileCategory::Unknown,
    ),
    (
        "application/x-zerosize",
        "application-x-zerosize",
        FileCategory::Unknown,
    ),
    // ── PAPIRUS-09: Task 9 coverage completion ────────────────────
    // Common MIME aliases for the required Task 9 classes; every icon
    // verified against the pinned manifest.
    (
        "text/x-typescript",
        "text-javascript",
        FileCategory::SourceCode,
    ),
    ("text/x-markdown", "text-markdown", FileCategory::Markdown),
    (
        "application/x-msdownload",
        "application-x-ms-dos-executable",
        FileCategory::Executable,
    ),
    (
        "application/x-msi",
        "package-x-generic",
        FileCategory::Installer,
    ),
    (
        "application/x-powershell",
        "text-x-script",
        FileCategory::SourceCode,
    ),
    (
        "application/x-gzip",
        "application-x-gzip",
        FileCategory::Archive,
    ),
    (
        "application/x-rar",
        "application-x-rar",
        FileCategory::Archive,
    ),
    (
        "application/x-font-woff",
        "font-x-generic",
        FileCategory::Font,
    ),
    ("font/woff", "font-x-generic", FileCategory::Font),
    ("font/woff2", "font-x-generic", FileCategory::Font),
    (
        "application/x-pkcs12",
        "application-pkix-cert",
        FileCategory::Certificate,
    ),
    (
        "application/pgp-signature",
        "application-pgp-keys",
        FileCategory::Key,
    ),
    (
        "image/x-canon-cr2",
        "image-x-adobe-dng",
        FileCategory::Image,
    ),
    (
        "image/x-nikon-nef",
        "image-x-adobe-dng",
        FileCategory::Image,
    ),
    ("image/x-sony-arw", "image-x-adobe-dng", FileCategory::Image),
    (
        "application/x-vhd",
        "application-x-cd-image",
        FileCategory::DiskImage,
    ),
    (
        "application/x-vmdk",
        "application-x-cd-image",
        FileCategory::DiskImage,
    ),
    (
        "application/x-dbf",
        "application-x-sqlite3",
        FileCategory::Database,
    ),
    ("model/step", "application-x-step", FileCategory::Cad),
];

/// Broad-category fallback icon per category (priority 7).
const CATEGORY_FALLBACK_ICONS: &[(FileCategory, &str)] = &[
    (FileCategory::Folder, "folder-open"),
    (FileCategory::Document, "application-msword"),
    (FileCategory::Pdf, "application-pdf"),
    (FileCategory::Spreadsheet, "application-vnd.ms-excel"),
    (FileCategory::Presentation, "application-vnd.ms-powerpoint"),
    (FileCategory::Text, "text-x-generic"),
    (FileCategory::Markdown, "text-markdown"),
    (FileCategory::SourceCode, "text-x-generic"),
    (FileCategory::Image, "image-x-generic"),
    (FileCategory::Video, "video-x-generic"),
    (FileCategory::Audio, "audio-x-generic"),
    (FileCategory::Archive, "application-x-archive"),
    (FileCategory::Executable, "application-x-executable"),
    (FileCategory::Installer, "package-x-generic"),
    (FileCategory::DiskImage, "application-x-cd-image"),
    (FileCategory::Database, "application-x-sqlite3"),
    (FileCategory::Font, "font-x-generic"),
    (FileCategory::Certificate, "application-certificate"),
    (FileCategory::Key, "application-pgp-keys"),
    (FileCategory::Ebook, "application-epub+zip"),
    (FileCategory::Torrent, "application-x-bittorrent"),
    (FileCategory::Cad, "application-x-step"),
    (FileCategory::ThreeDimensional, "model-stl"),
    (FileCategory::Unknown, UNKNOWN_ICON),
];

// ── Public API ───────────────────────────────────────────────────────

/// Resolve a file or folder to its bundled Papirus icon.
///
/// See the module docs for the exact priority chain.  The returned
/// `asset_path` is guaranteed to reference an icon in the pinned bundle.
///
/// ## Performance (PAPIRUS-17)
///
/// Results are cached by **normalised** inputs ([`ResolveCacheKey`]): the
/// same file type seen again (e.g. `REPORT.PDF` after `report.pdf`, or a
/// MIME whose case/whitespace/params differ) returns the identical
/// `ResolvedFileIcon` without re-running the priority chain or rebuilding
/// extension candidates.  The cache is bounded ([`RESOLVE_CACHE_MAX_ENTRIES`])
/// and holds only plain data, so it is safe to call from `view()` every
/// frame.
pub fn resolve_file_icon(
    filename: &str,
    advertised_mime_type: Option<&str>,
    locally_detected_mime_type: Option<&str>,
    is_directory: bool,
) -> ResolvedFileIcon {
    let key = resolve_cache_key(
        filename,
        advertised_mime_type,
        locally_detected_mime_type,
        is_directory,
    );
    {
        let cache = resolve_cache().lock().unwrap();
        if let Some(hit) = cache.get(&key) {
            return hit.clone();
        }
    }
    let resolved = resolve_file_icon_uncached(
        filename,
        advertised_mime_type,
        locally_detected_mime_type,
        is_directory,
    );
    bounded_resolve_cache_insert(&mut resolve_cache().lock().unwrap(), key, resolved.clone());
    resolved
}

/// The uncached resolution chain (priorities 1–8).  [`resolve_file_icon`]
/// wraps this with a bounded memo cache.
fn resolve_file_icon_uncached(
    filename: &str,
    advertised_mime_type: Option<&str>,
    locally_detected_mime_type: Option<&str>,
    is_directory: bool,
) -> ResolvedFileIcon {
    let catalog = PapirusCatalog::global();

    // Priority 1: explicit directory / folder state.
    if is_directory {
        return build_icon(
            catalog,
            DIRECTORY_ICON,
            FileCategory::Folder,
            IconConfidence::Exact,
            ResolutionSource::Directory,
            None,
        );
    }

    // Detect a strong peer/local MIME conflict up front (local wins).
    let mismatch = detect_mismatch(advertised_mime_type, locally_detected_mime_type);

    // Priorities 2–3: trusted local MIME (detected after download or
    // validated from the local sharing source).
    if let Some(local) = locally_detected_mime_type
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        if let Some((icon_id, category)) = mime_lookup(local) {
            return build_with_fallback(
                catalog,
                filename,
                icon_id,
                category,
                IconConfidence::Exact,
                ResolutionSource::LocalMime,
                mismatch,
            );
        }
        // The local MIME is present but unmappable.  When it conflicts
        // strongly with the peer hint, the locally detected type must
        // still win for the icon — so its broad category outranks the
        // advertised hint (spec: prefer the locally detected type).
        if mismatch.is_some() {
            if let Some(category) = mime_category_hint(local) {
                if category != FileCategory::Unknown {
                    if let Some(icon_id) = category_fallback_icon(category) {
                        return build_with_fallback(
                            catalog,
                            filename,
                            icon_id,
                            category,
                            IconConfidence::Weak,
                            ResolutionSource::CategoryFallback,
                            mismatch,
                        );
                    }
                }
            }
        }
        // Otherwise fall through: an un-mappable local MIME that does not
        // strongly conflict may still yield an icon from the peer hint.
    }

    // Priority 4: advertised MIME from a peer (a hint, not truth).
    // Only reached when no trusted local MIME resolved (or the local MIME
    // did not strongly conflict and could not be mapped).
    if let Some(advertised) = advertised_mime_type
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        if let Some((icon_id, category)) = mime_lookup(advertised) {
            return build_with_fallback(
                catalog,
                filename,
                icon_id,
                category,
                IconConfidence::Medium,
                ResolutionSource::AdvertisedMime,
                mismatch,
            );
        }
    }

    // Priority 5: compound filename extension.
    if let Some((icon_id, category)) = compound_extension_lookup(filename) {
        return build_with_fallback(
            catalog,
            filename,
            icon_id,
            category,
            IconConfidence::Strong,
            ResolutionSource::CompoundExtension,
            mismatch,
        );
    }

    // Priority 6: ordinary filename extension.
    if let Some((icon_id, category)) = extension_lookup(filename) {
        return build_with_fallback(
            catalog,
            filename,
            icon_id,
            category,
            IconConfidence::Medium,
            ResolutionSource::Extension,
            mismatch,
        );
    }

    // Priority 7: broad category fallback.  Use the best MIME signal we
    // have (local first, then advertised) to derive a category even when
    // no exact icon mapping exists.
    let category_hint = locally_detected_mime_type
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .and_then(mime_category_hint)
        .or_else(|| {
            advertised_mime_type
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .and_then(mime_category_hint)
        });

    if let Some(category) = category_hint {
        if category != FileCategory::Unknown {
            if let Some(icon_id) = category_fallback_icon(category) {
                return build_with_fallback(
                    catalog,
                    filename,
                    icon_id,
                    category,
                    IconConfidence::Weak,
                    ResolutionSource::CategoryFallback,
                    mismatch,
                );
            }
        }
    }

    // Priority 8: generic unknown-file fallback.
    build_icon(
        catalog,
        UNKNOWN_ICON,
        FileCategory::Unknown,
        IconConfidence::None,
        ResolutionSource::UnknownFallback,
        mismatch,
    )
}

// ── Lookups ──────────────────────────────────────────────────────────

/// Normalise a MIME string for lookup: strip any `;`-delimited parameters
/// (e.g. `text/plain; charset=utf-8` → `text/plain`), trim surrounding
/// whitespace, and lowercase.
///
/// This is the MIME-side equivalent of the PAPIRUS-06 extension
/// normalisation: a peer-advertised type may carry parameters or stray
/// case/space, and lookup must not fail on those (spec Task 16: normalise
/// MIME input before resolving).
fn normalise_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase()
}

/// Exact MIME → icon lookup (PAPIRUS-08 full mapping).
///
/// `application/octet-stream` is deliberately excluded (PAPIRUS-21): it is
/// the "unknown binary" placeholder, not a concrete type, so it carries no
/// type signal and must never outrank a real filename extension (priority
/// 6).  `mime_category_hint` and `detect_mismatch` build on this, so an
/// octet-stream side yields no category hint and no mismatch record either.
fn mime_lookup(mime: &str) -> Option<(&'static str, FileCategory)> {
    let mime = normalise_mime(mime);
    if mime == MIME_NO_INFO {
        return None;
    }
    MIME_ICONS
        .iter()
        .find(|(m, _, _)| *m == mime)
        .map(|(_, icon, category)| (*icon, *category))
}

/// Derive a broad category from a MIME top-level type even when no exact
/// icon is mapped (`video/*` → `Video`, `image/*` → `Image`, ...).
fn mime_category_hint(mime: &str) -> Option<FileCategory> {
    let mime = normalise_mime(mime);
    // Exact mapping wins if present.
    if let Some((_, category)) = mime_lookup(&mime) {
        return Some(category);
    }
    let top = mime.split('/').next().unwrap_or("");
    match top {
        "image" => Some(FileCategory::Image),
        "video" => Some(FileCategory::Video),
        "audio" => Some(FileCategory::Audio),
        "text" => Some(FileCategory::Text),
        "font" => Some(FileCategory::Font),
        "model" => Some(FileCategory::ThreeDimensional),
        _ => None,
    }
}

/// Ordinary extension lookup (case-insensitive, trimmed, multi-dot safe).
///
/// Candidates are evaluated longest-first, so an inner dot-segment like
/// `final.pdf` never shadows the real `pdf` suffix; only a known
/// `EXTENSION_ICONS` entry matches.
fn extension_lookup(filename: &str) -> Option<(&'static str, FileCategory)> {
    normalised_extensions(filename).iter().find_map(|ext| {
        EXTENSION_ICONS
            .iter()
            .find(|(e, _, _)| *e == ext)
            .map(|(_, icon, category)| (*icon, *category))
    })
}

/// Compound extension lookup, checked before ordinary extensions.
///
/// `archive.tar.gz` resolves to the tar archive icon (`application-x-tar`)
/// rather than the generic gzip icon because the `tar.gz` candidate is
/// checked first; a bare `file.gz` still resolves to `application-gzip`.
fn compound_extension_lookup(filename: &str) -> Option<(&'static str, FileCategory)> {
    normalised_extensions(filename).iter().find_map(|ext| {
        COMPOUND_EXTENSIONS
            .iter()
            .find(|(suffix, _, _)| *suffix == ext)
            .map(|(_, icon, category)| (*icon, *category))
    })
}

/// Normalise a filename into candidate extension suffixes, longest
/// (compound) first.
///
/// Normalisation rules (PAPIRUS-06):
/// - surrounding whitespace is trimmed,
/// - directory components are stripped,
/// - leading dots (hidden files) do not start an extension,
/// - trailing dots are ignored,
/// - comparison is case-insensitive (candidates are lowercased).
///
/// Examples:
/// - `"report.pdf"` → `["pdf"]`
/// - `"archive.tar.gz"` → `["tar.gz", "gz"]`
/// - `"report.final.PDF"` → `["final.pdf", "pdf"]`
/// - `".gitignore"` → `[]`
/// - `"README"` → `[]`
fn normalised_extensions(filename: &str) -> Vec<String> {
    let name = filename.trim();
    if name.is_empty() || name == "." || name == ".." {
        return Vec::new();
    }
    // Strip any directory components.
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    // Hidden files: a leading dot (or dots) does not start an extension.
    let base = base.trim_start_matches('.');
    // Trailing dots are not part of an extension.
    let base = base.trim_end_matches('.');
    if base.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for (idx, ch) in base.char_indices() {
        if ch == '.' && idx + 1 < base.len() {
            candidates.push(base[idx + 1..].to_ascii_lowercase());
        }
    }
    candidates
}

/// Broad category fallback icon for a category.
fn category_fallback_icon(category: FileCategory) -> Option<&'static str> {
    CATEGORY_FALLBACK_ICONS
        .iter()
        .find(|(c, _)| *c == category)
        .map(|(_, icon)| *icon)
}

/// Compare peer-advertised and locally detected MIME types; when they map
/// to different categories, prefer the local type for the icon and record
/// the mismatch through the existing `tracing` diagnostics channel.
fn detect_mismatch(
    advertised_mime_type: Option<&str>,
    locally_detected_mime_type: Option<&str>,
) -> Option<MimeMismatch> {
    let advertised = advertised_mime_type
        .map(str::trim)
        .filter(|m| !m.is_empty())?;
    let local = locally_detected_mime_type
        .map(str::trim)
        .filter(|m| !m.is_empty())?;
    let advertised_category = mime_category_hint(advertised)?;
    let local_category = mime_category_hint(local)?;
    if advertised_category == local_category {
        // Same broad category is not a strong conflict (e.g. png vs jpeg).
        return None;
    }
    let mismatch = MimeMismatch {
        advertised: advertised.to_string(),
        locally_detected: local.to_string(),
        advertised_category,
        locally_detected_category: local_category,
    };
    tracing::warn!(
        advertised = %advertised,
        locally_detected = %local,
        advertised_category = ?advertised_category,
        locally_detected_category = ?local_category,
        "file-type MIME mismatch: peer-advertised type conflicts with locally detected type; local type wins for the icon"
    );
    Some(mismatch)
}

// ── Icon assembly with the never-missing fallback chain ──────────────

/// Build a `ResolvedFileIcon` for the given preferred icon, walking the
/// guaranteed fallback chain when a candidate is missing from the bundle:
/// exact icon → related extension icon → broad category icon → unknown
/// generic icon.
fn build_with_fallback(
    catalog: &'static PapirusCatalog,
    filename: &str,
    preferred_icon: &str,
    category: FileCategory,
    confidence: IconConfidence,
    source: ResolutionSource,
    mismatch: Option<MimeMismatch>,
) -> ResolvedFileIcon {
    if catalog.has_icon(preferred_icon) {
        return build_icon(
            catalog,
            preferred_icon,
            category,
            confidence,
            source,
            mismatch,
        );
    }
    // Related extension-specific icon (compound first, then ordinary).
    if let Some((icon, _)) =
        compound_extension_lookup(filename).or_else(|| extension_lookup(filename))
    {
        if catalog.has_icon(icon) {
            return build_icon(catalog, icon, category, confidence, source, mismatch);
        }
    }
    // Broad category icon.
    if let Some(icon) = category_fallback_icon(category) {
        if catalog.has_icon(icon) {
            return build_icon(catalog, icon, category, confidence, source, mismatch);
        }
    }
    // Terminal unknown icon (guaranteed by the manifest's required_fallbacks).
    build_icon(
        catalog,
        UNKNOWN_ICON,
        FileCategory::Unknown,
        IconConfidence::None,
        ResolutionSource::UnknownFallback,
        mismatch,
    )
}

/// Assemble the final result for an icon that is guaranteed present.
fn build_icon(
    catalog: &'static PapirusCatalog,
    icon_id: &str,
    category: FileCategory,
    confidence: IconConfidence,
    source: ResolutionSource,
    mismatch: Option<MimeMismatch>,
) -> ResolvedFileIcon {
    debug_assert!(
        catalog.has_icon(icon_id),
        "icon {icon_id} must exist in manifest"
    );

    // Defensive: if the preferred size is somehow absent, fall back to the
    // terminal unknown icon so the returned path is always valid.
    let asset_path = catalog
        .asset_path(icon_id, DEFAULT_ICON_SIZE)
        .or_else(|| catalog.asset_path(UNKNOWN_ICON, DEFAULT_ICON_SIZE))
        .expect("manifest must contain an asset path at the default size");

    ResolvedFileIcon {
        icon_id: icon_id.to_string(),
        asset_path,
        file_category: category,
        display_label: category.display_label().to_string(),
        confidence,
        source,
        mime_mismatch: mismatch,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(filename: &str) -> ResolvedFileIcon {
        resolve_file_icon(filename, None, None, false)
    }

    // ── Priority chain ─────────────────────────────────────────────

    #[test]
    fn extension_only_resolves_icon_and_category() {
        let icon = resolve("report.pdf");
        assert_eq!(icon.icon_id, "application-pdf");
        assert_eq!(icon.file_category, FileCategory::Pdf);
        assert_eq!(icon.source, ResolutionSource::Extension);
        assert_eq!(icon.confidence, IconConfidence::Medium);
        assert!(icon.asset_path.ends_with("32/application-pdf.svg"));
        assert_eq!(icon.display_label, "PDF document");
        assert!(icon.mime_mismatch.is_none());
    }

    #[test]
    fn mime_only_resolves_icon_and_category() {
        let icon = resolve_file_icon("download", Some("application/pdf"), None, false);
        assert_eq!(icon.icon_id, "application-pdf");
        assert_eq!(icon.file_category, FileCategory::Pdf);
        assert_eq!(icon.source, ResolutionSource::AdvertisedMime);
    }

    #[test]
    fn mime_and_extension_agree_use_mime() {
        // Local detection agrees with the extension.
        let icon = resolve_file_icon("photo.png", Some("image/png"), Some("image/png"), false);
        assert_eq!(icon.icon_id, "image-png");
        assert_eq!(icon.source, ResolutionSource::LocalMime);
        assert_eq!(icon.confidence, IconConfidence::Exact);
        assert!(icon.mime_mismatch.is_none());
    }

    #[test]
    fn mime_and_extension_conflict_prefers_local() {
        // Peer advertises image/png but local detection says video/mp4:
        // local wins for the icon and the mismatch is recorded.
        let icon = resolve_file_icon("photo.png", Some("image/png"), Some("video/mp4"), false);
        assert_eq!(icon.icon_id, "video-mp4");
        assert_eq!(icon.file_category, FileCategory::Video);
        assert_eq!(icon.source, ResolutionSource::LocalMime);
        let mismatch = icon.mime_mismatch.expect("mismatch must be recorded");
        assert_eq!(mismatch.advertised, "image/png");
        assert_eq!(mismatch.locally_detected, "video/mp4");
        assert_eq!(mismatch.advertised_category, FileCategory::Image);
        assert_eq!(mismatch.locally_detected_category, FileCategory::Video);
    }

    #[test]
    fn strong_conflict_prefers_local_category_when_local_mime_unmappable() {
        // Peer advertises image/png (exact icon available) but the local
        // MIME is a video subtype with no exact icon mapping.  The locally
        // detected type still wins: its broad category outranks the
        // advertised hint, and the mismatch is recorded.
        let icon = resolve_file_icon(
            "clip.bin",
            Some("image/png"),
            Some("video/x-unknown-codec"),
            false,
        );
        assert_eq!(icon.icon_id, "video-x-generic");
        assert_eq!(icon.file_category, FileCategory::Video);
        assert_eq!(icon.source, ResolutionSource::CategoryFallback);
        let mismatch = icon.mime_mismatch.expect("mismatch must be recorded");
        assert_eq!(mismatch.advertised_category, FileCategory::Image);
        assert_eq!(mismatch.locally_detected_category, FileCategory::Video);
    }

    #[test]
    fn folder_state_wins_over_everything() {
        let icon = resolve_file_icon("anything.png", Some("image/png"), None, true);
        assert_eq!(icon.icon_id, DIRECTORY_ICON);
        assert_eq!(icon.file_category, FileCategory::Folder);
        assert_eq!(icon.source, ResolutionSource::Directory);
        assert_eq!(icon.confidence, IconConfidence::Exact);
    }

    #[test]
    fn unknown_file_gets_generic_fallback() {
        let icon = resolve("mystery.zzz");
        assert_eq!(icon.icon_id, UNKNOWN_ICON);
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert_eq!(icon.confidence, IconConfidence::None);
    }

    // ── Compound extensions ────────────────────────────────────────

    #[test]
    fn compound_extension_beats_ordinary() {
        let icon = resolve("archive.tar.gz");
        assert_eq!(icon.icon_id, "application-x-tar");
        assert_eq!(icon.source, ResolutionSource::CompoundExtension);
        assert_eq!(icon.confidence, IconConfidence::Strong);

        let icon = resolve("definitions.d.ts");
        assert_eq!(icon.icon_id, "text-javascript");
        assert_eq!(icon.source, ResolutionSource::CompoundExtension);
    }

    #[test]
    fn ordinary_extension_used_when_no_compound_matches() {
        let icon = resolve("photo.png");
        assert_eq!(icon.icon_id, "image-png");
        assert_eq!(icon.source, ResolutionSource::Extension);
    }

    // ── Extension normalisation basics ─────────────────────────────

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(resolve("REPORT.PDF").icon_id, "application-pdf");
        assert_eq!(resolve("Report.Pdf").icon_id, "application-pdf");
    }

    #[test]
    fn leading_dot_filenames_are_safe() {
        let icon = resolve(".gitignore");
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert_eq!(icon.file_category, FileCategory::Unknown);
    }

    #[test]
    fn extensionless_files_still_get_a_fallback() {
        let icon = resolve("README");
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert_eq!(icon.file_category, FileCategory::Unknown);
    }

    // ── PAPIRUS-06: compound + case-insensitive extensions ────────

    #[test]
    fn uppercase_and_lowercase_extensions_resolve_identically() {
        let lower = resolve("report.pdf");
        let upper = resolve("REPORT.PDF");
        let mixed = resolve("Report.PdF");
        for other in [&upper, &mixed] {
            assert_eq!(other.icon_id, lower.icon_id);
            assert_eq!(other.asset_path, lower.asset_path);
            assert_eq!(other.file_category, lower.file_category);
            assert_eq!(other.source, lower.source);
            assert_eq!(other.confidence, lower.confidence);
            assert_eq!(other.display_label, lower.display_label);
        }
        assert_eq!(upper.icon_id, "application-pdf");
        assert_eq!(upper.source, ResolutionSource::Extension);
    }

    #[test]
    fn uppercase_compound_extensions_resolve_identically() {
        let lower = resolve("archive.tar.gz");
        let upper = resolve("ARCHIVE.TAR.GZ");
        let mixed = resolve("Archive.Tar.Gz");
        for other in [&upper, &mixed] {
            assert_eq!(other.icon_id, lower.icon_id);
            assert_eq!(other.source, lower.source);
            assert_eq!(other.confidence, lower.confidence);
        }
        assert_eq!(upper.icon_id, "application-x-tar");
        assert_eq!(upper.source, ResolutionSource::CompoundExtension);
    }

    #[test]
    fn leading_and_trailing_whitespace_does_not_affect_lookup() {
        let plain = resolve("report.pdf");
        for name in [
            "  report.pdf",
            "report.pdf  ",
            "  report.pdf  ",
            "\treport.pdf\n",
        ] {
            let icon = resolve(name);
            assert_eq!(icon.icon_id, plain.icon_id, "for {name:?}");
            assert_eq!(icon.source, plain.source, "for {name:?}");
        }

        let compound = resolve("archive.tar.gz");
        for name in ["  archive.tar.gz", "archive.tar.gz  ", "  archive.tar.gz  "] {
            let icon = resolve(name);
            assert_eq!(icon.icon_id, compound.icon_id, "for {name:?}");
            assert_eq!(icon.source, compound.source, "for {name:?}");
        }
    }

    #[test]
    fn whitespace_only_filename_falls_back() {
        let icon = resolve("   ");
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert_eq!(icon.file_category, FileCategory::Unknown);
    }

    #[test]
    fn hidden_files_are_handled_safely() {
        // A hidden file with no further extension has no extension.
        let icon = resolve(".gitignore");
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert_eq!(icon.file_category, FileCategory::Unknown);

        // A hidden file with a real extension still resolves.
        let icon = resolve(".profile.pdf");
        assert_eq!(icon.icon_id, "application-pdf");
        assert_eq!(icon.source, ResolutionSource::Extension);

        // A hidden multi-dot file whose extension is unknown falls back.
        let icon = resolve(".env.local");
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert_eq!(icon.file_category, FileCategory::Unknown);

        // Bare "." / ".." never panic and fall back.
        for name in [".", ".."] {
            let icon = resolve(name);
            assert_eq!(
                icon.source,
                ResolutionSource::UnknownFallback,
                "for {name:?}"
            );
        }
    }

    #[test]
    fn multi_dot_filenames_do_not_break_resolution() {
        // Ordinary extension uses the final dot segment.
        let icon = resolve("report.final.pdf");
        assert_eq!(icon.icon_id, "application-pdf");
        assert_eq!(icon.source, ResolutionSource::Extension);

        // Compound extension still wins over the trailing single segment.
        let icon = resolve("backup.2026.tar.gz");
        assert_eq!(icon.icon_id, "application-x-tar");
        assert_eq!(icon.source, ResolutionSource::CompoundExtension);

        // Unknown multi-dot names still fall back safely.
        let icon = resolve("a.b.c.zzz");
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
    }

    #[test]
    fn compound_extensions_resolve_before_ordinary() {
        // tar.gz must resolve as a tar archive, NOT a generic .gz.
        let icon = resolve("archive.tar.gz");
        assert_eq!(icon.icon_id, "application-x-tar");
        assert_eq!(icon.file_category, FileCategory::Archive);
        assert_eq!(icon.source, ResolutionSource::CompoundExtension);
        assert_eq!(icon.confidence, IconConfidence::Strong);
        assert_ne!(icon.icon_id, "application-gzip");

        // A bare .gz still resolves as gzip via the ordinary path.
        let icon = resolve("file.gz");
        assert_eq!(icon.icon_id, "application-gzip");
        assert_eq!(icon.source, ResolutionSource::Extension);
    }

    #[test]
    fn all_supported_compound_extensions_resolve() {
        let cases = [
            ("archive.tar.gz", "application-x-tar", FileCategory::Archive),
            (
                "archive.tar.bz2",
                "application-x-tar",
                FileCategory::Archive,
            ),
            (
                "archive.tar.xz",
                "application-x-xz-compressed-tar",
                FileCategory::Archive,
            ),
            ("archive.tar.zst", "application-zstd", FileCategory::Archive),
            (
                "userscript.user.js",
                "application-javascript",
                FileCategory::SourceCode,
            ),
            (
                "definitions.d.ts",
                "text-javascript",
                FileCategory::SourceCode,
            ),
            (
                "bundle.min.js",
                "application-javascript",
                FileCategory::SourceCode,
            ),
            ("site.min.css", "text-css", FileCategory::SourceCode),
        ];
        for (name, icon_id, category) in cases {
            let icon = resolve(name);
            assert_eq!(icon.icon_id, icon_id, "for {name:?}");
            assert_eq!(icon.file_category, category, "for {name:?}");
            assert_eq!(
                icon.source,
                ResolutionSource::CompoundExtension,
                "for {name:?}"
            );
            assert_eq!(icon.confidence, IconConfidence::Strong, "for {name:?}");
        }
    }

    #[test]
    fn archive_extensions_resolve_to_specific_icons() {
        let cases = [
            ("file.zip", "application-zip", FileCategory::Archive),
            (
                "file.7z",
                "application-x-7z-compressed",
                FileCategory::Archive,
            ),
            ("file.rar", "application-vnd.rar", FileCategory::Archive),
            ("file.tar", "application-x-tar", FileCategory::Archive),
            ("file.gz", "application-gzip", FileCategory::Archive),
            ("file.bz2", "application-x-bzip2", FileCategory::Archive),
            (
                "file.xz",
                "application-x-xz-compressed-tar",
                FileCategory::Archive,
            ),
            ("file.zst", "application-zstd", FileCategory::Archive),
        ];
        for (name, icon_id, category) in cases {
            let icon = resolve(name);
            assert_eq!(icon.icon_id, icon_id, "for {name:?}");
            assert_eq!(icon.file_category, category, "for {name:?}");
            assert_eq!(icon.source, ResolutionSource::Extension, "for {name:?}");
        }
    }

    #[test]
    fn directory_components_do_not_affect_extension_lookup() {
        let icon = resolve("downloads/archive.tar.gz");
        assert_eq!(icon.icon_id, "application-x-tar");
        assert_eq!(icon.source, ResolutionSource::CompoundExtension);

        let icon = resolve("path/to/file.pdf");
        assert_eq!(icon.icon_id, "application-pdf");
        assert_eq!(icon.source, ResolutionSource::Extension);
    }

    // ── Category fallback (priority 7) ─────────────────────────────

    #[test]
    fn broad_category_fallback_uses_mime_top_level() {
        // "video/x-unknown-codec" has no exact icon but is a video.
        let icon = resolve_file_icon("clip", Some("video/x-unknown-codec"), None, false);
        assert_eq!(icon.icon_id, "video-x-generic");
        assert_eq!(icon.file_category, FileCategory::Video);
        assert_eq!(icon.source, ResolutionSource::CategoryFallback);
        assert_eq!(icon.confidence, IconConfidence::Weak);
    }

    // ── Never-missing asset guarantees ─────────────────────────────

    #[test]
    fn every_seed_icon_exists_in_the_pinned_manifest() {
        let catalog = PapirusCatalog::global();
        for (_, icon, _) in EXTENSION_ICONS {
            assert!(catalog.has_icon(icon), "missing extension icon {icon}");
        }
        for (_, icon, _) in COMPOUND_EXTENSIONS {
            assert!(catalog.has_icon(icon), "missing compound icon {icon}");
        }
        for (_, icon, _) in MIME_ICONS {
            assert!(catalog.has_icon(icon), "missing MIME icon {icon}");
        }
        for (_, icon) in CATEGORY_FALLBACK_ICONS {
            assert!(catalog.has_icon(icon), "missing category icon {icon}");
        }
        assert!(catalog.has_icon(DIRECTORY_ICON));
        assert!(catalog.has_icon(UNKNOWN_ICON));
    }

    #[test]
    fn unknown_icon_is_a_required_fallback_in_the_manifest() {
        let catalog = PapirusCatalog::global();
        assert!(
            catalog.required_fallbacks.iter().any(|f| f == UNKNOWN_ICON),
            "{UNKNOWN_ICON} must be listed in required_fallbacks"
        );
    }

    #[test]
    fn every_resolved_icon_has_an_existing_asset_path() {
        let catalog = PapirusCatalog::global();
        let cases = [
            "report.pdf",
            "photo.PNG",
            "archive.tar.gz",
            "song.mp3",
            "movie.mp4",
            "script.rs",
            "sheet.xlsx",
            ".hidden",
            "noext",
            "weird.zzz",
        ];
        for name in cases {
            let icon = resolve(name);
            assert!(
                catalog.has_icon(&icon.icon_id),
                "resolved icon {} for {name} must exist in manifest",
                icon.icon_id
            );
            assert!(
                icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT),
                "asset path must be repo-relative: {}",
                icon.asset_path
            );
            assert!(
                icon.asset_path.ends_with(".svg"),
                "asset path must point at an SVG: {}",
                icon.asset_path
            );
        }
    }

    #[test]
    fn fallback_chain_never_yields_a_missing_asset_for_unknown_mime() {
        // A MIME with no icon mapping and no extension must still resolve
        // to an existing asset.
        let catalog = PapirusCatalog::global();
        let icon = resolve_file_icon(
            "blob",
            Some("application/x-completely-unknown"),
            None,
            false,
        );
        assert!(catalog.has_icon(&icon.icon_id));
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert!(matches!(
            icon.source,
            ResolutionSource::UnknownFallback | ResolutionSource::CategoryFallback
        ));
    }

    // ── PAPIRUS-08: MIME → Papirus icon mapping ────────────────────

    #[test]
    fn known_mime_resolves_exact_icon() {
        let cases: &[(&str, &str, FileCategory)] = &[
            ("application/pdf", "application-pdf", FileCategory::Pdf),
            ("video/mp4", "video-mp4", FileCategory::Video),
            ("image/png", "image-png", FileCategory::Image),
            ("audio/mpeg", "audio-mpeg", FileCategory::Audio),
            ("text/plain", "text-plain", FileCategory::Text),
            (
                "text/tab-separated-values",
                "text-tab-separated-values",
                FileCategory::Spreadsheet,
            ),
            ("text/x-rust", "text-x-rust", FileCategory::SourceCode),
        ];
        // NOTE: `application/octet-stream` is intentionally NOT in this
        // exact-MIME list — PAPIRUS-21 treats octet-stream as "no MIME
        // info" (it never wins at priority 4); see the PAPIRUS-21 section
        // below for its dedicated scenarios.
        for (mime, icon_id, category) in cases {
            let icon = resolve_file_icon("download.bin", Some(mime), None, false);
            assert_eq!(&icon.icon_id, icon_id, "for MIME {mime}");
            assert_eq!(icon.file_category, *category, "for MIME {mime}");
            assert_eq!(
                icon.source,
                ResolutionSource::AdvertisedMime,
                "for MIME {mime}"
            );
            assert_eq!(icon.confidence, IconConfidence::Medium, "for MIME {mime}");
            assert!(
                PapirusCatalog::global().has_icon(&icon.icon_id),
                "icon {} must exist for MIME {mime}",
                icon.icon_id
            );
        }
    }

    #[test]
    fn mime_without_exact_icon_falls_back_to_existing_generic() {
        // image/webp has no image-webp.svg in the pinned bundle; the exact
        // MIME icon is absent so the resolver maps to the generic `image`
        // icon that does exist (spec Task 8 fallback chain).
        let icon = resolve_file_icon("photo.webp", Some("image/webp"), None, false);
        assert_eq!(icon.icon_id, "image");
        assert_eq!(icon.file_category, FileCategory::Image);
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));

        // video/quicktime has no exact icon; fall back to generic `video`.
        let icon = resolve_file_icon("movie.mov", Some("video/quicktime"), None, false);
        assert_eq!(icon.icon_id, "video");
        assert_eq!(icon.file_category, FileCategory::Video);
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));

        // audio/aac has no exact icon; fall back to generic audio.
        let icon = resolve_file_icon("track.aac", Some("audio/aac"), None, false);
        assert_eq!(icon.icon_id, "audio-x-generic");
        assert_eq!(icon.file_category, FileCategory::Audio);
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));
    }

    #[test]
    fn mime_with_fallback_to_related_extension_icon() {
        // video/x-m4v has no dedicated Papirus icon; the mapping prefers the
        // related video-mp4 icon (M4V is an MP4-family container).
        let icon = resolve_file_icon("clip.m4v", Some("video/x-m4v"), None, false);
        assert_eq!(icon.icon_id, "video-mp4");
        assert_eq!(icon.file_category, FileCategory::Video);

        // audio/mp4 (M4A container) uses the existing audio-x-m4a icon.
        let icon = resolve_file_icon("song.m4a", Some("audio/mp4"), None, false);
        assert_eq!(icon.icon_id, "audio-x-m4a");
        assert_eq!(icon.file_category, FileCategory::Audio);
    }

    #[test]
    fn unknown_mime_falls_back_to_unknown_generic() {
        let icon = resolve_file_icon("blob", Some("application/x-totally-unknown"), None, false);
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert!(matches!(
            icon.source,
            ResolutionSource::UnknownFallback | ResolutionSource::CategoryFallback
        ));
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));

        // A MIME with no slash at all is not a valid MIME and must fall
        // through to the unknown generic without panicking.
        let icon = resolve_file_icon("blob", Some("not-a-mime"), None, false);
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));
    }

    #[test]
    fn malformed_mime_strings_fall_through_safely() {
        let malformed: &[&str] = &[
            "",
            "   ",
            "/",
            "video/",
            "application/",
            "text/plain; charset=utf-8", // parameter suffix (valid but must not break lookup)
            "APPLICATION/PDF",           // uppercase
            " application/pdf ",         // surrounding whitespace
        ];
        for mime in malformed {
            // Must never panic and must always resolve to an existing asset.
            let icon = resolve_file_icon("blob.bin", Some(mime), None, false);
            assert!(
                PapirusCatalog::global().has_icon(&icon.icon_id),
                "malformed MIME {mime:?} resolved to missing icon {}",
                icon.icon_id
            );
        }

        // MIME parameters are stripped before lookup (Task 16: normalise
        // MIME input): text/plain; charset=utf-8 matches text-plain.
        let icon = resolve_file_icon("readme", Some("text/plain; charset=utf-8"), None, false);
        assert_eq!(icon.icon_id, "text-plain");

        // Uppercase and whitespace MIME strings match case-insensitively.
        let icon = resolve_file_icon("doc", Some("APPLICATION/PDF"), None, false);
        assert_eq!(icon.icon_id, "application-pdf");
        let icon = resolve_file_icon("doc", Some(" application/pdf "), None, false);
        assert_eq!(icon.icon_id, "application-pdf");
    }

    // ── PAPIRUS-09: Task 9 required file-type coverage ─────────────

    /// Spec-mandated examples (Task 19 "Required examples") must resolve
    /// to the exact icon/category, and the icon must exist in the pinned
    /// bundle with a real asset path.
    #[test]
    fn task9_required_examples_resolve_to_real_icons() {
        let catalog = PapirusCatalog::global();
        let cases: &[(&str, &str, FileCategory)] = &[
            ("report.pdf", "application-pdf", FileCategory::Pdf),
            (
                "document.docx",
                "application-vnd.openxmlformats-officedocument.wordprocessingml.document",
                FileCategory::Document,
            ),
            (
                "budget.xlsx",
                "application-vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                FileCategory::Spreadsheet,
            ),
            (
                "slides.pptx",
                "application-vnd.openxmlformats-officedocument.presentationml.presentation",
                FileCategory::Presentation,
            ),
            ("readme.md", "text-markdown", FileCategory::Markdown),
            ("main.rs", "text-rust", FileCategory::SourceCode),
            ("photo.png", "image-png", FileCategory::Image),
            ("animation.gif", "image-gif", FileCategory::Image),
            ("video.mp4", "video-mp4", FileCategory::Video),
            ("movie.mkv", "video-x-matroska", FileCategory::Video),
            ("music.flac", "audio-flac", FileCategory::Audio),
            ("archive.tar.gz", "application-x-tar", FileCategory::Archive),
            (
                "package.7z",
                "application-x-7z-compressed",
                FileCategory::Archive,
            ),
            (
                "database.sqlite",
                "application-x-sqlite3",
                FileCategory::Database,
            ),
            ("font.ttf", "application-x-font-ttf", FileCategory::Font),
            (
                "certificate.pem",
                "application-x-pem-key",
                FileCategory::Key,
            ),
            ("unknownfile", UNKNOWN_ICON, FileCategory::Unknown),
        ];
        for (name, icon_id, category) in cases {
            let icon = resolve(name);
            assert_eq!(&icon.icon_id, icon_id, "for {name:?}");
            assert_eq!(icon.file_category, *category, "for {name:?}");
            assert!(
                catalog.has_icon(&icon.icon_id),
                "resolved icon {} for {name:?} must exist in manifest",
                icon.icon_id
            );
            assert!(
                icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT),
                "asset path must be repo-relative: {}",
                icon.asset_path
            );
            assert!(
                icon.asset_path.ends_with(".svg"),
                "asset path must point at an SVG: {}",
                icon.asset_path
            );
        }

        // shared-folder is an explicit directory → folder icon, never a
        // filename-derived fallback.
        let folder = resolve_file_icon("shared-folder", None, None, true);
        assert_eq!(folder.icon_id, DIRECTORY_ICON);
        assert_eq!(folder.file_category, FileCategory::Folder);
        assert_eq!(folder.source, ResolutionSource::Directory);
        assert!(catalog.has_icon(&folder.icon_id));
    }

    /// Full Task 9 coverage table (extension → category).  Every required
    /// type must resolve by extension alone to the correct category with an
    /// icon that exists in the pinned bundle.
    #[test]
    fn task9_full_extension_coverage_table() {
        let catalog = PapirusCatalog::global();
        let cases: &[(&str, FileCategory)] = &[
            // Documents
            ("pdf", FileCategory::Pdf),
            ("doc", FileCategory::Document),
            ("docx", FileCategory::Document),
            ("odt", FileCategory::Document),
            ("rtf", FileCategory::Document),
            ("epub", FileCategory::Ebook),
            ("txt", FileCategory::Text),
            ("md", FileCategory::Markdown),
            ("markdown", FileCategory::Markdown),
            ("log", FileCategory::Text),
            // Spreadsheets
            ("xls", FileCategory::Spreadsheet),
            ("xlsx", FileCategory::Spreadsheet),
            ("ods", FileCategory::Spreadsheet),
            ("csv", FileCategory::Spreadsheet),
            ("tsv", FileCategory::Spreadsheet),
            // Presentations
            ("ppt", FileCategory::Presentation),
            ("pptx", FileCategory::Presentation),
            ("odp", FileCategory::Presentation),
            // Images
            ("jpg", FileCategory::Image),
            ("jpeg", FileCategory::Image),
            ("png", FileCategory::Image),
            ("gif", FileCategory::Image),
            ("webp", FileCategory::Image),
            ("svg", FileCategory::Image),
            ("bmp", FileCategory::Image),
            ("tiff", FileCategory::Image),
            ("tif", FileCategory::Image),
            ("heic", FileCategory::Image),
            ("heif", FileCategory::Image),
            ("dng", FileCategory::Image),
            ("cr2", FileCategory::Image),
            ("nef", FileCategory::Image),
            ("arw", FileCategory::Image),
            ("raw", FileCategory::Image),
            // Video
            ("mp4", FileCategory::Video),
            ("mkv", FileCategory::Video),
            ("webm", FileCategory::Video),
            ("mov", FileCategory::Video),
            ("avi", FileCategory::Video),
            ("mpeg", FileCategory::Video),
            ("mpg", FileCategory::Video),
            ("m4v", FileCategory::Video),
            ("ogv", FileCategory::Video),
            // Audio
            ("mp3", FileCategory::Audio),
            ("flac", FileCategory::Audio),
            ("wav", FileCategory::Audio),
            ("ogg", FileCategory::Audio),
            ("opus", FileCategory::Audio),
            ("m4a", FileCategory::Audio),
            ("aac", FileCategory::Audio),
            ("wma", FileCategory::Audio),
            // Archives
            ("zip", FileCategory::Archive),
            ("7z", FileCategory::Archive),
            ("rar", FileCategory::Archive),
            ("tar", FileCategory::Archive),
            ("gz", FileCategory::Archive),
            ("bz2", FileCategory::Archive),
            ("xz", FileCategory::Archive),
            ("zst", FileCategory::Archive),
            // Source code
            ("rs", FileCategory::SourceCode),
            ("js", FileCategory::SourceCode),
            ("ts", FileCategory::SourceCode),
            ("py", FileCategory::SourceCode),
            ("java", FileCategory::SourceCode),
            ("kt", FileCategory::SourceCode),
            ("kts", FileCategory::SourceCode),
            ("c", FileCategory::SourceCode),
            ("h", FileCategory::SourceCode),
            ("cpp", FileCategory::SourceCode),
            ("cc", FileCategory::SourceCode),
            ("cxx", FileCategory::SourceCode),
            ("hpp", FileCategory::SourceCode),
            ("cs", FileCategory::SourceCode),
            ("go", FileCategory::SourceCode),
            ("html", FileCategory::SourceCode),
            ("css", FileCategory::SourceCode),
            ("json", FileCategory::SourceCode),
            ("xml", FileCategory::SourceCode),
            ("yaml", FileCategory::SourceCode),
            ("yml", FileCategory::SourceCode),
            ("toml", FileCategory::SourceCode),
            ("sh", FileCategory::SourceCode),
            ("bash", FileCategory::SourceCode),
            ("zsh", FileCategory::SourceCode),
            ("fish", FileCategory::SourceCode),
            ("ps1", FileCategory::SourceCode),
            ("sql", FileCategory::SourceCode),
            // Executables
            ("exe", FileCategory::Executable),
            ("elf", FileCategory::Executable),
            ("so", FileCategory::Executable),
            ("dll", FileCategory::Executable),
            ("dylib", FileCategory::Executable),
            ("bat", FileCategory::Executable),
            ("cmd", FileCategory::Executable),
            // Installers / packages
            ("deb", FileCategory::Installer),
            ("rpm", FileCategory::Installer),
            ("apk", FileCategory::Installer),
            ("msi", FileCategory::Installer),
            ("pkg", FileCategory::Installer),
            ("appimage", FileCategory::Installer),
            ("flatpak", FileCategory::Installer),
            ("snap", FileCategory::Installer),
            // Disk images
            ("iso", FileCategory::DiskImage),
            ("img", FileCategory::DiskImage),
            ("dmg", FileCategory::DiskImage),
            ("vhd", FileCategory::DiskImage),
            ("vmdk", FileCategory::DiskImage),
            ("vdi", FileCategory::DiskImage),
            ("qcow2", FileCategory::DiskImage),
            // Databases
            ("sqlite", FileCategory::Database),
            ("sqlite3", FileCategory::Database),
            ("db", FileCategory::Database),
            ("dbf", FileCategory::Database),
            // Fonts
            ("ttf", FileCategory::Font),
            ("otf", FileCategory::Font),
            ("ttc", FileCategory::Font),
            ("woff", FileCategory::Font),
            ("woff2", FileCategory::Font),
            // Certificates
            ("crt", FileCategory::Certificate),
            ("cer", FileCategory::Certificate),
            ("der", FileCategory::Certificate),
            ("p12", FileCategory::Certificate),
            ("pfx", FileCategory::Certificate),
            // Keys
            ("pem", FileCategory::Key),
            ("key", FileCategory::Key),
            ("pub", FileCategory::Key),
            ("asc", FileCategory::Key),
            ("gpg", FileCategory::Key),
            ("ppk", FileCategory::Key),
            // Ebooks / torrents / CAD / 3D
            ("mobi", FileCategory::Ebook),
            ("torrent", FileCategory::Torrent),
            ("step", FileCategory::Cad),
            ("stp", FileCategory::Cad),
            ("dwg", FileCategory::Cad),
            ("dxf", FileCategory::Cad),
            ("stl", FileCategory::ThreeDimensional),
            ("obj", FileCategory::ThreeDimensional),
            ("fbx", FileCategory::ThreeDimensional),
            ("glb", FileCategory::ThreeDimensional),
            ("gltf", FileCategory::ThreeDimensional),
            ("blend", FileCategory::ThreeDimensional),
            ("3ds", FileCategory::ThreeDimensional),
            // Unknown binary
            ("bin", FileCategory::Unknown),
        ];
        for (ext, category) in cases {
            let filename = format!("file.{ext}");
            let icon = resolve(&filename);
            assert_eq!(
                icon.file_category, *category,
                "extension {ext:?} resolved to wrong category (icon {})",
                icon.icon_id
            );
            assert!(
                catalog.has_icon(&icon.icon_id),
                "extension {ext:?} resolved to missing icon {}",
                icon.icon_id
            );
            assert!(
                icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT),
                "extension {ext:?} asset path must be repo-relative: {}",
                icon.asset_path
            );
        }
    }

    /// The compound archive extensions added for Task 9 remain archive
    /// types and never fall through to the generic unknown icon.
    #[test]
    fn task9_compound_archives_resolve() {
        for name in [
            "backup.tar.gz",
            "backup.tar.bz2",
            "backup.tar.xz",
            "backup.tar.zst",
        ] {
            let icon = resolve(name);
            assert_eq!(icon.file_category, FileCategory::Archive, "for {name:?}");
            assert!(
                PapirusCatalog::global().has_icon(&icon.icon_id),
                "for {name:?}"
            );
            assert_ne!(
                icon.source,
                ResolutionSource::UnknownFallback,
                "for {name:?}"
            );
        }
    }

    /// Task 9 MIME coverage completion: the common MIME aliases added in
    /// PAPIRUS-09 resolve to the closest existing icon.
    #[test]
    fn task9_mime_aliases_resolve() {
        let cases: &[(&str, &str, FileCategory)] = &[
            (
                "text/x-typescript",
                "text-javascript",
                FileCategory::SourceCode,
            ),
            ("text/x-markdown", "text-markdown", FileCategory::Markdown),
            (
                "application/x-msdownload",
                "application-x-ms-dos-executable",
                FileCategory::Executable,
            ),
            (
                "application/x-msi",
                "package-x-generic",
                FileCategory::Installer,
            ),
            (
                "application/x-powershell",
                "text-x-script",
                FileCategory::SourceCode,
            ),
            (
                "application/x-gzip",
                "application-x-gzip",
                FileCategory::Archive,
            ),
            (
                "application/x-rar",
                "application-x-rar",
                FileCategory::Archive,
            ),
            (
                "application/x-font-woff",
                "font-x-generic",
                FileCategory::Font,
            ),
            ("font/woff", "font-x-generic", FileCategory::Font),
            ("font/woff2", "font-x-generic", FileCategory::Font),
            (
                "application/x-pkcs12",
                "application-pkix-cert",
                FileCategory::Certificate,
            ),
            (
                "application/pgp-signature",
                "application-pgp-keys",
                FileCategory::Key,
            ),
            (
                "image/x-canon-cr2",
                "image-x-adobe-dng",
                FileCategory::Image,
            ),
            (
                "image/x-nikon-nef",
                "image-x-adobe-dng",
                FileCategory::Image,
            ),
            ("image/x-sony-arw", "image-x-adobe-dng", FileCategory::Image),
            (
                "application/x-vhd",
                "application-x-cd-image",
                FileCategory::DiskImage,
            ),
            (
                "application/x-vmdk",
                "application-x-cd-image",
                FileCategory::DiskImage,
            ),
            (
                "application/x-dbf",
                "application-x-sqlite3",
                FileCategory::Database,
            ),
            ("model/step", "application-x-step", FileCategory::Cad),
        ];
        for (mime, icon_id, category) in cases {
            let icon = resolve_file_icon("download", Some(mime), None, false);
            assert_eq!(&icon.icon_id, icon_id, "for MIME {mime}");
            assert_eq!(icon.file_category, *category, "for MIME {mime}");
            assert_eq!(
                icon.source,
                ResolutionSource::AdvertisedMime,
                "for MIME {mime}"
            );
            assert!(
                PapirusCatalog::global().has_icon(&icon.icon_id),
                "icon {} must exist for MIME {mime}",
                icon.icon_id
            );
        }
    }

    /// Every required Task 9 type must also resolve when the MIME type is
    /// provided (exact MIME icon or closest fallback) — never a missing
    /// asset, never an unknown generic for a known class.
    #[test]
    fn task9_mime_class_coverage_never_missing() {
        let catalog = PapirusCatalog::global();
        let mimes: &[(&str, FileCategory)] = &[
            ("application/pdf", FileCategory::Pdf),
            ("application/msword", FileCategory::Document),
            (
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                FileCategory::Document,
            ),
            (
                "application/vnd.oasis.opendocument.text",
                FileCategory::Document,
            ),
            ("application/rtf", FileCategory::Document),
            ("application/epub+zip", FileCategory::Ebook),
            ("text/plain", FileCategory::Text),
            ("text/markdown", FileCategory::Markdown),
            ("text/x-log", FileCategory::Text),
            ("application/vnd.ms-excel", FileCategory::Spreadsheet),
            (
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                FileCategory::Spreadsheet,
            ),
            (
                "application/vnd.oasis.opendocument.spreadsheet",
                FileCategory::Spreadsheet,
            ),
            ("text/csv", FileCategory::Spreadsheet),
            ("text/tab-separated-values", FileCategory::Spreadsheet),
            ("application/vnd.ms-powerpoint", FileCategory::Presentation),
            (
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                FileCategory::Presentation,
            ),
            (
                "application/vnd.oasis.opendocument.presentation",
                FileCategory::Presentation,
            ),
            ("image/jpeg", FileCategory::Image),
            ("image/png", FileCategory::Image),
            ("image/gif", FileCategory::Image),
            ("image/webp", FileCategory::Image),
            ("image/svg+xml", FileCategory::Image),
            ("image/bmp", FileCategory::Image),
            ("image/tiff", FileCategory::Image),
            ("image/heic", FileCategory::Image),
            ("image/heif", FileCategory::Image),
            ("image/x-adobe-dng", FileCategory::Image),
            ("video/mp4", FileCategory::Video),
            ("video/x-matroska", FileCategory::Video),
            ("video/webm", FileCategory::Video),
            ("video/quicktime", FileCategory::Video),
            ("video/x-msvideo", FileCategory::Video),
            ("video/mpeg", FileCategory::Video),
            ("video/x-m4v", FileCategory::Video),
            ("video/ogg", FileCategory::Video),
            ("audio/mpeg", FileCategory::Audio),
            ("audio/mp3", FileCategory::Audio),
            ("audio/flac", FileCategory::Audio),
            ("audio/x-wav", FileCategory::Audio),
            ("audio/ogg", FileCategory::Audio),
            ("audio/opus", FileCategory::Audio),
            ("audio/x-m4a", FileCategory::Audio),
            ("audio/aac", FileCategory::Audio),
            ("audio/x-ms-wma", FileCategory::Audio),
            ("application/zip", FileCategory::Archive),
            ("application/x-7z-compressed", FileCategory::Archive),
            ("application/vnd.rar", FileCategory::Archive),
            ("application/x-tar", FileCategory::Archive),
            ("application/gzip", FileCategory::Archive),
            ("application/x-bzip2", FileCategory::Archive),
            ("application/x-xz-compressed-tar", FileCategory::Archive),
            ("application/zstd", FileCategory::Archive),
            ("text/x-rust", FileCategory::SourceCode),
            ("application/javascript", FileCategory::SourceCode),
            ("text/javascript", FileCategory::SourceCode),
            ("text/typescript", FileCategory::SourceCode),
            ("text/x-python", FileCategory::SourceCode),
            ("text/x-java", FileCategory::SourceCode),
            ("text/x-kotlin", FileCategory::SourceCode),
            ("text/x-c", FileCategory::SourceCode),
            ("text/x-c++", FileCategory::SourceCode),
            ("text/x-csharp", FileCategory::SourceCode),
            ("text/x-go", FileCategory::SourceCode),
            ("text/html", FileCategory::SourceCode),
            ("text/css", FileCategory::SourceCode),
            ("application/json", FileCategory::SourceCode),
            ("application/xml", FileCategory::SourceCode),
            ("application/yaml", FileCategory::SourceCode),
            ("application/toml", FileCategory::SourceCode),
            ("text/x-shellscript", FileCategory::SourceCode),
            ("text/x-powershell", FileCategory::SourceCode),
            ("application/sql", FileCategory::SourceCode),
            ("application/x-ms-dos-executable", FileCategory::Executable),
            ("application/x-executable", FileCategory::Executable),
            (
                "application/vnd.debian.binary-package",
                FileCategory::Installer,
            ),
            ("application/x-rpm", FileCategory::Installer),
            (
                "application/vnd.android.package-archive",
                FileCategory::Installer,
            ),
            ("application/x-cd-image", FileCategory::DiskImage),
            ("application/x-raw-disk-image", FileCategory::DiskImage),
            ("application/x-apple-diskimage", FileCategory::DiskImage),
            ("application/x-sqlite3", FileCategory::Database),
            ("application/vnd.sqlite3", FileCategory::Database),
            ("application/x-font-ttf", FileCategory::Font),
            ("application/x-font-otf", FileCategory::Font),
            ("application/x-x509-ca-cert", FileCategory::Certificate),
            ("application/pkix-cert", FileCategory::Certificate),
            ("application/x-pem-key", FileCategory::Key),
            ("application/pgp-keys", FileCategory::Key),
            ("application/x-bittorrent", FileCategory::Torrent),
            ("application/x-step", FileCategory::Cad),
            ("model/stl", FileCategory::ThreeDimensional),
            ("model/obj", FileCategory::ThreeDimensional),
            ("model/gltf", FileCategory::ThreeDimensional),
        ];
        for (mime, category) in mimes {
            let icon = resolve_file_icon("download", Some(mime), None, false);
            assert_eq!(
                icon.file_category, *category,
                "MIME {mime} resolved to wrong category (icon {})",
                icon.icon_id
            );
            assert!(
                catalog.has_icon(&icon.icon_id),
                "MIME {mime} resolved to missing icon {}",
                icon.icon_id
            );
            assert!(
                icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT),
                "MIME {mime} asset path must be repo-relative: {}",
                icon.asset_path
            );
        }
    }

    /// Extensionless files and hidden files stay on the unknown generic
    /// path, and unknown binary extensions map to the unknown category —
    /// the resolver never returns a missing asset for them.
    #[test]
    fn task9_unknown_and_extensionless_never_missing() {
        let catalog = PapirusCatalog::global();
        for name in ["README", "Makefile", ".env", "noextension"] {
            let icon = resolve(name);
            assert_eq!(icon.file_category, FileCategory::Unknown, "for {name:?}");
            assert_eq!(
                icon.source,
                ResolutionSource::UnknownFallback,
                "for {name:?}"
            );
            assert!(catalog.has_icon(&icon.icon_id), "for {name:?}");
        }
        // .bin resolves as an unknown binary via the extension path.
        let icon = resolve("firmware.bin");
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert_eq!(icon.source, ResolutionSource::Extension);
        assert!(catalog.has_icon(&icon.icon_id));
    }

    // ── PAPIRUS-16: Security requirements ──────────────────────────

    /// Task 16: a filename such as `../../icon.svg` is a **name**, never a
    /// filesystem path.  Resolution must keep the rendered asset inside the
    /// pinned bundle: repo-relative, no `..` component, no absolute prefix,
    /// and the icon id must exist in the manifest.
    #[test]
    fn path_traversal_filenames_never_escape_the_bundle() {
        let catalog = PapirusCatalog::global();
        let malicious: &[&str] = &[
            "../../icon.svg",
            "..\\..\\icon.svg",
            "../../../etc/passwd",
            "a/../../evil.svg",
            "folder/..\\..\\..\\icon.svg",
            "..",
            "%2e%2e/icon.svg", // percent-encoding is NOT decoded: literal name
            "...",             // multiple dots are ordinary name chars
        ];
        for name in malicious {
            let icon = resolve(name);
            assert!(
                icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT),
                "{name:?} escaped the bundle: {}",
                icon.asset_path
            );
            assert!(
                !icon.asset_path.contains(".."),
                "{name:?} produced a traversal path: {}",
                icon.asset_path
            );
            assert!(
                !icon.asset_path.starts_with('/') && !icon.asset_path.starts_with('\\'),
                "{name:?} produced an absolute path: {}",
                icon.asset_path
            );
            assert!(
                icon.asset_path.ends_with(".svg"),
                "{name:?} produced a non-SVG path: {}",
                icon.asset_path
            );
            assert!(
                catalog.has_icon(&icon.icon_id),
                "{name:?} resolved to missing icon {}",
                icon.icon_id
            );
        }
    }

    /// Task 16: a path-like filename must never influence which filesystem
    /// path is read.  Only the last path segment is used for extension
    /// lookup, and the asset path always comes from the manifest.
    #[test]
    fn path_like_filenames_are_sanitised_not_joined() {
        for name in [
            "downloads/report.pdf",
            "downloads/../../report.pdf",
            "..\\downloads\\report.pdf",
            "/tmp/report.pdf",
            "C:\\Users\\attacker\\report.pdf",
        ] {
            let icon = resolve(name);
            assert_eq!(icon.icon_id, "application-pdf", "for {name:?}");
            assert!(
                icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT),
                "for {name:?}: {}",
                icon.asset_path
            );
            assert!(
                !icon.asset_path.contains(".."),
                "for {name:?}: {}",
                icon.asset_path
            );
        }
    }

    /// Task 16: a peer-supplied MIME string is a hint matched against the
    /// static table only.  Even a MIME carrying traversal fragments can
    /// never influence the asset path, which always comes from the pinned
    /// manifest.
    #[test]
    fn malicious_mime_never_constructs_a_filesystem_path() {
        let catalog = PapirusCatalog::global();
        let malicious: &[&str] = &[
            "../../icon.svg",
            "application/pdf;../../icon.svg",
            "image/svg+xml\n../../x",
            "text/plain; charset=utf-8;../../../../etc/passwd",
            "application//pdf",
            "application/pdf ../../icon.svg",
            "..%2F..%2Ficon.svg",
            "image/svg+xml\0",
        ];
        for mime in malicious {
            let icon = resolve_file_icon("blob.bin", Some(mime), None, false);
            assert!(
                icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT),
                "MIME {mime:?} escaped the bundle: {}",
                icon.asset_path
            );
            assert!(
                !icon.asset_path.contains("..") && !icon.asset_path.contains('\0'),
                "MIME {mime:?} produced a traversal/control path: {}",
                icon.asset_path
            );
            assert!(
                !icon.asset_path.starts_with('/'),
                "MIME {mime:?} produced an absolute path: {}",
                icon.asset_path
            );
            assert!(
                catalog.has_icon(&icon.icon_id),
                "MIME {mime:?} resolved to missing icon {}",
                icon.icon_id
            );
        }
    }

    /// Task 16: an executable renamed to `.pdf` must not receive additional
    /// trust from the PDF icon.  The PDF icon alone is only a
    /// Medium-confidence hint; a trusted local detection of an executable
    /// wins for the icon and the mismatch is recorded — the file is
    /// presented as an executable, never granted PDF trust.
    #[test]
    fn executable_renamed_to_pdf_gets_no_pdf_trust() {
        // Extension-only (no local detection): the PDF icon is chosen, but
        // only as a hint — never Exact, and the resolved structure carries
        // no open/execute action (it is purely presentational).
        let hint_only = resolve_file_icon("evil.pdf", None, None, false);
        assert_eq!(hint_only.icon_id, "application-pdf");
        assert_eq!(hint_only.source, ResolutionSource::Extension);
        assert_eq!(hint_only.confidence, IconConfidence::Medium);
        assert!(hint_only.mime_mismatch.is_none());

        // An advertised PDF MIME alone is likewise only a hint.
        let advertised = resolve_file_icon("evil.pdf", Some("application/pdf"), None, false);
        assert_eq!(advertised.source, ResolutionSource::AdvertisedMime);
        assert_eq!(advertised.confidence, IconConfidence::Medium);
        assert_eq!(advertised.icon_id, "application-pdf");

        // With local detection (e.g. an ELF binary), the trusted local type
        // wins for the icon: the file is presented as an executable, and
        // the conflict with the PDF hint is recorded for a warning state.
        let detected = resolve_file_icon(
            "evil.pdf",
            Some("application/pdf"),
            Some("application/x-executable"),
            false,
        );
        assert_eq!(detected.icon_id, "application-x-executable");
        assert_eq!(detected.file_category, FileCategory::Executable);
        assert_eq!(detected.confidence, IconConfidence::Exact);
        let mismatch = detected.mime_mismatch.expect("mismatch must be recorded");
        assert_eq!(mismatch.advertised_category, FileCategory::Pdf);
        assert_eq!(mismatch.locally_detected_category, FileCategory::Executable);

        // A locally detected PDF, by contrast, is Exact — the confidence
        // ladder is what distinguishes trustworthy local data from hints.
        let real_pdf = resolve_file_icon(
            "evil.pdf",
            Some("application/pdf"),
            Some("application/pdf"),
            false,
        );
        assert_eq!(real_pdf.confidence, IconConfidence::Exact);
        assert_eq!(real_pdf.source, ResolutionSource::LocalMime);
        assert!(real_pdf.mime_mismatch.is_none());
    }

    /// The asset allow-list validator itself: reject absolute paths,
    /// drive prefixes, `..` components (both separators), control bytes,
    /// and manifest-relative fragments; accept real repo-relative paths.
    #[test]
    fn bundled_asset_path_validator_rejects_traversal() {
        let rejected: &[&str] = &[
            "",
            "../icon.svg",
            "..\\icon.svg",
            "assets/third_party/papirus/../../../../etc/passwd",
            "assets/third_party/papirus/32/..\\..\\icon.svg",
            "assets/third_party/papirus/32/icon.svg\0",
            "assets/third_party/papirus/32/icon.svg\n",
            "/etc/passwd",
            "\\etc\\passwd",
            "C:\\Windows\\system32\\icon.svg",
            "C:/Windows/system32/icon.svg",
            "32/application-pdf.svg", // manifest-relative, not repo-relative
        ];
        for path in rejected {
            assert!(
                !is_bundled_asset_path(path),
                "validator must reject {path:?}"
            );
        }

        let accepted: &[&str] = &[
            "assets/third_party/papirus/32/application-pdf.svg",
            "assets/third_party/papirus/64/folder-open.svg",
            "assets/third_party/papirus/16/image-svg+xml.svg",
        ];
        for path in accepted {
            assert!(
                is_bundled_asset_path(path),
                "validator must accept {path:?}"
            );
        }
    }

    /// Defense in depth: even a malicious/typo icon id must never yield a
    /// path outside the bundle root — the manifest is the allow-list and
    /// the path validator is the second gate.
    #[test]
    fn manifest_asset_paths_are_always_in_bundle() {
        for bad_id in [
            "../../icon",
            "/etc/passwd",
            "..\\..\\icon",
            "assets/third_party/papirus/32/application-pdf.svg",
        ] {
            assert!(
                papirus_asset_path(bad_id, 32).is_none(),
                "non-manifest id {bad_id:?} must not resolve"
            );
        }
        let path = papirus_asset_path("application-pdf", 32).expect("pdf icon at 32");
        assert!(path.starts_with(PAPIRUS_ASSET_ROOT));
        assert!(is_bundled_asset_path(&path));
    }

    /// Task 16: choosing an icon never inspects or decodes file contents on
    /// the UI thread — `resolve_file_icon` is pure and performs no file I/O.
    /// This test pins that contract: the resolver's only output is a
    /// manifest-grounded path, and every resolution below runs with no
    /// filesystem access (the test module never opens files).
    #[test]
    fn resolution_performs_no_file_io() {
        // If the resolver ever opened the filename as a path, these would
        // fail or read the wrong files.  They resolve from names only.
        let icon = resolve("/nonexistent/dir/report.pdf");
        assert_eq!(icon.icon_id, "application-pdf");
        let icon = resolve_file_icon("blob", Some("image/svg+xml;../../etc/passwd"), None, false);
        assert!(icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT));
        let folder = resolve_file_icon("../../..", None, None, true);
        assert_eq!(folder.icon_id, DIRECTORY_ICON);
    }

    // ── PAPIRUS-17: manifest load-once + curated bundle size ──────

    /// The manifest is parsed exactly once and shared: `global()` returns
    /// the same `'static` catalog for every caller (PAPIRUS-17 "avoid
    /// reading the same manifest repeatedly").
    #[test]
    fn catalog_global_is_a_singleton() {
        let a: &'static PapirusCatalog = PapirusCatalog::global();
        let b: &'static PapirusCatalog = PapirusCatalog::global();
        assert!(std::ptr::eq(a, b), "catalog must be a process singleton");
    }

    /// The curated bundle stays small (PAPIRUS-17 "do not bundle thousands
    /// of unused icons"): 114 curated icons × 5 sizes, far below the
    /// thousands-scale the full Papirus theme would add.
    #[test]
    fn curated_bundle_stays_small() {
        let catalog = PapirusCatalog::global();
        assert!(
            catalog.icons.len() < 200,
            "curated icon count must stay small, got {}",
            catalog.icons.len()
        );
        for (icon_id, sizes) in &catalog.icons {
            assert!(
                sizes.len() <= 5,
                "{icon_id} must not carry more than the 5 standard size dirs"
            );
        }
    }

    // ── PAPIRUS-17: resolver result cache ──────────────────────────

    /// The cache key is built from **normalised** inputs: case, whitespace,
    /// MIME parameters, and directory prefixes must not create distinct
    /// entries for the same underlying type.
    #[test]
    fn resolve_cache_key_normalises_mime_and_extension() {
        let a = resolve_cache_key("REPORT.PDF", Some("IMAGE/PNG"), None, false);
        let b = resolve_cache_key("report.pdf", Some(" image/png "), None, false);
        assert_eq!(a, b);

        let c = resolve_cache_key(
            "download.bin",
            Some("text/plain; charset=utf-8"),
            None,
            false,
        );
        let d = resolve_cache_key("download.bin", Some("text/plain"), None, false);
        assert_eq!(c, d, "MIME parameters must be stripped before keying");

        let e = resolve_cache_key("a/report.pdf", None, None, false);
        let f = resolve_cache_key("b/c/report.pdf", None, None, false);
        assert_eq!(e, f, "directory components must not affect the key");

        let dir1 = resolve_cache_key("report.pdf", None, None, true);
        let dir2 = resolve_cache_key("report.pdf", None, None, false);
        assert_ne!(dir1, dir2, "directory state must be part of the key");

        let none = resolve_cache_key("x", None, None, false);
        let empty = resolve_cache_key("x", Some("   "), None, false);
        assert_eq!(none, empty, "whitespace-only MIME is the same as absent");
    }

    /// Repeated resolution of the same normalised type returns the cached
    /// result — identical struct, including the mismatch record.
    #[test]
    fn resolve_cache_returns_identical_result_for_normalised_equivalents() {
        let a = resolve_file_icon("photo.png", Some("image/png"), Some("video/mp4"), false);
        let b = resolve_file_icon("PHOTO.PNG", Some(" image/png "), Some("video/mp4"), false);
        assert_eq!(a.icon_id, b.icon_id);
        assert_eq!(a.asset_path, b.asset_path);
        assert_eq!(a.source, b.source);
        assert_eq!(a.mime_mismatch, b.mime_mismatch);
    }

    /// The bounded insert helper never lets the map exceed the cap (the
    /// process-global cache is exercised indirectly through every other
    /// resolution test; this pins the bound itself).
    #[test]
    fn bounded_resolve_cache_insert_respects_cap() {
        let mut cache: HashMap<ResolveCacheKey, ResolvedFileIcon> = HashMap::new();
        let base = resolve_file_icon("x.pdf", None, None, false);
        for i in 0..RESOLVE_CACHE_MAX_ENTRIES + 100 {
            let key = ResolveCacheKey {
                is_directory: false,
                advertised_mime: Some(format!("application/x-test-{i}")),
                local_mime: None,
                extensions: vec![format!("ext{i}")],
            };
            bounded_resolve_cache_insert(&mut cache, key, base.clone());
        }
        assert!(
            cache.len() <= RESOLVE_CACHE_MAX_ENTRIES,
            "cache must be bounded at {RESOLVE_CACHE_MAX_ENTRIES}, got {}",
            cache.len()
        );
    }

    // ── PAPIRUS-17: alias dedup (canonical duplicate-group members) ──

    /// Aliases that share identical content must resolve to the same
    /// canonical asset path, so the SVG handle cache stores one entry per
    /// distinct content instead of one per alias.
    #[test]
    fn duplicate_group_aliases_resolve_to_one_canonical_path() {
        let audio_flac = papirus_asset_path("audio-flac", 32).expect("audio-flac must exist at 32");
        let audio_m4a =
            papirus_asset_path("audio-x-m4a", 32).expect("audio-x-m4a must exist at 32");
        assert_eq!(
            audio_flac, audio_m4a,
            "audio-x-m4a is a byte-identical alias of audio-flac and must resolve to the same path"
        );

        let img_png = papirus_asset_path("image-png", 32).expect("image-png at 32");
        let img_generic = papirus_asset_path("image-x-generic", 32).expect("image-x-generic at 32");
        assert_eq!(
            img_png, img_generic,
            "image-x-generic is a byte-identical alias of image-png"
        );

        // The canonical member is the lexicographically smallest path in
        // the group and is a real bundle path.
        for p in [&audio_flac, &img_png] {
            assert!(is_bundled_asset_path(p), "{p} must be a bundle path");
            assert!(p.ends_with(".svg"), "{p} must end in .svg");
        }
    }

    /// Singletons (icons not in any duplicate group) keep their own path.
    #[test]
    fn singleton_icons_keep_their_own_path() {
        let pdf = papirus_asset_path("application-pdf", 32).expect("pdf at 32");
        assert!(pdf.ends_with("32/application-pdf.svg"));
        let folder = papirus_asset_path("folder-open", 32).expect("folder at 32");
        assert!(folder.ends_with("32/folder-open.svg"));
    }

    // ── PAPIRUS-19: Task 19 required resolver scenarios ────────────

    /// Task 19: a Unicode filename must not confuse extension resolution —
    /// the extension is ASCII-safe regardless of the base name's script.
    /// The icon/category is chosen from the extension alone.
    #[test]
    fn task19_unicode_filename_resolves_by_extension() {
        let cases: &[(&str, &str, FileCategory)] = &[
            ("résumé.pdf", "application-pdf", FileCategory::Pdf),
            ("фотография.png", "image-png", FileCategory::Image),
            ("音乐.flac", "audio-flac", FileCategory::Audio),
            ("视频.mp4", "video-mp4", FileCategory::Video),
            (
                "دليل.docx",
                "application-vnd.openxmlformats-officedocument.wordprocessingml.document",
                FileCategory::Document,
            ),
            (
                "資料.xlsx",
                "application-vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                FileCategory::Spreadsheet,
            ),
            (
                "報告.pptx",
                "application-vnd.openxmlformats-officedocument.presentationml.presentation",
                FileCategory::Presentation,
            ),
            ("README.md", "text-markdown", FileCategory::Markdown),
            ("main.rs", "text-rust", FileCategory::SourceCode),
            ("backup.tar.gz", "application-x-tar", FileCategory::Archive),
        ];
        for (name, icon_id, category) in cases {
            let icon = resolve(name);
            assert_eq!(&icon.icon_id, icon_id, "for {name:?}");
            assert_eq!(icon.file_category, *category, "for {name:?}");
            assert!(
                PapirusCatalog::global().has_icon(&icon.icon_id),
                "for {name:?}: resolved icon {} must exist",
                icon.icon_id
            );
        }

        // A Unicode filename with NO extension still falls back safely.
        let icon = resolve("файлбезрасширения");
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));
    }

    /// Task 19: a very long filename (e.g. a 1 MiB path-like string) must
    /// never panic, must still resolve by its final extension, and must
    /// never produce a missing asset.  This pins the resolver against
    /// pathological input from peers / filesystems.
    #[test]
    fn task19_very_long_filename_resolves_safely() {
        let catalog = PapirusCatalog::global();

        // A 256 KiB filename with a real extension at the end.
        let mut long = "a".repeat(256 * 1024);
        long.push_str(".pdf");
        let icon = resolve(&long);
        assert_eq!(icon.icon_id, "application-pdf");
        assert!(catalog.has_icon(&icon.icon_id));
        assert!(icon.asset_path.ends_with(".svg"));

        // A 64 KiB name with a compound archive extension.
        let mut long_tar = "备份".repeat(16 * 1024);
        long_tar.push_str(".tar.gz");
        let icon = resolve(&long_tar);
        assert_eq!(icon.icon_id, "application-x-tar");
        assert_eq!(icon.file_category, FileCategory::Archive);
        assert!(catalog.has_icon(&icon.icon_id));

        // A 64 KiB name with NO extension → unknown fallback, never a
        // missing asset, never a panic.
        let mut long_noext = "x".repeat(64 * 1024);
        long_noext.push_str("─");
        let icon = resolve(&long_noext);
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert!(catalog.has_icon(&icon.icon_id));
    }

    /// Task 19: the full required resolver scenario matrix.  Each listed
    /// scenario resolves to the correct icon/category and — critically —
    /// every result is grounded in an existing bundled asset, so no
    /// broken-image symbol can ever appear.
    #[test]
    fn task19_required_scenarios_all_resolve_to_existing_assets() {
        let catalog = PapirusCatalog::global();
        let mut check = |name: &str,
                         mime: Option<&str>,
                         local: Option<&str>,
                         is_dir: bool,
                         expected_category: FileCategory| {
            let icon = resolve_file_icon(name, mime, local, is_dir);
            assert_eq!(
                icon.file_category, expected_category,
                "scenario {name:?} mime={mime:?} local={local:?} is_dir={is_dir}"
            );
            assert!(
                catalog.has_icon(&icon.icon_id),
                "scenario {name:?} resolved to missing icon {}",
                icon.icon_id
            );
            assert!(
                icon.asset_path.starts_with(PAPIRUS_ASSET_ROOT)
                    && icon.asset_path.ends_with(".svg"),
                "scenario {name:?} produced non-bundle path {}",
                icon.asset_path
            );
        };

        // 1. MIME type only (no extension signal).
        check(
            "download",
            Some("application/pdf"),
            None,
            false,
            FileCategory::Pdf,
        );
        // 2. Extension only.
        check("report.pdf", None, None, false, FileCategory::Pdf);
        // 3. MIME + extension agreement.
        check(
            "photo.png",
            Some("image/png"),
            None,
            false,
            FileCategory::Image,
        );
        // 4. MIME + extension conflict → locally detected wins (also tested in depth above).
        check(
            "photo.png",
            Some("image/png"),
            Some("video/mp4"),
            false,
            FileCategory::Video,
        );
        // 5. Uppercase extension.
        check("REPORT.PDF", None, None, false, FileCategory::Pdf);
        // 6. Compound extension.
        check("archive.tar.gz", None, None, false, FileCategory::Archive);
        // 7. Missing extension.
        check("README", None, None, false, FileCategory::Unknown);
        // 8. Hidden file.
        check(".gitignore", None, None, false, FileCategory::Unknown);
        // 9. Folder (explicit state).
        check("shared-folder", None, None, true, FileCategory::Folder);
        // 10. Unknown type.
        check("mystery.zzz", None, None, false, FileCategory::Unknown);
        // 11. Malformed MIME string.
        check(
            "blob.bin",
            Some("not-a-mime"),
            None,
            false,
            FileCategory::Unknown,
        );
        // 12. Path-like malicious filename.
        check("../../etc/passwd", None, None, false, FileCategory::Unknown);
        // 13. Unicode filename.
        check("résumé.pdf", None, None, false, FileCategory::Pdf);
        // 14. Very long filename.
        check(
            &format!("{}.pdf", "x".repeat(100_000)),
            None,
            None,
            false,
            FileCategory::Pdf,
        );
    }

    /// Task 19 fallback: when the exact icon for a known class is missing
    /// from the bundle, the resolver must fall back to the broad category
    /// icon (priority 7) — never a missing asset, never a broken image.
    /// Simulated by removing the exact icon from a copy of the catalog.
    #[test]
    fn task19_fallback_exact_icon_missing_uses_category_icon() {
        // Build a catalog copy without the exact `video-mp4` icon.
        let base = PapirusCatalog::global();
        let mut icons = base.icons.clone();
        icons.remove("video-mp4");
        let catalog: &'static PapirusCatalog = Box::leak(Box::new(PapirusCatalog {
            icons,
            required_fallbacks: base.required_fallbacks.clone(),
            canonical_paths: base.canonical_paths.clone(),
        }));

        let icon = build_with_fallback(
            catalog,
            "clip.mp4",
            "video-mp4",
            FileCategory::Video,
            IconConfidence::Medium,
            ResolutionSource::Extension,
            None,
        );
        assert_eq!(
            icon.icon_id, "video-x-generic",
            "exact video-mp4 missing → broad video category icon"
        );
        assert_eq!(icon.file_category, FileCategory::Video);
        assert!(catalog.has_icon(&icon.icon_id));
        assert!(icon.asset_path.ends_with(".svg"));

        // The category fallback icon itself must exist in the REAL bundle.
        assert!(base.has_icon("video-x-generic"));
    }

    /// Task 19 fallback: when BOTH the exact icon and the broad category
    /// icon are missing, the resolver must end on the generic unknown icon
    /// (priority 8) — the terminal fallback is always available.
    #[test]
    fn task19_fallback_category_icon_missing_uses_unknown() {
        let base = PapirusCatalog::global();
        let mut icons = base.icons.clone();
        icons.remove("video-mp4");
        icons.remove("video-x-generic");
        let catalog: &'static PapirusCatalog = Box::leak(Box::new(PapirusCatalog {
            icons,
            required_fallbacks: base.required_fallbacks.clone(),
            canonical_paths: base.canonical_paths.clone(),
        }));

        let icon = build_with_fallback(
            catalog,
            "clip.mp4",
            "video-mp4",
            FileCategory::Video,
            IconConfidence::Medium,
            ResolutionSource::Extension,
            None,
        );
        assert_eq!(
            icon.icon_id, UNKNOWN_ICON,
            "exact + category missing → generic unknown icon"
        );
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert!(catalog.has_icon(&icon.icon_id));
        // The terminal unknown icon is a required fallback in the real bundle.
        assert!(base.has_icon(UNKNOWN_ICON));
    }

    /// Every duplicate-group member maps to a canonical path that exists in
    /// the manifest — no alias can point outside the bundle.
    #[test]
    fn canonical_alias_paths_stay_inside_the_bundle() {
        let catalog = PapirusCatalog::global();
        for (member, canonical) in &catalog.canonical_paths {
            assert!(
                member.starts_with("16/")
                    || member.starts_with("24/")
                    || member.starts_with("32/")
                    || member.starts_with("48/")
                    || member.starts_with("64/"),
                "member {member} must be a size-relative manifest path"
            );
            assert!(
                canonical.ends_with(".svg"),
                "canonical {canonical} for {member} must be an SVG"
            );
            let repo = format!("{PAPIRUS_ASSET_ROOT}/{canonical}");
            assert!(
                is_bundled_asset_path(&repo),
                "canonical repo path {repo} for {member} must be a valid bundle path"
            );
            assert!(
                catalog
                    .icons
                    .values()
                    .any(|sizes| { sizes.values().any(|p| p == canonical.as_str()) }),
                "canonical {canonical} for {member} must appear in the manifest icons map"
            );
        }
    }

    // ── PAPIRUS-21: octet-stream MIME is "no MIME info" ──────────

    /// A stored/advertised `application/octet-stream` must never outrank a
    /// real filename extension: `budget.xlsx` with octet-stream resolves to
    /// the spreadsheet icon exactly as the same file does in a chat card
    /// (extension-only path).
    #[test]
    fn octet_stream_advertised_falls_through_to_extension() {
        let cases: &[(&str, &str, FileCategory)] = &[
            (
                "document.docx",
                "application-vnd.openxmlformats-officedocument.wordprocessingml.document",
                FileCategory::Document,
            ),
            (
                "budget.xlsx",
                "application-vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                FileCategory::Spreadsheet,
            ),
            (
                "slides.pptx",
                "application-vnd.openxmlformats-officedocument.presentationml.presentation",
                FileCategory::Presentation,
            ),
            ("movie.mp4", "video-mp4", FileCategory::Video),
            ("song.mp3", "audio-mp3", FileCategory::Audio),
            ("bundle.zip", "application-zip", FileCategory::Archive),
            (
                "package.7z",
                "application-x-7z-compressed",
                FileCategory::Archive,
            ),
            ("main.rs", "text-rust", FileCategory::SourceCode),
            ("script.py", "text-x-python", FileCategory::SourceCode),
        ];
        for (name, icon_id, category) in cases {
            let icon = resolve_file_icon(name, Some(MIME_NO_INFO), None, false);
            assert_eq!(&icon.icon_id, icon_id, "for {name:?}");
            assert_eq!(icon.file_category, *category, "for {name:?}");
            assert_eq!(
                icon.source,
                ResolutionSource::Extension,
                "for {name:?}: octet-stream must not win at priority 4"
            );
            assert!(
                PapirusCatalog::global().has_icon(&icon.icon_id),
                "for {name:?}: resolved icon {} must exist",
                icon.icon_id
            );
        }

        // The extension-resolved icon is pixel-identical to the chat-card
        // icon (same canonical asset path).
        let with_mime = resolve_file_icon("budget.xlsx", Some(MIME_NO_INFO), None, false);
        let chat_card = resolve("budget.xlsx");
        assert_eq!(with_mime.asset_path, chat_card.asset_path);
    }

    /// The same fall-through applies when the octet-stream value is stored
    /// as the locally detected MIME (e.g. legacy rows written before
    /// PAPIRUS-21) — the extension still wins.
    #[test]
    fn octet_stream_locally_detected_falls_through_to_extension() {
        let icon = resolve_file_icon("budget.xlsx", None, Some(MIME_NO_INFO), false);
        assert_eq!(icon.file_category, FileCategory::Spreadsheet);
        assert_eq!(
            icon.icon_id,
            "application-vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        assert_eq!(icon.source, ResolutionSource::Extension);
        assert!(icon.mime_mismatch.is_none());

        let icon = resolve_file_icon("movie.mp4", None, Some(MIME_NO_INFO), false);
        assert_eq!(icon.icon_id, "video-mp4");
        assert_eq!(icon.file_category, FileCategory::Video);
        assert_eq!(icon.source, ResolutionSource::Extension);
    }

    /// An unknown extension with octet-stream still ends on the generic
    /// unknown icon — octet-stream adds no signal, so `mystery.crypt`
    /// behaves exactly like `mystery.crypt` with no MIME at all.
    #[test]
    fn octet_stream_with_unknown_extension_stays_generic() {
        let icon = resolve_file_icon("mystery.crypt", Some(MIME_NO_INFO), None, false);
        assert_eq!(icon.icon_id, UNKNOWN_ICON);
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));

        // Identical to the no-MIME resolution of the same name.
        let plain = resolve("mystery.crypt");
        assert_eq!(icon.icon_id, plain.icon_id);
        assert_eq!(icon.asset_path, plain.asset_path);
    }

    /// An explicit `.bin` name still resolves to the bundled octet-stream
    /// binary icon — via the extension table, not via the MIME hint.
    #[test]
    fn octet_stream_with_bin_extension_resolves_binary_icon() {
        let icon = resolve_file_icon("firmware.bin", Some(MIME_NO_INFO), None, false);
        assert_eq!(icon.icon_id, "application-octet-stream");
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert_eq!(icon.source, ResolutionSource::Extension);
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));
    }

    /// An extensionless file with octet-stream falls to the generic unknown
    /// icon (the canonical "unknown" terminal), never a missing asset.
    #[test]
    fn octet_stream_without_extension_falls_back_to_unknown_generic() {
        let icon = resolve_file_icon("download", Some(MIME_NO_INFO), None, false);
        assert_eq!(icon.icon_id, UNKNOWN_ICON);
        assert_eq!(icon.file_category, FileCategory::Unknown);
        assert_eq!(icon.source, ResolutionSource::UnknownFallback);
        assert!(PapirusCatalog::global().has_icon(&icon.icon_id));
    }

    /// octet-stream carries no category signal, so it never triggers a
    /// MIME mismatch record when paired with a real type — there is no
    /// conflict to warn about.
    #[test]
    fn octet_stream_never_triggers_mime_mismatch() {
        let icon = resolve_file_icon("photo.png", Some(MIME_NO_INFO), Some("image/png"), false);
        assert_eq!(icon.icon_id, "image-png");
        assert_eq!(icon.source, ResolutionSource::LocalMime);
        assert_eq!(icon.confidence, IconConfidence::Exact);
        assert!(icon.mime_mismatch.is_none());

        let icon = resolve_file_icon("clip.mp4", Some("video/mp4"), Some(MIME_NO_INFO), false);
        assert_eq!(icon.icon_id, "video-mp4");
        assert_eq!(icon.source, ResolutionSource::AdvertisedMime);
        assert!(icon.mime_mismatch.is_none());
    }

    /// The cache key treats octet-stream as absent, so `budget.xlsx` with
    /// octet-stream and `budget.xlsx` with no MIME share one entry.
    #[test]
    fn octet_stream_cache_key_equals_absent() {
        let a = resolve_cache_key("budget.xlsx", Some(MIME_NO_INFO), None, false);
        let b = resolve_cache_key("budget.xlsx", None, None, false);
        assert_eq!(a, b);

        let c = resolve_cache_key("movie.mp4", None, Some(MIME_NO_INFO), false);
        let d = resolve_cache_key("movie.mp4", None, None, false);
        assert_eq!(c, d);
    }
}
