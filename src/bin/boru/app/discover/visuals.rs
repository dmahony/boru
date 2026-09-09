//! Public Rooms-only visual adapters. No asset loading or network access.
//!
//! Existing primary buttons assume white ink; keep that global contract (and
//! sidebar) untouched. Resolve ink against the actual composited fill here.
use super::{AppMessage, DiscoverPalette};
use crate::design_tokens::{self, AVATAR_MD, RADIUS_MD};
use iced::widget::{button, container};
use iced::{Background, Color};

fn over(foreground: Color, background: Color) -> Color {
    let a = foreground.a;
    Color::from_rgb(
        foreground.r * a + background.r * (1.0 - a),
        foreground.g * a + background.g * (1.0 - a),
        foreground.b * a + background.b * (1.0 - a),
    )
}

fn luminance(c: Color) -> f32 {
    let [r, g, b] = [c.r, c.g, c.b].map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Keep theme ink when accessible; otherwise select the higher-contrast neutral.
fn readable_ink(preferred: Color, background: Color) -> Color {
    let preferred = over(preferred, background);
    if contrast(preferred, background) >= 4.5 {
        preferred
    } else if contrast(Color::BLACK, background) >= contrast(Color::WHITE, background) {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

impl DiscoverPalette {
    pub(crate) fn button_style(self, selected: bool, status: button::Status) -> button::Style {
        let disabled = status == button::Status::Disabled;
        let fill = match (selected && !disabled, status) {
            (true, button::Status::Hovered) => self.primary_hover,
            (true, button::Status::Pressed) => self.primary_pressed,
            (true, _) => self.primary,
            (false, button::Status::Hovered) => self.surface_hover,
            (false, button::Status::Pressed) => self.surface_pressed,
            _ => self.surface,
        };
        let background = over(fill.color(), self.surface.color());
        button::Style {
            background: Some(Background::Color(background)),
            text_color: readable_ink(
                if disabled {
                    self.muted.color()
                } else {
                    self.text.color()
                },
                background,
            ),
            border: iced::Border {
                color: self.border.color(),
                width: f32::from_bits(self.hairline_bits),
                radius: RADIUS_MD.into(),
            },
            ..Default::default()
        }
    }

    pub(crate) fn card_style(self, theme: &iced::Theme) -> container::Style {
        let mut style = design_tokens::card_style(theme);
        style.background = Some(self.surface.color().into());
        style.text_color = Some(readable_ink(self.text.color(), self.surface.color()));
        style.border.color = self.border.color();
        style.border.width = f32::from_bits(self.hairline_bits);
        style
    }

    /// Quiet decoration without assuming any unapproved illustration license.
    /// For decorative areas only: foreground copy belongs on the solid surface.
    pub(crate) fn artwork_background(self) -> Background {
        let mut tint = self.primary.color();
        tint.a *= 0.08;
        iced::gradient::Linear::new(iced::Radians(std::f32::consts::FRAC_PI_4))
            .add_stop(0.0, over(tint, self.surface.color()))
            .add_stop(1.0, self.surface.color())
            .into()
    }
}

/// Directory rows have no approved image handle. Use the existing bundled room
/// icon, never interpret advertised strings as paths/URLs or fetch an avatar.
pub(super) fn room_artwork(palette: DiscoverPalette) -> iced::Element<'static, AppMessage> {
    use crate::icon_system::{Icon, IconSize};
    container(
        Icon::Users
            .build()
            .size(IconSize::Sm)
            .build()
            .style(move |_, _| iced::widget::svg::Style {
                color: Some(readable_ink(palette.text.color(), palette.surface.color())),
            }),
    )
    .center_x(AVATAR_MD)
    .center_y(AVATAR_MD)
    .style(move |theme| {
        let mut style = palette.card_style(theme);
        style.background = Some(palette.artwork_background());
        style
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_visuals_accent_ink_passes_aa_in_all_button_states() {
        for theme in [iced::Theme::Light, iced::Theme::Dark] {
            for accent in [
                Color::WHITE,
                Color::BLACK,
                Color::from_rgb(0.9, 0.95, 0.7),
                Color::from_rgba(0.1, 0.2, 0.3, 0.2),
            ] {
                let mut palette: DiscoverPalette =
                    crate::theme::BoruTheme::for_theme(&theme).into();
                palette.primary = accent.into();
                palette.primary_hover = accent.into();
                palette.primary_pressed = accent.into();
                for selected in [false, true] {
                    for status in [
                        button::Status::Active,
                        button::Status::Hovered,
                        button::Status::Pressed,
                        button::Status::Disabled,
                    ] {
                        let style = palette.button_style(selected, status);
                        let Some(Background::Color(bg)) = style.background else {
                            panic!("solid fill required")
                        };
                        assert!(contrast(style.text_color, bg) >= 4.5);
                    }
                }
            }
        }
    }

    #[test]
    fn discover_visuals_live_accent_changes_fill_and_cache_key() {
        let a: DiscoverPalette = crate::theme::BoruTheme::for_theme(&iced::Theme::Light).into();
        let mut b = a;
        b.primary = Color::WHITE.into();
        assert_ne!(a, b);
        assert_ne!(
            a.button_style(true, button::Status::Active).background,
            b.button_style(true, button::Status::Active).background
        );
        assert_ne!(a.artwork_background(), b.artwork_background());
        assert_eq!(
            a.card_style(&iced::Theme::Light).border.radius,
            design_tokens::RADIUS_CARD.into()
        );
        let _ = room_artwork(b);
    }
}
