//! Temporary developer component gallery — excluded from release navigation.
//!
//! Shows every primitive from `ui_components` in every applicable state.
//! Accessible only via `Screen::Gallery` (Ctrl+Shift+G in debug builds).

use iced::widget::{container, scrollable, text, Column, Row, Space};
use iced::{Alignment, Element, Length, Theme};

use crate::app::AppMessage;
use crate::design_tokens;
use crate::fonts::Typography;
use crate::icon_system::{Icon, IconSize};
use crate::ui_components::{
    self, badge, card_header, divider, elevated_card, empty_state, ghost_icon_button, icon_tile,
    primary_button, primary_button_icon, secondary_button, section_header, status_dot,
    text_input_field, Avatar, BadgeKind, Card, ListRow, StatusDotKind,
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
        .push(gallery_section("Elevated Card (Dialog)"))
        .push(dialog_example())
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
