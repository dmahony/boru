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
//! | Date separator     | `date_separator(…)`       | default (centered, muted)        |
//! | System event chip  | `system_event_chip(…)`    | default (centered, muted surface)|
//! | Connection footer  | `connection_footer(…)`    | live mesh summary                |
//! | Connectivity notice | `connectivity_notice(…)`  | offline / stale-data / warning   |
//! | Inline error       | `InlineError::new(…)`     | error message + optional retry   |
//! | Loading skeleton   | `LoadingSkeleton::new(…)` | row count, optional row height   |

use iced::widget::{
    button, container, rule, svg, text, text_input, tooltip as iced_tooltip, Column, Row, Space,
    Stack,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Pixels, Theme};

use crate::app::AppMessage;
use crate::design_tokens;
use crate::fonts::TypeRole;
use crate::icon_system::{Icon, IconSize};
use crate::presentation;

/// Build the compact, persistent mesh connectivity strip used at the bottom
/// of the home screen.
///
/// The component is deliberately data-only: callers provide the latest
/// counts and status labels from application state, so it can be reused by
/// responsive layouts without coupling it to `IcedChat`.
pub fn connection_footer<'a>(
    health_label: &'a str,
    health_color: fn(&Theme) -> Color,
    direct_peers: usize,
    relayed_peers: usize,
    neighbor_count: usize,
    encryption_status: &'a str,
) -> Element<'a, AppMessage> {
    let mesh_icon = Icon::Mesh
        .build()
        .size(IconSize::Xs)
        .build()
        .style(move |theme, _| svg::Style {
            color: Some(health_color(theme)),
        });
    let lock_icon = Icon::Lock
        .build()
        .size(IconSize::Xs)
        .build()
        .style(|theme, _| svg::Style {
            color: Some(design_tokens::text_secondary(theme)),
        });

    container(
        Row::new()
            .push(mesh_icon)
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!("Mesh {health_label}"),
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                "·",
            )
            .style(|theme| iced::widget::text::Style {
                color: Some(design_tokens::text_muted(theme)),
            }))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!("{direct_peers} direct"),
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                "·",
            )
            .style(|theme| iced::widget::text::Style {
                color: Some(design_tokens::text_muted(theme)),
            }))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!("{relayed_peers} relayed"),
            ))
            .push(Space::new().width(Length::Fill))
            .push(lock_icon)
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                encryption_status,
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                "·",
            )
            .style(|theme| iced::widget::text::Style {
                color: Some(design_tokens::text_muted(theme)),
            }))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!("{neighbor_count} neighbors"),
            ))
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center)
            .width(Length::Fill),
    )
    .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
    .width(Length::Fill)
    .style(|theme| design_tokens::card_style(theme))
    .into()
}

/// Build the compact, muted status line shown below the chat composer.
///
/// Reports the active conversation's connection route (direct mesh, relay,
/// or not connected) and, when connected, the peer count. The chat header
/// already reports presence and encryption (direct chats) or member count
/// (group chats), so this footer deliberately shows complementary route /
/// peer state only — no status text is duplicated between header and footer
/// (plan UI-16 step 129).
pub fn chat_status_footer<'a>(
    route_label: String,
    connected: bool,
    peer_label: Option<String>,
) -> Element<'a, AppMessage> {
    let route_color: fn(&Theme) -> Color = if connected {
        design_tokens::text_secondary
    } else {
        design_tokens::text_muted
    };
    let route_icon = Icon::Mesh
        .build()
        .size(IconSize::Xs)
        .build()
        .style(move |theme, _| svg::Style {
            color: Some(route_color(theme)),
        });
    let mut row = Row::new()
        .push(route_icon)
        .push(
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, route_label)
                .style(move |theme| iced::widget::text::Style {
                    color: Some(route_color(theme)),
                }),
        )
        .spacing(design_tokens::SPACE_6)
        .align_y(Alignment::Center);
    match peer_label {
        Some(peer) => {
            row = row
                .push(Space::new().width(Length::Fill))
                .push(text("·").font(TypeRole::Metadata.font()).style(|theme| {
                    iced::widget::text::Style {
                        color: Some(design_tokens::text_muted(theme)),
                    }
                }))
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, peer)
                        .style(|theme| iced::widget::text::Style {
                            color: Some(design_tokens::text_muted(theme)),
                        }),
                )
                .spacing(design_tokens::SPACE_8);
        }
        None => {
            row = row.push(Space::new().width(Length::Fill));
        }
    }
    container(row)
        .padding([design_tokens::SPACE_2, design_tokens::SPACE_4])
        .width(Length::Fill)
        .into()
}

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

/// A circular tile containing a centred icon, used in home cards and quick
/// actions. The container is a full circle (Figure 3: soft green circular
/// icon wells) whose diameter scales with the icon size class.
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
                radius: (tile_size / 2.0).into(),
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
            .font(TypeRole::ButtonLabel.font())
            .size(TypeRole::ButtonLabel.size_px()),
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
                .font(TypeRole::ButtonLabel.font())
                .size(TypeRole::ButtonLabel.size_px())
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
            .font(TypeRole::ButtonLabel.font())
            .size(TypeRole::ButtonLabel.size_px()),
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
                .size(TypeRole::Metadata.size_px())
                .font(TypeRole::Metadata.font()),
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
///
/// `pub(crate)` so the shared form components (`form_components.rs`) can reuse
/// the exact same input styling for searchable selects and multiline text
/// areas instead of maintaining parallel style definitions.
pub(crate) fn text_input_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
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
///
/// `pub(crate)` so shared form components reuse the same error styling.
pub(crate) fn text_input_error_style(theme: &Theme, status: text_input::Status) -> text_input::Style {
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
///
/// Input strings are cloned into the widget, so the returned element does not
/// borrow from `placeholder` or `value` (mirrors `iced::widget::text_input`).
pub fn text_input_field<'a, 'b>(
    placeholder: &'b str,
    value: &'b str,
    on_input: impl Fn(String) -> AppMessage + 'a,
    has_error: bool,
) -> Element<'a, AppMessage> {
    text_input_field_opts(placeholder, value, on_input, has_error, None, None)
}

/// A styled text input field with optional focus [`Id`] and Enter-to-submit.
///
/// `id` is used by the global Tab/focus machinery and by auto-focus on dialog
/// open (`iced::widget::operation::focus`). `on_submit` fires on Enter so a
/// dialog's primary field can submit the form from the keyboard. When
/// `on_submit` is `None`, Enter is a no-op (e.g. search/filter fields).
pub fn text_input_field_opts<'a, 'b>(
    placeholder: &'b str,
    value: &'b str,
    on_input: impl Fn(String) -> AppMessage + 'a,
    has_error: bool,
    id: Option<&'static str>,
    on_submit: Option<AppMessage>,
) -> Element<'a, AppMessage> {
    let mut input = text_input(placeholder, value)
        .on_input(on_input)
        .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
        .size(TypeRole::Body.size_px())
        .font(TypeRole::Body.font());

    if let Some(id) = id {
        input = input.id(id);
    }
    if let Some(on_submit) = on_submit {
        input = input.on_submit(on_submit);
    }

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
            .size(TypeRole::Metadata.size_px())
            .font(TypeRole::Metadata.font()),
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

/// A small rounded pill containing an owned label (for dynamic counts that
/// cannot borrow from the enclosing view).
pub fn badge_owned<'a, Message: 'a>(label: String, kind: BadgeKind) -> Element<'a, Message> {
    container(
        text(label)
            .size(TypeRole::Metadata.size_px())
            .font(TypeRole::Metadata.font()),
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
        // ... ListRow
        if let Some(leading) = self.leading {
            row = row.push(leading);
        }

        let mut text_col = Column::new()
            .push(
                text(self.primary_text)
                    .font(TypeRole::Body.font())
                    .size(TypeRole::Body.size_px())
                    .color(design_tokens::text_primary(theme)),
            )
            .spacing(2.0)
            .width(Length::Fill);

        if let Some(sec) = self.secondary_text {
            text_col = text_col.push(
                text(sec)
                    .font(TypeRole::SupportingText.font())
                    .size(TypeRole::SupportingText.size_px())
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
                .font(TypeRole::CardTitle.font())
                .size(TypeRole::CardTitle.size_px())
                .color(design_tokens::text_primary(&Theme::Light)),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(
            text(subtitle)
                .font(TypeRole::SupportingText.font())
                .size(TypeRole::SupportingText.size_px())
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
                .font(TypeRole::Metadata.font())
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
                .font(TypeRole::Metadata.font())
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
                        .font(TypeRole::Metadata.font()),
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
                // The online dot is colour-coded; give it a text label so
                // status is never communicated by colour alone (UI-19).
                iced_tooltip::Tooltip::new(
                    status_dot::<Message>(StatusDotKind::Online, 12.0),
                    text("Online")
                        .size(TypeRole::Metadata.size_px())
                        .font(TypeRole::Metadata.font()),
                    iced_tooltip::Position::Top,
                )
                .into()
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

            // Overlay the status badge at bottom-right while retaining the
            // avatar circle itself. (The badge-only layout would otherwise
            // make online avatars disappear entirely.)
            let badge_offset = self.size - 12.0;
            Stack::new()
                .push(circle)
                .push(
                    container(
                        Row::new()
                            .push(
                                Space::new()
                                    .width(Length::Fixed(badge_offset))
                                    .height(Length::Fixed(badge_offset)),
                            )
                            .push(badge_element),
                    )
                    .width(Length::Fixed(self.size))
                    .height(Length::Fixed(self.size))
                    .style(|_t| container::Style::default()),
                )
                .width(Length::Fixed(self.size))
                .height(Length::Fixed(self.size))
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
                .font(TypeRole::ButtonLabel.font())
                .size(TypeRole::ButtonLabel.size_px())
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
            .font(TypeRole::CardTitle.font())
            .size(TypeRole::CardTitle.size_px())
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
// 17. SIDEBAR SECTION HEADER
// ═══════════════════════════════════════════════════════════════════════

/// FONTS-06: sidebar section label size (IBM Plex Sans SemiBold 11–12 px).
///
/// All-caps sidebar section labels (CHATS / GROUPS / FRIENDS / DISCOVER /
/// PUBLIC ROOMS / REQUESTS) keep the `ButtonLabel` role's family and weight
/// (IBM Plex Sans SemiBold) but render at the tighter FONTS-06 11–12 px band
/// instead of the 14 px button default. iced 0.14's text widgets expose no
/// letter-spacing API, so the spec's "modest letter spacing" is approximated
/// by the SemiBold weight at this small size (no API to set it explicitly).
const SIDEBAR_SECTION_LABEL_SIZE: f32 = 12.0;

/// A collapsible sidebar section header with an optional count badge and a
/// trailing add action.
///
/// The whole label row is clickable when `on_toggle` is supplied (expand /
/// collapse); the optional add action is a separate icon button so it never
/// triggers the toggle.  All colours are theme-aware.
pub struct SidebarSectionHeader<'a> {
    title: &'a str,
    count: Option<usize>,
    collapsed: bool,
    on_toggle: Option<AppMessage>,
    add_action: Option<(Icon, AppMessage)>,
}

impl<'a> SidebarSectionHeader<'a> {
    /// Start a header with an ALL-CAPS section label.
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            count: None,
            collapsed: false,
            on_toggle: None,
            add_action: None,
        }
    }

    /// Show a count badge.  Zero is treated as no badge.
    pub fn count(mut self, count: usize) -> Self {
        self.count = (count > 0).then_some(count);
        self
    }

    /// Mark the section collapsed so the header shows a right chevron.
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    /// Make the whole header row clickable with the given message.
    pub fn on_toggle(mut self, msg: AppMessage) -> Self {
        self.on_toggle = Some(msg);
        self
    }

    /// Add a trailing icon action (e.g. a "＋" add button).
    pub fn add_action(mut self, icon: Icon, msg: AppMessage) -> Self {
        self.add_action = Some((icon, msg));
        self
    }

    /// Build the header element.
    pub fn build(self, theme: &Theme) -> Element<'a, AppMessage> {
        let chevron = if self.collapsed {
            Icon::ChevronRight
        } else {
            Icon::ChevronDown
        };

        let mut toggle_row = Row::new()
            .push(
                chevron
                    .build()
                    .size(IconSize::Sm)
                    .color_fn(design_tokens::text_muted)
                    .build(),
            )
            .push(
                Space::new()
                    .width(Length::Fixed(design_tokens::SPACE_4))
                    .height(Length::Shrink),
            )
            .push(
                // FONTS-06: all-caps section label in IBM Plex Sans SemiBold
                // at the 11–12 px band (ButtonLabel family/weight, tighter
                // sidebar size via SIDEBAR_SECTION_LABEL_SIZE).
                text(self.title)
                    .font(TypeRole::ButtonLabel.font())
                    .size(SIDEBAR_SECTION_LABEL_SIZE)
                    .color(design_tokens::text_muted(theme))
                    .width(Length::Shrink),
            );

        if let Some(count) = self.count {
            toggle_row = toggle_row
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_4))
                        .height(Length::Shrink),
                )
                .push(
                    text(count.to_string())
                        .font(TypeRole::Metadata.font())
                        .size(TypeRole::Metadata.size_px())
                        .color(design_tokens::text_muted(theme)),
                );
        }

        toggle_row = toggle_row.push(Space::new().width(Length::Fill).height(Length::Shrink));

        let on_toggle = self.on_toggle;

        let mut label_button = button(toggle_row)
            .width(Length::Fill)
            .padding([design_tokens::SPACE_6, design_tokens::SPACE_12])
            .style(move |t, status| {
                let bg = match status {
                    button::Status::Hovered => {
                        Some(Background::Color(design_tokens::surface_hover(t)))
                    }
                    button::Status::Pressed => {
                        Some(Background::Color(design_tokens::surface_selected(t)))
                    }
                    _ => None,
                };
                button::Style {
                    background: bg,
                    border: Border {
                        radius: design_tokens::RADIUS_MD.into(),
                        ..Default::default()
                    },
                    text_color: design_tokens::text_muted(t),
                    ..Default::default()
                }
            });

        // The whole label row is the toggle control.  `on_toggle` was stored
        // but never attached as `on_press`, so the header rendered as an
        // inert button and sidebar sections could not be collapsed.
        if let Some(msg) = on_toggle {
            label_button = label_button.on_press(msg);
        }

        let mut header_row = Row::new().push(label_button);

        if let Some((icon, msg)) = self.add_action {
            let add_btn = button(icon.build().size(IconSize::Md).interactive(true).build())
                .on_press(msg)
                .padding(design_tokens::SPACE_8)
                .style(move |t, status| {
                    let active = match status {
                        button::Status::Hovered => design_tokens::primary(t),
                        button::Status::Pressed => design_tokens::primary_pressed(t),
                        _ => design_tokens::text_secondary(t),
                    };
                    button::Style {
                        background: match status {
                            button::Status::Hovered => {
                                Some(Background::Color(design_tokens::surface_hover(t)))
                            }
                            button::Status::Pressed => {
                                Some(Background::Color(design_tokens::surface_selected(t)))
                            }
                            _ => None,
                        },
                        border: Border {
                            radius: design_tokens::RADIUS_SM.into(),
                            ..Default::default()
                        },
                        text_color: active,
                        ..Default::default()
                    }
                });
            header_row = header_row.push(add_btn);
        }

        header_row
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 17b. SCROLLABLE WITH EMBEDDED SCROLLBAR
// ═══════════════════════════════════════════════════════════════════════

/// A vertical `Scrollable` whose scrollbar is embedded in the layout instead
/// of floating over the content.
///
/// Iced 0.14 draws the scrollbar over the content by default (`spacing:
/// None`), which covers the right edge of bubbles, cards, and rows in chat
/// and every other list.  Setting `spacing` embeds the scrollbar: it takes
/// layout space of its own (10 px track + spacing), so content is never
/// obscured, and the gutter collapses again when the content fits.
pub fn gutter_scrollable<'a, Message>(
    content: impl Into<Element<'a, Message>>,
) -> iced::widget::Scrollable<'a, Message> {
    use iced::widget::scrollable;

    scrollable(content).direction(scrollable::Direction::Vertical(
        scrollable::Scrollbar::default().spacing(design_tokens::SPACE_4),
    ))
}

// ═══════════════════════════════════════════════════════════════════════
// 18. SIDEBAR EMPTY STATE
// ═══════════════════════════════════════════════════════════════════════

/// A compact left-aligned empty-state row for sidebar sections: a 20 px icon,
/// a primary title line, and a muted supporting explanation, with an optional
/// ghost action button.  Colours resolve against the live theme so the row
/// works in both light and dark mode.
pub fn sidebar_empty_state<'a>(
    icon: Icon,
    title: &'a str,
    supporting: &'a str,
    action: Option<(&'a str, AppMessage)>,
) -> Element<'a, AppMessage> {
    let mut copy = Column::new()
        .push(
            text(title)
                .font(TypeRole::Body.font())
                .size(TypeRole::Body.size_px())
                .style(move |t| text::Style {
                    color: Some(design_tokens::text_secondary(t)),
                }),
        )
        .push(
            text(supporting)
                .font(TypeRole::SupportingText.font())
                .size(TypeRole::SupportingText.size_px())
                .style(move |t| text::Style {
                    color: Some(design_tokens::text_muted(t)),
                }),
        )
        .spacing(design_tokens::SPACE_2)
        .width(Length::Fill);

    if let Some((label, message)) = action {
        copy = copy.push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_4)),
        );
        copy = copy.push(
            button(
                text(label)
                    .font(TypeRole::ButtonLabel.font())
                    .size(TypeRole::ButtonLabel.size_px()),
            )
            .on_press(message)
            .padding([design_tokens::SPACE_4, design_tokens::SPACE_10])
            .style(button_secondary_style),
        );
    }

    container(
        Row::new()
            .push(
                icon.build()
                    .size(IconSize::Md)
                    .color_fn(design_tokens::text_muted)
                    .build(),
            )
            .push(
                Space::new()
                    .width(Length::Fixed(design_tokens::SPACE_8))
                    .height(Length::Shrink),
            )
            .push(copy)
            .align_y(Alignment::Start),
    )
    .width(Length::Fill)
    .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
    .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 14. TIMELINE ITEMS — date separators and system-event chips (Figure 4)
// ═══════════════════════════════════════════════════════════════════════
//
// Presentational only: callers supply the already-formatted label / accent /
// body. The chat timeline maps business data to these inputs (see
// `presentation::date_divider_label` and `presentation::system_event_kind`),
// so the components stay free of classification logic.

/// A centered, muted date divider for the chat timeline.
///
/// Compact 12 px typography on a quiet surface; used between message groups
/// when the day changes (e.g. "Today", "Yesterday", "Sunday, August 2, 2026").
/// Accepts any text fragment (borrowed `&str` or owned `String`) so callers
/// can pass freshly formatted labels without lifetime gymnastics.
pub fn date_separator<'a, Message: 'a>(
    label: impl text::IntoFragment<'a>,
    theme: &Theme,
) -> Element<'a, Message> {
    container(
        text(label)
            .font(TypeRole::Metadata.font())
            .size(TypeRole::Metadata.size_px())
            .color(design_tokens::text_muted(theme)),
    )
    .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

/// A centered, muted chip for informational system entries in the timeline.
///
/// Displays a compact category label (`label`, rendered in `accent`) followed
/// by the original event text (`body`). The muted secondary surface keeps the
/// event readable but visually secondary to user message bubbles. No mapping
/// logic — the caller decides the label/accent pair.
pub fn system_event_chip<'a, Message: 'a>(
    label: &'a str,
    accent: Color,
    body: &'a str,
    theme: &Theme,
) -> Element<'a, Message> {
    Row::new()
        .push(
            container(
                Row::new()
                    .push(
                        text(label)
                            .size(TypeRole::Metadata.size_px())
                            .font(crate::fonts::source_sans(iced::font::Weight::Semibold))
                            .color(accent),
                    )
                    .push(
                        text(body)
                            .size(TypeRole::Metadata.size_px())
                            .color(design_tokens::text_secondary(theme))
                            .wrapping(iced::widget::text::Wrapping::Word),
                    )
                    .spacing(design_tokens::SPACE_8)
                    .align_y(Alignment::Center),
            )
            .padding([design_tokens::SPACE_6, design_tokens::SPACE_12])
            .center_x(Length::Fill)
            .max_width(720.0)
            .style(move |t| container::Style {
                background: Some(Background::Color(design_tokens::surface_secondary(t))),
                border: Border {
                    color: accent.scale_alpha(0.45),
                    width: 1.0,
                    radius: design_tokens::SPACE_8.into(),
                },
                ..Default::default()
            }),
        )
        .width(Length::Fill)
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 15. TAB STRIP — horizontal tabs with active underline
// ═══════════════════════════════════════════════════════════════════════

/// A horizontal tab strip builder. Each tab is a `button` styled as a tab;
/// the active tab gets a primary underline and SemiBold text.
pub struct TabStrip<Message> {
    pub(crate) tabs: Vec<(String, Message)>,
    pub(crate) active_index: usize,
}

impl<'a, Message: Clone + 'a> TabStrip<Message> {
    /// Create a tab strip with labelled tabs, each with its own message.
    /// The first tab is active by default.
    pub fn new(tabs: Vec<(&'a str, Message)>) -> Self {
        Self {
            tabs: tabs
                .into_iter()
                .map(|(label, msg)| (label.to_string(), msg))
                .collect(),
            active_index: 0,
        }
    }

    /// Override which tab is active.
    pub fn active(mut self, index: usize) -> Self {
        self.active_index = index.min(self.tabs.len().saturating_sub(1));
        self
    }

    /// Build the tab strip element.
    pub fn build(self, _theme: &Theme) -> Element<'a, Message> {
        let active = self.active_index;

        let tab_row: Vec<Element<'a, Message>> = self
            .tabs
            .into_iter()
            .enumerate()
            .map(|(i, (label, msg))| {
                let is_active = i == active;

                let style_active = is_active;

                let btn = button(
                    text(label)
                        .font(if is_active {
                            TypeRole::ButtonLabel.font()
                        } else {
                            TypeRole::ButtonLabel.font()
                        })
                        .size(TypeRole::ButtonLabel.size_px()),
                )
                .on_press(msg)
                .padding([design_tokens::SPACE_4, design_tokens::SPACE_12])
                .style(move |t, status| tab_button_style(t, status, style_active));

                btn.into()
            })
            .collect();

        let tabs_row = Row::with_children(tab_row)
            .spacing(design_tokens::SPACE_16)
            .align_y(Alignment::Center);

        // Full-width underline separator below the tabs
        let separator = rule::horizontal(1).style(move |t| rule::Style {
            color: design_tokens::border_muted(t),
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: false,
        });

        container(
            Column::new()
                .push(tabs_row)
                .push(separator)
                .spacing(design_tokens::SPACE_8),
        )
        .padding([design_tokens::SPACE_8, design_tokens::SPACE_24])
        .width(Length::Fill)
        .into()
    }
}

/// Button style for a tab — switches between active (primary text, bottom border)
/// and inactive (muted text, no border). Hover on inactive shows primary text.
fn tab_button_style(theme: &Theme, status: button::Status, active: bool) -> button::Style {
    let text_color = if active {
        design_tokens::text_primary(theme)
    } else {
        match status {
            button::Status::Hovered => design_tokens::primary(theme),
            button::Status::Pressed => design_tokens::primary_pressed(theme),
            _ => design_tokens::text_secondary(theme),
        }
    };

    let bottom_border = if active {
        Border {
            color: design_tokens::primary(theme),
            width: 2.0,
            radius: 0.0.into(),
        }
    } else {
        Border::default()
    };

    button::Style {
        background: match status {
            button::Status::Hovered => Some(Background::Color(design_tokens::surface_hover(theme))),
            button::Status::Pressed => {
                Some(Background::Color(design_tokens::surface_selected(theme)))
            }
            _ => None,
        },
        text_color,
        border: bottom_border,
        ..Default::default()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 16. PROGRESS BAR — thin determinate + indeterminate
// ═══════════════════════════════════════════════════════════════════════

/// Kind of progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressKind {
    /// Normal determinate progress (percentage). Fill uses `primary`.
    Normal,
    /// Paused — fill uses `color_warning`.
    Paused,
    /// Error — fill uses `color_danger`.
    Error,
    /// Complete — fill uses `color_success`.
    Complete,
}

/// Builder for a thin progress bar with optional numeric label.
pub struct ProgressBar<'a, Message> {
    pub(crate) fraction: f32,
    pub(crate) kind: ProgressKind,
    pub(crate) indeterminate: bool,
    pub(crate) show_label: bool,
    pub(crate) height: f32,
    _phantom: std::marker::PhantomData<&'a Message>,
}

impl<'a, Message: 'a> ProgressBar<'a, Message> {
    /// Start a progress bar at the given fraction (0.0–1.0).
    /// Pass any fraction (including 0.0) for indeterminate — the bar ignores it.
    pub fn new(fraction: f32) -> Self {
        Self {
            fraction: fraction.clamp(0.0, 1.0),
            kind: ProgressKind::Normal,
            indeterminate: false,
            show_label: true,
            height: design_tokens::PROGRESS_BAR_HEIGHT,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Set the progress kind (Normal, Paused, Error, Complete).
    pub fn kind(mut self, kind: ProgressKind) -> Self {
        self.kind = kind;
        self
    }

    /// Enable indeterminate mode — renders a shimmer instead of a percentage fill.
    pub fn indeterminate(mut self, yes: bool) -> Self {
        self.indeterminate = yes;
        if yes {
            self.show_label = false;
        }
        self
    }

    /// Show or hide the percentage label.
    pub fn show_label(mut self, yes: bool) -> Self {
        self.show_label = yes;
        self
    }

    /// Use the bold (6 px) height variant.
    pub fn bold(mut self) -> Self {
        self.height = design_tokens::PROGRESS_BAR_HEIGHT_BOLD;
        self
    }

    /// Build the progress bar element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        let fill_color = match self.kind {
            ProgressKind::Normal => design_tokens::primary(theme),
            ProgressKind::Paused => design_tokens::color_warning(theme),
            ProgressKind::Error => design_tokens::color_danger(theme),
            ProgressKind::Complete => design_tokens::color_success(theme),
        };

        let track_color = design_tokens::border_muted(theme);

        let bar: Element<'_, Message> = if self.indeterminate {
            // Indeterminate: a shimmer-style background that suggests activity.
            // Iced 0.14 has no animation API, so we render a static gradient
            // that visually communicates "in progress."
            let shimmer = container(
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(self.height)),
            )
            .width(Length::FillPortion(4))
            .height(Length::Fixed(self.height))
            .style(move |t| container::Style {
                background: Some(Background::Color(design_tokens::primary_soft(t))),
                border: Border {
                    radius: (self.height / 2.0).into(),
                    ..Default::default()
                },
                ..Default::default()
            });

            container(shimmer)
                .width(Length::Fill)
                .height(Length::Fixed(self.height))
                .style(move |_t| container::Style {
                    background: Some(Background::Color(track_color)),
                    border: Border {
                        radius: (self.height / 2.0).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        } else {
            // Determinate: fill portion grows proportionally using FillPortion.
            let fill_portion = (self.fraction * 1000.0) as u16;
            let track_portion = 1000u16.saturating_sub(fill_portion);

            let mut row = Row::new().spacing(0);
            if fill_portion > 0 {
                row = row.push(
                    container(Space::new().width(Length::Fill).height(Length::Shrink))
                        .width(Length::FillPortion(fill_portion.max(1)))
                        .style(move |_t| container::Style {
                            background: Some(Background::Color(fill_color)),
                            border: Border {
                                radius: (self.height / 2.0).into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                );
            }
            if track_portion > 0 {
                row = row.push(
                    container(Space::new().width(Length::Fill).height(Length::Shrink))
                        .width(Length::FillPortion(track_portion.max(1)))
                        .style(move |t| container::Style {
                            background: Some(Background::Color(track_color)),
                            border: Border {
                                radius: (self.height / 2.0).into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                );
            }

            container(row)
                .width(Length::Fill)
                .height(Length::Fixed(self.height))
                .into()
        };

        if self.show_label && !self.indeterminate {
            let pct = (self.fraction * 100.0) as u32;
            let label = text(format!("{pct}%"))
                .font(TypeRole::Metadata.font())
                .size(TypeRole::Metadata.size_px())
                .color(design_tokens::text_secondary(theme));

            Row::new()
                .push(container(bar).width(Length::Fill))
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_8))
                        .height(Length::Shrink),
                )
                .push(label)
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .into()
        } else {
            bar
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 17. FILE IDENTITY CELL — icon + primary name + secondary metadata
// ═══════════════════════════════════════════════════════════════════════

/// A compact identity cell for a file or folder: a file-type icon, a primary
/// name (truncated), and a secondary metadata line.
///
/// PAPIRUS-13: the leading icon is a caller-supplied [`Element`]. File
/// surfaces MUST pass the central Papirus file-type icon (see
/// [`crate::download_progress_view::file_type_icon_element`] /
/// `directory_icon_element`), never a Lucide `Icon` — the icon answers "what
/// type of file is this?", and status is shown separately by the caller.
pub struct FileIdentityCell<'a, Message> {
    icon: Element<'a, Message>,
    name: &'a str,
    metadata: &'a str,
    _phantom: std::marker::PhantomData<Message>,
}

impl<'a, Message: 'a> FileIdentityCell<'a, Message> {
    /// Start a file identity cell with a pre-built icon element.
    ///
    /// The icon must come from the central Papirus file-type component so
    /// every surface shows the same full-colour icon for the same file.
    pub fn new(icon: Element<'a, Message>, name: &'a str, metadata: &'a str) -> Self {
        Self {
            icon,
            name,
            metadata,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Build the cell element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        let icon_el = self.icon;

        let name_el = text(self.name)
            .font(TypeRole::Body.font())
            .size(TypeRole::Body.size_px())
            .color(design_tokens::text_primary(theme))
            .width(Length::Fill);

        let meta_el = text(self.metadata)
            .font(TypeRole::SupportingText.font())
            .size(TypeRole::SupportingText.size_px())
            .color(design_tokens::text_secondary(theme))
            .width(Length::Fill);

        Row::new()
            .push(icon_el)
            .push(
                Space::new()
                    .width(Length::Fixed(design_tokens::SPACE_12))
                    .height(Length::Shrink),
            )
            .push(
                Column::new()
                    .push(name_el)
                    .push(meta_el)
                    .spacing(design_tokens::SPACE_4)
                    .width(Length::Fill),
            )
            .spacing(0)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 18. PEER CHIP STACK — avatar/list with +N overflow
// ═══════════════════════════════════════════════════════════════════════

/// Builder for a stack of peer chips with overflow count.
pub struct PeerChipStack<'a, Message> {
    peers: Vec<&'a str>,
    max_visible: usize,
    _phantom: std::marker::PhantomData<Message>,
}

impl<'a, Message: 'a> PeerChipStack<'a, Message> {
    /// Start with a list of peer display names.
    pub fn new(peers: Vec<&'a str>) -> Self {
        Self {
            peers,
            max_visible: 3,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Override how many chips are visible before the "+N" overflow.
    pub fn max_visible(mut self, n: usize) -> Self {
        self.max_visible = n;
        self
    }

    /// Build the chip stack element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        let total = self.peers.len();
        let visible = if total > self.max_visible {
            self.max_visible
        } else {
            total
        };

        let mut row = Row::new()
            .spacing(design_tokens::SPACE_4)
            .align_y(Alignment::Center);

        for name in self.peers.iter().take(visible) {
            let truncated: String = if name.len() > 12 {
                format!("{}…", &name[..11])
            } else {
                name.to_string()
            };
            row = row.push(peer_chip::<Message>(&truncated));
        }

        if total > self.max_visible {
            let overflow = total - self.max_visible;
            row = row.push(peer_chip::<Message>(&format!("+{overflow} more")));
        }

        row.into()
    }
}

/// A single peer chip — a small, rounded pill with peer name.
fn peer_chip<'a, Message: 'a>(name: &str) -> Element<'a, Message> {
    let name_owned = name.to_string();
    container(
        text(name_owned)
            .font(TypeRole::Metadata.font())
            .size(TypeRole::Metadata.size_px()),
    )
    .padding([design_tokens::SPACE_2, design_tokens::SPACE_8])
    .height(Length::Fixed(design_tokens::CHIP_HEIGHT))
    .align_y(Alignment::Center)
    .style(move |t| container::Style {
        background: Some(Background::Color(design_tokens::surface(t))),
        text_color: Some(design_tokens::text_primary(t)),
        border: Border {
            color: design_tokens::border_muted(t),
            width: 1.0,
            radius: design_tokens::RADIUS_MD.into(),
        },
        ..Default::default()
    })
    .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 19. METRIC BLOCK — big number + label for summary cards
// ═══════════════════════════════════════════════════════════════════════

/// A compact metric display: a large value and a small label below it.
pub struct MetricBlock<'a, Message> {
    value: &'a str,
    label: &'a str,
    accent: Option<fn(&Theme) -> Color>,
    _phantom: std::marker::PhantomData<Message>,
}

impl<'a, Message: 'a> MetricBlock<'a, Message> {
    /// Start a metric block.
    pub fn new(value: &'a str, label: &'a str) -> Self {
        Self {
            value,
            label,
            accent: None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Apply an accent colour to the value.
    pub fn accent(mut self, color_fn: fn(&Theme) -> Color) -> Self {
        self.accent = Some(color_fn);
        self
    }

    /// Build the metric element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        let value_color = self.accent.unwrap_or(design_tokens::text_primary);

        Column::new()
            .push(
                text(self.value)
                    .font(TypeRole::CardTitle.font())
                    .size(TypeRole::CardTitle.size_px())
                    .color(value_color(theme)),
            )
            .push(
                text(self.label)
                    .font(TypeRole::Metadata.font())
                    .size(TypeRole::Metadata.size_px())
                    .color(design_tokens::text_secondary(theme)),
            )
            .spacing(design_tokens::SPACE_2)
            .align_x(Alignment::Center)
            .into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 20. LOADING SKELETON — placeholder shimmer for loading state
// ═══════════════════════════════════════════════════════════════════════

/// A loading skeleton placeholder: a row of pulsing-styled placeholder blocks
/// that match the dimensions of real content.
pub struct LoadingSkeleton<Message> {
    row_count: usize,
    row_height: f32,
    _phantom: std::marker::PhantomData<Message>,
}

impl<Message: 'static> LoadingSkeleton<Message> {
    /// Create a skeleton with the given number of placeholder rows.
    pub fn new(row_count: usize) -> Self {
        Self {
            row_count,
            row_height: design_tokens::TABLE_ROW_HEIGHT,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Set a custom row height.
    pub fn row_height(mut self, height: f32) -> Self {
        self.row_height = height;
        self
    }

    /// Build the skeleton element.
    pub fn build(self, theme: &Theme) -> Element<'static, Message> {
        let mut col = Column::new().spacing(design_tokens::SPACE_4);

        for _ in 0..self.row_count {
            let row = skeleton_row::<Message>(self.row_height, theme);
            col = col.push(row);
        }

        container(col)
            .width(Length::Fill)
            .padding(design_tokens::SPACE_12)
            .into()
    }
}

fn skeleton_row<Message: 'static>(height: f32, theme: &Theme) -> Element<'static, Message> {
    let skeleton_bg = design_tokens::surface_hover(theme);
    let icon_size = 24.0;

    let icon_placeholder = container(
        Space::new()
            .width(Length::Fixed(icon_size))
            .height(Length::Fixed(icon_size)),
    )
    .width(Length::Fixed(icon_size))
    .height(Length::Fixed(icon_size))
    .style(move |_t| container::Style {
        background: Some(Background::Color(skeleton_bg)),
        border: Border {
            radius: design_tokens::RADIUS_SM.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let text_line_1 = container(Space::new().width(Length::Fill).height(Length::Fixed(12.0)))
        .width(Length::FillPortion(3))
        .height(Length::Fixed(12.0))
        .style(move |_t| container::Style {
            background: Some(Background::Color(skeleton_bg)),
            border: Border {
                radius: design_tokens::RADIUS_SM.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    let text_line_2 = container(Space::new().width(Length::Fill).height(Length::Fixed(10.0)))
        .width(Length::FillPortion(2))
        .height(Length::Fixed(10.0))
        .style(move |_t| container::Style {
            background: Some(Background::Color(skeleton_bg)),
            border: Border {
                radius: design_tokens::RADIUS_SM.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    let text_col = Column::new()
        .push(text_line_1)
        .push(text_line_2)
        .spacing(design_tokens::SPACE_4)
        .width(Length::Fill);

    container(
        Row::new()
            .push(icon_placeholder)
            .push(
                Space::new()
                    .width(Length::Fixed(design_tokens::SPACE_12))
                    .height(Length::Shrink),
            )
            .push(text_col)
            .align_y(Alignment::Center)
            .width(Length::Fill),
    )
    .height(Length::Fixed(height))
    .width(Length::Fill)
    .align_y(Alignment::Center)
    .into()
}

// ═══════════════════════════════════════════════════════════════════════
// 21. INLINE ERROR — error text with optional retry action
// ═══════════════════════════════════════════════════════════════════════

/// An inline error message with an optional retry button.
///
/// The message is owned (`String`) so `build` can return a fully `'static`
/// element — required when the error is rendered inside an
/// `iced::widget::lazy` content builder.
pub struct InlineError {
    message: String,
    retry_msg: Option<AppMessage>,
}

impl InlineError {
    /// Create an inline error with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_msg: None,
        }
    }

    /// Add a retry action button.
    pub fn on_retry(mut self, msg: AppMessage) -> Self {
        self.retry_msg = Some(msg);
        self
    }

    /// Build the error element.
    pub fn build(self, theme: &Theme) -> Element<'static, AppMessage> {
        let icon = Icon::AlertTriangle
            .build()
            .size(IconSize::Xs)
            .color_fn(design_tokens::color_danger)
            .build();

        let msg_text = text(self.message)
            .font(TypeRole::SupportingText.font())
            .size(TypeRole::SupportingText.size_px())
            .color(design_tokens::color_danger(theme));

        let mut row = Row::new()
            .push(icon)
            .push(
                Space::new()
                    .width(Length::Fixed(design_tokens::SPACE_8))
                    .height(Length::Shrink),
            )
            .push(msg_text)
            .spacing(0)
            .align_y(Alignment::Center);

        if let Some(retry) = self.retry_msg {
            row = row.push(
                Space::new()
                    .width(Length::Fixed(design_tokens::SPACE_8))
                    .height(Length::Shrink),
            );
            row = row.push(
                button(
                    text("Retry")
                        .font(TypeRole::ButtonLabel.font())
                        .size(TypeRole::ButtonLabel.size_px()),
                )
                .on_press(retry)
                .padding([design_tokens::SPACE_4, design_tokens::SPACE_8])
                .style(move |t, status| {
                    let color = match status {
                        button::Status::Hovered => design_tokens::color_danger(t),
                        button::Status::Pressed => {
                            let mut c = design_tokens::color_danger(t);
                            c.r *= 0.85;
                            c.g *= 0.85;
                            c.b *= 0.85;
                            c
                        }
                        _ => design_tokens::text_secondary(t),
                    };
                    button::Style {
                        background: None,
                        text_color: color,
                        border: Border {
                            color,
                            width: 1.0,
                            radius: design_tokens::RADIUS_SM.into(),
                        },
                        ..Default::default()
                    }
                }),
            );
        }

        container(row)
            .padding([design_tokens::SPACE_8, 0.0])
            .width(Length::Fill)
            .into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 22. TABLE HEADER ROW — column labels for data tables
// ═══════════════════════════════════════════════════════════════════════

/// A table header row with column labels. Data-agnostic — callers provide
/// the column labels and optional widths.
pub struct TableHeaderRow<'a, Message> {
    columns: Vec<(&'a str, Option<f32>)>, // (label, optional fixed width in px)
    _phantom: std::marker::PhantomData<Message>,
}

impl<'a, Message: 'a> TableHeaderRow<'a, Message> {
    /// Create a header row. Each entry is (label, optional_fixed_width).
    /// Pass `None` for width to use `Length::Fill`.
    pub fn new(columns: Vec<(&'a str, Option<f32>)>) -> Self {
        Self {
            columns,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Build the header row element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        let mut row = Row::new()
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center);

        for (label, width) in self.columns {
            let label_el = text(label)
                .font(TypeRole::Metadata.font())
                .size(TypeRole::Metadata.size_px())
                .color(design_tokens::text_muted(theme));

            let w = match width {
                Some(px) => Length::Fixed(px),
                None => Length::Fill,
            };

            row = row.push(container(label_el).width(w));
        }

        container(row)
            .padding([design_tokens::SPACE_8, design_tokens::SPACE_12])
            .width(Length::Fill)
            .style(move |t| container::Style {
                border: Border {
                    color: design_tokens::border_muted(t),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 23. OVERFLOW MENU — trailing kebab action menu anchor
// ═══════════════════════════════════════════════════════════════════════

/// A trailing overflow menu anchor — renders the MoreVertical kebab icon as
/// a ghost button. The actual dropdown menu is owner-rendered (the anchor
/// just dispatches the toggle message).
pub struct OverflowMenu {}

impl OverflowMenu {
    /// Build the kebab anchor button that dispatches `on_toggle` on press.
    pub fn build<'a>(
        on_toggle: AppMessage,
        disabled: bool,
        theme: &Theme,
    ) -> Element<'a, AppMessage> {
        let _ = theme;
        let icon = Icon::MoreVertical
            .build()
            .size(IconSize::Sm)
            .interactive(!disabled)
            .build();

        let btn = button(icon)
            .padding(design_tokens::SPACE_8)
            .style(move |t, status| overflow_menu_button_style(t, status));

        if disabled {
            btn.into()
        } else {
            btn.on_press(on_toggle).into()
        }
    }
}

fn overflow_menu_button_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: match status {
            button::Status::Hovered => Some(Background::Color(design_tokens::surface_hover(theme))),
            button::Status::Pressed => {
                Some(Background::Color(design_tokens::surface_selected(theme)))
            }
            _ => None,
        },
        text_color: match status {
            button::Status::Hovered => design_tokens::primary(theme),
            button::Status::Pressed => design_tokens::primary_pressed(theme),
            button::Status::Disabled => design_tokens::text_muted(theme),
            _ => design_tokens::text_secondary(theme),
        },
        border: Border {
            radius: design_tokens::SPACE_8.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 24. CONNECTIVITY NOTICE — non-blocking offline / stale-data banner
// ═══════════════════════════════════════════════════════════════════════

/// Severity level for connectivity notices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NoticeSeverity {
    /// Local node is offline — no network at all.
    Offline,
    /// Data may be stale — cached values shown, network is recovering.
    Stale,
    /// Warning about a specific condition (e.g. storage degraded).
    Warning,
}

/// A non-blocking banner that sits at the top of a tab region to communicate
/// connectivity or freshness state without blocking the rest of the UI.
///
/// Unlike [`InlineError`], this banner is dismissible and does not take over
/// the entire content area — usable regions remain accessible.
pub(crate) struct ConnectivityNotice {
    message: String,
    severity: NoticeSeverity,
    dismiss_msg: Option<AppMessage>,
}

impl ConnectivityNotice {
    /// Create a connectivity notice with a severity and message.
    pub fn new(severity: NoticeSeverity, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            severity,
            dismiss_msg: None,
        }
    }

    /// Add a dismiss action. When `None`, the banner is persistent.
    pub fn on_dismiss(mut self, msg: AppMessage) -> Self {
        self.dismiss_msg = Some(msg);
        self
    }

    /// Build the notice element.
    pub fn build(self, theme: &Theme) -> Element<'static, AppMessage> {
        let (bg_color, border_color, text_color, icon) = match self.severity {
            NoticeSeverity::Offline => (
                design_tokens::color_danger(theme).scale_alpha(0.07),
                design_tokens::color_danger(theme).scale_alpha(0.25),
                design_tokens::color_danger(theme),
                Icon::AlertTriangle,
            ),
            NoticeSeverity::Stale => (
                design_tokens::color_warning(theme).scale_alpha(0.07),
                design_tokens::color_warning(theme).scale_alpha(0.25),
                design_tokens::color_warning(theme),
                Icon::Activity,
            ),
            NoticeSeverity::Warning => (
                design_tokens::color_warning(theme).scale_alpha(0.07),
                design_tokens::color_warning(theme).scale_alpha(0.25),
                design_tokens::color_warning(theme),
                Icon::AlertTriangle,
            ),
        };

        // color_fn requires a non-capturing fn pointer; select the token
        // function per severity instead of capturing the computed color.
        let icon_color: fn(&iced::Theme) -> iced::Color = match self.severity {
            NoticeSeverity::Offline => design_tokens::color_danger,
            NoticeSeverity::Stale | NoticeSeverity::Warning => design_tokens::color_warning,
        };
        let icon_el = icon
            .build()
            .size(IconSize::Xs)
            .color_fn(icon_color)
            .build();

        let msg_text = text(self.message)
            .font(TypeRole::SupportingText.font())
            .size(TypeRole::SupportingText.size_px())
            .color(text_color);

        let mut row = Row::new()
            .push(icon_el)
            .push(
                Space::new()
                    .width(Length::Fixed(design_tokens::SPACE_8))
                    .height(Length::Shrink),
            )
            .push(msg_text)
            .push(Space::new().width(Length::Fill))
            .spacing(0)
            .align_y(Alignment::Center);

        if let Some(dismiss) = self.dismiss_msg {
            row = row.push(
                button(
                    text("\u{2715}")
                        .font(TypeRole::ButtonLabel.font())
                        .size(TypeRole::ButtonLabel.size_px()),
                )
                .on_press(dismiss)
                .padding([design_tokens::SPACE_2, design_tokens::SPACE_8])
                .style(move |_t, status| {
                    let color = match status {
                        button::Status::Hovered => text_color,
                        _ => text_color.scale_alpha(0.6),
                    };
                    button::Style {
                        background: None,
                        text_color: color,
                        border: Border::default(),
                        ..Default::default()
                    }
                }),
            );
        }

        container(row)
            .padding([design_tokens::SPACE_6, design_tokens::SPACE_12])
            .width(Length::Fill)
            .style(move |_t| container::Style {
                background: Some(Background::Color(bg_color)),
                border: Border {
                    color: border_color,
                    width: 1.0,
                    radius: design_tokens::RADIUS_SM.into(),
                },
                ..Default::default()
            })
            .into()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 25. FILE THUMBNAIL — uniform-size preview for image/video rows
// ═══════════════════════════════════════════════════════════════════════

/// Uniform thumbnail box edge (px). Every image/video file row renders its
/// preview at this exact size, so thumbnails never vary by content.
pub(crate) const FILE_THUMBNAIL_EDGE: f32 = 40.0;

/// Render a uniform-size thumbnail preview for a picture or video file.
///
/// When `handle` is present the image is drawn with `ContentFit::Cover` so
/// the preview fills the fixed box regardless of the source aspect ratio.
/// When the handle is absent (still loading, unsupported, or non-media) the
/// caller-supplied fallback element is centred inside the same box, keeping
/// every row the same height.
///
/// PAPIRUS-13: the fallback must be the central Papirus file-type icon
/// (see [`crate::download_progress_view::file_type_icon_element`]) — never a
/// Lucide `Icon` — so a missing preview still answers "what type of file is
/// this?" with the same full-colour icon every other surface uses.
pub(crate) fn file_thumbnail(
    handle: Option<&iced::widget::image::Handle>,
    fallback: Element<'static, AppMessage>,
    _theme: &Theme,
) -> Element<'static, AppMessage> {
    let content: Element<'static, AppMessage> = match handle {
        Some(handle) => iced::widget::image(handle.clone())
            .width(Length::Fixed(FILE_THUMBNAIL_EDGE))
            .height(Length::Fixed(FILE_THUMBNAIL_EDGE))
            .content_fit(iced::ContentFit::Cover)
            .into(),
        None => container(fallback)
            .width(Length::Fixed(FILE_THUMBNAIL_EDGE))
            .height(Length::Fixed(FILE_THUMBNAIL_EDGE))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |t| container::Style {
                background: Some(Background::Color(design_tokens::surface_hover(t))),
                border: Border {
                    radius: design_tokens::RADIUS_SM.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into(),
    };

    container(content)
        .width(Length::Fixed(FILE_THUMBNAIL_EDGE))
        .height(Length::Fixed(FILE_THUMBNAIL_EDGE))
        .style(move |t| container::Style {
            border: Border {
                color: design_tokens::border_muted(t),
                radius: design_tokens::RADIUS_SM.into(),
                width: 1.0,
            },
            ..Default::default()
        })
        .into()
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract the production (non-test) source of one method body.
    fn method_source<'a>(src: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
        let start = src
            .find(start_marker)
            .unwrap_or_else(|| panic!("{start_marker} must exist"));
        let tests_start = src.find("#[cfg(test)]").unwrap_or(src.len());
        let end = src[start..tests_start]
            .find(end_marker)
            .map(|off| start + off)
            .unwrap_or(tests_start);
        &src[start..end]
    }

    #[test]
    fn chat_status_footer_connected_with_peer() {
        let el: Element<'static, AppMessage> = chat_status_footer(
            "Direct (mesh)".to_string(),
            true,
            Some("1 peer".to_string()),
        );
        let _ = el;
    }

    #[test]
    fn chat_status_footer_connected_without_peer() {
        let el: Element<'static, AppMessage> = chat_status_footer("Relay".to_string(), true, None);
        let _ = el;
    }

    #[test]
    fn chat_status_footer_disconnected() {
        let el: Element<'static, AppMessage> =
            chat_status_footer("Not connected".to_string(), false, None);
        let _ = el;
    }

    #[test]
    fn chat_status_footer_uses_plex_metadata_role() {
        // FONTS-08: the chat footer status line is chrome — it must resolve
        // through the central TypeRole::Metadata role (IBM Plex Sans), not
        // the Source Sans app default or a raw size literal.
        let src = include_str!("ui_components.rs");
        let footer = method_source(src, "fn chat_status_footer<'a>(", "fn white_color(");
        assert!(
            footer.contains("TypeRole::Metadata"),
            "chat footer status text must use TypeRole::Metadata (IBM Plex Sans)"
        );
        assert!(
            !footer.contains("source_sans("),
            "chat footer must not use Source Sans directly"
        );
    }

    #[test]
    fn date_separator_uses_plex_metadata_font() {
        // FONTS-08: date dividers in the chat timeline are chrome — they must
        // render in IBM Plex Sans via TypeRole::Metadata, not the app default.
        let src = include_str!("ui_components.rs");
        let sep = method_source(src, "fn date_separator<'a,", "fn system_event_chip(");
        assert!(
            sep.contains("TypeRole::Metadata.font()"),
            "date separator must use TypeRole::Metadata.font() (IBM Plex Sans)"
        );
    }

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

    #[test]
    fn sidebar_section_header_zero_count_is_suppressed() {
        let header: SidebarSectionHeader = SidebarSectionHeader::new("CHATS").count(0);
        assert_eq!(header.count, None, "zero count must not render a badge");
        let header = SidebarSectionHeader::new("CHATS").count(7);
        assert_eq!(header.count, Some(7));
    }

    #[test]
    fn sidebar_section_header_defaults_to_expanded_chevron() {
        let header: SidebarSectionHeader = SidebarSectionHeader::new("FRIENDS");
        assert!(!header.collapsed, "headers start expanded");
        let header = SidebarSectionHeader::new("FRIENDS").collapsed(true);
        assert!(header.collapsed);
    }

    #[test]
    fn sidebar_section_header_stores_add_action() {
        let header = SidebarSectionHeader::new("PUBLIC ROOMS")
            .add_action(Icon::Plus, AppMessage::CreateNewRoom);
        // AppMessage is not PartialEq; verify the icon and presence instead.
        match header.add_action {
            Some((icon, _msg)) => assert_eq!(icon, Icon::Plus),
            None => panic!("expected an add action to be stored"),
        }
    }

    #[test]
    fn sidebar_section_header_builds_with_toggle() {
        // Regression: `on_toggle` was stored but never attached as
        // `on_press`, so the header rendered as an inert button and sidebar
        // sections could not be collapsed.  Building with a toggle must not
        // panic and must produce an element (the button now carries a press
        // handler).
        let el: Element<'static, AppMessage> = SidebarSectionHeader::new("CHATS")
            .collapsed(true)
            .on_toggle(AppMessage::ToggleSidebarSectionCollapsed(0))
            .build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn sidebar_empty_state_accepts_no_action() {
        let el: Element<'static, AppMessage> =
            sidebar_empty_state(Icon::Search, "No peers", "They will appear here.", None);
        // Building should not panic; the element is opaque so we only verify
        // the builder produces an element.
        let _ = el;
    }

    #[test]
    fn date_separator_builds_with_sample_label() {
        // Presentational smoke test: the component must render for a sample
        // label without panicking and return an element.
        let el: Element<'static, AppMessage> = date_separator("Today", &Theme::Light);
        let _ = el;
    }

    #[test]
    fn system_event_chip_builds_with_sample_data() {
        // Presentational smoke test for every accent the timeline uses.
        let theme = Theme::Light;
        for (label, accent) in [
            ("MEMBER", design_tokens::online(&theme)),
            ("NAME", design_tokens::primary(&theme)),
            ("HELP", design_tokens::text_muted(&theme)),
            ("NOTICE", design_tokens::color_warning(&theme)),
            ("INFO", design_tokens::text_muted(&theme)),
        ] {
            let el: Element<'static, AppMessage> =
                system_event_chip(label, accent, "Sample system event body", &theme);
            let _ = el;
        }
    }

    // ── FS-07 Dashboard component tests ──────────────────────────────

    #[test]
    fn tab_strip_stores_tabs() {
        let tabs: Vec<(&str, AppMessage)> = vec![
            ("Tab 1", AppMessage::Noop),
            ("Tab 2", AppMessage::Noop),
        ];
        let strip = TabStrip::<AppMessage>::new(tabs);
        assert_eq!(strip.tabs.len(), 2);
        assert_eq!(strip.active_index, 0);
    }

    #[test]
    fn tab_strip_active_clamped() {
        let tabs: Vec<(&str, AppMessage)> = vec![("A", AppMessage::Noop)];
        let strip = TabStrip::<AppMessage>::new(tabs).active(5);
        assert_eq!(strip.active_index, 0); // clamped to 0
    }

    #[test]
    fn tab_strip_builds_without_panic() {
        let tabs: Vec<(&str, AppMessage)> = vec![
            ("First", AppMessage::Noop),
            ("Second", AppMessage::Noop),
            ("Third", AppMessage::Noop),
        ];
        let el: Element<'static, AppMessage> =
            TabStrip::<AppMessage>::new(tabs).active(1).build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn progress_bar_fraction_clamped() {
        let pb = ProgressBar::<AppMessage>::new(1.5);
        assert_eq!(pb.fraction, 1.0);
        let pb = ProgressBar::<AppMessage>::new(-0.5);
        assert_eq!(pb.fraction, 0.0);
    }

    #[test]
    fn progress_bar_defaults() {
        let pb = ProgressBar::<AppMessage>::new(0.5);
        assert_eq!(pb.kind, ProgressKind::Normal);
        assert!(!pb.indeterminate);
        assert!(pb.show_label);
        assert_eq!(pb.height, design_tokens::PROGRESS_BAR_HEIGHT);
    }

    #[test]
    fn progress_bar_kinds_are_distinct() {
        let kinds = [
            ProgressKind::Normal,
            ProgressKind::Paused,
            ProgressKind::Error,
            ProgressKind::Complete,
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
    fn progress_bar_builds_all_states() {
        let theme = Theme::Light;
        for (fraction, kind) in [
            (0.0, ProgressKind::Normal),
            (0.45, ProgressKind::Normal),
            (0.5, ProgressKind::Paused),
            (1.0, ProgressKind::Complete),
            (0.75, ProgressKind::Error),
        ] {
            let el: Element<'static, AppMessage> = ProgressBar::<AppMessage>::new(fraction)
                .kind(kind)
                .build(&theme);
            let _ = el;
        }
    }

    #[test]
    fn progress_bar_indeterminate_builds() {
        let theme = Theme::Light;
        let el: Element<'static, AppMessage> =
            ProgressBar::<AppMessage>::new(0.0).indeterminate(true).build(&theme);
        let _ = el;
    }

    #[test]
    fn progress_bar_bold_builds() {
        let theme = Theme::Light;
        let el: Element<'static, AppMessage> =
            ProgressBar::<AppMessage>::new(0.6).bold().build(&theme);
        let _ = el;
    }

    #[test]
    fn file_identity_cell_builds() {
        let theme = Theme::Light;
        // PAPIRUS-13: the identity cell's icon must come from the central
        // Papirus component (never a Lucide Icon) so the file surface shows
        // the same full-colour type icon as every other surface.
        let icon: Element<'static, AppMessage> =
            crate::download_progress_view::file_type_icon_element(
                "report.pdf",
                Some("application/pdf"),
                None,
                crate::file_type_icon::FileTypeIconSize::List,
                &theme,
            );
        let el: Element<'static, AppMessage> =
            FileIdentityCell::<AppMessage>::new(icon, "report.pdf", "application/pdf · 2.4 MB")
                .build(&theme);
        let _ = el;
    }

    #[test]
    fn file_identity_cell_long_name() {
        let theme = Theme::Light;
        let icon: Element<'static, AppMessage> =
            crate::download_progress_view::file_type_icon_element(
                "AVeryLongFileNameThatCouldExceedTheAvailableSpaceInTheTableRow.jpg",
                Some("image/jpeg"),
                None,
                crate::file_type_icon::FileTypeIconSize::List,
                &theme,
            );
        let el: Element<'static, AppMessage> = FileIdentityCell::<AppMessage>::new(
            icon,
            "AVeryLongFileNameThatCouldExceedTheAvailableSpaceInTheTableRow.jpg",
            "image/jpeg · 15 MB",
        )
        .build(&theme);
        let _ = el;
    }

    #[test]
    fn peer_chip_stack_no_overflow() {
        let peers: Vec<&str> = vec!["Alice", "Bob"];
        let stack = PeerChipStack::<AppMessage>::new(peers);
        let el: Element<'static, AppMessage> = stack.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn peer_chip_stack_with_overflow() {
        let peers: Vec<&str> = vec!["Alice", "Bob", "Carol", "Dave", "Eve"];
        let el: Element<'static, AppMessage> =
            PeerChipStack::<AppMessage>::new(peers)
                .max_visible(3)
                .build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn peer_chip_stack_empty() {
        let peers: Vec<&str> = vec![];
        let el: Element<'static, AppMessage> =
            PeerChipStack::<AppMessage>::new(peers).build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn metric_block_default() {
        let el: Element<'static, AppMessage> =
            MetricBlock::<AppMessage>::new("42", "files").build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn metric_block_accented() {
        let el: Element<'static, AppMessage> = MetricBlock::<AppMessage>::new("2.4 GB", "data")
            .accent(design_tokens::primary)
            .build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn loading_skeleton_builds() {
        let el: Element<'static, AppMessage> =
            LoadingSkeleton::<AppMessage>::new(5).build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn loading_skeleton_compact() {
        let el: Element<'static, AppMessage> = LoadingSkeleton::<AppMessage>::new(3)
            .row_height(design_tokens::TABLE_ROW_HEIGHT_COMPACT)
            .build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn inline_error_message_only() {
        let el: Element<'static, AppMessage> =
            InlineError::new("Something went wrong.").build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn inline_error_with_retry() {
        let el: Element<'static, AppMessage> = InlineError::new("Transfer failed.")
            .on_retry(AppMessage::Noop)
            .build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn table_header_row_builds() {
        let el: Element<'static, AppMessage> = TableHeaderRow::<AppMessage>::new(vec![
            ("Name", None),
            ("Kind", Some(100.0)),
            ("Size", Some(80.0)),
        ])
        .build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn overflow_menu_normal() {
        let el: Element<'static, AppMessage> =
            OverflowMenu::build(AppMessage::Noop, false, &Theme::Light);
        let _ = el;
    }

    #[test]
    fn overflow_menu_disabled() {
        let el: Element<'static, AppMessage> =
            OverflowMenu::build(AppMessage::Noop, true, &Theme::Light);
        let _ = el;
    }

    #[test]
    fn file_thumbnail_without_handle_builds_uniform_box() {
        // PAPIRUS-13: the fallback must be the central Papirus file-type
        // icon, never a Lucide Icon.
        let fallback: Element<'static, AppMessage> =
            crate::download_progress_view::file_type_icon_element(
                "photo.png",
                Some("image/png"),
                None,
                crate::file_type_icon::FileTypeIconSize::List,
                &Theme::Light,
            );
        let el: Element<'static, AppMessage> = file_thumbnail(None, fallback, &Theme::Light);
        let _ = el;
    }

    #[test]
    fn file_thumbnail_with_handle_builds_uniform_box() {
        let handle = iced::widget::image::Handle::from_bytes(vec![0xFF, 0xD8]);
        let fallback: Element<'static, AppMessage> =
            crate::download_progress_view::file_type_icon_element(
                "photo.png",
                Some("image/png"),
                None,
                crate::file_type_icon::FileTypeIconSize::List,
                &Theme::Light,
            );
        let el = file_thumbnail(Some(&handle), fallback, &Theme::Light);
        let _ = el;
    }
}
