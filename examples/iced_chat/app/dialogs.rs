//! Shared shell/dialog overlays.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the root overlay/dialog
//! views that wrap the base layout: incoming-call overlay, expanded inline
//! video, connection details, image lightbox, and the create room / group /
//! tunnel / receive ticket / short code / redeem code / invite member
//! dialogs. Reads app state via `use super::*`; app.rs re-exports the
//! pub(crate) items it still references with `use dialogs::*`.

use super::*;

impl IcedChat {
    pub(crate) fn view_incoming_call_overlay<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{button, column, container, row, text};
        use iced::{Alignment, Length};
        let Some(call) = self.incoming_call else { return base.into(); };
        let name = self.resolve_name(&call.peer);
        let kind = match call.kind {
            CallKind::Voice => crate::i18n::t("calls.incoming_voice"),
            CallKind::Video => crate::i18n::t("calls.incoming_video"),
        };
        let avatar: iced::Element<'a, AppMessage> = self.friend_image_handles.get(&call.peer).and_then(|h| h.clone())
            .map(|h| {
                let avatar_size = crate::theme::BoruTheme::default().dialogs.avatar_size;
                iced::widget::image(h).width(Length::Fixed(avatar_size)).height(Length::Fixed(avatar_size)).into()
            })
            .unwrap_or_else(|| {
                let glyph = crate::theme::BoruTheme::default().dialogs.avatar_glyph_size;
                text("👤").size(glyph).into()
            });
        let dialogs = crate::theme::BoruTheme::default().dialogs;
        let card = container(column![
            avatar,
            text(name).size(crate::theme::BoruTheme::default().typography.dialog_title),
            text(kind).size(crate::theme::BoruTheme::default().typography.body),
            row![
                button(text(crate::i18n::t("calls.decline"))).on_press(AppMessage::RejectIncomingCall(call.call_id)),
                button(text(crate::i18n::t("calls.accept"))).on_press(AppMessage::AcceptIncomingCall(call.call_id)),
            ]
            .spacing(dialogs.control_spacing)
        ]
        .spacing(dialogs.spacing)
        .align_x(Alignment::Center))
        .padding(dialogs.padding)
        .style(|t| {
            let theme = crate::theme::BoruTheme::for_theme(t);
            iced::widget::container::Style {
                background: Some(iced::Background::Color(theme.colors.dialog_panel_bg)),
                border: iced::Border {
                    color: theme.colors.dialog_panel_border,
                    width: theme.borders.hairline,
                    radius: theme.radii.dialog.into(),
                },
                ..Default::default()
            }
        });
        let overlay = container(card).width(Length::Fill).height(Length::Fill)
            .center_x(Length::Fill).center_y(Length::Fill)
            .style(|t| iced::widget::container::Style {
                background: Some(iced::Background::Color(
                    crate::theme::BoruTheme::for_theme(t).colors.incoming_call_backdrop,
                )),
                ..Default::default()
            });
        iced::widget::stack![base, overlay].into()
    }

    #[cfg(feature = "video-playback")]
    pub(crate) fn view_expanded_inline_video<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{button, column, container, stack, text};
        use iced::Length;

        let Some(session) = self.inline_video.as_ref() else {
            return base.into();
        };
        let Some(video) = session.video.as_ref() else {
            return base.into();
        };
        let Some((entry_index, entry)) = self.entries.iter().enumerate().find(|(_, entry)| {
            entry.event_id == session.key.message_id
                && entry
                    .download
                    .as_ref()
                    .is_some_and(|download| download.name == session.key.attachment_id)
        }) else {
            return base.into();
        };
        let Some(attachment) = entry.download.as_ref() else {
            return base.into();
        };
        let player = crate::download_progress_view::view_download_progress_with_player(
            entry_index,
            attachment,
            self.dark_mode,
            false,
            Some(video.as_ref()),
            false,
            self.inline_video_seek,
            true,
            true,
            entry.timestamp,
            // The expanded overlay fills the whole window, so the card sizes
            // against the tracked window width (Task 15 responsive band).
            self.window_width,
            // BORU-LAYOUT-05: the expanded-video dialog is an app overlay, not
            // a chat surface — it keeps the default video-card placement.
            crate::layout::ComponentPlacement::video_card_default(),
        );
        let panel = container(
            column![
                iced::widget::row![
                    crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Expanded video"),
                    iced::widget::Space::new().width(Length::Fill),
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Close expanded video",
                    ))
                    .on_press(AppMessage::InlineVideoToggleExpanded)
                    .padding([SPACE_6, SPACE_10]),
                ]
                .align_y(iced::Alignment::Center),
                player,
            ]
            .spacing(SPACE_8),
        )
        .width(Length::FillPortion(9))
        .height(Length::FillPortion(9))
        .padding(SPACE_12)
        .style(|t| iced::widget::container::Style {
            background: Some(iced::Background::Color(bg_surface(t))),
            border: iced::Border {
                color: border_muted(t),
                width: 1.0,
                radius: SPACE_10.into(),
            },
            ..Default::default()
        });
        let overlay = container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .padding(SPACE_16)
            .style(|t| iced::widget::container::Style {
                background: Some(iced::Background::Color(
                    crate::theme::BoruTheme::for_theme(t).colors.expanded_video_backdrop,
                )),
                ..Default::default()
            });
        stack![base, overlay].into()
    }

    /// Responsive dialog width: the preferred width, capped so the dialog
    /// stays fully inside smaller desktop windows (48 px of horizontal
    /// margin, with a 320 px floor).
    pub(crate) fn dialog_width(&self, preferred: f32) -> f32 {
        preferred.min((self.window_width - 48.0).max(320.0))
    }

    /// Shared scroll viewport for modal bodies. Keeping the footer outside
    /// this viewport makes primary actions reachable when a form grows.
    pub(crate) fn dialog_body_max_height(&self) -> f32 {
        self.boru_layout()
            .responsive
            .dialog_body_max_height_for_size(self.window_width, self.window_height)
    }

    /// Wrap the base layout in an overlay showing the advanced connection details.
    pub(crate) fn view_connection_details_dialog<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        let Some(state) = self.connection_details_dialog.as_ref() else {
            return base.into();
        };

        let dialog = connection_details::view(
            state,
            self.connection_details_announcement.as_deref(),
            |action| match action {
                ConnectionDetailsDialogAction::Close => AppMessage::CloseConnectionDetails,
                ConnectionDetailsDialogAction::CopyDetails => AppMessage::CopyConnectionDetails,
                ConnectionDetailsDialogAction::CopyValue { label, value } => {
                    AppMessage::CopyConnectionDetailsValue {
                        label: label.to_string(),
                        value,
                    }
                }
            },
            |_| AppMessage::Noop,
        );

        iced::widget::stack![base, dialog].into()
    }

    /// Full-screen image lightbox overlay.
    /// Shows the image at a large size on a dark backdrop.
    /// Click anywhere to dismiss.
    pub(crate) fn view_image_lightbox<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
        entry_index: usize,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{container, image, mouse_area, stack};
        use iced::Length;

        let Some(entry) = self.entries.get(entry_index) else {
            return base.into();
        };

        let dark_mode = self.dark_mode;
        let _theme = Self::theme_from_dark(dark_mode);

        // Large content element: animated GIF widget when frames exist,
        // otherwise the cached static image handle.
        let content: iced::Element<'a, AppMessage> =
            if let Some(frames) = entry.gif_frames.as_deref() {
                iced_moving_picture::widget::gif::Gif::new(frames)
                    .content_fit(iced::ContentFit::Contain)
                    .width(Length::FillPortion(3))
                    .height(Length::FillPortion(3))
                    .into()
            } else if let Some(handle) = self.image_handle_for_entry(entry) {
                image(handle)
                    .content_fit(iced::ContentFit::Contain)
                    .width(Length::FillPortion(3))
                    .height(Length::FillPortion(3))
                    .into()
            } else {
                return base.into();
            };

        // Dark backdrop that dismisses on click
        let backdrop = mouse_area(
            container(content)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .on_press(AppMessage::CloseImageLightbox);

        let overlay = container(backdrop)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |t| iced::widget::container::Style {
                background: Some(iced::Background::Color(
                    crate::theme::BoruTheme::for_theme(t).colors.lightbox_backdrop,
                )),
                ..Default::default()
            });

        stack![base, overlay].into()
    }

    /// Friends that can currently be messaged, as `(peer, display_label)`
    /// pairs in friends-store order. Shared by the peer-picker dialogs
    /// (create group, create tunnel, invite member); callers sort or filter
    /// as needed.
    pub(crate) fn messageable_friends(&self) -> Vec<(PublicKey, String)> {
        self.friends
            .iter()
            .filter_map(|(fid, record)| {
                if !record.relationship.can_message() {
                    return None;
                }
                let peer = fid.parse_public_key().ok()?;
                let label = record.display_label(fid, &peer);
                Some((peer, label))
            })
            .collect()
    }

    /// Boru-styled dialog for creating a new room.
    ///
    /// Restyled per UI-RESTYLE-05 and extended per BORU-DIR-05 (PDF Task
    /// 2.2): the creator explicitly chooses the room's visibility
    /// (Private / Public-Unlisted / Public-Discoverable) and can add an
    /// optional description and tags that are validated and normalized
    /// against the advertisement bounds before broadcast.
    pub(crate) fn view_create_room_dialog<'a>(
        &self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::{FormSection, TextInput, checkbox_field, helper_text, radio_field};
        use boru_core::control_plane::advertisement::{
            DEFAULT_MAX_DESCRIPTION_LEN, DEFAULT_MAX_ROOM_NAME_LEN, DEFAULT_MAX_TAG_LEN,
            DEFAULT_MAX_TAGS, RoomVisibility,
        };

        let theme = Self::theme_from_dark(self.dark_mode);

        // ── Room Details ────────────────────────────────────────────────
        let name_valid = !self.create_room_name.trim().is_empty();
        let submitting = self.create_room_submitting;
        let mut name_field = TextInput::new(
            "Room Name",
            "Room name…",
            &self.create_room_name,
            AppMessage::CreateNewRoomNameChanged,
        )
        .id(CREATE_ROOM_NAME_INPUT)
        .helper(format!(
            "A short name others will see in the directory (max {DEFAULT_MAX_ROOM_NAME_LEN} characters)."
        ));
        if let Some(error) = &self.create_room_error {
            name_field = name_field.error(error.clone());
        }
        // Enter submits only when the form is valid and not mid-submit.
        if name_valid && !submitting {
            name_field = name_field.on_submit(AppMessage::ConfirmCreateNewRoom);
        }
        let description_field = TextInput::new(
            "Description",
            "Optional — what is this room about?",
            &self.create_room_description,
            AppMessage::CreateNewRoomDescriptionChanged,
        )
        .helper(format!(
            "Optional short description shown in the directory (max {DEFAULT_MAX_DESCRIPTION_LEN} characters)."
        ));
        let tags_field = TextInput::new(
            "Tags",
            "Optional — e.g. rust, gaming",
            &self.create_room_tags,
            AppMessage::CreateNewRoomTagsChanged,
        )
        .helper(format!(
            "Optional comma-separated tags used to find the room (up to {DEFAULT_MAX_TAGS} tags, {DEFAULT_MAX_TAG_LEN} characters each)."
        ));
        let room_details = FormSection::new(crate::i18n::t("dialogs.create_room.room_details"))
            .push(name_field.build())
            .push(description_field.build())
            .push(tags_field.build());

        // ── Visibility / Discovery ──────────────────────────────────────
        let visibility = FormSection::new(crate::i18n::t("dialogs.create_room.visibility"))
            .helper(crate::i18n::t("dialogs.create_room.visibility_helper"))
            .push(radio_field(
                "Private",
                RoomVisibility::Private,
                Some(self.create_room_visibility),
                AppMessage::CreateNewRoomVisibilityChanged,
                Some("Invite-only. No directory listing; join by invitation or authorization."),
            ))
            .push(radio_field(
                "Public — Unlisted",
                RoomVisibility::PublicUnlisted,
                Some(self.create_room_visibility),
                AppMessage::CreateNewRoomVisibilityChanged,
                Some("Not listed in the directory. Others can join with the room ID, invite, or link."),
            ))
            .push(radio_field(
                "Public — Discoverable",
                RoomVisibility::PublicDiscoverable,
                Some(self.create_room_visibility),
                AppMessage::CreateNewRoomVisibilityChanged,
                Some("Listed in the directory. Other Boru users can find and join it."),
            ))
            .push(checkbox_field(
                crate::i18n::t("dialogs.create_room.dht_discovery"),
                self.create_room_dht_enabled,
                AppMessage::CreateNewRoomDhtToggled,
                Some(crate::i18n::t("dialogs.create_room.dht_discovery_helper")),
            ));

        // ── Access / Participation Options ──────────────────────────────
        // Public rooms are open by design; the backend exposes no join
        // limits, invite gates, or access rules, so this section is helper
        // text only.
        let access = FormSection::new(crate::i18n::t("dialogs.create_room.access")).push(helper_text(
            &crate::i18n::t("dialogs.create_room.access_helper"),
        ));

        // ── Preview / Info ──────────────────────────────────────────────
        let info = FormSection::new(crate::i18n::t("dialogs.create_room.preview")).push(helper_text(
            &crate::i18n::t("dialogs.create_room.preview_helper"),
        ));

        let overlay = BoruDialog::new(crate::i18n::t("dialogs.create_room.dialog_title"))
            .subtitle(crate::i18n::t("dialogs.create_room.dialog_subtitle"))
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(room_details.build())
            .push_body(visibility.build())
            .push_body(access.build())
            .push_body(info.build())
            .secondary("Cancel", AppMessage::CancelCreateRoom)
            .secondary_enabled(!submitting)
            .primary(
                if submitting { "Creating…" } else { "Create Room" },
                AppMessage::ConfirmCreateNewRoom,
            )
            .primary_enabled(name_valid && !submitting)
            .on_backdrop(AppMessage::CancelCreateRoom)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    /// Boru-styled dialog for editing an existing room's directory settings
    /// (BORU-DIR-06, PDF Task 2.3).
    ///
    /// Owner/admin-only (the caller gates opening on
    /// [`IcedChat::is_room_directory_owner`]). Lets the owner switch the
    /// room between Public-Discoverable and Public-Unlisted and edit the
    /// advertised metadata (name / description / tags). Changes are
    /// validated + normalized before save; on save the room is republished
    /// when discoverable, or unlisted (TTL expiry) when switched to unlisted.
    pub(crate) fn view_room_settings_dialog<'a>(
        &self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::{FormSection, TextInput, helper_text, radio_field};
        use boru_core::control_plane::advertisement::{
            DEFAULT_MAX_DESCRIPTION_LEN, DEFAULT_MAX_ROOM_NAME_LEN, DEFAULT_MAX_TAG_LEN,
            DEFAULT_MAX_TAGS, RoomVisibility,
        };

        let theme = Self::theme_from_dark(self.dark_mode);

        // ── Room Details ────────────────────────────────────────────────
        let name_valid = !self.room_settings_name.trim().is_empty();
        let mut name_field = TextInput::new(
            "Room Name",
            "Room name…",
            &self.room_settings_name,
            AppMessage::RoomSettingsNameChanged,
        )
        .helper(format!(
            "A short name others will see in the directory (max {DEFAULT_MAX_ROOM_NAME_LEN} characters)."
        ));
        if let Some(error) = &self.room_settings_error {
            name_field = name_field.error(error.clone());
        }
        if name_valid {
            name_field = name_field.on_submit(AppMessage::ConfirmRoomSettings);
        }
        let description_field = TextInput::new(
            "Description",
            "Optional — what is this room about?",
            &self.room_settings_description,
            AppMessage::RoomSettingsDescriptionChanged,
        )
        .helper(format!(
            "Optional short description shown in the directory (max {DEFAULT_MAX_DESCRIPTION_LEN} characters)."
        ));
        let tags_field = TextInput::new(
            "Tags",
            "Optional — e.g. rust, gaming",
            &self.room_settings_tags,
            AppMessage::RoomSettingsTagsChanged,
        )
        .helper(format!(
            "Optional comma-separated tags used to find the room (up to {DEFAULT_MAX_TAGS} tags, {DEFAULT_MAX_TAG_LEN} characters each)."
        ));
        let room_details = FormSection::new("Room Details")
            .push(name_field.build())
            .push(description_field.build())
            .push(tags_field.build());

        // ── Visibility / Discovery ──────────────────────────────────────
        // Only the two public states are offered: the directory controls
        // switch PublicDiscoverable <-> PublicUnlisted; Private rooms are
        // created private and are never part of the directory.
        let visibility = FormSection::new("Directory Visibility")
            .helper(
                "Choose how other Boru users can find this room. Discoverable rooms are advertised; unlisted rooms require a room ID/invite/link.",
            )
            .push(radio_field(
                "Public — Unlisted",
                RoomVisibility::PublicUnlisted,
                Some(self.room_settings_visibility),
                AppMessage::RoomSettingsVisibilityChanged,
                Some("Not listed in the directory. Others can join with the room ID, invite, or link."),
            ))
            .push(radio_field(
                "Public — Discoverable",
                RoomVisibility::PublicDiscoverable,
                Some(self.room_settings_visibility),
                AppMessage::RoomSettingsVisibilityChanged,
                Some("Listed in the directory. Other Boru users can find and join it."),
            ))
            .push(helper_text(
                "Switching to Discoverable publishes a fresh advertisement immediately. Switching to Unlisted stops refreshing; remote directories drop the room after the advertisement TTL (no withdrawal message yet).",
            ));

        let overlay = BoruDialog::new("Room Settings")
            .subtitle("Change how this room appears in the directory.")
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(room_details.build())
            .push_body(visibility.build())
            .secondary("Cancel", AppMessage::CancelRoomSettings)
            .primary(
                "Save",
                AppMessage::ConfirmRoomSettings,
            )
            .primary_enabled(name_valid)
            .on_backdrop(AppMessage::CancelRoomSettings)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    /// Dialog for creating a new group with name, description, and member selection.
    pub(crate) fn view_create_group_dialog<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_LARGE};
        use crate::form_components::{
            FormSection, SelectablePeerList, SelectablePeerRow, TextInput, remove_chip,
        };
        use iced::widget::Row;

        let theme = Self::theme_from_dark(self.dark_mode);

        // ── Available peers: friends who can be messaged, sorted by label ─
        let mut available = self.messageable_friends();
        available.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));

        // Search/filter over display label and short peer id.
        let query = self.create_group_search.trim().to_lowercase();
        let filtered: Vec<&(PublicKey, String)> = if query.is_empty() {
            available.iter().collect()
        } else {
            available
                .iter()
                .filter(|(pk, label)| {
                    label.to_lowercase().contains(&query)
                        || pk.fmt_short().to_string().to_lowercase().contains(&query)
                })
                .collect()
        };

        // Selected participants shown as removable chips above the list.
        let selected_count = self.create_group_selected_members.len();
        let label_of = |peer: &PublicKey| -> String {
            self.friends
                .iter()
                .find(|(fid, _)| fid.parse_public_key().map(|pk| &pk == peer).unwrap_or(false))
                .map(|(fid, record)| record.display_label(fid, peer))
                .unwrap_or_else(|| peer.fmt_short().to_string())
        };
        let mut chips = Row::new().spacing(crate::design_tokens::SPACE_4);
        for peer in &self.create_group_selected_members {
            chips = chips.push(remove_chip(
                label_of(peer),
                Some(AppMessage::CreateGroupMemberToggled(*peer)),
            ));
        }

        // Peer rows: avatar + display name + peer id / online status.
        let mut rows: Vec<iced::Element<'a, AppMessage>> = Vec::new();
        for (peer, label) in filtered {
            let presence = self.peer_presence(peer);
            let online = presence != PeerPresence::Offline;

            let mut avatar = Avatar::new(label.clone())
                .size(crate::design_tokens::AVATAR_SM)
                .dark_mode(self.dark_mode)
                .online_dot(online);
            if let Some(handle) = self.friend_image_handles.get(peer).and_then(|h| h.clone()) {
                avatar = avatar.image(handle);
            }

            rows.push(
                SelectablePeerRow::new(label.clone())
                    .secondary(format!("{} · {}", peer.fmt_short(), presence.label()))
                    .avatar(avatar.build())
                    .selected(self.create_group_selected_members.contains(peer))
                    .on_toggle(AppMessage::CreateGroupMemberToggled(*peer))
                    .build(&theme),
            );
        }

        let empty_text: String = if available.is_empty() {
            crate::i18n::t("dialogs.create_group.no_peers_available")
        } else {
            crate::i18n::t("dialogs.create_group.no_peers_match")
        };

        // Participants picker: search + chips + peer list + summary.
        let mut picker = SelectablePeerList::new(rows, 240.0, Some(empty_text));
        if !available.is_empty() {
            picker = picker.search(
                "Search participants…",
                &self.create_group_search,
                AppMessage::CreateGroupSearchChanged,
            );
        }
        if selected_count > 0 {
            picker = picker.chips(vec![chips.into()]);
        }
        picker = picker.summary(selected_count, "participant");
        let participants = FormSection::new("Participants").push(picker.build());

        let mut group_name_field = TextInput::new(
            "Group Name",
            "Group name…",
            &self.create_group_name,
            AppMessage::CreateGroupNameChanged,
        )
        .id(CREATE_GROUP_NAME_INPUT);
        if let Some(error) = &self.create_group_error {
            group_name_field = group_name_field.error(error.clone());
        }
        let group_name_valid = !self.create_group_name.trim().is_empty();
        let group_submitting = self.create_group_submitting;
        if group_name_valid && !group_submitting {
            group_name_field =
                group_name_field.on_submit(AppMessage::ConfirmCreateGroup);
        }
        let description_field = TextInput::new(
            "Description",
            "Description (optional)…",
            &self.create_group_description,
            AppMessage::CreateGroupDescriptionChanged,
        )
        .build();

        let overlay = BoruDialog::new("Create Group Chat")
            .subtitle("Start a private conversation with multiple selected peers.")
            .width(self.dialog_width(BORU_DIALOG_WIDTH_LARGE))
            .push_body(
                FormSection::new("Group Details")
                    .push(group_name_field.build())
                    .push(description_field)
                    .build(),
            )
            .push_body(participants.build())
            .secondary("Cancel", AppMessage::HideCreateGroupDialog)
            .secondary_enabled(!group_submitting)
            .primary(
                if group_submitting { "Creating…" } else { "Create Group" },
                AppMessage::ConfirmCreateGroup,
            )
            .primary_enabled(group_name_valid && !group_submitting)
            .on_close(AppMessage::HideCreateGroupDialog)
            .on_backdrop(AppMessage::HideCreateGroupDialog)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    /// Dialog for receiving a file shared outside the friend graph: paste a
    /// BlobTicket, run a pre-flight check (size + format), then download
    /// through the existing download machinery into a safe destination.
    pub(crate) fn view_receive_ticket_dialog<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::{FormSection, TextInput};

        let theme = Self::theme_from_dark(self.dark_mode);

        let mut ticket_field = TextInput::new(
            "Share ticket",
            "Paste a share ticket (starts with blob:…)",
            &self.receive_ticket_input,
            AppMessage::ReceiveTicketInputChanged,
        )
        .id("receive-ticket-input")
        .helper("Anyone with this ticket can receive the file — no friend relationship required.");
        if let Some(error) = &self.receive_ticket_error {
            ticket_field = ticket_field.error(error.clone());
        }

        let ticket_section = FormSection::new("Ticket")
            .push(ticket_field.build())
            .build();

        // Pre-flight result summary.
        let preflight_section: Option<iced::Element<'a, AppMessage>> =
            self.receive_ticket_preflight.as_ref().map(|pf| {
                let kind_label = if pf.is_collection {
                    format!("Folder · {} children", pf.child_count)
                } else {
                    "Single file".to_string()
                };
                let size = crate::dashboard_view_model::format_bytes(pf.total_size);
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::BodyEmphasised,
                    format!("{kind_label} · {size} · from {}", pf.node_short),
                )
                .into()
            });

        let mut overlay = BoruDialog::new("Receive from Ticket")
            .subtitle("Paste a share ticket to download a file shared outside the friend graph.")
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(ticket_section);
        if let Some(section) = preflight_section {
            overlay = overlay.push_body(section);
        }
        let overlay = overlay
            .secondary("Cancel", AppMessage::CloseReceiveTicketDialog)
            .secondary_enabled(!self.receive_ticket_preflight_busy)
            .primary(
                if self.receive_ticket_preflight.is_none() {
                    "Inspect Ticket"
                } else {
                    "Download"
                },
                if self.receive_ticket_preflight.is_none() {
                    AppMessage::ReceiveTicketPreflight
                } else {
                    AppMessage::ConfirmReceiveTicket
                },
            )
            .primary_enabled(
                !self.receive_ticket_preflight_busy
                    && !self.receive_ticket_downloading
                    && !self.receive_ticket_input.trim().is_empty(),
            )
            .on_close(AppMessage::CloseReceiveTicketDialog)
            .on_backdrop(AppMessage::CloseReceiveTicketDialog)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    /// Dialog for sharing a file via a short code (FS-26). The minted code is
    /// shown with a copy action; the rendezvous topic stays subscribed while
    /// the dialog is open so receivers that join late still receive the
    /// announcement.
    pub(crate) fn view_short_code_dialog<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::FormSection;

        let theme = Self::theme_from_dark(self.dark_mode);

        let mut overlay = BoruDialog::new("Share via Short Code")
            .subtitle(
                "Anyone who types this code on a device on the same relay can \
                 download the file — no friend relationship required.",
            )
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD));

        if let Some(code) = &self.short_code_dialog_code {
            let code_text = crate::fonts::type_role_text(
                crate::fonts::TypeRole::DisplayHeading,
                format!("  {code}  "),
            );
            let copy = iced::widget::button("Copy")
                .on_press(AppMessage::CopyShortCode(code.clone()))
                .padding([
                    crate::theme::BoruTheme::default().dialogs.control_padding_y,
                    crate::theme::BoruTheme::default().dialogs.control_padding_x,
                ]);
            let code_row: iced::Element<'_, AppMessage> = iced::widget::row![code_text, copy]
                .spacing(crate::theme::BoruTheme::default().dialogs.control_spacing)
                .align_y(iced::Alignment::Center)
                .into();
            overlay = overlay.push_body(FormSection::new("Code").push(code_row).build());
        } else if self.short_code_minting {
            let minting: iced::Element<'_, AppMessage> = crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "Minting…",
            )
            .into();
            overlay = overlay.push_body(FormSection::new("Code").push(minting).build());
        }
        if let Some(error) = &self.short_code_dialog_error {
            let err_text: iced::Element<'_, AppMessage> =
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, error.clone()).into();
            overlay = overlay.push_body(err_text);
        }
        let share = self.short_code_active.clone();
        if let Some(share) = &share {
            let file_text: iced::Element<'_, AppMessage> = crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                format!(
                    "{} — {}",
                    share.name,
                    crate::dashboard_view_model::format_bytes(share.size)
                ),
            )
            .into();
            overlay = overlay.push_body(FormSection::new("File").push(file_text).build());
        }

        let overlay = overlay
            .primary("Done", AppMessage::CloseShortCodeDialog)
            .on_close(AppMessage::CloseShortCodeDialog)
            .on_backdrop(AppMessage::CloseShortCodeDialog)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    /// Dialog for redeeming a short code (FS-26). Subscribes to the
    /// code-derived rendezvous topic and waits for a signed announcement from
    /// the sharing peer, then creates the same download card as pasting a
    /// ticket.
    pub(crate) fn view_redeem_code_dialog<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::{FormSection, TextInput};

        let theme = Self::theme_from_dark(self.dark_mode);

        let mut code_field = TextInput::new(
            "Short code",
            "e.g. 7 characters",
            &self.redeem_code_input,
            AppMessage::RedeemCodeInputChanged,
        )
        .id("redeem-code-input")
        .helper("Type the code the sharing peer shows. Both peers must be on the same relay.");
        if let Some(error) = &self.redeem_code_error {
            code_field = code_field.error(error.clone());
        }
        let code_section = FormSection::new("Code")
            .push(code_field.build())
            .build();

        let mut overlay = BoruDialog::new("Receive via Short Code")
            .subtitle(
                "Redeem a short code to download a file shared outside the friend graph.",
            )
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(code_section);
        if self.redeem_code_busy {
            let waiting: iced::Element<'_, AppMessage> = crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "Waiting for the sharing peer…",
            )
            .into();
            overlay = overlay.push_body(waiting);
        }

        let overlay = overlay
            .secondary("Cancel", AppMessage::CloseRedeemCodeDialog)
            .secondary_enabled(!self.redeem_code_busy)
            .primary("Redeem", AppMessage::RedeemShortCode)
            .primary_enabled(!self.redeem_code_busy && !self.redeem_code_input.trim().is_empty())
            .on_close(AppMessage::CloseRedeemCodeDialog)
            .on_backdrop(AppMessage::CloseRedeemCodeDialog)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    /// Dialog for sharing a tunnel with a friend — shows a friend picker
    /// with a per-friend "Share" action.
    pub(crate) fn view_create_tunnel_dialog<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::{
            FormSection, SelectablePeerList, SelectablePeerRow, TextInput,
        };

        let theme = Self::theme_from_dark(self.dark_mode);

        // Build friend selection list — only friends who can accept tunnels.
        let mut rows: Vec<iced::Element<'a, AppMessage>> = Vec::new();
        for (peer, label) in self.messageable_friends() {
            rows.push(
                SelectablePeerRow::new(label)
                    .on_toggle(AppMessage::CreateTunnel(peer))
                    .build(&theme),
            );
        }

        let connection_section = FormSection::new(crate::i18n::t("dialogs.create_tunnel.connection_target"))
            .helper(crate::i18n::t("dialogs.create_tunnel.connection_target_helper"))
            .push(SelectablePeerList::new(
                rows,
                250.0,
                Some(crate::i18n::t("dialogs.create_tunnel.no_friends_available")),
            )
            .build())
            .build();

        // Tunnel port — the loopback port the tunnel will listen on at the
        // receiving side. Empty means an automatic (ephemeral) port; a
        // chosen port is carried through the TunnelOffer so the receiver's
        // listener binds it when available.
        let mut port_field = TextInput::new(
            "Tunnel port",
            "Automatic",
            &self.create_tunnel_port,
            AppMessage::CreateTunnelPortChanged,
        )
        .helper(
            "Port the tunnel will listen on (1-65535). Leave empty for an automatic port.",
        );
        if let Some(error) = &self.create_tunnel_port_error {
            port_field = port_field.error(error.clone());
        }
        let port_section = FormSection::new("Tunnel Port")
            .push(port_field.build())
            .build();

        let overlay = BoruDialog::new("Create Tunnel")
            .subtitle("Securely route traffic between peers.")
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(connection_section)
            .push_body(port_section)
            .secondary("Cancel", AppMessage::CancelCreateTunnel)
            .on_close(AppMessage::CancelCreateTunnel)
            .on_backdrop(AppMessage::CancelCreateTunnel)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    /// Dialog for inviting members to the current group — a friend picker
    /// built on the shared BoruDialog + peer-list components.
    pub(crate) fn view_invite_member_dialog<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::BoruDialog;
        use crate::form_components::{FormSection, SelectablePeerList, SelectablePeerRow};

        let theme = Self::theme_from_dark(self.dark_mode);

        // Build friend selection list — only friends who can be messaged.
        let mut rows: Vec<iced::Element<'a, AppMessage>> = Vec::new();
        for (peer, label) in self.messageable_friends() {
            let is_selected = self.invite_member_selected.contains(&peer);
            rows.push(
                SelectablePeerRow::new(label)
                    .selected(is_selected)
                    .on_toggle(AppMessage::InviteMemberToggled(peer))
                    .build(&theme),
            );
        }

        let body = FormSection::new(crate::i18n::t("dialogs.invite_member.participants"))
            .push(SelectablePeerList::new(
                rows,
                250.0,
                Some(crate::i18n::t("dialogs.invite_member.no_friends_available")),
            )
            .build())
            .build();

        let overlay = BoruDialog::new("Invite to Group")
            .subtitle("Select friends to invite:")
            .push_body(body)
            .secondary("Cancel", AppMessage::HideInviteMemberDialog)
            .primary("Send Invite", AppMessage::ConfirmInviteMember)
            .scroll_body(self.dialog_body_max_height())
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }
}
