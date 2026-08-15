//! Boru dev layout override config — `boru-layout.toml` (BORU-LAYOUT-06 /
//! PDF Task 6).
//!
//! A development-only, human-editable TOML file that overrides **structural
//! layout** values on top of [`LayoutConfig::default()`](crate::layout::LayoutConfig).
//! It mirrors the `*Overrides` groups in `layout.rs` 1:1 (`home`, `sidebar`,
//! `chat`, `component`, `tables`, `responsive`, `screens`) — the same
//! Option-leaf partial-override organisation as `theme_config.rs` for
//! [`BoruTheme`](crate::theme::BoruTheme).
//!
//! ## Design rules
//!
//! - **Mirrors the layout model.** Every leaf is `Option<T>` and every
//!   struct carries `#[serde(default)]` (the derives live on the
//!   `*Overrides` types in `layout.rs`), so a partial file (or an empty
//!   one) deserializes to `None` leaves — the merge step later falls back
//!   to `LayoutConfig::default()`.
//! - **Missing file is fine.** [`load_layout_config`] returns an empty
//!   config when `<data_dir>/boru-layout.toml` does not exist; startup never
//!   fails because of the dev file.
//! - **Malformed files are reported, not fatal.** Parse errors surface as a
//!   structured [`LayoutConfigError`] with the file path and line/column;
//!   the caller logs it and keeps the last known-good layout.
//! - **Layout only.** This file never carries theme tokens, networking,
//!   chat, file transfer, video, tunnel, lobby, room or persistence
//!   behaviour. Only validated layouts are applied (BORU-LAYOUT-06 rejects
//!   unparseable files; BORU-LAYOUT-07 adds semantic validation — duplicate
//!   section ids in the order/visibility lists are rejected and the last
//!   known-good layout is retained).
//!
//! The sample file (`boru-layout.example.toml`, repo root) documents every
//! group with valid units and ranges.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::layout::{
    ButtonPlacement, CardOrientation, ComponentPlacementOverrides, LayoutOverrides,
    MetadataAlignment, ThumbnailPosition,
};

/// File name of the dev layout override file (inside the data dir).
pub const LAYOUT_CONFIG_FILE_NAME: &str = "boru-layout.toml";

/// Structured error returned when the dev layout override file cannot be
/// used. Mirrors `theme_config::UiThemeConfigError`: it carries the
/// offending path and (for parse errors) the line/column from the TOML
/// parser.
#[derive(Debug)]
pub enum LayoutConfigError {
    /// The file does not exist. [`load_layout_config`] treats this as
    /// "no overrides" (Ok); the inspector's explicit "Reload Layout From
    /// Disk" action reports it as an error so a missing file is not
    /// silently mistaken for a successful reload.
    NotFound {
        path: PathBuf,
    },
    /// The file exists but could not be read (permissions, I/O, …).
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file exists but is not valid TOML / not a valid layout config.
    /// `line`/`column` are 1-based positions of the offending byte span
    /// (when the parser provides one — syntax errors do, some serde
    /// type-mismatch errors do not).
    Parse {
        path: PathBuf,
        source: toml::de::Error,
        line: Option<usize>,
        column: Option<usize>,
    },
    /// The file parsed but failed semantic validation (BORU-LAYOUT-07):
    /// duplicate section ids in the section order / visibility lists.
    /// `issues` lists every problem found, human-readable.
    Validation { path: PathBuf, issues: Vec<String> },
}

impl std::fmt::Display for LayoutConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutConfigError::NotFound { path } => {
                write!(f, "cannot find dev layout override file {}", path.display())
            }
            LayoutConfigError::Io { path, source } => write!(
                f,
                "cannot read layout config {}: {source}",
                path.display()
            ),
            LayoutConfigError::Parse { path, source, .. } => {
                write!(
                    f,
                    "invalid dev layout override {}: {source}",
                    path.display()
                )
            }
            LayoutConfigError::Validation { path, issues } => {
                write!(
                    f,
                    "invalid dev layout override {}: {}",
                    path.display(),
                    issues.join("; ")
                )
            }
        }
    }
}

impl std::error::Error for LayoutConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LayoutConfigError::NotFound { .. } => None,
            LayoutConfigError::Io { source, .. } => Some(source),
            LayoutConfigError::Parse { source, .. } => Some(source),
            // Validation issues are plain strings, no underlying error.
            LayoutConfigError::Validation { .. } => None,
        }
    }
}

/// Machine-readable category of a layout load failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutReloadErrorKind {
    /// The file does not exist (only surfaced by the inspector's explicit
    /// reload action — the watcher treats a missing file as "no
    /// overrides").
    NotFound,
    /// The file exists but could not be read (permissions, I/O, …).
    Io,
    /// The file exists but is not valid TOML / not a valid layout config.
    Parse,
    /// The file parsed but failed semantic validation (duplicate section
    /// ids in the section order / visibility lists).
    Validation,
}

/// Clone-able structured summary of a layout load failure.
///
/// [`LayoutConfigError`] is the authoritative error (it holds the
/// non-`Clone` `toml::de::Error`), but it cannot ride inside `AppMessage`
/// (which derives `Clone`). This projection carries everything the
/// developer needs — file path, error kind, a human-readable message (with
/// parser detail where available) and 1-based line/column — across the
/// watcher → app boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutReloadError {
    /// The offending dev layout file.
    pub path: PathBuf,
    /// Machine-readable failure category.
    pub kind: LayoutReloadErrorKind,
    /// Human-readable description: path + parser detail (line/column for
    /// syntax errors, field/key path for serde type errors).
    pub message: String,
    /// 1-based line of the error when the parser provided a span.
    pub line: Option<usize>,
    /// 1-based column of the error when the parser provided a span.
    pub column: Option<usize>,
}

impl std::fmt::Display for LayoutReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LayoutReloadError {}

impl LayoutReloadError {
    /// Project a [`LayoutConfigError`] into the Clone-able summary used at
    /// the watcher → app boundary.
    pub fn from_layout_error(err: &LayoutConfigError) -> Self {
        match err {
            LayoutConfigError::NotFound { path } => LayoutReloadError {
                path: path.clone(),
                kind: LayoutReloadErrorKind::NotFound,
                line: None,
                column: None,
                message: err.to_string(),
            },
            LayoutConfigError::Io { path, .. } => LayoutReloadError {
                path: path.clone(),
                kind: LayoutReloadErrorKind::Io,
                message: err.to_string(),
                line: None,
                column: None,
            },
            LayoutConfigError::Parse {
                path, line, column, ..
            } => LayoutReloadError {
                path: path.clone(),
                kind: LayoutReloadErrorKind::Parse,
                message: err.to_string(),
                line: *line,
                column: *column,
            },
            LayoutConfigError::Validation { path, .. } => LayoutReloadError {
                path: path.clone(),
                kind: LayoutReloadErrorKind::Validation,
                message: err.to_string(),
                line: None,
                column: None,
            },
        }
    }
}

/// Compute a 1-based (line, column) for a byte span into `text`.
///
/// The TOML parser exposes only a byte range ([`toml::de::Error::span`]);
/// translating it to a human position requires the source text. `None`
/// when the parser gave no span (some serde type-mismatch errors).
fn toml_line_col(
    text: &str,
    span: Option<std::ops::Range<usize>>,
) -> (Option<usize>, Option<usize>) {
    let Some(span) = span else {
        return (None, None);
    };
    let start = span.start.min(text.len());
    let line = text[..start].bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let column = text[line_start..start].chars().count() + 1;
    (Some(line), Some(column))
}

/// Parse layout overrides from a TOML string.
///
/// Missing keys are allowed (every leaf is `Option`); the string does not
/// need to contain any group. A syntactically invalid file returns a
/// [`toml::de::Error`] which callers wrap into [`LayoutConfigError::Parse`].
pub fn parse_layout_config(text: &str) -> Result<LayoutOverrides, toml::de::Error> {
    toml::from_str(text)
}

/// Validate one section-id pair (an order list + a hidden list) for
/// duplicates (BORU-LAYOUT-07 / PDF Task 7).
///
/// Three mistakes are rejected:
///
/// - a section id repeated **inside** the order list,
/// - a section id repeated **inside** the hidden list,
/// - a section id present in **both** lists (contradictory visibility).
///
/// `T` is the section id type (`HomeSection`, `SidebarSection`, or
/// `String` for future screens); its `Debug` spelling matches the TOML
/// spelling (`Tunnels`, `Requests`, …), so the messages read naturally.
/// Issues are appended to `issues`; an empty list means this pair is fine.
fn validate_section_ids<T: Ord + std::fmt::Debug>(
    order_path: &str,
    hidden_path: &str,
    order: Option<&[T]>,
    hidden: Option<&[T]>,
    issues: &mut Vec<String>,
) {
    if let Some(order) = order {
        let mut seen = BTreeSet::new();
        for (index, id) in order.iter().enumerate() {
            if !seen.insert(id) {
                issues.push(format!(
                    "{order_path}: duplicate section id {id:?} at index {index}"
                ));
            }
        }
    }
    if let Some(hidden) = hidden {
        let mut seen = BTreeSet::new();
        for (index, id) in hidden.iter().enumerate() {
            if !seen.insert(id) {
                issues.push(format!(
                    "{hidden_path}: duplicate section id {id:?} at index {index}"
                ));
            }
        }
    }
    if let (Some(order), Some(hidden)) = (order, hidden) {
        let hidden_set: BTreeSet<&T> = hidden.iter().collect();
        for id in order {
            if hidden_set.contains(id) {
                issues.push(format!(
                    "{order_path}: section id {id:?} is also listed in {hidden_path}"
                ));
            }
        }
    }
}

/// Validate the semantic invariants of parsed layout overrides
/// (BORU-LAYOUT-07 / PDF Task 7).
///
/// Numeric ranges are clamped during merge; this pass rejects *structural*
/// mistakes that cannot be clamped — duplicate section ids in the section
/// order / visibility lists of `home`, `sidebar` and every future screen.
/// Returns a list of human-readable issues; empty means the overrides are
/// structurally valid.
pub fn validate_layout_overrides(overrides: &LayoutOverrides) -> Vec<String> {
    let mut issues = Vec::new();
    if let Some(home) = &overrides.home {
        validate_section_ids(
            "home.section_order",
            "home.hidden_sections",
            home.section_order.as_deref(),
            home.hidden_sections.as_deref(),
            &mut issues,
        );
    }
    if let Some(sidebar) = &overrides.sidebar {
        validate_section_ids(
            "sidebar.section_order",
            "sidebar.hidden_sections",
            sidebar.section_order.as_deref(),
            sidebar.hidden_sections.as_deref(),
            &mut issues,
        );
    }
    for (screen_id, screen) in &overrides.screens {
        validate_section_ids(
            &format!("screens.{screen_id}.section_order"),
            &format!("screens.{screen_id}.hidden_sections"),
            screen.section_order.as_deref(),
            screen.hidden_sections.as_deref(),
            &mut issues,
        );
    }
    if let Some(component) = &overrides.component {
        validate_component_placement(
            "component",
            component.thumbnail_position,
            component.metadata_alignment,
            component.button_placement,
            component.card_orientation,
            &mut issues,
        );
        if let Some(video_card) = &component.video_card {
            validate_component_placement_overrides("component.video_card", video_card, &mut issues);
        }
        if let Some(shared_by_me) = &component.shared_by_me {
            validate_component_placement_overrides("component.shared_by_me", shared_by_me, &mut issues);
        }
    }
    issues
}

fn validate_component_placement_overrides(
    path: &str,
    overrides: &ComponentPlacementOverrides,
    issues: &mut Vec<String>,
) {
    let defaults = crate::layout::ComponentPlacement::default();
    validate_component_placement(
        path,
        overrides.thumbnail_position.or(Some(defaults.thumbnail_position)),
        overrides.metadata_alignment.or(Some(defaults.metadata_alignment)),
        overrides.button_placement.or(Some(defaults.button_placement)),
        overrides.card_orientation.or(Some(defaults.card_orientation)),
        issues,
    );
}

fn validate_component_placement(
    path: &str,
    thumbnail: Option<ThumbnailPosition>,
    metadata: Option<MetadataAlignment>,
    buttons: Option<ButtonPlacement>,
    orientation: Option<CardOrientation>,
    issues: &mut Vec<String>,
) {
    let Some(orientation) = orientation else { return };
    let thumbnail = thumbnail.unwrap_or(ThumbnailPosition::Left);
    let buttons = buttons.unwrap_or(ButtonPlacement::Below);
    let vertical_thumbnail = matches!(thumbnail, ThumbnailPosition::Top | ThumbnailPosition::Bottom);
    let horizontal_thumbnail = matches!(thumbnail, ThumbnailPosition::Left | ThumbnailPosition::Right);
    if matches!(orientation, CardOrientation::Vertical) && !vertical_thumbnail {
        issues.push(format!("{path}.card_orientation = Vertical requires thumbnail_position Top or Bottom"));
    }
    if matches!(orientation, CardOrientation::Horizontal) && !horizontal_thumbnail {
        issues.push(format!("{path}.card_orientation = Horizontal requires thumbnail_position Left or Right"));
    }
    if matches!(buttons, ButtonPlacement::Side) && !matches!(orientation, CardOrientation::Horizontal) {
        issues.push(format!("{path}.button_placement = Side requires card_orientation Horizontal"));
    }
    // Non-start metadata alignment is a vertical-card affordance.  In a
    // horizontal card the metadata column shares a row with the thumbnail;
    // centering or trailing-aligning that column produces an unsupported
    // layout (and is especially ambiguous when actions are on the side).
    // Keep Start available for every placement, while Center/End require the
    // vertical card composition with a top/bottom thumbnail.
    if matches!(metadata, Some(MetadataAlignment::Center | MetadataAlignment::End))
        && (!matches!(orientation, CardOrientation::Vertical) || !vertical_thumbnail)
    {
        issues.push(format!(
            "{path}.metadata_alignment = Center or End requires card_orientation Vertical with thumbnail_position Top or Bottom"
        ));
    }
}

/// Load layout overrides from `<data_dir>/boru-layout.toml`.
///
/// - **Missing file** → `Ok(LayoutOverrides::default())` (empty overrides;
///   startup never fails because the dev file is absent).
/// - **Unreadable file** (permissions etc.) → `Err(LayoutConfigError::Io)`.
/// - **Malformed file** → `Err(LayoutConfigError::Parse)` with line/column;
///   the caller keeps the last known-good layout and logs the error.
/// - **Structurally invalid file** (duplicate section ids) →
///   `Err(LayoutConfigError::Validation)` with every issue listed; the
///   caller keeps the last known-good layout just like a parse error.
pub fn load_layout_config(data_dir: &Path) -> Result<LayoutOverrides, LayoutConfigError> {
    let path = data_dir.join(LAYOUT_CONFIG_FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LayoutOverrides::default());
        }
        Err(source) => return Err(LayoutConfigError::Io { path, source }),
    };
    let overrides = parse_layout_config(&text).map_err(|source| {
        let (line, column) = toml_line_col(&text, source.span());
        LayoutConfigError::Parse {
            path: path.clone(),
            source,
            line,
            column,
        }
    })?;
    let issues = validate_layout_overrides(&overrides);
    if !issues.is_empty() {
        return Err(LayoutConfigError::Validation { path, issues });
    }
    Ok(overrides)
}

// ── Save / reload path (BORU-LAYOUT-08 / PDF Task 8) ─────────────────
//
// The inspector edits the live layout through `set_layout_overrides` and
// persists the current override set to `boru-layout.toml`. The file the
// inspector writes is the same file the dev watcher reloads, so a saved
// edit and an external file edit converge on one format.

/// Serialize layout overrides to TOML text (only `Some` leaves are
/// emitted, exactly like the load path expects).
#[cfg(feature = "dev-ui")]
pub fn layout_config_to_toml(config: &LayoutOverrides) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(config)
}

/// Save layout overrides to `<data_dir>/boru-layout.toml` with an atomic
/// write (temp file + rename). The dev watcher therefore never observes a
/// partial file. Returns the final path.
#[cfg(feature = "dev-ui")]
pub fn save_layout_config(
    data_dir: &Path,
    config: &LayoutOverrides,
) -> Result<PathBuf, String> {
    let issues = validate_layout_overrides(config);
    if !issues.is_empty() {
        return Err(format!("invalid layout overrides: {}", issues.join("; ")));
    }
    let path = data_dir.join(LAYOUT_CONFIG_FILE_NAME);
    let text = layout_config_to_toml(config).map_err(|e| {
        format!("cannot serialize {}: {e}", path.display())
    })?;
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let tmp = data_dir.join(format!(
        "{}.{}.tmp",
        LAYOUT_CONFIG_FILE_NAME,
        std::process::id()
    ));
    std::fs::write(&tmp, text.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("cannot rename {} -> {}: {e}", tmp.display(), path.display()))?;
    Ok(path)
}

/// Reload layout overrides from disk for the inspector's explicit
/// "Reload Layout From Disk" action. Unlike [`load_layout_config`], a
/// missing file is an error (the panel must distinguish "no file" from
/// "reloaded defaults").
#[cfg(feature = "dev-ui")]
pub fn reload_layout_config(data_dir: &Path) -> Result<LayoutOverrides, LayoutConfigError> {
    let path = data_dir.join(LAYOUT_CONFIG_FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LayoutConfigError::NotFound { path });
        }
        Err(source) => return Err(LayoutConfigError::Io { path, source }),
    };
    let overrides = parse_layout_config(&text).map_err(|source| {
        let (line, column) = toml_line_col(&text, source.span());
        LayoutConfigError::Parse {
            path: path.clone(),
            source,
            line,
            column,
        }
    })?;
    let issues = validate_layout_overrides(&overrides);
    if !issues.is_empty() {
        return Err(LayoutConfigError::Validation { path, issues });
    }
    Ok(overrides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{
        ByTierOverrides, CardOrientation, ComposerButton, HomeLayoutMode, HomeSection,
        LayoutConfig, LayoutOverrides, ThumbnailPosition,
    };
    use crate::layout_merge::merge_layout_config;

    // ── BORU-LAYOUT-10: the shipped example file must stay valid ───────

    /// The repo-root `boru-layout.example.toml` (the documented example).
    /// `include_str!` is relative to this source file
    /// (`examples/iced_chat/layout_config.rs`), so `../..` reaches the repo
    /// root where the example lives. The test fails to COMPILE if the file
    /// is ever deleted or moved — exactly the guarantee the example needs.
    const EXAMPLE_TOML: &str = include_str!("../../boru-layout.example.toml");

    #[test]
    fn example_file_parses_and_merges_without_errors() {
        // BORU-LAYOUT-10: "Examples must parse cleanly against the schema
        // (no invalid TOML in the shipped example)."
        let cfg = parse_layout_config(EXAMPLE_TOML).expect("example file parses");

        // Every documented section id must be structurally valid (no
        // duplicates in order/visibility lists — BORU-LAYOUT-07).
        let issues = validate_layout_overrides(&cfg);
        assert!(
            issues.is_empty(),
            "example file must pass validation, got: {issues:?}"
        );

        // Merging onto the defaults must succeed with no developer
        // warnings (every example value is inside the documented ranges),
        // and the merged layout must reproduce the current appearance —
        // the example documents the baseline, not a modified layout.
        let (merged, warnings) = merge_layout_config(&LayoutConfig::default(), &cfg);
        assert!(
            warnings.is_empty(),
            "example file must merge without clamps/fallbacks, got: {warnings:?}"
        );
        assert_eq!(
            merged,
            LayoutConfig::default(),
            "example file values are the baseline and must reproduce the default layout"
        );
    }

    #[test]
    fn rejects_unsupported_card_orientation_combinations() {
        let vertical_with_left = parse_layout_config(
            "[component.video_card]\ncard_orientation = \"Vertical\"\nthumbnail_position = \"Left\"\n",
        )
        .expect("config parses");
        let issues = validate_layout_overrides(&vertical_with_left);
        assert!(issues.iter().any(|issue| issue.contains("Vertical requires")));

        let horizontal_with_top = parse_layout_config(
            "[component.shared_by_me]\ncard_orientation = \"Horizontal\"\nthumbnail_position = \"Top\"\n",
        )
        .expect("config parses");
        let issues = validate_layout_overrides(&horizontal_with_top);
        assert!(issues.iter().any(|issue| issue.contains("Horizontal requires")));
    }

    #[test]
    fn rejects_unsupported_metadata_alignment_combinations() {
        let horizontal_center = parse_layout_config(
            "[component.shared_by_me]\nmetadata_alignment = \"Center\"\n",
        )
        .expect("config parses");
        let issues = validate_layout_overrides(&horizontal_center);
        assert!(issues
            .iter()
            .any(|issue| issue.contains("metadata_alignment") && issue.contains("requires")));

        let vertical_end = parse_layout_config(
            "[component.video_card]\ncard_orientation = \"Vertical\"\nthumbnail_position = \"Top\"\nmetadata_alignment = \"End\"\n",
        )
        .expect("config parses");
        assert!(validate_layout_overrides(&vertical_end).is_empty());
    }

    #[test]
    fn example_file_covers_every_documented_group() {
        // BORU-LAYOUT-10: the example must document section order, columns,
        // visibility, spacing and responsive settings. Spot-check that the
        // shipped file actually exercises each required group.
        let cfg = parse_layout_config(EXAMPLE_TOML).expect("example file parses");

        let home = cfg.home.expect("home group documented");
        assert!(
            home.section_order.is_some(),
            "section order must be documented"
        );
        assert!(home.hidden_sections.is_some(), "visibility must be documented");
        assert!(home.mode.is_some(), "grid/list mode must be documented");
        assert!(home.grid.is_some(), "home.grid columns must be documented");
        assert!(
            home.quick_actions.is_some(),
            "quick-action columns must be documented"
        );
        assert!(home.padding.is_some(), "spacing (padding) must be documented");
        assert!(home.gaps.is_some(), "spacing (gaps) must be documented");
        assert!(
            home.card_sizing.is_some(),
            "card sizing must be documented"
        );

        let component = cfg.component.expect("component group documented");
        assert!(
            component.thumbnail_position.is_some()
                && component.metadata_alignment.is_some()
                && component.button_placement.is_some()
                && component.card_orientation.is_some(),
            "component placement leaves must be documented"
        );

        let responsive = cfg.responsive.expect("responsive group documented");
        assert!(
            responsive.narrow_max_width.is_some()
                && responsive.ultra_wide_min_width.is_some(),
            "responsive breakpoints must be documented"
        );
        let columns = responsive.home_columns.expect("per-breakpoint columns");
        assert!(
            columns.narrow.is_some() && columns.desktop.is_some() && columns.ultra_wide.is_some(),
            "per-breakpoint column counts must be documented"
        );
    }

    #[test]
    fn parse_full_config_all_groups() {
        let cfg = parse_layout_config(
            r##"
[home]
max_content_width = 1200.0
mode = "List"
section_order = ["Tunnels", "Hero"]

[home.grid]
main_portion = 3
rail_portion = 1

[home.quick_actions]
columns_wide = 4

[sidebar]
width = 310.0

[chat]
bubble_max_width = 600.0

[chat.composer]
button_order = ["Send", "Gif"]

[component]
thumbnail_position = "Top"

[component.video_card]
card_orientation = "Vertical"
thumbnail_position = "Hidden"

[tables.file_table]
size_col = 80.0

[tables.shared_table]
size = 70.0

[responsive]
narrow_max_width = 400.0
home_columns = { narrow = 1, desktop = 2, ultra_wide = 3 }
"##,
        )
        .expect("full config parses");

        let home = cfg.home.expect("home group present");
        assert_eq!(home.max_content_width, Some(1200.0));
        assert_eq!(home.mode, Some(HomeLayoutMode::List));
        assert_eq!(
            home.section_order,
            Some(vec![HomeSection::Tunnels, HomeSection::Hero])
        );
        let grid = home.grid.expect("home.grid present");
        assert_eq!(grid.main_portion, Some(3));
        assert_eq!(grid.rail_portion, Some(1));

        let sidebar = cfg.sidebar.expect("sidebar group present");
        assert_eq!(sidebar.width, Some(310.0));

        let chat = cfg.chat.expect("chat group present");
        assert_eq!(chat.bubble_max_width, Some(600.0));
        let composer = chat.composer.expect("chat.composer present");
        assert_eq!(
            composer.button_order,
            Some(vec![ComposerButton::Send, ComposerButton::Gif])
        );

        let component = cfg.component.expect("component group present");
        assert_eq!(component.thumbnail_position, Some(ThumbnailPosition::Top));
        let video_card = component.video_card.expect("component.video_card present");
        assert_eq!(
            video_card.thumbnail_position,
            Some(ThumbnailPosition::Hidden)
        );

        let tables = cfg.tables.expect("tables group present");
        let file_table = tables.file_table.expect("tables.file_table present");
        assert_eq!(file_table.size_col, Some(80.0));
        let shared = tables.shared_table.expect("tables.shared_table present");
        assert_eq!(shared.size, Some(70.0));

        let responsive = cfg.responsive.expect("responsive group present");
        assert_eq!(responsive.narrow_max_width, Some(400.0));
        let columns = responsive.home_columns.expect("home_columns present");
        assert_eq!(columns.ultra_wide, Some(3));
        assert!(columns.desktop.is_some());
    }

    #[test]
    fn parse_partial_config_missing_keys() {
        let cfg = parse_layout_config(
            r#"
[home]
max_content_width = 1200.0
"#,
        )
        .expect("partial config parses");

        let home = cfg.home.expect("home group present");
        assert_eq!(home.max_content_width, Some(1200.0));
        assert!(home.mode.is_none(), "missing leaf falls back to None");
        assert!(
            home.grid.is_none(),
            "missing nested table falls back to None"
        );

        assert!(cfg.sidebar.is_none(), "missing group falls back to None");
        assert!(cfg.chat.is_none());
        assert!(cfg.screens.is_empty(), "missing screens map is empty");
    }

    #[test]
    fn parse_empty_string_returns_empty_config() {
        let cfg = parse_layout_config("").expect("empty string parses");
        assert_eq!(cfg, LayoutOverrides::default());
        assert!(cfg.home.is_none() && cfg.sidebar.is_none() && cfg.chat.is_none());
    }

    #[test]
    fn parse_unknown_fields_are_ignored() {
        // Forward compatibility: unknown groups/fields must not break
        // parsing (old binaries ignore fields they don't know).
        let cfg = parse_layout_config(
            r#"
[home]
max_content_width = 1200.0
future_width = 9999.0

[future_group]
future_thing = 42.0
"#,
        )
        .expect("unknown fields ignored");
        assert_eq!(
            cfg.home.expect("home present").max_content_width,
            Some(1200.0)
        );
    }

    #[test]
    fn malformed_toml_surfaces_error_with_position() {
        let text = "[home\nmax_content_width = not-a-number\n";
        let err = parse_layout_config(text).expect_err("malformed TOML must fail");
        // The parse error knows the span; load_layout_config maps it.
        let span = err.span().expect("syntax error carries a span");
        assert!(span.start < text.len());
    }

    #[test]
    fn load_missing_file_is_empty_config() {
        let dir = std::env::temp_dir().join(format!("boru-layout-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        let cfg = load_layout_config(&dir).expect("missing file yields empty config");
        assert_eq!(cfg, LayoutOverrides::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_malformed_file_reports_structured_error() {
        let dir = std::env::temp_dir().join(format!("boru-layout-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        std::fs::write(dir.join(LAYOUT_CONFIG_FILE_NAME), "[home\nbad = yes\n")
            .expect("write malformed config");

        let err = load_layout_config(&dir).expect_err("malformed file must error");
        assert!(err.to_string().contains(LAYOUT_CONFIG_FILE_NAME));
        match &err {
            LayoutConfigError::Parse { line, column, .. } => {
                assert!(line.is_some(), "syntax errors carry a line");
                assert!(column.is_some(), "syntax errors carry a column");
            }
            other => panic!("expected Parse error, got {other:?}"),
        }

        // The Clone-able projection preserves path + parser detail.
        let reload = LayoutReloadError::from_layout_error(&err);
        assert!(reload.path.ends_with(LAYOUT_CONFIG_FILE_NAME));
        assert_eq!(reload.kind, LayoutReloadErrorKind::Parse);
        assert!(!reload.message.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn by_tier_overrides_serialize_round_trip() {
        // The per-tier generic leaf used by responsive.home_columns must
        // survive a TOML round trip (BORU-LAYOUT-04 responsive layer).
        let src = r#"
[responsive]
home_columns = { narrow = 1, desktop = 2, ultra_wide = 4 }
"#;
        let cfg = parse_layout_config(src).expect("parses");
        let columns: &ByTierOverrides<usize> = cfg
            .responsive
            .as_ref()
            .and_then(|r| r.home_columns.as_ref())
            .expect("home_columns present");
        assert_eq!(columns.narrow, Some(1));
        assert_eq!(columns.desktop, Some(2));
        assert_eq!(columns.ultra_wide, Some(4));

        // Serialize back and re-parse: leaves survive.
        let text = toml::to_string(&cfg).expect("serializes");
        let again = parse_layout_config(&text).expect("re-parses");
        assert_eq!(again, cfg);
    }

    // ── BORU-LAYOUT-07: semantic validation (duplicate section ids) ──────

    #[test]
    fn clean_config_passes_validation() {
        let cfg = parse_layout_config(
            r#"
[home]
section_order = ["Hero", "MeshHealth"]
hidden_sections = ["Tunnels"]

[sidebar]
section_order = ["Chats", "Friends"]

[screens.settings]
section_order = ["header", "body"]
"#,
        )
        .expect("parses");
        assert!(
            validate_layout_overrides(&cfg).is_empty(),
            "a clean config has no validation issues"
        );
    }

    #[test]
    fn duplicate_home_section_order_rejected() {
        let cfg = parse_layout_config(
            r#"
[home]
section_order = ["Tunnels", "Hero", "Tunnels"]
"#,
        )
        .expect("parses");
        let issues = validate_layout_overrides(&cfg);
        assert_eq!(issues.len(), 1, "issues: {issues:?}");
        assert!(issues[0].contains("home.section_order"), "{}", issues[0]);
        assert!(issues[0].contains("Tunnels"), "{}", issues[0]);
        assert!(issues[0].contains("duplicate"), "{}", issues[0]);
    }

    #[test]
    fn duplicate_hidden_sections_rejected() {
        let cfg = parse_layout_config(
            r#"
[home]
hidden_sections = ["Tunnels", "Tunnels"]
"#,
        )
        .expect("parses");
        let issues = validate_layout_overrides(&cfg);
        assert_eq!(issues.len(), 1, "issues: {issues:?}");
        assert!(
            issues[0].contains("home.hidden_sections"),
            "{}",
            issues[0]
        );
    }

    #[test]
    fn section_in_order_and_hidden_rejected() {
        // A section id appearing in BOTH lists is contradictory visibility —
        // rejected like any other duplicate id.
        let cfg = parse_layout_config(
            r#"
[home]
section_order = ["Hero", "Tunnels"]
hidden_sections = ["Tunnels"]
"#,
        )
        .expect("parses");
        let issues = validate_layout_overrides(&cfg);
        assert_eq!(issues.len(), 1, "issues: {issues:?}");
        assert!(issues[0].contains("Tunnels"), "{}", issues[0]);
        assert!(
            issues[0].contains("hidden_sections"),
            "{}",
            issues[0]
        );
    }

    #[test]
    fn duplicate_sidebar_section_order_rejected() {
        let cfg = parse_layout_config(
            r#"
[sidebar]
section_order = ["Chats", "Chats", "Friends"]
"#,
        )
        .expect("parses");
        let issues = validate_layout_overrides(&cfg);
        assert_eq!(issues.len(), 1, "issues: {issues:?}");
        assert!(
            issues[0].contains("sidebar.section_order"),
            "{}",
            issues[0]
        );
    }

    #[test]
    fn duplicate_screen_section_ids_rejected() {
        let cfg = parse_layout_config(
            r#"
[screens.settings]
section_order = ["header", "body", "header"]
"#,
        )
        .expect("parses");
        let issues = validate_layout_overrides(&cfg);
        assert_eq!(issues.len(), 1, "issues: {issues:?}");
        assert!(
            issues[0].contains("screens.settings.section_order"),
            "{}",
            issues[0]
        );
    }

    #[test]
    fn multiple_duplicate_issues_all_reported() {
        // Every offending list is reported, not just the first.
        let cfg = parse_layout_config(
            r#"
[home]
section_order = ["Tunnels", "Tunnels"]
hidden_sections = ["Hero", "Hero"]

[sidebar]
section_order = ["Chats", "Chats"]
"#,
        )
        .expect("parses");
        let issues = validate_layout_overrides(&cfg);
        assert_eq!(issues.len(), 3, "issues: {issues:?}");
    }

    #[test]
    fn load_duplicate_sections_reports_validation_error() {
        let dir = std::env::temp_dir().join(format!("boru-layout-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        std::fs::write(
            dir.join(LAYOUT_CONFIG_FILE_NAME),
            "[home]\nsection_order = [\"Tunnels\", \"Tunnels\"]\n",
        )
        .expect("write duplicate config");

        let err = load_layout_config(&dir).expect_err("duplicates must fail load");
        match &err {
            LayoutConfigError::Validation { issues, .. } => {
                assert_eq!(issues.len(), 1);
                assert!(issues[0].contains("duplicate"), "{}", issues[0]);
            }
            other => panic!("expected Validation error, got {other:?}"),
        }
        assert!(
            err.to_string().contains(LAYOUT_CONFIG_FILE_NAME),
            "{}",
            err
        );

        // The Clone-able projection reports the Validation kind so the app
        // can keep the last known-good layout exactly like a parse error.
        let reload = LayoutReloadError::from_layout_error(&err);
        assert_eq!(reload.kind, LayoutReloadErrorKind::Validation);
        assert!(
            reload.message.contains("duplicate"),
            "{}",
            reload.message
        );
        assert_eq!(reload.line, None);
        assert_eq!(reload.column, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Save / reload path (BORU-LAYOUT-08 / PDF Task 8) ──────

    #[cfg(feature = "dev-ui")]
    #[test]
    fn save_layout_config_writes_file_atomically_and_reload_round_trips() {
        let dir = std::env::temp_dir().join(format!("boru-layout-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut cfg = LayoutOverrides::default();
        cfg.home.get_or_insert_with(Default::default).mode = Some(HomeLayoutMode::List);
        cfg.home.get_or_insert_with(Default::default).max_content_width = Some(1200.0);
        cfg.component
            .get_or_insert_with(Default::default)
            .card_orientation = Some(CardOrientation::Vertical);
        cfg.component
            .get_or_insert_with(Default::default)
            .thumbnail_position = Some(ThumbnailPosition::Top);

        let path = save_layout_config(&dir, &cfg).expect("save succeeds");
        assert_eq!(path.file_name().unwrap(), LAYOUT_CONFIG_FILE_NAME);

        // The saved file parses back to the same overrides (round trip).
        let text = std::fs::read_to_string(&path).expect("read saved file");
        let parsed = parse_layout_config(&text).expect("saved file parses");
        assert_eq!(parsed, cfg, "save -> load round trip must be lossless");

        // The file contains the edited values and no temp sibling remains.
        assert!(text.contains("max_content_width = 1200.0"), "{text}");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "atomic write left no temp files: {leftovers:?}");

        // Reload reproduces the same overrides.
        let reloaded = reload_layout_config(&dir).expect("reload succeeds");
        assert_eq!(reloaded, cfg);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn reload_layout_config_missing_file_reports_not_found() {
        let dir = std::env::temp_dir().join(format!("boru-layout-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let err = reload_layout_config(&dir).expect_err("missing file must be an error");
        assert!(
            matches!(err, LayoutConfigError::NotFound { .. }),
            "expected NotFound, got {err:?}"
        );
        assert!(err.to_string().contains("cannot find"), "{}", err);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "dev-ui")]
    #[test]
    fn layout_config_to_toml_omits_none_leaves() {
        let cfg = LayoutOverrides::default();
        let text = layout_config_to_toml(&cfg).expect("serializes");
        // Default overrides serialize to an empty (or near-empty) doc.
        assert!(
            !text.contains("max_content_width"),
            "None leaves must not be emitted: {text}"
        );
    }
}
