//! Shared presentation primitives for the screen-sharing UI family
//! (BORU-SSUI-12, PDF Task 12).
//!
//! Both the sender card (`chat.rs` `view_screen_share_panel` sender branch)
//! and the viewer toolbar (`screen_share_surface.rs`
//! `view_screen_share_view_controls` + the viewer branch of the panel) now
//! consume these primitives, so the two sides look like ONE feature family
//! instead of two hand-rolled control languages.
//!
//! These are PRESENTATION primitives only. They take already-resolved
//! labels, messages, colors and dots; they hold no session state and they
//! dispatch nothing on their own. Sender-only and viewer-only semantics
//! stay in their own view code — the shared part is the visual language
//! (compact action button, status row, rounded card shell), never a state
//! machine.
//!
//! Components are named by BEHAVIOR (`compact_action_button`,
//! `status_row`, `screen_share_card`) rather than by mockup position, so a
//! future layout can rearrange them without renaming or rewriting them.

use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Color, Element, Length};

use crate::design_tokens;
use crate::icon_system::{Icon, IconSize};
use crate::theme::ScreenShareCardTheme;
use crate::ui_components::{status_dot, StatusDotKind};

// NOTE (BORU-SSUI-12): the segmented control and the destructive action
// already live in the shared-primitives locations identified by the
// sender audit (§3.5) — `ui_components::segmented_control` (extracted in
// SSUI-04/08) and `form_components::destructive_button_icon` (SSUI-07/08).
// Both the sender card and the viewer consume those from their canonical
// homes; this module adds the three primitives that did NOT exist yet:
// the compact action button, the status row, and the rounded card shell.

/// A compact action button — the shared "small button" language of the
/// screen-sharing UI (BORU-SSUI-12).
///
/// Renders an optional leading icon + label inside the compact
/// `padding([2, 6])` geometry used by the viewer toolbar (Fit / 100% / − /
/// + / Reset / Cursor / Fullscreen) and the sender consent actions
/// (grant / deny / revoke / clipboard). When `focus_ring_radius` is
/// `Some(r)` the button is wrapped in the app's `FocusableButton`
/// (keyboard-reachable, visible focus ring); `None` keeps the plain
/// toolbar button. `on_press` is only published on user intent — never
/// during redraws.
pub(crate) fn compact_action_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    icon: Option<Icon>,
    on_press: Option<Message>,
    focus_ring_radius: Option<f32>,
) -> Element<'a, Message> {
    let content: Element<'_, Message> = if let Some(icon) = icon {
        row![
            icon.build()
                .size(IconSize::Sm)
                .color_fn(design_tokens::text_secondary)
                .build(),
            text(label.into())
                .font(crate::fonts::TypeRole::ButtonLabel.font())
                .size(crate::fonts::TypeRole::SupportingText.size_px()),
        ]
        .spacing(design_tokens::SPACE_6)
        .align_y(Alignment::Center)
        .into()
    } else {
        text(label.into())
            .font(crate::fonts::TypeRole::ButtonLabel.font())
            .size(crate::fonts::TypeRole::SupportingText.size_px())
            .into()
    };
    let mut btn = button(content)
        // 32px-class controls: compact enough to sit inline while keeping
        // the label and icon comfortably centered.
        .padding([design_tokens::SPACE_6, design_tokens::SPACE_10])
        .style(compact_action_button_style);
    if let Some(msg) = on_press.clone() {
        btn = btn.on_press(msg);
    }
    match focus_ring_radius {
        Some(radius) => crate::focusable_button::focusable_button(btn, on_press)
            .ring_radius(radius)
            .into(),
        None => btn.into(),
    }
}

/// Neutral screen-share action styling. The default iced button style is a
/// filled accent control, which makes receiver actions visually oversized.
fn compact_action_button_style(theme: &iced::Theme, status: button::Status) -> button::Style {
    let (background, border_color) = match status {
        button::Status::Hovered => (
            design_tokens::surface_hover(theme),
            design_tokens::border_strong(theme),
        ),
        button::Status::Pressed => (
            design_tokens::surface_pressed(theme),
            design_tokens::border_strong(theme),
        ),
        button::Status::Disabled | button::Status::Active => (
            design_tokens::surface(theme),
            design_tokens::border_muted(theme),
        ),
    };
    button::Style {
        background: Some(iced::Background::Color(background)),
        text_color: if matches!(status, button::Status::Disabled) {
            design_tokens::text_muted(theme)
        } else {
            design_tokens::text_primary(theme)
        },
        border: iced::Border {
            color: border_color,
            width: design_tokens::BORDER_WIDTH,
            radius: design_tokens::RADIUS_SM.into(),
        },
        ..Default::default()
    }
}

/// A compact destructive action for screen-share toolbars.
///
/// It keeps the same padding and label geometry as [`compact_action_button`],
/// while making the stop action unmistakable with a stop icon and danger
/// treatment.
pub(crate) fn compact_destructive_action_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let danger = |theme: &iced::Theme| design_tokens::color_danger(theme);
    let content: Element<'_, Message> = row![
        Icon::Stop
            .build()
            .size(IconSize::Sm)
            .color_fn(danger)
            .build(),
        text(label.into()),
    ]
    .spacing(design_tokens::SPACE_6)
    .align_y(Alignment::Center)
    .into();
    let mut btn = button(content)
        .padding([design_tokens::SPACE_2, design_tokens::SPACE_6])
        .style(|theme: &iced::Theme, status| {
            let danger = design_tokens::color_danger(theme);
            let background = match status {
                button::Status::Hovered | button::Status::Pressed => Some(iced::Background::Color(
                    design_tokens::destructive_soft(theme),
                )),
                _ => None,
            };
            button::Style {
                background,
                text_color: danger,
                border: iced::Border {
                    color: danger,
                    width: 1.0,
                    radius: design_tokens::RADIUS_SM.into(),
                },
                ..Default::default()
            }
        });
    if let Some(msg) = on_press {
        btn = btn.on_press(msg);
    }
    btn.into()
}

/// A status chip/row — icon + label + optional state dot (BORU-SSUI-12).
///
/// The sender's remote-control status ("Remote control: ON/OFF" with the
/// mouse-pointer icon and online/offline dot) and the viewer's
/// remote-control line are the same presentation concept; this primitive
/// is how both sides render it. `icon` is `(Icon, color-fn)`; `dot` adds
/// the trailing state dot; `icon_tooltip` wraps the icon in a concise
/// tooltip (the icon alone is ambiguous without the adjacent label).
/// Label styling matches the sender's existing SupportingText treatment.
pub(crate) fn status_row<'a, Message: 'a>(
    icon: Option<(Icon, fn(&iced::Theme) -> iced::Color)>,
    label: String,
    label_color: Color,
    dot: Option<StatusDotKind>,
    icon_tooltip: Option<String>,
) -> Element<'a, Message> {
    let icon_el: Option<Element<'_, Message>> = icon.map(|(icon, color)| {
        let el: Element<'_, Message> = icon
            .build()
            .size(IconSize::Sm)
            .color_fn(color)
            .build()
            .into();
        if let Some(tip) = icon_tooltip {
            iced::widget::tooltip::Tooltip::new(
                el,
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, tip),
                iced::widget::tooltip::Position::Bottom,
            )
            .gap(design_tokens::SPACE_2)
            .into()
        } else {
            el
        }
    });
    let label_el = text(label)
        .size(crate::fonts::TypeRole::SupportingText.size_px())
        .font(crate::fonts::TypeRole::SupportingText.font())
        .color(label_color);
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    if let Some(icon) = icon_el {
        items.push(icon);
    }
    items.push(label_el.into());
    if let Some(kind) = dot {
        items.push(status_dot(kind, 8.0));
    }
    row(items)
        .spacing(design_tokens::SPACE_6)
        .align_y(Alignment::Center)
        .into()
}

/// The rounded toolbar/card shell shared by the sender card and the viewer
/// panel (BORU-SSUI-02 / BORU-SSUI-12).
///
/// Both branches of `view_screen_share_panel` (sender controls and viewer
/// surface) flow through this one shell — a subtle secondary surface,
/// thin neutral border, medium-large radius and a restrained shadow, all
/// driven by the `screen_share.card.*` TOML tokens (hot-reloadable). Color
/// values stay mode-aware via `design_tokens` (never baked-in light/dark).
pub(crate) fn screen_share_card<'a, Message: 'a>(
    body: Element<'a, Message>,
    card_theme: ScreenShareCardTheme,
    height: Length,
) -> Element<'a, Message> {
    container(body)
        .padding(card_theme.padding)
        .width(Length::Fill)
        .height(height)
        .style(move |t| iced::widget::container::Style {
            background: Some(iced::Background::Color(super::bg_surface_secondary(t))),
            border: iced::Border {
                color: design_tokens::border_muted(t),
                width: card_theme.border_width,
                radius: card_theme.radius.into(),
            },
            shadow: design_tokens::shadow_card(t),
            ..Default::default()
        })
        .into()
}

/// Compose the receiver's dedicated header, viewport, and toolbar regions.
///
/// The caller supplies already-resolved presentation elements; this helper
/// owns no media or session state. Sender controls continue to use the shared
/// `screen_share_card` shell directly.
pub(crate) fn receiver_screen_share_card<'a, Message: 'a>(
    header: Element<'a, Message>,
    viewport: Element<'a, Message>,
    toolbar: Element<'a, Message>,
    card_theme: ScreenShareCardTheme,
) -> Element<'a, Message> {
    let region_spacing = card_theme.spacing;
    screen_share_card(
        column![
            container(header).width(Length::Fill),
            container(viewport).width(Length::Fill),
            container(toolbar).width(Length::Fill),
        ]
        .spacing(region_spacing)
        .into(),
        card_theme,
        Length::Fill,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppMessage;

    #[test]
    fn compact_action_button_builds_plain_and_focusable() {
        // Plain viewer-toolbar style (no focus ring) — must build.
        let plain: Element<'static, AppMessage> =
            compact_action_button(crate::i18n::t("screenshare.fit"), None, None, None);
        let _ = plain;
        // Sender consent style (focusable with ring radius) — must build.
        let focusable: Element<'static, AppMessage> = compact_action_button(
            crate::i18n::t("screenshare.grant_pointer"),
            None,
            Some(AppMessage::Noop),
            Some(design_tokens::RADIUS_SM),
        );
        let _ = focusable;
    }

    #[test]
    fn compact_action_button_supports_leading_icon() {
        let el: Element<'static, AppMessage> = compact_action_button(
            "Fit".to_string(),
            Some(Icon::MousePointer),
            Some(AppMessage::Noop),
            None,
        );
        let _ = el;
    }

    #[test]
    fn compact_action_button_uses_neutral_theme_aware_style() {
        let light = compact_action_button_style(&iced::Theme::Light, button::Status::Active);
        let dark = compact_action_button_style(&iced::Theme::Dark, button::Status::Active);
        assert_eq!(
            light.background,
            Some(iced::Background::Color(design_tokens::surface(
                &iced::Theme::Light
            )))
        );
        assert_eq!(
            dark.background,
            Some(iced::Background::Color(design_tokens::surface(
                &iced::Theme::Dark
            )))
        );
        assert_eq!(light.border.radius, design_tokens::RADIUS_SM.into());
        assert_eq!(dark.border.radius, design_tokens::RADIUS_SM.into());
    }

    #[test]
    fn compact_destructive_action_button_builds() {
        let el: Element<'static, AppMessage> = compact_destructive_action_button(
            crate::i18n::t("screenshare.stop_viewing"),
            Some(AppMessage::StopScreenShare),
        );
        let _ = el;
    }

    #[test]
    fn status_row_builds_plain_and_with_icon_dot() {
        // Viewer-style plain status line.
        let plain: Element<'static, AppMessage> = status_row(
            None,
            "Remote control: OFF".to_string(),
            Color::BLACK,
            None,
            None,
        );
        let _ = plain;
        // Sender-style status row with icon + dot + tooltip.
        let full: Element<'static, AppMessage> = status_row(
            Some((Icon::MousePointer, design_tokens::primary)),
            "Remote control: ON".to_string(),
            Color::BLACK,
            Some(StatusDotKind::Online),
            Some("Remote control: ON".to_string()),
        );
        let _ = full;
    }

    #[test]
    fn screen_share_card_builds_with_theme() {
        let body: Element<'static, AppMessage> = text("body").into();
        // `ScreenShareCardTheme` has no `Default`; the canonical default
        // lives in `ScreenShareTheme::default().card`.
        let card_theme = crate::theme::ScreenShareTheme::default().card;
        let el: Element<'static, AppMessage> = screen_share_card(body, card_theme, Length::Shrink);
        let _ = el;
    }

    #[test]
    fn shared_primitives_resolve_from_canonical_locations() {
        // The segmented control and destructive action stay in their
        // shared-primitives homes (ui_components / form_components); the
        // sender card and viewer both consume those same functions. Smoke
        // that they still build as shared primitives.
        let segments: Vec<crate::ui_components::SegmentedOption<AppMessage>> =
            vec![crate::ui_components::SegmentedOption {
                label: "x".to_string(),
                selected: true,
                enabled: true,
                on_press: Some(AppMessage::Noop),
                tooltip: None,
            }];
        let el: Element<'static, AppMessage> = crate::ui_components::segmented_control(
            segments,
            crate::ui_components::SegmentedControlStyle::default(),
        );
        let _ = el;
        let destructive: Element<'static, AppMessage> =
            crate::form_components::destructive_button_icon(
                Icon::Stop,
                "Stop Sharing".to_string(),
                Some(AppMessage::Noop),
                false,
                crate::form_components::DestructiveButtonStyle::default(),
            );
        let _ = destructive;
    }
}
