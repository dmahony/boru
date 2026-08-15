//! Pure merge of `boru-layout.toml` overrides onto [`LayoutConfig`]
//! (BORU-LAYOUT-06 / PDF Task 6).
//!
//! This module implements the layout load path:
//!
//! ```text
//! LayoutConfig::default()  +  LayoutOverrides  ->  LayoutConfig
//! ```
//!
//! ## Design rules
//!
//! - **Defaults are the source of truth.** Every override leaf is
//!   `Option<T>`; a `None` keeps the corresponding `LayoutConfig` value.
//!   Only explicitly supplied TOML fields override.
//! - **Pure, no I/O.** [`merge_layout_config`] takes an already-parsed
//!   `LayoutOverrides`; file loading stays in
//!   `layout_config::load_layout_config`.
//! - **Only validated layouts are applied.** Unparseable files never reach
//!   this module (the watcher/loader rejects them and the app keeps the
//!   last known-good layout). Values that are structurally unsafe (negative
//!   padding, absurd widths, zero column counts, non-finite floats) are
//!   clamped or replaced by the field default, mirroring
//!   `theme_merge.rs`; every adjustment is reported as a developer warning
//!   string so the caller can log it. Semantic validation (duplicate
//!   section ids in the order/visibility lists) lives in
//!   `layout_config::validate_layout_overrides` (BORU-LAYOUT-07) and runs
//!   before merge — this module never sees an invalid section list.
//! - **Backward compatible.** Older partial layout files (fewer groups /
//!   fewer fields) deserialize to `None` leaves and merge unchanged;
//!   unknown fields are ignored by serde.
//! - **Defaults reproduce the current appearance.** `merge_layout_config`
//!   on an empty config is the identity — the UI is unchanged when
//!   `boru-layout.toml` is absent.

use std::collections::BTreeMap;

use crate::layout::{
    ByTier, ByTierOverrides, CardOrientation, ChatLayout, ChatOverrides, ComponentLayout,
    ComponentOverrides, ComponentPlacement, ComponentPlacementOverrides, ComposerButton,
    ComposerLayout, ComposerOverrides, FileTableColumns, FileTableOverrides, GifPickerLayout,
    GifPickerOverrides, HomeCardSizing, HomeCardSizingOverrides, HomeGaps, HomeGapsOverrides,
    HomeGrid, HomeGridOverrides, HomeLayout, HomeLayoutMode, HomeOverrides, HomePadding,
    HomePaddingOverrides, LayoutConfig, LayoutOverrides, MemberListLayout, MemberListOverrides,
    MetadataAlignment, PickerLayout, PickerOverrides, QuickActionsLayout, QuickActionsOverrides,
    ResponsiveLayout, ResponsiveOverrides, ScreenLayout, ScreenOverrides, ScreenShareLayout,
    ScreenShareOverrides, SharedTableColumns, SharedTableOverrides, SidebarLayout,
    SidebarOverrides, SidebarPadding, SidebarPaddingOverrides, SidebarRowHeights,
    SidebarRowHeightsOverrides, TablesLayout, TablesOverrides, ThumbnailPosition, VideoCardLayout,
    VideoCardOverrides,
};

// ── Validation bounds ─────────────────────────────────────────────────
//
// Sane ranges for structural layout values. Values outside these are
// developer errors: they are clamped where a clamp is meaningful (negative
// padding, absurd widths) or replaced by the field default where it is not
// (zero column count, NaN). Full cross-field validation is BORU-LAYOUT-07.

/// Any px size above this is absurd and gets clamped (widths, padding,
/// gaps, row heights, breakpoints, …).
const MAX_SIZE_PX: f32 = 4096.0;
/// Positive sizes (max content widths, breakpoints): must be > 0; a value
/// of 0 is not a valid positive size and falls back to the default.
const MIN_POSITIVE_PX: f32 = 1.0;
/// Fractions / ratios (bubble width ratio): 0..=1.
const MAX_FRACTION: f32 = 1.0;
/// Column counts (home quick actions, per-tier home columns, screen
/// columns): at least 1, at most 12 — beyond that is absurd.
const MAX_COLUMNS: usize = 12;
/// FillPortion split values (main/rail, member-list rows): at least 1, at
/// most 64.
const MAX_PORTION: u16 = 64;

/// Clamp an f32 size that may legitimately be 0 (padding, gaps).
fn clamp_size0(field: &str, v: f32, default: f32, warnings: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        warnings.push(format!(
            "{field}: {v} is not finite; using default {default}"
        ));
        return default;
    }
    if v < 0.0 {
        warnings.push(format!("{field}: {v} is negative; clamped to 0"));
        return 0.0;
    }
    if v > MAX_SIZE_PX {
        warnings.push(format!("{field}: {v} is absurd; clamped to {MAX_SIZE_PX}"));
        return MAX_SIZE_PX;
    }
    v
}

/// Clamp an f32 size that must be positive (max content widths,
/// breakpoints). 0 / negative is invalid and falls back to the default.
fn clamp_size_pos(field: &str, v: f32, default: f32, warnings: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        warnings.push(format!(
            "{field}: {v} is not finite; using default {default}"
        ));
        return default;
    }
    if v < MIN_POSITIVE_PX {
        warnings.push(format!(
            "{field}: {v} is not a valid positive size; using default {default}"
        ));
        return default;
    }
    if v > MAX_SIZE_PX {
        warnings.push(format!("{field}: {v} is absurd; clamped to {MAX_SIZE_PX}"));
        return MAX_SIZE_PX;
    }
    v
}

/// Clamp a fraction / ratio to 0..=1 (bubble width ratio).
fn clamp_fraction(field: &str, v: f32, default: f32, warnings: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        warnings.push(format!(
            "{field}: {v} is not finite; using default {default}"
        ));
        return default;
    }
    if !(0.0..=MAX_FRACTION).contains(&v) {
        let clamped = v.clamp(0.0, MAX_FRACTION);
        warnings.push(format!(
            "{field}: {v} is outside 0..=1; clamped to {clamped}"
        ));
        return clamped;
    }
    v
}

/// Clamp a column count to 1..=MAX_COLUMNS.
fn clamp_columns(field: &str, v: usize, default: usize, warnings: &mut Vec<String>) -> usize {
    if v == 0 {
        warnings.push(format!(
            "{field}: {v} is not a valid column count; using default {default}"
        ));
        return default;
    }
    if v > MAX_COLUMNS {
        warnings.push(format!("{field}: {v} is absurd; clamped to {MAX_COLUMNS}"));
        return MAX_COLUMNS;
    }
    v
}

/// Clamp a FillPortion split to 1..=MAX_PORTION.
fn clamp_portion(field: &str, v: u16, default: u16, warnings: &mut Vec<String>) -> u16 {
    if v == 0 {
        warnings.push(format!(
            "{field}: {v} is not a valid portion; using default {default}"
        ));
        return default;
    }
    if v > MAX_PORTION {
        warnings.push(format!("{field}: {v} is absurd; clamped to {MAX_PORTION}"));
        return MAX_PORTION;
    }
    v
}

/// Identity policy for typed enums, vectors and strings — the type system
/// already validated them, so the configured value is used as-is.
fn clamp_identity<T: Clone>(_field: &str, v: T, _default: T, _warnings: &mut Vec<String>) -> T {
    v
}

// ── Flat leaf merge macro ─────────────────────────────────────────────
//
// Generates `fn merge_<name>(base: &<Leaf>, cfg: &<LeafOverrides>,
// warnings) -> <Leaf>` that applies every configured leaf through its
// validation policy. Field names MUST match between the layout leaf and
// the override struct (guaranteed by the shared naming convention).

macro_rules! merge_leaf {
    ($fn:ident, $leaf:ident, $cfg:ident, $prefix:literal, { $($field:ident: $policy:ident),* $(,)? }) => {
        fn $fn(base: &$leaf, cfg: &$cfg, warnings: &mut Vec<String>) -> $leaf {
            $leaf {
                $(
                    $field: match cfg.$field.as_ref() {
                        Some(v) => $policy(concat!($prefix, ".", stringify!($field)), v.clone(), base.$field, warnings),
                        None => base.$field,
                    },
                )*
            }
        }
    };
}

// ── Home leaves ───────────────────────────────────────────────────────

merge_leaf! {
    merge_home_grid, HomeGrid, HomeGridOverrides, "home.grid", {
        main_portion: clamp_portion,
        rail_portion: clamp_portion,
        column_gap: clamp_size0,
        stack_breakpoint: clamp_size_pos,
    }
}

merge_leaf! {
    merge_quick_actions, QuickActionsLayout, QuickActionsOverrides, "home.quick_actions", {
        columns_wide: clamp_columns,
        columns_mid: clamp_columns,
        columns_narrow: clamp_columns,
        four_col_breakpoint: clamp_size_pos,
        two_col_breakpoint: clamp_size_pos,
        card_padding_y: clamp_size0,
        card_padding_x: clamp_size0,
        gap: clamp_size0,
    }
}

merge_leaf! {
    merge_home_padding, HomePadding, HomePaddingOverrides, "home.padding", {
        top: clamp_size0,
        bottom: clamp_size0,
        horizontal_large: clamp_size0,
        horizontal_default: clamp_size0,
    }
}

merge_leaf! {
    merge_home_gaps, HomeGaps, HomeGapsOverrides, "home.gaps", {
        card_gap: clamp_size0,
        hero_gap: clamp_size0,
        header_dashboard_gap: clamp_size0,
        footer_gap: clamp_size0,
        compact_header_stack_gap: clamp_size0,
    }
}

merge_leaf! {
    merge_home_card_sizing, HomeCardSizing, HomeCardSizingOverrides, "home.card_sizing", {
        peers_body_min: clamp_size0,
        activity_row_height: clamp_size0,
        quick_action_icon_size: clamp_size0,
        status_card_min_content_height: clamp_size0,
        status_card_medium_content: clamp_size_pos,
        status_card_narrow_content: clamp_size_pos,
        status_card_mesh_hide_content: clamp_size_pos,
        status_card_text_min_width: clamp_size_pos,
        status_card_text_min_width_medium: clamp_size_pos,
        status_card_mesh_max_width: clamp_size0,
        status_card_padding_x: clamp_size0,
        status_icon_text_gap_full: clamp_size0,
        status_icon_text_gap_medium: clamp_size0,
        status_text_graph_gap_full: clamp_size0,
        status_text_graph_gap_medium: clamp_size0,
        status_divider_width: clamp_size0,
        status_divider_height: clamp_size0,
    }
}

fn merge_home(base: &HomeLayout, cfg: &HomeOverrides, warnings: &mut Vec<String>) -> HomeLayout {
    HomeLayout {
        section_order: cfg
            .section_order
            .as_ref()
            .map_or_else(|| base.section_order.clone(), |v| v.clone()),
        hidden_sections: cfg
            .hidden_sections
            .as_ref()
            .map_or_else(|| base.hidden_sections.clone(), |v| v.clone()),
        mode: cfg.mode.map_or(base.mode, |v| {
            clamp_identity("home.mode", v, base.mode, warnings)
        }),
        grid: match &cfg.grid {
            Some(g) => merge_home_grid(&base.grid, g, warnings),
            None => base.grid,
        },
        quick_actions: match &cfg.quick_actions {
            Some(q) => merge_quick_actions(&base.quick_actions, q, warnings),
            None => base.quick_actions,
        },
        max_content_width: cfg.max_content_width.map_or(base.max_content_width, |v| {
            clamp_size_pos(
                "home.max_content_width",
                v,
                base.max_content_width,
                warnings,
            )
        }),
        padding: match &cfg.padding {
            Some(p) => merge_home_padding(&base.padding, p, warnings),
            None => base.padding,
        },
        gaps: match &cfg.gaps {
            Some(g) => merge_home_gaps(&base.gaps, g, warnings),
            None => base.gaps,
        },
        card_sizing: match &cfg.card_sizing {
            Some(c) => merge_home_card_sizing(&base.card_sizing, c, warnings),
            None => base.card_sizing,
        },
    }
}

// ── Sidebar leaves ────────────────────────────────────────────────────

merge_leaf! {
    merge_sidebar_padding, SidebarPadding, SidebarPaddingOverrides, "sidebar.padding", {
        brand_top: clamp_size0,
        brand_bottom: clamp_size0,
        identity_top: clamp_size0,
        identity_bottom: clamp_size0,
        section_top: clamp_size0,
        utility_top: clamp_size0,
        utility_bottom: clamp_size0,
        row_x: clamp_size0,
        join_top: clamp_size0,
        join_bottom: clamp_size0,
    }
}

merge_leaf! {
    merge_sidebar_row_heights, SidebarRowHeights, SidebarRowHeightsOverrides, "sidebar.row_heights", {
        conversation_row: clamp_size0,
        peer_row: clamp_size0,
        peer_panel_max_height: clamp_size_pos,
        default_list_max_height: clamp_size_pos,
    }
}

fn merge_sidebar(
    base: &SidebarLayout,
    cfg: &SidebarOverrides,
    warnings: &mut Vec<String>,
) -> SidebarLayout {
    SidebarLayout {
        width: cfg.width.map_or(base.width, |v| {
            clamp_size_pos("sidebar.width", v, base.width, warnings)
        }),
        width_min: cfg.width_min.map_or(base.width_min, |v| {
            clamp_size_pos("sidebar.width_min", v, base.width_min, warnings)
        }),
        width_max: cfg.width_max.map_or(base.width_max, |v| {
            clamp_size_pos("sidebar.width_max", v, base.width_max, warnings)
        }),
        inset: cfg.inset.map_or(base.inset, |v| {
            clamp_size0("sidebar.inset", v, base.inset, warnings)
        }),
        section_order: cfg
            .section_order
            .as_ref()
            .map_or_else(|| base.section_order.clone(), |v| v.clone()),
        hidden_sections: cfg
            .hidden_sections
            .as_ref()
            .map_or_else(|| base.hidden_sections.clone(), |v| v.clone()),
        padding: match &cfg.padding {
            Some(p) => merge_sidebar_padding(&base.padding, p, warnings),
            None => base.padding,
        },
        row_heights: match &cfg.row_heights {
            Some(r) => merge_sidebar_row_heights(&base.row_heights, r, warnings),
            None => base.row_heights,
        },
    }
}

// ── Chat leaves ───────────────────────────────────────────────────────

merge_leaf! {
    merge_picker, PickerLayout, PickerOverrides, "chat.emoji_picker", {
        width: clamp_size0,
        scroll_height: clamp_size0,
    }
}

merge_leaf! {
    merge_gif_picker, GifPickerLayout, GifPickerOverrides, "chat.gif_picker", {
        width: clamp_size0,
        scroll_height: clamp_size0,
        thumbnail_width: clamp_size0,
        thumbnail_height: clamp_size0,
    }
}

merge_leaf! {
    merge_screen_share, ScreenShareLayout, ScreenShareOverrides, "chat.screen_share", {
        width: clamp_size0,
        height: clamp_size0,
    }
}

fn merge_composer(
    base: &ComposerLayout,
    cfg: &ComposerOverrides,
    warnings: &mut Vec<String>,
) -> ComposerLayout {
    ComposerLayout {
        button_order: cfg
            .button_order
            .as_ref()
            .map_or_else(|| base.button_order.clone(), |v| v.clone()),
        spacing: cfg.spacing.map_or(base.spacing, |v| {
            clamp_size0("chat.composer.spacing", v, base.spacing, warnings)
        }),
        padding: cfg.padding.map_or(base.padding, |v| {
            clamp_size0("chat.composer.padding", v, base.padding, warnings)
        }),
    }
}

merge_leaf! {
    merge_member_list, MemberListLayout, MemberListOverrides, "chat.member_list", {
        width: clamp_size0,
        max_height: clamp_size0,
        name_portion: clamp_portion,
        role_portion: clamp_portion,
    }
}

fn merge_chat(base: &ChatLayout, cfg: &ChatOverrides, warnings: &mut Vec<String>) -> ChatLayout {
    ChatLayout {
        bubble_max_width: cfg.bubble_max_width.map_or(base.bubble_max_width, |v| {
            clamp_size0("chat.bubble_max_width", v, base.bubble_max_width, warnings)
        }),
        bubble_width_ratio: cfg.bubble_width_ratio.map_or(base.bubble_width_ratio, |v| {
            clamp_fraction(
                "chat.bubble_width_ratio",
                v,
                base.bubble_width_ratio,
                warnings,
            )
        }),
        message_max_width: cfg.message_max_width.map_or(base.message_max_width, |v| {
            clamp_size0(
                "chat.message_max_width",
                v,
                base.message_max_width,
                warnings,
            )
        }),
        image_preview_max_width: cfg.image_preview_max_width.map_or(
            base.image_preview_max_width,
            |v| {
                clamp_size0(
                    "chat.image_preview_max_width",
                    v,
                    base.image_preview_max_width,
                    warnings,
                )
            },
        ),
        image_preview_max_height: cfg.image_preview_max_height.map_or(
            base.image_preview_max_height,
            |v| {
                clamp_size0(
                    "chat.image_preview_max_height",
                    v,
                    base.image_preview_max_height,
                    warnings,
                )
            },
        ),
        context_menu_width: cfg.context_menu_width.map_or(base.context_menu_width, |v| {
            clamp_size0(
                "chat.context_menu_width",
                v,
                base.context_menu_width,
                warnings,
            )
        }),
        details_panel_width: cfg
            .details_panel_width
            .map_or(base.details_panel_width, |v| {
                clamp_size0(
                    "chat.details_panel_width",
                    v,
                    base.details_panel_width,
                    warnings,
                )
            }),
        emoji_picker: match &cfg.emoji_picker {
            Some(p) => merge_picker(&base.emoji_picker, p, warnings),
            None => base.emoji_picker,
        },
        gif_picker: match &cfg.gif_picker {
            Some(g) => merge_gif_picker(&base.gif_picker, g, warnings),
            None => base.gif_picker,
        },
        screen_share: match &cfg.screen_share {
            Some(s) => merge_screen_share(&base.screen_share, s, warnings),
            None => base.screen_share,
        },
        composer: match &cfg.composer {
            Some(c) => merge_composer(&base.composer, c, warnings),
            None => base.composer.clone(),
        },
        member_list: match &cfg.member_list {
            Some(m) => merge_member_list(&base.member_list, m, warnings),
            None => base.member_list,
        },
    }
}

// ── Component leaves (PDF Task 5) ─────────────────────────────────────

merge_leaf! {
    merge_component_placement, ComponentPlacement, ComponentPlacementOverrides, "component", {
        thumbnail_position: clamp_identity,
        metadata_alignment: clamp_identity,
        button_placement: clamp_identity,
        card_orientation: clamp_identity,
    }
}

merge_leaf! {
    merge_video_card, VideoCardLayout, VideoCardOverrides, "component.video", {
        narrow_breakpoint: clamp_size0,
        medium_breakpoint: clamp_size0,
        play_overlay_size: clamp_size0,
        header_filename_max_width: clamp_size0,
        controls_slider_width: clamp_size0,
    }
}

fn merge_component(
    base: &ComponentLayout,
    cfg: &ComponentOverrides,
    warnings: &mut Vec<String>,
) -> ComponentLayout {
    ComponentLayout {
        thumbnail_position: cfg.thumbnail_position.map_or(base.thumbnail_position, |v| {
            clamp_identity(
                "component.thumbnail_position",
                v,
                base.thumbnail_position,
                warnings,
            )
        }),
        metadata_alignment: cfg.metadata_alignment.map_or(base.metadata_alignment, |v| {
            clamp_identity(
                "component.metadata_alignment",
                v,
                base.metadata_alignment,
                warnings,
            )
        }),
        button_placement: cfg.button_placement.map_or(base.button_placement, |v| {
            clamp_identity(
                "component.button_placement",
                v,
                base.button_placement,
                warnings,
            )
        }),
        card_orientation: cfg.card_orientation.map_or(base.card_orientation, |v| {
            clamp_identity(
                "component.card_orientation",
                v,
                base.card_orientation,
                warnings,
            )
        }),
        video_card: match &cfg.video_card {
            Some(v) => merge_component_placement(&base.video_card, v, warnings),
            None => base.video_card,
        },
        shared_by_me: match &cfg.shared_by_me {
            Some(v) => merge_component_placement(&base.shared_by_me, v, warnings),
            None => base.shared_by_me,
        },
        video: match &cfg.video {
            Some(v) => merge_video_card(&base.video, v, warnings),
            None => base.video,
        },
    }
}

// ── Tables leaves ─────────────────────────────────────────────────────

merge_leaf! {
    merge_file_table, FileTableColumns, FileTableOverrides, "tables.file_table", {
        size_col: clamp_size0,
        source_col: clamp_size0,
        ago_col: clamp_size0,
        peer_col: clamp_size0,
        started_col: clamp_size0,
        state_col: clamp_size0,
        direction_col: clamp_size0,
        event_col: clamp_size0,
        details_col: clamp_size0,
        download_started_col: clamp_size0,
        download_state_col: clamp_size0,
        activity_ago_col: clamp_size0,
    }
}

merge_leaf! {
    merge_shared_table, SharedTableColumns, SharedTableOverrides, "tables.shared_table", {
        shared_with: clamp_size0,
        size: clamp_size0,
        shared_on: clamp_size0,
        downloads: clamp_size0,
        actions: clamp_size0,
    }
}

fn merge_tables(
    base: &TablesLayout,
    cfg: &TablesOverrides,
    warnings: &mut Vec<String>,
) -> TablesLayout {
    TablesLayout {
        file_table: match &cfg.file_table {
            Some(t) => merge_file_table(&base.file_table, t, warnings),
            None => base.file_table,
        },
        shared_table: match &cfg.shared_table {
            Some(t) => merge_shared_table(&base.shared_table, t, warnings),
            None => base.shared_table,
        },
    }
}

// ── Responsive leaves (PDF Task 4) ────────────────────────────────────

/// Merge one tier leaf through its policy; the tier name is appended to the
/// field path for warnings (`responsive.home_columns.narrow`).
fn merge_by_tier<T: Copy>(
    field: &str,
    base: ByTier<T>,
    cfg: &ByTierOverrides<T>,
    policy: impl Fn(&str, T, T, &mut Vec<String>) -> T,
    warnings: &mut Vec<String>,
) -> ByTier<T> {
    ByTier {
        narrow: match cfg.narrow {
            Some(v) => policy(&format!("{field}.narrow"), v, base.narrow, warnings),
            None => base.narrow,
        },
        desktop: match cfg.desktop {
            Some(v) => policy(&format!("{field}.desktop"), v, base.desktop, warnings),
            None => base.desktop,
        },
        ultra_wide: match cfg.ultra_wide {
            Some(v) => policy(&format!("{field}.ultra_wide"), v, base.ultra_wide, warnings),
            None => base.ultra_wide,
        },
    }
}

fn merge_responsive(
    base: &ResponsiveLayout,
    cfg: &ResponsiveOverrides,
    warnings: &mut Vec<String>,
) -> ResponsiveLayout {
    ResponsiveLayout {
        viewport_ref_width: cfg.viewport_ref_width.map_or(base.viewport_ref_width, |v| {
            clamp_size_pos(
                "responsive.viewport_ref_width",
                v,
                base.viewport_ref_width,
                warnings,
            )
        }),
        viewport_ref_height: cfg
            .viewport_ref_height
            .map_or(base.viewport_ref_height, |v| {
                clamp_size_pos(
                    "responsive.viewport_ref_height",
                    v,
                    base.viewport_ref_height,
                    warnings,
                )
            }),
        viewport_min_width: cfg.viewport_min_width.map_or(base.viewport_min_width, |v| {
            clamp_size_pos(
                "responsive.viewport_min_width",
                v,
                base.viewport_min_width,
                warnings,
            )
        }),
        viewport_min_height: cfg
            .viewport_min_height
            .map_or(base.viewport_min_height, |v| {
                clamp_size_pos(
                    "responsive.viewport_min_height",
                    v,
                    base.viewport_min_height,
                    warnings,
                )
            }),
        viewport_lg_width: cfg.viewport_lg_width.map_or(base.viewport_lg_width, |v| {
            clamp_size_pos(
                "responsive.viewport_lg_width",
                v,
                base.viewport_lg_width,
                warnings,
            )
        }),
        viewport_lg_height: cfg.viewport_lg_height.map_or(base.viewport_lg_height, |v| {
            clamp_size_pos(
                "responsive.viewport_lg_height",
                v,
                base.viewport_lg_height,
                warnings,
            )
        }),
        viewport_xl_width: cfg.viewport_xl_width.map_or(base.viewport_xl_width, |v| {
            clamp_size_pos(
                "responsive.viewport_xl_width",
                v,
                base.viewport_xl_width,
                warnings,
            )
        }),
        viewport_xl_height: cfg.viewport_xl_height.map_or(base.viewport_xl_height, |v| {
            clamp_size_pos(
                "responsive.viewport_xl_height",
                v,
                base.viewport_xl_height,
                warnings,
            )
        }),
        content_max_width: cfg.content_max_width.map_or(base.content_max_width, |v| {
            clamp_size_pos(
                "responsive.content_max_width",
                v,
                base.content_max_width,
                warnings,
            )
        }),
        home_illustration_full_content: cfg.home_illustration_full_content.map_or(
            base.home_illustration_full_content,
            |v| {
                clamp_size_pos(
                    "responsive.home_illustration_full_content",
                    v,
                    base.home_illustration_full_content,
                    warnings,
                )
            },
        ),
        home_illustration_hide_content: cfg.home_illustration_hide_content.map_or(
            base.home_illustration_hide_content,
            |v| {
                clamp_size_pos(
                    "responsive.home_illustration_hide_content",
                    v,
                    base.home_illustration_hide_content,
                    warnings,
                )
            },
        ),
        home_compact_header_content: cfg.home_compact_header_content.map_or(
            base.home_compact_header_content,
            |v| {
                clamp_size_pos(
                    "responsive.home_compact_header_content",
                    v,
                    base.home_compact_header_content,
                    warnings,
                )
            },
        ),
        narrow_max_width: cfg.narrow_max_width.map_or(base.narrow_max_width, |v| {
            clamp_size_pos(
                "responsive.narrow_max_width",
                v,
                base.narrow_max_width,
                warnings,
            )
        }),
        ultra_wide_min_width: cfg
            .ultra_wide_min_width
            .map_or(base.ultra_wide_min_width, |v| {
                clamp_size_pos(
                    "responsive.ultra_wide_min_width",
                    v,
                    base.ultra_wide_min_width,
                    warnings,
                )
            }),
        home_columns: match &cfg.home_columns {
            Some(c) => merge_by_tier(
                "responsive.home_columns",
                base.home_columns,
                c,
                clamp_columns,
                warnings,
            ),
            None => base.home_columns,
        },
        home_padding_x: match &cfg.home_padding_x {
            Some(p) => merge_by_tier(
                "responsive.home_padding_x",
                base.home_padding_x,
                p,
                clamp_size0,
                warnings,
            ),
            None => base.home_padding_x,
        },
    }
}

// ── Future screens (extension point) ──────────────────────────────────

fn merge_screen(
    base: &ScreenLayout,
    cfg: &ScreenOverrides,
    warnings: &mut Vec<String>,
) -> ScreenLayout {
    ScreenLayout {
        section_order: cfg
            .section_order
            .as_ref()
            .map_or_else(|| base.section_order.clone(), |v| v.clone()),
        hidden_sections: cfg
            .hidden_sections
            .as_ref()
            .map_or_else(|| base.hidden_sections.clone(), |v| v.clone()),
        max_content_width: cfg.max_content_width.map_or(base.max_content_width, |v| {
            clamp_size_pos(
                "screens.max_content_width",
                v,
                base.max_content_width,
                warnings,
            )
        }),
        columns: cfg.columns.map_or(base.columns, |v| {
            clamp_columns("screens.columns", v, base.columns, warnings)
        }),
    }
}

fn merge_screens(
    base: &BTreeMap<String, ScreenLayout>,
    cfg: &BTreeMap<String, ScreenOverrides>,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, ScreenLayout> {
    let mut merged = base.clone();
    for (id, overrides) in cfg {
        let base_screen = base.get(id).cloned().unwrap_or_default();
        merged.insert(id.clone(), merge_screen(&base_screen, overrides, warnings));
    }
    merged
}

// ── Root merge ────────────────────────────────────────────────────────

/// Merge `LayoutOverrides` onto a base layout (pure — no I/O).
///
/// `base` is typically [`LayoutConfig::default()`] (which reproduces the
/// current appearance). Returns the merged [`LayoutConfig`] plus a list of
/// developer warnings describing every value that was clamped or replaced
/// by its default.
pub fn merge_layout_config(
    base: &LayoutConfig,
    overrides: &LayoutOverrides,
) -> (LayoutConfig, Vec<String>) {
    let mut warnings = Vec::new();
    let merged = LayoutConfig {
        home: match &overrides.home {
            Some(h) => merge_home(&base.home, h, &mut warnings),
            None => base.home.clone(),
        },
        sidebar: match &overrides.sidebar {
            Some(s) => merge_sidebar(&base.sidebar, s, &mut warnings),
            None => base.sidebar.clone(),
        },
        chat: match &overrides.chat {
            Some(c) => merge_chat(&base.chat, c, &mut warnings),
            None => base.chat.clone(),
        },
        component: match &overrides.component {
            Some(c) => merge_component(&base.component, c, &mut warnings),
            None => base.component.clone(),
        },
        tables: match &overrides.tables {
            Some(t) => merge_tables(&base.tables, t, &mut warnings),
            None => base.tables.clone(),
        },
        responsive: match &overrides.responsive {
            Some(r) => merge_responsive(&base.responsive, r, &mut warnings),
            None => base.responsive.clone(),
        },
        screens: merge_screens(&base.screens, &overrides.screens, &mut warnings),
    };
    (merged, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{
        ButtonPlacement, CardOrientation, ComposerButton, HomeLayoutMode, HomeSection,
        SidebarSection, ThumbnailPosition, ViewportTier,
    };

    fn merge_toml(toml: &str) -> (LayoutConfig, Vec<String>) {
        let cfg = crate::layout_config::parse_layout_config(toml).expect("config parses");
        merge_layout_config(&LayoutConfig::default(), &cfg)
    }

    #[test]
    fn empty_config_is_identity() {
        let cfg = LayoutOverrides::default();
        let (merged, warnings) = merge_layout_config(&LayoutConfig::default(), &cfg);
        assert_eq!(merged, LayoutConfig::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn full_merge_applies_every_group() {
        let (merged, warnings) = merge_toml(
            r##"
[home]
max_content_width = 1200.0
mode = "List"
section_order = ["Tunnels", "Hero"]

[home.grid]
main_portion = 3
rail_portion = 1

[home.quick_actions]
columns_wide = 5

[home.padding]
top = 16.0

[home.gaps]
card_gap = 12.0

[home.card_sizing]
activity_row_height = 28.0

[sidebar]
width = 310.0
section_order = ["Requests", "Chats"]

[sidebar.padding]
row_x = 14.0

[sidebar.row_heights]
peer_row = 64.0

[chat]
bubble_max_width = 600.0
bubble_width_ratio = 0.7

[chat.emoji_picker]
width = 300.0

[chat.gif_picker]
thumbnail_width = 160.0

[chat.screen_share]
width = 800.0

[chat.composer]
spacing = 8.0
button_order = ["Send", "Gif"]

[chat.member_list]
width = 320.0

[component]
thumbnail_position = "Top"

[component.video_card]
card_orientation = "Vertical"

[component.shared_by_me]
button_placement = "Overlay"

[component.video]
play_overlay_size = 72.0

[tables.file_table]
size_col = 80.0

[tables.shared_table]
size = 70.0

[responsive]
narrow_max_width = 400.0
home_columns = { narrow = 1, desktop = 2, ultra_wide = 4 }
home_padding_x = { narrow = 12.0, desktop = 24.0, ultra_wide = 32.0 }
"##,
        );
        assert!(
            warnings.is_empty(),
            "expected no warnings, got {warnings:?}"
        );

        assert_eq!(merged.home.max_content_width, 1200.0);
        assert_eq!(merged.home.mode, HomeLayoutMode::List);
        assert_eq!(
            merged.home.section_order,
            vec![HomeSection::Tunnels, HomeSection::Hero]
        );
        assert_eq!(merged.home.grid.main_portion, 3);
        assert_eq!(merged.home.quick_actions.columns_wide, 5);
        assert_eq!(merged.home.padding.top, 16.0);
        assert_eq!(merged.home.gaps.card_gap, 12.0);
        assert_eq!(merged.home.card_sizing.activity_row_height, 28.0);

        assert_eq!(merged.sidebar.width, 310.0);
        assert_eq!(
            merged.sidebar.section_order,
            vec![SidebarSection::Requests, SidebarSection::Chats]
        );
        assert_eq!(merged.sidebar.padding.row_x, 14.0);
        assert_eq!(merged.sidebar.row_heights.peer_row, 64.0);

        assert_eq!(merged.chat.bubble_max_width, 600.0);
        assert_eq!(merged.chat.bubble_width_ratio, 0.7);
        assert_eq!(merged.chat.emoji_picker.width, 300.0);
        assert_eq!(merged.chat.gif_picker.thumbnail_width, 160.0);
        assert_eq!(merged.chat.screen_share.width, 800.0);
        assert_eq!(merged.chat.composer.spacing, 8.0);
        assert_eq!(
            merged.chat.composer.button_order,
            vec![ComposerButton::Send, ComposerButton::Gif]
        );
        assert_eq!(merged.chat.member_list.width, 320.0);

        assert_eq!(merged.component.thumbnail_position, ThumbnailPosition::Top);
        assert_eq!(
            merged.component.video_card.card_orientation,
            CardOrientation::Vertical
        );
        assert_eq!(
            merged.component.shared_by_me.button_placement,
            ButtonPlacement::Overlay
        );
        assert_eq!(merged.component.video.play_overlay_size, 72.0);

        assert_eq!(merged.tables.file_table.size_col, 80.0);
        assert_eq!(merged.tables.shared_table.size, 70.0);

        assert_eq!(merged.responsive.narrow_max_width, 400.0);
        assert_eq!(merged.responsive.home_columns.ultra_wide, 4);
        assert_eq!(merged.responsive.home_padding_x.desktop, 24.0);
        // Defaults untouched where not overridden.
        assert_eq!(merged.responsive.home_columns.narrow, 1);
        assert_eq!(merged.responsive.viewport_ref_width, 1280.0);
        assert_eq!(
            merged.home.padding.bottom,
            LayoutConfig::default().home.padding.bottom
        );
    }

    #[test]
    fn partial_merge_keeps_defaults_for_absent_fields() {
        let (merged, warnings) = merge_toml(
            r#"
[home]
max_content_width = 1200.0
"#,
        );
        assert!(
            warnings.is_empty(),
            "expected no warnings, got {warnings:?}"
        );

        // Explicit override lands…
        assert_eq!(merged.home.max_content_width, 1200.0);
        // …everything else stays at the default.
        assert_eq!(
            merged.home.section_order,
            LayoutConfig::default().home.section_order
        );
        assert_eq!(merged.home.mode, LayoutConfig::default().home.mode);
        assert_eq!(merged.sidebar, LayoutConfig::default().sidebar);
        assert_eq!(merged.chat, LayoutConfig::default().chat);
        assert_eq!(merged.component, LayoutConfig::default().component);
        assert_eq!(merged.tables, LayoutConfig::default().tables);
        assert_eq!(merged.responsive, LayoutConfig::default().responsive);
    }

    #[test]
    fn negative_padding_clamped_to_zero() {
        let (merged, warnings) = merge_toml(
            r#"
[home.padding]
top = -4.0
"#,
        );
        assert_eq!(merged.home.padding.top, 0.0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("home.padding.top"), "{}", warnings[0]);
        assert!(warnings[0].contains("negative"), "{}", warnings[0]);
    }

    #[test]
    fn zero_max_content_width_falls_back_to_default() {
        let (merged, warnings) = merge_toml(
            r#"
[home]
max_content_width = 0.0
"#,
        );
        assert_eq!(
            merged.home.max_content_width,
            LayoutConfig::default().home.max_content_width
        );
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("home.max_content_width"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn absurd_size_clamped_to_max() {
        let (merged, warnings) = merge_toml(
            r#"
[home.padding]
top = 1.0e9
"#,
        );
        assert_eq!(merged.home.padding.top, MAX_SIZE_PX);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn zero_column_count_falls_back_to_default() {
        let (merged, warnings) = merge_toml(
            r#"
[home.quick_actions]
columns_wide = 0
"#,
        );
        assert_eq!(
            merged.home.quick_actions.columns_wide,
            LayoutConfig::default().home.quick_actions.columns_wide
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("columns_wide"), "{}", warnings[0]);
    }

    #[test]
    fn absurd_column_count_clamped() {
        let (merged, warnings) = merge_toml(
            r#"
[home.quick_actions]
columns_wide = 999
"#,
        );
        assert_eq!(merged.home.quick_actions.columns_wide, MAX_COLUMNS);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn bubble_width_ratio_clamped_to_fraction() {
        let (merged, warnings) = merge_toml(
            r#"
[chat]
bubble_width_ratio = 5.0
"#,
        );
        assert_eq!(merged.chat.bubble_width_ratio, 1.0);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("chat.bubble_width_ratio"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn nan_values_fall_back_to_default() {
        let mut cfg = LayoutOverrides::default();
        cfg.home = Some(HomeOverrides {
            max_content_width: Some(f32::NAN),
            ..Default::default()
        });
        cfg.sidebar = Some(SidebarOverrides {
            width: Some(f32::INFINITY),
            ..Default::default()
        });
        let (merged, warnings) = merge_layout_config(&LayoutConfig::default(), &cfg);
        assert_eq!(
            merged.home.max_content_width,
            LayoutConfig::default().home.max_content_width
        );
        assert_eq!(merged.sidebar.width, LayoutConfig::default().sidebar.width);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let (merged, warnings) = merge_toml(
            r#"
[home]
max_content_width = 1100.0
future_width = 9999.0

[future_group]
future_thing = 42.0
"#,
        );
        assert!(
            warnings.is_empty(),
            "expected no warnings, got {warnings:?}"
        );
        assert_eq!(merged.home.max_content_width, 1100.0);
        assert_eq!(
            merged.home.padding.top,
            LayoutConfig::default().home.padding.top
        );
    }

    #[test]
    fn per_component_override_only_touches_that_component() {
        // PDF Task 5: a partial file can override one leaf of one component
        // without leaking into the other component or the global fallback.
        let (merged, warnings) = merge_toml(
            r#"
[component.video_card]
thumbnail_position = "Bottom"
"#,
        );
        assert!(
            warnings.is_empty(),
            "expected no warnings, got {warnings:?}"
        );
        assert_eq!(
            merged.component.video_card.thumbnail_position,
            ThumbnailPosition::Bottom
        );
        // The other component and the global fallback stay at defaults.
        assert_eq!(
            merged.component.shared_by_me.thumbnail_position,
            ThumbnailPosition::Left
        );
        assert_eq!(
            merged.component.thumbnail_position,
            LayoutConfig::default().component.thumbnail_position
        );
        assert_eq!(
            merged.component.video_card.button_placement,
            LayoutConfig::default()
                .component
                .video_card
                .button_placement
        );
    }

    #[test]
    fn screens_merge_onto_screen_defaults() {
        let (merged, warnings) = merge_toml(
            r#"
[screens.settings]
columns = 2
max_content_width = 900.0
"#,
        );
        assert!(
            warnings.is_empty(),
            "expected no warnings, got {warnings:?}"
        );
        let screen = merged.screens.get("settings").expect("settings screen");
        assert_eq!(screen.columns, 2);
        assert_eq!(screen.max_content_width, 900.0);
        // Skeleton fields keep their defaults.
        assert!(screen.section_order.is_empty());
        // Other screens untouched.
        assert!(merged.screens.get("files").is_none());
    }

    #[test]
    fn tier_resolution_uses_overridden_thresholds() {
        // BORU-LAYOUT-04: the responsive tier thresholds come from the
        // model, so a TOML override moves the tiers.
        let (merged, _) = merge_toml(
            r#"
[responsive]
narrow_max_width = 500.0
ultra_wide_min_width = 1000.0
"#,
        );
        assert_eq!(
            merged.responsive.tier_for_width(499.0),
            ViewportTier::Narrow
        );
        assert_eq!(
            merged.responsive.tier_for_width(500.0),
            ViewportTier::Desktop
        );
        assert_eq!(
            merged.responsive.tier_for_width(1000.0),
            ViewportTier::UltraWide
        );
    }

    #[test]
    fn older_partial_file_merges() {
        // A file written for an earlier schema (fewer groups) parses and
        // merges fine; missing leaves keep defaults.
        let (merged, warnings) = merge_toml(
            r#"
[home]
max_content_width = 1200.0
"#,
        );
        assert!(warnings.is_empty());
        assert_eq!(merged.home.max_content_width, 1200.0);
        assert_eq!(merged.sidebar.width, LayoutConfig::default().sidebar.width);
        assert_eq!(
            merged.chat.bubble_max_width,
            LayoutConfig::default().chat.bubble_max_width
        );
    }

    #[test]
    fn merge_reports_multiple_warnings() {
        let (_, warnings) = merge_toml(
            r#"
[home.padding]
top = -1.0
bottom = 999999.0

[home]
max_content_width = 0.0

[home.quick_actions]
columns_wide = 999
"#,
        );
        assert_eq!(warnings.len(), 4);
        assert!(warnings.iter().any(|w| w.contains("home.padding.top")));
        assert!(warnings.iter().any(|w| w.contains("home.padding.bottom")));
        assert!(warnings
            .iter()
            .any(|w| w.contains("home.max_content_width")));
        assert!(warnings.iter().any(|w| w.contains("columns_wide")));
    }

    #[test]
    fn negative_gaps_and_out_of_range_card_sizes_clamped() {
        // BORU-LAYOUT-07 scope: negative gaps and out-of-range card sizes
        // are clamped to sane bounds, never applied verbatim.
        let (merged, warnings) = merge_toml(
            r#"
[home.gaps]
card_gap = -8.0

[home.card_sizing]
activity_row_height = -1.0
quick_action_icon_size = 1.0e8
status_card_text_min_width = 0.0
"#,
        );
        assert_eq!(merged.home.gaps.card_gap, 0.0);
        assert_eq!(merged.home.card_sizing.activity_row_height, 0.0);
        assert_eq!(
            merged.home.card_sizing.quick_action_icon_size, MAX_SIZE_PX,
            "absurd icon size clamps to the max"
        );
        assert_eq!(
            merged.home.card_sizing.status_card_text_min_width,
            LayoutConfig::default()
                .home
                .card_sizing
                .status_card_text_min_width,
            "zero positive size falls back to the default"
        );
        assert_eq!(warnings.len(), 4, "warnings: {warnings:?}");
    }

    #[test]
    fn screens_columns_out_of_range_clamped() {
        // Screen-level columns follow the same clamp as home columns.
        let (merged, warnings) = merge_toml(
            r#"
[screens.settings]
columns = 0
"#,
        );
        assert_eq!(
            merged.screens.get("settings").expect("settings screen").columns,
            1,
            "zero columns falls back to the screen default (ScreenLayout::default)"
        );
        assert_eq!(warnings.len(), 1);

        let (merged, warnings) = merge_toml(
            r#"
[screens.settings]
columns = 999
"#,
        );
        assert_eq!(
            merged.screens.get("settings").expect("settings screen").columns,
            MAX_COLUMNS
        );
        assert_eq!(warnings.len(), 1);
    }
}
