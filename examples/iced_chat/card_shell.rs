//! Reusable card shell for the Boru home rail (Figure 3).
//!
//! A `CardShell` renders a titled card with an optional count badge and an
//! optional "View all" action, wrapping a bounded, scrollable list body:
//!
//! - **Header** — uppercase muted title, optional count badge, optional
//!   trailing "View all" ghost button.
//! - **Body** — when children are supplied, a scrollable list with a fixed
//!   maximum height (`max_height`, default [`DEFAULT_LIST_MAX_HEIGHT`]), so a
//!   long list scrolls instead of growing the dashboard without bound. When
//!   children are empty, the caller-provided `empty_message` is shown with
//!   UI-04 empty-state typography.
//!
//! All spacing, radii, borders, shadows, and colours come from
//! [`crate::design_tokens`]; typography comes from [`crate::fonts::Typography`].
//! Rows built by callers should target [`CARD_ROW_HEIGHT`] (48 px) so every
//! card in the rail shares the same rhythm.
//!
//! The shell is intentionally data-agnostic: it never constructs list rows
//! and holds no sample data — the caller owns the children.

use iced::widget::{button, container, scrollable, text, Column, Row, Space};
use iced::{Alignment, Background, Border, Element, Length, Theme};

use crate::design_tokens;
use crate::fonts::Typography;

/// Consistent height for a single list row inside a card shell (48 px).
///
/// Used by callers when building compact rows so Online Peers, Recent
/// Activity, and Tunnels cards all share the same row rhythm.
pub const CARD_ROW_HEIGHT: f32 = 48.0;

/// Default fixed maximum height of the scrollable list body (px).
///
/// Below this many rows the list shrinks to its content; beyond it a vertical
/// scrollbar appears.
pub const DEFAULT_LIST_MAX_HEIGHT: f32 = 180.0;

/// Builder for a reusable card shell (Figure 3 right rail).
///
/// Example:
/// ```
/// let shell = CardShell::new("Online Peers", rows)
///     .count(3)
///     .on_view_all(AppMessage::Noop)
///     .empty_message("No peers are online.")
///     .max_height(140.0)
///     .build(&theme);
/// ```
pub struct CardShell<'a, Message> {
    /// Header title — rendered uppercase per the Fig 3 rail look.
    title: &'a str,
    /// Optional count badge shown next to the title.
    count: Option<usize>,
    /// Optional total paired with `count` — when set the badge renders
    /// "online/total" (e.g. "3/12") instead of a bare number.
    count_total: Option<usize>,
    /// Optional "View all" header action.
    on_view_all: Option<Message>,
    /// Message shown when `children` is empty.
    empty_message: Option<&'a str>,
    /// Fixed max height of the scrollable list body.
    max_height: f32,
    /// Vertical spacing between list rows.
    row_spacing: f32,
    /// List rows rendered inside the bounded scrollable.
    children: Vec<Element<'a, Message>>,
}

impl<'a, Message: Clone + 'a> CardShell<'a, Message> {
    /// Start a card shell with a title and its list rows.
    ///
    /// Pass an empty `children` vec together with
    /// [`Self::empty_message`] to render the empty state.
    pub fn new(title: &'a str, children: Vec<Element<'a, Message>>) -> Self {
        Self {
            title,
            count: None,
            count_total: None,
            on_view_all: None,
            empty_message: None,
            max_height: DEFAULT_LIST_MAX_HEIGHT,
            row_spacing: design_tokens::SPACE_2,
            children,
        }
    }

    /// Show a count badge in the header (e.g. online/total peers).
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// Pair the count badge with a total so it reads "online/total".
    ///
    /// When set, the badge renders `"{count}/{total}"` (e.g. `3/12`);
    /// otherwise it renders just the count.
    pub fn count_total(mut self, total: usize) -> Self {
        self.count_total = Some(total);
        self
    }

    /// Add a "View all" action button to the header.
    pub fn on_view_all(mut self, msg: Message) -> Self {
        self.on_view_all = Some(msg);
        self
    }

    /// Message shown when `children` is empty.
    pub fn empty_message(mut self, message: &'a str) -> Self {
        self.empty_message = Some(message);
        self
    }

    /// Override the fixed maximum height of the scrollable list body.
    pub fn max_height(mut self, height: f32) -> Self {
        self.max_height = height;
        self
    }

    /// Override the vertical spacing between list rows.
    pub fn row_spacing(mut self, spacing: f32) -> Self {
        self.row_spacing = spacing;
        self
    }

    /// Build the card shell element.
    pub fn build(self, theme: &Theme) -> Element<'a, Message> {
        let mut header = Row::new()
            .spacing(design_tokens::SPACE_6)
            .align_y(Alignment::Center)
            .push(
                text(self.title.to_uppercase())
                    .font(Typography::SecondaryText.font())
                    .size(Typography::SecondaryText.size_px())
                    .color(design_tokens::text_muted(theme)),
            );

        if let Some(count) = self.count {
            let label = match self.count_total {
                Some(total) => format!("{count}/{total}"),
                None => count.to_string(),
            };
            header = header.push(count_badge::<Message>(label));
        }

        header = header.push(Space::new().width(Length::Fill).height(Length::Shrink));

        if let Some(msg) = self.on_view_all {
            header = header.push(
                button(
                    text("View all")
                        .font(Typography::SecondaryText.font())
                        .size(Typography::SecondaryText.size_px()),
                )
                .on_press(msg)
                .padding([design_tokens::SPACE_2, design_tokens::SPACE_6])
                .style(view_all_button_style),
            );
        }

        // Body: empty state (with UI-04 empty-state typography) or a bounded
        // scrollable list. The fixed height is what keeps many peers /
        // activities from growing the dashboard without bound.
        let body: Element<'a, Message> = if self.children.is_empty() {
            if let Some(message) = self.empty_message {
                container(
                    text(message)
                        .font(Typography::SecondaryText.font())
                        .size(Typography::SecondaryText.size_px())
                        .color(design_tokens::text_muted(theme)),
                )
                .width(Length::Fill)
                .padding([design_tokens::SPACE_6, 0.0])
                .into()
            } else {
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .into()
            }
        } else {
            scrollable(
                Column::with_children(self.children)
                    .spacing(self.row_spacing)
                    .width(Length::Fill),
            )
            .height(Length::Fixed(self.max_height))
            .width(Length::Fill)
            .into()
        };

        container(
            Column::new()
                .push(header)
                .push(Space::new().height(Length::Fixed(design_tokens::SPACE_6)))
                .push(body)
                .spacing(0)
                .width(Length::Fill),
        )
        .padding([design_tokens::SPACE_12, design_tokens::SPACE_16])
        .width(Length::Fill)
        .style(|t| design_tokens::card_style(t))
        .into()
    }
}

/// Count badge for the header — mirrors `ui_components::badge(.., BadgeKind::Accent)`
/// (primary_soft background, primary text) but takes an owned label so dynamic
/// values (including "online/total" pairs) never borrow from locals inside `build`.
fn count_badge<'a, Message: 'a>(label: String) -> Element<'a, Message> {
    container(
        text(label)
            .font(Typography::SecondaryText.font())
            .size(Typography::SecondaryText.size_px()),
    )
    .padding([2.0, design_tokens::SPACE_8])
    .style(move |t| container::Style {
        background: Some(Background::Color(design_tokens::primary_soft(t))),
        text_color: Some(design_tokens::primary(t)),
        border: Border {
            radius: design_tokens::SPACE_12.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// Ghost "View all" button — muted text, primary accent on hover/press.
fn view_all_button_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: match status {
            button::Status::Hovered => design_tokens::primary(theme),
            button::Status::Pressed => design_tokens::primary_pressed(theme),
            _ => design_tokens::text_secondary(theme),
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    #[test]
    fn row_height_token_is_48px() {
        // The shared rail row height must stay exactly 48 px per the plan.
        assert_eq!(CARD_ROW_HEIGHT, 48.0);
    }

    #[test]
    fn card_shell_default_max_height_is_bounded() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert_eq!(shell.max_height, DEFAULT_LIST_MAX_HEIGHT);
        assert!(
            shell.max_height > 0.0,
            "max height must be finite and positive"
        );
    }

    #[test]
    fn card_shell_has_no_count_by_default() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert_eq!(shell.count, None);
    }

    #[test]
    fn card_shell_stores_count() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]).count(7);
        assert_eq!(shell.count, Some(7));
    }

    #[test]
    fn card_shell_stores_count_total() {
        let shell: CardShell<'static, ()> =
            CardShell::new("Peers", vec![]).count(3).count_total(12);
        assert_eq!(shell.count, Some(3));
        assert_eq!(shell.count_total, Some(12));
    }

    #[test]
    fn card_shell_count_total_defaults_to_none() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]).count(3);
        assert_eq!(shell.count_total, None);
    }

    #[test]
    fn card_shell_build_count_total_badge_does_not_panic() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![])
            .count(3)
            .count_total(12)
            .empty_message("No peers online");
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_stores_view_all_action() {
        let shell = CardShell::new("Peers", vec![]).on_view_all(());
        assert!(
            shell.on_view_all.is_some(),
            "View all action must be stored"
        );
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert!(shell.on_view_all.is_none(), "no action by default");
    }

    #[test]
    fn card_shell_stores_empty_message() {
        let shell: CardShell<'static, ()> =
            CardShell::new("Peers", vec![]).empty_message("No peers online");
        assert_eq!(shell.empty_message, Some("No peers online"));
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert_eq!(shell.empty_message, None, "no empty message by default");
    }

    #[test]
    fn card_shell_stores_children() {
        let children: Vec<Element<'static, ()>> = vec![text("a").into(), text("b").into()];
        let shell = CardShell::new("Peers", children);
        assert_eq!(shell.children.len(), 2);
    }

    #[test]
    fn card_shell_build_empty_state_does_not_panic() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![])
            .count(0)
            .empty_message("No peers are online right now.");
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_build_populated_does_not_panic() {
        let children: Vec<Element<'static, ()>> =
            (0..12).map(|i| text(format!("Row {i}")).into()).collect();
        let shell = CardShell::new("Activity", children)
            .count(12)
            .on_view_all(())
            .max_height(120.0);
        let el = shell.build(&Theme::Light);
        let _ = el;
    }

    #[test]
    fn card_shell_row_spacing_defaults_to_token() {
        let shell: CardShell<'static, ()> = CardShell::new("Peers", vec![]);
        assert_eq!(shell.row_spacing, design_tokens::SPACE_2);
    }
}
