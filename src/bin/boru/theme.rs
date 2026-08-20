//! BoruTheme — typed visual theme model for the Boru desktop UI.
//!
//! This module introduces the central, typed structure for Boru's visual
//! properties (BORU-UI-02 / PDF Task 2 of the Live UI Editor chain). It
//! mirrors the values that today live in `design_tokens.rs`, `fonts.rs`,
//! `icon_system.rs`, `card_shell.rs` and the remaining raw literals mapped by
//! `docs/live-ui-editor/constants-audit.md`, in a Copy/Clone, semantic,
//! nested form that view/style code can consume.
//!
//! ## Design rules
//!
//! - **Semantic names over screen-coordinate names.** Token groups are named
//!   by role (`colors.text_primary`, `spacing.space_8`, `chat.bubble_max_width`)
//!   rather than by pixel position.
//! - **Mode-aware colours, mode-independent geometry.** Only `ColorTokens`
//!   differs between light and dark mode; spacing, radii, typography sizes,
//!   icons, avatars and the per-component geometry groups are shared. Use
//!   [`BoruTheme::for_theme`] to pick the mode for the active Iced theme.
//! - **`Default` = the current light-mode UI.** The light values are the
//!   byte-for-byte baseline; `ColorTokens::dark()` mirrors the dark palette
//!   from `design_tokens`. The test module asserts every value against the
//!   existing token modules so the two sources can never drift apart.
//! - **Copy/Clone everywhere** so view code can pass tokens by value without
//!   borrow gymnastics.
//!
//! ## Migration contract (BORU-UI-03+)
//!
//! View functions should obtain the active theme once per frame:
//!
//! ```rust,ignore
//! let theme = self.boru_theme();          // IcedChat accessor in app.rs
//! container(...).style(move |t| container::Style {
//!     background: Some(Background::Color(theme.colors.surface(t))),
//!     ...
//! })
//! ```
//!
//! For theme-independent helpers (spacing, radii, geometry) the value is a
//! plain `f32`/`Color` field; for mode-aware colours `ColorTokens` exposes a
//! `fn surface(&self) -> Color`-style accessor per token so call sites read
//! the right mode. Raw literals in view code should be replaced by these
//! fields — never by new literals.
//!
//! Later tasks (BORU-UI-04/05) layer `boru-ui.toml` overrides on top of
//! `BoruTheme::default()` without changing the model itself.

use iced::Color;

// ── Colour tokens ─────────────────────────────────────────────────────
//
// One `ColorTokens` value = one mode (light or dark). `Default` is the
// light palette — the current UI baseline. All values below match the
// `design_tokens.rs` constants / accessors exactly (verified by tests).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTokens {
    // ── Backgrounds ──
    /// Main canvas / panel background. Light #F7F9F8; dark rgb(0.10,0.10,0.18).
    pub canvas: Color,
    /// Sidebar background. Light #FCFDFC; dark rgb(0.16,0.16,0.24).
    pub sidebar: Color,
    /// Card / dialog / white panel surface. Light #FFFFFF; dark rgb(0.16,0.16,0.24).
    pub surface: Color,
    /// Elevated surface — dialogs, popovers, dropdowns that float above
    /// `surface`. Light #FFFFFF; dark rgb(0.16,0.16,0.24) (same as `surface`
    /// today; the token exists so elevated panels can be tuned independently).
    pub surface_elevated: Color,
    /// Selected row / selection background. Light #ECEFED; dark rgb(0.16,0.23,0.34).
    pub surface_selected: Color,
    /// Hover background. Light #EFF3F1; dark rgb(0.20,0.20,0.30).
    pub surface_hover: Color,
    /// Pressed background. Light #E4E8E5; dark rgb(0.18,0.18,0.26).
    pub surface_pressed: Color,
    /// Secondary surface (tabs, secondary panels). Light #EEF1EE; dark rgb(0.13,0.13,0.22).
    pub surface_secondary: Color,
    /// Input field background. Light #F0F0F4; dark rgb(0.13,0.13,0.22).
    pub input_bg: Color,
    // ── Borders ──
    /// Standard subtle border. Light #E8F0EB; dark rgb(0.22,0.22,0.32).
    pub border_muted: Color,
    /// Stronger border for emphasis. Light #C8D7CE; dark rgb(0.28,0.28,0.38).
    pub border_strong: Color,
    // ── Text ──
    /// Primary body text. Light #17211B; dark rgb(0.80,0.80,0.80).
    pub text_primary: Color,
    /// Secondary / supporting text. Light #5F6F66; dark rgb(0.60,0.60,0.60).
    pub text_secondary: Color,
    /// Muted / tertiary text. Light #626E68; dark rgb(0.60,0.60,0.60).
    pub text_muted: Color,
    /// Local (self) message label. Light rgb(0.0,0.45,0.0); dark rgb(0.2,0.8,0.2).
    pub text_local_label: Color,
    /// Local (self) message body. Light rgb(0.0,0.35,0.0); dark rgb(0.3,0.9,0.3).
    pub text_local_body: Color,
    /// Remote message label (nickname). Light rgb(0.0,0.33,0.66); dark rgb(0.4,0.65,1.0).
    pub text_remote_label: Color,
    /// Remote message body. Light = text_primary; dark rgb(0.8,0.8,0.8).
    pub text_remote_body: Color,
    // ── Accents ──
    /// Primary brand accent. Light reference #8EC07C; dark rgb(0.29,0.62,1.0).
    pub primary: Color,
    /// Primary hover state. Light #80AD70; dark rgb(0.36,0.70,1.0).
    pub primary_hover: Color,
    /// Primary pressed state. Light #729A63; dark rgb(0.24,0.52,0.86).
    pub primary_pressed: Color,
    /// Primary soft background tint. Light #EAF5E8; dark rgba(0.15,0.30,0.15,0.40).
    pub primary_soft: Color,
    /// Success / online green. Light #1A7F48; dark rgb(0.24,0.86,0.52).
    pub success: Color,
    /// Destructive / error red. Light #C84E4E; dark rgb(0.90,0.25,0.25).
    pub danger: Color,
    /// Warning / amber. Light #704505; dark rgb(0.95,0.65,0.15).
    pub warning: Color,
    /// Keyboard focus ring. Light #2B9B67; dark rgb(0.40,0.70,0.40).
    pub focus: Color,
    // ── Soft status tints ──
    /// Alpha applied to `danger`/`success`/`warning` for soft backgrounds
    /// (8 % light, 12 % dark — mirrors `destructive_soft` etc.).
    pub soft_tint_alpha: f32,
    // ── Overlays / media ──
    /// Modal dialog backdrop. Light rgba(0,0,0,0.35); dark rgba(0,0,0,0.55).
    pub dialog_backdrop: Color,
    /// Incoming-call dialog backdrop (currently a heavier 0.72 alpha).
    pub incoming_call_backdrop: Color,
    /// Chat-screen overlay backdrop — chat options & member-list panels
    /// (light rgba(0,0,0,0.25), dark rgba(0,0,0,0.45); chat.rs:137/257).
    pub chat_overlay_backdrop: Color,
    /// Chat search-panel backdrop (light rgba(0,0,0,0.15), dark
    /// rgba(0,0,0,0.35); chat.rs:169).
    pub chat_search_backdrop: Color,
    /// Elevated-panel drop shadow colour (rgba(0,0,0,0.30), both modes;
    /// chat.rs:226/1635/1817/2015).
    pub panel_shadow: Color,
    /// Incoming-call dialog panel background rgb(0.12,0.13,0.17).
    pub dialog_panel_bg: Color,
    /// Incoming-call dialog panel border rgb(0.35,0.38,0.45).
    pub dialog_panel_border: Color,
    /// Video media frame background rgb(0.055,0.06,0.07) — both themes.
    pub media_frame_bg: Color,
    /// Video media frame border rgba(1,1,1,0.10).
    pub media_frame_border: Color,
    /// Video play / loading overlay rgba(0,0,0,0.62).
    pub media_frame_overlay: Color,
    /// On-media placeholder / error text rgb(0.78,0.80,0.82).
    pub on_media_text: Color,
    // ── Muted glyphs (mode-independent raw literals today) ──
    /// Disabled / ghost-button glyph rgb(0.5,0.5,0.5).
    pub glyph_disabled: Color,
    /// Muted text-button glyph rgb(0.45,0.45,0.45).
    pub glyph_muted: Color,
    /// Dark-mode muted glyph / avatar fallback rgb(0.6,0.6,0.6).
    pub glyph_muted_dark: Color,
    /// Avatar fallback colour rgb(0.6,0.6,0.6) (contacts.rs).
    pub avatar_fallback: Color,
    /// Discover tag / status text grey rgb(0.4,0.4,0.4).
    pub tag_text: Color,
    /// Discover tag hover surface rgba(0.3,0.3,0.3,0.06).
    pub tag_bg: Color,
    /// Discover tag pressed surface rgba(0.3,0.3,0.3,0.12).
    pub tag_bg_pressed: Color,
    // ── Download / request state glyphs (audit §3.12 / §3.4) ──
    /// Completed download green rgb(0.2,0.7,0.2).
    pub download_completed: Color,
    /// Temporary failure amber rgb(0.78,0.58,0.16).
    pub download_temporary: Color,
    /// Terminal failure red rgb(0.8,0.22,0.22).
    pub download_terminal: Color,
    /// Cancelled download grey rgb(0.55,0.55,0.55).
    pub download_cancelled: Color,
    /// Pending friend request amber rgb(0.7,0.6,0.0).
    pub request_pending: Color,
    /// Accepted request green rgb(0.2,0.7,0.2).
    pub request_accepted: Color,
    /// Declined request red rgb(0.8,0.2,0.2).
    pub request_declined: Color,
    // ── Settings status colours (audit §3.12) ──
    /// Settings success green rgb(0.15,0.55,0.2).
    pub settings_success: Color,
    /// Settings danger text — light rgb(0.8,0.2,0.2), dark rgb(0.9,0.3,0.3).
    pub settings_danger: Color,
    /// Settings strong danger (Remove button) — light rgb(0.9,0.3,0.3), dark rgb(0.6,0.15,0.15).
    pub settings_danger_strong: Color,
    /// Settings row-title text (GIF privacy heading) — light rgb(0.15,0.15,0.15), dark rgb(0.9,0.9,0.9).
    pub settings_heading_text: Color,
    /// Expanded-video overlay backdrop rgba(0,0,0,0.82) — both modes.
    pub expanded_video_backdrop: Color,
    /// Image lightbox backdrop rgba(0,0,0,0.90) — both modes.
    pub lightbox_backdrop: Color,
    // ── Status card (dark privacy panel — theme-independent) ──
    /// Status card background — top/left gradient stop #10201C.
    pub status_card_bg_top: Color,
    /// Status card background — middle gradient stop #091714.
    pub status_card_bg_mid: Color,
    /// Status card background — bottom/right gradient stop #06100E.
    pub status_card_bg_bottom: Color,
    /// Status card border rgba(0x4D,0xE5,0xA3,0.22).
    pub status_card_border: Color,
    /// Connected accent green #4DE5A3.
    pub status_connected: Color,
    /// Status card primary text #F3F7F5.
    pub status_primary_text: Color,
    /// Status card secondary text #9FB3AA.
    pub status_secondary_text: Color,
    /// Status card network mesh line #4DE5A3.
    pub status_network_line: Color,
    /// Status card network mesh node #4DE5A3.
    pub status_network_node: Color,
    /// Status card warning amber #E8A33D.
    pub status_warning: Color,
    /// Status card danger red #E55B5B.
    pub status_danger: Color,
}

impl ColorTokens {
    /// Light-mode palette — the current UI baseline.
    pub const fn light() -> Self {
        Self {
            canvas: Color::from_rgb(
                0xF7 as f32 / 255.0,
                0xF9 as f32 / 255.0,
                0xF8 as f32 / 255.0,
            ),
            sidebar: Color::from_rgb(
                0xFC as f32 / 255.0,
                0xFD as f32 / 255.0,
                0xFC as f32 / 255.0,
            ),
            surface: Color::WHITE,
            surface_elevated: Color::WHITE,
            surface_selected: Color::from_rgb(
                0xEC as f32 / 255.0,
                0xEF as f32 / 255.0,
                0xED as f32 / 255.0,
            ),
            surface_hover: Color::from_rgb(
                0xEF as f32 / 255.0,
                0xF3 as f32 / 255.0,
                0xF1 as f32 / 255.0,
            ),
            surface_pressed: Color::from_rgb(
                0xE4 as f32 / 255.0,
                0xE8 as f32 / 255.0,
                0xE5 as f32 / 255.0,
            ),
            surface_secondary: Color::from_rgb(
                0xEE as f32 / 255.0,
                0xF1 as f32 / 255.0,
                0xEE as f32 / 255.0,
            ),
            input_bg: Color::from_rgb(
                0xf0 as f32 / 255.0,
                0xf0 as f32 / 255.0,
                0xf4 as f32 / 255.0,
            ),
            border_muted: Color::from_rgb(
                0xE8 as f32 / 255.0,
                0xF0 as f32 / 255.0,
                0xEB as f32 / 255.0,
            ),
            border_strong: Color::from_rgb(
                0xC8 as f32 / 255.0,
                0xD7 as f32 / 255.0,
                0xCE as f32 / 255.0,
            ),
            text_primary: Color::from_rgb(
                0x17 as f32 / 255.0,
                0x21 as f32 / 255.0,
                0x1B as f32 / 255.0,
            ),
            text_secondary: Color::from_rgb(
                0x5F as f32 / 255.0,
                0x6F as f32 / 255.0,
                0x66 as f32 / 255.0,
            ),
            text_muted: Color::from_rgb(
                0x62 as f32 / 255.0,
                0x6E as f32 / 255.0,
                0x68 as f32 / 255.0,
            ),
            text_local_label: Color::from_rgb(0.0, 0.45, 0.0),
            text_local_body: Color::from_rgb(0.0, 0.35, 0.0),
            text_remote_label: Color::from_rgb(0.0, 0.33, 0.66),
            text_remote_body: Color::from_rgb(
                0x17 as f32 / 255.0,
                0x21 as f32 / 255.0,
                0x1B as f32 / 255.0,
            ),
            primary: Color::from_rgb(
                0x18 as f32 / 255.0,
                0x7F as f32 / 255.0,
                0x50 as f32 / 255.0,
            ),
            primary_hover: Color::from_rgb(
                0x80 as f32 / 255.0,
                0xAD as f32 / 255.0,
                0x70 as f32 / 255.0,
            ),
            primary_pressed: Color::from_rgb(
                0x10 as f32 / 255.0,
                0x5F as f32 / 255.0,
                0x38 as f32 / 255.0,
            ),
            primary_soft: Color::from_rgb(
                0xEA as f32 / 255.0,
                0xF5 as f32 / 255.0,
                0xE8 as f32 / 255.0,
            ),
            success: Color::from_rgb(
                0x1A as f32 / 255.0,
                0x7F as f32 / 255.0,
                0x48 as f32 / 255.0,
            ),
            danger: Color::from_rgb(
                0xC8 as f32 / 255.0,
                0x4E as f32 / 255.0,
                0x4E as f32 / 255.0,
            ),
            warning: Color::from_rgb(
                0x70 as f32 / 255.0,
                0x45 as f32 / 255.0,
                0x05 as f32 / 255.0,
            ),
            focus: Color::from_rgb(
                0x2B as f32 / 255.0,
                0x9B as f32 / 255.0,
                0x67 as f32 / 255.0,
            ),
            soft_tint_alpha: 0.08,
            dialog_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            incoming_call_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.72),
            chat_overlay_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.25),
            chat_search_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.15),
            panel_shadow: Color::from_rgba(0.0, 0.0, 0.0, 0.30),
            dialog_panel_bg: Color::from_rgb(0.12, 0.13, 0.17),
            dialog_panel_border: Color::from_rgb(0.35, 0.38, 0.45),
            media_frame_bg: Color::from_rgb(0.055, 0.06, 0.07),
            media_frame_border: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
            media_frame_overlay: Color::from_rgba(0.0, 0.0, 0.0, 0.62),
            on_media_text: Color::from_rgb(0.78, 0.80, 0.82),
            glyph_disabled: Color::from_rgb(0.5, 0.5, 0.5),
            glyph_muted: Color::from_rgb(0.45, 0.45, 0.45),
            glyph_muted_dark: Color::from_rgb(0.6, 0.6, 0.6),
            avatar_fallback: Color::from_rgb(0.6, 0.6, 0.6),
            tag_text: Color::from_rgb(0.4, 0.4, 0.4),
            tag_bg: Color::from_rgba(0.3, 0.3, 0.3, 0.06),
            tag_bg_pressed: Color::from_rgba(0.3, 0.3, 0.3, 0.12),
            download_completed: Color::from_rgb(0.2, 0.7, 0.2),
            download_temporary: Color::from_rgb(0.78, 0.58, 0.16),
            download_terminal: Color::from_rgb(0.8, 0.22, 0.22),
            download_cancelled: Color::from_rgb(0.55, 0.55, 0.55),
            request_pending: Color::from_rgb(0.7, 0.6, 0.0),
            request_accepted: Color::from_rgb(0.2, 0.7, 0.2),
            request_declined: Color::from_rgb(0.8, 0.2, 0.2),
            settings_success: Color::from_rgb(0.15, 0.55, 0.2),
            settings_danger: Color::from_rgb(0.8, 0.2, 0.2),
            settings_danger_strong: Color::from_rgb(0.9, 0.3, 0.3),
            settings_heading_text: Color::from_rgb(0.15, 0.15, 0.15),
            expanded_video_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.82),
            lightbox_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.90),
            status_card_bg_top: Color::from_rgb(
                0x10 as f32 / 255.0,
                0x20 as f32 / 255.0,
                0x1C as f32 / 255.0,
            ),
            status_card_bg_mid: Color::from_rgb(
                0x09 as f32 / 255.0,
                0x17 as f32 / 255.0,
                0x14 as f32 / 255.0,
            ),
            status_card_bg_bottom: Color::from_rgb(
                0x06 as f32 / 255.0,
                0x10 as f32 / 255.0,
                0x0E as f32 / 255.0,
            ),
            status_card_border: Color::from_rgba(
                0x4D as f32 / 255.0,
                0xE5 as f32 / 255.0,
                0xA3 as f32 / 255.0,
                0.22,
            ),
            status_connected: Color::from_rgb(
                0x4D as f32 / 255.0,
                0xE5 as f32 / 255.0,
                0xA3 as f32 / 255.0,
            ),
            status_primary_text: Color::from_rgb(
                0xF3 as f32 / 255.0,
                0xF7 as f32 / 255.0,
                0xF5 as f32 / 255.0,
            ),
            status_secondary_text: Color::from_rgb(
                0x9F as f32 / 255.0,
                0xB3 as f32 / 255.0,
                0xAA as f32 / 255.0,
            ),
            status_network_line: Color::from_rgb(
                0x4D as f32 / 255.0,
                0xE5 as f32 / 255.0,
                0xA3 as f32 / 255.0,
            ),
            status_network_node: Color::from_rgb(
                0x4D as f32 / 255.0,
                0xE5 as f32 / 255.0,
                0xA3 as f32 / 255.0,
            ),
            status_warning: Color::from_rgb(
                0xE8 as f32 / 255.0,
                0xA3 as f32 / 255.0,
                0x3D as f32 / 255.0,
            ),
            status_danger: Color::from_rgb(
                0xE5 as f32 / 255.0,
                0x5B as f32 / 255.0,
                0x5B as f32 / 255.0,
            ),
        }
    }

    /// Dark-mode palette — mirrors the dark branches of the `design_tokens`
    /// accessors.
    pub const fn dark() -> Self {
        Self {
            canvas: Color::from_rgb(0.10, 0.10, 0.18),
            sidebar: Color::from_rgb(0.16, 0.16, 0.24),
            surface: Color::from_rgb(0.16, 0.16, 0.24),
            surface_elevated: Color::from_rgb(0.16, 0.16, 0.24),
            surface_selected: Color::from_rgb(0.16, 0.23, 0.34),
            surface_hover: Color::from_rgb(0.20, 0.20, 0.30),
            surface_pressed: Color::from_rgb(0.18, 0.18, 0.26),
            surface_secondary: Color::from_rgb(0.13, 0.13, 0.22),
            input_bg: Color::from_rgb(0.13, 0.13, 0.22),
            border_muted: Color::from_rgb(0.22, 0.22, 0.32),
            border_strong: Color::from_rgb(0.28, 0.28, 0.38),
            text_primary: Color::from_rgb(0.80, 0.80, 0.80),
            text_secondary: Color::from_rgb(0.60, 0.60, 0.60),
            text_muted: Color::from_rgb(0.60, 0.60, 0.60),
            text_local_label: Color::from_rgb(0.2, 0.8, 0.2),
            text_local_body: Color::from_rgb(0.3, 0.9, 0.3),
            text_remote_label: Color::from_rgb(0.4, 0.65, 1.0),
            text_remote_body: Color::from_rgb(0.8, 0.8, 0.8),
            primary: Color::from_rgb(0.29, 0.62, 1.0),
            primary_hover: Color::from_rgb(0.36, 0.70, 1.0),
            primary_pressed: Color::from_rgb(0.24, 0.52, 0.86),
            primary_soft: Color::from_rgba(0.15, 0.30, 0.15, 0.40),
            success: Color::from_rgb(0.24, 0.86, 0.52),
            danger: Color::from_rgb(0.90, 0.25, 0.25),
            warning: Color::from_rgb(0.95, 0.65, 0.15),
            focus: Color::from_rgb(0.40, 0.70, 0.40),
            soft_tint_alpha: 0.12,
            dialog_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.55),
            // Theme-independent surfaces stay the same in dark mode.
            incoming_call_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.72),
            chat_overlay_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            chat_search_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            panel_shadow: Color::from_rgba(0.0, 0.0, 0.0, 0.30),
            dialog_panel_bg: Color::from_rgb(0.12, 0.13, 0.17),
            dialog_panel_border: Color::from_rgb(0.35, 0.38, 0.45),
            media_frame_bg: Color::from_rgb(0.055, 0.06, 0.07),
            media_frame_border: Color::from_rgba(1.0, 1.0, 1.0, 0.10),
            media_frame_overlay: Color::from_rgba(0.0, 0.0, 0.0, 0.62),
            on_media_text: Color::from_rgb(0.78, 0.80, 0.82),
            glyph_disabled: Color::from_rgb(0.5, 0.5, 0.5),
            glyph_muted: Color::from_rgb(0.45, 0.45, 0.45),
            glyph_muted_dark: Color::from_rgb(0.6, 0.6, 0.6),
            avatar_fallback: Color::from_rgb(0.6, 0.6, 0.6),
            tag_text: Color::from_rgb(0.4, 0.4, 0.4),
            tag_bg: Color::from_rgba(0.3, 0.3, 0.3, 0.06),
            tag_bg_pressed: Color::from_rgba(0.3, 0.3, 0.3, 0.12),
            download_completed: Color::from_rgb(0.2, 0.7, 0.2),
            download_temporary: Color::from_rgb(0.78, 0.58, 0.16),
            download_terminal: Color::from_rgb(0.8, 0.22, 0.22),
            download_cancelled: Color::from_rgb(0.55, 0.55, 0.55),
            request_pending: Color::from_rgb(0.7, 0.6, 0.0),
            request_accepted: Color::from_rgb(0.2, 0.7, 0.2),
            request_declined: Color::from_rgb(0.8, 0.2, 0.2),
            settings_success: Color::from_rgb(0.15, 0.55, 0.2),
            settings_danger: Color::from_rgb(0.9, 0.3, 0.3),
            settings_danger_strong: Color::from_rgb(0.6, 0.15, 0.15),
            settings_heading_text: Color::from_rgb(0.9, 0.9, 0.9),
            expanded_video_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.82),
            lightbox_backdrop: Color::from_rgba(0.0, 0.0, 0.0, 0.90),
            status_card_bg_top: Color::from_rgb(
                0x10 as f32 / 255.0,
                0x20 as f32 / 255.0,
                0x1C as f32 / 255.0,
            ),
            status_card_bg_mid: Color::from_rgb(
                0x09 as f32 / 255.0,
                0x17 as f32 / 255.0,
                0x14 as f32 / 255.0,
            ),
            status_card_bg_bottom: Color::from_rgb(
                0x06 as f32 / 255.0,
                0x10 as f32 / 255.0,
                0x0E as f32 / 255.0,
            ),
            status_card_border: Color::from_rgba(
                0x4D as f32 / 255.0,
                0xE5 as f32 / 255.0,
                0xA3 as f32 / 255.0,
                0.22,
            ),
            status_connected: Color::from_rgb(
                0x4D as f32 / 255.0,
                0xE5 as f32 / 255.0,
                0xA3 as f32 / 255.0,
            ),
            status_primary_text: Color::from_rgb(
                0xF3 as f32 / 255.0,
                0xF7 as f32 / 255.0,
                0xF5 as f32 / 255.0,
            ),
            status_secondary_text: Color::from_rgb(
                0x9F as f32 / 255.0,
                0xB3 as f32 / 255.0,
                0xAA as f32 / 255.0,
            ),
            status_network_line: Color::from_rgb(
                0x4D as f32 / 255.0,
                0xE5 as f32 / 255.0,
                0xA3 as f32 / 255.0,
            ),
            status_network_node: Color::from_rgb(
                0x4D as f32 / 255.0,
                0xE5 as f32 / 255.0,
                0xA3 as f32 / 255.0,
            ),
            status_warning: Color::from_rgb(
                0xE8 as f32 / 255.0,
                0xA3 as f32 / 255.0,
                0x3D as f32 / 255.0,
            ),
            status_danger: Color::from_rgb(
                0xE5 as f32 / 255.0,
                0x5B as f32 / 255.0,
                0x5B as f32 / 255.0,
            ),
        }
    }

    /// Soft destructive background — `danger` at `soft_tint_alpha`.
    /// Mirrors `design_tokens::destructive_soft`.
    #[cfg_attr(not(test), allow(dead_code))] // exercised by tests; consumed by BORU-UI-03+
    pub fn destructive_soft(&self) -> Color {
        Color::from_rgba(
            self.danger.r,
            self.danger.g,
            self.danger.b,
            self.soft_tint_alpha,
        )
    }

    /// Soft success background — `success` at `soft_tint_alpha`.
    /// Mirrors `design_tokens::success_soft`.
    #[cfg_attr(not(test), allow(dead_code))] // exercised by tests; consumed by BORU-UI-03+
    pub fn success_soft(&self) -> Color {
        Color::from_rgba(
            self.success.r,
            self.success.g,
            self.success.b,
            self.soft_tint_alpha,
        )
    }

    /// Soft warning background — `warning` at `soft_tint_alpha`.
    /// Mirrors `design_tokens::warning_soft`.
    #[cfg_attr(not(test), allow(dead_code))] // exercised by tests; consumed by BORU-UI-03+
    pub fn warning_soft(&self) -> Color {
        Color::from_rgba(
            self.warning.r,
            self.warning.g,
            self.warning.b,
            self.soft_tint_alpha,
        )
    }
}

// ── Canonical semantic tokens (PDF T17) ──────────────────────────────
//
// The PDF colour system names the core tokens `background`, `surface`,
// `surface_elevated`, `surface_hover`, `text_primary`, `text_secondary`,
// `border`, `accent`, `accent_hover`, `success`, `warning`, `danger`.
// Most already exist as fields with the canonical name (`surface`,
// `surface_elevated`, `surface_hover`, `text_primary`, `text_secondary`,
// `success`, `warning`, `danger`). The four accessors below map the PDF
// names onto the older field names so every canonical token is reachable
// by its semantic name and components never invent raw literals:
//
// | PDF token       | backing field  |
// |-----------------|----------------|
// | `background`    | `canvas`       |
// | `border`        | `border_muted` |
// | `accent`        | `primary`      |
// | `accent_hover`  | `primary_hover`|
impl ColorTokens {
    /// Canonical semantic token: application background (PDF T17).
    /// Backed by `canvas`.
    pub fn background(&self) -> Color {
        self.canvas
    }

    /// Canonical semantic token: default border colour (PDF T17).
    /// Backed by `border_muted`.
    pub fn border(&self) -> Color {
        self.border_muted
    }

    /// Canonical semantic token: primary accent colour (PDF T17).
    /// Backed by `primary`.
    pub fn accent(&self) -> Color {
        self.primary
    }

    /// Canonical semantic token: accent hover state (PDF T17).
    /// Backed by `primary_hover`.
    pub fn accent_hover(&self) -> Color {
        self.primary_hover
    }
}

impl Default for ColorTokens {
    fn default() -> Self {
        Self::light()
    }
}

// ── Typography tokens ─────────────────────────────────────────────────
//
// Sizes mirror `fonts.rs` — `TypeRole::size_px()` for the 15 canonical
// roles plus the legacy `sizes` module (`HOME_SUBTITLE`, `DIALOG_TITLE`,
// `DIALOG_SUBTITLE`) and the audit-listed raw sizes (sidebar name, section
// label, badge, call text). BORU-UI-16 adds the *family choice* per role
// group (display/UI/chat/technical/brand — Boru bundles five families) and
// per-role weight + line-height mappings so typography can be live-edited
// through the inspector without reloading font files (the bundled files
// are loaded once at startup; the theme only changes which already-loaded
// family/weight a role resolves to).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypographyTokens {
    // Canonical TypeRole sizes (fonts.rs TypeRole::size_px()).
    pub display_heading: f32,
    pub page_title: f32,
    pub section_title: f32,
    pub card_title: f32,
    pub body: f32,
    pub body_emphasised: f32,
    pub button_label: f32,
    pub supporting_text: f32,
    pub metadata: f32,
    pub chat_message: f32,
    pub chat_sender: f32,
    pub chat_metadata: f32,
    pub composer_text: f32,
    pub technical_value: f32,
    pub brand_wordmark: f32,
    // Legacy named sizes (fonts::sizes).
    pub home_subtitle: f32,
    pub dialog_title: f32,
    pub dialog_subtitle: f32,
    // Audit §3.1 / §3.11 — raw sizes outside TypeRole.
    pub sidebar_name: f32,
    pub section_label: f32,
    pub badge: f32,
    // Audit §3.10 — call screen text.
    pub call_name: f32,
    /// Active-call (in-progress) name size — 26 px, one step above the
    /// outgoing-call `call_name` (24 px). Kept separate so the two call
    /// screens keep their distinct sizes.
    pub call_name_active: f32,
    pub call_remote_name: f32,
    pub call_status: f32,
    pub call_duration: f32,
    pub call_avatar_glyph: f32,
    pub call_avatar_glyph_large: f32,
    pub call_pip_label: f32,
    // ── BORU-UI-16: family choice per role group ──
    /// Family for display/heading roles (default Inter Tight).
    pub display_family: crate::fonts::FontFamilyKey,
    /// Family for UI/body roles (default Public Sans).
    pub ui_family: crate::fonts::FontFamilyKey,
    /// Family for chat/message roles (default Figtree).
    pub chat_family: crate::fonts::FontFamilyKey,
    /// Family for technical values (default JetBrains Mono).
    pub technical_family: crate::fonts::FontFamilyKey,
    /// Family for the brand wordmark (default Raleway).
    pub brand_family: crate::fonts::FontFamilyKey,
    // ── BORU-UI-16: weight mapping per canonical role ──
    pub display_heading_weight: crate::fonts::FontWeightKey,
    pub page_title_weight: crate::fonts::FontWeightKey,
    pub section_title_weight: crate::fonts::FontWeightKey,
    pub card_title_weight: crate::fonts::FontWeightKey,
    pub body_weight: crate::fonts::FontWeightKey,
    pub body_emphasised_weight: crate::fonts::FontWeightKey,
    pub button_label_weight: crate::fonts::FontWeightKey,
    pub supporting_text_weight: crate::fonts::FontWeightKey,
    pub metadata_weight: crate::fonts::FontWeightKey,
    pub chat_message_weight: crate::fonts::FontWeightKey,
    pub chat_sender_weight: crate::fonts::FontWeightKey,
    pub chat_metadata_weight: crate::fonts::FontWeightKey,
    pub composer_text_weight: crate::fonts::FontWeightKey,
    pub technical_value_weight: crate::fonts::FontWeightKey,
    pub brand_wordmark_weight: crate::fonts::FontWeightKey,
    // ── BORU-UI-16: line-height mapping per canonical role ──
    pub display_heading_line_height: f32,
    pub page_title_line_height: f32,
    pub section_title_line_height: f32,
    pub card_title_line_height: f32,
    pub body_line_height: f32,
    pub body_emphasised_line_height: f32,
    pub button_label_line_height: f32,
    pub supporting_text_line_height: f32,
    pub metadata_line_height: f32,
    pub chat_message_line_height: f32,
    pub chat_sender_line_height: f32,
    pub chat_metadata_line_height: f32,
    pub composer_text_line_height: f32,
    pub technical_value_line_height: f32,
    pub brand_wordmark_line_height: f32,
}

impl Default for TypographyTokens {
    fn default() -> Self {
        use crate::fonts::{FontFamilyKey, FontWeightKey, TypeRole};
        Self {
            display_heading: 32.0,
            page_title: 28.0,
            section_title: 20.0,
            card_title: 18.0,
            body: 15.0,
            body_emphasised: 15.0,
            button_label: 14.0,
            supporting_text: 13.0,
            metadata: 12.0,
            chat_message: 15.0,
            chat_sender: 14.0,
            chat_metadata: 12.0,
            composer_text: 15.0,
            technical_value: 12.0,
            brand_wordmark: 28.0,
            home_subtitle: 16.0,
            dialog_title: 26.0,
            dialog_subtitle: 14.0,
            sidebar_name: 15.0,
            section_label: 11.0,
            badge: 10.0,
            call_name: 24.0,
            call_name_active: 26.0,
            call_remote_name: 18.0,
            call_status: 16.0,
            call_duration: 22.0,
            call_avatar_glyph: 36.0,
            call_avatar_glyph_large: 44.0,
            call_pip_label: 18.0,
            display_family: FontFamilyKey::InterTight,
            ui_family: FontFamilyKey::PublicSans,
            chat_family: FontFamilyKey::Figtree,
            technical_family: FontFamilyKey::JetBrainsMono,
            brand_family: FontFamilyKey::Raleway,
            display_heading_weight: TypeRole::DisplayHeading.weight_key(),
            page_title_weight: TypeRole::PageTitle.weight_key(),
            section_title_weight: TypeRole::SectionTitle.weight_key(),
            card_title_weight: TypeRole::CardTitle.weight_key(),
            body_weight: TypeRole::Body.weight_key(),
            body_emphasised_weight: TypeRole::BodyEmphasised.weight_key(),
            button_label_weight: TypeRole::ButtonLabel.weight_key(),
            supporting_text_weight: TypeRole::SupportingText.weight_key(),
            metadata_weight: TypeRole::Metadata.weight_key(),
            chat_message_weight: TypeRole::ChatMessage.weight_key(),
            chat_sender_weight: TypeRole::ChatSender.weight_key(),
            chat_metadata_weight: TypeRole::ChatMetadata.weight_key(),
            composer_text_weight: TypeRole::ComposerText.weight_key(),
            technical_value_weight: TypeRole::TechnicalValue.weight_key(),
            brand_wordmark_weight: TypeRole::BrandWordmark.weight_key(),
            display_heading_line_height: TypeRole::DisplayHeading.default_line_height(),
            page_title_line_height: TypeRole::PageTitle.default_line_height(),
            section_title_line_height: TypeRole::SectionTitle.default_line_height(),
            card_title_line_height: TypeRole::CardTitle.default_line_height(),
            body_line_height: TypeRole::Body.default_line_height(),
            body_emphasised_line_height: TypeRole::BodyEmphasised.default_line_height(),
            button_label_line_height: TypeRole::ButtonLabel.default_line_height(),
            supporting_text_line_height: TypeRole::SupportingText.default_line_height(),
            metadata_line_height: TypeRole::Metadata.default_line_height(),
            chat_message_line_height: TypeRole::ChatMessage.default_line_height(),
            chat_sender_line_height: TypeRole::ChatSender.default_line_height(),
            chat_metadata_line_height: TypeRole::ChatMetadata.default_line_height(),
            composer_text_line_height: TypeRole::ComposerText.default_line_height(),
            technical_value_line_height: TypeRole::TechnicalValue.default_line_height(),
            brand_wordmark_line_height: TypeRole::BrandWordmark.default_line_height(),
        }
    }
}

impl TypographyTokens {
    /// The bundled family choice for a role (BORU-UI-16).
    ///
    /// Maps the role to its role group: display/heading roles → the display
    /// family, UI/body roles → the UI family, chat/message roles → the chat
    /// family, technical values → the technical family, wordmark → brand.
    pub fn family_for(&self, role: crate::fonts::TypeRole) -> crate::fonts::FontFamilyKey {
        use crate::fonts::TypeRole;
        match role {
            TypeRole::DisplayHeading | TypeRole::PageTitle => self.display_family,
            TypeRole::SectionTitle
            | TypeRole::CardTitle
            | TypeRole::Body
            | TypeRole::BodyEmphasised
            | TypeRole::ButtonLabel
            | TypeRole::SupportingText
            | TypeRole::Metadata => self.ui_family,
            TypeRole::ChatMessage
            | TypeRole::ChatSender
            | TypeRole::ChatMetadata
            | TypeRole::ComposerText => self.chat_family,
            TypeRole::TechnicalValue => self.technical_family,
            TypeRole::BrandWordmark => self.brand_family,
        }
    }

    /// The weight mapping for a role (BORU-UI-16).
    pub fn weight_for(&self, role: crate::fonts::TypeRole) -> crate::fonts::FontWeightKey {
        use crate::fonts::TypeRole;
        match role {
            TypeRole::DisplayHeading => self.display_heading_weight,
            TypeRole::PageTitle => self.page_title_weight,
            TypeRole::SectionTitle => self.section_title_weight,
            TypeRole::CardTitle => self.card_title_weight,
            TypeRole::Body => self.body_weight,
            TypeRole::BodyEmphasised => self.body_emphasised_weight,
            TypeRole::ButtonLabel => self.button_label_weight,
            TypeRole::SupportingText => self.supporting_text_weight,
            TypeRole::Metadata => self.metadata_weight,
            TypeRole::ChatMessage => self.chat_message_weight,
            TypeRole::ChatSender => self.chat_sender_weight,
            TypeRole::ChatMetadata => self.chat_metadata_weight,
            TypeRole::ComposerText => self.composer_text_weight,
            TypeRole::TechnicalValue => self.technical_value_weight,
            TypeRole::BrandWordmark => self.brand_wordmark_weight,
        }
    }

    /// The size mapping for a role (the canonical sizes).
    pub fn size_for(&self, role: crate::fonts::TypeRole) -> f32 {
        use crate::fonts::TypeRole;
        match role {
            TypeRole::DisplayHeading => self.display_heading,
            TypeRole::PageTitle => self.page_title,
            TypeRole::SectionTitle => self.section_title,
            TypeRole::CardTitle => self.card_title,
            TypeRole::Body => self.body,
            TypeRole::BodyEmphasised => self.body_emphasised,
            TypeRole::ButtonLabel => self.button_label,
            TypeRole::SupportingText => self.supporting_text,
            TypeRole::Metadata => self.metadata,
            TypeRole::ChatMessage => self.chat_message,
            TypeRole::ChatSender => self.chat_sender,
            TypeRole::ChatMetadata => self.chat_metadata,
            TypeRole::ComposerText => self.composer_text,
            TypeRole::TechnicalValue => self.technical_value,
            TypeRole::BrandWordmark => self.brand_wordmark,
        }
    }

    /// The line-height mapping for a role (relative, BORU-UI-16).
    pub fn line_height_for(&self, role: crate::fonts::TypeRole) -> f32 {
        use crate::fonts::TypeRole;
        match role {
            TypeRole::DisplayHeading => self.display_heading_line_height,
            TypeRole::PageTitle => self.page_title_line_height,
            TypeRole::SectionTitle => self.section_title_line_height,
            TypeRole::CardTitle => self.card_title_line_height,
            TypeRole::Body => self.body_line_height,
            TypeRole::BodyEmphasised => self.body_emphasised_line_height,
            TypeRole::ButtonLabel => self.button_label_line_height,
            TypeRole::SupportingText => self.supporting_text_line_height,
            TypeRole::Metadata => self.metadata_line_height,
            TypeRole::ChatMessage => self.chat_message_line_height,
            TypeRole::ChatSender => self.chat_sender_line_height,
            TypeRole::ChatMetadata => self.chat_metadata_line_height,
            TypeRole::ComposerText => self.composer_text_line_height,
            TypeRole::TechnicalValue => self.technical_value_line_height,
            TypeRole::BrandWordmark => self.brand_wordmark_line_height,
        }
    }
}

// ── Spacing tokens ────────────────────────────────────────────────────
//
// Mirrors `design_tokens.rs` SPACE_* scale (4 px base unit) plus the
// control heights.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpacingTokens {
    pub space_2: f32,
    pub space_4: f32,
    pub space_6: f32,
    pub space_8: f32,
    pub space_10: f32,
    pub space_12: f32,
    pub space_16: f32,
    pub space_18: f32,
    pub space_20: f32,
    pub space_24: f32,
    pub space_28: f32,
    pub space_32: f32,
    pub space_40: f32,
    pub control_height: f32,
    pub control_height_compact: f32,
}

impl Default for SpacingTokens {
    fn default() -> Self {
        Self {
            space_2: 2.0,
            space_4: 4.0,
            space_6: 6.0,
            space_8: 8.0,
            space_10: 10.0,
            space_12: 12.0,
            space_16: 16.0,
            space_18: 18.0,
            space_20: 20.0,
            space_24: 24.0,
            space_28: 28.0,
            space_32: 32.0,
            space_40: 40.0,
            control_height: 40.0,
            control_height_compact: 36.0,
        }
    }
}

// ── Corner radii ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadiusTokens {
    /// Flat / no radius (0 px).
    pub none: f32,
    /// Small controls (8 px).
    pub sm: f32,
    /// Buttons, list selections (10 px).
    pub md: f32,
    /// Chat bubbles, dialogs (12 px).
    pub lg: f32,
    /// Hero cards, composer (16 px).
    pub xl: f32,
    /// Card containers (16 px).
    pub card: f32,
    /// Sidebar hover pill / unread badge (10 px).
    pub pill: f32,
    /// Sidebar avatar container (12 px).
    pub avatar_container: f32,
    /// Call avatar circle (48 px — half of the 96 px call avatar).
    pub call_avatar: f32,
    /// Video media frame (13 px).
    pub media_frame: f32,
    /// Attachment thumbnail (10 px).
    pub attachment: f32,
    /// Dialog panel (16 px).
    pub dialog: f32,
    /// Picker cell / table chip (8 px).
    pub picker_cell: f32,
    /// Color-picker bar (4 px, settings).
    pub control_sm: f32,
    /// Status-card divider bar (1.5 px).
    pub status_divider: f32,
    /// Status-card security pill (14 px).
    pub security_pill: f32,
}

impl Default for RadiusTokens {
    fn default() -> Self {
        Self {
            none: 0.0,
            sm: 8.0,
            md: 10.0,
            lg: 12.0,
            xl: 16.0,
            card: 16.0,
            pill: 10.0,
            avatar_container: 12.0,
            call_avatar: 48.0,
            media_frame: 13.0,
            attachment: 10.0,
            dialog: 16.0,
            picker_cell: 8.0,
            control_sm: 4.0,
            status_divider: 1.5,
            security_pill: 14.0,
        }
    }
}

// ── Icon tokens ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconTokens {
    /// 16 px — metadata, status dots, inline hints.
    pub xs: f32,
    /// 18 px — inline actions, composer buttons.
    pub sm: f32,
    /// 20 px — sidebar, toolbar (default).
    pub md: f32,
    /// 24 px — quick actions, home cards.
    pub lg: f32,
    /// 28 px — hero / empty-state.
    pub xl: f32,
    /// 24 px utility/status icons in the sidebar footer.
    pub sidebar_utility: f32,
}

impl Default for IconTokens {
    fn default() -> Self {
        Self {
            xs: 16.0,
            sm: 18.0,
            md: 20.0,
            lg: 24.0,
            xl: 28.0,
            sidebar_utility: 24.0,
        }
    }
}

// ── Avatar tokens ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AvatarTokens {
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    /// Sidebar profile header avatar (72 px).
    pub profile: f32,
    /// Chat-list conversation row avatar (56 px).
    pub chat_list: f32,
    /// Chat conversation header avatar (52 px).
    pub chat_header: f32,
    /// Message bubble avatar (46 px).
    pub msg: f32,
    /// Status dot for normal-sized avatars (10 px).
    pub status_dot_sm: f32,
    /// Status dot for the large profile avatar (12 px).
    pub status_dot_lg: f32,
}

impl Default for AvatarTokens {
    fn default() -> Self {
        Self {
            sm: 36.0,
            md: 48.0,
            lg: 64.0,
            profile: 72.0,
            chat_list: 56.0,
            chat_header: 52.0,
            msg: 46.0,
            status_dot_sm: 10.0,
            status_dot_lg: 12.0,
        }
    }
}

// ── List / row tokens (card_shell + design_tokens table rows) ────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListTokens {
    pub card_row_height: f32,
    pub peer_row_height: f32,
    pub default_list_max_height: f32,
    pub table_row_height: f32,
    pub table_row_height_compact: f32,
    pub chip_height: f32,
    pub peer_panel_max_height: f32,
    pub progress_bar_height: f32,
    pub progress_bar_height_bold: f32,
}

impl Default for ListTokens {
    fn default() -> Self {
        Self {
            card_row_height: 48.0,
            peer_row_height: 60.0,
            default_list_max_height: 180.0,
            table_row_height: 56.0,
            table_row_height_compact: 48.0,
            chip_height: 28.0,
            peer_panel_max_height: 320.0,
            progress_bar_height: 4.0,
            progress_bar_height_bold: 6.0,
        }
    }
}

// ── Border widths ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderTokens {
    /// Standard 1 px hairline border / divider.
    pub hairline: f32,
    /// 2 px keyboard focus ring.
    pub focus: f32,
    /// 2 px active-tab underline (ui_components tab strip).
    pub tab_active: f32,
    /// 1 px selected sidebar row border.
    pub selected_row: f32,
    /// 1 px video media-frame border.
    pub media_frame: f32,
}

impl Default for BorderTokens {
    fn default() -> Self {
        Self {
            hairline: 1.0,
            focus: 2.0,
            tab_active: 2.0,
            selected_row: 1.0,
            media_frame: 1.0,
        }
    }
}

// ── Responsive / layout tokens ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResponsiveTokens {
    pub viewport_ref_width: f32,
    pub viewport_ref_height: f32,
    pub viewport_min_width: f32,
    pub viewport_min_height: f32,
    pub viewport_lg_width: f32,
    pub viewport_lg_height: f32,
    pub viewport_xl_width: f32,
    pub viewport_xl_height: f32,
    pub content_max_width: f32,
    pub dashboard_max_width: f32,
    pub home_two_col_content: f32,
    pub home_quick_one_col_content: f32,
    pub home_quick_four_col_content: f32,
    pub home_illustration_full_content: f32,
    pub home_illustration_hide_content: f32,
    pub home_compact_header_content: f32,
}

impl Default for ResponsiveTokens {
    fn default() -> Self {
        Self {
            viewport_ref_width: 1280.0,
            viewport_ref_height: 800.0,
            viewport_min_width: 1024.0,
            viewport_min_height: 720.0,
            viewport_lg_width: 1440.0,
            viewport_lg_height: 900.0,
            viewport_xl_width: 1920.0,
            viewport_xl_height: 1080.0,
            content_max_width: 720.0,
            dashboard_max_width: 1480.0,
            home_two_col_content: 720.0,
            home_quick_one_col_content: 520.0,
            home_quick_four_col_content: 1000.0,
            home_illustration_full_content: 720.0,
            home_illustration_hide_content: 520.0,
            home_compact_header_content: 560.0,
        }
    }
}

// ── Motion tokens (presentation) ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionTokens {
    /// Sidebar section appear-animation frame count (ui_components.rs:1605).
    pub sidebar_fade_frames: u32,
}

impl Default for MotionTokens {
    fn default() -> Self {
        Self {
            sidebar_fade_frames: 5,
        }
    }
}

// ── Sidebar theme ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarTheme {
    /// Target sidebar width at the reference viewport (304 px).
    pub width: f32,
    /// Minimum responsive sidebar width (288 px).
    pub width_min: f32,
    /// Maximum responsive sidebar width (320 px).
    pub width_max: f32,
    /// Horizontal inset from sidebar edges to content (24 px).
    pub inset: f32,
    /// Hover pill / unread badge radius (10 px).
    pub item_radius: f32,
    /// Avatar container radius (12 px).
    pub avatar_container_radius: f32,
    /// Utility/status icon size in the sidebar footer (24 px).
    pub utility_icon_size: f32,
    /// Sidebar contact/peer name size (15 px).
    pub name_size: f32,
    /// All-caps section label size (11 px).
    pub section_label_size: f32,
    /// Padding regions (audit §3.1 — sidebar.rs `iced::Padding` literals).
    pub padding: SidebarPadding,
}

/// Sidebar padding regions, decomposed from the `iced::Padding` literals in
/// `app/sidebar.rs` (values are SPACE_* tokens).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarPadding {
    /// Pinned brand row: top (SPACE_16).
    pub brand_top: f32,
    /// Pinned brand row: bottom (SPACE_8).
    pub brand_bottom: f32,
    /// Pinned identity row: top (SPACE_4).
    pub identity_top: f32,
    /// Pinned identity row: bottom (SPACE_8).
    pub identity_bottom: f32,
    /// Scrollable sections column: top (SPACE_4).
    pub section_top: f32,
    /// Bottom utility row: top (SPACE_8).
    pub utility_top: f32,
    /// Bottom utility row: bottom (SPACE_12).
    pub utility_bottom: f32,
    /// Horizontal row padding for sidebar rows (SPACE_12).
    pub row_x: f32,
    /// Join-by-ticket label block: top (SPACE_8).
    pub join_top: f32,
    /// Join-by-ticket label block: bottom (SPACE_4).
    pub join_bottom: f32,
}

impl Default for SidebarTheme {
    fn default() -> Self {
        Self {
            width: 304.0,
            width_min: 288.0,
            width_max: 320.0,
            inset: 24.0,
            item_radius: 10.0,
            avatar_container_radius: 12.0,
            utility_icon_size: 24.0,
            name_size: 15.0,
            section_label_size: 11.0,
            padding: SidebarPadding {
                brand_top: 16.0,
                brand_bottom: 8.0,
                identity_top: 4.0,
                identity_bottom: 8.0,
                section_top: 4.0,
                utility_top: 8.0,
                utility_bottom: 12.0,
                row_x: 12.0,
                join_top: 8.0,
                join_bottom: 4.0,
            },
        }
    }
}

// ── Home dashboard theme ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HomeTheme {
    /// Minimum Online Peers body height (128 px).
    pub peers_body_min: f32,
    /// Recent-activity row height (32 px).
    pub activity_row_height: f32,
    /// Hero/status card spacing (40 px, home.rs:775).
    pub hero_gap: f32,
    /// Vertical gap between dashboard cards (SPACE_20 = 20 px).
    pub quick_action_gap: f32,
    /// Quick-action icon container diameter (40 px).
    pub quick_action_icon_size: f32,
    /// Quick-action card title size (16 px).
    pub quick_action_title_size: f32,
    /// Quick-action card description size (14 px).
    pub quick_action_desc_size: f32,
    /// Quick-action card description line height (1.45).
    pub quick_action_desc_line_height: f32,
    /// Status card: minimum text width in Medium tier (260 px).
    pub status_card_text_min_width_medium: f32,
    /// Status card: decorative mesh max width (170 px).
    pub status_card_mesh_max_width: f32,
    /// Status card: horizontal padding (SPACE_24 = 24 px).
    pub status_card_padding_x: f32,
    /// Status card: icon→text gap, Full tier (24 px).
    pub status_icon_text_gap_full: f32,
    /// Status card: icon→text gap, Medium tier (20 px).
    pub status_icon_text_gap_medium: f32,
    /// Status card: text→graph gap, Full tier (24 px).
    pub status_text_graph_gap_full: f32,
    /// Status card: text→graph gap, Medium tier (24 px).
    pub status_text_graph_gap_medium: f32,
    /// Status card: accent divider width (44 px).
    pub status_divider_width: f32,
    /// Status card: accent divider height (3 px).
    pub status_divider_height: f32,
    /// Status card: accent divider radius (1.5 px).
    pub status_divider_radius: f32,
    /// Status card: security pill radius (14 px).
    pub security_pill_radius: f32,
    /// Whether the Recent Activity feed slice inside the home "People &
    /// Activity" card is rendered (BORU-UI-09: optional visual feature,
    /// toggled from the dev UI Inspector). `true` is the baseline UI.
    pub show_activity_feed: bool,
}

impl Default for HomeTheme {
    fn default() -> Self {
        Self {
            peers_body_min: 128.0,
            activity_row_height: 32.0,
            hero_gap: 40.0,
            quick_action_gap: 20.0,
            quick_action_icon_size: 40.0,
            quick_action_title_size: 16.0,
            quick_action_desc_size: 14.0,
            quick_action_desc_line_height: 1.45,
            status_card_text_min_width_medium: 260.0,
            status_card_mesh_max_width: 170.0,
            status_card_padding_x: 24.0,
            status_icon_text_gap_full: 24.0,
            status_icon_text_gap_medium: 20.0,
            status_text_graph_gap_full: 24.0,
            status_text_graph_gap_medium: 24.0,
            status_divider_width: 44.0,
            status_divider_height: 3.0,
            status_divider_radius: 1.5,
            security_pill_radius: 14.0,
            show_activity_feed: true,
        }
    }
}

// ── Chat theme ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatTheme {
    /// Spinner glyph size (40 px).
    pub spinner_size: f32,
    /// Right-click context menu column width (180 px).
    pub context_menu_width: f32,
    /// Emoji picker card width reference (336 px — fits the reference
    /// 8-column grid of 36 px cells with 4 px spacing plus body padding;
    /// BORU-TWEMOJI-10). BORU-TWEMOJI-11 made the live card responsive:
    /// this token is the baseline the picker prefers when the window has
    /// room, while the actual width adapts to the available space
    /// (334–374 px, capped at ~400 px) and never exceeds it. Since
    /// BORU-TWEMOJI-12 the card is also at least wide enough for the
    /// 8-category tab row (~302 px) when space permits.
    pub emoji_picker_width: f32,
    /// Emoji picker scroll region height baseline (200 px — a comfortable
    /// floor for the active category's grid rows at the reference 8-column
    /// layout). BORU-TWEMOJI-11 made the live region responsive and
    /// BORU-TWEMOJI-12 made it category-aware: this token is the minimum
    /// when the window is tall enough, and the region grows with the
    /// category's grid content up to ~340 px (card ≤ ~400 px) when space
    /// permits, or shrinks to fit short windows.
    pub emoji_picker_scroll_height: f32,
    /// GIF picker panel width (320 px).
    pub gif_picker_width: f32,
    /// GIF picker scroll height (300 px).
    pub gif_picker_scroll_height: f32,
    /// GIF thumbnail width (150 px).
    pub gif_thumbnail_width: f32,
    /// GIF thumbnail height (100 px).
    pub gif_thumbnail_height: f32,
    /// Screen-share viewer box width (640 px).
    pub screen_share_w: f32,
    /// Screen-share viewer box height (360 px).
    pub screen_share_h: f32,
    /// Chat bubble hard maximum width (560 px).
    pub bubble_max_width: f32,
    /// Chat bubble width as a fraction of the timeline (0.68).
    pub bubble_width_ratio: f32,
    /// Message content max width (480 px).
    pub message_max_width: f32,
    /// Inline image preview max width (360 px).
    pub image_preview_max_width: f32,
    /// Inline image preview max height (400 px).
    pub image_preview_max_height: f32,
}

impl Default for ChatTheme {
    fn default() -> Self {
        Self {
            spinner_size: 40.0,
            context_menu_width: 180.0,
            emoji_picker_width: 336.0,
            emoji_picker_scroll_height: 200.0,
            gif_picker_width: 320.0,
            gif_picker_scroll_height: 300.0,
            gif_thumbnail_width: 150.0,
            gif_thumbnail_height: 100.0,
            screen_share_w: 640.0,
            screen_share_h: 360.0,
            bubble_max_width: 560.0,
            bubble_width_ratio: 0.68,
            message_max_width: 480.0,
            image_preview_max_width: 360.0,
            image_preview_max_height: 400.0,
        }
    }
}

// ── Attachment theme (files / shared table / downloads / video) ──────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttachmentTheme {
    /// Empty-state art box height (200 px, files.rs:1172).
    pub empty_state_height: f32,
    /// Share-menu popover width (176 px, shared_by_me_table.rs:531).
    pub menu_width: f32,
    /// Recipient chip avatar diameter (16 px).
    pub chip_avatar_size: f32,
    /// Recipient chip avatar label size (9 px).
    pub chip_label_size: f32,
    /// Row detail label width (96 px, shared_by_me_table.rs:1250).
    pub detail_label_width: f32,
    /// Download progress-bar girth (6 px).
    pub progress_bar_girth: f32,
    /// Percentage label fixed width next to the bar (44 px).
    pub progress_pct_label_width: f32,
    /// Reserved progress-row slot height (20 px).
    pub progress_slot_height: f32,
    /// Reserved in-flight detail slot height (18 px).
    pub detail_slot_height: f32,
    /// Reserved overwrite-policy slot height (30 px).
    pub policy_slot_height: f32,
    /// Action-row button line height estimate (30 px).
    pub action_button_line: f32,
    /// File-sharing search box width, medium tier (240 px, files.rs:3976).
    pub search_width_medium: f32,
    /// File-sharing search box width, full tier (320 px, files.rs:3978).
    pub search_width_full: f32,
    /// File-dashboard table column widths (app/files.rs).
    pub file_table: FileTableColumns,
    /// "Files I'm Sharing" table column widths (shared_by_me_table.rs).
    pub shared_table: SharedTableColumns,
    /// Video attachment card geometry (video_file_card.rs).
    pub video: VideoTokens,
}

/// Column widths for the file-dashboard tables (`app/files.rs` fixed widths:
/// 72 / 120 / 140 / 100 / 110 / 90 / 80 / 240 …). Values are the current
/// `Length::Fixed(...)` literals at the audit-cited rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileTableColumns {
    pub size_col: f32,
    pub source_col: f32,
    pub ago_col: f32,
    pub peer_col: f32,
    pub started_col: f32,
    pub state_col: f32,
    pub direction_col: f32,
    pub event_col: f32,
    pub details_col: f32,
    /// Download-manager transfer row: Started column (100 px, files.rs:2616).
    pub download_started_col: f32,
    /// Download-manager / uploads row: State column (100 px, files.rs:2622/2754).
    pub download_state_col: f32,
    /// Activity-log row: Ago column (110 px, files.rs:3572).
    pub activity_ago_col: f32,
}

/// Column widths for the "Files I'm Sharing" card (`COL_*` in
/// shared_by_me_table.rs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SharedTableColumns {
    pub shared_with: f32,
    pub size: f32,
    pub shared_on: f32,
    pub downloads: f32,
    pub actions: f32,
}

/// Video attachment card tokens (`video_file_card.rs`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoTokens {
    /// Below this timeline width the card switches to the 100%-width layout (560 px).
    pub narrow_breakpoint: f32,
    /// Below this timeline width the media caps are scaled down (780 px).
    pub medium_breakpoint: f32,
    /// Play overlay button diameter (64 px).
    pub play_overlay_size: f32,
    /// Hard width cap for the header filename (420 px).
    pub header_filename_max_width: f32,
    /// Inline volume slider width (90 px).
    pub controls_slider_width: f32,
}

impl Default for AttachmentTheme {
    fn default() -> Self {
        Self {
            empty_state_height: 200.0,
            menu_width: 176.0,
            chip_avatar_size: 16.0,
            chip_label_size: 9.0,
            detail_label_width: 96.0,
            progress_bar_girth: 6.0,
            progress_pct_label_width: 44.0,
            progress_slot_height: 20.0,
            detail_slot_height: 18.0,
            policy_slot_height: 30.0,
            action_button_line: 30.0,
            search_width_medium: 240.0,
            search_width_full: 320.0,
            file_table: FileTableColumns {
                size_col: 72.0,
                source_col: 120.0,
                ago_col: 120.0,
                peer_col: 140.0,
                started_col: 120.0,
                state_col: 110.0,
                direction_col: 90.0,
                event_col: 110.0,
                details_col: 80.0,
                download_started_col: 100.0,
                download_state_col: 100.0,
                activity_ago_col: 110.0,
            },
            shared_table: SharedTableColumns {
                shared_with: 144.0,
                size: 64.0,
                shared_on: 122.0,
                downloads: 80.0,
                actions: 36.0,
            },
            video: VideoTokens {
                narrow_breakpoint: 560.0,
                medium_breakpoint: 780.0,
                play_overlay_size: 64.0,
                header_filename_max_width: 420.0,
                controls_slider_width: 90.0,
            },
        }
    }
}

// ── Public room / discover theme ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomTheme {
    /// Catalogue row height (52 px).
    pub catalogue_row_height: f32,
    /// Catalogue lazy-list overscan (800 px).
    pub overscan: f32,
    /// Room banner / context-menu width (200 px, discover.rs:1459).
    pub banner_width: f32,
    /// Room join-progress bar length (80 px).
    pub progress_length: f32,
    /// Room join-progress bar girth (6 px).
    pub progress_girth: f32,
}

impl Default for RoomTheme {
    fn default() -> Self {
        Self {
            catalogue_row_height: 52.0,
            overscan: 800.0,
            banner_width: 200.0,
            progress_length: 80.0,
            progress_girth: 6.0,
        }
    }
}

// ── Tunnel theme ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TunnelTheme {
    /// Tunnel status chip horizontal padding (6 px, tunnels.rs:192 `[2, 6]`).
    pub chip_padding_x: f32,
    /// Tunnel status chip vertical padding (2 px).
    pub chip_padding_y: f32,
}

impl Default for TunnelTheme {
    fn default() -> Self {
        Self {
            chip_padding_x: 6.0,
            chip_padding_y: 2.0,
        }
    }
}

// ── Dialog theme ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialogTheme {
    /// Dialog avatar size (72 px, dialogs.rs:26).
    pub avatar_size: f32,
    /// Dialog avatar fallback glyph size (48 px).
    pub avatar_glyph_size: f32,
    /// Incoming-call dialog title size (22 px).
    pub title_size: f32,
    /// Dialog body size (15 px).
    pub body_size: f32,
    /// Dialog column spacing (12 px).
    pub spacing: f32,
    /// Dialog panel padding (32 px).
    pub padding: f32,
    /// Dialog control button horizontal padding (14 px, dialogs.rs:724 `[6, 14]`).
    pub control_padding_x: f32,
    /// Dialog control button vertical padding (6 px).
    pub control_padding_y: f32,
    /// Dialog control row spacing (12 px).
    pub control_spacing: f32,
}

impl Default for DialogTheme {
    fn default() -> Self {
        Self {
            avatar_size: 72.0,
            avatar_glyph_size: 48.0,
            title_size: 22.0,
            body_size: 15.0,
            spacing: 12.0,
            padding: 32.0,
            control_padding_x: 14.0,
            control_padding_y: 6.0,
            control_spacing: 12.0,
        }
    }
}

// ── Call theme ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CallTheme {
    /// Call avatar box (96 × 96 px).
    pub avatar_size: f32,
    /// Outgoing-call avatar glyph size (36 px).
    pub avatar_glyph_size: f32,
    /// Remote-fallback avatar glyph size (44 px).
    pub avatar_glyph_size_large: f32,
    /// Local PiP frame width (220 px).
    pub pip_w: f32,
    /// Local PiP frame height (150 px).
    pub pip_h: f32,
    /// Gap above the controls row (40 px, calls.rs:47).
    pub controls_gap: f32,
}

impl Default for CallTheme {
    fn default() -> Self {
        Self {
            avatar_size: 96.0,
            avatar_glyph_size: 36.0,
            avatar_glyph_size_large: 44.0,
            pip_w: 220.0,
            pip_h: 150.0,
            controls_gap: 40.0,
        }
    }
}

// ── Settings / control tokens ─────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlTokens {
    /// Settings header bar height (52 px).
    pub header_height: f32,
    /// Slider width (160 px, settings.rs:999).
    pub slider_width: f32,
    /// Color-picker border radius (8 px).
    pub color_picker_radius: f32,
    /// Color-picker bar radius (4 px).
    pub color_picker_bar_radius: f32,
}

impl Default for ControlTokens {
    fn default() -> Self {
        Self {
            header_height: 52.0,
            slider_width: 160.0,
            color_picker_radius: 8.0,
            color_picker_bar_radius: 4.0,
        }
    }
}

// ── Screen-share sender UI theme (BORU-SSUI-08) ──────────────────────
//
// Semantic style tokens for the sender screen-share control card
// (`screen_share.card.*`, `source_card.*`, `segmented.*`, `toggle.*`,
// `action.*`, `destructive.*`). Geometry defaults reuse the shared Boru
// spacing / radius values from `design_tokens`; colours stay mode-aware
// and are resolved at style time through `design_tokens` (never baked as
// white backgrounds or fixed dark text). The group mirrors the PDF Task 8
// suggested categories so `boru-ui.toml` can restyle the sender controls
// through the same TOML/hot-reload system as the rest of the UI.

/// `screen_share.card.*` — the parent screen-share control card shell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShareCardTheme {
    /// Inner padding of the card shell (16 px — `SPACE_16`).
    pub padding: f32,
    /// Card corner radius (12 px — `RADIUS_LG`).
    pub radius: f32,
    /// Card border width (1 px — `BORDER_WIDTH`).
    pub border_width: f32,
    /// Vertical rhythm between the card's control rows (8 px — `SPACE_8`).
    pub spacing: f32,
    /// Card-title peer-name char budget before ellipsis (BORU-SSUI-09).
    /// The "Sharing your screen with {name}" title truncates a longer peer
    /// name with an ellipsis so it never blows the card width (32 chars).
    pub title_max_chars: f32,
}

/// `screen_share.source_card.*` — the screen/window source selector cards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShareSourceCardTheme {
    /// Fixed width of one source card (192 px).
    pub width: f32,
    /// Source-card corner radius (10 px — `RADIUS_MD`).
    pub radius: f32,
    /// Horizontal padding inside a source card (10 px — `SPACE_10`).
    pub padding_x: f32,
    /// Vertical padding inside a source card (8 px — `SPACE_8`).
    pub padding_y: f32,
    /// Source-kind icon size (20 px — `IconSize::Md`).
    pub icon_size: f32,
    /// Selected-check icon size (18 px — `IconSize::Sm`).
    pub check_icon_size: f32,
    /// Selected source accent border width (2 px).
    pub selected_border_width: f32,
    /// Title char budget before ellipsis (20 chars).
    pub title_max_chars: f32,
    /// Horizontal gap between cards in the scrollable source row (8 px — `SPACE_8`).
    pub row_spacing: f32,
}

/// `screen_share.segmented.*` — the quality segmented control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShareSegmentedTheme {
    /// Segment corner radius (10 px — `RADIUS_MD`).
    pub radius: f32,
    /// Gap between segments (4 px — `SPACE_4`).
    pub spacing: f32,
    /// Horizontal padding inside a segment (10 px — `SPACE_10`).
    pub padding_x: f32,
    /// Vertical padding inside a segment (4 px — `SPACE_4`).
    pub padding_y: f32,
    /// Selected-segment checkmark size (16 px — `IconSize::Xs`). The check
    /// is the non-colour secondary cue on the selected segment
    /// (BORU-SSUI-10).
    pub check_icon_size: f32,
}

/// `screen_share.toggle.*` — the audio toggle row geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShareToggleTheme {
    /// Gap between the toggle row's icon / label / switch (8 px — `SPACE_8`).
    pub row_spacing: f32,
    /// Speaker icon size (18 px — `IconSize::Sm`).
    pub icon_size: f32,
}

/// `screen_share.action.*` — the neutral action row (Share Again / Dismiss).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShareActionTheme {
    /// Gap between actions in the row (8 px — `SPACE_8`).
    pub row_spacing: f32,
}

/// `screen_share.destructive.*` — the destructive Stop Sharing button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShareDestructiveTheme {
    /// Horizontal padding inside the destructive button (16 px — `SPACE_16`).
    pub padding_x: f32,
    /// Vertical padding inside the destructive button (8 px — `SPACE_8`).
    pub padding_y: f32,
    /// Destructive button corner radius (10 px — `RADIUS_MD`).
    pub radius: f32,
    /// Gap between the stop icon and the label (8 px — `SPACE_8`).
    pub icon_gap: f32,
}

/// `screen_share.*` — semantic style tokens for the sender screen-share UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShareTheme {
    /// Parent control card shell.
    pub card: ScreenShareCardTheme,
    /// Screen/window source selector cards.
    pub source_card: ScreenShareSourceCardTheme,
    /// Quality segmented control.
    pub segmented: ScreenShareSegmentedTheme,
    /// Audio toggle row.
    pub toggle: ScreenShareToggleTheme,
    /// Neutral action row.
    pub action: ScreenShareActionTheme,
    /// Destructive Stop Sharing button.
    pub destructive: ScreenShareDestructiveTheme,
}

impl Default for ScreenShareTheme {
    fn default() -> Self {
        use crate::design_tokens::{
            BORDER_WIDTH, RADIUS_LG, RADIUS_MD, SPACE_10, SPACE_16, SPACE_4, SPACE_8,
        };
        use crate::icon_system::IconSize;
        Self {
            card: ScreenShareCardTheme {
                padding: SPACE_16,
                radius: RADIUS_LG,
                border_width: BORDER_WIDTH,
                spacing: SPACE_8,
                title_max_chars: 32.0,
            },
            source_card: ScreenShareSourceCardTheme {
                width: 192.0,
                radius: RADIUS_MD,
                padding_x: SPACE_10,
                padding_y: SPACE_8,
                icon_size: IconSize::Md.px(),
                check_icon_size: IconSize::Sm.px(),
                selected_border_width: 2.0,
                title_max_chars: 20.0,
                row_spacing: SPACE_8,
            },
            segmented: ScreenShareSegmentedTheme {
                radius: RADIUS_MD,
                spacing: SPACE_4,
                padding_x: SPACE_10,
                padding_y: SPACE_4,
                check_icon_size: IconSize::Xs.px(),
            },
            toggle: ScreenShareToggleTheme {
                row_spacing: SPACE_8,
                icon_size: IconSize::Sm.px(),
            },
            action: ScreenShareActionTheme {
                row_spacing: SPACE_8,
            },
            destructive: ScreenShareDestructiveTheme {
                padding_x: SPACE_16,
                padding_y: SPACE_8,
                radius: RADIUS_MD,
                icon_gap: SPACE_8,
            },
        }
    }
}

// ── BoruTheme — the typed theme root ──────────────────────────────────

/// Central typed theme for Boru's visual properties (PDF Task 2).
///
/// `Default` reproduces the current light-mode UI byte-for-byte;
/// [`BoruTheme::dark`] mirrors the dark palette. The per-component groups
/// (`sidebar`, `home`, `chat`, …) hold the audit-cited geometry tokens;
/// `colors` holds both modes' palettes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoruTheme {
    /// Semantic colour tokens (mode-aware).
    pub colors: ColorTokens,
    /// Typography sizes (families/weights stay in `fonts::TypeRole`).
    pub typography: TypographyTokens,
    /// Spacing scale and control heights.
    pub spacing: SpacingTokens,
    /// Corner radii.
    pub radii: RadiusTokens,
    /// Icon sizes.
    pub icons: IconTokens,
    /// Avatar sizes.
    pub avatars: AvatarTokens,
    /// List / row heights.
    pub lists: ListTokens,
    /// Border widths.
    pub borders: BorderTokens,
    /// Responsive / layout breakpoints.
    pub responsive: ResponsiveTokens,
    /// Presentation motion counts.
    pub motion: MotionTokens,
    /// Sidebar / global shell.
    pub sidebar: SidebarTheme,
    /// Home dashboard.
    pub home: HomeTheme,
    /// Chat message list + composer.
    pub chat: ChatTheme,
    /// File / shared / download / video attachments.
    pub attachments: AttachmentTheme,
    /// Public rooms / discover.
    pub rooms: RoomTheme,
    /// Tunnels.
    pub tunnels: TunnelTheme,
    /// Dialogs.
    pub dialogs: DialogTheme,
    /// Calls.
    pub calls: CallTheme,
    /// Settings / generic controls.
    pub controls: ControlTokens,
    /// Screen-share sender UI (BORU-SSUI-08).
    pub screen_share: ScreenShareTheme,
}

impl BoruTheme {
    /// The current light-mode theme — the baseline appearance.
    pub fn light() -> Self {
        Self::default()
    }

    /// The current dark-mode theme (palette swapped, geometry unchanged).
    pub fn dark() -> Self {
        Self {
            colors: ColorTokens::dark(),
            ..Self::default()
        }
    }

    /// Pick the theme matching an Iced theme (Light/Dark).
    pub fn for_theme(theme: &iced::Theme) -> Self {
        if matches!(theme, iced::Theme::Dark) {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// Resolve the font for a canonical role from the live theme
    /// (BORU-UI-16).
    ///
    /// Uses the role's family-group choice and weight mapping. If the
    /// configured family is not one of the bundled families (should not
    /// happen after merge validation, but guarded defensively), it logs a
    /// warning and falls back to the role's `TypeRole` default font. This
    /// never reloads font files — the bundled families are loaded once at
    /// startup and the theme only picks which already-loaded family/weight
    /// a role resolves to.
    pub fn type_font(&self, role: crate::fonts::TypeRole) -> iced::Font {
        let family = self.typography.family_for(role);
        if !family.is_bundled() {
            tracing::warn!(
                family = family.name(),
                role = role.label(),
                "configured font family is not bundled; falling back to default font"
            );
            return role.font();
        }
        family.font(self.typography.weight_for(role))
    }

    /// Resolve the size for a canonical role from the live theme
    /// (BORU-UI-16).
    pub fn type_size(&self, role: crate::fonts::TypeRole) -> f32 {
        self.typography.size_for(role)
    }

    /// Resolve the relative line height for a canonical role from the live
    /// theme (BORU-UI-16).
    pub fn type_line_height(&self, role: crate::fonts::TypeRole) -> f32 {
        self.typography.line_height_for(role)
    }
}

impl Default for BoruTheme {
    fn default() -> Self {
        Self {
            colors: ColorTokens::default(),
            typography: TypographyTokens::default(),
            spacing: SpacingTokens::default(),
            radii: RadiusTokens::default(),
            icons: IconTokens::default(),
            avatars: AvatarTokens::default(),
            lists: ListTokens::default(),
            borders: BorderTokens::default(),
            responsive: ResponsiveTokens::default(),
            motion: MotionTokens::default(),
            sidebar: SidebarTheme::default(),
            home: HomeTheme::default(),
            chat: ChatTheme::default(),
            attachments: AttachmentTheme::default(),
            rooms: RoomTheme::default(),
            tunnels: TunnelTheme::default(),
            dialogs: DialogTheme::default(),
            calls: CallTheme::default(),
            controls: ControlTokens::default(),
            screen_share: ScreenShareTheme::default(),
        }
    }
}

/// The live theme produced by merging `BoruTheme::default()` with optional
/// `boru-ui.toml` overrides (PDF Task 5 / BORU-UI-05).
///
/// This is an alias, not a separate struct: `BoruTheme` already is the type
/// every view/style call site consumes, so the merge in `theme_merge` just
/// returns a (possibly overridden) `BoruTheme`. The alias keeps the PDF's
/// `ActiveTheme` vocabulary available for later tasks (BORU-UI-06/07 file
/// watching + live redraw) without introducing a parallel type.
pub type ActiveTheme = BoruTheme;

// ── Tests ─────────────────────────────────────────────────────────────
//
// Every token is asserted against the existing source modules
// (`design_tokens`, `fonts`, `icon_system`, `card_shell`) so the typed
// theme can never drift from the live UI. If a source value is ever
// intentionally changed, these tests force the theme to be updated in the
// same commit.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_shell;
    use crate::design_tokens;
    use crate::fonts::{self, TypeRole};
    use crate::icon_system::IconSize;
    use iced::Theme;

    #[test]
    fn light_palette_matches_design_tokens() {
        let c = ColorTokens::light();
        let t = Theme::Light;
        assert_eq!(c.canvas, design_tokens::color_canvas(&t));
        assert_eq!(c.sidebar, design_tokens::color_sidebar(&t));
        assert_eq!(c.surface, design_tokens::surface(&t));
        assert_eq!(c.surface_elevated, design_tokens::surface(&t));
        assert_eq!(c.surface_selected, design_tokens::surface_selected(&t));
        assert_eq!(c.surface_hover, design_tokens::surface_hover(&t));
        assert_eq!(c.surface_pressed, design_tokens::surface_pressed(&t));
        assert_eq!(c.surface_secondary, design_tokens::surface_secondary(&t));
        assert_eq!(c.input_bg, design_tokens::bg_input(&t));
        assert_eq!(c.border_muted, design_tokens::border_muted(&t));
        assert_eq!(c.border_strong, design_tokens::border_strong(&t));
        assert_eq!(c.text_primary, design_tokens::text_primary(&t));
        assert_eq!(c.text_secondary, design_tokens::text_secondary(&t));
        assert_eq!(c.text_muted, design_tokens::text_muted(&t));
        assert_eq!(c.text_local_label, design_tokens::text_local_label(&t));
        assert_eq!(c.text_local_body, design_tokens::text_local_body(&t));
        assert_eq!(c.text_remote_label, design_tokens::text_remote_label(&t));
        assert_eq!(c.text_remote_body, design_tokens::text_remote_body(&t));
        assert_eq!(c.primary, design_tokens::primary(&t));
        assert_eq!(c.primary_hover, design_tokens::primary_hover(&t));
        assert_eq!(c.primary_pressed, design_tokens::primary_pressed(&t));
        assert_eq!(c.primary_soft, design_tokens::primary_soft(&t));
        assert_eq!(c.success, design_tokens::color_success(&t));
        assert_eq!(c.danger, design_tokens::color_danger(&t));
        assert_eq!(c.warning, design_tokens::color_warning(&t));
        assert_eq!(c.focus, design_tokens::color_focus(&t));
        assert_eq!(c.dialog_backdrop, design_tokens::dialog_backdrop(&t));
        assert_eq!(c.destructive_soft(), design_tokens::destructive_soft(&t));
        assert_eq!(c.success_soft(), design_tokens::success_soft(&t));
        assert_eq!(c.warning_soft(), design_tokens::warning_soft(&t));
    }

    #[test]
    fn dark_palette_matches_design_tokens() {
        let c = ColorTokens::dark();
        let t = Theme::Dark;
        assert_eq!(c.canvas, design_tokens::color_canvas(&t));
        assert_eq!(c.sidebar, design_tokens::color_sidebar(&t));
        assert_eq!(c.surface, design_tokens::surface(&t));
        assert_eq!(c.surface_elevated, design_tokens::surface(&t));
        assert_eq!(c.surface_selected, design_tokens::surface_selected(&t));
        assert_eq!(c.surface_hover, design_tokens::surface_hover(&t));
        assert_eq!(c.surface_pressed, design_tokens::surface_pressed(&t));
        assert_eq!(c.surface_secondary, design_tokens::surface_secondary(&t));
        assert_eq!(c.input_bg, design_tokens::bg_input(&t));
        assert_eq!(c.border_muted, design_tokens::border_muted(&t));
        assert_eq!(c.border_strong, design_tokens::border_strong(&t));
        assert_eq!(c.text_primary, design_tokens::text_primary(&t));
        assert_eq!(c.text_secondary, design_tokens::text_secondary(&t));
        assert_eq!(c.text_muted, design_tokens::text_muted(&t));
        assert_eq!(c.text_local_label, design_tokens::text_local_label(&t));
        assert_eq!(c.text_local_body, design_tokens::text_local_body(&t));
        assert_eq!(c.text_remote_label, design_tokens::text_remote_label(&t));
        assert_eq!(c.text_remote_body, design_tokens::text_remote_body(&t));
        assert_eq!(c.primary, design_tokens::primary(&t));
        assert_eq!(c.primary_hover, design_tokens::primary_hover(&t));
        assert_eq!(c.primary_pressed, design_tokens::primary_pressed(&t));
        assert_eq!(c.primary_soft, design_tokens::primary_soft(&t));
        assert_eq!(c.success, design_tokens::color_success(&t));
        assert_eq!(c.danger, design_tokens::color_danger(&t));
        assert_eq!(c.warning, design_tokens::color_warning(&t));
        assert_eq!(c.focus, design_tokens::color_focus(&t));
        assert_eq!(c.dialog_backdrop, design_tokens::dialog_backdrop(&t));
        assert_eq!(c.destructive_soft(), design_tokens::destructive_soft(&t));
        assert_eq!(c.success_soft(), design_tokens::success_soft(&t));
        assert_eq!(c.warning_soft(), design_tokens::warning_soft(&t));
    }

    #[test]
    fn semantic_colour_tokens_map_to_backing_fields() {
        // PDF T17: every canonical semantic colour token must resolve to a
        // real ColorTokens value (fields or accessors) in both modes.
        for c in [ColorTokens::light(), ColorTokens::dark()] {
            // Tokens that exist as fields.
            let _ = (c.surface, c.surface_elevated, c.surface_hover);
            let _ = (c.text_primary, c.text_secondary);
            let _ = (c.success, c.warning, c.danger);
            // Tokens that exist as accessors over the older field names.
            assert_eq!(c.background(), c.canvas);
            assert_eq!(c.border(), c.border_muted);
            assert_eq!(c.accent(), c.primary);
            assert_eq!(c.accent_hover(), c.primary_hover);
        }
    }

    #[test]
    fn reference_accent_and_derived_states_are_stable() {
        let c = ColorTokens::light();
        assert_eq!(
            c.accent(),
            Color::from_rgb(142.0 / 255.0, 192.0 / 255.0, 124.0 / 255.0)
        );
        assert!(c.accent_hover().r < c.accent().r);
        assert!(c.primary_pressed.g < c.primary_hover.g);
        assert_eq!(c.primary_soft.a, 1.0);
    }

    #[test]
    fn status_card_palette_matches_design_tokens() {
        let c = ColorTokens::light();
        assert_eq!(c.status_card_bg_top, design_tokens::STATUS_CARD_BG_TOP);
        assert_eq!(c.status_card_bg_mid, design_tokens::STATUS_CARD_BG_MID);
        assert_eq!(
            c.status_card_bg_bottom,
            design_tokens::STATUS_CARD_BG_BOTTOM
        );
        assert_eq!(c.status_card_border, design_tokens::STATUS_CARD_BORDER);
        assert_eq!(c.status_connected, design_tokens::STATUS_CONNECTED);
        assert_eq!(c.status_primary_text, design_tokens::STATUS_PRIMARY_TEXT);
        assert_eq!(
            c.status_secondary_text,
            design_tokens::STATUS_SECONDARY_TEXT
        );
        assert_eq!(c.status_network_line, design_tokens::STATUS_NETWORK_LINE);
        assert_eq!(c.status_network_node, design_tokens::STATUS_NETWORK_NODE);
        // Dark mode carries the same theme-independent status-card palette.
        let dark = ColorTokens::dark();
        assert_eq!(dark.status_card_bg_top, design_tokens::STATUS_CARD_BG_TOP);
        assert_eq!(dark.status_connected, design_tokens::STATUS_CONNECTED);
    }

    #[test]
    fn typography_matches_fonts() {
        let t = TypographyTokens::default();
        assert_eq!(t.display_heading, TypeRole::DisplayHeading.size_px());
        assert_eq!(t.page_title, TypeRole::PageTitle.size_px());
        assert_eq!(t.section_title, TypeRole::SectionTitle.size_px());
        assert_eq!(t.card_title, TypeRole::CardTitle.size_px());
        assert_eq!(t.body, TypeRole::Body.size_px());
        assert_eq!(t.body_emphasised, TypeRole::BodyEmphasised.size_px());
        assert_eq!(t.button_label, TypeRole::ButtonLabel.size_px());
        assert_eq!(t.supporting_text, TypeRole::SupportingText.size_px());
        assert_eq!(t.metadata, TypeRole::Metadata.size_px());
        assert_eq!(t.chat_message, TypeRole::ChatMessage.size_px());
        assert_eq!(t.chat_sender, TypeRole::ChatSender.size_px());
        assert_eq!(t.chat_metadata, TypeRole::ChatMetadata.size_px());
        assert_eq!(t.composer_text, TypeRole::ComposerText.size_px());
        assert_eq!(t.technical_value, TypeRole::TechnicalValue.size_px());
        assert_eq!(t.brand_wordmark, TypeRole::BrandWordmark.size_px());
        assert_eq!(t.home_subtitle, fonts::HOME_SUBTITLE);
        assert_eq!(t.dialog_title, fonts::DIALOG_TITLE);
        assert_eq!(t.dialog_subtitle, fonts::DIALOG_SUBTITLE);
        // BORU-UI-16: family choices per role group match TypeRole families.
        assert_eq!(t.display_family, TypeRole::DisplayHeading.family_key());
        assert_eq!(t.ui_family, TypeRole::Body.family_key());
        assert_eq!(t.chat_family, TypeRole::ChatMessage.family_key());
        assert_eq!(t.technical_family, TypeRole::TechnicalValue.family_key());
        assert_eq!(t.brand_family, TypeRole::BrandWordmark.family_key());
    }

    #[test]
    fn typography_weights_match_type_role() {
        // BORU-UI-16: the per-role weight mapping defaults equal the
        // TypeRole weights so the live theme reproduces the baseline UI.
        let t = TypographyTokens::default();
        use crate::fonts::FontWeightKey;
        let cases: &[(TypeRole, FontWeightKey)] = &[
            (TypeRole::DisplayHeading, FontWeightKey::Bold),
            (TypeRole::PageTitle, FontWeightKey::Bold),
            (TypeRole::SectionTitle, FontWeightKey::Semibold),
            (TypeRole::CardTitle, FontWeightKey::Semibold),
            (TypeRole::Body, FontWeightKey::Normal),
            (TypeRole::BodyEmphasised, FontWeightKey::Semibold),
            (TypeRole::ButtonLabel, FontWeightKey::Semibold),
            (TypeRole::SupportingText, FontWeightKey::Normal),
            (TypeRole::Metadata, FontWeightKey::Normal),
            (TypeRole::ChatMessage, FontWeightKey::Normal),
            (TypeRole::ChatSender, FontWeightKey::Semibold),
            (TypeRole::ChatMetadata, FontWeightKey::Normal),
            (TypeRole::ComposerText, FontWeightKey::Normal),
            (TypeRole::TechnicalValue, FontWeightKey::Normal),
            (TypeRole::BrandWordmark, FontWeightKey::ExtraBold),
        ];
        for (role, expected) in cases {
            assert_eq!(t.weight_for(*role), *expected, "{role:?} weight");
        }
    }

    #[test]
    fn typography_line_heights_match_type_role() {
        // BORU-UI-16: per-role line-height defaults equal TypeRole's
        // defaults (display 1.2, chat message 1.45, everything else 1.3).
        let t = TypographyTokens::default();
        for role in TypeRole::ALL {
            assert_eq!(
                t.line_height_for(role),
                role.default_line_height(),
                "{role:?} line height"
            );
        }
    }

    #[test]
    fn type_font_resolves_from_theme_family_and_weight() {
        // BORU-UI-16: `BoruTheme::type_font` builds the bundled font for
        // the role's family-group choice + weight mapping.
        let theme = BoruTheme::default();
        assert_eq!(
            theme.type_font(TypeRole::ChatMessage),
            crate::fonts::figtree(iced::font::Weight::Normal)
        );
        assert_eq!(
            theme.type_font(TypeRole::DisplayHeading),
            crate::fonts::inter_tight(iced::font::Weight::Bold)
        );
        assert_eq!(
            theme.type_font(TypeRole::TechnicalValue),
            crate::fonts::jetbrains_mono(iced::font::Weight::Normal)
        );
        // A remapped family+weight changes the resolved font.
        let mut remapped = theme;
        remapped.typography.chat_family = crate::fonts::FontFamilyKey::PublicSans;
        remapped.typography.chat_message_weight = crate::fonts::FontWeightKey::Semibold;
        assert_eq!(
            remapped.type_font(TypeRole::ChatMessage),
            crate::fonts::public_sans(iced::font::Weight::Semibold)
        );
    }

    #[test]
    fn spacing_matches_design_tokens() {
        let s = SpacingTokens::default();
        assert_eq!(s.space_2, design_tokens::SPACE_2);
        assert_eq!(s.space_4, design_tokens::SPACE_4);
        assert_eq!(s.space_6, design_tokens::SPACE_6);
        assert_eq!(s.space_8, design_tokens::SPACE_8);
        assert_eq!(s.space_10, design_tokens::SPACE_10);
        assert_eq!(s.space_12, design_tokens::SPACE_12);
        assert_eq!(s.space_16, design_tokens::SPACE_16);
        assert_eq!(s.space_18, design_tokens::SPACE_18);
        assert_eq!(s.space_20, design_tokens::SPACE_20);
        assert_eq!(s.space_24, design_tokens::SPACE_24);
        assert_eq!(s.space_28, design_tokens::SPACE_28);
        assert_eq!(s.space_32, design_tokens::SPACE_32);
        assert_eq!(s.space_40, design_tokens::SPACE_40);
        assert_eq!(s.control_height, design_tokens::CONTROL_HEIGHT);
        assert_eq!(
            s.control_height_compact,
            design_tokens::CONTROL_HEIGHT_COMPACT
        );
    }

    #[test]
    fn radii_match_design_tokens() {
        let r = RadiusTokens::default();
        assert_eq!(r.sm, design_tokens::RADIUS_SM);
        assert_eq!(r.md, design_tokens::RADIUS_MD);
        assert_eq!(r.lg, design_tokens::RADIUS_LG);
        assert_eq!(r.xl, design_tokens::RADIUS_XL);
        assert_eq!(r.card, design_tokens::RADIUS_CARD);
    }

    #[test]
    fn icons_match_icon_system() {
        let i = IconTokens::default();
        assert_eq!(i.xs, IconSize::Xs.px());
        assert_eq!(i.sm, IconSize::Sm.px());
        assert_eq!(i.md, IconSize::Md.px());
        assert_eq!(i.lg, IconSize::Lg.px());
        assert_eq!(i.xl, IconSize::Xl.px());
    }

    #[test]
    fn avatars_match_design_tokens() {
        let a = AvatarTokens::default();
        assert_eq!(a.sm, design_tokens::AVATAR_SM);
        assert_eq!(a.md, design_tokens::AVATAR_MD);
        assert_eq!(a.lg, design_tokens::AVATAR_LG);
        assert_eq!(a.profile, design_tokens::AVATAR_PROFILE);
        assert_eq!(a.chat_list, design_tokens::AVATAR_CHAT_LIST);
        assert_eq!(a.chat_header, design_tokens::AVATAR_CHAT_HEADER);
        assert_eq!(a.msg, design_tokens::AVATAR_MSG);
        assert_eq!(a.status_dot_sm, design_tokens::STATUS_DOT_SM);
        assert_eq!(a.status_dot_lg, design_tokens::STATUS_DOT_LG);
    }

    #[test]
    fn lists_match_card_shell_and_design_tokens() {
        let l = ListTokens::default();
        assert_eq!(l.card_row_height, card_shell::CARD_ROW_HEIGHT);
        assert_eq!(l.peer_row_height, card_shell::PEER_ROW_HEIGHT);
        assert_eq!(
            l.default_list_max_height,
            card_shell::DEFAULT_LIST_MAX_HEIGHT
        );
        assert_eq!(l.table_row_height, design_tokens::TABLE_ROW_HEIGHT);
        assert_eq!(
            l.table_row_height_compact,
            design_tokens::TABLE_ROW_HEIGHT_COMPACT
        );
        assert_eq!(l.chip_height, design_tokens::CHIP_HEIGHT);
        assert_eq!(
            l.peer_panel_max_height,
            design_tokens::PEER_PANEL_MAX_HEIGHT
        );
        assert_eq!(l.progress_bar_height, design_tokens::PROGRESS_BAR_HEIGHT);
        assert_eq!(
            l.progress_bar_height_bold,
            design_tokens::PROGRESS_BAR_HEIGHT_BOLD
        );
    }

    #[test]
    fn sidebar_geometry_matches_design_tokens() {
        let s = SidebarTheme::default();
        assert_eq!(s.width, design_tokens::SIDEBAR_WIDTH);
        assert_eq!(s.width_min, design_tokens::SIDEBAR_WIDTH_MIN);
        assert_eq!(s.width_max, design_tokens::SIDEBAR_WIDTH_MAX);
        assert_eq!(s.inset, design_tokens::SIDEBAR_INSET);
    }

    #[test]
    fn chat_geometry_matches_design_tokens() {
        let c = ChatTheme::default();
        assert_eq!(c.bubble_max_width, design_tokens::CHAT_BUBBLE_MAX_WIDTH);
        assert_eq!(c.bubble_width_ratio, design_tokens::CHAT_BUBBLE_WIDTH_RATIO);
        assert_eq!(c.message_max_width, design_tokens::MESSAGE_MAX_WIDTH);
        assert_eq!(
            c.image_preview_max_width,
            design_tokens::IMAGE_PREVIEW_MAX_WIDTH
        );
        assert_eq!(
            c.image_preview_max_height,
            design_tokens::IMAGE_PREVIEW_MAX_HEIGHT
        );
    }

    #[test]
    fn responsive_matches_design_tokens() {
        let r = ResponsiveTokens::default();
        assert_eq!(r.viewport_ref_width, design_tokens::VIEWPORT_REF_WIDTH);
        assert_eq!(r.viewport_ref_height, design_tokens::VIEWPORT_REF_HEIGHT);
        assert_eq!(r.viewport_min_width, design_tokens::VIEWPORT_MIN_WIDTH);
        assert_eq!(r.viewport_min_height, design_tokens::VIEWPORT_MIN_HEIGHT);
        assert_eq!(r.viewport_lg_width, design_tokens::VIEWPORT_LG_WIDTH);
        assert_eq!(r.viewport_lg_height, design_tokens::VIEWPORT_LG_HEIGHT);
        assert_eq!(r.viewport_xl_width, design_tokens::VIEWPORT_XL_WIDTH);
        assert_eq!(r.viewport_xl_height, design_tokens::VIEWPORT_XL_HEIGHT);
        assert_eq!(r.content_max_width, design_tokens::CONTENT_MAX_WIDTH);
        assert_eq!(r.dashboard_max_width, design_tokens::DASHBOARD_MAX_WIDTH);
        assert_eq!(r.home_two_col_content, design_tokens::HOME_TWO_COL_CONTENT);
        assert_eq!(
            r.home_quick_one_col_content,
            design_tokens::HOME_QUICK_ONE_COL_CONTENT
        );
        assert_eq!(
            r.home_quick_four_col_content,
            design_tokens::HOME_QUICK_FOUR_COL_CONTENT
        );
        assert_eq!(
            r.home_illustration_full_content,
            design_tokens::HOME_ILLUSTRATION_FULL_CONTENT
        );
        assert_eq!(
            r.home_illustration_hide_content,
            design_tokens::HOME_ILLUSTRATION_HIDE_CONTENT
        );
        assert_eq!(
            r.home_compact_header_content,
            design_tokens::HOME_COMPACT_HEADER_CONTENT
        );
    }

    #[test]
    fn theme_is_copy_clone() {
        // Compile-time proof that the theme and all token groups are
        // Copy/Clone — passing by value twice must compile.
        fn assert_copy<T: Copy>(v: T) -> T {
            v
        }
        let theme = BoruTheme::default();
        let again = assert_copy(theme);
        let c = assert_copy(theme.colors);
        let t = assert_copy(theme.typography);
        let s = assert_copy(theme.spacing);
        let r = assert_copy(theme.radii);
        let i = assert_copy(theme.icons);
        let a = assert_copy(theme.avatars);
        let l = assert_copy(theme.lists);
        let b = assert_copy(theme.borders);
        let resp = assert_copy(theme.responsive);
        let m = assert_copy(theme.motion);
        let sb = assert_copy(theme.sidebar);
        let h = assert_copy(theme.home);
        let ch = assert_copy(theme.chat);
        let at = assert_copy(theme.attachments);
        let rm = assert_copy(theme.rooms);
        let tn = assert_copy(theme.tunnels);
        let dg = assert_copy(theme.dialogs);
        let cl = assert_copy(theme.calls);
        let ct = assert_copy(theme.controls);
        // Values survive the copy.
        assert_eq!(again, theme);
        assert_eq!(c.primary, theme.colors.primary);
        assert_eq!(t.body, theme.typography.body);
        assert_eq!(s.space_8, theme.spacing.space_8);
        assert_eq!(r.card, theme.radii.card);
        assert_eq!(i.lg, theme.icons.lg);
        assert_eq!(a.msg, theme.avatars.msg);
        assert_eq!(l.peer_row_height, theme.lists.peer_row_height);
        assert_eq!(b.hairline, theme.borders.hairline);
        assert_eq!(
            resp.dashboard_max_width,
            theme.responsive.dashboard_max_width
        );
        assert_eq!(m.sidebar_fade_frames, theme.motion.sidebar_fade_frames);
        assert_eq!(sb.width, theme.sidebar.width);
        assert_eq!(h.peers_body_min, theme.home.peers_body_min);
        assert_eq!(ch.bubble_max_width, theme.chat.bubble_max_width);
        assert_eq!(at.progress_bar_girth, theme.attachments.progress_bar_girth);
        assert_eq!(rm.catalogue_row_height, theme.rooms.catalogue_row_height);
        assert_eq!(tn.chip_padding_x, theme.tunnels.chip_padding_x);
        assert_eq!(dg.avatar_size, theme.dialogs.avatar_size);
        assert_eq!(cl.avatar_size, theme.calls.avatar_size);
        assert_eq!(ct.header_height, theme.controls.header_height);
    }

    #[test]
    fn for_theme_selects_mode() {
        assert_eq!(BoruTheme::for_theme(&Theme::Light), BoruTheme::light());
        assert_eq!(BoruTheme::for_theme(&Theme::Dark), BoruTheme::dark());
        assert_eq!(BoruTheme::default(), BoruTheme::light());
        // Dark differs only in the palette; geometry is shared.
        let light = BoruTheme::light();
        let dark = BoruTheme::dark();
        assert_ne!(light.colors, dark.colors);
        assert_eq!(light.typography, dark.typography);
        assert_eq!(light.spacing, dark.spacing);
        assert_eq!(light.sidebar, dark.sidebar);
        assert_eq!(light.chat, dark.chat);
        assert_eq!(light.home, dark.home);
        assert_eq!(light.attachments, dark.attachments);
        assert_eq!(light.rooms, dark.rooms);
    }

    #[test]
    fn default_matches_audit_source_values() {
        // Spot-check the audit-cited values that come from raw literals
        // (not design_tokens), so the typed theme captures them exactly.
        let theme = BoruTheme::default();
        // Sidebar (audit §3.1)
        assert_eq!(theme.sidebar.name_size, 15.0);
        assert_eq!(theme.sidebar.section_label_size, 11.0);
        assert_eq!(theme.sidebar.item_radius, 10.0);
        assert_eq!(theme.sidebar.utility_icon_size, 24.0);
        // Home (audit §3.2)
        assert_eq!(theme.home.peers_body_min, 128.0);
        assert_eq!(theme.home.activity_row_height, 32.0);
        assert_eq!(theme.home.hero_gap, 40.0);
        assert_eq!(theme.home.quick_action_icon_size, 40.0);
        assert_eq!(theme.home.quick_action_title_size, 16.0);
        assert_eq!(theme.home.quick_action_desc_size, 14.0);
        assert_eq!(theme.home.quick_action_desc_line_height, 1.45);
        // BORU-UI-09: optional visual features default to the baseline UI.
        assert!(
            theme.home.show_activity_feed,
            "activity feed shown by default"
        );
        // Chat (audit §3.3)
        assert_eq!(theme.chat.spinner_size, 40.0);
        assert_eq!(theme.chat.context_menu_width, 180.0);
        assert_eq!(theme.chat.emoji_picker_width, 336.0);
        assert_eq!(theme.chat.emoji_picker_scroll_height, 200.0);
        assert_eq!(theme.chat.gif_picker_width, 320.0);
        assert_eq!(theme.chat.gif_picker_scroll_height, 300.0);
        assert_eq!(theme.chat.gif_thumbnail_width, 150.0);
        assert_eq!(theme.chat.gif_thumbnail_height, 100.0);
        assert_eq!(theme.chat.screen_share_w, 640.0);
        assert_eq!(theme.chat.screen_share_h, 360.0);
        // Attachments (audit §3.4/§3.5)
        assert_eq!(theme.attachments.file_table.size_col, 72.0);
        assert_eq!(theme.attachments.file_table.source_col, 120.0);
        assert_eq!(theme.attachments.file_table.peer_col, 140.0);
        assert_eq!(theme.attachments.shared_table.shared_with, 144.0);
        assert_eq!(theme.attachments.shared_table.size, 64.0);
        assert_eq!(theme.attachments.shared_table.shared_on, 122.0);
        assert_eq!(theme.attachments.shared_table.downloads, 80.0);
        assert_eq!(theme.attachments.shared_table.actions, 36.0);
        assert_eq!(theme.attachments.progress_bar_girth, 6.0);
        assert_eq!(theme.attachments.progress_pct_label_width, 44.0);
        assert_eq!(theme.attachments.progress_slot_height, 20.0);
        assert_eq!(theme.attachments.detail_slot_height, 18.0);
        assert_eq!(theme.attachments.policy_slot_height, 30.0);
        assert_eq!(theme.attachments.action_button_line, 30.0);
        assert_eq!(theme.attachments.search_width_medium, 240.0);
        assert_eq!(theme.attachments.search_width_full, 320.0);
        assert_eq!(theme.attachments.file_table.download_started_col, 100.0);
        assert_eq!(theme.attachments.file_table.download_state_col, 100.0);
        assert_eq!(theme.attachments.file_table.activity_ago_col, 110.0);
        assert_eq!(theme.attachments.video.narrow_breakpoint, 560.0);
        assert_eq!(theme.attachments.video.medium_breakpoint, 780.0);
        assert_eq!(theme.attachments.video.play_overlay_size, 64.0);
        assert_eq!(theme.attachments.video.header_filename_max_width, 420.0);
        // Rooms (audit §3.6)
        assert_eq!(theme.rooms.catalogue_row_height, 52.0);
        assert_eq!(theme.rooms.overscan, 800.0);
        assert_eq!(theme.rooms.banner_width, 200.0);
        assert_eq!(theme.rooms.progress_length, 80.0);
        assert_eq!(theme.rooms.progress_girth, 6.0);
        // Tunnels (audit §3.8)
        assert_eq!(theme.tunnels.chip_padding_x, 6.0);
        assert_eq!(theme.tunnels.chip_padding_y, 2.0);
        // Dialogs (audit §3.9)
        assert_eq!(theme.dialogs.avatar_size, 72.0);
        assert_eq!(theme.dialogs.avatar_glyph_size, 48.0);
        assert_eq!(theme.dialogs.title_size, 22.0);
        assert_eq!(theme.dialogs.body_size, 15.0);
        assert_eq!(theme.dialogs.padding, 32.0);
        assert_eq!(theme.dialogs.spacing, 12.0);
        assert_eq!(theme.dialogs.control_padding_x, 14.0);
        assert_eq!(theme.dialogs.control_padding_y, 6.0);
        // Calls (audit §3.10)
        assert_eq!(theme.calls.avatar_size, 96.0);
        assert_eq!(theme.calls.avatar_glyph_size, 36.0);
        assert_eq!(theme.calls.avatar_glyph_size_large, 44.0);
        assert_eq!(theme.calls.pip_w, 220.0);
        assert_eq!(theme.calls.pip_h, 150.0);
        assert_eq!(theme.calls.controls_gap, 40.0);
        // Status card (audit §3.2)
        assert_eq!(theme.home.status_card_text_min_width_medium, 260.0);
        assert_eq!(theme.home.status_card_mesh_max_width, 170.0);
        assert_eq!(theme.home.status_icon_text_gap_full, 24.0);
        assert_eq!(theme.home.status_icon_text_gap_medium, 20.0);
        assert_eq!(theme.home.status_divider_width, 44.0);
        assert_eq!(theme.home.status_divider_height, 3.0);
        // Typography extras (audit §3.1/§3.11)
        assert_eq!(theme.typography.sidebar_name, 15.0);
        assert_eq!(theme.typography.section_label, 11.0);
        assert_eq!(theme.typography.badge, 10.0);
        // Controls (audit §3.12)
        assert_eq!(theme.controls.header_height, 52.0);
        assert_eq!(theme.controls.slider_width, 160.0);
        // Area-10 pinned raw literals (audit §3.9/§3.10/§3.12)
        assert_eq!(theme.typography.call_name_active, 26.0);
        assert_eq!(
            theme.colors.settings_heading_text,
            iced::Color::from_rgb(0.15, 0.15, 0.15)
        );
        assert_eq!(
            BoruTheme::dark().colors.settings_heading_text,
            iced::Color::from_rgb(0.9, 0.9, 0.9)
        );
        assert_eq!(
            theme.colors.expanded_video_backdrop,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.82)
        );
        assert_eq!(
            BoruTheme::dark().colors.expanded_video_backdrop,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.82)
        );
        assert_eq!(
            theme.colors.lightbox_backdrop,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.90)
        );
        assert_eq!(
            BoruTheme::dark().colors.lightbox_backdrop,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.90)
        );
        // Chat overlay backdrops + panel shadow (audit §3.3 raw rgba values)
        assert_eq!(
            theme.colors.chat_overlay_backdrop,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25)
        );
        assert_eq!(
            theme.colors.chat_search_backdrop,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.15)
        );
        assert_eq!(
            theme.colors.panel_shadow,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.30)
        );
        assert_eq!(
            BoruTheme::dark().colors.chat_overlay_backdrop,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.45)
        );
        assert_eq!(
            BoruTheme::dark().colors.chat_search_backdrop,
            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)
        );
    }

    /// BORU-SSUI-08: the screen-share sender UI geometry defaults reuse the
    /// shared Boru spacing / radius / icon-size values so the feature stays
    /// visually consistent with the rest of the app (PDF Task 8: "Prefer
    /// spacing/radius values already used elsewhere in Boru"). Colours are
    /// mode-aware and resolved at style time through `design_tokens` — this
    /// group intentionally holds no fixed light/dark colours (never bake in
    /// white backgrounds or fixed dark text).
    #[test]
    fn screen_share_geometry_matches_design_tokens() {
        let s = ScreenShareTheme::default();
        assert_eq!(s.card.padding, design_tokens::SPACE_16);
        assert_eq!(s.card.radius, design_tokens::RADIUS_LG);
        assert_eq!(s.card.border_width, design_tokens::BORDER_WIDTH);
        assert_eq!(s.card.spacing, design_tokens::SPACE_8);
        assert_eq!(s.card.title_max_chars, 32.0);
        assert_eq!(s.source_card.radius, design_tokens::RADIUS_MD);
        assert_eq!(s.source_card.padding_x, design_tokens::SPACE_10);
        assert_eq!(s.source_card.padding_y, design_tokens::SPACE_8);
        assert_eq!(s.source_card.icon_size, IconSize::Md.px());
        assert_eq!(s.source_card.check_icon_size, IconSize::Sm.px());
        assert_eq!(s.source_card.selected_border_width, 2.0);
        assert_eq!(s.source_card.row_spacing, design_tokens::SPACE_8);
        assert_eq!(s.segmented.radius, design_tokens::RADIUS_MD);
        assert_eq!(s.segmented.spacing, design_tokens::SPACE_4);
        assert_eq!(s.segmented.padding_x, design_tokens::SPACE_10);
        assert_eq!(s.segmented.padding_y, design_tokens::SPACE_4);
        assert_eq!(s.toggle.row_spacing, design_tokens::SPACE_8);
        assert_eq!(s.toggle.icon_size, IconSize::Sm.px());
        assert_eq!(s.action.row_spacing, design_tokens::SPACE_8);
        assert_eq!(s.destructive.padding_x, design_tokens::SPACE_16);
        assert_eq!(s.destructive.padding_y, design_tokens::SPACE_8);
        assert_eq!(s.destructive.radius, design_tokens::RADIUS_MD);
        assert_eq!(s.destructive.icon_gap, design_tokens::SPACE_8);
    }

    /// BORU-SSUI-08: the screen-share geometry is mode-independent (same in
    /// light and dark), exactly like the other geometry groups — only the
    /// mode-aware colour tokens change with the theme.
    #[test]
    fn screen_share_geometry_is_mode_independent() {
        assert_eq!(
            BoruTheme::light().screen_share,
            BoruTheme::dark().screen_share,
            "geometry must not depend on the colour mode"
        );
    }
}
