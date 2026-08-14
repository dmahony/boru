//! Boru UI theme override config — `boru-ui.toml` (BORU-UI-04 / PDF Task 4).
//!
//! A development-only, human-editable TOML file that overrides visual
//! values on top of [`BoruTheme::default()`](crate::theme::BoruTheme).
//! It contains **only visual values** — no networking, chat, file transfer,
//! video, tunnel, lobby, room or persistence behaviour lives here.
//!
//! ## Design rules
//!
//! - **Mirrors the theme model.** Every group in `BoruTheme` has a matching
//!   `*Config` struct here with the same field names (`sidebar.width`,
//!   `chat.bubble_max_width`, `colors.canvas`, …). BORU-UI-05 merges these
//!   into `BoruTheme::default()`.
//! - **Missing keys are fine.** Every leaf is `Option<T>` and every struct
//!   carries `#[serde(default)]`, so a partial file (or an empty one)
//!   deserializes to `None` leaves — the merge step later falls back to
//!   `BoruTheme` defaults.
//! - **Missing file is fine.** [`load_ui_theme_config`] returns an empty
//!   config when `<data_dir>/boru-ui.toml` does not exist; startup never
//!   fails because of the dev file.
//! - **Malformed files are reported, not fatal.** Parse errors surface as a
//!   structured [`UiThemeConfigError`] with the file path and line/column;
//!   the caller logs it and keeps the last known-good theme.
//!
//! The sample file (`boru-ui.example.toml`, repo root) documents every
//! group with valid units and ranges.

use std::path::{Path, PathBuf};

/// File name of the dev theme override file (inside the data dir).
pub const UI_CONFIG_FILE_NAME: &str = "boru-ui.toml";

// ── Colour value ──────────────────────────────────────────────────────
//
// `iced::Color` has no serde support in this build, so the config uses its
// own small colour type that parses from a human-editable TOML value:
//
//   canvas = "#F7F9F8"        # hex, 6 or 8 digits
//   primary = [0.094, 0.498, 0.314]        # [r, g, b] floats 0..=1
//   dialog_backdrop = [0.0, 0.0, 0.0, 0.35]  # [r, g, b, a] floats 0..=1
//
// `ColorValue::to_iced()` converts to `iced::Color` at merge time.

/// A human-editable RGBA colour (0.0..=1.0 components).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorValue {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl ColorValue {
    /// Convert to the Iced colour used by the theme model.
    pub fn to_iced(&self) -> iced::Color {
        iced::Color::from_rgba(self.r, self.g, self.b, self.a)
    }

    fn from_hex(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if !(hex.len() == 6 || hex.len() == 8) {
            return None;
        }
        let mut channels = [0u8; 4];
        for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
            let pair = std::str::from_utf8(pair).ok()?;
            channels[i] = u8::from_str_radix(pair, 16).ok()?;
        }
        let (r, g, b) = (channels[0], channels[1], channels[2]);
        let a = if hex.len() == 8 { channels[3] } else { 255 };
        Some(ColorValue {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        })
    }
}

/// Serialize as the `[r, g, b(, a)]` float array — the exact inverse of the
/// `visit_seq` branch of the `Deserialize` impl below, so a save→load
/// round-trip is lossless (hex-string output would quantize to 8-bit
/// channels and break float equality). Alpha is omitted when exactly 1.0,
/// matching the deserializer's default, so saved files stay compact
/// (`primary = [0.1, 0.2, 0.3]` rather than `[0.1, 0.2, 0.3, 1.0]`).
impl serde::Serialize for ColorValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let len = if self.a == 1.0 { 3 } else { 4 };
        let mut seq = serializer.serialize_seq(Some(len))?;
        seq.serialize_element(&self.r)?;
        seq.serialize_element(&self.g)?;
        seq.serialize_element(&self.b)?;
        if len == 4 {
            seq.serialize_element(&self.a)?;
        }
        seq.end()
    }
}

impl<'de> serde::Deserialize<'de> for ColorValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ColorVisitor;

        impl<'de> serde::de::Visitor<'de> for ColorVisitor {
            type Value = ColorValue;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "a colour as \"#RRGGBB\", \"#RRGGBBAA\", or [r, g, b(, a)] with 0..=1 floats",
                )
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                ColorValue::from_hex(v).ok_or_else(|| {
                    E::custom(format!(
                        "invalid colour {v:?}: expected \"#RRGGBB\" or \"#RRGGBBAA\""
                    ))
                })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let r = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("expected colour [r, g, b(, a)]"))?;
                let g = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("expected colour [r, g, b(, a)]"))?;
                let b = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("expected colour [r, g, b(, a)]"))?;
                let a = seq.next_element()?.unwrap_or(1.0);
                Ok(ColorValue { r, g, b, a })
            }
        }

        deserializer.deserialize_any(ColorVisitor)
    }
}

// ── Config group macro ────────────────────────────────────────────────
//
// Every `*Config` struct is a 1:1 mirror of a `BoruTheme` group where each
// leaf is `Option<T>`. The macro generates `#[serde(default)]` + derives
// for a flat list of (field, type) pairs, keeping the ~200 fields compact
// and reviewable. Field names MUST match the theme model so BORU-UI-05 can
// merge them without a mapping table.

macro_rules! config_group {
    ($(#[$doc:meta])* $name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
        #[serde(default)]
        pub struct $name {
            $(pub $field: Option<$ty>,)*
        }
    };
}

// ── Colour tokens (mirrors ColorTokens) ───────────────────────────────

config_group! {
    /// Colour overrides (`BoruTheme::colors`). Each entry is a hex string
    /// `"#RRGGBB[AA]"` or an `[r, g, b(, a)]` array of 0..=1 floats.
    ColorConfig {
        canvas: ColorValue,
        sidebar: ColorValue,
        surface: ColorValue,
        surface_elevated: ColorValue,
        surface_selected: ColorValue,
        surface_hover: ColorValue,
        surface_pressed: ColorValue,
        surface_secondary: ColorValue,
        input_bg: ColorValue,
        border_muted: ColorValue,
        border_strong: ColorValue,
        text_primary: ColorValue,
        text_secondary: ColorValue,
        text_muted: ColorValue,
        text_local_label: ColorValue,
        text_local_body: ColorValue,
        text_remote_label: ColorValue,
        text_remote_body: ColorValue,
        primary: ColorValue,
        primary_hover: ColorValue,
        primary_pressed: ColorValue,
        primary_soft: ColorValue,
        success: ColorValue,
        danger: ColorValue,
        warning: ColorValue,
        focus: ColorValue,
        soft_tint_alpha: f32,
        dialog_backdrop: ColorValue,
        incoming_call_backdrop: ColorValue,
        chat_overlay_backdrop: ColorValue,
        chat_search_backdrop: ColorValue,
        panel_shadow: ColorValue,
        dialog_panel_bg: ColorValue,
        dialog_panel_border: ColorValue,
        media_frame_bg: ColorValue,
        media_frame_border: ColorValue,
        media_frame_overlay: ColorValue,
        on_media_text: ColorValue,
        glyph_disabled: ColorValue,
        glyph_muted: ColorValue,
        glyph_muted_dark: ColorValue,
        avatar_fallback: ColorValue,
        tag_text: ColorValue,
        tag_bg: ColorValue,
        tag_bg_pressed: ColorValue,
        download_completed: ColorValue,
        download_temporary: ColorValue,
        download_terminal: ColorValue,
        download_cancelled: ColorValue,
        request_pending: ColorValue,
        request_accepted: ColorValue,
        request_declined: ColorValue,
        settings_success: ColorValue,
        settings_danger: ColorValue,
        settings_danger_strong: ColorValue,
        settings_heading_text: ColorValue,
        expanded_video_backdrop: ColorValue,
        lightbox_backdrop: ColorValue,
        status_card_bg_top: ColorValue,
        status_card_bg_mid: ColorValue,
        status_card_bg_bottom: ColorValue,
        status_card_border: ColorValue,
        status_connected: ColorValue,
        status_primary_text: ColorValue,
        status_secondary_text: ColorValue,
        status_network_line: ColorValue,
        status_network_node: ColorValue,
        status_warning: ColorValue,
        status_danger: ColorValue,
    }
}

// ── Typography (mirrors TypographyTokens) ─────────────────────────────

config_group! {
    /// Typography size overrides (`BoruTheme::typography`), px.
    TypographyConfig {
        display_heading: f32,
        page_title: f32,
        section_title: f32,
        card_title: f32,
        body: f32,
        body_emphasised: f32,
        button_label: f32,
        supporting_text: f32,
        metadata: f32,
        chat_message: f32,
        chat_sender: f32,
        chat_metadata: f32,
        composer_text: f32,
        technical_value: f32,
        brand_wordmark: f32,
        home_subtitle: f32,
        dialog_title: f32,
        dialog_subtitle: f32,
        sidebar_name: f32,
        section_label: f32,
        badge: f32,
        call_name: f32,
        call_name_active: f32,
        call_remote_name: f32,
        call_status: f32,
        call_duration: f32,
        call_avatar_glyph: f32,
        call_avatar_glyph_large: f32,
        call_pip_label: f32,
        // ── BORU-UI-16: font family choices per role group ──
        // Values are family-name strings ("Figtree", "Public Sans",
        // "Inter Tight", "JetBrains Mono", "Raleway"). Unknown names are
        // rejected at merge time with a warning and fall back to the
        // bundled default family.
        display_family: String,
        ui_family: String,
        chat_family: String,
        technical_family: String,
        brand_family: String,
        // ── BORU-UI-16: weight mapping per canonical role ──
        // Values are weight-name strings ("Normal", "Medium", "Semibold",
        // "Bold", "ExtraBold"). Unknown names fall back to the role's
        // TypeRole weight with a warning.
        display_heading_weight: String,
        page_title_weight: String,
        section_title_weight: String,
        card_title_weight: String,
        body_weight: String,
        body_emphasised_weight: String,
        button_label_weight: String,
        supporting_text_weight: String,
        metadata_weight: String,
        chat_message_weight: String,
        chat_sender_weight: String,
        chat_metadata_weight: String,
        composer_text_weight: String,
        technical_value_weight: String,
        brand_wordmark_weight: String,
        // ── BORU-UI-16: line-height mapping per canonical role ──
        // Relative multipliers (1.0 = 1× the font size). Clamped to a sane
        // 0.5..=4.0 band at merge time.
        display_heading_line_height: f32,
        page_title_line_height: f32,
        section_title_line_height: f32,
        card_title_line_height: f32,
        body_line_height: f32,
        body_emphasised_line_height: f32,
        button_label_line_height: f32,
        supporting_text_line_height: f32,
        metadata_line_height: f32,
        chat_message_line_height: f32,
        chat_sender_line_height: f32,
        chat_metadata_line_height: f32,
        composer_text_line_height: f32,
        technical_value_line_height: f32,
        brand_wordmark_line_height: f32,
    }
}

// ── Spacing (mirrors SpacingTokens) ───────────────────────────────────

config_group! {
    /// Spacing scale overrides (`BoruTheme::spacing`), px.
    SpacingConfig {
        space_2: f32,
        space_4: f32,
        space_6: f32,
        space_8: f32,
        space_10: f32,
        space_12: f32,
        space_16: f32,
        space_18: f32,
        space_20: f32,
        space_24: f32,
        space_28: f32,
        space_32: f32,
        space_40: f32,
        control_height: f32,
        control_height_compact: f32,
    }
}

// ── Radii (mirrors RadiusTokens) ──────────────────────────────────────

config_group! {
    /// Corner radius overrides (`BoruTheme::radii`), px.
    RadiusConfig {
        none: f32,
        sm: f32,
        md: f32,
        lg: f32,
        xl: f32,
        card: f32,
        pill: f32,
        avatar_container: f32,
        call_avatar: f32,
        media_frame: f32,
        attachment: f32,
        dialog: f32,
        picker_cell: f32,
        control_sm: f32,
        status_divider: f32,
        security_pill: f32,
    }
}

// ── Icons (mirrors IconTokens) ────────────────────────────────────────

config_group! {
    /// Icon size overrides (`BoruTheme::icons`), px.
    IconConfig {
        xs: f32,
        sm: f32,
        md: f32,
        lg: f32,
        xl: f32,
        sidebar_utility: f32,
    }
}

// ── Avatars (mirrors AvatarTokens) ────────────────────────────────────

config_group! {
    /// Avatar size overrides (`BoruTheme::avatars`), px.
    AvatarConfig {
        sm: f32,
        md: f32,
        lg: f32,
        profile: f32,
        chat_list: f32,
        chat_header: f32,
        msg: f32,
        status_dot_sm: f32,
        status_dot_lg: f32,
    }
}

// ── Lists / rows (mirrors ListTokens) ─────────────────────────────────

config_group! {
    /// List/row height overrides (`BoruTheme::lists`), px.
    ListConfig {
        card_row_height: f32,
        peer_row_height: f32,
        default_list_max_height: f32,
        table_row_height: f32,
        table_row_height_compact: f32,
        chip_height: f32,
        peer_panel_max_height: f32,
        progress_bar_height: f32,
        progress_bar_height_bold: f32,
    }
}

// ── Borders (mirrors BorderTokens) ────────────────────────────────────

config_group! {
    /// Border width overrides (`BoruTheme::borders`), px.
    BorderConfig {
        hairline: f32,
        focus: f32,
        tab_active: f32,
        selected_row: f32,
        media_frame: f32,
    }
}

// ── Responsive (mirrors ResponsiveTokens) ─────────────────────────────

config_group! {
    /// Responsive / layout breakpoint overrides (`BoruTheme::responsive`), px.
    ResponsiveConfig {
        viewport_ref_width: f32,
        viewport_ref_height: f32,
        viewport_min_width: f32,
        viewport_min_height: f32,
        viewport_lg_width: f32,
        viewport_lg_height: f32,
        viewport_xl_width: f32,
        viewport_xl_height: f32,
        content_max_width: f32,
        dashboard_max_width: f32,
        home_two_col_content: f32,
        home_quick_one_col_content: f32,
        home_quick_four_col_content: f32,
        home_illustration_full_content: f32,
        home_illustration_hide_content: f32,
        home_compact_header_content: f32,
    }
}

// ── Motion (mirrors MotionTokens) ─────────────────────────────────────

config_group! {
    /// Presentation motion overrides (`BoruTheme::motion`).
    MotionConfig {
        sidebar_fade_frames: u32,
    }
}

// ── Sidebar (mirrors SidebarTheme) ────────────────────────────────────

config_group! {
    /// Sidebar padding overrides (`BoruTheme::sidebar.padding`), px.
    SidebarPaddingConfig {
        brand_top: f32,
        brand_bottom: f32,
        identity_top: f32,
        identity_bottom: f32,
        section_top: f32,
        utility_top: f32,
        utility_bottom: f32,
        row_x: f32,
        join_top: f32,
        join_bottom: f32,
    }
}

config_group! {
    /// Sidebar / global shell overrides (`BoruTheme::sidebar`), px.
    SidebarConfig {
        width: f32,
        width_min: f32,
        width_max: f32,
        inset: f32,
        item_radius: f32,
        avatar_container_radius: f32,
        utility_icon_size: f32,
        name_size: f32,
        section_label_size: f32,
        padding: SidebarPaddingConfig,
    }
}

// ── Home (mirrors HomeTheme) ──────────────────────────────────────────

config_group! {
    /// Home dashboard overrides (`BoruTheme::home`), px (line height is a ratio).
    HomeConfig {
        peers_body_min: f32,
        activity_row_height: f32,
        hero_gap: f32,
        quick_action_gap: f32,
        quick_action_icon_size: f32,
        quick_action_title_size: f32,
        quick_action_desc_size: f32,
        quick_action_desc_line_height: f32,
        status_card_text_min_width_medium: f32,
        status_card_mesh_max_width: f32,
        status_card_padding_x: f32,
        status_icon_text_gap_full: f32,
        status_icon_text_gap_medium: f32,
        status_text_graph_gap_full: f32,
        status_text_graph_gap_medium: f32,
        status_divider_width: f32,
        status_divider_height: f32,
        status_divider_radius: f32,
        security_pill_radius: f32,
        show_activity_feed: bool,
    }
}

// ── Chat (mirrors ChatTheme) ──────────────────────────────────────────

config_group! {
    /// Chat message-list + composer overrides (`BoruTheme::chat`), px
    /// (`bubble_width_ratio` is a fraction 0..1).
    ChatConfig {
        spinner_size: f32,
        context_menu_width: f32,
        emoji_picker_width: f32,
        emoji_picker_scroll_height: f32,
        gif_picker_width: f32,
        gif_picker_scroll_height: f32,
        gif_thumbnail_width: f32,
        gif_thumbnail_height: f32,
        screen_share_w: f32,
        screen_share_h: f32,
        bubble_max_width: f32,
        bubble_width_ratio: f32,
        message_max_width: f32,
        image_preview_max_width: f32,
        image_preview_max_height: f32,
    }
}

// ── Attachments (mirrors AttachmentTheme) ─────────────────────────────

config_group! {
    /// File-dashboard table column widths (`BoruTheme::attachments.file_table`), px.
    FileTableColumnsConfig {
        size_col: f32,
        source_col: f32,
        ago_col: f32,
        peer_col: f32,
        started_col: f32,
        state_col: f32,
        direction_col: f32,
        event_col: f32,
        details_col: f32,
        download_started_col: f32,
        download_state_col: f32,
        activity_ago_col: f32,
    }
}

config_group! {
    /// "Files I'm Sharing" table column widths (`BoruTheme::attachments.shared_table`), px.
    SharedTableColumnsConfig {
        shared_with: f32,
        size: f32,
        shared_on: f32,
        downloads: f32,
        actions: f32,
    }
}

config_group! {
    /// Video attachment card overrides (`BoruTheme::attachments.video`), px.
    VideoConfig {
        narrow_breakpoint: f32,
        medium_breakpoint: f32,
        play_overlay_size: f32,
        header_filename_max_width: f32,
        controls_slider_width: f32,
    }
}

config_group! {
    /// Attachment overrides (`BoruTheme::attachments`), px.
    AttachmentConfig {
        empty_state_height: f32,
        menu_width: f32,
        chip_avatar_size: f32,
        chip_label_size: f32,
        detail_label_width: f32,
        progress_bar_girth: f32,
        progress_pct_label_width: f32,
        progress_slot_height: f32,
        detail_slot_height: f32,
        policy_slot_height: f32,
        action_button_line: f32,
        search_width_medium: f32,
        search_width_full: f32,
        file_table: FileTableColumnsConfig,
        shared_table: SharedTableColumnsConfig,
        video: VideoConfig,
    }
}

// ── Rooms (mirrors RoomTheme) ─────────────────────────────────────────

config_group! {
    /// Public room / discover overrides (`BoruTheme::rooms`), px.
    RoomConfig {
        catalogue_row_height: f32,
        overscan: f32,
        banner_width: f32,
        progress_length: f32,
        progress_girth: f32,
    }
}

// ── Tunnels (mirrors TunnelTheme) ─────────────────────────────────────

config_group! {
    /// Tunnel overrides (`BoruTheme::tunnels`), px.
    TunnelConfig {
        chip_padding_x: f32,
        chip_padding_y: f32,
    }
}

// ── Dialogs (mirrors DialogTheme) ─────────────────────────────────────

config_group! {
    /// Dialog overrides (`BoruTheme::dialogs`), px.
    DialogConfig {
        avatar_size: f32,
        avatar_glyph_size: f32,
        title_size: f32,
        body_size: f32,
        spacing: f32,
        padding: f32,
        control_padding_x: f32,
        control_padding_y: f32,
        control_spacing: f32,
    }
}

// ── Calls (mirrors CallTheme) ─────────────────────────────────────────

config_group! {
    /// Call screen overrides (`BoruTheme::calls`), px.
    CallConfig {
        avatar_size: f32,
        avatar_glyph_size: f32,
        avatar_glyph_size_large: f32,
        pip_w: f32,
        pip_h: f32,
        controls_gap: f32,
    }
}

// ── Controls (mirrors ControlTokens) ──────────────────────────────────

config_group! {
    /// Settings / generic control overrides (`BoruTheme::controls`), px.
    ControlConfig {
        header_height: f32,
        slider_width: f32,
        color_picker_radius: f32,
        color_picker_bar_radius: f32,
    }
}

// ── Root config ───────────────────────────────────────────────────────

/// Root of the `boru-ui.toml` dev theme override file.
///
/// Every group is optional; a missing group (or a missing file) means
/// "no overrides" and the merge step (BORU-UI-05) falls back to
/// `BoruTheme::default()`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct UiThemeConfig {
    pub colors: Option<ColorConfig>,
    pub typography: Option<TypographyConfig>,
    pub spacing: Option<SpacingConfig>,
    pub radii: Option<RadiusConfig>,
    pub icons: Option<IconConfig>,
    pub avatars: Option<AvatarConfig>,
    pub lists: Option<ListConfig>,
    pub borders: Option<BorderConfig>,
    pub responsive: Option<ResponsiveConfig>,
    pub motion: Option<MotionConfig>,
    pub sidebar: Option<SidebarConfig>,
    pub home: Option<HomeConfig>,
    pub chat: Option<ChatConfig>,
    pub attachments: Option<AttachmentConfig>,
    pub rooms: Option<RoomConfig>,
    pub tunnels: Option<TunnelConfig>,
    pub dialogs: Option<DialogConfig>,
    pub calls: Option<CallConfig>,
    pub controls: Option<ControlConfig>,
}

// ── Load path ─────────────────────────────────────────────────────────

/// Structured error returned when the dev theme override file cannot be
/// used. This is the developer-error reporting path that BORU-UI-18 builds
/// on later — it carries the offending path and (for parse errors) the
/// line/column from the TOML parser.
#[derive(Debug)]
pub enum UiThemeConfigError {
    /// The file does not exist. Only the inspector's explicit "Reload From
    /// Disk" action treats this as an error (there is nothing to reload);
    /// the startup/watcher load path treats a missing file as "no
    /// overrides" and returns an empty config instead.
    #[cfg(feature = "dev-ui")]
    NotFound { path: PathBuf },
    /// The file exists but could not be read (permissions, I/O, …).
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file exists but is not valid TOML / not a valid theme config.
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl std::fmt::Display for UiThemeConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(feature = "dev-ui")]
            UiThemeConfigError::NotFound { path } => {
                write!(f, "dev theme override {} not found", path.display())
            }
            UiThemeConfigError::Io { path, source } => write!(
                f,
                "cannot read dev theme override {}: {source}",
                path.display()
            ),
            UiThemeConfigError::Parse { path, source } => {
                // The toml error's Display already includes the offending
                // line/column when the parser has a span (syntax errors);
                // serde type-mismatch errors show the field/key path.
                write!(f, "invalid dev theme override {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for UiThemeConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(feature = "dev-ui")]
            UiThemeConfigError::NotFound { .. } => None,
            UiThemeConfigError::Io { source, .. } => Some(source),
            UiThemeConfigError::Parse { source, .. } => Some(source),
        }
    }
}

/// Parse theme overrides from a TOML string.
///
/// Missing keys are allowed (every leaf is `Option`); the string does not
/// need to contain any group. A syntactically invalid file returns a
/// [`toml::de::Error`] which callers wrap into [`UiThemeConfigError::Parse`].
pub fn parse_ui_theme_config(text: &str) -> Result<UiThemeConfig, toml::de::Error> {
    toml::from_str(text)
}

/// Load theme overrides from `<data_dir>/boru-ui.toml`.
///
/// - **Missing file** → `Ok(UiThemeConfig::default())` (empty overrides;
///   startup never fails because the dev file is absent).
/// - **Unreadable file** (permissions etc.) → `Err(UiThemeConfigError::Io)`.
/// - **Malformed file** → `Err(UiThemeConfigError::Parse)` with line/column;
///   the caller keeps the last known-good theme and logs the error.
pub fn load_ui_theme_config(data_dir: &Path) -> Result<UiThemeConfig, UiThemeConfigError> {
    let path = data_dir.join(UI_CONFIG_FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UiThemeConfig::default());
        }
        Err(source) => return Err(UiThemeConfigError::Io { path, source }),
    };
    parse_ui_theme_config(&text).map_err(|source| UiThemeConfigError::Parse { path, source })
}

/// Reload theme overrides from `<data_dir>/boru-ui.toml` for the
/// inspector's "Reload From Disk" action (PDF Task 13 / BORU-UI-13).
///
/// Unlike [`load_ui_theme_config`] — which treats a **missing** file as
/// "no overrides" so startup never fails — an explicit reload from disk
/// treats a missing file as an error: there is nothing to reload, so the
/// caller keeps the current theme and reports the error (BORU-UI-18).
/// Malformed files keep the current theme too; the parse error carries
/// the path and (where available) line/column.
#[cfg(feature = "dev-ui")]
pub fn reload_ui_theme_config(data_dir: &Path) -> Result<UiThemeConfig, UiThemeConfigError> {
    let path = data_dir.join(UI_CONFIG_FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(UiThemeConfigError::NotFound { path });
        }
        Err(source) => return Err(UiThemeConfigError::Io { path, source }),
    };
    parse_ui_theme_config(&text).map_err(|source| UiThemeConfigError::Parse { path, source })
}

// ── Save path (BORU-UI-12 / PDF Task 12) ────────────────────────────

/// Serialize the current editable theme overrides to TOML text.
///
/// Only `Some` (present) leaves are emitted, in stable struct field order
/// (serde serializes struct fields in declaration order), so Git diffs of
/// `boru-ui.toml` stay readable and minimal: editing one field adds/removes
/// exactly that key, and defaults remain code defaults. `None` leaves are
/// omitted entirely — the merge path (BORU-UI-05) treats a missing key as
/// "keep `BoruTheme::default()`", so the round trip
/// `config → toml → parse → merge` reproduces the same active theme.
///
/// A short header comment makes the file self-describing (parsers ignore
/// comments, so it does not affect the round trip).
#[cfg(feature = "dev-ui")]
pub fn ui_theme_config_to_toml(config: &UiThemeConfig) -> Result<String, toml::ser::Error> {
    let mut text =
        String::from("# boru-ui.toml — Boru dev theme overrides (saved from the UI Inspector).\n");
    text.push_str("# Visual values only; missing keys fall back to BoruTheme::default().\n");
    text.push_str(&toml::to_string(config)?);
    Ok(text)
}

/// Save the current editable theme overrides to `<data_dir>/boru-ui.toml`.
///
/// The write is **atomic** (temp sibling + `fsync` + rename via
/// `boru_core::chat_core::atomic_write::atomic_write_bytes`), so the dev
/// file watcher (BORU-UI-06) can never observe a partial file — it either
/// sees the previous complete file or the new complete file. Only theme
/// overrides are persisted; no non-theme state is ever written here.
///
/// Returns the path written on success, or a developer-facing error string.
#[cfg(feature = "dev-ui")]
pub fn save_ui_theme_config(data_dir: &Path, config: &UiThemeConfig) -> Result<PathBuf, String> {
    let path = data_dir.join(UI_CONFIG_FILE_NAME);
    let text = ui_theme_config_to_toml(config)
        .map_err(|e| format!("cannot serialize {}: {e}", path.display()))?;
    boru_core::chat_core::atomic_write::atomic_write_bytes(
        &path,
        text.as_bytes(),
        "dev theme overrides",
    )
    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A full config exercising every group (values are illustrative but
    /// within the documented ranges).
    const FULL_TOML: &str = r##"
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

    #[test]
    fn parse_full_config() {
        let cfg = parse_ui_theme_config(FULL_TOML).expect("full config parses");

        let colors = cfg.colors.expect("colors group present");
        assert_eq!(
            colors.canvas,
            Some(ColorValue {
                r: 247.0 / 255.0,
                g: 249.0 / 255.0,
                b: 248.0 / 255.0,
                a: 1.0
            })
        );
        assert_eq!(colors.soft_tint_alpha, Some(0.08));
        assert_eq!(
            colors.surface_elevated,
            Some(ColorValue {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0
            })
        );
        assert_eq!(
            colors.dialog_backdrop,
            Some(ColorValue {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.35
            })
        );

        let typography = cfg.typography.expect("typography group present");
        assert_eq!(typography.body, Some(15.0));

        let sidebar = cfg.sidebar.expect("sidebar group present");
        assert_eq!(sidebar.width, Some(270.0));
        let padding = sidebar.padding.expect("sidebar.padding present");
        assert_eq!(padding.row_x, Some(12.0));

        let chat = cfg.chat.expect("chat group present");
        assert_eq!(chat.bubble_max_width, Some(560.0));
        assert_eq!(chat.bubble_width_ratio, Some(0.68));

        let attachments = cfg.attachments.expect("attachments group present");
        let video = attachments.video.expect("attachments.video present");
        assert_eq!(video.narrow_breakpoint, Some(560.0));

        assert_eq!(
            cfg.motion.expect("motion present").sidebar_fade_frames,
            Some(5)
        );
        assert_eq!(
            cfg.tunnels.expect("tunnels present").chip_padding_x,
            Some(6.0)
        );
        assert_eq!(
            cfg.controls.expect("controls present").header_height,
            Some(52.0)
        );
    }

    #[test]
    fn parse_partial_config_missing_keys() {
        let cfg = parse_ui_theme_config(
            r#"
[sidebar]
width = 300.0

[attachments.video]
play_overlay_size = 70.0
"#,
        )
        .expect("partial config parses");

        // Present group, missing leaves → None.
        let sidebar = cfg.sidebar.expect("sidebar group present");
        assert_eq!(sidebar.width, Some(300.0));
        assert_eq!(sidebar.item_radius, None, "missing leaf falls back to None");
        assert_eq!(
            sidebar.padding, None,
            "missing nested table falls back to None"
        );

        let video = cfg
            .attachments
            .expect("attachments group present")
            .video
            .expect("attachments.video present");
        assert_eq!(video.play_overlay_size, Some(70.0));
        assert_eq!(video.narrow_breakpoint, None);

        // Missing groups → None.
        assert!(cfg.colors.is_none());
        assert!(cfg.home.is_none());
        assert!(cfg.rooms.is_none());
        assert!(cfg.dialogs.is_none());
        assert!(cfg.calls.is_none());
    }

    #[test]
    fn parse_empty_string_returns_empty_config() {
        let cfg = parse_ui_theme_config("").expect("empty string parses");
        assert_eq!(cfg, UiThemeConfig::default());
        assert!(cfg.sidebar.is_none() && cfg.colors.is_none() && cfg.chat.is_none());
    }

    #[test]
    fn malformed_toml_surfaces_error() {
        let err = parse_ui_theme_config("[sidebar\nwidth = 'unclosed").expect_err("malformed TOML");
        // The TOML error should carry a byte span for the developer.
        assert!(err.span().is_some(), "span available on toml parse error");

        // Load path wraps it into the structured error with path + line/col
        // (the toml Display renders "line N, column M" from the span).
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join(UI_CONFIG_FILE_NAME),
            "[sidebar\nwidth = 'unclosed",
        )
        .expect("write malformed file");
        let err = load_ui_theme_config(dir.path()).expect_err("malformed file is an error");
        let msg = err.to_string();
        match err {
            UiThemeConfigError::Parse { path, source } => {
                assert!(path.ends_with(UI_CONFIG_FILE_NAME));
                assert!(source.span().is_some());
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
        assert!(msg.contains(UI_CONFIG_FILE_NAME));
        assert!(
            msg.contains("line"),
            "error Display includes line info: {msg}"
        );
    }

    #[test]
    fn malformed_value_type_surfaces_error() {
        // `width` is a number; a string is a type mismatch.
        let err = parse_ui_theme_config("[sidebar]\nwidth = 'wide'").expect_err("type mismatch");
        // Type-mismatch errors may carry a span; the Display at minimum names
        // the field/key so the developer knows what to fix.
        let msg = err.to_string();
        assert!(!msg.is_empty());
    }

    #[test]
    fn missing_file_returns_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cfg = load_ui_theme_config(dir.path()).expect("missing file is Ok(default)");
        assert_eq!(cfg, UiThemeConfig::default());
        assert!(cfg.sidebar.is_none());
    }

    #[test]
    fn load_full_file_round_trip() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join(UI_CONFIG_FILE_NAME), FULL_TOML).expect("write config");
        let cfg = load_ui_theme_config(dir.path()).expect("loads");
        assert_eq!(cfg.sidebar.expect("sidebar").width, Some(270.0));
        assert_eq!(cfg.chat.expect("chat").bubble_max_width, Some(560.0));
    }

    #[test]
    fn color_value_hex_parsing() {
        assert_eq!(
            ColorValue::from_hex("#187F50"),
            Some(ColorValue {
                r: 0x18 as f32 / 255.0,
                g: 0x7F as f32 / 255.0,
                b: 0x50 as f32 / 255.0,
                a: 1.0
            })
        );
        assert_eq!(
            ColorValue::from_hex("#187F5080"),
            Some(ColorValue {
                r: 0x18 as f32 / 255.0,
                g: 0x7F as f32 / 255.0,
                b: 0x50 as f32 / 255.0,
                a: 0x80 as f32 / 255.0
            })
        );
        assert_eq!(
            ColorValue::from_hex("187F50"),
            Some(ColorValue {
                r: 0x18 as f32 / 255.0,
                g: 0x7F as f32 / 255.0,
                b: 0x50 as f32 / 255.0,
                a: 1.0
            }),
            "leading # optional"
        );
        assert_eq!(ColorValue::from_hex("#12"), None);
        assert_eq!(ColorValue::from_hex("#GGGGGG"), None);
    }

    #[test]
    fn color_value_serde_string_and_array() {
        // Hex string form through the config struct.
        let cfg: ColorConfig = toml::from_str("canvas = \"#F7F9F8\"").unwrap();
        assert_eq!(
            cfg.canvas,
            Some(ColorValue {
                r: 247.0 / 255.0,
                g: 249.0 / 255.0,
                b: 248.0 / 255.0,
                a: 1.0
            })
        );

        // Array form with alpha.
        let cfg: ColorConfig = toml::from_str("dialog_backdrop = [0.0, 0.0, 0.0, 0.35]").unwrap();
        assert_eq!(
            cfg.dialog_backdrop,
            Some(ColorValue {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.35
            })
        );
        assert_eq!(cfg.canvas, None);

        // Bad hex string is a serde error.
        let err = toml::from_str::<ColorConfig>("canvas = \"#ZZZZZZ\"");
        assert!(err.is_err());
    }

    #[test]
    fn color_value_to_iced() {
        let c = ColorValue {
            r: 0.1,
            g: 0.2,
            b: 0.3,
            a: 0.4,
        };
        let ic = c.to_iced();
        assert!((ic.r - 0.1).abs() < 1e-6);
        assert!((ic.a - 0.4).abs() < 1e-6);
    }

    #[test]
    fn io_error_surfaces_path() {
        // A path that exists as a directory (not a file) yields an Io error
        // on some platforms; on Linux read_to_string on a dir fails with
        // EISDIR (io error). If the platform returns Ok, skip the assertion.
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(dir.path().join(UI_CONFIG_FILE_NAME))
            .expect("create dir with file name");
        match load_ui_theme_config(dir.path()) {
            Ok(_) => { /* platform returned empty for a dir — acceptable */ }
            Err(UiThemeConfigError::Io { path, .. }) => {
                assert!(path.ends_with(UI_CONFIG_FILE_NAME));
            }
            Err(other) => panic!("expected Io error, got {other:?}"),
        }
    }

    // ── Reload path (BORU-UI-13 / PDF Task 13) — dev-ui feature only ──

    /// Reload From Disk on a missing file must error (unlike the startup
    /// load path which returns an empty config) so the app keeps the
    /// current theme and reports the error (BORU-UI-18).
    #[cfg(feature = "dev-ui")]
    #[test]
    fn reload_missing_file_is_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(!dir.path().join(UI_CONFIG_FILE_NAME).exists());
        match reload_ui_theme_config(dir.path()) {
            Err(UiThemeConfigError::NotFound { path }) => {
                assert!(path.ends_with(UI_CONFIG_FILE_NAME));
            }
            other => panic!("expected NotFound error, got {other:?}"),
        }
    }

    /// Reload From Disk on a valid file returns exactly the overrides.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn reload_valid_file_returns_overrides() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join(UI_CONFIG_FILE_NAME),
            "[sidebar]\nwidth = 280.0\n",
        )
        .expect("write theme file");
        let cfg = reload_ui_theme_config(dir.path()).expect("reloads");
        assert_eq!(
            cfg.sidebar.expect("sidebar group").width,
            Some(280.0),
            "reload parses the overrides"
        );
    }

    /// Reload From Disk on a malformed file errors with the path + parser
    /// detail, so the app keeps the current theme and reports the error.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn reload_malformed_file_is_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join(UI_CONFIG_FILE_NAME), "[sidebar\nwidth =")
            .expect("write malformed theme file");
        match reload_ui_theme_config(dir.path()) {
            Err(UiThemeConfigError::Parse { path, source }) => {
                assert!(path.ends_with(UI_CONFIG_FILE_NAME));
                let msg = source.to_string();
                assert!(
                    !msg.is_empty(),
                    "parser detail is included for the developer"
                );
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    // ── Save path (BORU-UI-12 / PDF Task 12) — dev-ui feature only ────

    /// Build the config equivalent of `FULL_TOML` by parsing it, then
    /// serialize → parse → merge and compare to the direct merge. This is
    /// the PDF Task 12 round trip: current theme → toml → parse → merge
    /// reproduces the same active theme.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn save_round_trip_merge_reproduces_same_theme() {
        let cfg = parse_ui_theme_config(FULL_TOML).expect("full config parses");
        let (before, _) =
            crate::theme_merge::merge_ui_theme(&crate::theme::BoruTheme::default(), &cfg);

        let text = ui_theme_config_to_toml(&cfg).expect("serializes");
        let reparsed = parse_ui_theme_config(&text).expect("saved text parses");
        let (after, _) =
            crate::theme_merge::merge_ui_theme(&crate::theme::BoruTheme::default(), &reparsed);

        assert_eq!(before, after, "round trip preserves the active theme");
        // And the reparsed config itself equals the original (sparse, exact).
        assert_eq!(reparsed, cfg);
    }

    /// The serialized file contains ONLY present (Some) leaves, in stable
    /// struct field order — no `None` placeholders, no default materialization.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn save_serialization_omits_none_and_keeps_stable_order() {
        // A sparse config: one leaf in colors, one in sidebar, one nested.
        let cfg = parse_ui_theme_config(
            r#"
[colors]
primary = [0.1, 0.2, 0.3]

[sidebar]
width = 270.0

[attachments.video]
play_overlay_size = 64.0
"#,
        )
        .expect("sparse config parses");
        let text = ui_theme_config_to_toml(&cfg).expect("serializes");

        // Present keys appear...
        assert!(text.contains("[colors]"));
        assert!(
            text.contains("primary = [0.1, 0.2, 0.3]"),
            "color array form"
        );
        assert!(text.contains("[sidebar]"));
        assert!(text.contains("width = 270.0"));
        assert!(text.contains("[attachments.video]"));
        assert!(text.contains("play_overlay_size = 64.0"));
        // ...absent keys never appear (no `None` serialization).
        assert!(!text.contains("canvas"), "unset leaf not serialized");
        assert!(!text.contains("item_radius"), "unset leaf not serialized");
        assert!(
            !text.contains("narrow_breakpoint"),
            "unset leaf not serialized"
        );

        // Stable group order follows UiThemeConfig declaration order.
        let colors = text.find("[colors]").expect("colors group");
        let sidebar = text.find("[sidebar]").expect("sidebar group");
        let attachments = text.find("[attachments.video]").expect("attachments group");
        assert!(
            colors < sidebar && sidebar < attachments,
            "group order stable"
        );
    }

    /// Empty config serializes to the header comment only, which parses
    /// back to defaults — saving a reset theme is safe.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn save_empty_config_is_header_only_and_parses_to_defaults() {
        let text = ui_theme_config_to_toml(&UiThemeConfig::default()).expect("serializes");
        assert!(text.starts_with('#'), "file starts with header comment");
        let cfg = parse_ui_theme_config(&text).expect("header-only file parses");
        assert_eq!(cfg, UiThemeConfig::default());
    }

    /// Save writes the file atomically: target present, tmp sibling gone,
    /// content parses to the same config.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn save_ui_theme_config_writes_file_atomically() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cfg = parse_ui_theme_config(FULL_TOML).expect("full config parses");

        let path = save_ui_theme_config(dir.path(), &cfg).expect("save succeeds");
        assert!(path.ends_with(UI_CONFIG_FILE_NAME));
        assert!(path.exists(), "target file present");

        // No temp sibling left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temp files remain: {leftovers:?}");

        // Reload path parses the saved file to the same config.
        let reloaded = load_ui_theme_config(dir.path()).expect("reloads");
        assert_eq!(
            reloaded, cfg,
            "saved file round-trips through the load path"
        );
    }

    /// Colors serialize losslessly: [r,g,b(,a)] floats → toml → parse gives
    /// the exact same f32 values (hex output would quantize to 8-bit).
    /// Serialized inside a real config group — TOML has no bare top-level
    /// array, so the value is exercised through its struct field.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn save_color_value_serializes_losslessly() {
        let cv = ColorValue {
            r: 24.0 / 255.0,
            g: 127.0 / 255.0,
            b: 80.0 / 255.0,
            a: 0.5,
        };
        let cfg = crate::theme_config::ColorConfig {
            canvas: Some(cv),
            ..Default::default()
        };
        let text = toml::to_string(&cfg).expect("color serializes");
        assert!(text.contains("canvas = [0.09411765, 0.49803922, 0.3137255, 0.5]"));
        let reparsed: crate::theme_config::ColorConfig =
            toml::from_str(&text).expect("color reparses");
        assert_eq!(reparsed.canvas, Some(cv), "exact float round trip");
        assert!(!text.contains('#'), "serialized as array, not hex");
    }

    /// Alpha = 1.0 is omitted for compact files and defaults back on parse,
    /// keeping the round trip exact.
    #[cfg(feature = "dev-ui")]
    #[test]
    fn save_color_value_omits_default_alpha() {
        let cfg = crate::theme_config::ColorConfig {
            canvas: Some(ColorValue {
                r: 0.1,
                g: 0.2,
                b: 0.3,
                a: 1.0,
            }),
            ..Default::default()
        };
        let text = toml::to_string(&cfg).expect("color serializes");
        assert!(
            text.contains("canvas = [0.1, 0.2, 0.3]"),
            "alpha 1.0 omitted: {text}"
        );
        let reparsed: crate::theme_config::ColorConfig =
            toml::from_str(&text).expect("color reparses");
        assert_eq!(
            reparsed.canvas.expect("canvas"),
            ColorValue {
                r: 0.1,
                g: 0.2,
                b: 0.3,
                a: 1.0
            },
            "default alpha restored on parse"
        );
    }
}
