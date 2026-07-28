//! Central visual tokens and reusable style primitives for the Boru desktop UI.
//!
//! Keep product decisions here so screens compose the same palette, rhythm, and
//! interaction states instead of inventing local literals.
//!
//! See `docs/chat-interface-design-tokens.md` for the full specification.

use iced::widget::{button, container};
use iced::{Background, Color, Theme};

// ── Palette ───────────────────────────────────────────────────────────
// Light values are the product palette. Dark values preserve the existing
// theme while sharing the same semantic roles.
const PRIMARY: Color = Color::from_rgb(
    0x2f as f32 / 255.0,
    0x6b as f32 / 255.0,
    0x4f as f32 / 255.0,
);
const PRIMARY_HOVER: Color = Color::from_rgb(
    0x28 as f32 / 255.0,
    0x5b as f32 / 255.0,
    0x44 as f32 / 255.0,
);
const PRIMARY_PRESSED: Color = Color::from_rgb(
    0x21 as f32 / 255.0,
    0x4c as f32 / 255.0,
    0x39 as f32 / 255.0,
);
const APP_BACKGROUND: Color = Color::from_rgb(
    0xf4 as f32 / 255.0,
    0xf6 as f32 / 255.0,
    0xf4 as f32 / 255.0,
);
const SURFACE: Color = Color::WHITE;
const SURFACE_SECONDARY: Color = Color::from_rgb(
    0xee as f32 / 255.0,
    0xf1 as f32 / 255.0,
    0xee as f32 / 255.0,
);
const SURFACE_HOVER: Color = Color::from_rgb(
    0xe7 as f32 / 255.0,
    0xeb as f32 / 255.0,
    0xe8 as f32 / 255.0,
);
const TEXT: Color = Color::from_rgb(
    0x20 as f32 / 255.0,
    0x25 as f32 / 255.0,
    0x22 as f32 / 255.0,
);
const TEXT_SECONDARY: Color = Color::from_rgb(
    0x6b as f32 / 255.0,
    0x74 as f32 / 255.0,
    0x6e as f32 / 255.0,
);
const TEXT_MUTED: Color = Color::from_rgb(
    0x8a as f32 / 255.0,
    0x92 as f32 / 255.0,
    0x8d as f32 / 255.0,
);
const BORDER: Color = Color::from_rgb(
    0xdd as f32 / 255.0,
    0xe2 as f32 / 255.0,
    0xde as f32 / 255.0,
);
const ONLINE: Color = Color::from_rgb(
    0x28 as f32 / 255.0,
    0xa4 as f32 / 255.0,
    0x5d as f32 / 255.0,
);
const DESTRUCTIVE: Color = Color::from_rgb(
    0xb6 as f32 / 255.0,
    0x41 as f32 / 255.0,
    0x41 as f32 / 255.0,
);

// ── Extended palette (light-only constants) ───────────────────────────
const WARNING: Color = Color::from_rgb(
    0x70 as f32 / 255.0,
    0x45 as f32 / 255.0,
    0x05 as f32 / 255.0, // #B3730D
);
const INPUT_BG: Color = Color::from_rgb(
    0xf0 as f32 / 255.0,
    0xf0 as f32 / 255.0,
    0xf4 as f32 / 255.0, // #F0F0F4
);

// ── Spacing scale ─────────────────────────────────────────────────────
pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_24: f32 = 24.0;
pub const SPACE_32: f32 = 32.0;

// ── Control heights ───────────────────────────────────────────────────
pub const CONTROL_HEIGHT: f32 = 40.0;
pub const CONTROL_HEIGHT_COMPACT: f32 = 36.0;

// ── Corner radii ──────────────────────────────────────────────────────
pub const RADIUS_SM: f32 = 8.0;
pub const RADIUS_MD: f32 = 10.0;
pub const RADIUS_LG: f32 = 12.0;
pub const RADIUS_XL: f32 = 14.0;

// ── Focus ─────────────────────────────────────────────────────────────
pub const FOCUS_WIDTH: f32 = 2.0;

// ── Avatar sizes ──────────────────────────────────────────────────────
pub const AVATAR_SM: f32 = 36.0;
pub const AVATAR_MD: f32 = 48.0;
pub const AVATAR_LG: f32 = 64.0;

// ── Layout ────────────────────────────────────────────────────────────
pub const SIDEBAR_WIDTH: f32 = 300.0;
pub const MESSAGE_MAX_WIDTH: f32 = 480.0;
pub const IMAGE_PREVIEW_MAX_WIDTH: f32 = 360.0;
pub const IMAGE_PREVIEW_MAX_HEIGHT: f32 = 400.0;

// ── Shadow tokens ─────────────────────────────────────────────────────
/// Subtle card shadow — rgba(0,0,0,0.08) offset(0,1) blur(3).
pub fn shadow_card(theme: &Theme) -> iced::Shadow {
    let _ = theme;
    iced::Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.08),
        offset: iced::Vector::new(0.0, 1.0),
        blur_radius: 3.0,
    }
}

/// Dialog / popover shadow — rgba(0,0,0,0.20) offset(0,4) blur(12).
pub fn shadow_dialog(theme: &Theme) -> iced::Shadow {
    let _ = theme;
    iced::Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.20),
        offset: iced::Vector::new(0.0, 4.0),
        blur_radius: 12.0,
    }
}

/// Elevated modal shadow — rgba(0,0,0,0.30) offset(0,4) blur(24).
pub fn shadow_elevated(theme: &Theme) -> iced::Shadow {
    let _ = theme;
    iced::Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.30),
        offset: iced::Vector::new(0.0, 4.0),
        blur_radius: 24.0,
    }
}

// ── Theme helpers ─────────────────────────────────────────────────────
fn dark(theme: &Theme) -> bool {
    matches!(theme, Theme::Dark)
}

pub fn primary(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.29, 0.62, 1.0)
    } else {
        PRIMARY
    }
}
pub fn primary_hover(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.36, 0.70, 1.0)
    } else {
        PRIMARY_HOVER
    }
}
pub fn primary_pressed(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.24, 0.52, 0.86)
    } else {
        PRIMARY_PRESSED
    }
}
pub fn app_background(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.10, 0.10, 0.18)
    } else {
        APP_BACKGROUND
    }
}
pub fn surface(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.16, 0.16, 0.24)
    } else {
        SURFACE
    }
}
pub fn surface_secondary(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.13, 0.13, 0.22)
    } else {
        SURFACE_SECONDARY
    }
}
pub fn surface_hover(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.20, 0.20, 0.30)
    } else {
        SURFACE_HOVER
    }
}
pub fn selected_surface(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.16, 0.23, 0.34)
    } else {
        Color::from_rgb(0.88, 0.93, 0.98)
    }
}
pub fn text(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.80, 0.80, 0.80)
    } else {
        TEXT
    }
}
pub fn text_secondary(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.60, 0.60, 0.60)
    } else {
        TEXT_SECONDARY
    }
}
pub fn text_muted(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.60, 0.60, 0.60)
    } else {
        TEXT_MUTED
    }
}
pub fn border(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.22, 0.22, 0.32)
    } else {
        BORDER
    }
}
pub fn online(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.24, 0.86, 0.52)
    } else {
        ONLINE
    }
}
pub fn destructive(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.90, 0.25, 0.25)
    } else {
        DESTRUCTIVE
    }
}

// ── Extended theme-aware colors ───────────────────────────────────────

/// Warning / amber colour for reconnecting states.
pub fn color_warning(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.95, 0.65, 0.15) // #f2a626
    } else {
        WARNING
    }
}

/// Input field background.
pub fn bg_input(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.13, 0.13, 0.22) // #222238
    } else {
        INPUT_BG
    }
}

/// Color for local (self) message label.
pub fn text_local_label(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.2, 0.8, 0.2)
    } else {
        Color::from_rgb(0.0, 0.45, 0.0) // #0073, ~5.8:1 ✓ AA
    }
}

/// Color for local message body text.
pub fn text_local_body(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.3, 0.9, 0.3)
    } else {
        Color::from_rgb(0.0, 0.35, 0.0) // #0059, ~6.5:1 ✓ AA
    }
}

/// Color for remote message label (nickname).
pub fn text_remote_label(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.4, 0.65, 1.0) // light blue on dark
    } else {
        Color::from_rgb(0.0, 0.33, 0.66) // #0054A8, ~5.5:1 ✓ AA
    }
}

/// Color for remote message body text.
pub fn text_remote_body(theme: &Theme) -> Color {
    if dark(theme) {
        Color::from_rgb(0.8, 0.8, 0.8)
    } else {
        Color::from_rgb(0.13, 0.13, 0.13) // #222, ~11.5:1 ✓ AA
    }
}

/// Background tint for message bubbles. System messages get no bubble.
pub fn bubble_bg(theme: &Theme, is_local: bool, is_system: bool) -> Option<Background> {
    if is_system {
        return None;
    }
    let (r, g, b, a) = match (theme, is_local) {
        (Theme::Dark, true) => (0.15, 0.3, 0.15, 0.4),
        (Theme::Dark, false) => (0.2, 0.2, 0.25, 0.4),
        (_, true) => (0.0, 0.5, 0.0, 0.06),
        (_, false) => (0.1, 0.2, 0.5, 0.05),
    };
    Some(Background::Color(Color::from_rgba(r, g, b, a)))
}

// ── Style helpers ─────────────────────────────────────────────────────

pub fn focus(theme: &Theme) -> iced::Border {
    iced::Border {
        color: primary(theme),
        width: FOCUS_WIDTH,
        radius: RADIUS_SM.into(),
    }
}

pub fn surface_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface(theme))),
        border: iced::Border {
            color: border(theme),
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
        ..Default::default()
    }
}

/// Card container style — surface bg, subtle border, rounded corners,
/// light drop shadow.
pub fn card_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface(theme))),
        border: iced::Border {
            color: border(theme),
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
        shadow: shadow_card(theme),
        ..Default::default()
    }
}

/// Elevated container style — for dialogs, popovers, modals.
pub fn elevated_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface(theme))),
        border: iced::Border {
            color: border(theme),
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        shadow: shadow_elevated(theme),
        ..Default::default()
    }
}

pub fn icon_button(theme: &Theme, status: button::Status) -> button::Style {
    let color = match status {
        button::Status::Hovered => primary(theme),
        button::Status::Pressed => primary_pressed(theme),
        button::Status::Disabled => text_muted(theme),
        _ => text_secondary(theme),
    };
    button::Style {
        background: match status {
            button::Status::Hovered => Some(Background::Color(surface_hover(theme))),
            button::Status::Pressed => Some(Background::Color(selected_surface(theme))),
            _ => None,
        },
        text_color: color,
        border: iced::Border {
            radius: RADIUS_SM.into(),
            color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                primary(theme)
            } else {
                iced::Color::TRANSPARENT
            },
            width: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                1.0
            } else {
                0.0
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_palette_matches_product_spec() {
        let theme = Theme::Light;
        assert_eq!(primary(&theme), PRIMARY);
        assert_eq!(primary_hover(&theme), PRIMARY_HOVER);
        assert_eq!(primary_pressed(&theme), PRIMARY_PRESSED);
        assert_eq!(app_background(&theme), APP_BACKGROUND);
        assert_eq!(surface(&theme), SURFACE);
        assert_eq!(surface_secondary(&theme), SURFACE_SECONDARY);
        assert_eq!(surface_hover(&theme), SURFACE_HOVER);
        assert_eq!(selected_surface(&theme), Color::from_rgb(0.88, 0.93, 0.98));
        assert_eq!(text(&theme), TEXT);
        assert_eq!(text_secondary(&theme), TEXT_SECONDARY);
        assert_eq!(text_muted(&theme), TEXT_MUTED);
        assert_eq!(border(&theme), BORDER);
        assert_eq!(online(&theme), ONLINE);
        assert_eq!(destructive(&theme), DESTRUCTIVE);
    }

    #[test]
    fn interaction_tokens_have_consistent_geometry() {
        assert_eq!(
            (SPACE_4, SPACE_8, SPACE_12, SPACE_16, SPACE_24, SPACE_32),
            (4.0, 8.0, 12.0, 16.0, 24.0, 32.0)
        );
        assert_eq!(
            (RADIUS_SM, RADIUS_MD, RADIUS_LG, RADIUS_XL),
            (8.0, 10.0, 12.0, 14.0)
        );
        assert!((36.0..=40.0).contains(&CONTROL_HEIGHT));
        assert!((36.0..=40.0).contains(&CONTROL_HEIGHT_COMPACT));
    }

    #[test]
    fn extended_palette_tokens() {
        let light = Theme::Light;
        let dark = Theme::Dark;
        // Warning
        assert_eq!(color_warning(&light), WARNING);
        assert_eq!(
            color_warning(&dark),
            Color::from_rgb(0.95, 0.65, 0.15)
        );
        // Input bg
        assert_eq!(bg_input(&light), INPUT_BG);
        // Message text colours exist and differ per theme
        assert_ne!(text_local_label(&light), text_remote_label(&light));
        assert_ne!(text_local_body(&light), text_remote_body(&light));
        // Bubble bg — system returns None
        assert!(bubble_bg(&light, true, true).is_none());
        assert!(bubble_bg(&light, false, true).is_none());
        // Local and remote bubbles exist
        assert!(bubble_bg(&light, true, false).is_some());
        assert!(bubble_bg(&light, false, false).is_some());
    }

    #[test]
    fn layout_tokens_are_reasonable() {
        assert_eq!(SIDEBAR_WIDTH, 300.0);
        assert_eq!(AVATAR_LG, 64.0);
        assert!(MESSAGE_MAX_WIDTH > 0.0);
        assert!(IMAGE_PREVIEW_MAX_WIDTH > 0.0);
        assert!(IMAGE_PREVIEW_MAX_HEIGHT > 0.0);
    }

    #[test]
    fn shadow_tokens_are_valid() {
        let theme = Theme::Light;
        let card = shadow_card(&theme);
        assert!(card.blur_radius > 0.0);
        let dialog = shadow_dialog(&theme);
        assert!(dialog.blur_radius > card.blur_radius);
        let elevated = shadow_elevated(&theme);
        assert!(elevated.blur_radius >= dialog.blur_radius);
    }
}
