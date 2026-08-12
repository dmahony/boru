//! Sidebar feature.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the left sidebar: Chats /
//! Groups / Friends / Discover / Public Rooms / Requests sections, their
//! Hash-compatible dependency snapshots and the `impl IcedChat` methods that
//! build and render them. Reads app state via `use super::*`; app.rs
//! re-exports the pub(crate) items it still references with `use sidebar::*`.

use super::*;

/// Cached dependency for the sidebar's Chats section.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarChatsRow {
    pub(crate) topic: TopicId,
    pub(crate) name: String,
    pub(crate) preview: String,
    pub(crate) preview_sender: String,
    pub(crate) unread: u64,
    pub(crate) last_seen_at_unix_ms: u64,
    pub(crate) online: bool,
    pub(crate) avatar: SidebarAvatarHandle,
    pub(crate) profile_version: u64,
    pub(crate) is_group: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarAvatarHandle {
    pub(crate) handle: Option<iced::widget::image::Handle>,
    pub(crate) key: Option<u64>,
}

impl std::hash::Hash for SidebarAvatarHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarChatsDependency {
    pub(crate) dark_mode: bool,
    pub(crate) conversations: Vec<SidebarChatsRow>,
    pub(crate) is_empty: bool,
    pub(crate) room_delete_confirm_topic: Option<TopicId>,
}

/// Cached dependency for the sidebar's Discovered Peers section.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarDiscoveredPeerRow {
    pub(crate) peer: PublicKey,
    pub(crate) display_name: String,
    pub(crate) avatar: SidebarAvatarHandle,
    pub(crate) online: bool,
    pub(crate) is_friend: bool,
    pub(crate) request_state: Option<OutgoingRequestState>,
    pub(crate) profile_version: u64,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarDiscoveredPeersDependency {
    pub(crate) dark_mode: bool,
    pub(crate) peers: Vec<SidebarDiscoveredPeerRow>,
}

/// A single public room advertisement shown in the sidebar.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarPublicRoomRow {
    pub(crate) room_name: String,
    pub(crate) member_count: u32,
    pub(crate) author: PublicKey,
    pub(crate) advertisement: RoomAdvertisement,
}

/// Cached dependency for the sidebar's Public Rooms section.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarPublicRoomsDependency {
    pub(crate) dark_mode: bool,
    pub(crate) local_public: PublicKey,
    pub(crate) rooms: Vec<SidebarPublicRoomRow>,
}

/// Cached dependency for the sidebar's Friends section.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarFriendRow {
    pub(crate) peer: PublicKey,
    pub(crate) label: String,
    pub(crate) avatar: SidebarAvatarHandle,
    pub(crate) presence: PeerPresence,
    pub(crate) profile_version: u64,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarFriendsDependency {
    pub(crate) dark_mode: bool,
    pub(crate) sidebar_revision: u64,
    pub(crate) friend_request_search_input: String,
    pub(crate) friend_request_error: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarFriendsRowsDependency {
    pub(crate) dark_mode: bool,
    pub(crate) sidebar_revision: u64,
    pub(crate) friends: Vec<SidebarFriendRow>,
}

/// Cached dependency for the sidebar's Friend Requests section.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarRequestRow {
    pub(crate) request_id: String,
    pub(crate) requester: PublicKey,
    pub(crate) label: String,
}

/// A pending group-invite row displayed in the REQUESTS sidebar section.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SidebarGroupInviteRow {
    pub(crate) invite_id: Vec<u8>,
    pub(crate) inviter_public_key: Vec<u8>,
    pub(crate) group_name: String,
    pub(crate) ticket: String,
    pub(crate) inviter_label: String,
}


/// A tunnel request row rendered in the REQUESTS sidebar section.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SidebarTunnelRequestRow {
    pub(crate) peer: PublicKey,
    pub(crate) tunnel_id: String,
    pub(crate) label: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SidebarRequestsDependency {
    pub(crate) dark_mode: bool,
    /// Changes whenever the persistent request store changes so iced::lazy
    /// cannot retain a stale list after an incoming request arrives.
    pub(crate) requests_revision: u64,
    pub(crate) incoming: Vec<SidebarRequestRow>,
    pub(crate) friend_request_error: String,
    pub(crate) group_invites: Vec<SidebarGroupInviteRow>,
    pub(crate) tunnel_requests: Vec<SidebarTunnelRequestRow>,
}

impl IcedChat {
    // ── Sidebar ────────────────────────────────────────────────────────

    /// Left sidebar containing Chats, Friends, Discover, and Requests sections.
    ///
    /// Layout: pinned brand row + identity row + divider at top;
    /// scrollable collapsible sections in the middle;
    /// pinned utility row at the bottom.
    pub(crate) fn view_sidebar(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{container, rule, text, Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = self.theme();
        let inset = SIDEBAR_INSET; // 24 px

        // ═══════════════════════════════════════════════════════════════
        // 1. BRAND ROW — Raleway ExtraBold "BORU" + settings
        // ═══════════════════════════════════════════════════════════════
        let mut brand_row = Row::new()
            .push(
                // "BORU" wordmark in Raleway ExtraBold
                text("BORU")
                    .font(crate::fonts::raleway_extra_bold())
                    .size(20.0)
                    .color(crate::design_tokens::text_primary(&theme)),
            )
            .push(Space::new().width(Length::Fill));

        #[cfg(feature = "terminal")]
        {
            brand_row = brand_row.push(ghost_icon_button(
                Icon::Terminal,
                IconSize::Md,
                Some("Terminal"),
                Some(AppMessage::OpenTerminal),
                false,
                false,
                false,
            ));
            brand_row = brand_row.push(Space::new().width(Length::Fixed(SPACE_4)));
        }

        brand_row = brand_row
            .push(ghost_icon_button(
                Icon::Settings,
                IconSize::Md,
                Some("Settings"),
                Some(AppMessage::OpenSettings),
                false,
                false,
                false,
            ))
            .align_y(Alignment::Center);

        // ═══════════════════════════════════════════════════════════════
        // 2. IDENTITY ROW — avatar, name, online status, profile action
        // ═══════════════════════════════════════════════════════════════
        let local_presence = match &self.mesh_health {
            MeshHealth::Good => PeerPresence::Online,
            MeshHealth::Degraded(_) => PeerPresence::Away,
            MeshHealth::Offline(_) => PeerPresence::Offline,
        };
        let identity_key = SidebarIdentityCacheKey {
            local_label: self.local_label.clone(),
            presence: local_presence,
            dark_mode: self.dark_mode,
            has_profile_image: self.profile_image_handle.is_some(),
        };
        let identity_label = self.local_label.clone();
        let identity_presence = local_presence;
        let identity_dark = self.dark_mode;
        let identity_pk = self.local_public;
        let identity_profile_image = self.profile_image_handle.clone();
        let identity_row: iced::Element<'static, AppMessage> =
            iced::widget::lazy(identity_key, move |_| {
                view_local_profile_block(
                    identity_label.clone(),
                    identity_presence,
                    identity_dark,
                    identity_pk,
                    identity_profile_image.clone(),
                )
            })
            .into();

        // ═══════════════════════════════════════════════════════════════
        // 3. DIVIDER beneath identity
        // ═══════════════════════════════════════════════════════════════
        let identity_divider = rule::horizontal(1).style(move |t| rule::Style {
            color: crate::design_tokens::border_muted(t),
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: false,
        });

        // ═══════════════════════════════════════════════════════════════
        // 4. SCROLLABLE SECTIONS (chats, groups, friends, discover, etc.)
        // ═══════════════════════════════════════════════════════════════
        let chat_count = self.cached_chat_count;
        let group_count = self.cached_group_count;
        let friend_count = self.cached_friend_count;
        let discover_count = self.cached_discover_count;
        let public_room_count = self.cached_public_room_count;
        let request_count = self.cached_request_count;

        let mut sections = Column::new()
            .padding(iced::Padding {
                top: SPACE_4,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            })
            .spacing(0);

        // SIDEBAR-01: a section renders collapsed while it is empty (or the
        // user collapsed it). Empty sections can never be expanded manually,
        // so `effective` = manual flag OR no items.
        let chats_collapsed = self.sidebar_section_collapsed[0] || chat_count == 0;
        let groups_collapsed = self.sidebar_section_collapsed[1] || group_count == 0;
        let friends_collapsed = self.sidebar_section_collapsed[2] || friend_count == 0;
        let discover_collapsed = self.sidebar_section_collapsed[3] || discover_count == 0;
        let public_rooms_collapsed = self.sidebar_section_collapsed[5] || public_room_count == 0;
        let requests_collapsed = self.sidebar_section_collapsed[4] || request_count == 0;

        // CHATS section
        sections = sections.push(
            SidebarSectionHeader::new("CHATS")
                .count(chat_count)
                .collapsed(chats_collapsed)
                .on_toggle(AppMessage::ToggleSidebarSectionCollapsed(0))
                .add_action(Icon::Plus, AppMessage::CreateNewRoom)
                .build(&theme),
        );
        if !chats_collapsed {
            sections = sections.push(section_fade(
                self.sidebar_fade_frame[0],
                self.view_sidebar_chats(),
            ));
        }

        // GROUPS section
        sections = sections.push(
            SidebarSectionHeader::new("GROUPS")
                .count(group_count)
                .collapsed(groups_collapsed)
                .on_toggle(AppMessage::ToggleSidebarSectionCollapsed(1))
                .add_action(Icon::Users, AppMessage::OpenGroups)
                .build(&theme),
        );
        if !groups_collapsed {
            sections = sections.push(section_fade(
                self.sidebar_fade_frame[1],
                self.view_sidebar_groups(),
            ));
        }

        // FRIENDS section
        sections = sections.push(
            SidebarSectionHeader::new("FRIENDS")
                .count(friend_count)
                .collapsed(friends_collapsed)
                .on_toggle(AppMessage::ToggleSidebarSectionCollapsed(2))
                .add_action(Icon::UserPlus, AppMessage::OpenFriendRequests)
                .build(&theme),
        );
        if !friends_collapsed {
            sections = sections.push(section_fade(
                self.sidebar_fade_frame[2],
                self.view_sidebar_friends(),
            ));
        }

        // DISCOVER section
        sections = sections.push(
            SidebarSectionHeader::new("DISCOVER")
                .count(discover_count)
                .collapsed(discover_collapsed)
                .on_toggle(AppMessage::ToggleSidebarSectionCollapsed(3))
                .build(&theme),
        );
        if !discover_collapsed {
            sections = sections.push(section_fade(
                self.sidebar_fade_frame[3],
                self.view_sidebar_discovered_peers(),
            ));
        }

        // PUBLIC ROOMS section
        sections = sections.push(
            SidebarSectionHeader::new("PUBLIC ROOMS")
                .count(public_room_count)
                .collapsed(public_rooms_collapsed)
                .on_toggle(AppMessage::ToggleSidebarSectionCollapsed(5))
                .add_action(Icon::Plus, AppMessage::CreateNewRoom)
                .build(&theme),
        );
        if !public_rooms_collapsed {
            sections = sections.push(section_fade(
                self.sidebar_fade_frame[5],
                self.view_sidebar_public_rooms(),
            ));
        }

        // REQUESTS section
        sections = sections.push(
            SidebarSectionHeader::new("REQUESTS")
                .count(request_count)
                .collapsed(requests_collapsed)
                .on_toggle(AppMessage::ToggleSidebarSectionCollapsed(4))
                .build(&theme),
        );
        if !requests_collapsed {
            sections = sections.push(section_fade(
                self.sidebar_fade_frame[4],
                self.view_sidebar_requests(),
            ));
        }

        let sections_scroll = crate::ui_components::gutter_scrollable(sections)
            .width(Length::Fill)
            .height(Length::Fill);

        // ═══════════════════════════════════════════════════════════════
        // 5. BOTTOM UTILITY ROW — new chat, search, mesh, notifications
        // POLISH-05 / BORU-HOME-09: inactive toolbar icons are dimmed
        // (text_muted) so the selected Home icon stays visually stronger.
        // Section header chrome recedes behind chat/friend content.
        // ═══════════════════════════════════════════════════════════════
        let utility_row = Row::new()
            .push(ghost_icon_button(
                Icon::Home,
                IconSize::Md,
                Some("Home"),
                Some(AppMessage::GoToChatList),
                false,
                false,
                false,
            ))
            .push(Space::new().width(Length::Fixed(SPACE_4)))
            .push(ghost_icon_button(
                Icon::Plus,
                IconSize::Md,
                Some("New chat"),
                Some(AppMessage::CreateNewRoom),
                false,
                false,
                true,
            ))
            .push(Space::new().width(Length::Fixed(SPACE_4)))
            .push(ghost_icon_button(
                Icon::Search,
                IconSize::Md,
                Some("Search"),
                Some(AppMessage::Noop),
                false,
                false,
                true,
            ))
            .push(Space::new().width(Length::Fill))
            .push(ghost_icon_button(
                Icon::Folder,
                IconSize::Md,
                Some("File Sharing"),
                Some(AppMessage::OpenFileSharing),
                false,
                false,
                true,
            ))
            .push(Space::new().width(Length::Fixed(SPACE_4)))
            .push(ghost_icon_button(
                Icon::Mesh,
                IconSize::Md,
                Some("Network"),
                Some(AppMessage::OpenConnectionDetails),
                false,
                false,
                true,
            ))
            .push(Space::new().width(Length::Fixed(SPACE_4)))
            .push(ghost_icon_button(
                Icon::Notification,
                IconSize::Md,
                Some("Notifications"),
                Some(AppMessage::OpenSettings),
                false,
                false,
                true,
            ))
            .align_y(Alignment::Center);

        // ═══════════════════════════════════════════════════════════════
        // ASSEMBLE — pinned top, scrollable middle, pinned bottom
        // ═══════════════════════════════════════════════════════════════
        Column::new()
            // Pinned: brand row
            .push(container(brand_row).padding(iced::Padding {
                top: SPACE_16,
                right: inset,
                bottom: SPACE_8,
                left: inset,
            }))
            // Pinned: identity row
            .push(container(identity_row).padding(iced::Padding {
                top: SPACE_4,
                right: inset,
                bottom: SPACE_8,
                left: inset,
            }))
            // Pinned: subtle divider
            .push(container(identity_divider).padding(iced::Padding {
                top: 0.0,
                right: inset,
                bottom: 0.0,
                left: inset,
            }))
            // Scrollable: all sections
            .push(sections_scroll)
            // Pinned: bottom utility row with top divider
            .push(
                container(
                    Column::new()
                        .push(rule::horizontal(1).style(move |t| rule::Style {
                            color: crate::design_tokens::border_muted(t),
                            radius: 0.0.into(),
                            fill_mode: rule::FillMode::Full,
                            snap: false,
                        }))
                        .push(container(utility_row).padding(iced::Padding {
                            top: SPACE_8,
                            right: inset,
                            bottom: SPACE_12,
                            left: inset,
                        })),
                )
                .padding(iced::Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                }),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn sidebar_chats_dependency(&self) -> SidebarChatsDependency {
        // Return cached dependency if revision hasn't changed.
        let cur_revision = self.chats_sidebar_revision;
        if self.cached_chats_revision.get() == cur_revision {
            if let Some(ref dep) = *self.cached_chats_dep.borrow() {
                return dep.clone();
            }
        }

        let mut conversations: Vec<SidebarChatsRow> = self
            .conversation_store
            .active_iter()
            .into_iter()
            .filter(|entry| {
                !matches!(
                    entry.kind,
                    boru_core::conversations::ConversationKind::Group
                )
            })
            .map(|entry| {
                let peer_pk = if entry.peer_id.is_empty() {
                    None
                } else {
                    PublicKey::from_str(&entry.peer_id).ok()
                };
                let is_group = matches!(
                    entry.kind,
                    boru_core::conversations::ConversationKind::Group
                );
                let online = peer_pk
                    .map(|pk| self.peer_presence(&pk) != PeerPresence::Offline)
                    .unwrap_or(false);
                let avatar = peer_pk.and_then(|pk| {
                    self.friend_image_handles
                        .get(&pk)
                        .and_then(|avatar| avatar.as_ref())
                });
                let profile_version = peer_pk
                    .and_then(|pk| self.friend_profile_versions.get(&pk).copied())
                    .unwrap_or(0);
                let room_entry = self.room_history.find(&entry.topic);
                SidebarChatsRow {
                    topic: entry.topic,
                    name: entry.display_name().to_string(),
                    preview: room_entry
                        .and_then(|r| {
                            if r.last_preview.is_empty() {
                                None
                            } else {
                                Some(r.last_preview.clone())
                            }
                        })
                        .unwrap_or_default(),
                    preview_sender: room_entry
                        .map(|r| r.last_sender_name.clone())
                        .unwrap_or_default(),
                    unread: self
                        .conversations
                        .get(&entry.topic)
                        .map(|c| c.unread)
                        .unwrap_or(0),
                    last_seen_at_unix_ms: entry.last_seen_at_unix_ms,
                    online,
                    avatar: Self::sidebar_avatar_handle(avatar),
                    profile_version,
                    is_group,
                }
            })
            .collect();

        // Sort: online + has messages / online + recent → recent → name
        conversations.sort_by(|a, b| {
            let a_recent = a.last_seen_at_unix_ms;
            let b_recent = b.last_seen_at_unix_ms;
            match (a.online, b.online) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    // Both online or both offline: sort by recency, newest first
                    b_recent.cmp(&a_recent).then_with(|| a.name.cmp(&b.name))
                }
            }
        });

        let dep = SidebarChatsDependency {
            dark_mode: self.dark_mode,
            conversations,
            is_empty: self.conversation_store.is_empty(),
            room_delete_confirm_topic: self.room_delete_confirm_topic,
        };
        self.cached_chats_revision.set(cur_revision);
        *self.cached_chats_dep.borrow_mut() = Some(dep.clone());
        dep
    }

    /// "Chats" section of the sidebar — public room pinned at top, then
    /// conversations from the conversation store sorted by most-recent activity.
    pub(crate) fn view_sidebar_chats(&self) -> iced::Element<'_, AppMessage> {
        let selected_topic = self.sidebar_selected_topic.clone();
        selected_topic.set(match self.screen {
            Screen::Chat { topic } => Some(topic),
            _ => None,
        });
        iced::widget::lazy(self.sidebar_chats_dependency(), move |dep| {
            Self::view_sidebar_chats_content(dep, selected_topic.clone())
        })
        .into()
    }

    /// Render a standard empty-state block with optional ghost action button.
    /// The caller supplies context-specific padding (sidebar or main panel).
    fn empty_state_block<'a>(
        theme: &iced::Theme,
        message: &'a str,
        action: Option<(&'a str, AppMessage)>,
        padding: [f32; 2],
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{button, container, Column};
        use iced::Length;
        let mut col = Column::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, message)
                    .color(text_muted(theme)),
            )
            .spacing(SPACE_6)
            .width(Length::Fill);
        if let Some((label, msg)) = action {
            col = col.push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    label,
                ))
                .on_press(msg)
                .padding([SPACE_4, SPACE_8])
                .style(BUTTON_GHOST),
            );
        }
        container(col).width(Length::Fill).padding(padding).into()
    }

    /// Old compact sidebar empty-state block (pre-UI-06).  Superseded by the
    /// shared `ui_components::sidebar_empty_state`; kept only until UI-22
    /// cleanup so reviewers can compare the old styling.
    #[expect(dead_code)]
    fn sidebar_empty_state_block<'a>(
        theme: &iced::Theme,
        icon: &'static [u8],
        title: &'a str,
        supporting: &'a str,
        action: Option<(&'a str, AppMessage)>,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{button, container, Column, Row};
        use iced::{Alignment, Length};

        let mut copy = Column::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, title)
                    .color(text_system(theme)),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, supporting)
                    .color(text_muted(theme)),
            )
            .spacing(SPACE_2)
            .width(Length::Fill);
        if let Some((label, message)) = action {
            copy = copy.push(
                button(crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, label))
                    .on_press(message)
                    .padding([SPACE_4, SPACE_8])
                    .style(BUTTON_GHOST_BG),
            );
        }

        container(
            Row::new()
                .push(
                    icon_svg(icon, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                        color: Some(text_muted(t)),
                    }),
                )
                .push(copy)
                .spacing(SPACE_8)
                .align_y(Alignment::Start),
        )
        .width(Length::Fill)
        .padding([SPACE_8, SPACE_12])
        .into()
    }

    pub(crate) fn view_sidebar_chats_content(
        dep: &SidebarChatsDependency,
        selected_topic: Rc<Cell<Option<TopicId>>>,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::Column;

        let mut section = Column::new().spacing(SPACE_2);

        let delete_confirm = dep.room_delete_confirm_topic;
        for row in &dep.conversations {
            section = section.push(Self::view_sidebar_conversation_row(
                dep.dark_mode,
                row.topic,
                row.name.clone(),
                row.preview.clone(),
                row.preview_sender.clone(),
                row.unread,
                selected_topic.clone(),
                row.last_seen_at_unix_ms,
                row.online,
                row.avatar.clone(),
                delete_confirm,
                row.is_group,
            ));
        }

        if dep.is_empty {
            section = section.push(sidebar_empty_state(
                Icon::Chat,
                "No conversations yet",
                "Start a chat with one of your friends.",
                Some(("Start Chat", AppMessage::CreateNewRoom)),
            ));
        }

        section.into()
    }

    /// Build the Groups screen's dependency snapshot (theme + group list).
    /// Shared by the live lazy view and the pre-warm cache so both hash the
    /// same state.
    pub(crate) fn groups_dependency(&self) -> GroupsDependency {
        let dark_mode = self.dark_mode;

        // Collect group data into owned tuples so we can return an Element
        // without borrowing local state.
        let group_data: Vec<(TopicId, String)> = self
            .conversation_store
            .active_iter()
            .into_iter()
            .filter(|e| matches!(e.kind, ConversationKind::Group))
            .map(|e| (e.topic, e.display_name().to_string()))
            .collect();

        GroupsDependency {
            dark_mode,
            groups: group_data,
        }
    }

    pub(crate) fn view_sidebar_groups(&self) -> iced::Element<'_, AppMessage> {
        // Groups screen + sidebar section are cached with `lazy` so switching
        // away and back to the Groups screen reuses the built widget tree
        // (zero diff / layout / render) unless the group list actually changed.
        let dep = self.groups_dependency();
        iced::widget::lazy(dep, Self::view_groups_section_content).into()
    }

    /// Full-screen Groups view: a header with a back button (returning to the
    /// previous screen, e.g. the File Sharing dashboard) above the shared
    /// groups section content.
    pub(crate) fn view_groups_screen(&self) -> iced::Element<'_, AppMessage> {
        let dep = self.groups_dependency();
        iced::widget::lazy(dep, Self::view_groups_screen_content).into()
    }

    /// Static renderer for the full-screen Groups view, driven by the
    /// [`GroupsDependency`] snapshot so `iced::widget::lazy` can cache it.
    pub(crate) fn view_groups_screen_content(dep: &GroupsDependency) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, Column, Row};
        use iced::{Alignment, Length};

        let header = Row::new()
            .push(
                // FILES-04: explicit back button returning to the previous
                // screen (File Sharing dashboard when opened from there).
                button(
                    Row::new()
                        .push(Icon::Back.build().size(IconSize::Sm).build())
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::ButtonLabel,
                                "Back",
                            ),
                        )
                        .spacing(SPACE_4)
                        .align_y(Alignment::Center),
                )
                .on_press(AppMessage::CloseGroups)
                .padding([SPACE_4, SPACE_8])
                .style(BUTTON_GHOST_BG),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::SectionTitle, "Groups")
                    .width(Length::Fill),
            )
            .align_y(Alignment::Center)
            .spacing(SPACE_12);

        Column::new()
            .push(header)
            .push(Self::view_groups_section_content(dep))
            .padding(SPACE_16)
            .spacing(SPACE_12)
            .into()
    }

    /// Static renderer for the Groups section/screen, driven by the
    /// [`GroupsDependency`] snapshot so `iced::widget::lazy` can cache it.
    pub(crate) fn view_groups_section_content(dep: &GroupsDependency) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let dark_mode = dep.dark_mode;
        let theme = Self::theme_from_dark(dark_mode);
        let group_data = &dep.groups;

        let mut section = Column::new().spacing(SPACE_2);

        // Compact action row: Create Group (secondary style)
        let create_btn = button(
            Row::new()
                .push(
                    Icon::Plus
                        .build()
                        .size(IconSize::Sm)
                        .interactive(true)
                        .build(),
                )
                .push(
                    Space::new()
                        .width(Length::Fixed(SPACE_6))
                        .height(Length::Shrink),
                )
                .push(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Create Group",
                ))
                .align_y(Alignment::Center),
        )
        .on_press(AppMessage::ShowCreateGroupDialog)
        .width(Length::Fill)
        .padding([SPACE_6, SPACE_12])
        .style(crate::ui_components::button_secondary_style);
        section = section.push(create_btn);

        // Group list — build all rows in a separate vec, then extend
        let mut rows: Vec<iced::Element<'_, AppMessage>> = Vec::new();
        for (topic, name) in group_data {
            // Name line: clip long group names and show the full name in a tooltip.
            // `Wrapping::None` keeps the row on a single line — without it the
            // text wraps inside the clip container and the row grows taller
            // instead of truncating (UI-18 long-value stress finding).
            let name_label = container(
                // FONTS-06: group name in IBM Plex Sans Medium.
                sidebar_name_text(name.clone())
                    .wrapping(iced::widget::text::Wrapping::None)
                    .color(crate::design_tokens::text_primary(&theme))
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .clip(true);
            let name_element: iced::Element<'_, AppMessage> = if name.chars().count() > 24 {
                iced::widget::tooltip::Tooltip::new(
                    name_label,
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        name.clone(),
                    )
                    .color(crate::design_tokens::text_primary(&theme)),
                    iced::widget::tooltip::Position::Right,
                )
                .into()
            } else {
                name_label.into()
            };

            let row_btn = button(
                Row::new()
                    .push(
                        Avatar::new(name.clone())
                            .size(crate::design_tokens::AVATAR_CHAT_LIST)
                            .dark_mode(dark_mode)
                            .build(),
                    )
                    .push(name_element)
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center),
            )
            .on_press(AppMessage::OpenGroupChat(*topic))
            .width(Length::Fill)
            .padding([SPACE_6, SPACE_12])
            .style(move |t, status| iced::widget::button::Style {
                background: matches!(status, iced::widget::button::Status::Hovered)
                    .then(|| iced::Background::Color(crate::design_tokens::surface_hover(t))),
                border: iced::Border {
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                text_color: crate::design_tokens::text_primary(t),
                ..Default::default()
            });
            rows.push(row_btn.into());
        }
        for row in rows {
            section = section.push(row);
        }

        if group_data.is_empty() {
            section = section.push(sidebar_empty_state(
                Icon::Chat,
                "No groups yet",
                "Create a group to chat with multiple friends.",
                None::<(&str, AppMessage)>,
            ));
        }

        section.into()
    }

    pub(crate) fn view_sidebar_ticket_join(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, row, text_input, Column};
        use iced::{Alignment, Length};

        let mut section = Column::new().spacing(SPACE_2);

        section = section.push(
            container(
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Join by ticket")
                    .style(text_muted_style),
            )
                .padding(iced::Padding {
                    top: SPACE_8,
                    right: SPACE_12,
                    bottom: SPACE_4,
                    left: SPACE_12,
                })
                .width(Length::Fill),
        );

        section = section.push(
            container(
                row![
                    text_input("Enter ticket ID", &self.join_ticket_input)
                        .on_input(AppMessage::JoinTicketInputChanged)
                        .on_submit(AppMessage::JoinFromTicket)
                        .size(crate::fonts::TypeRole::Body.size_px())
                        .font(crate::fonts::TypeRole::Body.font())
                        .padding([SPACE_4, SPACE_8])
                        .width(Length::Fill),
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Join",
                    ))
                    .on_press(AppMessage::JoinFromTicket)
                    .padding([SPACE_4, SPACE_8]),
                ]
                .spacing(SPACE_6)
                .align_y(Alignment::Center),
            )
            .padding(iced::Padding {
                top: SPACE_2,
                right: SPACE_12,
                bottom: SPACE_2,
                left: SPACE_12,
            })
            .width(Length::Fill),
        );

        if !self.chat_list_error.is_empty() {
            section = section.push(
                container(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        &self.chat_list_error,
                    )
                    .color(Color::from_rgb(0.8, 0.2, 0.2)),
                )
                .padding(iced::Padding {
                    top: 0.0,
                    right: SPACE_12,
                    bottom: SPACE_2,
                    left: SPACE_12,
                })
                .width(Length::Fill),
            );
        }

        section.into()
    }

    #[expect(clippy::too_many_arguments)]
    pub(crate) fn view_sidebar_conversation_row(
        dark_mode: bool,
        topic: TopicId,
        name: String,
        preview: String,
        preview_sender: String,
        unread: u64,
        selected_topic: Rc<Cell<Option<TopicId>>>,
        last_seen_at_unix_ms: u64,
        online: bool,
        avatar: SidebarAvatarHandle,
        delete_confirm_topic: Option<TopicId>,
        is_group: bool,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row};
        use iced::{Alignment, Background, Border, Length};

        // ── Avatar (shared Avatar component, 36px) ─────────────────
        let avatar_element: iced::Element<'static, AppMessage> = {
            let sidebar_avatar = avatar; // SidebarAvatarHandle
            let mut avatar = Avatar::new(name.clone())
                .size(crate::design_tokens::AVATAR_CHAT_LIST)
                .dark_mode(dark_mode);
            if !is_group {
                if let Some(handle) = sidebar_avatar.handle.clone() {
                    avatar = avatar.image(handle);
                }
                avatar = avatar.online_dot(online);
            }
            avatar.build()
        };

        // ── Timestamp (relative) ──────────────────────────────────
        let time_label_str = if last_seen_at_unix_ms > 0 {
            format_last_seen(Some(last_seen_at_unix_ms))
        } else {
            String::new()
        };

        // ── Preview text (single line, truncated) ──────────────────
        let preview_text = if preview.is_empty() {
            String::new()
        } else if is_group && !preview_sender.is_empty() {
            format!("{}: {}", preview_sender, format_preview(&preview))
        } else {
            format_preview(&preview)
        };

        // ── Name color and preview color: brighter/bolder if selected or unread ──
        let name_color_value = selected_topic.clone();
        let name_color = move |theme: &iced::Theme| -> Color {
            let is_selected = name_color_value.get() == Some(topic);
            if is_selected || unread > 0 {
                crate::design_tokens::text_primary(theme)
            } else {
                crate::design_tokens::text_secondary(theme)
            }
        };
        let preview_color_value = selected_topic.clone();
        let preview_color = move |_theme: &iced::Theme| -> Color {
            if preview_color_value.get() == Some(topic) {
                crate::design_tokens::text_secondary(_theme)
            } else {
                crate::design_tokens::text_muted(_theme)
            }
        };
        let time_color_value = selected_topic.clone();
        let time_color = move |_theme: &iced::Theme| -> Color {
            if time_color_value.get() == Some(topic) {
                crate::design_tokens::text_secondary(_theme)
            } else {
                crate::design_tokens::text_muted(_theme)
            }
        };

        // ── Preview row with optional unread badge ─────────────────
        let mut preview_row = Row::new()
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    preview_text.clone(),
                )
                .wrapping(iced::widget::text::Wrapping::None)
                .style(move |t| iced::widget::text::Style {
                    color: Some(preview_color(t)),
                })
                .width(Length::Fill),
            )
            .spacing(SPACE_6)
            .align_y(Alignment::Center);
        if unread > 0 {
            let count_str = if unread > 99 {
                "99+".to_string()
            } else {
                unread.to_string()
            };
            // ── Circular count badge (centered) — ICEDAW-01: iced_aw Badge
            // replacing the hand-rolled 20×20 container. Styled to match the
            // previous circled-unread look exactly: error-red fill, full
            // circle (10px radius at 20×20), white text.
            preview_row = preview_row.push(
                iced_aw::Badge::new(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, count_str)
                        .color(Color::WHITE)
                        .width(Length::Shrink)
                        .height(Length::Shrink),
                )
                .width(20.0)
                .height(20.0)
                .padding(0)
                .style(move |t, _status| iced_aw::style::badge::Style {
                    background: iced::Background::Color(color_error(t)),
                    border_radius: Some(10.0),
                    border_width: 0.0,
                    border_color: None,
                    text_color: Color::WHITE,
                }),
            );
        }

        // ── Build the content row ─────────────────────────────────
        // ── Delete button ────────────────
        let is_deleting = delete_confirm_topic == Some(topic);
        let delete_btn = button(
            container(if is_deleting {
                iced::Element::<'_, AppMessage>::from(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Delete?",
                ))
            } else {
                iced::Element::<'_, AppMessage>::from(icon_svg(ICON_CLOSE, TYPO_XS))
            })
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .width(Length::Fixed(24.0))
            .height(Length::Fixed(24.0)),
        )
        .on_press(if is_deleting {
            AppMessage::ConfirmDeleteRoom(topic)
        } else {
            AppMessage::DeleteRoomRequested(topic)
        })
        .padding(SPACE_2)
        .style(move |t, status| {
            if is_deleting {
                iced::widget::button::Style {
                    background: Some(Background::Color(color_error(t))),
                    text_color: Color::WHITE,
                    border: Border {
                        radius: SPACE_4.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            } else if matches!(status, iced::widget::button::Status::Hovered) {
                iced::widget::button::Style {
                    background: Some(Background::Color(crate::design_tokens::destructive_soft(t))),
                    text_color: color_error(t),
                    border: Border {
                        radius: SPACE_4.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            } else {
                // Invisible by default — appears only on hover (hover/overflow menu pattern)
                iced::widget::button::Style {
                    text_color: Color::from_rgba(0.0, 0.0, 0.0, 0.0),
                    background: None,
                    border: Border {
                        radius: SPACE_4.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }
        });

        // ── Name line: clip long names, show a tooltip with the full name ──
        // `Wrapping::None` keeps the row single-line; without it the text
        // wraps inside the clip container and the row grows (UI-18 finding).
        let name_label = container(
            // FONTS-06: conversation/contact name in IBM Plex Sans Medium.
            sidebar_name_text(name.clone())
                .wrapping(iced::widget::text::Wrapping::None)
                .width(Length::Fill)
                .style(move |t| iced::widget::text::Style {
                    color: Some(name_color(t)),
                }),
        )
        .width(Length::Fill)
        .clip(true);
        let name_element: iced::Element<'static, AppMessage> = if name.chars().count() > 24 {
            iced::widget::tooltip::Tooltip::new(
                name_label,
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    name.clone(),
                )
                .color(crate::design_tokens::text_primary(&Self::theme_from_dark(
                    dark_mode,
                ))),
                iced::widget::tooltip::Position::Right,
            )
            .into()
        } else {
            name_label.into()
        };

        let content_row = Row::new()
            .push(avatar_element)
            .push(
                Column::new()
                    .push(
                        Row::new()
                            .push(name_element)
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Metadata,
                                    time_label_str.clone(),
                                )
                                .style(move |t| iced::widget::text::Style {
                                    color: Some(time_color(t)),
                                }),
                            )
                            .spacing(SPACE_4)
                            .align_y(Alignment::Center),
                    )
                    .push(preview_row)
                    .spacing(SPACE_2)
                    .width(Length::Fill),
            )
            .push(delete_btn)
            .spacing(SPACE_8)
            .padding([SPACE_6, SPACE_12])
            .width(Length::Fill);

        // ── Clickable button wrapper ──────────────────────────────
        let selected_for_btn = selected_topic.clone();
        let btn = button(content_row)
            .on_press(AppMessage::SelectConversation(topic))
            .width(Length::Fill)
            .padding(0)
            .style(move |t, status| {
                let is_selected = selected_for_btn.get() == Some(topic);
                let bg = if is_selected {
                    Some(Background::Color(crate::design_tokens::surface_selected(t)))
                } else if matches!(status, iced::widget::button::Status::Pressed) {
                    Some(Background::Color(crate::design_tokens::surface_pressed(t)))
                } else if matches!(status, iced::widget::button::Status::Hovered) {
                    Some(Background::Color(crate::design_tokens::surface_hover(t)))
                } else {
                    None
                };
                iced::widget::button::Style {
                    background: bg,
                    border: iced::Border {
                        // A thin primary border keeps the selected row visible
                        // for keyboard focus (Tab / arrow navigation).
                        color: if is_selected {
                            crate::design_tokens::primary(t)
                        } else {
                            iced::Color::TRANSPARENT
                        },
                        width: if is_selected { 1.0 } else { 0.0 },
                        radius: crate::design_tokens::RADIUS_MD.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        let selected_for_unread = selected_topic.clone();
        container(btn)
            .width(Length::Fill)
            .style(move |t| {
                let is_selected = selected_for_unread.get() == Some(topic);
                if unread > 0 && !is_selected {
                    let primary = crate::design_tokens::primary(t);
                    container::Style {
                        background: Some(Background::Color(Color::from_rgba(
                            primary.r, primary.g, primary.b, 0.07,
                        ))),
                        border: Border::default(),
                        ..Default::default()
                    }
                } else {
                    container::Style {
                        ..Default::default()
                    }
                }
            })
            .into()
    }

    pub(crate) fn sidebar_discovered_peers_dependency(&self) -> SidebarDiscoveredPeersDependency {
        // Return cached dependency if revision hasn't changed.
        let cur_revision = self.discovered_sidebar_revision;
        if self.cached_discovered_revision.get() == cur_revision {
            if let Some(ref dep) = *self.cached_discovered_dep.borrow() {
                return dep.clone();
            }
        }

        let mut peers: Vec<SidebarDiscoveredPeerRow> = self
            .discovered_peers
            .iter()
            .filter_map(|peer| {
                let fid = boru_core::friends::FriendId::from_public_key(*peer);
                // Skip peers who are already friends — show only non-friend peers.
                let is_friend = self
                    .friends
                    .get(&fid)
                    .map(|r| r.relationship.can_message())
                    .unwrap_or(false);
                if is_friend {
                    return None;
                }
                Some(SidebarDiscoveredPeerRow {
                    peer: *peer,
                    display_name: self.resolve_name(peer),
                    avatar: Self::sidebar_avatar_handle(
                        self.friend_image_handles
                            .get(peer)
                            .and_then(|avatar| avatar.as_ref()),
                    ),
                    online: self.neighbors.contains(peer),
                    is_friend: false,
                    request_state: self.outgoing_request_states.get(peer).cloned(),
                    profile_version: self.friend_profile_versions.get(peer).copied().unwrap_or(0),
                })
            })
            .collect();
        peers.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        let dep = SidebarDiscoveredPeersDependency {
            dark_mode: self.dark_mode,
            peers,
        };
        self.cached_discovered_revision.set(cur_revision);
        *self.cached_discovered_dep.borrow_mut() = Some(dep.clone());
        dep
    }

    /// "Discovered Peers" section of the sidebar - gossip-connected peers.
    pub(crate) fn view_sidebar_discovered_peers(&self) -> iced::Element<'_, AppMessage> {
        iced::widget::lazy(
            self.sidebar_discovered_peers_dependency(),
            Self::view_sidebar_discovered_peers_content,
        )
        .into()
    }

    pub(crate) fn view_sidebar_discovered_peers_content(
        dep: &SidebarDiscoveredPeersDependency,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row};
        use iced::{Alignment, Length};

        let mut section = Column::new().spacing(SPACE_2);

        let theme = Self::theme_from_dark(dep.dark_mode);
        let has_peers = !dep.peers.is_empty();
        for peer in &dep.peers {
            // Avatar with online dot.
            let mut avatar = Avatar::new(peer.display_name.clone())
                .size(crate::design_tokens::AVATAR_CHAT_LIST)
                .dark_mode(dep.dark_mode)
                .online_dot(peer.online);
            if let Some(handle) = peer.avatar.handle.clone() {
                avatar = avatar.image(handle);
            }

            // Label line: clip long display names and show the full text in a tooltip.
            let label_text = sidebar_name_text(peer.display_name.clone())
                .color(crate::design_tokens::text_primary(&theme))
                .width(Length::Fill);
            let label_el: iced::Element<'static, AppMessage> =
                if peer.display_name.chars().count() > 24 {
                    iced::widget::tooltip::Tooltip::new(
                        container(label_text).width(Length::Fill).clip(true),
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            peer.display_name.clone(),
                        )
                        .color(crate::design_tokens::text_primary(&theme)),
                        iced::widget::tooltip::Position::Right,
                    )
                    .into()
                } else {
                    container(label_text).width(Length::Fill).clip(true).into()
                };

            let mut row_el = Row::new()
                .push(avatar.build())
                .push(label_el)
                .spacing(SPACE_8)
                .align_y(Alignment::Center)
                .padding([SPACE_4, SPACE_8])
                .width(Length::Fill);

            // Chat button for every discovered peer (friend features disabled)
            row_el = row_el.push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Chat",
                ))
                .on_press(AppMessage::OpenFriendChat(peer.peer))
                    .style(crate::ui_components::button_secondary_style)
                    .padding([SPACE_4, SPACE_10]),
            );

            // Browse Files button for every discovered peer
            row_el = row_el.push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Browse Files",
                ))
                .on_press(AppMessage::BrowsePeerCatalogue(peer.peer))
                    .style(crate::ui_components::button_secondary_style)
                    .padding([SPACE_4, SPACE_10]),
            );

            section = section.push(container(row_el).width(Length::Fill));
        }

        if !has_peers {
            section = section.push(sidebar_empty_state(
                Icon::Search,
                "No peers discovered yet",
                "Peers on your local network will appear here.",
                None,
            ));
        }

        section.into()
    }

    /// Cached dependency for the sidebar's Public Rooms section.
    pub(crate) fn sidebar_public_rooms_dependency(&self) -> SidebarPublicRoomsDependency {
        // Return cached dependency if revision hasn't changed — avoids the
        // directory_store lock on every render when public rooms haven't changed.
        let cur_revision = self.public_rooms_sidebar_revision;
        if self.cached_public_rooms_revision.get() == cur_revision {
            if let Some(ref dep) = *self.cached_public_rooms_dep.borrow() {
                return dep.clone();
            }
        }

        let rooms: Vec<SidebarPublicRoomRow> = {
            let store = self.directory_store.lock().unwrap();
            let mut list = store.list_active();
            // Sort by member count descending, then by room name.
            list.sort_by(|(a, _), (b, _)| {
                b.member_count
                    .cmp(&a.member_count)
                    .then_with(|| a.room_name.cmp(&b.room_name))
            });
            list.into_iter()
                .map(|(ad, author)| SidebarPublicRoomRow {
                    room_name: ad.room_name.clone(),
                    member_count: ad.member_count,
                    author,
                    advertisement: ad,
                })
                .collect()
        };
        let dep = SidebarPublicRoomsDependency {
            dark_mode: self.dark_mode,
            local_public: self.local_public,
            rooms,
        };
        self.cached_public_rooms_revision.set(cur_revision);
        *self.cached_public_rooms_dep.borrow_mut() = Some(dep.clone());
        dep
    }

    /// \"Public Rooms\" section of the sidebar — rooms advertised on the directory topic.
    pub(crate) fn view_sidebar_public_rooms(&self) -> iced::Element<'_, AppMessage> {
        iced::widget::lazy(
            self.sidebar_public_rooms_dependency(),
            Self::view_sidebar_public_rooms_content,
        )
        .into()
    }

    pub(crate) fn view_sidebar_public_rooms_content(
        dep: &SidebarPublicRoomsDependency,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row};
        use iced::{Alignment, Length};

        let mut section = Column::new().spacing(SPACE_2);

        for room in &dep.rooms {
            let room_name = room.room_name.clone();
            let member_info = if room.member_count > 0 {
                format!("{} members", room.member_count)
            } else {
                String::new()
            };
            let ad_for_join = room.advertisement.clone();
            let is_local_room = room.author == dep.local_public;
            let mut actions = Row::new().push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Join",
                ))
                .on_press(AppMessage::DirectoryRoomJoin(ad_for_join))
                .style(crate::ui_components::button_secondary_style)
                .padding([SPACE_4, SPACE_8]),
            );
            if is_local_room {
                actions = actions.push(
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Delete",
                    ))
                    .on_press(AppMessage::DeleteDirectoryRoom(room.advertisement.topic))
                        .style(crate::ui_components::button_secondary_style)
                        .padding([SPACE_4, SPACE_8]),
                );
            }

            // Room name line: clip long names and show the full text in a tooltip.
            let name_text = sidebar_name_text(room_name.clone())
                .color(crate::design_tokens::text_primary(&Self::theme_from_dark(
                    dep.dark_mode,
                )))
                .width(Length::Fill);
            let name_el: iced::Element<'static, AppMessage> = if room_name.chars().count() > 24 {
                iced::widget::tooltip::Tooltip::new(
                    container(name_text).width(Length::Fill).clip(true),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        room_name.clone(),
                    )
                    .color(crate::design_tokens::text_primary(&Self::theme_from_dark(
                        dep.dark_mode,
                    ))),
                    iced::widget::tooltip::Position::Right,
                )
                .into()
            } else {
                container(name_text).width(Length::Fill).clip(true).into()
            };

            let row_el = Row::new()
                .push(
                    Column::new()
                        .push(name_el)
                        .push(
                            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, member_info)
                                .style(text_muted_style),
                        )
                        .spacing(SPACE_2)
                        .width(Length::Fill),
                )
                .push(actions)
                .spacing(SPACE_4)
                .align_y(Alignment::Center)
                .padding([SPACE_4, SPACE_12])
                .width(Length::Fill);

            section = section.push(container(row_el).width(Length::Fill));
        }

        if dep.rooms.is_empty() {
            section = section.push(sidebar_empty_state(
                Icon::Chat,
                "No public rooms discovered yet",
                "Rooms advertised on the directory will appear here.",
                None,
            ));
        }

        section.into()
    }

    /// Old small avatar block (pre-UI-06).  Superseded by the shared
    /// `ui_components::Avatar`; kept only until UI-22 cleanup.
    #[expect(dead_code)]
    fn peer_avatar_block(
        avatar: SidebarAvatarHandle,
        peer: PublicKey,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{container};
        use iced::{Background, Border, Length};

        if let Some(handle) = avatar.handle {
            return iced::widget::image(handle)
                .width(Length::Fixed(24.0))
                .height(Length::Fixed(24.0))
                .into();
        }

        let bytes = peer.as_bytes();
        let r = bytes[0] as f32 / 255.0;
        let g = bytes[1] as f32 / 255.0;
        let b = bytes[2] as f32 / 255.0;
        let avatar_color = Color::from_rgb(r, g, b);

        let short = peer.fmt_short().to_string();
        let first_char = short.chars().next().unwrap_or('?').to_string();

        container(
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, first_char)
                .color(Color::WHITE)
                .width(Length::Fill),
        )
        .center_y(Length::Fill)
        .width(Length::Fixed(24.0))
        .height(Length::Fixed(24.0))
        .style(move |_t| container::Style {
            background: Some(Background::Color(avatar_color)),
            border: Border {
                radius: 12.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    }

    pub(crate) fn sidebar_friends_dependency(&self) -> SidebarFriendsDependency {
        SidebarFriendsDependency {
            dark_mode: self.dark_mode,
            sidebar_revision: self.friends_sidebar_revision,
            friend_request_search_input: self.friend_request_search_input.clone(),
            friend_request_error: self.friend_request_error.clone(),
        }
    }

    /// "Friends" section of the sidebar — all friends with "Message" button.
    pub(crate) fn view_sidebar_friends(&self) -> iced::Element<'_, AppMessage> {
        let rows_dep = self.sidebar_friends_rows_dependency();
        iced::widget::lazy(self.sidebar_friends_dependency(), move |dep| {
            Self::view_sidebar_friends_content(dep, rows_dep.clone())
        })
        .into()
    }

    pub(crate) fn sidebar_friends_rows_dependency(&self) -> SidebarFriendsRowsDependency {
        // Return cached dependency if revision hasn't changed.
        let cur_revision = self.friends_sidebar_revision;
        if self.cached_friends_rows_revision.get() == cur_revision {
            if let Some(ref dep) = *self.cached_friends_rows_dep.borrow() {
                return dep.clone();
            }
        }

        let mut friends: Vec<SidebarFriendRow> = self
            .friends
            .iter()
            .filter_map(|(fid, record)| {
                if !record.relationship.can_message() {
                    return None;
                }
                let peer = fid.parse_public_key().ok()?;
                Some(SidebarFriendRow {
                    peer,
                    label: record.display_label(fid, &peer),
                    avatar: Self::sidebar_avatar_handle(
                        self.friend_image_handles
                            .get(&peer)
                            .and_then(|avatar| avatar.as_ref()),
                    ),
                    presence: self.ui_presence(&peer),
                    profile_version: self
                        .friend_profile_versions
                        .get(&peer)
                        .copied()
                        .unwrap_or(0),
                })
            })
            .collect();
        friends.sort_by(|a, b| a.label.cmp(&b.label));
        let dep = SidebarFriendsRowsDependency {
            dark_mode: self.dark_mode,
            sidebar_revision: self.friends_sidebar_revision,
            friends,
        };
        self.cached_friends_rows_revision.set(cur_revision);
        *self.cached_friends_rows_dep.borrow_mut() = Some(dep.clone());
        dep
    }

    pub(crate) fn view_sidebar_friends_content(
        dep: &SidebarFriendsDependency,
        rows_dep: SidebarFriendsRowsDependency,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let mut section = Column::new().spacing(SPACE_2);

        // Add-friend field: shared text input + trailing add-person icon.
        // Submission (Enter), validation (error border) and focus behaviour
        // are preserved: Enter submits the current value, the trailing icon
        // button submits the same message, and the field shows an error state
        // when a previous submission failed.
        let add_input = text_input_field(
            "Add friend by key…",
            &dep.friend_request_search_input,
            AppMessage::FriendRequestSearchChanged,
            !dep.friend_request_error.is_empty(),
        );
        let add_btn = button(
            Icon::UserPlus
                .build()
                .size(IconSize::Sm)
                .interactive(true)
                .build(),
        )
        .on_press(AppMessage::FriendRequestSend(
            dep.friend_request_search_input.clone(),
        ))
        .padding([SPACE_6, SPACE_8])
        .style(crate::ui_components::button_secondary_style);

        section = section.push(
            container(
                Row::new()
                    .push(add_input)
                    .push(
                        Space::new()
                            .width(Length::Fixed(SPACE_4))
                            .height(Length::Shrink),
                    )
                    .push(add_btn)
                    .align_y(Alignment::Center),
            )
            .padding(iced::Padding {
                top: SPACE_2,
                right: SPACE_12,
                bottom: SPACE_4,
                left: SPACE_12,
            })
            .width(Length::Fill),
        );

        let rows = iced::widget::lazy(rows_dep, Self::view_sidebar_friends_rows_content);

        section = section.push(rows);

        section.into()
    }

    pub(crate) fn view_sidebar_friends_rows_content(
        dep: &SidebarFriendsRowsDependency,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row};
        use iced::{Alignment, Length};

        let _timer = PerfTracker::timer("view_sidebar_friends_rows", "build");
        let theme = Self::theme_from_dark(dep.dark_mode);
        let mut section = Column::new().spacing(SPACE_2);

        let has_friends = !dep.friends.is_empty();
        for friend in &dep.friends {
            let online = friend.presence != PeerPresence::Offline;

            // Avatar with online status dot.
            let mut avatar = Avatar::new(friend.label.clone())
                .size(crate::design_tokens::AVATAR_CHAT_LIST)
                .dark_mode(dep.dark_mode)
                .online_dot(online);
            if let Some(handle) = friend.avatar.handle.clone() {
                avatar = avatar.image(handle);
            }

            // Label line: clip long friend labels / peer IDs and show the
            // full text in a tooltip.
            let label_text = sidebar_name_text(friend.label.clone())
                .color(if online {
                    crate::design_tokens::text_primary(&theme)
                } else {
                    crate::design_tokens::text_secondary(&theme)
                })
                .width(Length::Fill);
            let label_el: iced::Element<'static, AppMessage> = if friend.label.chars().count() > 24
            {
                iced::widget::tooltip::Tooltip::new(
                    container(label_text).width(Length::Fill).clip(true),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        friend.label.clone(),
                    )
                    .color(crate::design_tokens::text_primary(&theme)),
                    iced::widget::tooltip::Position::Right,
                )
                .into()
            } else {
                container(label_text).width(Length::Fill).clip(true).into()
            };

            // Overflow menu (⋮) — opens the friend profile overflow menu.
            let overflow_btn = button(
                Icon::MoreVertical
                    .build()
                    .size(IconSize::Md)
                    .interactive(true)
                    .build(),
            )
            .on_press(AppMessage::OpenFriendProfile(friend.peer))
            .padding([SPACE_4, SPACE_8])
            .style(move |t, status| iced::widget::button::Style {
                background: matches!(status, iced::widget::button::Status::Hovered)
                    .then(|| iced::Background::Color(crate::design_tokens::surface_hover(t))),
                border: iced::Border {
                    radius: crate::design_tokens::RADIUS_SM.into(),
                    ..Default::default()
                },
                text_color: if matches!(status, iced::widget::button::Status::Hovered) {
                    crate::design_tokens::primary(t)
                } else {
                    crate::design_tokens::text_muted(t)
                },
                ..Default::default()
            });

            let row_el = Row::new()
                .push(avatar.build())
                .push(label_el)
                .push(overflow_btn)
                .spacing(SPACE_8)
                .align_y(Alignment::Center)
                .padding([SPACE_4, SPACE_8])
                .width(Length::Fill);

            // Make the entire row clickable to open the friend profile
            let row_container = button(row_el)
                .on_press(AppMessage::OpenFriendProfile(friend.peer))
                .width(Length::Fill)
                .padding(0)
                .style(move |t, status| iced::widget::button::Style {
                    background: match status {
                        iced::widget::button::Status::Pressed => {
                            Some(iced::Background::Color(crate::design_tokens::surface_pressed(t)))
                        }
                        iced::widget::button::Status::Hovered => {
                            Some(iced::Background::Color(crate::design_tokens::surface_hover(t)))
                        }
                        _ => None,
                    },
                    border: iced::Border {
                        radius: crate::design_tokens::RADIUS_MD.into(),
                        ..Default::default()
                    },
                    text_color: iced::Color::TRANSPARENT,
                    ..Default::default()
                });

            section = section.push(container(row_container).width(Length::Fill));
        }

        if !has_friends {
            section = section.push(sidebar_empty_state(
                Icon::Friend,
                "No friends added yet",
                "Add someone using a key or invitation.",
                Some(("Add Friend", AppMessage::OpenFriendRequests)),
            ));
        }

        section.into()
    }

    pub(crate) fn sidebar_requests_dependency(&self) -> SidebarRequestsDependency {
        // Return cached dependency if revision hasn't changed.
        let cur_revision = self.requests_sidebar_revision;
        if self.cached_requests_revision.get() == cur_revision {
            if let Some(ref dep) = *self.cached_requests_dep.borrow() {
                return dep.clone();
            }
        }

        let local_pk_str = self.local_public.to_string();
        let mut incoming: Vec<SidebarRequestRow> = self
            .friend_request_store
            .list_incoming_by_status(
                &local_pk_str,
                boru_core::friend_request::FriendRequestStatus::Pending,
            )
            .into_iter()
            .filter_map(|request| {
                let requester = std::str::FromStr::from_str(&request.requester).ok()?;
                Some(SidebarRequestRow {
                    request_id: request.id.clone(),
                    requester,
                    label: self.resolve_name(&requester),
                })
            })
            .collect();
        incoming.sort_by(|a, b| a.label.cmp(&b.label));

        // Fetch pending group invites
        let group_invites: Vec<SidebarGroupInviteRow> = self
            .storage
            .as_ref()
            .map(|st| {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                st.get_pending_group_invites(&self.local_public.to_vec(), now_ms)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|inv| SidebarGroupInviteRow {
                        invite_id: inv.invite_id.to_vec(),
                        inviter_public_key: inv.inviter_public_key.clone(),
                        group_name: String::new(), // group name not stored in invite row
                        ticket: inv.ticket,
                        inviter_label: {
                            let mut arr = [0u8; 32];
                            let len = inv.inviter_public_key.len().min(32);
                            arr[..len].copy_from_slice(&inv.inviter_public_key[..len]);
                            PublicKey::from_bytes(&arr)
                                .map(|pk| self.resolve_name(&pk))
                                .unwrap_or_else(|_| "Unknown".to_string())
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();

        let dep = SidebarRequestsDependency {
            dark_mode: self.dark_mode,
            requests_revision: self.requests_sidebar_revision,
            incoming,
            friend_request_error: self.friend_request_error.clone(),
            group_invites,
            tunnel_requests: self
                .tunnel_requests
                .iter()
                .map(|request| SidebarTunnelRequestRow {
                    peer: request.peer,
                    tunnel_id: request.tunnel_id.clone(),
                    label: self.resolve_name(&request.peer),
                })
                .collect(),
        };
        self.cached_requests_revision.set(cur_revision);
        *self.cached_requests_dep.borrow_mut() = Some(dep.clone());
        dep
    }

    /// "Friend Requests" section of the sidebar — incoming pending requests.
    pub(crate) fn view_sidebar_requests(&self) -> iced::Element<'_, AppMessage> {
        iced::widget::lazy(
            self.sidebar_requests_dependency(),
            Self::view_sidebar_requests_content,
        )
        .into()
    }

    pub(crate) fn view_sidebar_requests_content(
        dep: &SidebarRequestsDependency,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        let mut section = Column::new().spacing(SPACE_2);

        // Manage button for opening the full friend requests screen
        section = section.push(
            container(secondary_button(
                "Manage Requests",
                Some(AppMessage::OpenFriendRequests),
                false,
            ))
            .padding(iced::Padding {
                top: SPACE_2,
                right: SPACE_12,
                bottom: SPACE_4,
                left: SPACE_12,
            })
            .width(Length::Fill),
        );

        let has_requests = !dep.incoming.is_empty()
            || !dep.group_invites.is_empty()
            || !dep.tunnel_requests.is_empty();

        if !has_requests {
            section = section.push(sidebar_empty_state(
                Icon::Notification,
                "No pending requests",
                "New friend requests will appear here.",
                None,
            ));
        } else {
            // ── Friend requests ──
            for request in &dep.incoming {
                let row_el = Row::new()
                    .push(
                        sidebar_name_text(request.label.clone()).width(Length::Fill),
                    )
                    .push(
                        button(Icon::Check.build().size(IconSize::Xs).build())
                            .on_press(AppMessage::IncomingFriendRequestAccept {
                                request_id: request.request_id.clone(),
                                peer: request.requester,
                            })
                            .padding([SPACE_2, SPACE_4])
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(accent_primary(t))),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_4.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .push(
                        button(Icon::Close.build().size(IconSize::Xs).build())
                            .on_press(AppMessage::IncomingFriendRequestDecline {
                                request_id: request.request_id.clone(),
                                peer: request.requester,
                            })
                            .padding([SPACE_2, SPACE_4])
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(color_error(t))),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_4.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .spacing(SPACE_4)
                    .align_y(Alignment::Center)
                    .padding([SPACE_4, SPACE_12])
                    .width(Length::Fill);

                section = section.push(container(row_el).width(Length::Fill));
            }

            // ── Group invites ──
            if !dep.group_invites.is_empty() {
                for invite in &dep.group_invites {
                    let inviter_label = &invite.inviter_label;
                    let invite_id = invite.invite_id.clone();
                    let row_el = Row::new()
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                format!("Group invite from {inviter_label}"),
                            )
                            .width(Length::Fill),
                        )
                        .push(
                            button(crate::fonts::type_role_text(
                                crate::fonts::TypeRole::ButtonLabel,
                                "Join",
                            ))
                            .on_press(AppMessage::AcceptGroupInvite(invite_id))
                                .padding([SPACE_2, SPACE_4])
                                .style(move |t, _status| iced::widget::button::Style {
                                    background: Some(iced::Background::Color(accent_primary(t))),
                                    text_color: Color::WHITE,
                                    border: iced::Border {
                                        radius: SPACE_4.into(),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                        )
                        .spacing(SPACE_4)
                        .align_y(Alignment::Center)
                        .padding([SPACE_4, SPACE_12])
                        .width(Length::Fill);

                    section = section.push(container(row_el).width(Length::Fill));
                }
            }

            // ── Tunnel requests ──
            for request in &dep.tunnel_requests {
                let row_el = Row::new()
                    .push(
                        Row::new()
                            .push(
                                sidebar_name_text(request.label.clone()).width(Length::Fill),
                            )
                            .push(
                                container(
                                    crate::fonts::type_role_text(
                                        crate::fonts::TypeRole::Metadata,
                                        "Tunnel",
                                    )
                                    .color(accent_primary(&theme)),
                                )
                                .padding([SPACE_2, SPACE_4])
                                .style(move |t| {
                                    iced::widget::container::Style {
                                        background: Some(iced::Background::Color(bg_surface(t))),
                                        border: iced::Border {
                                            color: border_muted(t),
                                            width: 1.0,
                                            radius: SPACE_4.into(),
                                        },
                                        ..Default::default()
                                    }
                                }),
                            )
                            .spacing(SPACE_4)
                            .align_y(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .push(
                        button(Icon::Check.build().size(IconSize::Xs).build())
                            .on_press(AppMessage::AcceptTunnelRequest(request.tunnel_id.clone()))
                            .padding([SPACE_2, SPACE_4])
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(accent_primary(t))),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_4.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .push(
                        button(Icon::Close.build().size(IconSize::Xs).build())
                            .on_press(AppMessage::DeclineTunnelRequest(request.tunnel_id.clone()))
                            .padding([SPACE_2, SPACE_4])
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(color_error(t))),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_4.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .spacing(SPACE_4)
                    .align_y(Alignment::Center)
                    .padding([SPACE_4, SPACE_12])
                    .width(Length::Fill);

                section = section.push(container(row_el).width(Length::Fill));
            }
        }

        if !dep.friend_request_error.is_empty() {
            section = section.push(
                container(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        dep.friend_request_error.clone(),
                    )
                    .color(color_error(&theme)),
                )
                .padding([SPACE_2, SPACE_12]),
            );
        }

        section.into()
    }
}
