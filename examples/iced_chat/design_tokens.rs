//! Central visual tokens and reusable style primitives for the Boru desktop UI.
//!
//! Keep product decisions here so screens compose the same palette, rhythm, and
//! interaction states instead of inventing local literals.

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

pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_24: f32 = 24.0;
pub const SPACE_32: f32 = 32.0;
pub const CONTROL_HEIGHT: f32 = 40.0;
pub const CONTROL_HEIGHT_COMPACT: f32 = 36.0;
pub const RADIUS_SM: f32 = 8.0;
pub const RADIUS_MD: f32 = 10.0;
pub const RADIUS_LG: f32 = 12.0;
pub const RADIUS_XL: f32 = 14.0;
pub const FOCUS_WIDTH: f32 = 2.0;
pub const AVATAR_SM: f32 = 36.0;
pub const AVATAR_MD: f32 = 48.0;

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
}
