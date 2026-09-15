//! Home-scoped visual primitives and component contracts.
//!
//! This module is deliberately separate from the global widget theme. Home
//! views receive a copy of the already-merged [`BoruTheme`] from
//! `IcedChat::boru_theme()`, so settings/dev-file overrides (including the
//! accent) are reflected without changing protected screens or Iced defaults.
//!
//! Card tasks should use [`HomeVisualFoundation`] for visual decisions and
//! retain their existing data projections and `AppMessage` callbacks.

use iced::widget::{button, container, text};
use iced::{Background, Border, Color, Theme};

use crate::{design_tokens, theme::BoruTheme};

/// Semantic text roles available to Home components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeTextRole {
    Primary,
    Secondary,
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
}

/// Surface tone for an interactive Home action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeActionTone {
    Primary,
    Secondary,
    Destructive,
}

/// Theme-aware, Home-local visual foundation.
///
/// The value is a frame snapshot. Construct it from `IcedChat::boru_theme()`
/// in the Home dependency/view path, rather than from `Theme::Light` or a
/// hard-coded accent. The snapshot is `Copy`, which keeps iced style closures
/// independent from `IcedChat` borrows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HomeVisualFoundation {
    theme: BoruTheme,
}

impl HomeVisualFoundation {
    /// Build a Home foundation from the live merged application theme.
    pub const fn new(theme: BoruTheme) -> Self {
        Self { theme }
    }

    /// Access the immutable token snapshot for component-specific geometry.
    pub const fn theme(self) -> BoruTheme {
        self.theme
    }

    /// Home spacing/radius tokens, kept as a stable component contract.
    pub const fn spacing(self) -> crate::theme::SpacingTokens {
        self.theme.spacing
    }

    /// Home corner-radius tokens, kept as a stable component contract.
    pub const fn radii(self) -> crate::theme::RadiusTokens {
        self.theme.radii
    }

    /// Return a semantic text colour without introducing local literals.
    pub const fn text_color(self, role: HomeTextRole) -> Color {
        match role {
            HomeTextRole::Primary => self.theme.colors.text_primary,
            HomeTextRole::Secondary => self.theme.colors.text_secondary,
            HomeTextRole::Muted => self.theme.colors.text_muted,
            HomeTextRole::Accent => self.theme.colors.primary,
            HomeTextRole::Success => self.theme.colors.success,
            HomeTextRole::Warning => self.theme.colors.warning,
            HomeTextRole::Danger => self.theme.colors.danger,
        }
    }

    /// Style for ordinary Home cards. This does not alter global containers.
    pub fn card_style(self) -> container::Style {
        container::Style {
            background: Some(Background::Color(self.theme.colors.surface)),
            border: Border {
                color: self.theme.colors.border_muted,
                width: self.theme.borders.hairline,
                radius: self.theme.radii.card.into(),
            },
            shadow: design_tokens::shadow_card(&Theme::Light),
            ..Default::default()
        }
    }

    /// Style for a Home row, with an optional selected state.
    pub fn row_style(self, selected: bool) -> container::Style {
        container::Style {
            background: Some(Background::Color(if selected {
                self.theme.colors.surface_selected
            } else {
                self.theme.colors.surface
            })),
            border: Border {
                color: if selected {
                    self.theme.colors.primary
                } else {
                    Color::TRANSPARENT
                },
                width: if selected {
                    self.theme.borders.selected_row
                } else {
                    0.0
                },
                radius: self.theme.radii.md.into(),
            },
            ..Default::default()
        }
    }

    /// Style for a Home action, preserving semantic destructive colouring.
    pub fn action_style(self, tone: HomeActionTone, status: button::Status) -> button::Style {
        let (normal, hover, pressed) = match tone {
            HomeActionTone::Primary => (
                self.theme.colors.primary,
                self.theme.colors.primary_hover,
                self.theme.colors.primary_pressed,
            ),
            HomeActionTone::Secondary => (
                self.theme.colors.text_secondary,
                self.theme.colors.text_primary,
                self.theme.colors.text_primary,
            ),
            HomeActionTone::Destructive => (
                self.theme.colors.danger,
                self.theme.colors.danger,
                self.theme.colors.danger,
            ),
        };
        let (text_color, background) = match status {
            button::Status::Hovered => (
                hover,
                Some(Background::Color(self.theme.colors.surface_hover)),
            ),
            button::Status::Pressed => (
                pressed,
                Some(Background::Color(self.theme.colors.surface_pressed)),
            ),
            button::Status::Disabled => (self.theme.colors.text_muted, None),
            button::Status::Active => (normal, None),
        };
        button::Style {
            background,
            text_color,
            border: Border {
                color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    self.theme.colors.primary
                } else {
                    Color::TRANSPARENT
                },
                width: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                    self.theme.borders.hairline
                } else {
                    0.0
                },
                radius: self.theme.radii.md.into(),
            },
            ..Default::default()
        }
    }

    /// Style for a meaningful keyboard focus outline (Home only).
    pub fn focus_border(self) -> Border {
        Border {
            color: self.theme.colors.focus,
            width: self.theme.borders.focus,
            radius: self.theme.radii.md.into(),
        }
    }

    /// Style callback for text widgets using the live Home palette.
    pub fn text_style(self, role: HomeTextRole) -> impl Fn(&Theme) -> text::Style + 'static {
        let color = self.text_color(role);
        move |_| text::Style { color: Some(color) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foundation_uses_live_accent_override() {
        let mut theme = BoruTheme::default();
        theme.colors.primary = Color::from_rgb(0.8, 0.1, 0.2);
        let foundation = HomeVisualFoundation::new(theme);
        assert_eq!(
            foundation.text_color(HomeTextRole::Accent),
            theme.colors.primary
        );
        assert_eq!(
            foundation
                .action_style(HomeActionTone::Primary, button::Status::Active)
                .text_color,
            theme.colors.primary
        );
    }

    #[test]
    fn light_and_dark_foundations_keep_semantic_roles_distinct() {
        let light = HomeVisualFoundation::new(BoruTheme::light());
        let dark = HomeVisualFoundation::new(BoruTheme::dark());
        assert_ne!(
            light.text_color(HomeTextRole::Primary),
            dark.text_color(HomeTextRole::Primary)
        );
        assert_eq!(
            light.text_color(HomeTextRole::Danger),
            light.theme().colors.danger
        );
        assert_eq!(
            dark.text_color(HomeTextRole::Success),
            dark.theme().colors.success
        );
    }

    #[test]
    fn focus_outline_has_meaningful_width_and_theme_colour() {
        let foundation = HomeVisualFoundation::new(BoruTheme::default());
        let border = foundation.focus_border();
        assert!(border.width >= 2.0);
        assert_eq!(border.color, foundation.theme().colors.focus);
    }
}
