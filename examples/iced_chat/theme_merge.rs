//! Pure merge of `boru-ui.toml` overrides onto [`BoruTheme`] (BORU-UI-05 /
//! PDF Task 5).
//!
//! This module implements the theme load path:
//!
//! ```text
//! BoruTheme::default()  +  UiThemeConfig overrides  ->  ActiveTheme
//! ```
//!
//! ## Design rules
//!
//! - **Defaults are the source of truth.** Every config leaf is
//!   `Option<T>`; a `None` keeps the corresponding `BoruTheme` value. Only
//!   explicitly supplied TOML fields override.
//! - **Pure, no I/O.** [`merge_ui_theme`] takes an already-parsed
//!   `UiThemeConfig`; file loading stays in `theme_config::load_ui_theme_config`.
//! - **Validation before activation.** Unsafe/nonsensical values are
//!   clamped (negative padding → 0, absurd sidebar width → sane max) or, when
//!   clamping cannot fix them (zero font size, NaN), fall back to the
//!   default for that field. Every adjustment is reported as a developer
//!   warning string so the caller can log it once at startup.
//! - **Backward compatible.** Older partial theme files (fewer groups /
//!   fewer fields) deserialize to `None` leaves and merge unchanged; unknown
//!   fields are ignored by serde so new files keep working on old binaries.
//!
//! The merge functions below mirror `BoruTheme`'s group structure 1:1. The
//! `merge_group!` macro generates the boilerplate for flat groups; groups
//! with nested config tables (`sidebar`, `attachments`) are written by hand
//! so the nested merge functions are composed explicitly.

use iced::Color;

use crate::theme::{
    AttachmentTheme, AvatarTokens, BorderTokens, BoruTheme, CallTheme, ChatTheme, ColorTokens,
    ControlTokens, DialogTheme, FileTableColumns, HomeTheme, IconTokens, ListTokens, MotionTokens,
    RadiusTokens, ResponsiveTokens, RoomTheme, ScreenShareActionTheme, ScreenShareCardTheme,
    ScreenShareDestructiveTheme, ScreenShareSegmentedTheme, ScreenShareSourceCardTheme,
    ScreenShareTheme, ScreenShareToggleTheme, SharedTableColumns, SidebarPadding, SidebarTheme,
    SpacingTokens, TunnelTheme, TypographyTokens, VideoTokens,
};
use crate::theme_config::{
    AttachmentConfig, AvatarConfig, BorderConfig, CallConfig, ChatConfig, ColorConfig, ColorValue,
    ControlConfig, DialogConfig, FileTableColumnsConfig, HomeConfig, IconConfig, ListConfig,
    MotionConfig, RadiusConfig, ResponsiveConfig, RoomConfig, ScreenShareActionConfig,
    ScreenShareCardConfig, ScreenShareConfig, ScreenShareDestructiveConfig,
    ScreenShareSegmentedConfig, ScreenShareSourceCardConfig, ScreenShareToggleConfig,
    SharedTableColumnsConfig, SidebarConfig, SidebarPaddingConfig, SpacingConfig, TunnelConfig,
    TypographyConfig, UiThemeConfig, VideoConfig,
};

// ── Validation bounds ─────────────────────────────────────────────────
//
// Sane ranges for the visual values. Values outside these are developer
// errors: they are clamped where a clamp is meaningful (negative padding,
// absurd sidebar width) or replaced by the field default where it is not
// (zero font size, NaN).

/// Any px size above this is absurd and gets clamped (spacing, radii,
/// icon/avatar sizes, row heights, table column widths, …).
const MAX_SIZE_PX: f32 = 4096.0;
/// Positive sizes (font sizes, control heights): must be > 0; a value of 0
/// is not a valid font size and falls back to the default.
const MIN_POSITIVE_PX: f32 = 1.0;
/// Fractions / ratios (bubble width ratio, alpha): 0..=1.
const MAX_FRACTION: f32 = 1.0;
/// Sidebar width family: sane band around the 288–320 px responsive range.
const SIDEBAR_WIDTH_MIN: f32 = 80.0;
const SIDEBAR_WIDTH_MAX: f32 = 2000.0;
/// Motion frame counts: 240 frames @60fps = 4 s — beyond that is absurd.
const MAX_FRAMES: u32 = 240;

/// Clamp an f32 size that may legitimately be 0 (padding, spacing, radii).
fn clamp_size0(field: &str, v: f32, default: f32, warnings: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        warnings.push(format!("{field}: {v} is not finite; using default {default}"));
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

/// Clamp an f32 size that must be positive (font sizes). 0 / negative is
/// invalid and falls back to the field default.
fn clamp_size_pos(field: &str, v: f32, default: f32, warnings: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        warnings.push(format!("{field}: {v} is not finite; using default {default}"));
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

/// Clamp a fraction / ratio to 0..=1 (bubble width ratio, soft-tint alpha).
fn clamp_fraction(field: &str, v: f32, default: f32, warnings: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        warnings.push(format!("{field}: {v} is not finite; using default {default}"));
        return default;
    }
    if !(0.0..=MAX_FRACTION).contains(&v) {
        warnings.push(format!(
            "{field}: {v} is outside 0..=1; clamped to {}",
            v.clamp(0.0, MAX_FRACTION)
        ));
        return v.clamp(0.0, MAX_FRACTION);
    }
    v
}

/// Clamp the sidebar width family (width / width_min / width_max).
fn clamp_sidebar(field: &str, v: f32, default: f32, warnings: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        warnings.push(format!("{field}: {v} is not finite; using default {default}"));
        return default;
    }
    if v < SIDEBAR_WIDTH_MIN || v > SIDEBAR_WIDTH_MAX {
        let clamped = v.clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
        warnings.push(format!(
            "{field}: {v} is outside the sane sidebar range [{SIDEBAR_WIDTH_MIN}, {SIDEBAR_WIDTH_MAX}]; clamped to {clamped}"
        ));
        return clamped;
    }
    v
}

/// Clamp an integer frame count (motion).
fn clamp_frames(field: &str, v: u32, default: u32, warnings: &mut Vec<String>) -> u32 {
    if v > MAX_FRAMES {
        warnings.push(format!("{field}: {v} is absurd; clamped to {MAX_FRAMES}"));
        return MAX_FRAMES;
    }
    v
}

/// Identity policy for boolean visual flags — a bool cannot be nonsense, so
/// the configured value is used as-is (BORU-UI-09 optional visual features).
fn clamp_flag(_field: &str, v: bool, _default: bool, _warnings: &mut Vec<String>) -> bool {
    v
}

/// Line-height multipliers: relative 0.5..=4.0; non-finite or absurd values
/// fall back to the field default (BORU-UI-16).
const MIN_LINE_HEIGHT: f32 = 0.5;
const MAX_LINE_HEIGHT: f32 = 4.0;

fn clamp_line_height(field: &str, v: f32, default: f32, warnings: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        warnings.push(format!("{field}: {v} is not finite; using default {default}"));
        return default;
    }
    if v < MIN_LINE_HEIGHT || v > MAX_LINE_HEIGHT {
        warnings.push(format!(
            "{field}: {v} is outside the sane line-height range [{MIN_LINE_HEIGHT}, {MAX_LINE_HEIGHT}]; clamped to {}",
            v.clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT)
        ));
        return v.clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT);
    }
    v
}

/// Resolve a configured family-name string to a bundled `FontFamilyKey`.
/// Unknown names (a font that is not bundled / unavailable) log a warning
/// and fall back to the field default — graceful fallback per BORU-UI-16.
fn clamp_family(
    field: &str,
    v: String,
    default: crate::fonts::FontFamilyKey,
    warnings: &mut Vec<String>,
) -> crate::fonts::FontFamilyKey {
    match crate::fonts::FontFamilyKey::from_name(v.trim()) {
        Some(key) => key,
        None => {
            warnings.push(format!(
                "{field}: unknown font family {v:?} is not bundled; using default {:?}",
                default.name()
            ));
            default
        }
    }
}

/// Resolve a configured weight-name string to a registered
/// `FontWeightKey`. Unknown names log a warning and fall back to the field
/// default (BORU-UI-16).
fn clamp_weight(
    field: &str,
    v: String,
    default: crate::fonts::FontWeightKey,
    warnings: &mut Vec<String>,
) -> crate::fonts::FontWeightKey {
    match crate::fonts::FontWeightKey::from_name(v.trim()) {
        Some(key) => key,
        None => {
            warnings.push(format!(
                "{field}: unknown weight {v:?}; using default {}",
                default.label()
            ));
            default
        }
    }
}

/// Convert a config colour to `iced::Color`, clamping channels to 0..=1 and
/// falling back to the default on NaN.
fn clamp_color(field: &str, v: ColorValue, default: Color, warnings: &mut Vec<String>) -> Color {
    let r = v.r;
    let g = v.g;
    let b = v.b;
    let a = v.a;
    if !r.is_finite() || !g.is_finite() || !b.is_finite() || !a.is_finite() {
        warnings.push(format!("{field}: colour has a non-finite channel; using default"));
        return default;
    }
    if !(0.0..=1.0).contains(&r) || !(0.0..=1.0).contains(&g) || !(0.0..=1.0).contains(&b) || !(0.0..=1.0).contains(&a) {
        let clamped = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), a.clamp(0.0, 1.0)];
        warnings.push(format!(
            "{field}: colour channels outside 0..=1; clamped to {:?}",
            clamped
        ));
        return Color::from_rgba(clamped[0], clamped[1], clamped[2], clamped[3]);
    }
    Color::from_rgba(r, g, b, a)
}

// ── Flat group merge macro ────────────────────────────────────────────
//
// Generates `fn merge_<group>(base: &<ThemeGroup>, cfg: &<ConfigGroup>,
// warnings) -> <ThemeGroup>` that applies every configured leaf through its
// validation policy. Field names MUST match between the theme and config
// structs (guaranteed by the tests below + the shared naming convention
// documented in theme_config.rs).

macro_rules! merge_group {
    ($fn:ident, $theme:ident, $cfg:ident, $prefix:literal, { $($field:ident: $policy:ident),* $(,)? }) => {
        fn $fn(base: &$theme, cfg: &$cfg, warnings: &mut Vec<String>) -> $theme {
            $theme {
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

merge_group! {
    merge_color_tokens, ColorTokens, ColorConfig, "colors", {
        canvas: clamp_color,
        sidebar: clamp_color,
        surface: clamp_color,
        surface_elevated: clamp_color,
        surface_selected: clamp_color,
        surface_hover: clamp_color,
        surface_pressed: clamp_color,
        surface_secondary: clamp_color,
        input_bg: clamp_color,
        border_muted: clamp_color,
        border_strong: clamp_color,
        text_primary: clamp_color,
        text_secondary: clamp_color,
        text_muted: clamp_color,
        text_local_label: clamp_color,
        text_local_body: clamp_color,
        text_remote_label: clamp_color,
        text_remote_body: clamp_color,
        primary: clamp_color,
        primary_hover: clamp_color,
        primary_pressed: clamp_color,
        primary_soft: clamp_color,
        success: clamp_color,
        danger: clamp_color,
        warning: clamp_color,
        focus: clamp_color,
        soft_tint_alpha: clamp_fraction,
        dialog_backdrop: clamp_color,
        incoming_call_backdrop: clamp_color,
        chat_overlay_backdrop: clamp_color,
        chat_search_backdrop: clamp_color,
        panel_shadow: clamp_color,
        dialog_panel_bg: clamp_color,
        dialog_panel_border: clamp_color,
        media_frame_bg: clamp_color,
        media_frame_border: clamp_color,
        media_frame_overlay: clamp_color,
        on_media_text: clamp_color,
        glyph_disabled: clamp_color,
        glyph_muted: clamp_color,
        glyph_muted_dark: clamp_color,
        avatar_fallback: clamp_color,
        tag_text: clamp_color,
        tag_bg: clamp_color,
        tag_bg_pressed: clamp_color,
        download_completed: clamp_color,
        download_temporary: clamp_color,
        download_terminal: clamp_color,
        download_cancelled: clamp_color,
        request_pending: clamp_color,
        request_accepted: clamp_color,
        request_declined: clamp_color,
        settings_success: clamp_color,
        settings_danger: clamp_color,
        settings_danger_strong: clamp_color,
        settings_heading_text: clamp_color,
        expanded_video_backdrop: clamp_color,
        lightbox_backdrop: clamp_color,
        status_card_bg_top: clamp_color,
        status_card_bg_mid: clamp_color,
        status_card_bg_bottom: clamp_color,
        status_card_border: clamp_color,
        status_connected: clamp_color,
        status_primary_text: clamp_color,
        status_secondary_text: clamp_color,
        status_network_line: clamp_color,
        status_network_node: clamp_color,
        status_warning: clamp_color,
        status_danger: clamp_color,
    }
}

merge_group! {
    merge_typography_tokens, TypographyTokens, TypographyConfig, "typography", {
        display_heading: clamp_size_pos,
        page_title: clamp_size_pos,
        section_title: clamp_size_pos,
        card_title: clamp_size_pos,
        body: clamp_size_pos,
        body_emphasised: clamp_size_pos,
        button_label: clamp_size_pos,
        supporting_text: clamp_size_pos,
        metadata: clamp_size_pos,
        chat_message: clamp_size_pos,
        chat_sender: clamp_size_pos,
        chat_metadata: clamp_size_pos,
        composer_text: clamp_size_pos,
        technical_value: clamp_size_pos,
        brand_wordmark: clamp_size_pos,
        home_subtitle: clamp_size_pos,
        dialog_title: clamp_size_pos,
        dialog_subtitle: clamp_size_pos,
        sidebar_name: clamp_size_pos,
        section_label: clamp_size_pos,
        badge: clamp_size_pos,
        call_name: clamp_size_pos,
        call_name_active: clamp_size_pos,
        call_remote_name: clamp_size_pos,
        call_status: clamp_size_pos,
        call_duration: clamp_size_pos,
        call_avatar_glyph: clamp_size_pos,
        call_avatar_glyph_large: clamp_size_pos,
        call_pip_label: clamp_size_pos,
        display_family: clamp_family,
        ui_family: clamp_family,
        chat_family: clamp_family,
        technical_family: clamp_family,
        brand_family: clamp_family,
        display_heading_weight: clamp_weight,
        page_title_weight: clamp_weight,
        section_title_weight: clamp_weight,
        card_title_weight: clamp_weight,
        body_weight: clamp_weight,
        body_emphasised_weight: clamp_weight,
        button_label_weight: clamp_weight,
        supporting_text_weight: clamp_weight,
        metadata_weight: clamp_weight,
        chat_message_weight: clamp_weight,
        chat_sender_weight: clamp_weight,
        chat_metadata_weight: clamp_weight,
        composer_text_weight: clamp_weight,
        technical_value_weight: clamp_weight,
        brand_wordmark_weight: clamp_weight,
        display_heading_line_height: clamp_line_height,
        page_title_line_height: clamp_line_height,
        section_title_line_height: clamp_line_height,
        card_title_line_height: clamp_line_height,
        body_line_height: clamp_line_height,
        body_emphasised_line_height: clamp_line_height,
        button_label_line_height: clamp_line_height,
        supporting_text_line_height: clamp_line_height,
        metadata_line_height: clamp_line_height,
        chat_message_line_height: clamp_line_height,
        chat_sender_line_height: clamp_line_height,
        chat_metadata_line_height: clamp_line_height,
        composer_text_line_height: clamp_line_height,
        technical_value_line_height: clamp_line_height,
        brand_wordmark_line_height: clamp_line_height,
    }
}

merge_group! {
    merge_spacing_tokens, SpacingTokens, SpacingConfig, "spacing", {
        space_2: clamp_size0,
        space_4: clamp_size0,
        space_6: clamp_size0,
        space_8: clamp_size0,
        space_10: clamp_size0,
        space_12: clamp_size0,
        space_16: clamp_size0,
        space_18: clamp_size0,
        space_20: clamp_size0,
        space_24: clamp_size0,
        space_28: clamp_size0,
        space_32: clamp_size0,
        space_40: clamp_size0,
        control_height: clamp_size0,
        control_height_compact: clamp_size0,
    }
}

merge_group! {
    merge_radius_tokens, RadiusTokens, RadiusConfig, "radii", {
        none: clamp_size0,
        sm: clamp_size0,
        md: clamp_size0,
        lg: clamp_size0,
        xl: clamp_size0,
        card: clamp_size0,
        pill: clamp_size0,
        avatar_container: clamp_size0,
        call_avatar: clamp_size0,
        media_frame: clamp_size0,
        attachment: clamp_size0,
        dialog: clamp_size0,
        picker_cell: clamp_size0,
        control_sm: clamp_size0,
        status_divider: clamp_size0,
        security_pill: clamp_size0,
    }
}

merge_group! {
    merge_icon_tokens, IconTokens, IconConfig, "icons", {
        xs: clamp_size0,
        sm: clamp_size0,
        md: clamp_size0,
        lg: clamp_size0,
        xl: clamp_size0,
        sidebar_utility: clamp_size0,
    }
}

merge_group! {
    merge_avatar_tokens, AvatarTokens, AvatarConfig, "avatars", {
        sm: clamp_size0,
        md: clamp_size0,
        lg: clamp_size0,
        profile: clamp_size0,
        chat_list: clamp_size0,
        chat_header: clamp_size0,
        msg: clamp_size0,
        status_dot_sm: clamp_size0,
        status_dot_lg: clamp_size0,
    }
}

merge_group! {
    merge_list_tokens, ListTokens, ListConfig, "lists", {
        card_row_height: clamp_size0,
        peer_row_height: clamp_size0,
        default_list_max_height: clamp_size0,
        table_row_height: clamp_size0,
        table_row_height_compact: clamp_size0,
        chip_height: clamp_size0,
        peer_panel_max_height: clamp_size0,
        progress_bar_height: clamp_size0,
        progress_bar_height_bold: clamp_size0,
    }
}

merge_group! {
    merge_border_tokens, BorderTokens, BorderConfig, "borders", {
        hairline: clamp_size0,
        focus: clamp_size0,
        tab_active: clamp_size0,
        selected_row: clamp_size0,
        media_frame: clamp_size0,
    }
}

merge_group! {
    merge_responsive_tokens, ResponsiveTokens, ResponsiveConfig, "responsive", {
        viewport_ref_width: clamp_size_pos,
        viewport_ref_height: clamp_size_pos,
        viewport_min_width: clamp_size_pos,
        viewport_min_height: clamp_size_pos,
        viewport_lg_width: clamp_size_pos,
        viewport_lg_height: clamp_size_pos,
        viewport_xl_width: clamp_size_pos,
        viewport_xl_height: clamp_size_pos,
        content_max_width: clamp_size_pos,
        dashboard_max_width: clamp_size_pos,
        home_two_col_content: clamp_size_pos,
        home_quick_one_col_content: clamp_size_pos,
        home_quick_four_col_content: clamp_size_pos,
        home_illustration_full_content: clamp_size_pos,
        home_illustration_hide_content: clamp_size_pos,
        home_compact_header_content: clamp_size_pos,
    }
}

merge_group! {
    merge_motion_tokens, MotionTokens, MotionConfig, "motion", {
        sidebar_fade_frames: clamp_frames,
    }
}

merge_group! {
    merge_sidebar_padding, SidebarPadding, SidebarPaddingConfig, "sidebar.padding", {
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

merge_group! {
    merge_home_theme, HomeTheme, HomeConfig, "home", {
        peers_body_min: clamp_size0,
        activity_row_height: clamp_size0,
        hero_gap: clamp_size0,
        quick_action_gap: clamp_size0,
        quick_action_icon_size: clamp_size0,
        quick_action_title_size: clamp_size_pos,
        quick_action_desc_size: clamp_size_pos,
        quick_action_desc_line_height: clamp_size_pos,
        status_card_text_min_width_medium: clamp_size0,
        status_card_mesh_max_width: clamp_size0,
        status_card_padding_x: clamp_size0,
        status_icon_text_gap_full: clamp_size0,
        status_icon_text_gap_medium: clamp_size0,
        status_text_graph_gap_full: clamp_size0,
        status_text_graph_gap_medium: clamp_size0,
        status_divider_width: clamp_size0,
        status_divider_height: clamp_size0,
        status_divider_radius: clamp_size0,
        security_pill_radius: clamp_size0,
        show_activity_feed: clamp_flag,
    }
}

merge_group! {
    merge_chat_theme, ChatTheme, ChatConfig, "chat", {
        spinner_size: clamp_size0,
        context_menu_width: clamp_size0,
        emoji_picker_width: clamp_size0,
        emoji_picker_scroll_height: clamp_size0,
        gif_picker_width: clamp_size0,
        gif_picker_scroll_height: clamp_size0,
        gif_thumbnail_width: clamp_size0,
        gif_thumbnail_height: clamp_size0,
        screen_share_w: clamp_size0,
        screen_share_h: clamp_size0,
        bubble_max_width: clamp_size0,
        bubble_width_ratio: clamp_fraction,
        message_max_width: clamp_size0,
        image_preview_max_width: clamp_size0,
        image_preview_max_height: clamp_size0,
    }
}

merge_group! {
    merge_file_table_columns, FileTableColumns, FileTableColumnsConfig, "attachments.file_table", {
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

merge_group! {
    merge_shared_table_columns, SharedTableColumns, SharedTableColumnsConfig, "attachments.shared_table", {
        shared_with: clamp_size0,
        size: clamp_size0,
        shared_on: clamp_size0,
        downloads: clamp_size0,
        actions: clamp_size0,
    }
}

merge_group! {
    merge_video_tokens, VideoTokens, VideoConfig, "attachments.video", {
        narrow_breakpoint: clamp_size0,
        medium_breakpoint: clamp_size0,
        play_overlay_size: clamp_size0,
        header_filename_max_width: clamp_size0,
        controls_slider_width: clamp_size0,
    }
}

merge_group! {
    merge_room_theme, RoomTheme, RoomConfig, "rooms", {
        catalogue_row_height: clamp_size0,
        overscan: clamp_size0,
        banner_width: clamp_size0,
        progress_length: clamp_size0,
        progress_girth: clamp_size0,
    }
}

merge_group! {
    merge_tunnel_theme, TunnelTheme, TunnelConfig, "tunnels", {
        chip_padding_x: clamp_size0,
        chip_padding_y: clamp_size0,
    }
}

merge_group! {
    merge_dialog_theme, DialogTheme, DialogConfig, "dialogs", {
        avatar_size: clamp_size0,
        avatar_glyph_size: clamp_size0,
        title_size: clamp_size_pos,
        body_size: clamp_size_pos,
        spacing: clamp_size0,
        padding: clamp_size0,
        control_padding_x: clamp_size0,
        control_padding_y: clamp_size0,
        control_spacing: clamp_size0,
    }
}

merge_group! {
    merge_call_theme, CallTheme, CallConfig, "calls", {
        avatar_size: clamp_size0,
        avatar_glyph_size: clamp_size0,
        avatar_glyph_size_large: clamp_size0,
        pip_w: clamp_size0,
        pip_h: clamp_size0,
        controls_gap: clamp_size0,
    }
}

merge_group! {
    merge_control_tokens, ControlTokens, ControlConfig, "controls", {
        header_height: clamp_size0,
        slider_width: clamp_size0,
        color_picker_radius: clamp_size0,
        color_picker_bar_radius: clamp_size0,
    }
}

// ── Screen-share sender UI (BORU-SSUI-08) ─────────────────────────────
//
// Mirrors `ScreenShareTheme` 1:1. Every group is flat geometry, so the
// `merge_group!` macro applies; the nested `[screen_share]` group is
// composed by hand (same pattern as `sidebar` / `attachments`).

merge_group! {
    merge_screen_share_card, ScreenShareCardTheme, ScreenShareCardConfig, "screen_share.card", {
        padding: clamp_size0,
        radius: clamp_size0,
        border_width: clamp_size0,
        spacing: clamp_size0,
    }
}

merge_group! {
    merge_screen_share_source_card, ScreenShareSourceCardTheme, ScreenShareSourceCardConfig, "screen_share.source_card", {
        width: clamp_size0,
        radius: clamp_size0,
        padding_x: clamp_size0,
        padding_y: clamp_size0,
        icon_size: clamp_size0,
        check_icon_size: clamp_size0,
        selected_border_width: clamp_size0,
        title_max_chars: clamp_size0,
        row_spacing: clamp_size0,
    }
}

merge_group! {
    merge_screen_share_segmented, ScreenShareSegmentedTheme, ScreenShareSegmentedConfig, "screen_share.segmented", {
        radius: clamp_size0,
        spacing: clamp_size0,
        padding_x: clamp_size0,
        padding_y: clamp_size0,
    }
}

merge_group! {
    merge_screen_share_toggle, ScreenShareToggleTheme, ScreenShareToggleConfig, "screen_share.toggle", {
        row_spacing: clamp_size0,
        icon_size: clamp_size0,
    }
}

merge_group! {
    merge_screen_share_action, ScreenShareActionTheme, ScreenShareActionConfig, "screen_share.action", {
        row_spacing: clamp_size0,
    }
}

merge_group! {
    merge_screen_share_destructive, ScreenShareDestructiveTheme, ScreenShareDestructiveConfig, "screen_share.destructive", {
        padding_x: clamp_size0,
        padding_y: clamp_size0,
        radius: clamp_size0,
        icon_gap: clamp_size0,
    }
}

fn merge_screen_share_theme(
    base: &ScreenShareTheme,
    cfg: &ScreenShareConfig,
    warnings: &mut Vec<String>,
) -> ScreenShareTheme {
    ScreenShareTheme {
        card: merge_screen_share_card(
            &base.card,
            cfg.card
                .as_ref()
                .unwrap_or(&ScreenShareCardConfig::default()),
            warnings,
        ),
        source_card: merge_screen_share_source_card(
            &base.source_card,
            cfg.source_card
                .as_ref()
                .unwrap_or(&ScreenShareSourceCardConfig::default()),
            warnings,
        ),
        segmented: merge_screen_share_segmented(
            &base.segmented,
            cfg.segmented
                .as_ref()
                .unwrap_or(&ScreenShareSegmentedConfig::default()),
            warnings,
        ),
        toggle: merge_screen_share_toggle(
            &base.toggle,
            cfg.toggle
                .as_ref()
                .unwrap_or(&ScreenShareToggleConfig::default()),
            warnings,
        ),
        action: merge_screen_share_action(
            &base.action,
            cfg.action
                .as_ref()
                .unwrap_or(&ScreenShareActionConfig::default()),
            warnings,
        ),
        destructive: merge_screen_share_destructive(
            &base.destructive,
            cfg.destructive
                .as_ref()
                .unwrap_or(&ScreenShareDestructiveConfig::default()),
            warnings,
        ),
    }
}

// ── Groups with nested config tables ──────────────────────────────────
//
// `sidebar` nests `sidebar.padding`; `attachments` nests `file_table`,
// `shared_table` and `video`. Composed by hand so the nested merge
// functions are called explicitly.

fn merge_sidebar_theme(base: &SidebarTheme, cfg: &SidebarConfig, warnings: &mut Vec<String>) -> SidebarTheme {
    SidebarTheme {
        width: cfg
            .width
            .map_or(base.width, |v| clamp_sidebar("sidebar.width", v, base.width, warnings)),
        width_min: cfg.width_min.map_or(base.width_min, |v| {
            clamp_sidebar("sidebar.width_min", v, base.width_min, warnings)
        }),
        width_max: cfg.width_max.map_or(base.width_max, |v| {
            clamp_sidebar("sidebar.width_max", v, base.width_max, warnings)
        }),
        inset: cfg
            .inset
            .map_or(base.inset, |v| clamp_size0("sidebar.inset", v, base.inset, warnings)),
        item_radius: cfg.item_radius.map_or(base.item_radius, |v| {
            clamp_size0("sidebar.item_radius", v, base.item_radius, warnings)
        }),
        avatar_container_radius: cfg.avatar_container_radius.map_or(base.avatar_container_radius, |v| {
            clamp_size0(
                "sidebar.avatar_container_radius",
                v,
                base.avatar_container_radius,
                warnings,
            )
        }),
        utility_icon_size: cfg.utility_icon_size.map_or(base.utility_icon_size, |v| {
            clamp_size0("sidebar.utility_icon_size", v, base.utility_icon_size, warnings)
        }),
        name_size: cfg.name_size.map_or(base.name_size, |v| {
            clamp_size_pos("sidebar.name_size", v, base.name_size, warnings)
        }),
        section_label_size: cfg.section_label_size.map_or(base.section_label_size, |v| {
            clamp_size_pos("sidebar.section_label_size", v, base.section_label_size, warnings)
        }),
        padding: match &cfg.padding {
            Some(p) => merge_sidebar_padding(&base.padding, p, warnings),
            None => base.padding,
        },
    }
}

fn merge_attachment_theme(
    base: &AttachmentTheme,
    cfg: &AttachmentConfig,
    warnings: &mut Vec<String>,
) -> AttachmentTheme {
    AttachmentTheme {
        empty_state_height: cfg.empty_state_height.map_or(base.empty_state_height, |v| {
            clamp_size0("attachments.empty_state_height", v, base.empty_state_height, warnings)
        }),
        menu_width: cfg
            .menu_width
            .map_or(base.menu_width, |v| clamp_size0("attachments.menu_width", v, base.menu_width, warnings)),
        chip_avatar_size: cfg.chip_avatar_size.map_or(base.chip_avatar_size, |v| {
            clamp_size0("attachments.chip_avatar_size", v, base.chip_avatar_size, warnings)
        }),
        chip_label_size: cfg.chip_label_size.map_or(base.chip_label_size, |v| {
            clamp_size_pos("attachments.chip_label_size", v, base.chip_label_size, warnings)
        }),
        detail_label_width: cfg.detail_label_width.map_or(base.detail_label_width, |v| {
            clamp_size0("attachments.detail_label_width", v, base.detail_label_width, warnings)
        }),
        progress_bar_girth: cfg.progress_bar_girth.map_or(base.progress_bar_girth, |v| {
            clamp_size0("attachments.progress_bar_girth", v, base.progress_bar_girth, warnings)
        }),
        progress_pct_label_width: cfg.progress_pct_label_width.map_or(base.progress_pct_label_width, |v| {
            clamp_size0(
                "attachments.progress_pct_label_width",
                v,
                base.progress_pct_label_width,
                warnings,
            )
        }),
        progress_slot_height: cfg.progress_slot_height.map_or(base.progress_slot_height, |v| {
            clamp_size0(
                "attachments.progress_slot_height",
                v,
                base.progress_slot_height,
                warnings,
            )
        }),
        detail_slot_height: cfg.detail_slot_height.map_or(base.detail_slot_height, |v| {
            clamp_size0(
                "attachments.detail_slot_height",
                v,
                base.detail_slot_height,
                warnings,
            )
        }),
        policy_slot_height: cfg.policy_slot_height.map_or(base.policy_slot_height, |v| {
            clamp_size0(
                "attachments.policy_slot_height",
                v,
                base.policy_slot_height,
                warnings,
            )
        }),
        action_button_line: cfg.action_button_line.map_or(base.action_button_line, |v| {
            clamp_size0(
                "attachments.action_button_line",
                v,
                base.action_button_line,
                warnings,
            )
        }),
        search_width_medium: cfg.search_width_medium.map_or(base.search_width_medium, |v| {
            clamp_size0(
                "attachments.search_width_medium",
                v,
                base.search_width_medium,
                warnings,
            )
        }),
        search_width_full: cfg.search_width_full.map_or(base.search_width_full, |v| {
            clamp_size0(
                "attachments.search_width_full",
                v,
                base.search_width_full,
                warnings,
            )
        }),
        file_table: match &cfg.file_table {
            Some(t) => merge_file_table_columns(&base.file_table, t, warnings),
            None => base.file_table,
        },
        shared_table: match &cfg.shared_table {
            Some(t) => merge_shared_table_columns(&base.shared_table, t, warnings),
            None => base.shared_table,
        },
        video: match &cfg.video {
            Some(v) => merge_video_tokens(&base.video, v, warnings),
            None => base.video,
        },
    }
}

// ── Root merge ────────────────────────────────────────────────────────

/// Merge `UiThemeConfig` overrides onto a base theme (pure — no I/O).
///
/// `base` is typically `BoruTheme::default()` (light) or
/// `BoruTheme::for_theme(&active_iced_theme)` (mode-aware). Returns the
/// merged `BoruTheme` (the app's `ActiveTheme`) plus a list of developer
/// warnings describing every value that was clamped or replaced by its
/// default.
pub fn merge_ui_theme(base: &BoruTheme, cfg: &UiThemeConfig) -> (BoruTheme, Vec<String>) {
    let mut warnings = Vec::new();
    let merged = BoruTheme {
        colors: merge_color_tokens(
            &base.colors,
            cfg.colors.as_ref().unwrap_or(&ColorConfig::default()),
            &mut warnings,
        ),
        typography: merge_typography_tokens(
            &base.typography,
            cfg.typography.as_ref().unwrap_or(&TypographyConfig::default()),
            &mut warnings,
        ),
        spacing: merge_spacing_tokens(
            &base.spacing,
            cfg.spacing.as_ref().unwrap_or(&SpacingConfig::default()),
            &mut warnings,
        ),
        radii: merge_radius_tokens(
            &base.radii,
            cfg.radii.as_ref().unwrap_or(&RadiusConfig::default()),
            &mut warnings,
        ),
        icons: merge_icon_tokens(
            &base.icons,
            cfg.icons.as_ref().unwrap_or(&IconConfig::default()),
            &mut warnings,
        ),
        avatars: merge_avatar_tokens(
            &base.avatars,
            cfg.avatars.as_ref().unwrap_or(&AvatarConfig::default()),
            &mut warnings,
        ),
        lists: merge_list_tokens(
            &base.lists,
            cfg.lists.as_ref().unwrap_or(&ListConfig::default()),
            &mut warnings,
        ),
        borders: merge_border_tokens(
            &base.borders,
            cfg.borders.as_ref().unwrap_or(&BorderConfig::default()),
            &mut warnings,
        ),
        responsive: merge_responsive_tokens(
            &base.responsive,
            cfg.responsive.as_ref().unwrap_or(&ResponsiveConfig::default()),
            &mut warnings,
        ),
        motion: merge_motion_tokens(
            &base.motion,
            cfg.motion.as_ref().unwrap_or(&MotionConfig::default()),
            &mut warnings,
        ),
        sidebar: merge_sidebar_theme(
            &base.sidebar,
            cfg.sidebar.as_ref().unwrap_or(&SidebarConfig::default()),
            &mut warnings,
        ),
        home: merge_home_theme(
            &base.home,
            cfg.home.as_ref().unwrap_or(&HomeConfig::default()),
            &mut warnings,
        ),
        chat: merge_chat_theme(
            &base.chat,
            cfg.chat.as_ref().unwrap_or(&ChatConfig::default()),
            &mut warnings,
        ),
        attachments: merge_attachment_theme(
            &base.attachments,
            cfg.attachments.as_ref().unwrap_or(&AttachmentConfig::default()),
            &mut warnings,
        ),
        rooms: merge_room_theme(
            &base.rooms,
            cfg.rooms.as_ref().unwrap_or(&RoomConfig::default()),
            &mut warnings,
        ),
        tunnels: merge_tunnel_theme(
            &base.tunnels,
            cfg.tunnels.as_ref().unwrap_or(&TunnelConfig::default()),
            &mut warnings,
        ),
        dialogs: merge_dialog_theme(
            &base.dialogs,
            cfg.dialogs.as_ref().unwrap_or(&DialogConfig::default()),
            &mut warnings,
        ),
        calls: merge_call_theme(
            &base.calls,
            cfg.calls.as_ref().unwrap_or(&CallConfig::default()),
            &mut warnings,
        ),
        controls: merge_control_tokens(
            &base.controls,
            cfg.controls.as_ref().unwrap_or(&ControlConfig::default()),
            &mut warnings,
        ),
        screen_share: merge_screen_share_theme(
            &base.screen_share,
            cfg.screen_share
                .as_ref()
                .unwrap_or(&ScreenShareConfig::default()),
            &mut warnings,
        ),
    };
    (merged, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merge_toml(toml: &str) -> (BoruTheme, Vec<String>) {
        let cfg = crate::theme_config::parse_ui_theme_config(toml).expect("config parses");
        merge_ui_theme(&BoruTheme::default(), &cfg)
    }

    #[test]
    fn empty_config_is_identity() {
        let cfg = UiThemeConfig::default();
        let (merged, warnings) = merge_ui_theme(&BoruTheme::default(), &cfg);
        assert_eq!(merged, BoruTheme::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn full_merge_applies_every_group() {
        let (merged, warnings) = merge_toml(
            r##"
[colors]
canvas = "#112233"
surface_elevated = "#445566"
primary = [0.1, 0.2, 0.3]
soft_tint_alpha = 0.25

[typography]
body = 17.0

[spacing]
space_8 = 9.0

[radii]
card = 18.0

[icons]
md = 22.0

[avatars]
msg = 48.0

[lists]
card_row_height = 50.0

[borders]
hairline = 2.0

[responsive]
content_max_width = 740.0

[motion]
sidebar_fade_frames = 7

[sidebar]
width = 310.0
item_radius = 11.0

[sidebar.padding]
row_x = 14.0

[home]
hero_gap = 42.0

[chat]
bubble_max_width = 600.0
bubble_width_ratio = 0.7

[attachments]
empty_state_height = 210.0

[attachments.file_table]
size_col = 80.0

[attachments.shared_table]
size = 70.0

[attachments.video]
play_overlay_size = 70.0

[rooms]
catalogue_row_height = 56.0

[tunnels]
chip_padding_x = 8.0

[dialogs]
avatar_size = 80.0

[calls]
avatar_size = 100.0

[controls]
header_height = 56.0

[screen_share.card]
padding = 18.0

[screen_share.source_card]
width = 200.0
title_max_chars = 24.0

[screen_share.segmented]
radius = 12.0

[screen_share.toggle]
row_spacing = 10.0

[screen_share.action]
row_spacing = 10.0

[screen_share.destructive]
radius = 12.0

"##,
        );
        assert!(warnings.is_empty(), "expected no warnings, got {warnings:?}");

        assert_eq!(merged.colors.canvas, Color::from_rgb(0x11 as f32 / 255.0, 0x22 as f32 / 255.0, 0x33 as f32 / 255.0));
        assert_eq!(merged.colors.surface_elevated, Color::from_rgb(0x44 as f32 / 255.0, 0x55 as f32 / 255.0, 0x66 as f32 / 255.0));
        assert_eq!(merged.colors.primary, Color::from_rgba(0.1, 0.2, 0.3, 1.0));
        assert_eq!(merged.colors.soft_tint_alpha, 0.25);
        assert_eq!(merged.typography.body, 17.0);
        assert_eq!(merged.spacing.space_8, 9.0);
        assert_eq!(merged.radii.card, 18.0);
        assert_eq!(merged.icons.md, 22.0);
        assert_eq!(merged.avatars.msg, 48.0);
        assert_eq!(merged.lists.card_row_height, 50.0);
        assert_eq!(merged.borders.hairline, 2.0);
        assert_eq!(merged.responsive.content_max_width, 740.0);
        assert_eq!(merged.motion.sidebar_fade_frames, 7);
        assert_eq!(merged.sidebar.width, 310.0);
        assert_eq!(merged.sidebar.item_radius, 11.0);
        assert_eq!(merged.sidebar.padding.row_x, 14.0);
        assert_eq!(merged.home.hero_gap, 42.0);
        assert_eq!(merged.chat.bubble_max_width, 600.0);
        assert_eq!(merged.chat.bubble_width_ratio, 0.7);
        assert_eq!(merged.attachments.empty_state_height, 210.0);
        assert_eq!(merged.attachments.file_table.size_col, 80.0);
        assert_eq!(merged.attachments.shared_table.size, 70.0);
        assert_eq!(merged.attachments.video.play_overlay_size, 70.0);
        assert_eq!(merged.rooms.catalogue_row_height, 56.0);
        assert_eq!(merged.tunnels.chip_padding_x, 8.0);
        assert_eq!(merged.dialogs.avatar_size, 80.0);
        assert_eq!(merged.calls.avatar_size, 100.0);
        assert_eq!(merged.controls.header_height, 56.0);
        // BORU-SSUI-08: screen-share tokens merge from `[screen_share.*]`.
        assert_eq!(merged.screen_share.card.padding, 18.0);
        assert_eq!(merged.screen_share.source_card.width, 200.0);
        assert_eq!(merged.screen_share.source_card.title_max_chars, 24.0);
        assert_eq!(merged.screen_share.segmented.radius, 12.0);
        assert_eq!(merged.screen_share.toggle.row_spacing, 10.0);
        assert_eq!(merged.screen_share.action.row_spacing, 10.0);
        assert_eq!(merged.screen_share.destructive.radius, 12.0);
    }

    #[test]
    fn partial_merge_keeps_defaults_for_absent_fields() {
        let (merged, warnings) = merge_toml(
            r#"
[sidebar]
width = 330.0

[chat]
bubble_max_width = 620.0
"#,
        );
        assert!(warnings.is_empty(), "expected no warnings, got {warnings:?}");

        // Explicit overrides land…
        assert_eq!(merged.sidebar.width, 330.0);
        assert_eq!(merged.chat.bubble_max_width, 620.0);
        // …everything else stays at the default.
        assert_eq!(merged.sidebar.width_min, BoruTheme::default().sidebar.width_min);
        assert_eq!(merged.sidebar.padding.row_x, BoruTheme::default().sidebar.padding.row_x);
        assert_eq!(merged.colors.canvas, BoruTheme::default().colors.canvas);
        assert_eq!(merged.typography.body, BoruTheme::default().typography.body);
        assert_eq!(merged.home.hero_gap, BoruTheme::default().home.hero_gap);
        assert_eq!(merged.chat.bubble_width_ratio, BoruTheme::default().chat.bubble_width_ratio);
    }

    #[test]
    fn negative_padding_clamped_to_zero() {
        let (merged, warnings) = merge_toml(
            r#"
[spacing]
space_8 = -4.0
"#,
        );
        assert_eq!(merged.spacing.space_8, 0.0);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("spacing.space_8"), "{}", warnings[0]);
        assert!(warnings[0].contains("negative"), "{}", warnings[0]);
    }

    #[test]
    fn zero_font_size_falls_back_to_default() {
        let (merged, warnings) = merge_toml(
            r#"
[typography]
body = 0.0
"#,
        );
        assert_eq!(merged.typography.body, BoruTheme::default().typography.body);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("typography.body"), "{}", warnings[0]);
    }

    #[test]
    fn absurd_sidebar_width_clamped() {
        let (merged, warnings) = merge_toml(
            r#"
[sidebar]
width = 100000.0
"#,
        );
        assert_eq!(merged.sidebar.width, SIDEBAR_WIDTH_MAX);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("sidebar.width"), "{}", warnings[0]);
        assert!(warnings[0].contains("clamped"), "{}", warnings[0]);
    }

    #[test]
    fn absurd_size_clamped_to_max() {
        let (merged, warnings) = merge_toml(
            r#"
[spacing]
space_8 = 1.0e9
"#,
        );
        assert_eq!(merged.spacing.space_8, MAX_SIZE_PX);
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
        assert!(warnings[0].contains("chat.bubble_width_ratio"), "{}", warnings[0]);
    }

    #[test]
    fn motion_frames_clamped() {
        let (merged, warnings) = merge_toml(
            r#"
[motion]
sidebar_fade_frames = 100000
"#,
        );
        assert_eq!(merged.motion.sidebar_fade_frames, MAX_FRAMES);
        assert_eq!(warnings.len(), 1);
    }

    /// BORU-SSUI-08: screen-share tokens merge from `[screen_share.*]`
    /// through the same clamp/fallback machinery as the other groups.
    #[test]
    fn screen_share_tokens_merge_and_clamp() {
        let (merged, warnings) = merge_toml(
            r#"
[screen_share.card]
padding = 24.0
radius = -5.0

[screen_share.source_card]
width = 300.0

[screen_share.destructive]
icon_gap = 99999.0
"#,
        );
        assert_eq!(merged.screen_share.card.padding, 24.0);
        // Negative radius clamps to 0 (clamp_size0), warning emitted.
        assert_eq!(merged.screen_share.card.radius, 0.0);
        assert_eq!(merged.screen_share.source_card.width, 300.0);
        // Absurd icon gap clamps to MAX_SIZE_PX.
        assert_eq!(merged.screen_share.destructive.icon_gap, MAX_SIZE_PX);
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings[0].contains("screen_share.card.radius"),
            "{}",
            warnings[0]
        );
        assert!(
            warnings[1].contains("screen_share.destructive.icon_gap"),
            "{}",
            warnings[1]
        );
        // Unset fields keep the shared defaults.
        assert_eq!(
            merged.screen_share.card.spacing,
            BoruTheme::default().screen_share.card.spacing
        );
        assert_eq!(
            merged.screen_share.source_card.radius,
            BoruTheme::default().screen_share.source_card.radius
        );
    }

    #[test]
    fn color_channels_clamped() {
        let (merged, warnings) = merge_toml(
            r#"
[colors]
primary = [2.0, -1.0, 0.5]
"#,
        );
        assert_eq!(merged.colors.primary, Color::from_rgba(1.0, 0.0, 0.5, 1.0));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("colors.primary"), "{}", warnings[0]);
    }

    #[test]
    fn nan_values_fall_back_to_default() {
        let mut cfg = UiThemeConfig::default();
        cfg.typography = Some(TypographyConfig {
            body: Some(f32::NAN),
            ..Default::default()
        });
        cfg.spacing = Some(SpacingConfig {
            space_8: Some(f32::INFINITY),
            ..Default::default()
        });
        let (merged, warnings) = merge_ui_theme(&BoruTheme::default(), &cfg);
        assert_eq!(merged.typography.body, BoruTheme::default().typography.body);
        assert_eq!(merged.spacing.space_8, BoruTheme::default().spacing.space_8);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // New / unknown keys must not break parsing or merging (forward
        // compatibility: old binaries ignore fields they don't know).
        let (merged, warnings) = merge_toml(
            r#"
[sidebar]
width = 320.0
future_width = 9999.0

[future_group]
future_thing = 42.0
"#,
        );
        assert!(warnings.is_empty(), "expected no warnings, got {warnings:?}");
        assert_eq!(merged.sidebar.width, 320.0);
        assert_eq!(merged.sidebar.width_min, BoruTheme::default().sidebar.width_min);
    }

    #[test]
    fn older_partial_file_merges() {
        // A file written for an older version (fewer groups / fields)
        // parses and merges fine; missing leaves keep defaults.
        let (merged, warnings) = merge_toml(
            r#"
[sidebar]
width = 300.0
"#,
        );
        assert!(warnings.is_empty());
        assert_eq!(merged.sidebar.width, 300.0);
        assert_eq!(merged.chat.bubble_max_width, BoruTheme::default().chat.bubble_max_width);
        assert_eq!(merged.typography.body, BoruTheme::default().typography.body);
    }

    #[test]
    fn dark_base_merges_geometry_and_overrides_colors() {
        // The merge is mode-agnostic: geometry tokens are shared, colour
        // overrides apply on top of whichever palette is the base.
        let base = BoruTheme::dark();
        let cfg = crate::theme_config::parse_ui_theme_config(
            r##"
[colors]
primary = "#FF0000"
[sidebar]
width = 400.0
"##,
        )
        .expect("config parses");
        let (merged, warnings) = merge_ui_theme(&base, &cfg);
        assert!(warnings.is_empty());
        // Dark palette preserved where not overridden.
        assert_eq!(merged.colors.canvas, base.colors.canvas);
        // Explicit override wins over the dark palette.
        assert_eq!(merged.colors.primary, Color::from_rgb(1.0, 0.0, 0.0));
        // Geometry override lands; rest of geometry stays at base.
        assert_eq!(merged.sidebar.width, 400.0);
        assert_eq!(merged.sidebar.width_min, base.sidebar.width_min);
    }

    #[test]
    fn merge_preserves_rgba_alpha_for_surfaces_and_borders() {
        // PDF T17: RGBA/alpha must survive parse→merge for subtle
        // backgrounds and borders.
        let (merged, warnings) = merge_toml(
            r#"
[colors]
surface_elevated = [0.9, 0.9, 0.95, 0.55]
border_muted = [0.0, 0.0, 0.0, 0.12]
"#,
        );
        assert!(warnings.is_empty(), "expected no warnings, got {warnings:?}");
        let c = merged.colors;
        assert_eq!(c.surface_elevated, Color::from_rgba(0.9, 0.9, 0.95, 0.55));
        assert_eq!(c.border_muted, Color::from_rgba(0.0, 0.0, 0.0, 0.12));
    }

    #[test]
    fn merge_reports_multiple_warnings() {
        let (_, warnings) = merge_toml(
            r#"
[spacing]
space_8 = -1.0
space_16 = 999999.0
[typography]
body = 0.0
[sidebar]
width = 123456.0
"#,
        );
        assert_eq!(warnings.len(), 4);
        assert!(warnings.iter().any(|w| w.contains("spacing.space_8")));
        assert!(warnings.iter().any(|w| w.contains("spacing.space_16")));
        assert!(warnings.iter().any(|w| w.contains("typography.body")));
        assert!(warnings.iter().any(|w| w.contains("sidebar.width")));
    }

    #[test]
    fn unknown_font_family_falls_back_to_default() {
        // BORU-UI-16: a configured family that is not bundled (unavailable)
        // logs a warning and falls back to the field default — the UI never
        // renders with an unresolvable font.
        let (merged, warnings) = merge_toml(
            r#"
[typography]
display_family = "Comic Sans"
chat_family = "Papyrus"
"#,
        );
        assert_eq!(merged.typography.display_family, crate::fonts::FontFamilyKey::InterTight);
        assert_eq!(merged.typography.chat_family, crate::fonts::FontFamilyKey::Figtree);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("typography.display_family"), "{}", warnings[0]);
        assert!(warnings[1].contains("typography.chat_family"), "{}", warnings[1]);
    }

    #[test]
    fn unknown_weight_falls_back_to_default() {
        // BORU-UI-16: an unknown weight name logs a warning and falls back
        // to the role's default weight.
        let (merged, warnings) = merge_toml(
            r#"
[typography]
chat_sender_weight = "Heavy"
body_weight = "UltraLight"
"#,
        );
        assert_eq!(merged.typography.chat_sender_weight, crate::fonts::FontWeightKey::Semibold);
        assert_eq!(merged.typography.body_weight, crate::fonts::FontWeightKey::Normal);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn valid_family_and_weight_are_applied() {
        // BORU-UI-16: a known family/weight mapping is applied verbatim.
        let (merged, warnings) = merge_toml(
            r#"
[typography]
chat_family = "Public Sans"
chat_sender_weight = "Bold"
"#,
        );
        assert_eq!(merged.typography.chat_family, crate::fonts::FontFamilyKey::PublicSans);
        assert_eq!(merged.typography.chat_sender_weight, crate::fonts::FontWeightKey::Bold);
        assert!(warnings.is_empty(), "expected no warnings, got {warnings:?}");
    }

    #[test]
    fn line_height_out_of_range_clamped_and_nan_falls_back() {
        // BORU-UI-16: line-height multipliers are clamped to the sane band
        // (0.5..=4.0); non-finite values fall back to the default.
        let (merged, warnings) = merge_toml(
            r#"
[typography]
chat_message_line_height = 20.0
body_line_height = 0.01
"#,
        );
        assert_eq!(merged.typography.chat_message_line_height, 4.0);
        assert_eq!(merged.typography.body_line_height, 0.5);
        assert_eq!(warnings.len(), 2);

        let mut cfg = UiThemeConfig::default();
        cfg.typography = Some(TypographyConfig {
            chat_message_line_height: Some(f32::NAN),
            ..Default::default()
        });
        let (merged, warnings) = merge_ui_theme(&BoruTheme::default(), &cfg);
        assert_eq!(
            merged.typography.chat_message_line_height,
            BoruTheme::default().typography.chat_message_line_height
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("typography.chat_message_line_height"), "{}", warnings[0]);
    }
}
