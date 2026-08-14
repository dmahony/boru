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
//!   unparseable files; full validation rules land in BORU-LAYOUT-07).
//!
//! The sample file (`boru-layout.example.toml`, repo root) documents every
//! group with valid units and ranges.

use std::path::{Path, PathBuf};

use crate::layout::LayoutOverrides;

/// File name of the dev layout override file (inside the data dir).
pub const LAYOUT_CONFIG_FILE_NAME: &str = "boru-layout.toml";

/// Structured error returned when the dev layout override file cannot be
/// used. Mirrors `theme_config::UiThemeConfigError`: it carries the
/// offending path and (for parse errors) the line/column from the TOML
/// parser.
#[derive(Debug)]
pub enum LayoutConfigError {
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
}

impl std::fmt::Display for LayoutConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutConfigError::Io { path, source } => write!(
                f,
                "cannot read dev layout override {}: {source}",
                path.display()
            ),
            LayoutConfigError::Parse { path, source, .. } => {
                write!(
                    f,
                    "invalid dev layout override {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for LayoutConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LayoutConfigError::Io { source, .. } => Some(source),
            LayoutConfigError::Parse { source, .. } => Some(source),
        }
    }
}

/// Machine-readable category of a layout load failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutReloadErrorKind {
    /// The file exists but could not be read (permissions, I/O, …).
    Io,
    /// The file exists but is not valid TOML / not a valid layout config.
    Parse,
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

/// Load layout overrides from `<data_dir>/boru-layout.toml`.
///
/// - **Missing file** → `Ok(LayoutOverrides::default())` (empty overrides;
///   startup never fails because the dev file is absent).
/// - **Unreadable file** (permissions etc.) → `Err(LayoutConfigError::Io)`.
/// - **Malformed file** → `Err(LayoutConfigError::Parse)` with line/column;
///   the caller keeps the last known-good layout and logs the error.
pub fn load_layout_config(data_dir: &Path) -> Result<LayoutOverrides, LayoutConfigError> {
    let path = data_dir.join(LAYOUT_CONFIG_FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LayoutOverrides::default());
        }
        Err(source) => return Err(LayoutConfigError::Io { path, source }),
    };
    parse_layout_config(&text).map_err(|source| {
        let (line, column) = toml_line_col(&text, source.span());
        LayoutConfigError::Parse {
            path,
            source,
            line,
            column,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{
        ByTierOverrides, ComposerButton, HomeLayoutMode, HomeSection, LayoutOverrides,
        ThumbnailPosition,
    };

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
}
