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

//! BORU-UI-20 (PDF Task 20): regression test matrix for the live theme system.
//!
//! This module consolidates the live-theme regression suite into the
//! required matrix from the Live UI Editor plan (Task 20). Each test maps
//! 1:1 to a bullet in that task:
//!
//! | PDF Task 20 bullet | Test(s) here |
//! |---|---|
//! | Parsing a complete config | [`matrix_parse_complete_config`] |
//! | Parsing a partial config | [`matrix_parse_partial_config`] |
//! | Malformed TOML | [`matrix_malformed_toml`] |
//! | Out-of-range values | [`matrix_out_of_range_values`] |
//! | Missing file | [`matrix_missing_file`] |
//! | Merge-with-default behaviour | [`matrix_merge_with_defaults`] |
//! | Serialization round-trip for Save Theme | [`matrix_save_round_trip`] |
//! | Live theme changes do NOT replace core app state | `ui_theme_reload_*` in `app.rs` (conversation map, composer draft, scroll position, transfer state) |
//!
//! The in-module unit tests in `theme.rs` / `theme_config.rs` /
//! `theme_merge.rs` / `theme_watcher.rs` remain the deep per-field tests;
//! this module is the compact matrix every release must pass. It exercises
//! the public seam (`parse → merge → save → reload`) end to end so a broken
//! seam (not just a broken field) fails here.

use crate::theme::BoruTheme;
use crate::theme_config::{parse_ui_theme_config, ColorValue, UiThemeConfig, UI_CONFIG_FILE_NAME};
use crate::theme_merge::merge_ui_theme;

/// A complete config exercising every group in `UiThemeConfig`, with values
/// inside the documented ranges (same shape as the repo's
/// `boru-ui.example.toml`).
const COMPLETE_TOML: &str = r##"
[colors]
canvas = "#F7F9F8"
sidebar = "#FCFDFC"
surface = "#FFFFFF"
surface_elevated = "#FFFFFF"
primary = [0.094, 0.498, 0.314]
soft_tint_alpha = 0.08
dialog_backdrop = [0.0, 0.0, 0.0, 0.35]

[typography]
body = 15.0
page_title = 22.0
chat_message = 15.0
display_family = "Inter Tight"
chat_family = "Figtree"
chat_message_weight = "Normal"
chat_sender_weight = "Semibold"
chat_message_line_height = 1.45
body_line_height = 1.45

[spacing]
space_8 = 8.0
control_height = 40.0

[radii]
md = 10.0
card = 16.0

[icons]
md = 20.0

[avatars]
msg = 46.0

[lists]
card_row_height = 48.0

[borders]
hairline = 1.0

[responsive]
content_max_width = 720.0

[motion]
sidebar_fade_frames = 5

[sidebar]
width = 270.0
item_radius = 10.0
name_size = 15.0

[sidebar.padding]
row_x = 12.0

[home]
activity_row_height = 32.0
quick_action_gap = 20.0

[chat]
bubble_max_width = 560.0
bubble_width_ratio = 0.68

[attachments]
empty_state_height = 200.0

[attachments.file_table]
size_col = 72.0

[attachments.shared_table]
size = 64.0

[attachments.video]
narrow_breakpoint = 560.0
play_overlay_size = 64.0

[rooms]
catalogue_row_height = 52.0

[tunnels]
chip_padding_x = 6.0

[dialogs]
avatar_size = 72.0

[calls]
avatar_size = 96.0

[controls]
header_height = 52.0
"##;

/// PDF T20 bullet 1 — a complete config parses, every group present, and
/// merges onto the default theme without warnings.
#[test]
fn matrix_parse_complete_config() {
    let cfg = parse_ui_theme_config(COMPLETE_TOML).expect("complete config parses");

    // Every top-level group is present.
    assert!(cfg.colors.is_some());
    assert!(cfg.typography.is_some());
    assert!(cfg.spacing.is_some());
    assert!(cfg.radii.is_some());
    assert!(cfg.icons.is_some());
    assert!(cfg.avatars.is_some());
    assert!(cfg.lists.is_some());
    assert!(cfg.borders.is_some());
    assert!(cfg.responsive.is_some());
    assert!(cfg.motion.is_some());
    assert!(cfg.sidebar.is_some());
    assert!(cfg.home.is_some());
    assert!(cfg.chat.is_some());
    assert!(cfg.attachments.is_some());
    assert!(cfg.rooms.is_some());
    assert!(cfg.tunnels.is_some());
    assert!(cfg.dialogs.is_some());
    assert!(cfg.calls.is_some());
    assert!(cfg.controls.is_some());

    // Spot-check representative leaves across the groups.
    let colors = cfg.colors.as_ref().unwrap();
    assert_eq!(
        colors.canvas,
        Some(ColorValue {
            r: 247.0 / 255.0,
            g: 249.0 / 255.0,
            b: 248.0 / 255.0,
            a: 1.0
        })
    );
    let typography = cfg.typography.as_ref().unwrap();
    assert_eq!(typography.body, Some(15.0));
    let sidebar = cfg.sidebar.as_ref().unwrap();
    assert_eq!(sidebar.width, Some(270.0));
    let chat = cfg.chat.as_ref().unwrap();
    assert_eq!(chat.bubble_max_width, Some(560.0));

    // The full config merges with no warnings (all values in range).
    let (merged, warnings) = merge_ui_theme(&BoruTheme::default(), &cfg);
    assert!(
        warnings.is_empty(),
        "no warnings for in-range complete config: {warnings:?}"
    );
    assert_eq!(merged.sidebar.width, 270.0);
    assert_eq!(merged.chat.bubble_max_width, 560.0);
    assert_eq!(merged.typography.body, 15.0);
}

/// PDF T20 bullet 2 — a partial config parses; missing leaves/groups are
/// `None`, so the merge falls back to defaults.
#[test]
fn matrix_parse_partial_config() {
    let cfg = parse_ui_theme_config(
        r#"
[sidebar]
width = 300.0

[attachments.video]
play_overlay_size = 70.0
"#,
    )
    .expect("partial config parses");

    // Present group with a missing leaf → that leaf is None.
    let sidebar = cfg.sidebar.as_ref().expect("sidebar group present");
    assert_eq!(sidebar.width, Some(300.0));
    assert_eq!(sidebar.item_radius, None);

    // Missing groups → None.
    assert!(cfg.colors.is_none());
    assert!(cfg.home.is_none());
    assert!(cfg.rooms.is_none());
    assert!(cfg.calls.is_none());

    // Merge keeps the default for absent leaves.
    let (merged, warnings) = merge_ui_theme(&BoruTheme::default(), &cfg);
    assert!(
        warnings.is_empty(),
        "no warnings for partial config: {warnings:?}"
    );
    assert_eq!(merged.sidebar.width, 300.0, "explicit override applies");
    assert_eq!(
        merged.sidebar.item_radius,
        BoruTheme::default().sidebar.item_radius,
        "absent leaf keeps default"
    );
    assert_eq!(
        merged.colors.canvas,
        BoruTheme::default().colors.canvas,
        "absent group keeps default"
    );
}

/// PDF T20 bullet 3 — malformed TOML surfaces a parse error (not a panic),
/// and the load path wraps it with the file path + parser position.
#[test]
fn matrix_malformed_toml() {
    let err = parse_ui_theme_config("[sidebar\nwidth = 'unclosed").expect_err("malformed TOML");
    assert!(err.span().is_some(), "parser reports a byte span");

    // Load path wraps it: structured error carries the path and line/col.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join(UI_CONFIG_FILE_NAME),
        "[sidebar\nwidth = 'unclosed",
    )
    .expect("write malformed file");
    let err = crate::theme_config::load_ui_theme_config(dir.path())
        .expect_err("malformed file is an error");
    match err {
        crate::theme_config::UiThemeConfigError::Parse {
            path, line, column, ..
        } => {
            assert!(path.ends_with(UI_CONFIG_FILE_NAME));
            assert!(
                line.is_some() && column.is_some(),
                "parser position reported"
            );
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// PDF T20 bullet 4 — out-of-range values are clamped (or fall back to the
/// default) with a warning, never activated.
#[test]
fn matrix_out_of_range_values() {
    let (merged, warnings) = merge_ui_theme(
        &BoruTheme::default(),
        &parse_ui_theme_config(
            r#"
[spacing]
space_8 = -4.0
space_16 = 1.0e9

[typography]
body = 0.0

[sidebar]
width = 100000.0

[chat]
bubble_width_ratio = 5.0

[colors]
primary = [2.0, -1.0, 0.5]
"#,
        )
        .expect("out-of-range config parses"),
    );

    assert_eq!(merged.spacing.space_8, 0.0, "negative padding clamped to 0");
    assert_eq!(
        merged.spacing.space_16, 4096.0,
        "absurd size clamped to max"
    );
    assert_eq!(
        merged.typography.body,
        BoruTheme::default().typography.body,
        "zero font size falls back to default"
    );
    assert_eq!(
        merged.sidebar.width, 2000.0,
        "absurd sidebar width clamped to max"
    );
    assert_eq!(merged.chat.bubble_width_ratio, 1.0, "ratio clamped to 1");
    assert_eq!(
        merged.colors.primary,
        iced::Color::from_rgba(1.0, 0.0, 0.5, 1.0),
        "colour channels clamped to 0..=1"
    );

    // Every adjustment is reported with the offending field name.
    assert_eq!(warnings.len(), 6, "one warning per adjusted value");
    for field in [
        "spacing.space_8",
        "spacing.space_16",
        "typography.body",
        "sidebar.width",
        "chat.bubble_width_ratio",
        "colors.primary",
    ] {
        assert!(
            warnings.iter().any(|w| w.contains(field)),
            "warning names {field}: {warnings:?}"
        );
    }
}

/// PDF T20 bullet 5 — a missing `boru-ui.toml` is not an error: startup
/// loads an empty override set and the app keeps the default theme.
#[test]
fn matrix_missing_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(!dir.path().join(UI_CONFIG_FILE_NAME).exists());

    let cfg = crate::theme_config::load_ui_theme_config(dir.path()).expect("missing file is Ok");
    assert_eq!(
        cfg,
        UiThemeConfig::default(),
        "empty overrides for a missing file"
    );

    let (merged, warnings) = merge_ui_theme(&BoruTheme::default(), &cfg);
    assert!(warnings.is_empty());
    assert_eq!(merged, BoruTheme::default(), "default theme stays active");
}

/// PDF T20 bullet 6 — merge-with-default behaviour: only explicitly
/// supplied fields override; everything else keeps `BoruTheme::default()`.
#[test]
fn matrix_merge_with_defaults() {
    let cfg = parse_ui_theme_config(
        r#"
[sidebar]
width = 330.0
"#,
    )
    .expect("partial config parses");
    let (merged, warnings) = merge_ui_theme(&BoruTheme::default(), &cfg);
    assert!(warnings.is_empty(), "no warnings for in-range merge");

    assert_eq!(merged.sidebar.width, 330.0, "explicit override lands");
    // Every other group/leaf stays at the default.
    let defaults = BoruTheme::default();
    assert_eq!(merged.sidebar.width_min, defaults.sidebar.width_min);
    assert_eq!(merged.sidebar.padding.row_x, defaults.sidebar.padding.row_x);
    assert_eq!(merged.colors.canvas, defaults.colors.canvas);
    assert_eq!(merged.typography.body, defaults.typography.body);
    assert_eq!(merged.chat.bubble_max_width, defaults.chat.bubble_max_width);
    assert_eq!(
        merged.attachments.video.play_overlay_size,
        defaults.attachments.video.play_overlay_size
    );

    // Empty config merges to exactly the default (Reset All path).
    let (merged, warnings) = merge_ui_theme(&BoruTheme::default(), &UiThemeConfig::default());
    assert!(warnings.is_empty());
    assert_eq!(merged, BoruTheme::default());
}

/// PDF T20 bullet 7 — serialization round-trip for Save Theme:
/// current theme → TOML → parse → merge reproduces the same active theme.
#[cfg(feature = "dev-ui")]
#[test]
fn matrix_save_round_trip() {
    let cfg = parse_ui_theme_config(COMPLETE_TOML).expect("complete config parses");
    let (before, _) = merge_ui_theme(&BoruTheme::default(), &cfg);

    let text = crate::theme_config::ui_theme_config_to_toml(&cfg).expect("serializes");
    let reparsed = parse_ui_theme_config(&text).expect("saved text parses");
    let (after, _) = merge_ui_theme(&BoruTheme::default(), &reparsed);

    assert_eq!(before, after, "round trip preserves the active theme");
    assert_eq!(reparsed, cfg, "sparse config is preserved exactly");

    // Writing to disk and loading back is also lossless (Save Theme flow).
    let dir = tempfile::tempdir().expect("temp dir");
    crate::theme_config::save_ui_theme_config(dir.path(), &cfg).expect("save succeeds");
    let reloaded = crate::theme_config::load_ui_theme_config(dir.path()).expect("load succeeds");
    assert_eq!(reloaded, cfg, "on-disk round trip is lossless");
}
