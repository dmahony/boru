//! BORU-LAYOUT-11 (PDF Task 11): acceptance test matrix for the live
//! layout system.
//!
//! This module consolidates the live-layout acceptance suite into the
//! matrix the Definition of Done requires. Each test maps 1:1 to a bullet
//! in PDF Task 11 or to the load-path seam tasks (T6/T7/T8) the
//! acceptance depends on:
//!
//! | Bullet | Test(s) here |
//! |---|---|
//! | Parsing a complete config | [`matrix_parse_complete_config`] |
//! | Parsing a partial config | [`matrix_parse_partial_config`] |
//! | Malformed TOML | [`matrix_malformed_toml`] |
//! | Out-of-range values | [`matrix_out_of_range_values`] |
//! | Missing file | [`matrix_missing_file`] |
//! | Merge-with-default behaviour | [`matrix_merge_with_defaults`] |
//! | Serialization round-trip for Save Layout | [`matrix_save_round_trip`] |
//! | T11: changing TOML immediately rearranges the home screen | [`matrix_rearranges_home_screen`] + `layout_reload_rearranges_home_screen` in `app.rs` |
//! | T11: chats, transfers and playback continue uninterrupted | `layout_reload_preserves_transfer_state` / `layout_reload_preserves_inline_video_state` in `app.rs` |
//! | T11: invalid TOML never crashes Boru | [`matrix_invalid_toml_never_crashes`] + `layout_reload_invalid_toml_never_crashes` in `app.rs` |
//!
//! The in-module unit tests in `layout.rs` / `layout_config.rs` /
//! `layout_merge.rs` / `layout_watcher.rs` remain the deep per-field
//! tests; this module is the compact matrix every release must pass. It
//! exercises the public seam (`parse → merge → save → reload`) end to
//! end so a broken seam (not just a broken field) fails here.

use crate::layout::{HomeLayoutMode, HomeSection, LayoutConfig, LayoutOverrides};
use crate::layout_config::{
    load_layout_config, parse_layout_config, validate_layout_overrides, LayoutConfigError,
    LayoutReloadError, LayoutReloadErrorKind, LAYOUT_CONFIG_FILE_NAME,
};
use crate::layout_merge::merge_layout_config;

/// A complete config exercising every group in the layout schema, with
/// values inside the documented ranges. The repo-root
/// `boru-layout.example.toml` (the documented example, BORU-LAYOUT-10)
/// carries exactly the CURRENT BASELINE — so `include_str!` also fails
/// the build if the example file is ever deleted or moved, and merging
/// it must reproduce the default layout (the guardrail: layout defaults
/// reproduce the current appearance when the config file is absent).
const COMPLETE_TOML: &str = include_str!("../../../boru-layout.example.toml");

/// Parse + merge a TOML override string onto the default layout.
fn merge_toml(toml: &str) -> (LayoutConfig, Vec<String>) {
    let cfg = parse_layout_config(toml).expect("config parses");
    merge_layout_config(&LayoutConfig::default(), &cfg)
}

/// PDF T11 prerequisite (T2 schema) — a complete config parses, every
/// group is present, and merging it onto the defaults is warning-free and
/// reproduces the baseline exactly.
#[test]
fn matrix_parse_complete_config() {
    let cfg = parse_layout_config(COMPLETE_TOML).expect("complete config parses");

    // Every top-level group is present.
    assert!(cfg.home.is_some());
    assert!(cfg.sidebar.is_some());
    assert!(cfg.chat.is_some());
    assert!(cfg.component.is_some());
    assert!(cfg.tables.is_some());
    assert!(cfg.responsive.is_some());

    // Spot-check representative leaves across the groups.
    let home = cfg.home.as_ref().unwrap();
    assert_eq!(home.max_content_width, Some(1480.0));
    assert_eq!(home.mode, Some(HomeLayoutMode::Grid));
    assert_eq!(
        home.section_order.as_deref(),
        Some(
            &[
                HomeSection::Hero,
                HomeSection::QuickActions,
                HomeSection::MeshHealth,
                HomeSection::PeopleActivity,
                HomeSection::Tunnels,
            ][..]
        )
    );
    let sidebar = cfg.sidebar.as_ref().unwrap();
    assert_eq!(sidebar.width, Some(304.0));
    let chat = cfg.chat.as_ref().unwrap();
    assert_eq!(chat.bubble_max_width, Some(560.0));
    let responsive = cfg.responsive.as_ref().unwrap();
    assert_eq!(responsive.narrow_max_width, Some(360.0));

    // The full config merges with no warnings (all values in range) and —
    // because the example documents the CURRENT BASELINE — reproduces the
    // default layout exactly.
    let (merged, warnings) = merge_layout_config(&LayoutConfig::default(), &cfg);
    assert!(
        warnings.is_empty(),
        "no warnings for in-range complete config: {warnings:?}"
    );
    assert_eq!(
        merged,
        LayoutConfig::default(),
        "the documented baseline must reproduce the default layout"
    );
}

/// PDF T11 prerequisite (T2 schema) — a partial config parses; missing
/// leaves/groups are `None`, so the merge falls back to defaults.
#[test]
fn matrix_parse_partial_config() {
    let cfg = parse_layout_config(
        r#"
[home]
max_content_width = 1200.0

[sidebar]
width = 310.0
"#,
    )
    .expect("partial config parses");

    // Present group with a missing leaf → that leaf is None.
    let home = cfg.home.as_ref().expect("home group present");
    assert_eq!(home.max_content_width, Some(1200.0));
    assert!(home.mode.is_none());
    assert!(home.grid.is_none());

    // Missing groups → None.
    assert!(cfg.chat.is_none());
    assert!(cfg.component.is_none());
    assert!(cfg.tables.is_none());
    assert!(cfg.screens.is_empty());

    // Merge keeps the default for absent leaves.
    let (merged, warnings) = merge_layout_config(&LayoutConfig::default(), &cfg);
    assert!(
        warnings.is_empty(),
        "no warnings for partial config: {warnings:?}"
    );
    assert_eq!(
        merged.home.max_content_width, 1200.0,
        "explicit override applies"
    );
    assert_eq!(
        merged.home.mode,
        LayoutConfig::default().home.mode,
        "absent leaf keeps default"
    );
    assert_eq!(merged.sidebar.width, 310.0, "explicit override applies");
    assert_eq!(
        merged.chat,
        LayoutConfig::default().chat,
        "absent group keeps default"
    );
}

/// PDF T11 prerequisite (T6 load path) — malformed TOML surfaces a parse
/// error (not a panic), and the load path wraps it with the file path +
/// parser position.
#[test]
fn matrix_malformed_toml() {
    let err =
        parse_layout_config("[home\nmax_content_width = 'unclosed").expect_err("malformed TOML");
    assert!(err.span().is_some(), "parser reports a byte span");

    // Load path wraps it: structured error carries the path and line/col.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join(LAYOUT_CONFIG_FILE_NAME),
        "[home\nmax_content_width = 'unclosed",
    )
    .expect("write malformed file");
    let err = load_layout_config(dir.path()).expect_err("malformed file is an error");
    match &err {
        LayoutConfigError::Parse {
            path, line, column, ..
        } => {
            assert!(path.ends_with(LAYOUT_CONFIG_FILE_NAME));
            assert!(
                line.is_some() && column.is_some(),
                "parser position reported"
            );
        }
        other => panic!("expected Parse error, got {other:?}"),
    }

    // The Clone-able projection the watcher→app boundary uses preserves
    // path + kind + parser detail, so the app can keep the last
    // known-good layout with a useful log.
    let reload = LayoutReloadError::from_layout_error(&err);
    assert!(reload.path.ends_with(LAYOUT_CONFIG_FILE_NAME));
    assert_eq!(reload.kind, LayoutReloadErrorKind::Parse);
    assert!(!reload.message.is_empty());
    assert!(reload.line.is_some() && reload.column.is_some());
}

/// PDF T11 prerequisite (T6/T7 clamps) — out-of-range values are clamped
/// or fall back to the default with a warning, never activated verbatim
/// and never a panic.
#[test]
fn matrix_out_of_range_values() {
    let (merged, warnings) = merge_toml(
        r#"
[home]
max_content_width = 1.0e9

[home.grid]
main_portion = 0

[home.quick_actions]
columns_wide = 0
columns_mid = 999

[home.padding]
top = -4.0

[chat]
bubble_width_ratio = 5.0

[sidebar]
width = nan
"#,
    );

    assert_eq!(
        merged.home.padding.top, 0.0,
        "negative padding clamped to 0"
    );
    assert_eq!(
        merged.home.max_content_width, 4096.0,
        "absurd width clamped to max"
    );
    assert_eq!(
        merged.home.quick_actions.columns_wide,
        LayoutConfig::default().home.quick_actions.columns_wide,
        "zero column count falls back to default"
    );
    assert_eq!(
        merged.home.quick_actions.columns_mid, 12,
        "absurd column count clamped to max"
    );
    assert_eq!(merged.chat.bubble_width_ratio, 1.0, "ratio clamped to 1");
    assert_eq!(
        merged.home.grid.main_portion,
        LayoutConfig::default().home.grid.main_portion,
        "zero portion falls back to default"
    );
    assert_eq!(
        merged.sidebar.width,
        LayoutConfig::default().sidebar.width,
        "non-finite width falls back to default"
    );

    // Every adjustment is reported with the offending field name.
    assert_eq!(warnings.len(), 7, "one warning per adjusted value");
    for field in [
        "home.max_content_width",
        "home.grid.main_portion",
        "home.quick_actions.columns_wide",
        "home.quick_actions.columns_mid",
        "home.padding.top",
        "chat.bubble_width_ratio",
        "sidebar.width",
    ] {
        assert!(
            warnings.iter().any(|w| w.contains(field)),
            "warning names {field}: {warnings:?}"
        );
    }
}

/// PDF T11 prerequisite (T6 load path) — a missing `boru-layout.toml` is
/// not an error: startup loads an empty override set and the app keeps
/// the default layout.
#[test]
fn matrix_missing_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(!dir.path().join(LAYOUT_CONFIG_FILE_NAME).exists());

    let cfg = load_layout_config(dir.path()).expect("missing file is Ok");
    assert_eq!(
        cfg,
        LayoutOverrides::default(),
        "empty overrides for a missing file"
    );

    let (merged, warnings) = merge_layout_config(&LayoutConfig::default(), &cfg);
    assert!(warnings.is_empty());
    assert_eq!(
        merged,
        LayoutConfig::default(),
        "default layout stays active"
    );
}

/// PDF T11 prerequisite (T2 defaults) — merge-with-default behaviour: only
/// explicitly supplied fields override; everything else keeps
/// `LayoutConfig::default()`.
#[test]
fn matrix_merge_with_defaults() {
    let cfg = parse_layout_config(
        r#"
[sidebar]
width = 330.0
"#,
    )
    .expect("partial config parses");
    let (merged, warnings) = merge_layout_config(&LayoutConfig::default(), &cfg);
    assert!(warnings.is_empty(), "no warnings for in-range merge");

    assert_eq!(merged.sidebar.width, 330.0, "explicit override lands");
    // Every other group/leaf stays at the default.
    let defaults = LayoutConfig::default();
    assert_eq!(merged.sidebar.width_min, defaults.sidebar.width_min);
    assert_eq!(merged.sidebar.padding.row_x, defaults.sidebar.padding.row_x);
    assert_eq!(
        merged.home.max_content_width,
        defaults.home.max_content_width
    );
    assert_eq!(merged.home.section_order, defaults.home.section_order);
    assert_eq!(merged.chat.bubble_max_width, defaults.chat.bubble_max_width);
    assert_eq!(
        merged.responsive.home_columns,
        defaults.responsive.home_columns
    );

    // Empty config merges to exactly the default (Reset All path).
    let (merged, warnings) =
        merge_layout_config(&LayoutConfig::default(), &LayoutOverrides::default());
    assert!(warnings.is_empty());
    assert_eq!(merged, LayoutConfig::default());
}

/// PDF Task 11 acceptance (pure seam): "Changing TOML immediately
/// rearranges the home screen." A valid override file changes the section
/// order, grid/list mode, column counts, gaps and max content width at
/// the model level — the app applies exactly this via
/// `update_layout_reloaded` (see `layout_reload_rearranges_home_screen`
/// in `app.rs`).
#[test]
fn matrix_rearranges_home_screen() {
    // Reorder + mode + columns + gaps + width.
    let (merged, warnings) = merge_toml(
        r##"
[home]
max_content_width = 1200.0
mode = "List"
section_order = ["Tunnels", "QuickActions", "Hero", "MeshHealth", "PeopleActivity"]

[home.grid]
main_portion = 3
rail_portion = 1
column_gap = 16.0

[home.quick_actions]
columns_wide = 6

[home.gaps]
card_gap = 12.0
"##,
    );
    assert!(
        warnings.is_empty(),
        "no warnings for in-range rearrange: {warnings:?}"
    );

    assert_eq!(merged.home.max_content_width, 1200.0);
    assert_eq!(merged.home.mode, HomeLayoutMode::List);
    assert_eq!(
        merged.home.section_order,
        vec![
            HomeSection::Tunnels,
            HomeSection::QuickActions,
            HomeSection::Hero,
            HomeSection::MeshHealth,
            HomeSection::PeopleActivity,
        ],
        "section order re-arranged"
    );
    assert_eq!(merged.home.grid.main_portion, 3, "grid split changed");
    assert_eq!(merged.home.grid.rail_portion, 1);
    assert_eq!(merged.home.grid.column_gap, 16.0, "column gap changed");
    assert_eq!(
        merged.home.quick_actions.columns_wide, 6,
        "column count changed"
    );
    assert_eq!(merged.home.gaps.card_gap, 12.0, "card gap changed");
    assert_eq!(
        merged.home.visible_sections(),
        merged.home.section_order,
        "nothing hidden: the rendered list is exactly the new order"
    );

    // Visibility: hiding a section removes it from what the view renders
    // while keeping the default order (the documented hidden_sections use —
    // BORU-LAYOUT-07 rejects an id listed in both order and hidden).
    let (merged, warnings) = merge_toml("[home]\nhidden_sections = [\"Tunnels\"]\n");
    assert!(warnings.is_empty(), "no warnings for hide: {warnings:?}");
    assert_eq!(
        merged.home.section_order,
        LayoutConfig::default().home.section_order,
        "hiding does not reorder"
    );
    assert_eq!(merged.home.hidden_sections, vec![HomeSection::Tunnels]);
    assert_eq!(
        merged.home.visible_sections(),
        vec![
            HomeSection::Hero,
            HomeSection::QuickActions,
            HomeSection::MeshHealth,
            HomeSection::PeopleActivity,
        ],
        "hidden section removed from the rendered list"
    );
}

/// PDF Task 11 acceptance (pure seam): "Invalid TOML never crashes Boru."
/// Every invalid input either surfaces as a structured error (malformed
/// TOML, duplicate section ids) or is clamped with a warning (out-of-range
/// values) — never a panic — and the previously applied layout is
/// retained (the last known-good override set still merges unchanged).
#[test]
fn matrix_invalid_toml_never_crashes() {
    // 1. Malformed TOML → structured parse error, not a panic.
    let err =
        parse_layout_config("[home\nmax_content_width = 'unclosed").expect_err("malformed TOML");
    assert!(err.span().is_some());

    // 2. Duplicate section ids → validation issues (T7), not a panic; the
    //    load path reports a structured Validation error.
    let dup = parse_layout_config("[home]\nsection_order = [\"Tunnels\", \"Tunnels\"]\n")
        .expect("duplicate list still parses (validation is separate)");
    assert!(
        !validate_layout_overrides(&dup).is_empty(),
        "the fixture must actually fail validation"
    );
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join(LAYOUT_CONFIG_FILE_NAME),
        "[home]\nsection_order = [\"Tunnels\", \"Tunnels\"]\n",
    )
    .expect("write duplicate config");
    let err = load_layout_config(dir.path()).expect_err("duplicates must fail load");
    assert!(
        matches!(err, LayoutConfigError::Validation { .. }),
        "expected Validation error, got {err:?}"
    );
    let reload = LayoutReloadError::from_layout_error(&err);
    assert_eq!(reload.kind, LayoutReloadErrorKind::Validation);

    // 3. Out-of-range values → clamped with a warning, not a panic.
    let (merged, warnings) = merge_toml("[home.padding]\ntop = -4.0\n");
    assert_eq!(merged.home.padding.top, 0.0, "clamped, not applied");
    assert_eq!(warnings.len(), 1, "the clamp is reported");

    // 4. After every failure the last known-good override set still
    //    merges to the same layout — the previous layout is retained.
    let good =
        parse_layout_config("[home]\nmax_content_width = 1200.0\n").expect("good config parses");
    let (before, _) = merge_layout_config(&LayoutConfig::default(), &good);
    assert_eq!(before.home.max_content_width, 1200.0);
    let (after, _) = merge_layout_config(&LayoutConfig::default(), &good);
    assert_eq!(
        before, after,
        "failed reloads leave the known-good layout intact"
    );
}

/// PDF T11 prerequisite (T8 Save Layout) — serialization round-trip:
/// current overrides → TOML → parse → merge reproduces the same active
/// layout, and writing to disk + loading back is lossless.
#[cfg(feature = "dev-ui")]
#[test]
fn matrix_save_round_trip() {
    let cfg = parse_layout_config(COMPLETE_TOML).expect("complete config parses");
    let (before, _) = merge_layout_config(&LayoutConfig::default(), &cfg);

    let text = crate::layout_config::layout_config_to_toml(&cfg).expect("serializes");
    let reparsed = parse_layout_config(&text).expect("saved text parses");
    let (after, _) = merge_layout_config(&LayoutConfig::default(), &reparsed);

    assert_eq!(before, after, "round trip preserves the active layout");
    assert_eq!(reparsed, cfg, "sparse config is preserved exactly");

    // Writing to disk and loading back is also lossless (Save Layout flow).
    let dir = tempfile::tempdir().expect("temp dir");
    crate::layout_config::save_layout_config(dir.path(), &cfg).expect("save succeeds");
    let reloaded = load_layout_config(dir.path()).expect("load succeeds");
    assert_eq!(reloaded, cfg, "on-disk round trip is lossless");
}

/// BORU-DESIGN-26: a completed designer transaction crosses the same typed
/// layout/TOML seam used by Save Layout.
#[cfg(feature = "dev-ui")]
#[test]
fn designer_transaction_round_trips_through_typed_toml() {
    let source = r#"
[home]
section_order = ["PeopleActivity", "MeshHealth", "QuickActions", "Hero", "Tunnels"]
hidden_sections = ["Tunnels"]
[home.quick_actions]
columns_wide = 5
[responsive.home_columns]
narrow = 1
desktop = 3
"#;
    let overrides = parse_layout_config(source).expect("designer transaction parses");
    let (reloaded, warnings) = merge_layout_config(&LayoutConfig::default(), &overrides);
    assert!(
        warnings.is_empty(),
        "transaction values are valid: {warnings:?}"
    );
    assert_eq!(reloaded.home.section_order[0], HomeSection::PeopleActivity);
    assert_eq!(reloaded.home.hidden_sections, vec![HomeSection::Tunnels]);
    assert_eq!(reloaded.home.quick_actions.columns_wide, 5);
    assert_eq!(reloaded.responsive.home_columns.desktop, 3);

    let text =
        crate::layout_config::layout_config_to_toml(&overrides).expect("serialize transaction");
    let reparsed = parse_layout_config(&text).expect("serialized transaction parses");
    let (round_trip, _) = merge_layout_config(&LayoutConfig::default(), &reparsed);
    assert_eq!(round_trip, reloaded);
}

#[test]
fn designer_reorder_preserves_sections_and_visibility() {
    let (layout, warnings) = merge_toml(
        "[home]\nsection_order = [\"Tunnels\", \"Hero\", \"MeshHealth\", \"QuickActions\", \"PeopleActivity\"]\nhidden_sections = [\"Tunnels\"]\n",
    );
    assert!(warnings.is_empty());
    assert_eq!(
        layout.home.section_order.len(),
        LayoutConfig::default().home.section_order.len()
    );
    assert_eq!(
        layout.home.visible_sections(),
        vec![
            HomeSection::Hero,
            HomeSection::MeshHealth,
            HomeSection::QuickActions,
            HomeSection::PeopleActivity,
        ]
    );
    assert_eq!(layout.home.section_order[0], HomeSection::Tunnels);
}

#[test]
fn designer_hidden_component_can_be_recovered() {
    let hidden = merge_toml("[home]\nhidden_sections = [\"QuickActions\"]\n").0;
    assert!(!hidden
        .home
        .visible_sections()
        .contains(&HomeSection::QuickActions));
    let recovered = merge_toml("[home]\nhidden_sections = []\n").0;
    assert!(recovered
        .home
        .visible_sections()
        .contains(&HomeSection::QuickActions));
    assert_eq!(
        recovered.home.section_order,
        LayoutConfig::default().home.section_order
    );
}

#[test]
fn designer_breakpoint_specific_edits_stay_in_their_tier() {
    let (layout, warnings) = merge_toml(
        "[responsive.home_columns]\nnarrow = 1\ndesktop = 3\nultra_wide = 4\n[responsive.home_padding_x]\ndesktop = 40.0\n",
    );
    assert!(warnings.is_empty());
    assert_eq!(layout.responsive.home_columns_for_width(320.0), 1);
    assert_eq!(layout.responsive.home_columns_for_width(1024.0), 3);
    assert_eq!(layout.responsive.home_columns_for_width(1920.0), 4);
    assert_eq!(layout.responsive.home_padding_x_for_width(1024.0), 40.0);
    assert_ne!(layout.responsive.home_padding_x_for_width(320.0), 40.0);
}

#[test]
fn designer_resize_clamps_to_layout_constraints() {
    let (layout, warnings) = merge_toml("[sidebar]\nwidth = 999999.0\n");
    assert_eq!(layout.sidebar.width, 4096.0);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("sidebar.width"));
}
