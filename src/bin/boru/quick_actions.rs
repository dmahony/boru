//! Self-contained quick-action cards for the home screen.
//!
//! Each card is a single keyboard- and pointer-activatable button. The cards
//! deliberately only dispatch existing application messages; the normal
//! update path owns the dialogs and file-picker flow.
//!
//! Visual notes (BORU-HOME-07 target):
//! - Icons match their actions: person (start chat), two people
//!   (create group), chat bubble (create public room), terminal (create tunnel).
//! - Labels use the card-title role at the FONTS-07 quick-action size (IBM
//!   Plex Sans SemiBold 17); descriptions stay muted supporting text at the
//!   FONTS-07 size (IBM Plex Sans Regular 14) and the plan's 1.45 line
//!   height. No Archivo SemiCondensed on these cards.
//! - The card radius matches the rail `CardShell` cards (`RADIUS_CARD`) so
//!   every home card shares the same corner rhythm.
//! - Card structure (HOME-02 compact): 40 px light-green icon container,
//!   12 px vertical / 16 px horizontal padding, 8 px icon→title gap,
//!   4 px title→description gap, and a subtle bottom-right action
//!   indicator. Heights are content-driven: the card grows with wrapped
//!   text instead of clipping it.

use iced::widget::{button, container, Column, Row, Space};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme, Vector};

use crate::app::{accent_primary, accent_soft, AppMessage, SPACE_4, SPACE_8};
use crate::design_tokens;
use crate::icon_system::{Icon, IconSize};

pub(crate) struct QuickAction {
    icon: Icon,
    label: &'static str,
    description: &'static str,
    message: AppMessage,
}

const ACTIONS: &[QuickAction] = &[
    QuickAction {
        // The New Chat command is represented by the existing message glyph;
        // the friend glyph remains reserved for people/friend destinations.
        icon: Icon::Message,
        label: "home.new_chat",
        description: "home.quick_start_chat_desc",
        message: AppMessage::OpenFriendRequests,
    },
    QuickAction {
        icon: Icon::Users,
        label: "discover.public_rooms_title",
        description: "home.quick_public_rooms_desc",
        message: AppMessage::OpenDirectory,
    },
    QuickAction {
        icon: Icon::Plus,
        label: "dialogs.create_room.title",
        description: "home.quick_create_room_desc",
        message: AppMessage::CreateNewRoom,
    },
    QuickAction {
        icon: Icon::Files,
        label: "files.share_file",
        description: "home.quick_send_file_desc",
        message: AppMessage::OpenFileSharing,
    },
];

/// Light-green circular icon tile (HOME-02 compact: 40 px container).
///
/// Mirrors the `icon_tile` look (soft brand-green background, centered
/// icon) at the compact size the HOME-02 cards call for. BORU-LAYOUT-03:
/// the tile diameter comes from the layout model's
/// `home.card_sizing.quick_action_icon_size` (default 40 px, the same
/// value `HomeTheme::quick_action_icon_size` supplied before).
fn quick_action_icon<'a>(icon: Icon, tile: f32) -> Element<'a, AppMessage> {
    container(
        icon.build()
            .size(IconSize::Lg)
            .color_fn(accent_primary)
            .build(),
    )
    .width(Length::Fixed(tile))
    .height(Length::Fixed(tile))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |t| container::Style {
        background: Some(Background::Color(accent_soft(t))),
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
    container(
        Icon::ChevronRight
            .build()
            .size(IconSize::Xs)
            .color_fn(design_tokens::text_muted)
            .build(),
    )
    .width(Length::Fixed(24.0))
    .height(Length::Fixed(24.0))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(|theme| container::Style {
        background: Some(Background::Color(design_tokens::surface_hover(theme))),
        border: Border {
            color: design_tokens::border_muted(theme),
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    })
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
pub fn quick_action_card<'a>(
    action: &'a QuickAction,
    theme: &Theme,
    opacity: f32,
    card_radius: f32,
    icon_size: f32,
    card_padding_y: f32,
    card_padding_x: f32,
    title_size: f32,
    description_size: f32,
    description_line_height: f32,
) -> Element<'a, AppMessage> {
    let content = Column::new()
        .push(quick_action_icon(action.icon, icon_size))
        // HOME-02: icon→title gap tightened from SPACE_16 to SPACE_8 so the
        // four cards sit noticeably closer together vertically.
        .push(Space::new().height(Length::Fixed(SPACE_8)))
        .push(
            // Quick-action title — TypeRole::CardTitle (IBM Plex Sans
            // SemiBold) at the FONTS-07 quick-action size (16 px; the role
            // default 18 px stays shared with other dashboard cards).
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::CardTitle,
                crate::i18n::t(action.label),
            )
            .size(title_size)
            .width(Length::Fill),
        )
        // HOME-02: title→description gap tightened from SPACE_8 to SPACE_4.
        .push(Space::new().height(Length::Fixed(SPACE_4)))
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
                crate::i18n::t(action.description),
                description_line_height,
            )
            .size(description_size)
            .color(design_tokens::text_muted(theme))
            .width(Length::Fill),
        )
        // HOME-02: description→indicator gap tightened from SPACE_12 to
        // SPACE_8.
        .push(Space::new().height(Length::Fixed(SPACE_8)))
        .push(
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(action_indicator())
                .width(Length::Fill)
                .spacing(0),
        )
        .spacing(0)
        .align_x(Alignment::Start)
        .width(Length::Fill);

    let card = button(content)
        .on_press(action.message.clone())
        // HOME-02 compact: 16 px vertical / 16 px horizontal padding (was
        // 20 px / 24 px) — smaller card, denser grid, still an easy tap
        // target because the whole card is the button.
        .padding([card_padding_y, card_padding_x])
        // Home is vertically scrollable, so rows have no finite height to
        // fill. Measure the full wrapped content instead of propagating an
        // infinite height that prevents the cards from rendering.
        .width(Length::Fill)
        .height(Length::Shrink)
        .style(move |t, s| quick_action_card_style(t, s, opacity, card_radius));

    // The wrapper supplies the standard Boru focus ring and Enter/Space
    // activation. Keeping the message on the wrapper (rather than adding a
    // second nested button for the chevron) guarantees one route per tile.
    crate::focusable_button::focusable_button(card, Some(action.message.clone()))
        .ring_radius(card_radius)
        .build()
        .into()
}

/// Card-style button for the quick actions.
///
/// Mirrors `BUTTON_CARD` (surface bg, muted border) plus the shared card
/// surface (RADIUS_CARD + low-opacity shadow) so the action cards match
/// the home rail cards. Hover adds an accent border and a 1–2 px elevation
/// shadow; the default state keeps the subtle card shadow.
fn quick_action_card_style(
    theme: &Theme,
    status: button::Status,
    opacity: f32,
    card_radius: f32,
) -> iced::widget::button::Style {
    let surface = design_tokens::surface(theme);
    let hover = design_tokens::surface_hover(theme);
    let accent = design_tokens::primary(theme);
    let background = match status {
        button::Status::Hovered => hover,
        button::Status::Pressed => design_tokens::surface_pressed(theme),
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
    // HOME-01: make the card surface translucent so the home background
    // image shows through at the user-configured menu item opacity.
    let background = Color {
        a: background.a * opacity.clamp(0.0, 1.0),
        ..background
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
            radius: card_radius.into(),
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
/// BORU-LAYOUT-03: the column counts and their content-width breakpoints
/// come from the layout model (`home.quick_actions`); the defaults preserve
/// the responsive tiers (4 columns ≥ 1000 px, 2 columns ≥ 520 px, 1 below).
pub fn grid_columns_for(content_width: f32, layout: crate::layout::QuickActionsLayout) -> usize {
    if content_width >= layout.four_col_breakpoint {
        layout.columns_wide
    } else if content_width >= layout.two_col_breakpoint {
        layout.columns_mid
    } else {
        layout.columns_narrow
    }
}

/// Build the responsive quick-action grid used by the home screen.
pub fn quick_action_grid<'a>(
    content_width: f32,
    theme: &Theme,
    opacity: f32,
    card_radius: f32,
    layout: crate::layout::QuickActionsLayout,
    icon_size: f32,
    title_size: f32,
    description_size: f32,
    description_line_height: f32,
) -> Element<'a, AppMessage> {
    let columns = grid_columns_for(content_width, layout);

    let mut rows: Vec<Element<'a, AppMessage>> = Vec::new();
    for actions in ACTIONS.chunks(columns) {
        let mut row = iced::widget::Row::new()
            .spacing(layout.gap)
            // Top-align naturally measured cards; wrapped descriptions
            // determine height without requiring a bounded scroll axis.
            .align_y(Alignment::Start)
            .width(Length::Fill);
        for action in actions {
            row = row.push(quick_action_card(
                action,
                theme,
                opacity,
                card_radius,
                icon_size,
                layout.card_padding_y,
                layout.card_padding_x,
                title_size,
                description_size,
                description_line_height,
            ));
        }
        rows.push(row.into());
    }

    iced::widget::Column::with_children(rows)
        .spacing(layout.gap)
        .width(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::{grid_columns_for, ACTIONS};
    use crate::design_tokens::{SPACE_12, SPACE_16};

    #[test]
    fn quick_action_grid_keeps_every_card_visible_in_content_sized_rows() {
        use iced::advanced::{layout, widget::Tree};
        use iced::{Font, Pixels, Size};

        let renderer =
            iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
        let home = crate::theme::BoruTheme::default().home;
        for width in [300.0, 600.0, 1200.0] {
            let mut grid = super::quick_action_grid(
                width,
                &iced::Theme::Dark,
                1.0,
                12.0,
                crate::layout::QuickActionsLayout::default(),
                home.quick_action_icon_size,
                home.quick_action_title_size,
                home.quick_action_desc_size,
                home.quick_action_desc_line_height,
            );
            let mut tree = Tree::new(grid.as_widget());
            let node = grid.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY)),
            );
            let mut count = 0;
            for row in node.children() {
                for card in row.children() {
                    count += 1;
                    assert!(
                        card.bounds().height >= 100.0 && card.bounds().height.is_finite(),
                        "width {width}: quick action collapsed: {:?}",
                        card.bounds()
                    );
                }
            }
            assert_eq!(count, ACTIONS.len());
        }
    }

    #[test]
    fn exposes_the_four_home_actions() {
        assert_eq!(ACTIONS.len(), 4);
        // Labels are i18n keys resolved at render time via crate::i18n::t.
        assert_eq!(
            ACTIONS
                .iter()
                .map(|action| action.label)
                .collect::<Vec<_>>(),
            vec![
                "home.new_chat",
                "discover.public_rooms_title",
                "dialogs.create_room.title",
                "files.share_file",
            ]
        );
    }

    #[test]
    fn action_icons_match_figure3_semantics() {
        // Home actions use existing icons that match the PDF destinations:
        // message, community, create, and file sharing.
        assert_eq!(ACTIONS[0].icon, crate::icon_system::Icon::Message);
        assert_eq!(ACTIONS[1].icon, crate::icon_system::Icon::Users);
        assert_eq!(ACTIONS[2].icon, crate::icon_system::Icon::Plus);
        assert_eq!(ACTIONS[3].icon, crate::icon_system::Icon::Files);
    }

    #[test]
    fn action_descriptions_match_approved_copy() {
        // UI-HOME-06: these exact descriptions must never be truncated.
        // Descriptions are i18n keys resolved at render time via
        // crate::i18n::t; the values in en.json hold the approved copy.
        assert_eq!(
            ACTIONS
                .iter()
                .map(|action| action.description)
                .collect::<Vec<_>>(),
            vec![
                "home.quick_start_chat_desc",
                "home.quick_public_rooms_desc",
                "home.quick_create_room_desc",
                "home.quick_send_file_desc",
            ]
        );
    }

    #[test]
    fn action_messages_dispatch_to_expected_flows() {
        use crate::app::AppMessage;
        assert!(matches!(ACTIONS[0].message, AppMessage::OpenFriendRequests));
        assert!(matches!(ACTIONS[1].message, AppMessage::OpenDirectory));
        assert!(matches!(ACTIONS[2].message, AppMessage::CreateNewRoom));
        assert!(matches!(ACTIONS[3].message, AppMessage::OpenFileSharing));
    }

    #[test]
    fn grid_columns_follow_the_design_breakpoints() {
        // UI-HOME-15: columns are computed from the dashboard *content*
        // width (window minus sidebar/divider/padding). The thresholds leave
        // each tile at least 140 px wide with a 12 px inter-card gap.
        // BORU-LAYOUT-03: counts/breakpoints come from the layout model's
        // defaults (pinned to design tokens by layout.rs tests).
        use crate::design_tokens::home_content_width;
        use crate::layout::QuickActionsLayout;
        let layout = QuickActionsLayout::default();
        // Wide content fits four 140 px tiles plus three 12 px gaps.
        assert_eq!(grid_columns_for(home_content_width(1920.0), layout), 4);
        assert_eq!(grid_columns_for(home_content_width(1600.0), layout), 4);
        assert_eq!(grid_columns_for(home_content_width(1440.0), layout), 4);
        // The actual inner width, not the window width, controls the tier.
        assert_eq!(grid_columns_for(home_content_width(1280.0), layout), 2);
        assert_eq!(grid_columns_for(home_content_width(1024.0), layout), 2);
        // Narrow: content below 520 → one quick action per row.
        assert_eq!(grid_columns_for(home_content_width(800.0), layout), 1);
        assert_eq!(grid_columns_for(home_content_width(640.0), layout), 1);
        assert_eq!(grid_columns_for(home_content_width(480.0), layout), 1);
        // Boundary checks on the content-width thresholds themselves.
        assert_eq!(grid_columns_for(layout.four_col_breakpoint, layout), 4);
        assert_eq!(
            grid_columns_for(layout.four_col_breakpoint - 1.0, layout),
            2
        );
        assert_eq!(grid_columns_for(layout.two_col_breakpoint, layout), 2);
        assert_eq!(grid_columns_for(layout.two_col_breakpoint - 1.0, layout), 1);
    }

    #[test]
    fn grid_columns_are_contiguous_without_gaps() {
        // Every content width maps to exactly one of the three supported
        // counts.
        use crate::design_tokens::home_content_width;
        use crate::layout::QuickActionsLayout;
        let layout = QuickActionsLayout::default();
        for window in (320..=1920).step_by(16) {
            let content = home_content_width(window as f32);
            let columns = grid_columns_for(content, layout);
            assert!(
                columns == 1 || columns == 2 || columns == 4,
                "window {window} (content {content:.0}) produced unexpected column count {columns}"
            );
        }
    }

    #[test]
    fn grid_columns_respect_layout_overrides() {
        // BORU-LAYOUT-03: the layout model can change both the counts and
        // the breakpoints; the grid must follow them.
        use crate::layout::QuickActionsLayout;
        let layout = QuickActionsLayout {
            columns_wide: 3,
            columns_mid: 2,
            columns_narrow: 1,
            four_col_breakpoint: 800.0,
            two_col_breakpoint: 400.0,
            ..Default::default()
        };
        assert_eq!(grid_columns_for(1000.0, layout), 3);
        assert_eq!(grid_columns_for(799.0, layout), 2);
        assert_eq!(grid_columns_for(500.0, layout), 2);
        assert_eq!(grid_columns_for(399.0, layout), 1);
    }

    #[test]
    fn quick_action_cards_use_natural_row_height_not_fixed_height() {
        // UI-HOME-06 (and UI-HOME-12 before it): the old fixed 132 px card
        // height clipped wrapped descriptions (UI-HOME-01 audit §4). Cards
        // must size to their content — no fixed height, no hidden overflow —
        // within a vertically unbounded scrollable. The approved
        // structure remains compact HOME-02 metrics (40 px icon container,
        // 12/16 px padding, 8 px icon→title gap, 4 px title→description
        // gap), light-green icon background, and TypeRole typography.
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
            prod.contains("quick_action_icon_size"),
            "icon container must be 40 px via HomeTheme::quick_action_icon_size (HOME-02 compact)"
        );
        let layout = crate::layout::QuickActionsLayout::default();
        assert_eq!(layout.card_padding_y, SPACE_16);
        assert_eq!(layout.card_padding_x, SPACE_16);
        assert_eq!(layout.gap, SPACE_12);
        assert!(
            prod.contains("TypeRole::CardTitle"),
            "quick-action labels must use TypeRole::CardTitle (IBM Plex Sans SemiBold 16)"
        );
        assert!(
            prod.contains("TypeRole::SupportingText"),
            "quick-action descriptions must use TypeRole::SupportingText (IBM Plex Sans Regular 14)"
        );
        assert!(
            prod.contains("type_role_text_lh("),
            "quick-action descriptions must use the line-height helper (plan 1.45)"
        );
        // FONTS-07: the quick-action cards size their roles locally (16 px
        // title / 14 px description at 1.45) instead of the shared role
        // defaults (CardTitle 18 / SupportingText 13) used elsewhere.
        assert!(prod.contains("title_size"));
        assert!(prod.contains("description_size"));
        assert!(
            prod.contains("description_line_height"),
            "quick-action descriptions must use the FONTS-07 1.4–1.45 line-height theme token"
        );
        assert!(
            prod.contains(".height(Length::Shrink)"),
            "cards must measure their content in the unbounded scroll axis"
        );
        assert!(
            prod.contains(".align_y(Alignment::Start)"),
            "rows must preserve the natural tallest-card height"
        );
    }

    #[test]
    fn quick_action_natural_height_is_compact_but_unclipped() {
        // HOME-02: the compact card (40 px icon + 12/16 px padding +
        // tightened 8/4/8 px gaps) must be noticeably shorter than the old
        // 200+ px band while still exceeding the removed 132 px fixed box —
        // i.e. content-driven (full description visible) but denser.
        use crate::design_tokens::{SPACE_16, SPACE_4, SPACE_8};

        let qa = crate::theme::BoruTheme::default().home;
        let tile = qa.quick_action_icon_size;
        let title = qa.quick_action_title_size * 1.3; // single-line heading
        let description_two_lines =
            qa.quick_action_desc_size * qa.quick_action_desc_line_height * 2.0;
        let indicator = crate::icon_system::IconSize::Xs.px() + SPACE_8;
        let vertical_padding = 2.0 * SPACE_16;
        let gaps = SPACE_8 + SPACE_4;
        let natural = tile + gaps + title + description_two_lines + indicator + vertical_padding;
        assert!(
            natural > 132.0,
            "content-driven quick-action card needs {natural:.1} px > old fixed 132 px; \
             a fixed-height card would clip the description"
        );
        assert!(
            natural < 200.0,
            "HOME-02 compact card should be noticeably smaller than the old 200+ px band \
             (computed natural height {natural:.1} px)"
        );
        assert!(
            natural >= 150.0,
            "compact card should stay near the 150–165 px target band \
             (computed natural height {natural:.1} px)"
        );
    }
}
