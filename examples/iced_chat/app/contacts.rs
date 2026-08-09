//! Contacts / friend-requests feature.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the Friend Requests screen:
//! its Hash-compatible dependency snapshot (`FriendRequestsDependency`,
//! `FriendRequestRow`) and the `impl IcedChat` methods that build and render
//! it. Reads app state via `use super::*`; app.rs re-exports the pub(crate)
//! items it still references with `use contacts::*`.

use super::*;

/// Hash-compatible snapshot of one friend-request row rendered in the Friend
/// Requests screen. The live `FriendRequest` is not Hash, so the builder
/// pre-resolves the display label and copies the id + message.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct FriendRequestRow {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) message: String,
}

/// Dependency for the Friend Requests screen. Holds the Hash-compatible state
/// slice the screen renders: search input, error feedback, and pre-resolved
/// incoming/outgoing request rows.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct FriendRequestsDependency {
    pub(crate) dark_mode: bool,
    pub(crate) friend_request_search_input: String,
    pub(crate) chat_list_error: String,
    pub(crate) incoming: Vec<FriendRequestRow>,
    pub(crate) outgoing: Vec<FriendRequestRow>,
}

impl IcedChat {
    pub(crate) fn view_friend_requests(&self) -> iced::Element<'_, AppMessage> {
        let dep = self.friend_requests_dependency();
        iced::widget::lazy(dep, Self::view_friend_requests_content).into()
    }

    /// Build the Hash-compatible snapshot the Friend Requests screen renders
    /// from. Incoming/outgoing request rows are pre-resolved to display labels
    /// so the static content fn needs no live store access.
    pub(crate) fn friend_requests_dependency(&self) -> FriendRequestsDependency {
        let local_pk_str = self.local_public.to_string();
        let incoming = self
            .friend_request_store
            .list_incoming_by_status(&local_pk_str, FriendRequestStatus::Pending)
            .iter()
            .map(|req| {
                let label = self.resolve_name(
                    &PublicKey::from_str(&req.requester)
                        .unwrap_or_else(|_| iroh::SecretKey::generate().public()),
                );
                FriendRequestRow {
                    id: req.id.clone(),
                    label,
                    message: req.message.clone().unwrap_or_default(),
                }
            })
            .collect();
        let outgoing = self
            .friend_request_store
            .list_outgoing_by_status(&local_pk_str, FriendRequestStatus::Pending)
            .iter()
            .map(|req| {
                let recipient = PublicKey::from_str(&req.recipient).ok();
                let label = recipient
                    .as_ref()
                    .map(|pk| self.resolve_name(pk))
                    .unwrap_or_else(|| req.recipient.chars().take(12).collect());
                FriendRequestRow {
                    id: req.id.clone(),
                    label,
                    message: String::new(),
                }
            })
            .collect();
        FriendRequestsDependency {
            dark_mode: self.dark_mode,
            friend_request_search_input: self.friend_request_search_input.clone(),
            chat_list_error: self.chat_list_error.clone(),
            incoming,
            outgoing,
        }
    }

    /// Static renderer for the Friend Requests screen. Reads only from the
    /// [`FriendRequestsDependency`] snapshot.
    pub(crate) fn view_friend_requests_content(dep: &FriendRequestsDependency) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, row, text, text_input, Column, Space};
        use iced::{Alignment, Color, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        let muted = text_muted(&theme);

        let mut content = Column::new().spacing(SPACE_12).padding(SPACE_24);

        // ── Header ──
        let back_btn = button(crate::fonts::type_role_text(
            crate::fonts::TypeRole::ButtonLabel,
            "← Back",
        ))
        .on_press(AppMessage::CloseFriendRequests)
        .style(|t, _status| iced::widget::button::Style {
            background: Some(iced::Background::Color(bg_surface(t))),
            border: iced::Border {
                color: border_muted(t),
                width: 1.0,
                radius: SPACE_8.into(),
            },
            text_color: text_muted_style(t)
                .color
                .unwrap_or(iced::Color::from_rgb(0.6, 0.6, 0.6)),
            ..Default::default()
        })
        .padding([SPACE_8, SPACE_16]);

        content = content.push(
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::SectionTitle, "Friend Requests")
                    .width(Length::Fill),
                back_btn,
            ]
            .spacing(SPACE_8)
            .align_y(Alignment::Center),
        );

        content = content.push(Space::new().height(Length::Fixed(SPACE_16)));

        // ── Send a Friend Request ──
        let send_section = section_card(
            "SEND A FRIEND REQUEST",
            vec![
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "Enter the recipient's public key below and tap Send.",
                )
                .style(text_muted_style)
                .into(),
                row![
                    // The input value must be `'static` for the cached tree;
                    // leak a clone of the search text (small, bounded by the
                    // friend-request search input length).
                    text_input(
                        "Peer public key…",
                        Box::leak(dep.friend_request_search_input.clone().into_boxed_str()),
                    )
                        .on_input(AppMessage::FriendRequestSearchChanged)
                        .width(Length::Fill),
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Send",
                    ))
                    .on_press(AppMessage::FriendRequestSend(
                        dep.friend_request_search_input.clone()
                    ))
                    .padding([SPACE_6, SPACE_12])
                    .style(BUTTON_PRIMARY),
                ]
                .spacing(SPACE_8)
                .align_y(Alignment::Center)
                .into(),
            ],
        );
        content = content.push(send_section);

        content = content.push(Space::new().height(Length::Fixed(SPACE_12)));

        // ── Incoming Requests ──
        let incoming = &dep.incoming;
        let incoming_section = Column::new()
            .push(
                row![
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SectionTitle,
                        "Incoming Requests",
                    )
                    .width(Length::Fill),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        format!("{} pending", incoming.len()),
                    )
                    .color(muted),
                ]
                .spacing(SPACE_4),
            )
            .push(Space::new().height(Length::Fixed(SPACE_8)));

        if incoming.is_empty() {
            let empty_msg: iced::Element<'static, AppMessage> =
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Body,
                    "No incoming friend requests.",
                )
                .color(muted)
                .into();
            content = content.push(
                container(
                    Column::new()
                        .push(incoming_section)
                        .push(empty_msg)
                        .spacing(SPACE_4),
                )
                .width(Length::Fill)
                .padding(SPACE_12)
                .style(container_surface),
            );
        } else {
            let mut list = Column::new().spacing(SPACE_4);
            for req in incoming {
                let msg_display = &req.message;
                let row_el = row![
                    Column::new()
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                req.label.clone(),
                            )
                            .width(Length::Fill)
                        )
                        .push(if msg_display.is_empty() {
                            iced::widget::text("").into()
                        } else {
                            let msg: iced::Element<'static, AppMessage> =
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Metadata,
                                    format!("\"{msg_display}\""),
                                )
                                .color(muted)
                                .into();
                            msg
                        })
                        .spacing(SPACE_4),
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Accept",
                    ))
                    .on_press(AppMessage::FriendRequestAccept(req.id.clone()))
                    .padding([SPACE_6, SPACE_12])
                    .style(BUTTON_PRIMARY_GREEN),
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Decline",
                    ))
                    .on_press(AppMessage::FriendRequestDecline(req.id.clone()))
                    .padding([SPACE_6, SPACE_12])
                    .style(BUTTON_DANGER),
                ]
                .spacing(SPACE_8)
                .align_y(Alignment::Center)
                .padding(SPACE_8);
                list = list.push(container(row_el).width(Length::Fill).style(container_hover));
            }
            content = content.push(
                container(
                    Column::new()
                        .push(incoming_section)
                        .push(list)
                        .spacing(SPACE_4),
                )
                .width(Length::Fill)
                .padding(SPACE_12)
                .style(container_surface),
            );
        }

        content = content.push(Space::new().height(Length::Fixed(SPACE_12)));

        // ── Outgoing Requests ──
        let outgoing = &dep.outgoing;
        let outgoing_section = Column::new()
            .push(
                row![
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SectionTitle,
                        "Outgoing Requests",
                    )
                    .width(Length::Fill),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        format!("{} pending", outgoing.len()),
                    )
                    .color(muted),
                ]
                .spacing(SPACE_4),
            )
            .push(Space::new().height(Length::Fixed(SPACE_8)));

        if outgoing.is_empty() {
            let empty_msg: iced::Element<'static, AppMessage> =
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Body,
                    "No outgoing friend requests.",
                )
                .color(muted)
                .into();
            content = content.push(
                container(
                    Column::new()
                        .push(outgoing_section)
                        .push(empty_msg)
                        .spacing(SPACE_4),
                )
                .width(Length::Fill)
                .padding(SPACE_12)
                .style(container_surface),
            );
        } else {
            let mut list = Column::new().spacing(SPACE_4);
            for req in outgoing {
                let row_el = row![
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Body, req.label.clone())
                        .width(Length::Fill),
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Pending")
                        .color(Color::from_rgb(0.7, 0.6, 0.0)),
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Cancel",
                    ))
                    .on_press(AppMessage::FriendRequestCancel(req.id.clone()))
                    .padding([SPACE_4, SPACE_8])
                    .style(move |t, _status| {
                        iced::widget::button::Style {
                            text_color: color_error(t),
                            border: iced::Border {
                                color: color_error(t),
                                width: 1.0,
                                radius: SPACE_6.into(),
                            },
                            ..Default::default()
                        }
                    }),
                ]
                .spacing(SPACE_8)
                .align_y(Alignment::Center)
                .padding(SPACE_8);
                list = list.push(container(row_el).width(Length::Fill).style(container_hover));
            }
            content = content.push(
                container(
                    Column::new()
                        .push(outgoing_section)
                        .push(list)
                        .spacing(SPACE_4),
                )
                .width(Length::Fill)
                .padding(SPACE_12)
                .style(container_surface),
            );
        }

        // ── Error feedback ──
        if !dep.chat_list_error.is_empty() {
            content = content.push(Space::new().height(Length::Fixed(SPACE_8)));
            content = content.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Body,
                    dep.chat_list_error.clone(),
                )
                .color(color_error(&theme)),
            );
        }

        crate::ui_components::gutter_scrollable(container(content).width(Length::Fill).padding(SPACE_16))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
