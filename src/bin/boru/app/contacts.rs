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
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
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
            theme_revision: self.theme_revision,
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
        .style(|t, _status| {
            let b = crate::theme::BoruTheme::for_theme(t);
            iced::widget::button::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    color: border_muted(t),
                    width: b.borders.hairline,
                    radius: b.radii.sm.into(),
                },
                // BORU-UI-03: the muted fallback grey rgb(0.6,0.6,0.6) is
                // captured by ColorTokens::glyph_muted_dark in both modes.
                text_color: text_muted_style(t)
                    .color
                    .unwrap_or(b.colors.glyph_muted_dark),
                ..Default::default()
            }
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
                        .color(crate::theme::BoruTheme::for_theme(&theme).colors.request_pending),
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
                                width: crate::theme::BoruTheme::for_theme(t).borders.hairline,
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

    /// State-layer update for contacts / friend requests (BORU-AUDIT-22
    /// spec step 5).
    ///
    /// Handles the friend-requests screen actions (open/close, search, send,
    /// accept, decline, cancel, results) and friend events. The root
    /// `update()` dispatches these variants here via combined match arms.
    pub(crate) fn update_contacts(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::OpenFriendRequests => {
                // FILES-04: remember where the user came from so the back
                // button returns to the File Sharing dashboard (or whatever
                // screen the user was on) instead of always dumping to the
                // chat list.
                if !matches!(self.screen, Screen::FriendRequests) {
                    self.friend_requests_return_to = Some(self.screen.clone());
                }
                self.screen = Screen::FriendRequests;
                if let Some(action_id) = self.pending_open_friends_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                // Load the durable Recent Download Activity card data whenever
                // the dashboard becomes visible.
                self.refresh_dashboard_activity()
                    .chain(self.refresh_shared_by_me())
                    .chain(self.refresh_sharing_summary())
            }

            AppMessage::CloseFriendRequests => {
                self.screen = self.friend_requests_return_to.take().unwrap_or(Screen::ChatList);
                iced::Task::none()
            }

            AppMessage::FriendRequestSearchChanged(text) => {
                self.friend_request_search_input = text;
                iced::Task::none()
            }

            AppMessage::FriendRequestSend(peer_key) => {
                // Parse the public key from the input text
                match PublicKey::from_str(&peer_key) {
                    Ok(peer) => {
                        self.friend_request_search_input.clear();
                        iced::Task::done(AppMessage::SendFriendRequest(peer))
                    }
                    Err(_) => {
                        self.friend_request_error = format!("Invalid public key: {peer_key}");
                        iced::Task::none()
                    }
                }
            }

            AppMessage::FriendRequestAccept(request_id) => {
                // Forward to the existing IncomingFriendRequestAccept handler
                // by looking up the request to get the peer key
                let local_pk = self.local_public.to_string();
                let req_opt = self
                    .friend_request_store
                    .list_incoming_by_status(&local_pk, FriendRequestStatus::Pending)
                    .into_iter()
                    .find(|r| r.id == request_id)
                    .cloned();
                match req_opt {
                    Some(req) => {
                        let req_id = req.id.clone();
                        match self.friend_request_store.accept_request(&req_id, &local_pk) {
                            Ok(_) => {
                                self.requests_sidebar_revision =
                                    self.requests_sidebar_revision.wrapping_add(1);
                                self.send_save_friend_requests();
                                if let Ok(peer) = PublicKey::from_str(&req.requester) {
                                    iced::Task::done(AppMessage::IncomingFriendRequestProcessed {
                                        request_id: req_id,
                                        peer,
                                        status: FriendRequestStatus::Accepted,
                                    })
                                } else {
                                    iced::Task::none()
                                }
                            }
                            Err(err) => iced::Task::done(AppMessage::ErrorMsg(format!(
                                "Failed to accept friend request: {err}"
                            ))),
                        }
                    }
                    None => iced::Task::done(AppMessage::ErrorMsg(
                        "Friend request not found".to_string(),
                    )),
                }
            }

            AppMessage::FriendRequestDecline(request_id) => {
                let local_pk = self.local_public.to_string();
                let req_opt = self
                    .friend_request_store
                    .list_incoming_by_status(&local_pk, FriendRequestStatus::Pending)
                    .into_iter()
                    .find(|r| r.id == request_id)
                    .cloned();
                match req_opt {
                    Some(req) => {
                        let req_id = req.id.clone();
                        match self
                            .friend_request_store
                            .decline_request(&req_id, &local_pk)
                        {
                            Ok(_) => {
                                self.requests_sidebar_revision =
                                    self.requests_sidebar_revision.wrapping_add(1);
                                self.send_save_friend_requests();
                                if let Ok(peer) = PublicKey::from_str(&req.requester) {
                                    iced::Task::done(AppMessage::IncomingFriendRequestProcessed {
                                        request_id: req_id,
                                        peer,
                                        status: FriendRequestStatus::Declined,
                                    })
                                } else {
                                    iced::Task::none()
                                }
                            }
                            Err(err) => iced::Task::done(AppMessage::ErrorMsg(format!(
                                "Failed to decline friend request: {err}"
                            ))),
                        }
                    }
                    None => iced::Task::done(AppMessage::ErrorMsg(
                        "Friend request not found".to_string(),
                    )),
                }
            }

            AppMessage::FriendRequestCancel(request_id) => {
                let local_pk = self.local_public.to_string();
                match self
                    .friend_request_store
                    .cancel_request(&request_id, &local_pk)
                {
                    Ok(_) => {
                        self.send_save_friend_requests();
                        iced::Task::none()
                    }
                    Err(err) => iced::Task::done(AppMessage::ErrorMsg(format!(
                        "Failed to cancel friend request: {err}"
                    ))),
                }
            }

            AppMessage::FriendRequestSentResult(result) => {
                match result {
                    Ok(request) => {
                        // Request was sent successfully (this is from the earlier
                        // simple UI flow; the full whisper-based flow uses SendFriendRequest)
                        if let Ok(peer) = PublicKey::from_str(&request.recipient) {
                            self.outgoing_request_states
                                .insert(peer, OutgoingRequestState::Pending);
                        }
                        self.send_save_friend_requests();
                        self.rebuild_join_request_list();
                    }
                    Err(error) => {
                        self.friend_request_error = error;
                    }
                }
                iced::Task::none()
            }

            AppMessage::FriendRequestActionResult(result) => {
                if let Err(error) = result {
                    self.friend_request_error = error;
                }
                iced::Task::none()
            }

            AppMessage::FriendEvent(event) => {
                self.handle_friend_event(event);
                self.try_save_friends();
                iced::Task::none()
            }
            AppMessage::OpenPeerProfile(peer) => {
                if !matches!(self.screen, Screen::PeerProfile(peer) | Screen::PeerCatalogue(peer)) {
                    self.peer_profile_return_to = Some(self.screen.clone());
                }
                if !self.profile_cache.contains_key(&peer) {
                    // Create a minimal profile from the friend record as fallback,
                    // so the profile page is accessible even without gossip ProfileUpdate data.
                    let fid = FriendId::from_public_key(peer);
                    if let Some(record) = self.friends.get(&fid) {
                        self.profile_cache.insert(
                            peer,
                            PeerProfileData {
                                display_name: record.display_label(&fid, &peer),
                                bio: String::new(),
                                last_updated: SystemTime::UNIX_EPOCH,
                            },
                        );
                    }
                }
                self.screen = Screen::PeerProfile(peer);
                if self
                    .pending_select_peer_action
                    .as_ref()
                    .is_some_and(|(_, expected)| *expected == peer)
                {
                    if let Some((action_id, _)) = self.pending_select_peer_action.take() {
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::AppMessageHandled);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Completed);
                    }
                }
                iced::Task::none()
            }
            AppMessage::ClosePeerProfile => {
                self.screen = self.peer_profile_return_to.take().unwrap_or(Screen::ChatList);
                iced::Task::none()
            }
            AppMessage::OpenFriendProfile(peer) => {
                self.notifications_state.dismiss_toast();
                self.friend_profile_menu_open = false;
                self.friend_remove_confirm = false;
                self.friend_block_confirm = false;
                self.friend_profile_renaming = false;
                if !matches!(self.screen, Screen::FriendProfile(peer)) {
                    self.friend_profile_return_to = Some(self.screen.clone());
                }
                self.screen = Screen::FriendProfile(peer);
                iced::Task::none()
            }
            AppMessage::CloseFriendProfile => {
                self.notifications_state.dismiss_toast();
                self.friend_profile_menu_open = false;
                self.friend_remove_confirm = false;
                self.friend_block_confirm = false;
                self.friend_profile_renaming = false;
                self.screen = self.friend_profile_return_to.take().unwrap_or(Screen::ChatList);
                iced::Task::none()
            }
            AppMessage::ToggleFriendProfileMenu => {
                self.friend_profile_menu_open = !self.friend_profile_menu_open;
                iced::Task::none()
            }
            AppMessage::FriendRenameInputChanged(value) => {
                self.friend_profile_rename_input = value;
                iced::Task::none()
            }
            AppMessage::FriendRenameConfirm => {
                // Rename logic
                let new_name = self.friend_profile_rename_input.trim().to_string();
                if !new_name.is_empty() {
                    if let Screen::FriendProfile(peer) = &self.screen {
                        let fid = boru_core::friends::FriendId::from_public_key(*peer);
                        self.friends.set_label(fid, &new_name);
                        self.friends_sidebar_revision =
                            self.friends_sidebar_revision.wrapping_add(1);
                    }
                }
                self.friend_profile_renaming = false;
                iced::Task::none()
            }
            AppMessage::CopyPeerId(peer) => {
                let peer_str = peer.to_string();
                self.notifications_state.show_toast("Peer ID copied to clipboard".to_string(), 120); // ~2 seconds at 60fps
                self.friend_profile_menu_open = false;
                return iced::clipboard::write(peer_str);
            }
            AppMessage::FriendAdded {
                fid,
                label,
                was_new,
            } => {
                self.first_run = false;
                let friend_id = FriendId::new(fid);
                self.friends.ensure_friend(friend_id.clone());
                if let Ok(peer) = friend_id.parse_public_key() {
                    let authorized = self
                        .friends
                        .get(&friend_id)
                        .is_some_and(|record| record.relationship.can_message());
                    self.call_handle.set_peer_authorized(peer, authorized);
                }
                if self
                    .friends
                    .get(&friend_id)
                    .and_then(|r| r.label.clone())
                    .is_some()
                {
                    // Already has a label
                } else if label != friend_id.as_str().chars().take(12).collect::<String>() {
                    self.friends.set_label(friend_id, &label);
                }
                self.mark_friends_sidebar_dirty();
                if was_new {
                    self.push_system(format!("Added friend: {label}"));
                } else {
                    self.push_system(format!("Updated friend: {label}"));
                }
                self.try_save_friends();
                iced::Task::none()
            }
            AppMessage::RemoveFriend(peer) => {
                self.call_handle.set_peer_authorized(peer, false);
                let mgr = self.friend_mgr.clone();
                iced::Task::perform(
                    async move {
                        let removed = mgr.remove_friend(&peer).await.unwrap_or(false);
                        let label = if removed {
                            peer.fmt_short().to_string()
                        } else {
                            peer.to_string()
                        };
                        AppMessage::FriendRemoved { label }
                    },
                    |msg| msg,
                )
            }
            AppMessage::FriendRemoved { label } => {
                self.push_system(format!("Removed friend: {label}"));
                iced::Task::none()
            }
            AppMessage::FriendListResult(items) => {
                if items.is_empty() {
                    self.push_system("No friends tracked yet.");
                } else {
                    self.push_system(format!("Friends ({}):", items.len()));
                    for (peer, status) in &items {
                        self.push_system(format!("  {peer}: {status}"));
                    }
                }
                iced::Task::none()
            }
            AppMessage::CopyFriendId => {
                let pk = self.local_public.to_string();
                self.friend_id_copied = true;
                let clear_task = iced::Task::perform(
                    tokio::time::sleep(std::time::Duration::from_secs(2)),
                    |_| AppMessage::FriendIdCopiedClear,
                );
                return iced::Task::batch(vec![iced::clipboard::write(pk), clear_task]);
            }
            AppMessage::FriendIdCopiedClear => {
                self.friend_id_copied = false;
                iced::Task::none()
            }
            AppMessage::SendFriendRequest(peer) => {
                // Prevent duplicate pending requests
                let local_pk = self.local_public.to_string();
                let peer_pk = peer.to_string();
                match self
                    .friend_request_store
                    .send_request(&local_pk, &peer_pk, None)
                {
                    Ok(request) => {
                        self.outgoing_request_states
                            .insert(peer, OutgoingRequestState::Pending);
                        self.rebuild_join_request_list();

                        // Send the conversation invite via whisper
                        let fid = FriendId::from_public_key(peer);
                        let topic = direct_topic(&self.local_public, &peer);
                        let known_addrs = self
                            .friends
                            .get(&fid)
                            .map(|record| record.known_addrs.clone())
                            .unwrap_or_default();
                        let record = self.friends.ensure_friend(fid);
                        record.set_direct_conversation(topic, DirectConversationState::Pending);
                        let _room =
                            RoomStore::with_peers(&self.data_dir, topic, known_addrs.clone());
                        self.try_save_friends();
                        self.send_save_friend_requests();

                        let secret_key = self.secret_key.clone();
                        let whisper_handle = self.whisper_handle.clone();
                        // A friend request is a control-plane request, not a
                        // conversation invite.  Keeping these actions distinct is
                        // important: ConversationInvite is sent only after the
                        // recipient accepts, while FriendRequest must remain
                        // pending so it can be rendered and acted on locally.
                        let action = ContactAction::FriendRequest { name: None };
                        let payload = match SignedContactMessage::sign(&secret_key, &action) {
                            Ok(payload) => payload.into(),
                            Err(err) => {
                                return iced::Task::done(AppMessage::FriendRequestFailed {
                                    peer,
                                    error: format!("Could not create contact invite: {err}"),
                                });
                            }
                        };
                        info!(
                            peer = %peer.fmt_short(),
                            "dispatching whisper send_control for friend request"
                        );
                        iced::Task::batch(vec![iced::Task::perform(
                            async move { whisper_handle.send_control(peer, payload).await },
                            move |result| match result {
                                Ok(()) => AppMessage::FriendRequestSent {
                                    peer,
                                    request_id: request.id.clone(),
                                },
                                Err(e) => AppMessage::FriendRequestFailed {
                                    peer,
                                    error: e.to_string(),
                                },
                            },
                        )])
                    }
                    Err(FriendRequestError::DuplicatePending { existing_id }) => {
                        info!(
                            %existing_id,
                            peer = %peer.fmt_short(),
                            "outgoing friend request is duplicate pending — ignoring"
                        );
                        self.outgoing_request_states
                            .insert(peer, OutgoingRequestState::Pending);
                        self.rebuild_join_request_list();
                        iced::Task::none()
                    }
                    Err(err) => iced::Task::done(AppMessage::FriendRequestFailed {
                        peer,
                        error: err.to_string(),
                    }),
                }
            }

            AppMessage::FriendRequestSent {
                peer: _,
                request_id: _,
            } => {
                // Request was sent — keep state as Pending
                // Future: when we hear back via whisper, update state to Accepted/Declined
                // The pending request appears in the Friend Requests screen's
                // outgoing list; drop the pre-warmed tree so it is rebuilt.
                self.invalidate_prewarm(&[Screen::FriendRequests]);
                iced::Task::none()
            }

            AppMessage::FriendRequestFailed { peer, error } => {
                self.outgoing_request_states
                    .insert(peer, OutgoingRequestState::Failed(error));
                self.rebuild_join_request_list();
                iced::Task::none()
            }

            AppMessage::FriendRequestReceived {
                peer,
                request_id: _,
                status,
            } => {
                match status {
                    FriendRequestStatus::Accepted => {
                        self.outgoing_request_states
                            .insert(peer, OutgoingRequestState::Accepted);
                    }
                    FriendRequestStatus::Declined => {
                        self.outgoing_request_states
                            .insert(peer, OutgoingRequestState::Declined);
                    }
                    _ => {}
                }
                self.rebuild_join_request_list();
                // The incoming/outgoing request lists changed.
                self.invalidate_prewarm(&[Screen::FriendRequests]);
                iced::Task::none()
            }

            AppMessage::FriendRequestRetry(peer) => {
                // Re-send the friend request
                iced::Task::done(AppMessage::SendFriendRequest(peer))
            }

            AppMessage::IncomingFriendRequestAccept { request_id, peer } => {
                let local_pk = self.local_public.to_string();
                match self
                    .friend_request_store
                    .accept_request(&request_id, &local_pk)
                {
                    Ok(_) => {
                        self.requests_sidebar_revision =
                            self.requests_sidebar_revision.wrapping_add(1);
                        self.send_save_friend_requests();
                        iced::Task::done(AppMessage::IncomingFriendRequestProcessed {
                            request_id,
                            peer,
                            status: FriendRequestStatus::Accepted,
                        })
                    }
                    Err(err) => iced::Task::done(AppMessage::ErrorMsg(format!(
                        "Failed to accept friend request: {err}"
                    ))),
                }
            }

            AppMessage::IncomingFriendRequestDecline { request_id, peer } => {
                let local_pk = self.local_public.to_string();
                match self
                    .friend_request_store
                    .decline_request(&request_id, &local_pk)
                {
                    Ok(_) => {
                        self.requests_sidebar_revision =
                            self.requests_sidebar_revision.wrapping_add(1);
                        self.send_save_friend_requests();
                        iced::Task::done(AppMessage::IncomingFriendRequestProcessed {
                            request_id,
                            peer,
                            status: FriendRequestStatus::Declined,
                        })
                    }
                    Err(err) => iced::Task::done(AppMessage::ErrorMsg(format!(
                        "Failed to decline friend request: {err}"
                    ))),
                }
            }

            AppMessage::IncomingFriendRequestProcessed {
                request_id: _,
                peer,
                status,
            } => {
                if status.is_accepted() {
                    // Set up friend record with Active direct conversation
                    let fid = FriendId::from_public_key(peer);
                    let topic = direct_topic(&self.local_public, &peer);
                    let known_addrs = self
                        .friends
                        .get(&fid)
                        .map(|record| record.known_addrs.clone())
                        .unwrap_or_default();
                    let record = self.friends.ensure_friend(fid);
                    record.set_direct_conversation(topic, DirectConversationState::Active);
                    record.relationship = FriendRelationship::Friends;
                    self.call_handle.set_peer_authorized(peer, true);
                    let _room = RoomStore::with_peers(&self.data_dir, topic, known_addrs.clone());
                    self.try_save_friends();

                    // Show the accepted friend immediately in the sidebar.
                    self.peer_presence_map.insert(peer, now_ms().max(0) as u64);
                    self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                    self.mark_friends_sidebar_dirty();

                    // Send a ConversationInvite back to the original requester
                    // so they know the request was accepted and can join the topic.
                    let secret_key = self.secret_key.clone();
                    let whisper_handle = self.whisper_handle.clone();
                    let local_addr = self.endpoint.addr();
                    // Advertise our mailbox key alongside the invite.
                    let mailbox_key = self.local_mailbox_key;
                    let action = ContactAction::ConversationInvite {
                        topic,
                        addrs: vec![local_addr],
                    };
                    // Use BackgroundSubscribe for the direct conversation to avoid
                    // slow-path gossip subscription with WAL replay storm on startup.
                    // The conversation appears in the sidebar; user clicks to open.
                    let bootstrap_peers = self.discovered_peers.clone();
                    if let Ok(payload) = SignedContactMessage::sign(&secret_key, &action) {
                        let mut tasks: Vec<iced::Task<AppMessage>> = vec![
                            iced::Task::perform(
                                async move {
                                    let _ = whisper_handle.send_control(peer, payload.into()).await;
                                },
                                |_| AppMessage::Noop,
                            ),
                            iced::Task::done(AppMessage::BackgroundSubscribe(
                                topic,
                                bootstrap_peers.clone(),
                            )),
                        ];
                        // Also advertise our mailbox key so the friend can
                        // encrypt offline messages to us.
                        if let Some(mailbox) = mailbox_key {
                            let mb_action = ContactAction::MailboxAdvertise { mailbox };
                            if let Ok(mb_payload) =
                                SignedContactMessage::sign(&secret_key, &mb_action)
                            {
                                let wh = self.whisper_handle.clone();
                                tasks.push(iced::Task::perform(
                                    async move {
                                        let _ = wh.send_control(peer, mb_payload.into()).await;
                                    },
                                    |_| AppMessage::Noop,
                                ));
                            }
                        }
                        iced::Task::batch(tasks)
                    } else {
                        iced::Task::done(AppMessage::BackgroundSubscribe(topic, bootstrap_peers))
                    }
                } else {
                    iced::Task::none()
                }
            }
            AppMessage::OpenFriendChat(peer) => {
                // A Chat click is an explicit direct-chat invitation.  Do not
                // require a prior friend-request round trip: the recipient
                // treats this authenticated invitation as acceptance and opens
                // the same deterministic room automatically.
                let fid = FriendId::from_public_key(peer);
                let topic = direct_topic(&self.local_public, &peer);
                let known_addrs = self
                    .friends
                    .get(&fid)
                    .map(|record| record.known_addrs.clone())
                    .unwrap_or_default();
                let record = self.friends.ensure_friend(fid.clone());
                record.set_direct_conversation(topic, DirectConversationState::Active);
                // Opening a chat is NOT a friendship signal.  The peer stays in
                // the Discover section until a friend request is explicitly
                // accepted by both sides.  Do NOT set relationship=Friends here.
                self.conversation_store.upsert(ConversationEntry::new(
                    topic,
                    peer.to_string(),
                    record.display_label(&fid, &peer),
                ));
                self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                let _room = RoomStore::with_peers(&self.data_dir, topic, known_addrs.clone());
                self.mark_friends_sidebar_dirty();
                self.discovered_sidebar_revision = self.discovered_sidebar_revision.wrapping_add(1);
                self.try_save_friends();
                let action = ContactAction::ConversationInvite {
                    topic,
                    addrs: vec![self.endpoint.addr()],
                };
                let payload = match SignedContactMessage::sign(&self.secret_key, &action) {
                    Ok(payload) => payload,
                    Err(err) => {
                        return iced::Task::done(AppMessage::ErrorMsg(format!(
                            "Could not create chat invite: {err}"
                        )));
                    }
                };
                let whisper_handle = self.whisper_handle.clone();
                // Advertise our mailbox key alongside the chat invite so
                // the peer can encrypt offline messages to us.
                let mailbox_task: Option<iced::Task<AppMessage>> =
                    self.local_mailbox_key.map(|mailbox| {
                        let mailbox_action = ContactAction::MailboxAdvertise { mailbox };
                        match SignedContactMessage::sign(&self.secret_key, &mailbox_action) {
                            Ok(mailbox_payload) => {
                                let wh = whisper_handle.clone();
                                iced::Task::perform(
                                    async move { wh.send_control(peer, mailbox_payload.into()).await },
                                    |r| match r {
                                        Ok(()) => AppMessage::Noop,
                                        Err(_) => AppMessage::Noop,
                                    },
                                )
                            }
                            Err(_) => iced::Task::none(),
                        }
                    });
                let mut tasks: Vec<iced::Task<AppMessage>> = vec![
                    iced::Task::perform(
                        async move { whisper_handle.send_control(peer, payload.into()).await },
                        |result| match result {
                            Ok(()) => AppMessage::Noop,
                            Err(err) => {
                                AppMessage::ErrorMsg(format!("Could not send chat invite: {err}"))
                            }
                        },
                    ),
                    iced::Task::done(AppMessage::BackgroundSubscribe(
                        topic,
                        self.discovered_peers.clone(),
                    )),
                    // Do NOT also dispatch OpenRoom here: the slow-path
                    // subscription replays the gossip WAL for this topic,
                    // and combined with the BackgroundSubscribe above it
                    // double-subscribes the direct topic (WAL-replay storm,
                    // pending-events cap reached).  The conversation is
                    // already in the sidebar via conversation_store.upsert;
                    // the user opens it with a click, using the fast path.
                ];
                if let Some(t) = mailbox_task {
                    tasks.push(t);
                }
                iced::Task::batch(tasks)
            }
            // ── Friend confirm / block / rename (state layer) ──
            AppMessage::ShowRemoveFriendConfirm => {
                self.friend_remove_confirm = true;
                self.friend_profile_menu_open = false;
                iced::Task::none()
            }
            AppMessage::CancelRemoveFriend => {
                self.friend_remove_confirm = false;
                iced::Task::none()
            }
            AppMessage::ConfirmRemoveFriend => {
                self.friend_remove_confirm = false;
                if let Screen::FriendProfile(peer) = &self.screen {
                    let mgr = self.friend_mgr.clone();
                    let peer = *peer;
                    let label = self.resolve_name(&peer);
                    return iced::Task::perform(
                        async move {
                            let removed = mgr.remove_friend(&peer).await.unwrap_or(false);
                            if removed {
                                AppMessage::FriendRemoved { label }
                            } else {
                                AppMessage::FriendRemoved { label }
                            }
                        },
                        |msg| msg,
                    );
                }
                iced::Task::none()
            }
            AppMessage::ShowBlockFriendConfirm => {
                self.friend_block_confirm = true;
                self.friend_profile_menu_open = false;
                iced::Task::none()
            }
            AppMessage::CancelBlockFriend => {
                self.friend_block_confirm = false;
                iced::Task::none()
            }
            AppMessage::ShowRenameFriendInput => {
                self.friend_profile_renaming = true;
                self.friend_profile_menu_open = false;
                if let Screen::FriendProfile(peer) = &self.screen {
                    self.friend_profile_rename_input = self.resolve_name(peer);
                }
                iced::Task::none()
            }
            AppMessage::ConfirmBlockFriend => {
                self.friend_block_confirm = false;
                if let Screen::FriendProfile(peer) = &self.screen {
                    let fid = boru_core::friends::FriendId::from_public_key(*peer);
                    if let Some(record) = self.friends.get_mut(&fid) {
                        record.relationship = boru_core::friends::FriendRelationship::Blocked;
                        self.friends_sidebar_revision =
                            self.friends_sidebar_revision.wrapping_add(1);
                    }
                    self.call_handle.set_peer_authorized(*peer, false);
                    self.notifications_state.show_toast(format!("Blocked {}", self.resolve_name(peer)), 120);
                }
                iced::Task::none()
            }
            // ── Import friend from file (state layer) ──
            AppMessage::ImportFriendFromFile => iced::Task::perform(
                rfd::AsyncFileDialog::new()
                    .set_title("Select file with friend's public key")
                    .pick_file(),
                |file| {
                    if let Some(file) = file {
                        AppMessage::ImportFriendFromFilePicked(
                            file.path().to_string_lossy().to_string(),
                        )
                    } else {
                        AppMessage::Noop
                    }
                },
            ),
            AppMessage::ImportFriendFromFilePicked(path) => {
                if path.is_empty() {
                    return iced::Task::none();
                }
                // Read the file content (public key) and send a friend request
                match std::fs::read_to_string(&path) {
                    Ok(key) => {
                        let trimmed = key.trim().to_string();
                        if trimmed.is_empty() {
                            self.chat_list_error =
                                "File is empty — expected a public key.".to_string();
                        } else {
                            // Dispatch a FriendRequestSend with the key from the file
                            return iced::Task::done(AppMessage::FriendRequestSend(trimmed));
                        }
                    }
                    Err(e) => {
                        self.chat_list_error = format!("Failed to read file: {e}");
                    }
                }
                iced::Task::none()
            }
            // update() only dispatches the contacts variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
