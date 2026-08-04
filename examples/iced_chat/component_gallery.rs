//! Temporary developer component gallery — excluded from release navigation.
//!
//! Shows every primitive from `ui_components` in every applicable state.
//! Accessible only via `Screen::Gallery` (Ctrl+Shift+G in debug builds).

use iced::widget::{container, scrollable, text, Column, Row, Space};
use iced::{Alignment, Element, Length, Theme};

use crate::app::AppMessage;
use crate::boru_dialog::BoruDialog;
use crate::card_shell::{CardShell, CARD_ROW_HEIGHT};
use crate::design_tokens;
use crate::fonts::Typography;
use crate::icon_system::{Icon, IconSize};
use crate::ui_components::{
    self, badge, card_header, date_separator, divider, elevated_card, empty_state,
    ghost_icon_button, icon_tile, primary_button, primary_button_icon, secondary_button,
    section_header, status_dot, system_event_chip, text_input_field, Avatar, BadgeKind, Card,
    FileIdentityCell, InlineError, ListRow, LoadingSkeleton, MetricBlock, OverflowMenu,
    PeerChipStack, ProgressBar, ProgressKind, StatusDotKind, TabStrip, TableHeaderRow,
};

/// Build the complete component gallery view.
pub fn view_gallery() -> Element<'static, AppMessage> {
    let content = Column::new()
        .push(gallery_heading("Component Gallery"))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
        .push(gallery_section("Buttons"))
        .push(button_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Cards"))
        .push(card_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Card Shell (Figure 3 rail)"))
        .push(card_shell_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("List Rows"))
        .push(list_row_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Avatars"))
        .push(avatar_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Status Dots & Badges"))
        .push(status_and_badge_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Text Input"))
        .push(text_input_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Empty State"))
        .push(empty_state_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Section Header & Card Header"))
        .push(header_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Dividers"))
        .push(divider_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Timeline (Figure 4)"))
        .push(timeline_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Elevated Card (Dialog)"))
        .push(dialog_example())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("BoruDialog (Reusable Modal)"))
        .push(boru_dialog_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Tab Strip"))
        .push(tab_strip_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Progress Bar"))
        .push(progress_bar_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("File Identity Cell"))
        .push(file_identity_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Peer Chip Stack"))
        .push(peer_chip_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Metric Block"))
        .push(metric_block_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Loading Skeleton"))
        .push(loading_skeleton_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Inline Error & Retry"))
        .push(inline_error_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Table Header Row"))
        .push(table_header_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Overflow Menu"))
        .push(overflow_menu_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_24)))
        .push(gallery_section("Form Primitives (UI-RESTYLE-03)"))
        .push(form_gallery())
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_32)))
        .spacing(0);

    scrollable(
        container(content)
            .padding(design_tokens::SPACE_24)
            .width(Length::Fill),
    )
    .into()
}

fn gallery_heading(label: &str) -> Element<'static, AppMessage> {
    let owned = label.to_string();
    text(owned)
        .font(Typography::PageTitle.font())
        .size(Typography::PageTitle.size_px())
        .color(design_tokens::text_primary(&Theme::Light))
        .into()
}

fn gallery_section(label: &str) -> Element<'static, AppMessage> {
    let label_str = label.to_string();
    let label_el: Element<'_, AppMessage> = text(label_str)
        .font(Typography::SectionHeading.font())
        .size(Typography::SectionHeading.size_px())
        .color(design_tokens::primary(&Theme::Light))
        .into();
    Column::new()
        .push(label_el)
        .push(divider::<AppMessage>())
        .spacing(design_tokens::SPACE_8)
        .into()
}

fn state_label(label: &str) -> Element<'static, AppMessage> {
    let owned = label.to_string();
    text(owned)
        .size(Typography::SecondaryText.size_px())
        .color(design_tokens::text_muted(&Theme::Light))
        .into()
}

// ── Button gallery ────────────────────────────────────────────────────

fn button_gallery() -> Element<'static, AppMessage> {
    let row = Row::new()
        .push(button_pair(
            "Primary",
            primary_button("Primary", None, false),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(button_pair(
            "Primary disabled",
            primary_button("Disabled", None, true),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(button_pair(
            "Secondary",
            secondary_button("Secondary", None, false),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(button_pair(
            "Secondary disabled",
            secondary_button("Disabled", None, true),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(button_pair(
            "Primary + Icon",
            primary_button_icon(Icon::Plus, "Add", None, false),
        ))
        .spacing(0)
        .align_y(Alignment::Center)
        .wrap();

    Column::new()
        .push(row)
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_12)),
        )
        .push(
            Row::new()
                .push(button_pair(
                    "Ghost icon",
                    ghost_icon_button(
                        Icon::Settings,
                        IconSize::Md,
                        Some("Settings"),
                        None,
                        false,
                        false,
                    ),
                ))
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(button_pair(
                    "Ghost destructive",
                    ghost_icon_button(
                        Icon::Delete,
                        IconSize::Md,
                        Some("Delete"),
                        None,
                        false,
                        true,
                    ),
                ))
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(button_pair(
                    "Ghost disabled",
                    ghost_icon_button(Icon::Chat, IconSize::Md, None, None, true, false),
                ))
                .spacing(0)
                .align_y(Alignment::Center),
        )
        .spacing(0)
        .into()
}

fn button_pair(label: &str, btn: Element<'static, AppMessage>) -> Element<'static, AppMessage> {
    Column::new()
        .push(state_label(label))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(btn)
        .spacing(0)
        .align_x(Alignment::Center)
        .into()
}

// ── Card gallery ──────────────────────────────────────────────────────

fn card_gallery() -> Element<'static, AppMessage> {
    let card_content: Vec<Element<'static, AppMessage>> = vec![
        text("This is a card with some content.")
            .size(Typography::Body.size_px())
            .into(),
        primary_button("Action", None, false),
    ];

    let clickable_content: Vec<Element<'static, AppMessage>> =
        vec![text("Click me — I'm interactive!")
            .size(Typography::Body.size_px())
            .into()];

    let card_noop: AppMessage = AppMessage::Noop;

    Row::new()
        .push(
            Column::new()
                .push(state_label("Standard card"))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(Card::new(card_content).build(&Theme::Light))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("Clickable card"))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(
                    Card::new(clickable_content)
                        .on_press(card_noop)
                        .build(&Theme::Light),
                )
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("Icon tile"))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(icon_tile::<AppMessage>(Icon::Chat, IconSize::Lg, None))
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .align_y(Alignment::Start)
        .into()
}

// ── Card shell gallery (Figure 3 rail) ────────────────────────────────

/// A single demo row at the shared 48 px rail row height.
fn card_shell_row(label: &str, meta: &str) -> Element<'static, AppMessage> {
    container(
        Row::new()
            .push(
                text(label.to_string())
                    .font(Typography::Body.font())
                    .size(Typography::Body.size_px())
                    .color(design_tokens::text_primary(&Theme::Light)),
            )
            .push(Space::new().width(Length::Fill))
            .push(
                text(meta.to_string())
                    .font(Typography::SecondaryText.font())
                    .size(Typography::SecondaryText.size_px())
                    .color(design_tokens::text_muted(&Theme::Light)),
            )
            .spacing(design_tokens::SPACE_8)
            .align_y(Alignment::Center),
    )
    .height(Length::Fixed(CARD_ROW_HEIGHT))
    .width(Length::Fill)
    .align_y(Alignment::Center)
    .into()
}

fn card_shell_gallery() -> Element<'static, AppMessage> {
    let noop: AppMessage = AppMessage::Noop;

    // Empty state — title + count badge + caller-provided empty message.
    let empty_shell = CardShell::new("Online Peers", vec![])
        .count(0)
        .empty_message("No peers are online right now.")
        .build(&Theme::Light);

    // Populated state — 8 rows at 48 px exceed the bounded 140 px body, so a
    // vertical scrollbar appears instead of the card growing without bound.
    let rows: Vec<Element<'static, AppMessage>> = (1..=8)
        .map(|i| {
            card_shell_row(
                &format!("Peer {i}"),
                if i % 2 == 0 { "online" } else { "idle" },
            )
        })
        .collect();
    let populated_shell = CardShell::new("Online Peers", rows)
        .count(5)
        .on_view_all(noop)
        .max_height(140.0)
        .row_spacing(design_tokens::SPACE_4)
        .build(&Theme::Light);

    Row::new()
        .push(
            Column::new()
                .push(state_label("Empty state"))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(empty_shell)
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label(
                    "8 rows > max height → scrollbar, count badge, View all",
                ))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(populated_shell)
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .align_y(Alignment::Start)
        .into()
}

// ── List row gallery ──────────────────────────────────────────────────

fn list_row_gallery() -> Element<'static, AppMessage> {
    let noop: AppMessage = AppMessage::Noop;
    let theme = Theme::Light;

    let avatar_alice: Element<'static, AppMessage> = Avatar::<AppMessage>::new("Alice")
        .size(design_tokens::AVATAR_SM)
        .build();
    let avatar_bob: Element<'static, AppMessage> = Avatar::<AppMessage>::new("Bob")
        .size(design_tokens::AVATAR_SM)
        .build();
    let avatar_carol: Element<'static, AppMessage> = Avatar::<AppMessage>::new("Carol")
        .size(design_tokens::AVATAR_SM)
        .build();

    let dot_online: Element<'static, AppMessage> = status_dot(StatusDotKind::Online, 10.0);
    let badge_count: Element<'static, AppMessage> = badge("3", BadgeKind::Count);

    Column::new()
        .push(state_label("Default rows"))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(
            ListRow::<AppMessage>::new("Alice — with avatar & timestamp")
                .leading(avatar_alice)
                .secondary("Last seen 2m ago")
                .trailing(dot_online)
                .build(&theme),
        )
        .push(
            ListRow::<AppMessage>::new("Bob — with unread badge")
                .leading(avatar_bob)
                .secondary("Hey, are you free?")
                .trailing(badge_count)
                .build(&theme),
        )
        .push(
            ListRow::<AppMessage>::new("Carol — selected state")
                .leading(avatar_carol)
                .secondary("Selected row example")
                .selected(true)
                .build(&theme),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(state_label("Clickable row"))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(
            ListRow::<AppMessage>::new("Clickable row — full-width hit target")
                .secondary("Press anywhere on this row")
                .on_press(noop)
                .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Avatar gallery ────────────────────────────────────────────────────

fn avatar_gallery() -> Element<'static, AppMessage> {
    Row::new()
        .push(avatar_example("Alice", "Initial"))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_example("Bob Smith", "Two initials"))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_example("", "Empty (fallback ?)"))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_with_extra("Carol", "With online dot", true, None))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_with_extra(
            "Dave",
            "With unread badge",
            false,
            Some(5),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_12))
                .height(Length::Shrink),
        )
        .push(avatar_example("Eve", "Size: SM (36px)"))
        .spacing(0)
        .align_y(Alignment::End)
        .into()
}

fn avatar_example(name: &str, label: &str) -> Element<'static, AppMessage> {
    let mut avatar = Avatar::<AppMessage>::new(name);
    if name == "Eve" {
        avatar = avatar.size(design_tokens::AVATAR_SM);
    }

    Column::new()
        .push(avatar.build())
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(state_label(label))
        .spacing(0)
        .align_x(Alignment::Center)
        .into()
}

fn avatar_with_extra(
    name: &str,
    label: &str,
    online: bool,
    unread: Option<u32>,
) -> Element<'static, AppMessage> {
    let mut avatar = Avatar::<AppMessage>::new(name);
    if online {
        avatar = avatar.online_dot(true);
    }
    if let Some(count) = unread {
        avatar = avatar.unread_badge(count);
    }

    Column::new()
        .push(avatar.build())
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(state_label(label))
        .spacing(0)
        .align_x(Alignment::Center)
        .into()
}

// ── Status dots & badges ──────────────────────────────────────────────

fn status_and_badge_gallery() -> Element<'static, AppMessage> {
    Row::new()
        .push(badge_sample(
            "Online",
            status_dot::<AppMessage>(StatusDotKind::Online, 12.0),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Offline",
            status_dot::<AppMessage>(StatusDotKind::Offline, 12.0),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Warning",
            status_dot::<AppMessage>(StatusDotKind::Warning, 12.0),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_24))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Default",
            badge::<AppMessage>("Default", BadgeKind::Default),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Accent",
            badge::<AppMessage>("Accent", BadgeKind::Accent),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Count",
            badge::<AppMessage>("42", BadgeKind::Count),
        ))
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(badge_sample(
            "Danger",
            badge::<AppMessage>("Error", BadgeKind::Danger),
        ))
        .spacing(0)
        .align_y(Alignment::Center)
        .into()
}

fn badge_sample(label: &str, sample: Element<'static, AppMessage>) -> Element<'static, AppMessage> {
    Column::new()
        .push(sample)
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(state_label(label))
        .spacing(0)
        .align_x(Alignment::Center)
        .into()
}

// ── Text input gallery ────────────────────────────────────────────────

fn text_input_gallery() -> Element<'static, AppMessage> {
    Row::new()
        .push(
            Column::new()
                .push(state_label("Default"))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(text_input_field(
                    "Placeholder text…",
                    "",
                    |_| AppMessage::Noop,
                    false,
                ))
                .width(Length::Fixed(240.0)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("With value"))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(text_input_field(
                    "Placeholder…",
                    "Current value",
                    |_| AppMessage::Noop,
                    false,
                ))
                .width(Length::Fixed(240.0)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("Error state"))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(text_input_field(
                    "Placeholder…",
                    "bad input",
                    |_| AppMessage::Noop,
                    true,
                ))
                .width(Length::Fixed(240.0)),
        )
        .spacing(0)
        .align_y(Alignment::Start)
        .into()
}

// ── Empty state gallery ───────────────────────────────────────────────

fn empty_state_gallery() -> Element<'static, AppMessage> {
    container(
        Column::new()
            .push(state_label("Empty state with action"))
            .push(
                Space::new()
                    .width(Length::Shrink)
                    .height(Length::Fixed(8.0)),
            )
            .push(empty_state(
                Icon::Chat,
                "No conversations yet",
                "Start a new conversation to see it here.",
                Some("New Chat"),
                Some(AppMessage::Noop),
            ))
            .spacing(0),
    )
    .height(Length::Fixed(240.0))
    .into()
}

// ── Section header & card header ──────────────────────────────────────

fn header_gallery() -> Element<'static, AppMessage> {
    let action_btn = ghost_icon_button(
        Icon::Plus,
        IconSize::Sm,
        Some("Add"),
        Some(AppMessage::Noop),
        false,
        false,
    );

    Column::new()
        .push(state_label("Section header"))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(section_header("CONVERSATIONS", Some(action_btn)))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_16)),
        )
        .push(state_label("Card header"))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(card_header(
            Some(Icon::Chat),
            "Active Chats",
            Some("12"),
            BadgeKind::Accent,
            Some(ghost_icon_button(
                Icon::More,
                IconSize::Sm,
                None,
                Some(AppMessage::Noop),
                false,
                false,
            )),
        ))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(card_header(
            Some(Icon::AlertTriangle),
            "Error Monitoring",
            Some("3"),
            BadgeKind::Danger,
            None,
        ))
        .spacing(0)
        .into()
}

// ── Divider gallery ───────────────────────────────────────────────────

fn divider_gallery() -> Element<'static, AppMessage> {
    Column::new()
        .push(text("Above divider").size(Typography::Body.size_px()))
        .push(divider::<AppMessage>())
        .push(text("Below divider").size(Typography::Body.size_px()))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(state_label(
            "(Divider is a thin horizontal line between items above)",
        ))
        .spacing(design_tokens::SPACE_8)
        .into()
}

// ── Timeline (Figure 4) gallery ──────────────────────────────────────

fn timeline_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;
    let spacing = design_tokens::SPACE_24;

    // Sample timeline: date separators open each day; system-event chips use
    // the same muted/centred treatment as the real chat log. The caller
    // supplies label + accent — no classification logic lives in the chip.
    let timeline = Column::new()
        .push(date_separator("Today", &theme))
        .push(system_event_chip(
            "MEMBER",
            design_tokens::online(&theme),
            "Alice joined the chat.",
            &theme,
        ))
        .push(system_event_chip(
            "NAME",
            design_tokens::primary(&theme),
            "Alice renamed the room to Kitchen",
            &theme,
        ))
        .push(system_event_chip(
            "HELP",
            design_tokens::text_muted(&theme),
            "Usage: /help — show available commands",
            &theme,
        ))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_16)),
        )
        .push(date_separator("Yesterday", &theme))
        .push(system_event_chip(
            "NOTICE",
            design_tokens::color_warning(&theme),
            "Message delivery failed after 3 attempts.",
            &theme,
        ))
        .push(system_event_chip(
            "INFO",
            design_tokens::text_muted(&theme),
            "Invite sent to Bob.",
            &theme,
        ))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_16)),
        )
        .push(date_separator("Sunday, August 2, 2026", &theme))
        .push(system_event_chip(
            "MEMBER",
            design_tokens::online(&theme),
            "Chat joined.",
            &theme,
        ))
        .spacing(design_tokens::SPACE_4);

    Row::new()
        .push(
            Column::new()
                .push(state_label(
                    "Date separators: centered, muted, 12 px. Chips: muted surface, 1 px accent border.",
                ))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(timeline)
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(spacing))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("Chip inputs come from the caller:"))
                .push(
                    Space::new()
                        .width(Length::Shrink)
                        .height(Length::Fixed(4.0)),
                )
                .push(
                    container(
                        Column::new()
                            .push(state_label("system_event_chip(label, accent, body, theme)"))
                            .push(
                                Space::new()
                                    .width(Length::Shrink)
                                    .height(Length::Fixed(design_tokens::SPACE_4)),
                            )
                            .push(system_event_chip(
                                "MEMBER",
                                design_tokens::online(&theme),
                                "accent = online green",
                                &theme,
                            ))
                            .push(system_event_chip(
                                "NAME",
                                design_tokens::primary(&theme),
                                "accent = primary green",
                                &theme,
                            ))
                            .push(system_event_chip(
                                "HELP",
                                design_tokens::text_muted(&theme),
                                "accent = muted text",
                                &theme,
                            ))
                            .push(system_event_chip(
                                "NOTICE",
                                design_tokens::color_warning(&theme),
                                "accent = warning amber",
                                &theme,
                            ))
                            .spacing(design_tokens::SPACE_4),
                    )
                    .padding(design_tokens::SPACE_12)
                    .style(design_tokens::card_style),
                )
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .align_y(Alignment::Start)
        .into()
}

// ── Elevated card / dialog example ────────────────────────────────────

fn dialog_example() -> Element<'static, AppMessage> {
    let content = Column::new()
        .push(
            text("Elevated Card / Dialog")
                .font(Typography::SectionHeading.font())
                .size(Typography::SectionHeading.size_px()),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_8)),
        )
        .push(
            text("This is an elevated card with a higher drop shadow. Use for modals, dialogs, and popovers that need to float above other content.")
                .size(Typography::Body.size_px()),
        )
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(design_tokens::SPACE_12)),
        )
        .push(
            Row::new()
                .push(secondary_button("Cancel", None, false))
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_8))
                        .height(Length::Shrink),
                )
                .push(primary_button("Confirm", None, false))
                .spacing(0),
        )
        .spacing(0);

    container(elevated_card::<AppMessage>(vec![content.into()]))
        .width(Length::Fixed(400.0))
        .into()
}

// ── BoruDialog (reusable modal) gallery ─────────────────────────────

fn boru_dialog_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    let state_label = text("The reusable BoruDialog shell: header (title + subtitle + close), body, and footer (Cancel + primary). It is generic over Message and styled entirely from design_tokens.")
        .size(Typography::SecondaryText.size_px())
        .style(move |_| iced::widget::text::Style {
            color: Some(design_tokens::text_secondary(&Theme::Light)),
        });

    // Bound the full-screen modal overlay inside a fixed-height frame so the
    // gallery can demonstrate the backdrop + centred panel without taking over
    // the whole window.
    let modal = BoruDialog::new("Create Group Chat")
        .subtitle("Start a private group conversation")
        .push_body(text_input_field("Group name…", "", |_| AppMessage::Noop, false))
        .push_body(text_input_field(
            "Description (optional)…",
            "",
            |_| AppMessage::Noop,
            false,
        ))
        .push_body(
            text("Long form content scrolls internally inside the dialog instead of growing the panel.")
                .size(Typography::SecondaryText.size_px())
                .into(),
        )
        .scroll_body(120.0)
        .secondary("Cancel", AppMessage::Noop)
        .primary("Create", AppMessage::Noop)
        .on_close(AppMessage::Noop)
        .width(560.0)
        .build(&theme);

    let framed: Element<'static, AppMessage> = container(modal)
        .width(Length::Fill)
        .height(Length::Fixed(340.0))
        .into();

    Column::new()
        .push(state_label)
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_12)))
        .push(framed)
        .spacing(0)
        .into()
}

// ── Tab strip gallery ─────────────────────────────────────────────────

fn tab_strip_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    let tabs: Vec<(&str, AppMessage)> = vec![
        ("Shared by Me", AppMessage::Noop),
        ("Downloading", AppMessage::Noop),
        ("Downloaded", AppMessage::Noop),
        ("Shared w/ Me", AppMessage::Noop),
        ("Activity Log", AppMessage::Noop),
    ];

    Column::new()
        .push(state_label(
            "Active tab: Shared by Me (second tab is clickable for state demo)",
        ))
        .push(
            Space::new()
                .width(Length::Shrink)
                .height(Length::Fixed(4.0)),
        )
        .push(TabStrip::<AppMessage>::new(tabs).active(0).build(&theme))
        .spacing(0)
        .into()
}

// ── Progress bar gallery ──────────────────────────────────────────────

fn progress_bar_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Row::new()
        .push(
            Column::new()
                .push(state_label("0%"))
                .push(ProgressBar::<AppMessage>::new(0.0).build(&theme))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("45%"))
                .push(ProgressBar::<AppMessage>::new(0.45).build(&theme))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("100% (complete)"))
                .push(
                    ProgressBar::<AppMessage>::new(1.0)
                        .kind(ProgressKind::Complete)
                        .build(&theme),
                )
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .push(
            Column::new()
                .push(state_label("Paused"))
                .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
                .push(state_label("Indeterminate"))
                .push(
                    ProgressBar::<AppMessage>::new(0.0)
                        .indeterminate(true)
                        .build(&theme),
                )
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .into()
}

// ── File identity cell gallery ────────────────────────────────────────

fn file_identity_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Column::new()
        .push(state_label("PDF document"))
        .push(
            FileIdentityCell::<AppMessage>::new(
                Icon::Files,
                "QuarterlyReport.pdf",
                "application/pdf · 2.4 MB · shared 3h ago",
            )
            .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Image file"))
        .push(
            FileIdentityCell::<AppMessage>::new(
                Icon::Image,
                "vacation-photo-2024.jpg",
                "image/jpeg · 5.1 MB · downloaded yesterday",
            )
            .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Long name (truncated)"))
        .push(
            FileIdentityCell::<AppMessage>::new(
                Icon::Files,
                "VeryLongFileNameThatMightGetClippedByTheContainerOrTruncatedWithEllipsis.zip",
                "application/zip · 128 MB",
            )
            .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Peer chip stack gallery ───────────────────────────────────────────

fn peer_chip_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    let few: Vec<&str> = vec!["Alice", "Bob"];
    let many: Vec<&str> = vec!["Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace"];

    Column::new()
        .push(state_label("2 peers (no overflow)"))
        .push(PeerChipStack::<AppMessage>::new(few.clone()).build(&theme))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("7 peers (max 3 visible + overflow)"))
        .push(
            PeerChipStack::<AppMessage>::new(many)
                .max_visible(3)
                .build(&theme),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Long names truncated to 12 chars"))
        .push(
            PeerChipStack::<AppMessage>::new(vec!["AlexanderTheGreat", "ChristopherColumbus"])
                .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Metric block gallery ──────────────────────────────────────────────

fn metric_block_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Row::new()
        .push(
            Column::new()
                .push(state_label("Files shared"))
                .push(MetricBlock::<AppMessage>::new("42", "files shared").build(&theme))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("Data transferred (accented)"))
                .push(
                    MetricBlock::<AppMessage>::new("2.4 GB", "transferred")
                        .accent(design_tokens::primary)
                        .build(&theme),
                )
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("Active peers (success)"))
                .push(
                    MetricBlock::<AppMessage>::new("12", "active peers")
                        .accent(design_tokens::color_success)
                        .build(&theme),
                )
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .into()
}

// ── Loading skeleton gallery ──────────────────────────────────────────

fn loading_skeleton_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Column::new()
        .push(state_label("5-row skeleton (default 56 px row height)"))
        .push(LoadingSkeleton::<AppMessage>::new(5).build(&theme))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("3-row skeleton (compact 48 px)"))
        .push(
            LoadingSkeleton::<AppMessage>::new(3)
                .row_height(design_tokens::TABLE_ROW_HEIGHT_COMPACT)
                .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Inline error gallery ──────────────────────────────────────────────

fn inline_error_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Column::new()
        .push(state_label("Error message only"))
        .push(InlineError::new("Transfer failed: hash mismatch.").build(&theme))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Error with retry action"))
        .push(
            InlineError::new("Network error: peer disconnected.")
                .on_retry(AppMessage::Noop)
                .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Table header row gallery ──────────────────────────────────────────

fn table_header_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Column::new()
        .push(state_label("File table header (4 columns)"))
        .push(
            TableHeaderRow::<AppMessage>::new(vec![
                ("Name", None),
                ("Kind", Some(100.0)),
                ("Size", Some(80.0)),
                ("Shared", Some(100.0)),
            ])
            .build(&theme),
        )
        .spacing(0)
        .into()
}

// ── Overflow menu gallery ─────────────────────────────────────────────

fn overflow_menu_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    Row::new()
        .push(
            Column::new()
                .push(state_label("Normal"))
                .push(OverflowMenu::build(AppMessage::Noop, false, &theme))
                .width(Length::FillPortion(1)),
        )
        .push(
            Space::new()
                .width(Length::Fixed(design_tokens::SPACE_16))
                .height(Length::Shrink),
        )
        .push(
            Column::new()
                .push(state_label("Disabled"))
                .push(OverflowMenu::build(AppMessage::Noop, true, &theme))
                .width(Length::FillPortion(1)),
        )
        .spacing(0)
        .into()
}

// ── Form primitives gallery (UI-RESTYLE-03) ───────────────────────────

fn form_gallery() -> Element<'static, AppMessage> {
    let theme = Theme::Light;

    // Labels / helper / error text
    let labels = Column::new()
        .push(state_label("Field label"))
        .push(crate::form_components::form_label("Room name"))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Helper text"))
        .push(crate::form_components::helper_text("Alphanumeric, 3–40 chars."))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Error text"))
        .push(crate::form_components::error_text("Group name is required."))
        .width(Length::FillPortion(1));

    // Labelled text input — default / with value / error
    let text_inputs = Column::new()
        .push(state_label("Labelled text input — default"))
        .push(
            crate::form_components::TextInput::new(
                "Room name",
                "Room name…",
                "",
                |_| AppMessage::Noop,
            )
            .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("With value + helper"))
        .push(
            crate::form_components::TextInput::new(
                "Description",
                "Optional description…",
                "Weekly sync",
                |_| AppMessage::Noop,
            )
            .helper("Shown in the room directory.")
            .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Error state"))
        .push(
            crate::form_components::TextInput::new(
                "Group name",
                "Group name…",
                "",
                |_| AppMessage::Noop,
            )
            .error("Group name is required.")
            .build(),
        )
        .width(Length::FillPortion(1));

    let toggles = Column::new()
        .push(state_label("Checkbox"))
        .push(crate::form_components::checkbox_field(
            "Enable DHT discovery",
            true,
            |_| AppMessage::Noop,
            None,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Toggle / switch"))
        .push(crate::form_components::toggle_field(
            "Advertise in Directory",
            true,
            |_| AppMessage::Noop,
            None,
        ))
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Chips (selected peers)"))
        .push(
            Row::new()
                .push(crate::form_components::remove_chip("Alice", Some(AppMessage::Noop)))
                .push(crate::form_components::remove_chip("Bob", None))
                .spacing(design_tokens::SPACE_4),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Selection summary"))
        .push(crate::form_components::selection_summary(3, "participant"))
        .width(Length::FillPortion(1));

    let selectable_rows = Column::new()
        .push(state_label("Selectable peer list (bordered panel)"))
        .push(
            crate::form_components::peer_list(
                vec![
                    crate::form_components::SelectablePeerRow::new("Alice")
                        .secondary("abc123…")
                        .selected(true)
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                    crate::form_components::SelectablePeerRow::new("Bob")
                        .secondary("def456…")
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                    crate::form_components::SelectablePeerRow::new("Carol")
                        .secondary("7890ab…")
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                ],
                160.0,
                Some("No peers available to add right now."),
            ),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("SelectablePeerList (search + chips + summary)"))
        .push(
            crate::form_components::SelectablePeerList::new(
                vec![
                    crate::form_components::SelectablePeerRow::new("Alice")
                        .secondary("abc123…")
                        .selected(true)
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                    crate::form_components::SelectablePeerRow::new("Bob")
                        .secondary("def456…")
                        .on_toggle(AppMessage::Noop)
                        .build(&theme),
                ],
                120.0,
                Some("No peers available to add right now."),
            )
            .search("Search participants…", "", |_| AppMessage::Noop)
            .chips(vec![crate::form_components::remove_chip(
                "Alice",
                Some(AppMessage::Noop),
            )])
            .summary(1, "participant")
            .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Dialog footer"))
        .push(
            crate::form_components::DialogFooter::new()
                .cancel("Cancel", AppMessage::Noop)
                .confirm("Create", AppMessage::Noop)
                .build(),
        )
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_8)))
        .push(state_label("Destructive button"))
        .push(crate::form_components::destructive_button(
            "Remove",
            Some(AppMessage::Noop),
            false,
        ))
        .width(Length::FillPortion(1));

    // Form section wrapping a couple of fields
    let section = crate::form_components::FormSection::new("Room Details")
        .helper("These settings control who can find and join the room.")
        .push(
            crate::form_components::TextInput::new(
                "Room name",
                "Room name…",
                "Design Sync",
                |_| AppMessage::Noop,
            )
            .build(),
        )
        .push(crate::form_components::checkbox_field(
            "Advertise in Directory",
            true,
            |_| AppMessage::Noop,
            None,
        ))
        .build();

    Column::new()
        .push(state_label("Form section"))
        .push(section)
        .push(Space::new().height(Length::Fixed(design_tokens::SPACE_16)))
        .push(
            Row::new()
                .push(labels)
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(text_inputs)
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(toggles)
                .push(
                    Space::new()
                        .width(Length::Fixed(design_tokens::SPACE_16))
                        .height(Length::Shrink),
                )
                .push(selectable_rows)
                .align_y(Alignment::Start),
        )
        .spacing(0)
        .into()
}
