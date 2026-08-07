//! Self-contained quick-action cards for the home screen.
//!
//! Each card is a single keyboard- and pointer-activatable button. The cards
//! deliberately only dispatch existing application messages; the normal
//! update path owns the dialogs and file-picker flow.
//!
//! Visual notes (Figure 3 target):
//! - Icons match the mockup semantics: chat bubble (public room), two people
//!   (group chat), person + plus (add friend), upload arrow (share files).
//! - Labels use the card-title role at the FONTS-07 quick-action size (IBM
//!   Plex Sans SemiBold 17); descriptions stay muted supporting text at the
//!   FONTS-07 size (IBM Plex Sans Regular 14) and the plan's 1.45 line
//!   height. No Archivo SemiCondensed on these cards.
//! - The card radius matches the rail `CardShell` cards (`RADIUS_CARD`) so
//!   every home card shares the same corner rhythm.
//! - Card structure (UI-HOME-06): 56 px light-green icon container, 24 px
//!   horizontal padding, 16 px icon→title gap, 8 px title→description gap,
//!   and a subtle bottom-right action indicator. Heights are content-driven:
//!   the card grows with wrapped text instead of clipping it.

use iced::widget::{button, container, Column, Row, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme, Vector};

use crate::app::{AppMessage, SPACE_12, SPACE_16, SPACE_20, SPACE_24, SPACE_8};
use crate::design_tokens;
use crate::icon_system::{Icon, IconSize};

/// Diameter of the light-green quick-action icon container (task: 52–60 px).
const QUICK_ACTION_ICON_SIZE: f32 = 56.0;

/// Quick-action card title size (FONTS-07: IBM Plex Sans SemiBold ~16–17 px).
///
/// `TypeRole::CardTitle` defaults to 18 px for dashboard cards app-wide; the
/// approved quick-action mockup uses the tighter 16–17 px band, so the role's
/// font/weight (IBM Plex Sans SemiBold) is kept and only the size is
/// overridden locally for these cards.
const QUICK_ACTION_TITLE_SIZE: f32 = 17.0;

/// Quick-action card description size (FONTS-07: IBM Plex Sans Regular ~14 px).
const QUICK_ACTION_DESCRIPTION_SIZE: f32 = 14.0;

/// Quick-action card description line height (FONTS-07: 1.4–1.45).
const QUICK_ACTION_DESCRIPTION_LINE_HEIGHT: f32 = 1.45;

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
        description: "Open a public room for anyone to join.",
        message: AppMessage::CreateNewRoom,
    },
    QuickAction {
        icon: Icon::Users,
        label: "Create Group Chat",
        description: "Start a private group conversation.",
        message: AppMessage::ShowCreateGroupDialog,
    },
    QuickAction {
        icon: Icon::UserPlus,
        label: "Add Friend",
        description: "Connect with a friend by public key.",
        message: AppMessage::OpenFriendRequests,
    },
    QuickAction {
        icon: Icon::Upload,
        label: "Share Files",
        description: "Choose a file to share in a chat.",
        message: AppMessage::AttachPressed,
    },
];

/// Light-green circular icon tile (UI-HOME-06: 52–60 px container).
///
/// Mirrors the `icon_tile` look (soft brand-green background, centered
/// icon) at the larger size the approved quick-action card calls for.
fn quick_action_icon<'a>(icon: Icon) -> Element<'a, AppMessage> {
    let tile = QUICK_ACTION_ICON_SIZE;
    container(icon.build().size(IconSize::Lg).build())
        .width(Length::Fixed(tile))
        .height(Length::Fixed(tile))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |t| container::Style {
            background: Some(Background::Color(design_tokens::primary_soft(t))),
            border: Border {
                radius: (tile / 2.0).into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Subtle bottom-right action indicator (chevron) hinting the card is a
/// button without competing with the title or description.
fn action_indicator<'a>() -> Element<'a, AppMessage> {
    Row::new()
        .push(Space::new().width(Length::Fill))
        .push(
            Icon::ChevronRight
                .build()
                .size(IconSize::Xs)
                .color_fn(design_tokens::text_muted)
                .build(),
        )
        .width(Length::Fill)
        .into()
}

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
        .push(quick_action_icon(action.icon))
        .push(Space::new().height(Length::Fixed(SPACE_16)))
        .push(
            // Quick-action title — TypeRole::CardTitle (IBM Plex Sans
            // SemiBold) at the FONTS-07 quick-action size (17 px; the role
            // default 18 px stays shared with other dashboard cards).
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, action.label)
                .size(QUICK_ACTION_TITLE_SIZE)
                .width(Length::Fill),
        )
        .push(Space::new().height(Length::Fixed(SPACE_8)))
        .push(
            // Supporting description — TypeRole::SupportingText (IBM Plex
            // Sans Regular) at the FONTS-07 quick-action size (14 px) with
            // the plan's 1.4–1.45 line height. Width is Fill so the text
            // wraps within the card; the card height is content-driven so
            // the full description is always visible (UI-HOME-01 audit §4
            // root cause: a fixed 132 px height clipped wrapped
            // descriptions — that box is gone).
            crate::fonts::type_role_text_lh(
                crate::fonts::TypeRole::SupportingText,
                action.description,
                QUICK_ACTION_DESCRIPTION_LINE_HEIGHT,
            )
            .size(QUICK_ACTION_DESCRIPTION_SIZE)
            .color(design_tokens::text_muted(theme))
            .width(Length::Fill),
        )
        .push(Space::new().height(Length::Fixed(SPACE_12)))
        .push(action_indicator())
        .spacing(0)
        .align_x(Alignment::Center)
        .width(Length::Fill);

    button(content)
        .on_press(action.message.clone())
        // 20 px vertical / 24 px horizontal padding (UI-HOME-06: 20–24 px).
        .padding([SPACE_20, SPACE_24])
        // Content-driven height: no fixed box, no hidden overflow — the
        // card grows to contain icon + title + full description.
        .width(Length::Fill)
        .style(quick_action_card_style)
        .into()
}

/// Card-style button for the quick actions.
///
/// Mirrors `BUTTON_CARD` (surface bg, muted border) plus the shared card
/// surface (RADIUS_CARD + low-opacity shadow) so the action cards match
/// the home rail cards. Hover adds an accent border and a 1–2 px elevation
/// shadow; the default state keeps the subtle card shadow.
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
    // 1–2 px hover elevation: subtle lift over the resting card shadow.
    let shadow = match status {
        button::Status::Hovered | button::Status::Pressed => iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.10),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        _ => design_tokens::shadow_card(theme),
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
            radius: design_tokens::RADIUS_CARD.into(),
        },
        shadow,
        ..Default::default()
    }
}

/// Number of quick-action columns for a given *content* width (UI-HOME-15).
///
/// Content width is the home dashboard's available width after the sidebar,
/// divider and page padding are removed (`design_tokens::home_content_width`),
/// so the grid never starves on narrow windows with a fixed 288 px sidebar.
/// Breakpoints keep four columns only where cards are wide enough to stay
/// readable; the grid drops to two-by-two before cards become too narrow,
/// and to one column at the minimum supported width. This matches the
/// design system's "Large ≥ 1440 px → full four-column quick actions"
/// (window 1440 → content ~1071 px ≥ HOME_QUICK_FOUR_COL_CONTENT).
pub fn grid_columns_for(content_width: f32) -> usize {
    if content_width >= crate::design_tokens::HOME_QUICK_FOUR_COL_CONTENT {
        4
    } else if content_width >= crate::design_tokens::HOME_QUICK_ONE_COL_CONTENT {
        2
    } else {
        1
    }
}

/// Build the responsive quick-action grid used by the home screen.
pub fn quick_action_grid<'a>(content_width: f32, theme: &Theme) -> Element<'a, AppMessage> {
    let columns = grid_columns_for(content_width);

    let mut rows: Vec<Element<'a, AppMessage>> = Vec::new();
    for actions in ACTIONS.chunks(columns) {
        let mut row = iced::widget::Row::new()
            .spacing(SPACE_8)
            // Top-align cards so a wrapped description in one card never
            // shifts its neighbours' icons/titles vertically (content-driven
            // heights differ per card).
            .align_y(Alignment::Start)
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
    fn action_descriptions_match_approved_copy() {
        // UI-HOME-06: these exact descriptions must never be truncated.
        assert_eq!(
            ACTIONS
                .iter()
                .map(|action| action.description)
                .collect::<Vec<_>>(),
            vec![
                "Open a public room for anyone to join.",
                "Start a private group conversation.",
                "Connect with a friend by public key.",
                "Choose a file to share in a chat.",
            ]
        );
    }

    #[test]
    fn action_messages_dispatch_to_expected_flows() {
        use crate::app::AppMessage;
        assert!(matches!(ACTIONS[0].message, AppMessage::CreateNewRoom));
        assert!(matches!(
            ACTIONS[1].message,
            AppMessage::ShowCreateGroupDialog
        ));
        assert!(matches!(ACTIONS[2].message, AppMessage::OpenFriendRequests));
        assert!(matches!(ACTIONS[3].message, AppMessage::AttachPressed));
    }

    #[test]
    fn grid_columns_follow_the_design_breakpoints() {
        // UI-HOME-15: columns are computed from the dashboard *content*
        // width (window minus sidebar/divider/padding). Four columns only
        // on wide layouts (window ≥ 1440 → content ≥ 1000, matching
        // DESIGN_SYSTEM.md "Large"), two-by-two before cards get too narrow
        // (content 520–999), one column at the minimum supported width
        // (content < 520, e.g. an 800×600 window).
        use crate::design_tokens::home_content_width;
        // Window 1920/1600/1440 → content ~1551/1231/1071 → 4 columns.
        assert_eq!(grid_columns_for(home_content_width(1920.0)), 4);
        assert_eq!(grid_columns_for(home_content_width(1600.0)), 4);
        assert_eq!(grid_columns_for(home_content_width(1440.0)), 4);
        // Medium: content 520–999 (e.g. 1280×800 and 1024×720 windows).
        assert_eq!(grid_columns_for(home_content_width(1280.0)), 2);
        assert_eq!(grid_columns_for(home_content_width(1024.0)), 2);
        // Narrow: content < 520 → one quick action per row.
        assert_eq!(grid_columns_for(home_content_width(800.0)), 1);
        assert_eq!(grid_columns_for(home_content_width(640.0)), 1);
        // Boundary checks on the content-width thresholds themselves.
        assert_eq!(grid_columns_for(1000.0), 4);
        assert_eq!(grid_columns_for(999.0), 2);
        assert_eq!(grid_columns_for(520.0), 2);
        assert_eq!(grid_columns_for(519.0), 1);
    }

    #[test]
    fn grid_columns_are_contiguous_without_gaps() {
        // Every content width maps to exactly one of the three supported
        // counts.
        use crate::design_tokens::home_content_width;
        for window in (320..=1920).step_by(16) {
            let content = home_content_width(window as f32);
            let columns = grid_columns_for(content);
            assert!(
                columns == 1 || columns == 2 || columns == 4,
                "window {window} (content {content:.0}) produced unexpected column count {columns}"
            );
        }
    }

    #[test]
    fn quick_action_cards_are_content_driven_not_fixed_height() {
        // UI-HOME-06 (and UI-HOME-12 before it): the old fixed 132 px card
        // height clipped wrapped descriptions (UI-HOME-01 audit §4). Cards
        // must size to their content — no fixed height, no hidden overflow —
        // and keep the approved structure: 52–60 px icon container, 20–24 px
        // padding, 14–18 px icon→title gap, ~8 px title→description gap,
        // light-green icon background, and TypeRole-based typography.
        let src = include_str!("quick_actions.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap();
        assert!(
            !prod.contains("Length::Fixed(132.0)"),
            "quick-action cards must not force a fixed 132 px height"
        );
        assert!(
            !prod.contains("clip("),
            "quick-action cards must not hide overflow with clipping"
        );
        assert!(
            prod.contains("QUICK_ACTION_ICON_SIZE: f32 = 56.0"),
            "icon container must be 56 px (52–60 px band)"
        );
        assert!(
            prod.contains(".padding([SPACE_20, SPACE_24])"),
            "card padding must be 20 px vertical / 24 px horizontal"
        );
        assert!(
            prod.contains("TypeRole::CardTitle"),
            "quick-action labels must use TypeRole::CardTitle (IBM Plex Sans SemiBold 17)"
        );
        assert!(
            prod.contains("TypeRole::SupportingText"),
            "quick-action descriptions must use TypeRole::SupportingText (IBM Plex Sans Regular 14)"
        );
        assert!(
            prod.contains("type_role_text_lh("),
            "quick-action descriptions must use the line-height helper (plan 1.45)"
        );
        // FONTS-07: the quick-action cards size their roles locally (17 px
        // title / 14 px description at 1.45) instead of the shared role
        // defaults (CardTitle 18 / SupportingText 13) used elsewhere.
        assert!(
            prod.contains(".size(QUICK_ACTION_TITLE_SIZE)"),
            "quick-action titles must override the card-title size to the FONTS-07 17 px band"
        );
        assert!(
            prod.contains(".size(QUICK_ACTION_DESCRIPTION_SIZE)"),
            "quick-action descriptions must override the supporting-text size to the FONTS-07 14 px"
        );
        assert!(
            prod.contains("QUICK_ACTION_DESCRIPTION_LINE_HEIGHT"),
            "quick-action descriptions must use the FONTS-07 1.4–1.45 line-height constant"
        );
    }

    #[test]
    fn quick_action_natural_height_exceeds_old_fixed_box() {
        // Clipping-math regression (UI-HOME-01 audit root cause + UI-HOME-12):
        // with the FONTS-07 quick-action metrics the tallest card content —
        // 56 px icon tile + 16 px gap + 17 px title + 8 px gap + a two-line
        // 14 px description at 1.45 line height + 20 px vertical padding +
        // action indicator — needs more than the removed 132 px fixed height,
        // which is why cards must be content-driven.
        use crate::design_tokens::{SPACE_12, SPACE_16, SPACE_20, SPACE_8};

        let tile = super::QUICK_ACTION_ICON_SIZE;
        let title = super::QUICK_ACTION_TITLE_SIZE * 1.3; // single-line heading
        let description_two_lines =
            super::QUICK_ACTION_DESCRIPTION_SIZE * super::QUICK_ACTION_DESCRIPTION_LINE_HEIGHT * 2.0;
        let indicator = crate::icon_system::IconSize::Xs.px() + SPACE_12;
        let vertical_padding = 2.0 * SPACE_20;
        let gaps = SPACE_16 + SPACE_8;
        let natural = tile + gaps + title + description_two_lines + indicator + vertical_padding;
        assert!(
            natural > 132.0,
            "content-driven quick-action card needs {natural:.1} px > old fixed 132 px; \
             a fixed-height card would clip the description"
        );
        assert!(
            natural >= 200.0,
            "quick-action card should be near the plan's 220–250 px band \
             when text wraps (computed natural height {natural:.1} px)"
        );
    }
}
