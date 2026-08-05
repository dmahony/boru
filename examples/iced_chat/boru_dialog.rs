//! Reusable modal dialog shell for the Boru desktop UI.
//!
//! `BoruDialog` is the shared chrome used by the creation flows (Create New
//! Room / Public Chat, Create Group Chat, Create Tunnel) and by any other
//! surface that needs a centred modal: header (title + optional subtitle +
//! close button), body (form content, internally scrollable when long), and
//! footer (secondary + primary actions).
//!
//! The component is deliberately generic over `Message` (mirroring
//! `connection_details::view`) so it never imports `AppMessage` — callers map
//! their own messages onto the dialog actions. All styling comes from
//! `design_tokens` (dialog surface, shadow, radius, backdrop) and
//! `ui_components` button styles; no hard-coded colours live here.
//!
//! Usage:
//! ```ignore
//! let overlay = BoruDialog::new("Create Group Chat")
//!     .subtitle("Start a private group conversation")
//!     .push_body(text_input_field("Group name…", &name, on_input, false))
//!     .secondary("Cancel", AppMessage::HideCreateGroupDialog)
//!     .primary("Create", AppMessage::ConfirmCreateGroup)
//!     .on_close(AppMessage::HideCreateGroupDialog)
//!     .width(560.0)
//!     .build(&theme);
//!
//! iced::widget::stack![base, overlay].into()
//! ```

use iced::widget::{button, container, text, Column, Row, Space};
use iced::{Alignment, Background, Element, Length, Theme};

use crate::design_tokens;
use crate::fonts::TypeRole;
use crate::icon_system::{Icon, IconSize};
use crate::ui_components::{button_primary_style, button_secondary_style};

/// Standard dialog width for forms — inside the 520–680 px desktop band.
pub const BORU_DIALOG_WIDTH_STANDARD: f32 = 560.0;

/// Larger dialog width for forms that need more room (member lists, etc.).
pub const BORU_DIALOG_WIDTH_LARGE: f32 = 760.0;

/// Reusable modal dialog shell.
pub struct BoruDialog<'a, Message> {
    title: &'a str,
    subtitle: Option<&'a str>,
    body: Vec<Element<'a, Message>>,
    width: f32,
    /// When set, the body is wrapped in a `scrollable` capped at this height
    /// instead of letting the dialog grow unbounded.
    max_body_height: Option<f32>,
    /// Close button in the header (optional).
    on_close: Option<Message>,
    /// Secondary (Cancel) footer action: `(label, message)`.
    secondary: Option<(&'a str, Message)>,
    /// Whether the secondary footer action is enabled (defaults to true).
    /// Disabled buttons render without `on_press` (iced disabled state).
    secondary_enabled: bool,
    /// Primary (Create / Continue / Start / Save) footer action: `(label, message)`.
    primary: Option<(&'a str, Message)>,
    /// Whether the primary footer action is enabled (defaults to true).
    /// Callers set this to `false` until required inputs are valid, and while
    /// a submit is in flight, so the button renders disabled.
    primary_enabled: bool,
    /// When set, clicking the dimmed backdrop emits this message.
    on_backdrop: Option<Message>,
}

impl<'a, Message: Clone + 'a> BoruDialog<'a, Message> {
    /// Start a dialog with the given header title.
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            subtitle: None,
            body: Vec::new(),
            width: BORU_DIALOG_WIDTH_STANDARD,
            max_body_height: None,
            on_close: None,
            secondary: None,
            secondary_enabled: true,
            primary: None,
            primary_enabled: true,
            on_backdrop: None,
        }
    }

    /// Set an optional subtitle shown under the title in the header.
    pub fn subtitle(mut self, subtitle: &'a str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    /// Append one body element (form field, section, notice, …).
    pub fn push_body(mut self, element: Element<'a, Message>) -> Self {
        self.body.push(element);
        self
    }

    /// Override the dialog width. Defaults to [`BORU_DIALOG_WIDTH_STANDARD`];
    /// use [`BORU_DIALOG_WIDTH_LARGE`] for wider forms.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Cap the body height and scroll internally when content is longer.
    pub fn scroll_body(mut self, max_height: f32) -> Self {
        self.max_body_height = Some(max_height);
        self
    }

    /// Show a close (×) button in the header that emits `message`.
    pub fn on_close(mut self, message: Message) -> Self {
        self.on_close = Some(message);
        self
    }

    /// Set the secondary footer action (typically Cancel / back).
    pub fn secondary(mut self, label: &'a str, message: Message) -> Self {
        self.secondary = Some((label, message));
        self
    }

    /// Set the primary footer action (Create / Continue / Start / Save).
    pub fn primary(mut self, label: &'a str, message: Message) -> Self {
        self.primary = Some((label, message));
        self
    }

    /// Enable/disable the secondary footer action (default: enabled).
    ///
    /// While a submit is in flight the secondary action (Cancel) should be
    /// disabled so the dialog cannot be dismissed mid-processing — Escape,
    /// backdrop click and the Cancel button all become no-ops for safety.
    pub fn secondary_enabled(mut self, enabled: bool) -> Self {
        self.secondary_enabled = enabled;
        self
    }

    /// Enable/disable the primary footer action (default: enabled).
    ///
    /// Set to `false` until the form's required inputs are valid, and again
    /// while the submit is in flight, so the user cannot double-submit or
    /// submit an invalid form.
    pub fn primary_enabled(mut self, enabled: bool) -> Self {
        self.primary_enabled = enabled;
        self
    }

    /// Emit `message` when the dimmed backdrop is clicked (click-outside closes).
    pub fn on_backdrop(mut self, message: Message) -> Self {
        self.on_backdrop = Some(message);
        self
    }

    /// Build the full modal overlay: dimmed backdrop + centred dialog panel.
    ///
    /// Callers compose it over the base layout with
    /// `iced::widget::stack![base, overlay]`, matching the existing dialog
    /// composition pattern used by `view_connection_details_dialog`.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        // All styles below are theme-aware closures that receive the current
        // theme at render time; the parameter is accepted for API symmetry
        // with the other ui_components builders.
        let _ = theme;

        let BoruDialog {
            title,
            subtitle,
            body,
            width,
            max_body_height,
            on_close,
            secondary,
            secondary_enabled,
            primary,
            primary_enabled,
            on_backdrop,
        } = self;

        // ── Header: title + optional subtitle + close button ────────────
        let mut title_col = Column::new().spacing(design_tokens::SPACE_2);
        title_col = title_col.push(
            text(title)
                .font(TypeRole::SectionTitle.font())
                .size(TypeRole::SectionTitle.size_px())
                .style(move |t| iced::widget::text::Style {
                    color: Some(design_tokens::text_primary(t)),
                }),
        );
        if let Some(subtitle) = subtitle {
            title_col = title_col.push(
                text(subtitle)
                    .font(TypeRole::SupportingText.font())
                    .size(TypeRole::SupportingText.size_px())
                    .style(move |t| iced::widget::text::Style {
                        color: Some(design_tokens::text_secondary(t)),
                    }),
            );
        }

        let mut header = Row::new()
            .push(title_col)
            .push(Space::new().width(Length::Fill).height(Length::Shrink))
            .align_y(Alignment::Center);
        if let Some(msg) = on_close {
            header = header.push(close_button(msg));
        }

        // ── Body: form content, internally scrollable when capped ────────
        let body_column = Column::with_children(body)
            .spacing(design_tokens::SPACE_12)
            .width(Length::Fill);
        let body_el: Element<'a, Message> = match max_body_height {
            Some(height) => crate::ui_components::gutter_scrollable(body_column)
                .height(Length::Fixed(height))
                .width(Length::Fill)
                .into(),
            None => body_column.into(),
        };

        // ── Footer: secondary (Cancel) + primary (Create/…) ─────────────
        let footer = footer_row(secondary, secondary_enabled, primary, primary_enabled);

        let dialog = Column::new()
            .push(header)
            .push(Space::new().height(design_tokens::SPACE_16))
            .push(body_el)
            .push(Space::new().height(design_tokens::SPACE_16))
            .push(footer)
            .width(Length::Fill);

        let panel = container(dialog)
            .width(Length::Fixed(width))
            .height(Length::Shrink)
            .padding(design_tokens::SPACE_24)
            .style(design_tokens::dialog_style);

        // ── Overlay: dimmed backdrop behind the centred panel ────────────
        let backdrop = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |t| iced::widget::container::Style {
                background: Some(Background::Color(design_tokens::dialog_backdrop(t))),
                ..Default::default()
            });

        let centred = container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        match on_backdrop {
            Some(msg) => {
                let click_catcher = iced::widget::mouse_area(backdrop).on_press(msg);
                iced::widget::stack![click_catcher, centred].into()
            }
            None => iced::widget::stack![backdrop, centred].into(),
        }
    }
}

/// Ghost icon button with the Close (×) glyph.
fn close_button<'a, Message: Clone + 'a>(message: Message) -> Element<'a, Message> {
    let icon = Icon::Close
        .build()
        .size(IconSize::Sm)
        .interactive(true)
        .build();
    button(icon)
        .on_press(message)
        .padding(design_tokens::SPACE_4)
        .style(design_tokens::icon_button)
        .into()
}

/// Footer row: spacer, then secondary + primary actions, right-aligned.
///
/// Buttons are only given `on_press` when their enabled flag is true; iced
/// renders a button without `on_press` in its disabled state (muted, no
/// hover), which is how "primary disabled until valid" and "loading"
/// states are expressed.
fn footer_row<'a, Message: Clone + 'a>(
    secondary: Option<(&'a str, Message)>,
    secondary_enabled: bool,
    primary: Option<(&'a str, Message)>,
    primary_enabled: bool,
) -> Element<'a, Message> {
    let mut row = Row::new()
        .push(Space::new().width(Length::Fill).height(Length::Shrink))
        .spacing(design_tokens::SPACE_8)
        .align_y(Alignment::Center);

    if let Some((label, msg)) = secondary {
        let mut btn = button(
            text(label)
                .font(TypeRole::ButtonLabel.font())
                .size(TypeRole::ButtonLabel.size_px()),
        )
        .padding([design_tokens::SPACE_8, design_tokens::SPACE_16])
        .style(button_secondary_style);
        if secondary_enabled {
            btn = btn.on_press(msg);
        }
        row = row.push(btn);
    }

    if let Some((label, msg)) = primary {
        let mut btn = button(
            text(label)
                .font(TypeRole::ButtonLabel.font())
                .size(TypeRole::ButtonLabel.size_px()),
        )
        .padding([design_tokens::SPACE_8, design_tokens::SPACE_16])
        .style(button_primary_style);
        if primary_enabled {
            btn = btn.on_press(msg);
        }
        row = row.push(btn);
    }

    row.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    #[derive(Debug, Clone)]
    enum TestMessage {
        Close,
        Cancel,
        Create,
        Backdrop,
    }

    fn theme() -> Theme {
        Theme::Light
    }

    #[test]
    fn dialog_builds_with_title_only() {
        let el: Element<'static, TestMessage> = BoruDialog::new("Create New Room").build(&theme());
        let _ = el;
    }

    #[test]
    fn dialog_builds_full_form() {
        let el: Element<'static, TestMessage> = BoruDialog::new("Create Group Chat")
            .subtitle("Start a private group conversation")
            .push_body(text("Group name").into())
            .push_body(text("Description").into())
            .secondary("Cancel", TestMessage::Cancel)
            .primary("Create", TestMessage::Create)
            .on_close(TestMessage::Close)
            .width(BORU_DIALOG_WIDTH_LARGE)
            .build(&theme());
        let _ = el;
    }

    #[test]
    fn dialog_builds_with_scrollable_body() {
        let el: Element<'static, TestMessage> = BoruDialog::new("Share Tunnel")
            .push_body(text("Friend list").into())
            .scroll_body(250.0)
            .secondary("Cancel", TestMessage::Cancel)
            .build(&theme());
        let _ = el;
    }

    #[test]
    fn dialog_builds_with_backdrop_dismiss() {
        let el: Element<'static, TestMessage> = BoruDialog::new("Create Tunnel")
            .on_backdrop(TestMessage::Backdrop)
            .build(&theme());
        let _ = el;
    }

    #[test]
    fn default_width_is_standard() {
        let dialog = BoruDialog::<TestMessage>::new("x");
        assert_eq!(dialog.width, BORU_DIALOG_WIDTH_STANDARD);
        assert_eq!(dialog.body.len(), 0);
        assert!(dialog.subtitle.is_none());
        assert!(dialog.on_close.is_none());
        assert!(dialog.secondary.is_none());
        assert!(dialog.primary.is_none());
        assert!(dialog.on_backdrop.is_none());
    }
}
