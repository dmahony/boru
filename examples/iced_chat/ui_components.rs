//! Reusable UI primitives for the Boru desktop GUI.
//!
//! Every component is a pure function or builder struct — it accepts content
//! and state parameters and never reads global application state directly.
//! All styling uses the centralized tokens from `design_tokens`, icons from
//! `icon_system`, and typography from `fonts`.
//!
//! ## Component catalogue
//!
//! | Component          | Builder / fn              | States                           |
//! |--------------------|---------------------------|----------------------------------|
//! | Card               | `card(…)`                 | default, hover (if clickable)    |
//! | Elevated card      | `elevated_card(…)`        | default                          |
//! | Icon tile          | `icon_tile(…)`            | default                          |
//! | Primary button     | `primary_button(…)`       | default, hover, pressed, disabled|
//! | Secondary button   | `secondary_button(…)`     | default, hover, pressed, disabled|
//! | Ghost icon button  | `ghost_icon_button(…)`    | default, hover, pressed, disabled|
//! | Text input         | `text_input_field(…)`     | default, focused, error          |
//! | Status dot         | `status_dot(…)`           | online / offline / warning       |
//! | Badge / pill       | `badge(…)`                | default, accent, danger, muted   |
//! | Divider            | `divider()`               | default                          |
//! | List row           | `list_row(…)`             | default, selected, hover         |
//! | Empty state        | `empty_state(…)`          | default                          |
//! | Avatar             | `Avatar::new(…)`          | default, online dot, unread badge|
//! | Section header     | `section_header(…)`       | default                          |
//! | Tooltip            | `tooltip(…)`              | default                          |
//! | Card header        | `card_header(…)`          | default                          |

use iced::widget::{
    button, container, rule, svg, text, text_input, tooltip as iced_tooltip, Column, Row, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Pixels, Theme};

use crate::app::AppMessage;
use crate::design_tokens;
use crate::fonts::Typography;
use crate::icon_system::{self, Icon, IconSize};
use crate::presentation;

// ── Helper: fn pointer for Icon::color_fn ─────────────────────────────

fn white_color(_theme: &Theme) -> Color {
    Color::WHITE
}

// ═══════════════════════════════════════════════════════════════════════
// 1. CARD — surface background, subtle shadow, rounded corners
// ═══════════════════════════════════════════════════════════════════════

/// Builder for a standard card container.
pub struct Card<'a, Message> {
    children: Vec<Element<'a, Message>>,
    padding_v: f32,
    padding_h: f32,
    spacing: f32,
    width: Length,
    on_press: Option<Message>,
}

impl<'a, Message: Clone + 'a> Card<'a, Message> {
    /// Start a card with the given content.
    pub fn new(children: Vec<Element<'a, Message>>) -> Self {
        Self {
            children,
            padding_v: design_tokens::SPACE_16,
            padding_h: design_tokens::SPACE_16,
            spacing: design_tokens::SPACE_8,
            width: Length::Fill,
            on_press: None,
        }
    }

    /// Override the default padding.
    pub fn padding(mut self, vertical: f32, horizontal: f32) -> Self {
        self.padding_v = vertical;
        self.padding_h = horizontal;
        self
    }

    /// Override spacing between children.
    pub fn spacing(mut self, s: f32) -> Self {
        self.spacing = s;
        self
    }

    /// Override width.
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Make the card clickable with the given message on press.
    pub fn on_press(mut self, msg: Message) -> Self {
        self.on_press = Some(msg);
        self
    }

    /// Build the card element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        let _ = theme; // used by style closures below
        let content: Element<'_, Message> = if self.children.is_empty() {
            Space::new()
                .width(Length::Fill)
                .height(Length::Shrink)
                .into()
        } else if self.children.len() == 1 {
            self.children.into_iter().next().unwrap()
        } else {
            Column::with_children(self.children)
                .spacing(self.spacing)
                .into()
        };

        let inner = container(content)
            .padding(Padding {
                top: self.padding_v,
                right: self.padding_h,
                bottom: self.padding_v,
                left: self.padding_h,
            })
            .width(self.width)
            .style(|t| design_tokens::card_style(t));

        if let Some(msg) = self.on_press {
            button(inner)
                .on_press(msg)
                .padding(0)
                .style(card_button_style)
                .into()
        } else {
            inner.into()
        }
    }
}

/// Button style that looks like a card — surface bg with hover tint.
fn card_button_style(theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => design_tokens::surface_hover(theme),
        button::Status::Pressed => {
            let mut c = design_tokens::surface_hover(theme);
            c.r *= 0.92;
            c.g *= 0.92;
            c.b *= 0.92;
            c
        }
        _ => design_tokens::surface(theme),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: design_tokens::text_primary(theme),
        border: Border {
            color: match status {
                button::Status::Hovered | button::Status::Pressed => design_tokens::primary(theme),
                _ => design_tokens::border_muted(theme),
            },
            width: design_tokens::BORDER_WIDTH,
            radius: design_tokens::RADIUS_LG.into(),
        },
        shadow: match status {
            button::Status::Hovered => design_tokens::shadow_card(theme),
            _ => iced::Shadow::default(),
        },
        ..Default::default()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 2. ELEVATED CARD — for dialogs, popovers, modals
// ═══════════════════════════════════════════════════════════════════════

/// A card with a higher shadow — suitable for dialogs, popovers, and modals.
pub fn elevated_card<'a, Message: 'a>(children: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    container(Column::with_children(children).spacing(design_tokens::SPACE_8))
        .padding(design_tokens::SPACE_16)
        .width(Length::Fill)
        .style(|t| design_tokens::elevated_style(t))
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 3. ICON TILE — square container with a centred icon
// ═══════════════════════════════════════════════════════════════════════

/// A square tile containing a centred icon, used in home cards and quick actions.
pub fn icon_tile<'a, Message: 'a>(
    icon: Icon,
    size: IconSize,
    bg_color: Option<fn(&Theme) -> Color>,
) -> Element<'a, Message> {
    let px = size.px();
    let tile_size = px + design_tokens::SPACE_16;
    let svg_el = icon.build().size(size).build();

    container(svg_el)
        .width(Length::Fixed(tile_size))
        .height(Length::Fixed(tile_size))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |t| container::Style {
            background: Some(Background::Color(bg_color
                .unwrap_or(design_tokens::primary_soft)(
                t
            ))),
            border: Border {
                radius: design_tokens::RADIUS_MD.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 4. BUTTON STYLES — reusable style functions
// ═══════════════════════════════════════════════════════════════════════

/// Filled primary button style — accent background, white text.
pub fn button_primary_style(theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => design_tokens::primary_hover(theme),
        button::Status::Pressed => design_tokens::primary_pressed(theme),
        button::Status::Disabled => design_tokens::text_muted(theme),
        _ => design_tokens::primary(theme),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            radius: design_tokens::RADIUS_MD.into(),
            ..Default::default()
        },
        shadow: match status {
            button::Status::Hovered => design_tokens::shadow_card(theme),
            _ => iced::Shadow::default(),
        },
        ..Default::default()
    }
}

/// Outline / secondary button style — transparent bg, muted border, accent on hover.
pub fn button_secondary_style(theme: &Theme, status: button::Status) -> button::Style {
    let border_color = match status {
        button::Status::Hovered | button::Status::Pressed => design_tokens::primary(theme),
        button::Status::Disabled => design_tokens::text_muted(theme),
        _ => design_tokens::border_muted(theme),
    };
    button::Style {
        background: match status {
            button::Status::Hovered => Some(Background::Color(design_tokens::surface_hover(theme))),
            button::Status::Pressed => {
                Some(Background::Color(design_tokens::surface_selected(theme)))
            }
            _ => None,
        },
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => design_tokens::primary(theme),
            button::Status::Disabled => design_tokens::text_muted(theme),
            _ => design_tokens::text_secondary(theme),
        },
        border: Border {
            color: border_color,
            width: design_tokens::BORDER_WIDTH,
            radius: design_tokens::RADIUS_MD.into(),
        },
        ..Default::default()
    }
}

/// Ghost / minimal button style — no background, subtle text.
pub fn button_ghost_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: match status {
            button::Status::Hovered => Some(Background::Color(design_tokens::surface_hover(theme))),
            button::Status::Pressed => {
                Some(Background::Color(design_tokens::surface_selected(theme)))
            }
            _ => None,
        },
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => design_tokens::primary(theme),
            button::Status::Disabled => design_tokens::text_muted(theme),
            _ => design_tokens::text_secondary(theme),
        },
        border: Border {
            radius: design_tokens::RADIUS_SM.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Ghost icon button style — for icon-only toolstrip items.
pub fn button_icon_ghost_style(theme: &Theme, status: button::Status) -> button::Style {
    design_tokens::icon_button(theme, status)
}

// ═══════════════════════════════════════════════════════════════════════
// 5. PRIMARY BUTTON — filled, accent bg
// ═══════════════════════════════════════════════════════════════════════

/// A filled primary button with label and optional icon.
pub fn primary_button<'a>(
    label: &'a str,
    on_press: Option<AppMessage>,
    disabled: bool,
) -> Element<'a, AppMessage> {
    let btn = button(
        text(label)
            .font(Typography::ButtonLabel.font())
            .size(Typography::ButtonLabel.size_px()),
    )
    .padding([design_tokens::SPACE_8, design_tokens::SPACE_16])
    .style(button_primary_style);

    if disabled {
        btn.into()
    } else if let Some(msg) = on_press {
        btn.on_press(msg).into()
    } else {
        btn.into()
    }
}

/// A primary button with a leading icon.
pub fn primary_button_icon<'a>(
    icon: Icon,
    label: &'a str,
    on_press: Option<AppMessage>,
    disabled: bool,
) -> Element<'a, AppMessage> {
    let row = Row::new()
        .push(
            icon.build()
                .size(IconSize::Sm)
                .color_fn(white_color)
                .build(),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_8))
                .height(Length::Shrink),
        )
        .push(
            text(label)
                .font(Typography::ButtonLabel.font())
                .size(Typography::ButtonLabel.size_px())
                .color(Color::WHITE),
        )
        .align_y(Alignment::Center);

    let btn = button(row)
        .padding([design_tokens::SPACE_8, design_tokens::SPACE_16])
        .style(button_primary_style);

    if disabled {
        btn.into()
    } else if let Some(msg) = on_press {
        btn.on_press(msg).into()
    } else {
        btn.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 6. SECONDARY / OUTLINE BUTTON
// ═══════════════════════════════════════════════════════════════════════

/// An outline secondary button.
pub fn secondary_button<'a>(
    label: &'a str,
    on_press: Option<AppMessage>,
    disabled: bool,
) -> Element<'a, AppMessage> {
    let btn = button(
        text(label)
            .font(Typography::ButtonLabel.font())
            .size(Typography::ButtonLabel.size_px()),
    )
    .padding([design_tokens::SPACE_8, design_tokens::SPACE_16])
    .style(button_secondary_style);

    if disabled {
        btn.into()
    } else if let Some(msg) = on_press {
        btn.on_press(msg).into()
    } else {
        btn.into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 7. GHOST ICON BUTTON — transparent bg, icon-only
// ═══════════════════════════════════════════════════════════════════════

/// An icon-only ghost button — transparent background, subtle icon.
pub fn ghost_icon_button<'a>(
    icon: Icon,
    size: IconSize,
    tooltip_label: Option<&'a str>,
    on_press: Option<AppMessage>,
    disabled: bool,
    destructive: bool,
) -> Element<'a, AppMessage> {
    let svg_el = icon
        .build()
        .size(size)
        .destructive(destructive)
        .interactive(!disabled)
        .build();

    let btn = button(svg_el)
        .padding(design_tokens::SPACE_8)
        .style(button_icon_ghost_style);

    let element: Element<'_, AppMessage> = if disabled {
        btn.into()
    } else if let Some(msg) = on_press {
        btn.on_press(msg).into()
    } else {
        btn.into()
    };

    if let Some(label) = tooltip_label {
        iced_tooltip::Tooltip::new(
            element,
            text(label)
                .size(Typography::SecondaryText.size_px())
                .font(Typography::SecondaryText.font()),
            iced_tooltip::Position::Bottom,
        )
        .into()
    } else {
        element
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 8. TEXT INPUT
// ═══════════════════════════════════════════════════════════════════════

/// Style for a text input field.
fn text_input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Background::Color(design_tokens::bg_input(theme)),
        border: Border {
            color: match status {
                text_input::Status::Focused { .. } => design_tokens::color_focus(theme),
                _ => design_tokens::border_muted(theme),
            },
            width: match status {
                text_input::Status::Focused { .. } => design_tokens::FOCUS_WIDTH,
                _ => design_tokens::BORDER_WIDTH,
            },
            radius: design_tokens::RADIUS_MD.into(),
        },
        icon: Color::default(),
        placeholder: design_tokens::text_muted(theme),
        value: design_tokens::text_primary(theme),
        selection: design_tokens::primary_soft(theme),
    }
}

/// Style for a text input in error state.
fn text_input_error_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let base = text_input_style(theme, status);
    text_input::Style {
        border: Border {
            color: design_tokens::color_danger(theme),
            width: design_tokens::BORDER_WIDTH,
            radius: design_tokens::RADIUS_MD.into(),
        },
        ..base
    }
}

/// A styled text input field.
pub fn text_input_field<'a>(
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> AppMessage + 'a,
    has_error: bool,
) -> Element<'a, AppMessage> {
    let input = text_input(placeholder, value)
        .on_input(on_input)
        .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
        .size(Typography::Body.size_px())
        .font(Typography::Body.font());

    if has_error {
        input.style(text_input_error_style).into()
    } else {
        input.style(text_input_style).into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 9. STATUS DOT — small coloured circle for presence / state
// ═══════════════════════════════════════════════════════════════════════

/// Kinds of status a dot can represent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusDotKind {
    Online,
    Offline,
    Warning,
}

/// A small coloured circle indicating presence or state.
pub fn status_dot<'a, Message: 'a>(kind: StatusDotKind, dot_size: f32) -> Element<'a, Message> {
    let size = dot_size.max(6.0);
    container(
        Space::new()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size)),
    )
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .style(move |t| container::Style {
        background: Some(Background::Color(match kind {
            StatusDotKind::Online => design_tokens::color_success(t),
            StatusDotKind::Offline => design_tokens::text_muted(t),
            StatusDotKind::Warning => design_tokens::color_warning(t),
        })),
        border: Border {
            radius: (size / 2.0).into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 10. BADGE / PILL
// ═══════════════════════════════════════════════════════════════════════

/// Kinds of badge styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    /// Neutral / default — muted bg.
    Default,
    /// Accent — primary_soft bg, primary text.
    Accent,
    /// Count badge — primary bg, white text (for unread counts).
    Count,
    /// Danger — red bg.
    Danger,
}

/// A small rounded pill containing text.
pub fn badge<'a, Message: 'a>(label: &'a str, kind: BadgeKind) -> Element<'a, Message> {
    container(
        text(label)
            .size(Typography::SecondaryText.size_px())
            .font(Typography::SecondaryText.font()),
    )
    .padding([2.0, design_tokens::SPACE_8])
    .style(move |t| container::Style {
        background: Some(Background::Color(match kind {
            BadgeKind::Default => design_tokens::surface_hover(t),
            BadgeKind::Accent => design_tokens::primary_soft(t),
            BadgeKind::Count => design_tokens::primary(t),
            BadgeKind::Danger => design_tokens::color_danger(t),
        })),
        text_color: Some(match kind {
            BadgeKind::Default => design_tokens::text_secondary(t),
            BadgeKind::Accent => design_tokens::primary(t),
            BadgeKind::Count => Color::WHITE,
            BadgeKind::Danger => Color::WHITE,
        }),
        border: Border {
            radius: design_tokens::SPACE_12.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 11. DIVIDER
// ═══════════════════════════════════════════════════════════════════════

/// A horizontal divider line.
pub fn divider<'a, Message: 'a>() -> Element<'a, Message> {
    rule::horizontal(1)
        .style(move |t| rule::Style {
            color: design_tokens::border_muted(t),
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: false,
        })
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 12. LIST ROW
// ═══════════════════════════════════════════════════════════════════════

/// A builder for list rows — supports leading icon/avatar, primary +
/// secondary text, trailing metadata, selected background, full-row click.
pub struct ListRow<'a, Message> {
    leading: Option<Element<'a, Message>>,
    primary_text: String,
    secondary_text: Option<String>,
    trailing: Option<Element<'a, Message>>,
    selected: bool,
    on_press: Option<Message>,
    padding_v: f32,
    padding_h: f32,
}

impl<'a, Message: Clone + 'a> ListRow<'a, Message> {
    /// Start a list row with primary text.
    pub fn new(primary_text: impl Into<String>) -> Self {
        Self {
            leading: None,
            primary_text: primary_text.into(),
            secondary_text: None,
            trailing: None,
            selected: false,
            on_press: None,
            padding_v: design_tokens::SPACE_8,
            padding_h: design_tokens::SPACE_12,
        }
    }

    /// Add a leading element (icon, avatar, etc.).
    pub fn leading(mut self, element: Element<'a, Message>) -> Self {
        self.leading = Some(element);
        self
    }

    /// Add secondary text below the primary.
    pub fn secondary(mut self, text: impl Into<String>) -> Self {
        self.secondary_text = Some(text.into());
        self
    }

    /// Add trailing metadata or control.
    pub fn trailing(mut self, element: Element<'a, Message>) -> Self {
        self.trailing = Some(element);
        self
    }

    /// Mark as selected — uses the selected-surface background.
    pub fn selected(mut self, yes: bool) -> Self {
        self.selected = yes;
        self
    }

    /// Make the row clickable with full-row hit area.
    pub fn on_press(mut self, msg: Message) -> Self {
        self.on_press = Some(msg);
        self
    }

    /// Set custom padding.
    pub fn padding(mut self, vertical: f32, horizontal: f32) -> Self {
        self.padding_v = vertical;
        self.padding_h = horizontal;
        self
    }

    /// Build the row element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        let mut row = Row::new()
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center);

        if let Some(leading) = self.leading {
            row = row.push(leading);
        }

        let mut text_col = Column::new()
            .push(
                text(self.primary_text)
                    .font(Typography::Body.font())
                    .size(Typography::Body.size_px())
                    .color(design_tokens::text_primary(theme)),
            )
            .spacing(2.0)
            .width(Length::Fill);

        if let Some(sec) = self.secondary_text {
            text_col = text_col.push(
                text(sec)
                    .font(Typography::SecondaryText.font())
                    .size(Typography::SecondaryText.size_px())
                    .color(design_tokens::text_secondary(theme)),
            );
        }

        row = row.push(text_col);

        if let Some(trailing) = self.trailing {
            row = row.push(trailing);
        }

        let bg = if self.selected {
            design_tokens::surface_selected(theme)
        } else {
            Color::TRANSPARENT
        };

        let inner = container(row)
            .padding(Padding {
                top: self.padding_v,
                right: self.padding_h,
                bottom: self.padding_v,
                left: self.padding_h,
            })
            .style(move |_t| container::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    radius: design_tokens::RADIUS_SM.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        if let Some(msg) = self.on_press {
            let selected = self.selected;
            button(inner)
                .on_press(msg)
                .padding(0)
                .style(move |theme, status| list_row_button_style(theme, status, selected))
                .into()
        } else {
            inner.into()
        }
    }
}

/// Button style for a clickable list row — matches selected/enabled states.
fn list_row_button_style(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    let bg = match status {
        button::Status::Hovered => design_tokens::surface_hover(theme),
        button::Status::Pressed => design_tokens::surface_selected(theme),
        _ => {
            if selected {
                design_tokens::surface_selected(theme)
            } else {
                Color::TRANSPARENT
            }
        }
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: design_tokens::text_primary(theme),
        border: Border {
            radius: design_tokens::RADIUS_SM.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 13. EMPTY STATE
// ═══════════════════════════════════════════════════════════════════════

/// A centred empty-state placeholder with icon, title, subtitle, and
/// optional action button.
pub fn empty_state<'a>(
    icon: Icon,
    title: &'a str,
    subtitle: &'a str,
    action_label: Option<&'a str>,
    action_msg: Option<AppMessage>,
) -> Element<'a, AppMessage> {
    let mut col = Column::new()
        .push(
            icon.build()
                .size(IconSize::Xl)
                .color_fn(design_tokens::text_muted)
                .build(),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_16)),
        )
        .push(
            text(title)
                .font(Typography::SectionHeading.font())
                .size(Typography::SectionHeading.size_px())
                .color(design_tokens::text_primary(&Theme::Light)),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(
            text(subtitle)
                .font(Typography::Body.font())
                .size(Typography::Body.size_px())
                .color(design_tokens::text_secondary(&Theme::Light)),
        )
        .align_x(Alignment::Center)
        .spacing(0);

    if let (Some(label), Some(msg)) = (action_label, action_msg) {
        col = col.push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_16)),
        );
        col = col.push(primary_button(label, Some(msg), false));
    }

    container(col)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 14. AVATAR
// ═══════════════════════════════════════════════════════════════════════

/// Builder for a consistent avatar — supports initial + generated colour,
/// online dot, unread badge, and optional fallback icon.
pub struct Avatar<Message> {
    /// Display name (used to derive initials and colour).
    name: String,
    /// Optional image handle override (if a profile image is available).
    image: Option<iced::widget::image::Handle>,
    /// Avatar size (default: MD = 48px).
    size: f32,
    /// Show an online status dot.
    online_dot: bool,
    /// Show an unread count badge.
    unread_count: Option<u32>,
    /// Fallback icon when no initials can be derived.
    fallback_icon: Option<Icon>,
    /// Dark mode for colour generation.
    dark_mode: bool,
    _phantom: std::marker::PhantomData<Message>,
}

impl<Message: 'static> Avatar<Message> {
    /// Create a new avatar from a display name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            image: None,
            size: design_tokens::AVATAR_MD,
            online_dot: false,
            unread_count: None,
            fallback_icon: None,
            dark_mode: false,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Use a profile image instead of generated initials.
    pub fn image(mut self, handle: iced::widget::image::Handle) -> Self {
        self.image = Some(handle);
        self
    }

    /// Set avatar size.
    pub fn size(mut self, px: f32) -> Self {
        self.size = px;
        self
    }

    /// Show an online status dot in the bottom-right.
    pub fn online_dot(mut self, show: bool) -> Self {
        self.online_dot = show;
        self
    }

    /// Show an unread count badge.
    pub fn unread_badge(mut self, count: u32) -> Self {
        self.unread_count = Some(count);
        self
    }

    /// Set a fallback icon for when initials are empty.
    pub fn fallback_icon(mut self, icon: Icon) -> Self {
        self.fallback_icon = Some(icon);
        self
    }

    /// Set dark mode for colour generation.
    pub fn dark_mode(mut self, yes: bool) -> Self {
        self.dark_mode = yes;
        self
    }

    /// Build the avatar element.
    pub fn build(self) -> Element<'static, Message> {
        let radius = self.size / 2.0;

        if let Some(handle) = self.image {
            let img = iced::widget::image(handle)
                .content_fit(iced::ContentFit::Cover)
                .width(Length::Fixed(self.size))
                .height(Length::Fixed(self.size));

            return container(img)
                .width(Length::Fixed(self.size))
                .height(Length::Fixed(self.size))
                .style(move |_t| container::Style {
                    border: Border {
                        radius: radius.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into();
        }

        let initials = presentation::initials(&self.name);
        let display_text = if initials.is_empty() {
            "?".to_string()
        } else if initials.len() > 2 {
            initials[..2].to_string()
        } else {
            initials
        };

        let avatar_color = presentation::initials_color(&self.name, self.dark_mode);
        let font_size = (self.size * 0.4).clamp(10.0, 28.0);

        let label: Element<'static, Message> = if !display_text.is_empty() && display_text != "?" {
            text(display_text)
                .size(Pixels(font_size))
                .color(Color::WHITE)
                .font(Typography::SecondaryText.font())
                .into()
        } else if let Some(icon) = self.fallback_icon {
            icon.build()
                .size(IconSize::Sm)
                .color_fn(white_color)
                .build()
                .into()
        } else {
            text("?")
                .size(Pixels(font_size))
                .color(Color::WHITE)
                .font(Typography::SecondaryText.font())
                .into()
        };

        // Build badge overlay if needed
        let has_badge = self.online_dot || self.unread_count.is_some();
        if has_badge {
            let badge_element: Element<'static, Message> = if let Some(count) = self.unread_count {
                let count_text = if count > 99 {
                    "99+".to_string()
                } else {
                    count.to_string()
                };
                container(
                    text(count_text)
                        .size(10.0)
                        .color(Color::WHITE)
                        .font(Typography::Timestamp.font()),
                )
                .padding([1.0, 5.0])
                .style(move |t| container::Style {
                    background: Some(Background::Color(design_tokens::primary(t))),
                    border: Border {
                        radius: design_tokens::SPACE_8.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
            } else {
                status_dot::<Message>(StatusDotKind::Online, 12.0)
            };

            // Stack avatar with badge at bottom-right
            let circle = container(label)
                .width(Length::Fixed(self.size))
                .height(Length::Fixed(self.size))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .style(move |_t| container::Style {
                    background: Some(Background::Color(avatar_color)),
                    border: Border {
                        radius: radius.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                });

            // Position badge at bottom-right of avatar using a stacking column/row
            let badge_offset = self.size - 12.0;
            container(
                Column::new().push(
                    Row::new()
                        .push(
                            Space::new()
                                .width(Length::Fixed(badge_offset))
                                .height(Length::Fixed(badge_offset)),
                        )
                        .push(badge_element),
                ),
            )
            .style(|_t| container::Style::default())
            .into()
        } else {
            container(label)
                .width(Length::Fixed(self.size))
                .height(Length::Fixed(self.size))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .style(move |_t| container::Style {
                    background: Some(Background::Color(avatar_color)),
                    border: Border {
                        radius: radius.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 15. SECTION HEADER
// ═══════════════════════════════════════════════════════════════════════

/// A section header with label and optional trailing action button.
pub fn section_header<'a>(
    title: &'a str,
    trailing: Option<Element<'a, AppMessage>>,
) -> Element<'a, AppMessage> {
    let mut row = Row::new()
        .push(
            text(title)
                .font(Typography::SidebarSectionLabel.font())
                .size(Typography::SidebarSectionLabel.size_px())
                .color(design_tokens::text_muted(&Theme::Light)),
        )
        .push(Space::new().width(Length::Fill).height(Length::Shrink));

    if let Some(trail) = trailing {
        row = row.push(trail);
    }

    container(row)
        .padding(Padding {
            top: design_tokens::SPACE_12,
            right: design_tokens::SPACE_16,
            bottom: design_tokens::SPACE_4,
            left: design_tokens::SPACE_16,
        })
        .width(Length::Fill)
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 16. CARD HEADER
// ═══════════════════════════════════════════════════════════════════════

/// A card header pattern — icon, title, count/status badge, optional trailing
/// action button.
pub fn card_header<'a>(
    icon: Option<Icon>,
    title: &'a str,
    badge_label: Option<&'a str>,
    badge_kind: BadgeKind,
    trailing_action: Option<Element<'a, AppMessage>>,
) -> Element<'a, AppMessage> {
    let mut row = Row::new()
        .spacing(design_tokens::SPACE_8)
        .align_y(Alignment::Center);

    if let Some(ic) = icon {
        row = row.push(ic.build().size(IconSize::Md).build());
    }

    row = row.push(
        text(title)
            .font(Typography::SectionHeading.font())
            .size(Typography::SectionHeading.size_px())
            .color(design_tokens::text_primary(&Theme::Light)),
    );

    if let Some(badge_text) = badge_label {
        row = row.push(badge::<AppMessage>(badge_text, badge_kind));
    }

    row = row.push(Space::new().width(Length::Fill).height(Length::Shrink));

    if let Some(trail) = trailing_action {
        row = row.push(trail);
    }

    container(row).width(Length::Fill).into()
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_dot_kinds_are_distinct() {
        let kinds = [
            StatusDotKind::Online,
            StatusDotKind::Offline,
            StatusDotKind::Warning,
        ];
        for (i, a) in kinds.iter().enumerate() {
            for (j, b) in kinds.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn badge_kinds_are_distinct() {
        let kinds = [
            BadgeKind::Default,
            BadgeKind::Accent,
            BadgeKind::Count,
            BadgeKind::Danger,
        ];
        for (i, a) in kinds.iter().enumerate() {
            for (j, b) in kinds.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn avatar_size_default_is_md() {
        let avatar: Avatar<()> = Avatar::new("Alice");
        assert_eq!(avatar.size, design_tokens::AVATAR_MD);
    }

    #[test]
    fn card_builder_stores_children() {
        let card: Card<'static, ()> = Card::new(vec![]);
        assert!(card.children.is_empty());
    }

    #[test]
    fn list_row_stores_text() {
        let row: ListRow<'static, ()> = ListRow::new("Hello");
        assert_eq!(row.primary_text, "Hello");
    }
}
