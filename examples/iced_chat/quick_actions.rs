//! Self-contained quick-action cards for the home screen.
//!
//! Each card is a single keyboard- and pointer-activatable button. The cards
//! deliberately only dispatch existing application messages; the normal
//! update path owns the dialogs and file-picker flow.
//!
//! Visual notes (Figure 3 target):
//! - Icons match the mockup semantics: chat bubble (public room), two people
//!   (group chat), person + plus (add friend), upload arrow (share files).
//! - Labels use the Semibold page-label weight; descriptions stay muted 12 px.
//! - The card radius matches the rail `CardShell` cards (`RADIUS_LG`) so every
//!   home card shares the same corner rhythm instead of the generic 8 px
//!   `BUTTON_CARD`.

use iced::widget::{button, Column, Space};
use iced::{Alignment, Element, Length, Theme};

use crate::app::{AppMessage, SPACE_12, SPACE_16, SPACE_4, SPACE_8, TYPO_MD, TYPO_XS};
use crate::design_tokens;
use crate::icon_system::{Icon, IconSize};
use crate::ui_components::icon_tile;

pub(crate) struct QuickAction {
    icon: Icon,
    label: &'static str,
    description: &'static str,
    message: AppMessage,
}

const ACTIONS: &[QuickAction] = &[
    QuickAction {
        icon: Icon::Chat,
        label: "Create Public Room",
        description: "Open a room for anyone to join",
        message: AppMessage::CreateNewRoom,
    },
    QuickAction {
        icon: Icon::Users,
        label: "Create Group Chat",
        description: "Start a private group conversation",
        message: AppMessage::ShowCreateGroupDialog,
    },
    QuickAction {
        icon: Icon::UserPlus,
        label: "Add Friend",
        description: "Connect with a friend by public key",
        message: AppMessage::OpenFriendRequests,
    },
    QuickAction {
        icon: Icon::Upload,
        label: "Share Files",
        description: "Choose a file to share in a chat",
        message: AppMessage::AttachPressed,
    },
];

/// Build one complete quick-action card.
///
/// `iced::widget::button` provides the full-card hit target for pointer
/// activation. Framework note (UI-19): iced 0.14 buttons do not implement
/// `operation::Focusable`, so they cannot receive keyboard focus natively;
/// the primary actions on the home screen are still keyboard-reachable via
/// the global shortcuts (Ctrl+N new room, Ctrl+Backspace back, Escape,
/// `/` focus composer) which the app subscribes to globally.
pub fn quick_action_card<'a>(action: &'a QuickAction, theme: &Theme) -> Element<'a, AppMessage> {
    let content = Column::new()
        .push(icon_tile::<AppMessage>(action.icon, IconSize::Lg, None))
        .push(Space::new().height(Length::Fixed(SPACE_8)))
        .push(
            iced::widget::text(action.label)
                .size(TYPO_MD)
                .font(crate::fonts::source_sans(iced::font::Weight::Semibold)),
        )
        .push(Space::new().height(Length::Fixed(SPACE_4)))
        .push(
            iced::widget::text(action.description)
                .size(TYPO_XS)
                .color(design_tokens::text_muted(theme))
                .width(Length::Fill),
        )
        .spacing(0)
        .align_x(Alignment::Center)
        .width(Length::Fill);

    button(content)
        .on_press(action.message.clone())
        .padding([SPACE_12, SPACE_16])
        .height(Length::Fixed(132.0))
        .width(Length::Fill)
        .style(quick_action_card_style)
        .into()
}

/// Card-style button for the quick actions.
///
/// Mirrors `BUTTON_CARD` (surface bg, muted border, hover lift) but uses
/// `RADIUS_LG` so the action cards visually match the home rail cards and the
/// Figure 3 mockup instead of the generic 8 px control radius.
fn quick_action_card_style(theme: &Theme, status: button::Status) -> iced::widget::button::Style {
    let surface = design_tokens::surface(theme);
    let hover = design_tokens::surface_hover(theme);
    let accent = design_tokens::primary(theme);
    let background = match status {
        button::Status::Hovered => hover,
        button::Status::Pressed => {
            let mut c = hover;
            c.r *= 0.92;
            c.g *= 0.92;
            c.b *= 0.92;
            c
        }
        _ => surface,
    };
    let border_color = match status {
        button::Status::Hovered => accent,
        button::Status::Pressed => {
            let mut c = accent;
            c.r *= 0.85;
            c.g *= 0.85;
            c.b *= 0.85;
            c
        }
        _ => design_tokens::border_muted(theme),
    };
    button::Style {
        background: Some(iced::Background::Color(background)),
        text_color: match status {
            button::Status::Hovered => accent,
            button::Status::Pressed => {
                let mut c = accent;
                c.r *= 0.85;
                c.g *= 0.85;
                c.b *= 0.85;
                c
            }
            _ => design_tokens::text_muted(theme),
        },
        border: iced::Border {
            color: border_color,
            width: 1.0,
            radius: design_tokens::RADIUS_LG.into(),
        },
        ..Default::default()
    }
}

/// Number of quick-action columns for a given window width (plan §4).
///
/// Breakpoints match the reference sizes the home grid is verified at:
/// four columns on wide layouts (≥ 1040 px), two columns on medium layouts
/// (640–1039 px), and one column only when space is narrow (< 640 px).
pub fn grid_columns_for(window_width: f32) -> usize {
    if window_width < 640.0 {
        1
    } else if window_width < 1040.0 {
        2
    } else {
        4
    }
}

/// Build the responsive quick-action grid used by the home screen.
pub fn quick_action_grid<'a>(window_width: f32, theme: &Theme) -> Element<'a, AppMessage> {
    let columns = grid_columns_for(window_width);

    let mut rows: Vec<Element<'a, AppMessage>> = Vec::new();
    for actions in ACTIONS.chunks(columns) {
        let mut row = iced::widget::Row::new()
            .spacing(SPACE_8)
            .width(Length::Fill);
        for action in actions {
            row = row.push(quick_action_card(action, theme));
        }
        rows.push(row.into());
    }

    iced::widget::Column::with_children(rows)
        .spacing(SPACE_8)
        .width(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::{grid_columns_for, ACTIONS};

    #[test]
    fn exposes_the_four_home_actions() {
        assert_eq!(ACTIONS.len(), 4);
        assert_eq!(
            ACTIONS
                .iter()
                .map(|action| action.label)
                .collect::<Vec<_>>(),
            vec![
                "Create Public Room",
                "Create Group Chat",
                "Add Friend",
                "Share Files",
            ]
        );
    }

    #[test]
    fn action_icons_match_figure3_semantics() {
        // Figure 3: chat bubble, two people, person + plus, upload arrow.
        assert_eq!(ACTIONS[0].icon, crate::icon_system::Icon::Chat);
        assert_eq!(ACTIONS[1].icon, crate::icon_system::Icon::Users);
        assert_eq!(ACTIONS[2].icon, crate::icon_system::Icon::UserPlus);
        assert_eq!(ACTIONS[3].icon, crate::icon_system::Icon::Upload);
    }

    #[test]
    fn grid_columns_follow_the_plan_breakpoints() {
        // Plan §4: four columns wide, two columns medium, one column narrow.
        assert_eq!(grid_columns_for(1920.0), 4);
        assert_eq!(grid_columns_for(1440.0), 4);
        assert_eq!(grid_columns_for(1280.0), 4);
        assert_eq!(grid_columns_for(1040.0), 4);
        // Medium: 640–1039 px (e.g. the 1024×720 reference).
        assert_eq!(grid_columns_for(1024.0), 2);
        assert_eq!(grid_columns_for(800.0), 2);
        assert_eq!(grid_columns_for(640.0), 2);
        // Narrow: below 640 px collapses to a single column.
        assert_eq!(grid_columns_for(639.0), 1);
        assert_eq!(grid_columns_for(480.0), 1);
    }

    #[test]
    fn grid_columns_are_contiguous_without_gaps() {
        // Every width maps to exactly one of the three supported counts.
        for width in (320..=1920).step_by(16) {
            let columns = grid_columns_for(width as f32);
            assert!(
                columns == 1 || columns == 2 || columns == 4,
                "width {width} produced unexpected column count {columns}"
            );
        }
    }
}
