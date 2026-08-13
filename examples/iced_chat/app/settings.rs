//! Settings feature.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the Settings screen: its
//! Hash-compatible dependency snapshots (`SettingsDependency`,
//! `SettingsCachedKey`, `ProfileIdentityCacheKey`, the Secure Tunnels row
//! types and status helpers) and the `impl IcedChat` methods that build and
//! render the settings screen. Reads app state via `use super::*`; app.rs
//! re-exports the pub(crate) items it still references with
//! `use settings::*`.

use super::*;

/// Hash-compatible snapshot of one SHARING tunnel row rendered in the
/// Settings → Secure Tunnels section. The live [`TunnelDefinition`] is not
/// Hash, so the builder pre-renders every display field into this row.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SettingsSharingTunnelRow {
    pub(crate) id: boru_core::tunnel::TunnelId,
    pub(crate) name: String,
    pub(crate) friend: String,
    pub(crate) target: String,
    /// 0 = expired, 1 = Active, 2 = Connecting, 3 = Connected, 4 = Revoked,
    /// 5 = Failed, 6 = Disconnected, 7 = Reconnecting. Mirrors
    /// `tunnel_status_color` ordering.
    pub(crate) status_kind: u8,
    /// Pre-rendered status label (e.g. "Expired", "Available", "Failed").
    pub(crate) status_label: String,
    pub(crate) remaining: String,
    pub(crate) connection_info: Option<String>,
}

/// Hash-compatible snapshot of one CONNECTED (received) tunnel row rendered
/// in the Settings → Secure Tunnels section.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SettingsConnectedTunnelRow {
    pub(crate) id: boru_core::tunnel::TunnelId,
    pub(crate) label: String,
    pub(crate) address: String,
    pub(crate) route_label: String,
    pub(crate) connection_info: Option<String>,
}

/// Hash-compatible snapshot of one hidden room rendered in the Settings →
/// Hidden rooms section (BORU-DIR-20, PDF Task 7.2).
///
/// The persisted preference (storage) is the source of truth for *which*
/// rooms are hidden; the room name is resolved from the directory cache's
/// full snapshot (which still contains hidden entries). A room whose
/// advertisement has expired or been evicted renders by its short id only.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SettingsHiddenRoomRow {
    /// The room's gossip topic bytes (the advertised room id).
    pub(crate) room_id: [u8; 32],
    /// Last-known room name from the directory cache, or a short id.
    pub(crate) room_name: String,
}

/// Dependency for the Settings screen. Delegates to the existing
/// `SettingsCachedKey` and `ProfileIdentityCacheKey` (both already Hash) and
/// adds Hash-compatible snapshots of the shared-files list and the Secure
/// Tunnels section (SHARING + CONNECTED rows).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct SettingsDependency {
    pub(crate) dark_mode: bool,
    pub(crate) cached_key: SettingsCachedKey,
    pub(crate) identity_key: ProfileIdentityCacheKey,
    pub(crate) shared_files: Vec<(String, String)>,
    pub(crate) sharing_tunnels: Vec<SettingsSharingTunnelRow>,
    pub(crate) connected_tunnels: Vec<SettingsConnectedTunnelRow>,
}

/// Map a [`TunnelDefinition`] to the [`SettingsSharingTunnelRow::status_kind`]
/// discriminant, mirroring `tunnel_status_color`'s expiry check first.
fn settings_tunnel_status_kind(
    def: &boru_core::tunnel::service::TunnelDefinition,
    now: u64,
) -> u8 {
    use boru_core::tunnel::service::TunnelStatus;
    if def.status != TunnelStatus::Revoked && def.expires_at_ms <= now {
        return 0; // Expired
    }
    match def.status {
        TunnelStatus::Active => 1,
        TunnelStatus::Connecting => 2,
        TunnelStatus::Connected => 3,
        TunnelStatus::Revoked => 4,
        TunnelStatus::Failed => 5,
        TunnelStatus::Disconnected => 6,
        TunnelStatus::Reconnecting => 7,
    }
}

/// Map a [`SettingsSharingTunnelRow::status_kind`] discriminant back to the
/// themed color used by the Settings → Secure Tunnels section, mirroring
/// `tunnel_status_color` exactly (expired tunnels render muted).
fn settings_tunnel_status_color(theme: &iced::Theme, status_kind: u8) -> iced::Color {
    match status_kind {
        0 | 4 | 6 => text_muted(theme),
        1 => accent_primary(theme),
        2 | 7 => color_warning(theme),
        3 => accent_green(theme),
        5 => color_error(theme),
        _ => text_muted(theme),
    }
}

/// Map a [`SettingsSharingTunnelRow::status_kind`] discriminant back to the
/// human-readable label, mirroring `tunnel_status_label`.
fn settings_tunnel_status_label(status_kind: u8) -> &'static str {
    use boru_core::tunnel::service::TunnelStatus;
    match status_kind {
        0 => "Expired",
        1 => TunnelStatus::Active.label(),
        2 => TunnelStatus::Connecting.label(),
        3 => TunnelStatus::Connected.label(),
        4 => TunnelStatus::Revoked.label(),
        5 => TunnelStatus::Failed.label(),
        6 => TunnelStatus::Disconnected.label(),
        7 => TunnelStatus::Reconnecting.label(),
        _ => "Unknown",
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct SettingsCachedKey {
    dark_mode: bool,
    sound_enabled: bool,
    direct_address_sharing: bool,
    chat_text_size_bits: u32,
    direct_peers: usize,
    relayed_peers: usize,
    neighbors_len: usize,
    mesh_health_label: String,
    relay_mode_label: String,
    history_confirm_clear: bool,
    history_clear_pending: bool,
    history_clear_feedback: Option<String>,
    history_clear_feedback_is_error: bool,
    local_public_key: String,
    /// Path of the configured home-screen background image, if any.
    home_background_image: Option<String>,
    /// f32 bit pattern of the home menu item background opacity, so the
    /// lazy settings screen re-renders when the slider moves.
    home_menu_item_opacity_bits: u32,
    /// Optional user-selected accent color (RGB bytes) for the ColorPicker.
    accent_color: Option<[u8; 3]>,
    /// Whether the iced_aw ColorPicker overlay is open.
    show_accent_picker: bool,
    /// Whether the optional BORU-CP-06 presence indicator is shown.
    show_presence_indicator: bool,
    /// BORU-DIR-20 (PDF Task 7.2): hidden rooms restore surface — the
    /// persisted hidden room ids resolved against the directory cache.
    /// Part of the Hash key so the lazy Settings screen re-renders when
    /// the set changes (hide/unhide).
    hidden_rooms: Vec<SettingsHiddenRoomRow>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct ProfileIdentityCacheKey {
    local_label: String,
    public_key: String,
    friend_id_copied: bool,
    profile_image_identifier: Option<String>,
    profile_image_ticket: Option<String>,
    has_profile_image: bool,
}

impl IcedChat {
    pub(crate) fn settings_cached_key(&self) -> SettingsCachedKey {
        let mesh_health_label = match &self.mesh_health {
            MeshHealth::Good => "Mesh: healthy".to_string(),
            MeshHealth::Degraded(reason) => format!("Mesh: degraded — {reason}"),
            MeshHealth::Offline(reason) => format!("Mesh: offline — {reason}"),
        };

        SettingsCachedKey {
            dark_mode: self.dark_mode,
            sound_enabled: self.sound_enabled,
            direct_address_sharing: self.share_direct_addresses,
            chat_text_size_bits: self.chat_text_size.to_bits(),
            direct_peers: self.direct_peers,
            relayed_peers: self.relayed_peers,
            neighbors_len: self.neighbors.len(),
            mesh_health_label,
            relay_mode_label: fmt_relay_mode(&self.relay_mode),
            history_confirm_clear: self.history_confirm_clear,
            history_clear_pending: self.history_clear_pending,
            history_clear_feedback: self.history_clear_feedback.clone(),
            history_clear_feedback_is_error: self.history_clear_feedback_is_error,
            local_public_key: self.local_public.to_string(),
            home_background_image: self.home_background_path.clone(),
            home_menu_item_opacity_bits: self.home_menu_item_opacity.to_bits(),
            accent_color: self.accent_color,
            show_accent_picker: self.show_accent_picker,
            show_presence_indicator: self.show_presence_indicator,
            hidden_rooms: self.settings_hidden_rooms(),
        }
    }

    /// Build the Hash-compatible hidden-rooms list for the Settings →
    /// Hidden rooms section (BORU-DIR-20, PDF Task 7.2).
    ///
    /// The persisted hide preference (storage) is the source of truth for
    /// *which* rooms are hidden; names are resolved from the directory
    /// cache's full snapshot (which still contains hidden entries), so a
    /// hidden room whose advertisement is still cached shows its last
    /// known name, and an expired/evicted one renders by short id.
    pub(crate) fn settings_hidden_rooms(&self) -> Vec<SettingsHiddenRoomRow> {
        use std::collections::{BTreeMap, BTreeSet};

        let hidden_ids: BTreeSet<[u8; 32]> = match self.storage.as_ref() {
            Some(storage) => storage.room_hidden_ids().unwrap_or_default().into_iter().collect(),
            None => BTreeSet::new(),
        };
        if hidden_ids.is_empty() {
            return Vec::new();
        }
        // Resolve last-known names from the directory cache (includes
        // hidden entries via snapshot_all).
        let mut names: BTreeMap<[u8; 32], String> = BTreeMap::new();
        if let Some(dir) = &self.room_directory {
            let guard = dir.lock().unwrap();
            for entry in guard.snapshot_all() {
                if entry.local_join_state == boru_core::room_directory::LocalJoinState::Blocked {
                    names.insert(
                        *entry.advert.room_id.as_bytes(),
                        entry.advert.room_name.clone(),
                    );
                }
            }
        }
        hidden_ids
            .into_iter()
            .map(|room_id| SettingsHiddenRoomRow {
                room_id,
                room_name: names.get(&room_id).cloned().unwrap_or_else(|| {
                    format!(
                        "{:02x}{:02x}{:02x}{:02x}…",
                        room_id[0], room_id[1], room_id[2], room_id[3]
                    )
                }),
            })
            .collect()
    }

    pub(crate) fn settings_dependency(&self) -> SettingsDependency {
        let identity_key = ProfileIdentityCacheKey {
            local_label: self.local_label.clone(),
            public_key: self.local_public.to_string(),
            friend_id_copied: self.friend_id_copied,
            profile_image_identifier: self.profile_image_identifier.clone(),
            profile_image_ticket: self.profile_image_ticket.clone(),
            has_profile_image: self.profile_image_handle.is_some(),
        };
        let cached_key = self.settings_cached_key();
        let shared_files: Vec<(String, String)> = self
            .shared_files
            .iter()
            .map(|f| (f.display_filename.clone(), f.content_hash.clone()))
            .collect();

        // SHARING tunnels: same filter + sort as the renderer used to do, but
        // every display field is pre-rendered into a Hash row so the static
        // content fn needs no live TunnelService access.
        let now = now_ms().max(0) as u64;
        let mut sharing = self
            .tunnel_service
            .list_tunnels()
            .into_iter()
            .filter(|def| {
                def.owner == self.local_public
                    && def.status != boru_core::tunnel::service::TunnelStatus::Revoked
            })
            .collect::<Vec<_>>();
        sharing.sort_by_key(|def| {
            let expired = def.expires_at_ms <= now;
            let failed = def.status == boru_core::tunnel::service::TunnelStatus::Failed;
            (expired as u8 * 2 + failed as u8, def.expires_at_ms)
        });
        let sharing_tunnels = sharing
            .into_iter()
            .map(|def| {
                let name = self
                    .shared_tunnels
                    .get(&def.id)
                    .map(|state| state.service_name.clone())
                    .unwrap_or_else(|| "Shared service".to_string());
                let friend = self.resolve_name(&def.allowed_peer);
                let target = match def.target {
                    boru_core::tunnel::service::TunnelTarget::Tcp { host, port } => {
                        tunnel_target_label(host, port)
                    }
                };
                let status_kind = settings_tunnel_status_kind(&def, now);
                let status_label = settings_tunnel_status_label(status_kind).to_string();
                let remaining = tunnel_remaining_label(def.expires_at_ms);
                let connection_info = self
                    .tunnel_service
                    .connection_info(def.id)
                    .map(tunnel_connection_info_label);
                SettingsSharingTunnelRow {
                    id: def.id,
                    name,
                    friend,
                    target,
                    status_kind,
                    status_label,
                    remaining,
                    connection_info,
                }
            })
            .collect();

        // CONNECTED (received) tunnels: same filter + sort as the renderer.
        let mut connected = self
            .received_tunnels
            .values()
            .filter(|state| state.connected)
            .collect::<Vec<_>>();
        connected.sort_by_key(|state| state.offer.expires_at_ms);
        let connected_tunnels = connected
            .into_iter()
            .map(|state| {
                let label = format!("{} — {}", state.sharer_label, state.offer.service_name);
                let address = state
                    .local_addr
                    .map(|addr| tunnel_local_address(&state.offer, addr))
                    .unwrap_or_else(|| "Connecting…".to_string());
                let route_label = state
                    .live_info
                    .as_ref()
                    .map(|live| {
                        let snapshot = live.snapshot();
                        tunnel_route_label(snapshot.route).to_string()
                    })
                    .unwrap_or_default();
                let connection_info = state
                    .live_info
                    .as_ref()
                    .map(|live| tunnel_connection_info_label(live.snapshot()));
                SettingsConnectedTunnelRow {
                    id: state.offer.tunnel_id,
                    label,
                    address,
                    route_label,
                    connection_info,
                }
            })
            .collect();

        SettingsDependency {
            dark_mode: self.dark_mode,
            cached_key,
            identity_key,
            shared_files,
            sharing_tunnels,
            connected_tunnels,
        }
    }

    pub(crate) fn view_settings_screen(&self) -> iced::Element<'_, AppMessage> {
        let dep = self.settings_dependency();
        let profile_image_handle = self.profile_image_handle.clone();
        iced::widget::lazy(dep, move |dep| {
            Self::view_settings_screen_content(dep, profile_image_handle.clone())
        })
        .into()
    }

    /// Static renderer for the Settings screen. Reads only from the
    /// Hash-compatible [`SettingsDependency`] snapshot plus the (non-Hash)
    /// profile image handle captured by the `lazy` closure.
    pub(crate) fn view_settings_screen_content(
        dep: &SettingsDependency,
        profile_image_handle: Option<iced::widget::image::Handle>,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{
            button, column, container, lazy, row, text, Column, Row, Space,
        };
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);

        // ── Header row ──────────────────────────────────────────────
        let back_btn = button(crate::fonts::type_role_text(
            crate::fonts::TypeRole::ButtonLabel,
            "←",
        ))
        .on_press(AppMessage::CloseSettings)
        .padding([SPACE_4, SPACE_6])
        .style(BUTTON_ICON);

        let header = container(
            row![
                back_btn,
                crate::fonts::type_role_text(crate::fonts::TypeRole::SectionTitle, "Settings"),
                Space::new().width(Length::Fill),
            ]
            .spacing(SPACE_8)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(crate::theme::BoruTheme::default().controls.header_height))
        .padding([SPACE_6, SPACE_10])
        .style(container_header);

        // ── Identity section ──
        let profile_identity_key = dep.identity_key.clone();
        let profile_local_label = dep.identity_key.local_label.clone();
        let profile_public_key = dep.identity_key.public_key.clone();
        let profile_friend_id_copied = dep.identity_key.friend_id_copied;
        let identity_card: iced::Element<'static, AppMessage> =
            lazy(profile_identity_key, move |_| {
                profile_identity_card(
                    profile_local_label.clone(),
                    profile_public_key.clone(),
                    profile_friend_id_copied,
                    profile_image_handle.clone(),
                )
            })
            .into();

        // ── Cacheable sections ──
        let cached_key = dep.cached_key.clone();
        let cached_sections = lazy(cached_key, Self::view_settings_screen_cached);

        // ── Shared files ──
        let mut shared_file_rows: Vec<iced::Element<'static, AppMessage>> = Vec::new();

        if dep.shared_files.is_empty() {
            shared_file_rows.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "No shared files. Add files to share them with your contacts.",
                )
                .style(text_muted_style)
                .into(),
            );
        } else {
            for (display_filename, content_hash) in &dep.shared_files {
                let hash_short = if content_hash.len() > 8 {
                    &content_hash[..8]
                } else {
                    content_hash
                };
                let file_row = Row::new()
                    .push(
                        Column::new()
                            .push(crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                display_filename.clone(),
                            ))
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::TechnicalValue,
                                    format!("hash: {hash_short}…"),
                                )
                                .style(text_muted_style),
                            )
                            .spacing(SPACE_2)
                            .width(Length::Fill)
                            .align_x(Alignment::Start),
                    )
                    .push(
                        button(crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Remove",
                        ))
                        .on_press(AppMessage::RemoveSharedFile(content_hash.clone()))
                            .padding([SPACE_2, SPACE_6])
                            .style(|t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(
                                    crate::theme::BoruTheme::for_theme(t)
                                        .colors
                                        .settings_danger_strong,
                                )),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_4.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center);
                shared_file_rows.push(file_row.into());
            }
        }

        let add_button_row = Row::new().push(
            button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                "Add File",
            ))
            .on_press(AppMessage::AddSharedFile)
            .style(BUTTON_PRIMARY)
            .padding([SPACE_6, SPACE_12]),
        );

        shared_file_rows.push(add_button_row.into());

        let shared_files_card = section_card("SHARED FILES", shared_file_rows);

        // ── Secure Tunnels section ──────────────────────────────────
        // Two groups: SHARING (tunnels this user created locally, live
        // metadata from the backend TunnelService) and CONNECTED (received
        // tunnel offers the user has connected a local listener to).
        // Both groups are pre-rendered into the Hash snapshot by
        // `settings_dependency()` so this renderer stays static.
        let mut tunnel_rows: Vec<iced::Element<'static, AppMessage>> = Vec::new();

        let sharing_empty = dep.sharing_tunnels.is_empty();

        if sharing_empty {
            tunnel_rows.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "No services currently shared.",
                )
                .style(text_muted_style)
                .into(),
            );
        } else {
            tunnel_rows.push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "SHARING")
                    .style(text_muted_style)
                    .into(),
            );
            for row in &dep.sharing_tunnels {
                let status_color = settings_tunnel_status_color(&theme, row.status_kind);
                let mut info_column = Column::new()
                    .push(
                        Row::new()
                            .push(crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                row.name.clone(),
                            ))
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Metadata,
                                    format!(" · {}", row.status_label),
                                )
                                .color(status_color),
                            )
                            .spacing(SPACE_4)
                            .align_y(Alignment::Center),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            format!("With {}", row.friend),
                        )
                        .style(text_muted_style),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            format!("{}  ·  {}", row.target, row.remaining),
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start);
                if let Some(label) = &row.connection_info {
                    info_column = info_column.push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            label.clone(),
                        )
                        .style(text_muted_style),
                    );
                }
                let row_el = Row::new()
                    .push(info_column)
                    .push(
                        button(crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Stop Sharing",
                        ))
                        .on_press(AppMessage::StopSharingTunnel(row.id))
                        .padding([SPACE_2, SPACE_8])
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
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center);
                tunnel_rows.push(row_el.into());
            }
        }

        let connected_empty = dep.connected_tunnels.is_empty();

        if !connected_empty {
            tunnel_rows.push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "CONNECTED")
                    .style(text_muted_style)
                    .into(),
            );
            for row in &dep.connected_tunnels {
                let mut info_column = Column::new()
                    .push(
                        Row::new()
                            .push(crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                row.label.clone(),
                            ))
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Metadata,
                                    if !row.route_label.is_empty() {
                                        format!(" · {}", row.route_label)
                                    } else {
                                        String::new()
                                    },
                                )
                                .color(accent_green(&theme)),
                            )
                            .spacing(SPACE_4)
                            .align_y(Alignment::Center),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::TechnicalValue,
                            row.address.clone(),
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start);
                if let Some(info) = &row.connection_info {
                    info_column = info_column.push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            info.clone(),
                        )
                        .style(text_muted_style),
                    );
                }
                let row_el = Row::new()
                    .push(info_column)
                    .push(
                        button(crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Disconnect",
                        ))
                        .on_press(AppMessage::DisconnectReceivedTunnel(row.id))
                        .padding([SPACE_2, SPACE_8]),
                    )
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center);
                tunnel_rows.push(row_el.into());
            }
        }

        if sharing_empty && connected_empty {
            tunnel_rows.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "Share a local service from a friend's profile to see it here.",
                )
                .style(text_muted_style)
                .into(),
            );
        }

        let secure_tunnels_card = section_card("SECURE TUNNELS", tunnel_rows);

        // ── Body (scrollable) ──
        let body = column![
            identity_card,
            Space::new().height(Length::Fixed(SPACE_12)),
            cached_sections,
            Space::new().height(Length::Fixed(SPACE_12)),
            shared_files_card,
            Space::new().height(Length::Fixed(SPACE_12)),
            secure_tunnels_card,
            Space::new().height(Length::Fixed(SPACE_24)),
        ]
        .spacing(SPACE_6)
        .padding(SPACE_24)
        .align_x(Alignment::Start)
        .width(Length::Fill)
        .max_width(680.0);

        let scrollable = crate::ui_components::gutter_scrollable(container(body).width(Length::Fill).center_x(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill);

        column![header, scrollable]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_settings_screen_cached(key: &SettingsCachedKey) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, row, text, Column, Row, Space};
        use iced::{Alignment, Color, Length};

        let appearance_theme = if key.dark_mode { "Dark" } else { "Light" };

        let appearance_row = Row::new()
            .push(
                Column::new()
                    .push(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Body,
                        appearance_theme,
                    ))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Switch between dark and light colour themes.",
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    if key.dark_mode { "Light" } else { "Dark" },
                ))
                .on_press(AppMessage::ToggleDark(!key.dark_mode))
                .style(BUTTON_OUTLINE)
                .padding([SPACE_6, SPACE_12]),
            )
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        // ── Chat text size ──
        let text_sizes: &[(f32, &str)] = &[
            (TYPO_XS, "XS"),
            (TYPO_SM, "SM"),
            (TYPO_MD, "MD"),
            (TYPO_LG, "LG"),
            (TYPO_XL, "XL"),
        ];
        let current_size = f32::from_bits(key.chat_text_size_bits);
        let text_size_row = Row::new().push(
            Column::new()
                .push(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Body,
                    format!("Text size: {}px", current_size as u32),
                ))
                .push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        "Choose the font size for chat message bodies.",
                    )
                    .style(text_muted_style),
                )
                .spacing(SPACE_2)
                .width(Length::Fill)
                .align_x(Alignment::Start),
        );
        let text_size_row = text_sizes
            .iter()
            .fold(text_size_row, |row, &(size, label)| {
                let is_active = (current_size - size).abs() < 0.5;
                row.push(
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        label,
                    ))
                    .on_press(AppMessage::SetChatTextSize(size))
                    .padding([SPACE_2, SPACE_6])
                    .style(move |t, status| {
                        if is_active {
                            iced::widget::button::Style {
                                background: Some(iced::Background::Color(accent_primary(t))),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_6.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        } else {
                            iced::widget::button::Style {
                                background: None,
                                text_color: match status {
                                    iced::widget::button::Status::Hovered => accent_primary(t),
                                    _ => crate::theme::BoruTheme::for_theme(t).colors.glyph_disabled,
                                },
                                border: iced::Border {
                                    radius: SPACE_6.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        }
                    }),
                )
                .spacing(SPACE_6)
            })
            .align_y(Alignment::Center)
            .spacing(SPACE_8);

        // ── Home screen background ──
        // Choose/remove the image rendered behind the home (ChatList) screen.
        let home_background_label = key
            .home_background_image
            .as_deref()
            .and_then(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
            })
            .unwrap_or("None")
            .to_string();
        let mut home_background_actions = Row::new()
            .push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Choose image…",
                ))
                .on_press(AppMessage::PickHomeBackgroundImage)
                .style(BUTTON_OUTLINE)
                .padding([SPACE_6, SPACE_12]),
            )
            .spacing(SPACE_8);
        if key.home_background_image.is_some() {
            home_background_actions = home_background_actions.push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Remove",
                ))
                .on_press(AppMessage::RemoveHomeBackgroundImage)
                .style(BUTTON_OUTLINE)
                .padding([SPACE_6, SPACE_12]),
            );
        }
        let home_background_row = Row::new()
            .push(
                Column::new()
                    .push(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Body,
                        format!("Home background: {home_background_label}"),
                    ))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Show an image behind the home screen content.",
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(home_background_actions)
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        // ── Accent color (ICEDAW-01) ──
        // Optional accent-color customization via iced_aw ColorPicker. The
        // picked RGB value is persisted in AppSettings and overrides
        // `accent_primary` app-wide. The dark-mode toggle above is untouched.
        let accent_theme = Self::theme_from_dark(key.dark_mode);
        let accent_rgb = key.accent_color.unwrap_or_else(|| {
            let c = accent_primary(&accent_theme);
            [
                (c.r * 255.0).round() as u8,
                (c.g * 255.0).round() as u8,
                (c.b * 255.0).round() as u8,
            ]
        });
        let accent_color = iced::Color::from_rgb(
            accent_rgb[0] as f32 / 255.0,
            accent_rgb[1] as f32 / 255.0,
            accent_rgb[2] as f32 / 255.0,
        );
        let accent_swatch = container(
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, " "),
        )
        .width(24.0)
        .height(24.0)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_t| container::Style {
            background: Some(iced::Background::Color(accent_color)),
            border: iced::Border {
                radius: SPACE_6.into(),
                ..Default::default()
            },
            ..Default::default()
        });
        let accent_button = button(
            row![accent_swatch, crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                if key.show_accent_picker { "Pick color…" } else { "Customize…" },
            )]
            .spacing(SPACE_6)
            .align_y(Alignment::Center),
        )
        .on_press(AppMessage::ToggleAccentColorPicker)
        .style(BUTTON_OUTLINE)
        .padding([SPACE_4, SPACE_8]);
        let accent_color_picker = iced_aw::ColorPicker::new(
            key.show_accent_picker,
            accent_color,
            accent_button,
            AppMessage::AccentColorCancelled,
            |c| {
                AppMessage::AccentColorSelected([
                    (c.r * 255.0).round() as u8,
                    (c.g * 255.0).round() as u8,
                    (c.b * 255.0).round() as u8,
                ])
            },
        )
        .style(move |t, _status| iced_aw::style::color_picker::Style {
            background: iced::Background::Color(bg_surface(t)),
            border_radius: crate::theme::BoruTheme::default().controls.color_picker_radius,
            border_width: 1.0,
            border_color: border_muted(t),
            bar_border_radius: crate::theme::BoruTheme::default().controls.color_picker_bar_radius,
            bar_border_width: 1.0,
            bar_border_color: border_muted(t),
        });
        let accent_row = Row::new()
            .push(
                Column::new()
                    .push(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Body,
                        "Accent color",
                    ))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Customize the app's primary accent color.",
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(accent_color_picker)
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        // ── Home menu item transparency (HOME-01) ──
        // Shown only when a home background image is set: controls the
        // opacity of the home-screen menu/action card backgrounds that sit
        // over the image.
        let mut appearance_children: Vec<iced::Element<'static, AppMessage>> = vec![
            appearance_row.into(),
            Space::new().height(Length::Fixed(SPACE_8)).into(),
            accent_row.into(),
            Space::new().height(Length::Fixed(SPACE_8)).into(),
            text_size_row.into(),
            Space::new().height(Length::Fixed(SPACE_8)).into(),
            home_background_row.into(),
        ];
        if key.home_background_image.is_some() {
            let opacity = f32::from_bits(key.home_menu_item_opacity_bits);
            let pct = (opacity * 100.0).round() as u32;
            appearance_children.push(Space::new().height(Length::Fixed(SPACE_8)).into());
            appearance_children.push(
                Row::new()
                    .push(
                        Column::new()
                            .push(crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                format!("Menu item opacity: {pct}%"),
                            ))
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::SupportingText,
                                    "Set the transparency of home menu item backgrounds over the image.",
                                )
                                .style(text_muted_style),
                            )
                            .spacing(SPACE_2)
                            .width(Length::Fill)
                            .align_x(Alignment::Start),
                    )
                    .push(
                        iced::widget::slider(
                            0.20..=1.0,
                            opacity,
                            AppMessage::SetHomeMenuItemOpacity,
                        )
                        .step(0.05)
                        .width(Length::Fixed(crate::theme::BoruTheme::default().controls.slider_width)),
                    )
                    .spacing(SPACE_12)
                    .align_y(Alignment::Center)
                    .into(),
            );
        }
        let appearance_card = section_card("APPEARANCE", appearance_children);

        // ── Notifications section ──
        let sound_label = if key.sound_enabled {
            "Sound on"
        } else {
            "Sound off"
        };
        let notifications_row = Row::new()
            .push(
                Column::new()
                    .push(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Body,
                        sound_label,
                    ))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Play a notification sound when a new message arrives.",
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    if key.sound_enabled { "Mute" } else { "Unmute" },
                ))
                .on_press(AppMessage::ToggleSound(!key.sound_enabled))
                .style(BUTTON_OUTLINE)
                .padding([SPACE_6, SPACE_12]),
            )
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        let notifications_card = section_card("NOTIFICATIONS", vec![notifications_row.into()]);

        // ── Presence section (BORU-CP-06, PDF 2.3) ──
        // Optional UI presence indicator derived from the backend
        // connectivity state machine. Disabling it only hides the badge —
        // it never affects discovery or reconnection.
        let presence_label = if key.show_presence_indicator {
            "On"
        } else {
            "Off"
        };
        let presence_row = Row::new()
            .push(
                Column::new()
                    .push(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Body,
                        "Presence indicator",
                    ))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Show Online / Recently seen / Connecting / Offline status \
                             next to contacts. Derived from the connectivity state \
                             machine; hiding it never affects discovery or reconnection.",
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    presence_label,
                ))
                .on_press(AppMessage::TogglePresenceIndicator(!key.show_presence_indicator))
                .style(BUTTON_OUTLINE)
                .padding([SPACE_6, SPACE_12]),
            )
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        let presence_card = section_card("PRESENCE", vec![presence_row.into()]);

        // ── Network section ──
        let connection_details_focus_anchor = iced::widget::text_input("", "")
            .id(CONNECTION_DETAILS_TRIGGER_INPUT)
            .on_input(|_| AppMessage::Noop)
            .padding([0.0, 0.0])
            .width(Length::Fixed(1.0));

        let connection_details_trigger = iced::widget::Stack::new()
            .push(connection_details_focus_anchor)
            .push(
                button(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Advanced details",
                ))
                .on_press(AppMessage::OpenConnectionDetails)
                .style(BUTTON_OUTLINE)
                .padding([SPACE_6, SPACE_12]),
            );

        let connection_details_row = Row::new()
            .push(
                Column::new()
                    .push(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Body,
                        "Advanced details",
                    ))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Open the redacted support snapshot for connection diagnostics.",
                        )
                        .style(text_muted_style)
                        .wrapping(iced::widget::text::Wrapping::Glyph),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill)
                    .align_x(Alignment::Start),
            )
            .push(connection_details_trigger)
            .spacing(SPACE_12)
            .align_y(Alignment::Center);

        let connection_info = row![crate::fonts::type_role_text(
            crate::fonts::TypeRole::Body,
            format!(
                "{} direct · {} relay · {} neighbors",
                key.direct_peers, key.relayed_peers, key.neighbors_len,
            ),
        )]
        .spacing(SPACE_4);

        let mesh_status = row![crate::fonts::type_role_text(
            crate::fonts::TypeRole::Body,
            key.mesh_health_label.clone(),
        )]
        .spacing(SPACE_4);

        let network_card = section_card(
            "NETWORK",
            vec![
                connection_info.into(),
                mesh_status.into(),
                connection_details_row.into(),
            ],
        );

        // ── Relay section ──
        let relay_info =
            row![crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                format!("Mode: {}", key.relay_mode_label),
            )]
            .spacing(SPACE_4);

        let relay_note = crate::fonts::type_role_text(
            crate::fonts::TypeRole::SupportingText,
            "Relay mode is set at startup and cannot be changed at runtime.",
        )
        .style(text_muted_style);

        let relay_card = section_card("RELAY", vec![relay_info.into(), relay_note.into()]);

        // ── Logs & Diagnostics section removed per user request ──
        // ── Data Management section ──
        let clear_history_feedback = key
            .history_clear_feedback
            .clone()
            .map(|message| (message, key.history_clear_feedback_is_error));
        let clear_history_row = {
            let title = if key.history_confirm_clear {
                "Clear chat history?"
            } else {
                "Clear chat history"
            };
            let description = if key.history_clear_pending {
                "Clearing the active chat's stored messages…"
            } else if key.history_confirm_clear {
                "This will delete the active chat's stored messages permanently."
            } else {
                "Delete all stored messages for the active chat permanently."
            };
            let status_line = clear_history_feedback.as_ref().map(|(message, is_error)| {
                // Theme-independent by design (both modes render the same
                // literal); use the light-palette captures.
                let colors = crate::theme::BoruTheme::default().colors;
                let color = if *is_error {
                    colors.settings_danger
                } else {
                    colors.settings_success
                };
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, message.clone())
                    .style(move |_| iced::widget::text::Style { color: Some(color) })
            });

            let action_buttons = if key.history_clear_pending {
                Row::new().push(
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Clearing…",
                    ))
                    .padding([SPACE_6, SPACE_12]),
                )
            } else if key.history_confirm_clear {
                Row::new()
                    .push(
                        button(crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Confirm",
                        ))
                        .on_press(AppMessage::ConfirmClearHistory)
                        .padding([SPACE_6, SPACE_12]),
                    )
                    .push(
                        button(crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Cancel",
                        ))
                        .on_press(AppMessage::ClearHistoryRequested)
                        .padding([SPACE_6, SPACE_12]),
                    )
                    .spacing(SPACE_8)
            } else {
                Row::new().push(
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Clear",
                    ))
                    .on_press(AppMessage::ClearHistoryRequested)
                    .padding([SPACE_6, SPACE_12]),
                )
            };

            let is_danger_state = key.history_confirm_clear || key.history_clear_pending;

            let mut column = Column::new()
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Body, title)
                        .style(move |t| iced::widget::text::Style {
                            color: Some(if is_danger_state {
                                crate::theme::BoruTheme::for_theme(t).colors.settings_danger
                            } else {
                                accent_primary(t)
                            }),
                        }),
                )
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, description)
                        .style(text_muted_style),
                )
                .spacing(SPACE_2)
                .width(Length::Fill)
                .align_x(Alignment::Start);

            if let Some(status_line) = status_line {
                column = column.push(status_line);
            }

            Row::new()
                .push(column)
                .push(action_buttons)
                .spacing(SPACE_12)
                .align_y(Alignment::Center)
        };

        let data_card = section_card("DATA", vec![clear_history_row.into()]);

        // ── Hidden rooms section (BORU-DIR-20, PDF Task 7.2) ──
        // Local Hide/Block choices stay private (never broadcast); this
        // settings surface is where the user can undo local hiding — the
        // explicit reset path the PDF requires. Restoring a room removes
        // the persisted preference so the room is offered again in
        // Discover on the next refresh.
        let mut hidden_room_rows: Vec<iced::Element<'static, AppMessage>> = Vec::new();
        if key.hidden_rooms.is_empty() {
            hidden_room_rows.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "No rooms hidden. Rooms you Hide in Discover appear here so you can restore them.",
                )
                .style(text_muted_style)
                .into(),
            );
        } else {
            hidden_room_rows.push(
                Row::new()
                    .push(
                        Column::new()
                            .push(crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                format!("{} hidden room(s)", key.hidden_rooms.len()),
                            ))
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::SupportingText,
                                    "These rooms are hidden only on this device and are never broadcast.",
                                )
                                .style(text_muted_style),
                            )
                            .spacing(SPACE_2)
                            .width(Length::Fill)
                            .align_x(Alignment::Start),
                    )
                    .push(
                        button(crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Restore all",
                        ))
                        .on_press(AppMessage::DirectoryRoomUnhideAll)
                        .style(BUTTON_OUTLINE)
                        .padding([SPACE_6, SPACE_12]),
                    )
                    .spacing(SPACE_12)
                    .align_y(Alignment::Center)
                    .into(),
            );
            for room in &key.hidden_rooms {
                let short_id = format!(
                    "{:02x}{:02x}{:02x}{:02x}…",
                    room.room_id[0], room.room_id[1], room.room_id[2], room.room_id[3]
                );
                hidden_room_rows.push(
                    Row::new()
                        .push(
                            Column::new()
                                .push(crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Body,
                                    room.room_name.clone(),
                                ))
                                .push(
                                    crate::fonts::type_role_text(
                                        crate::fonts::TypeRole::TechnicalValue,
                                        short_id,
                                    )
                                    .style(text_muted_style),
                                )
                                .spacing(SPACE_2)
                                .width(Length::Fill)
                                .align_x(Alignment::Start),
                        )
                        .push(
                            button(crate::fonts::type_role_text(
                                crate::fonts::TypeRole::ButtonLabel,
                                "Unhide",
                            ))
                            .on_press(AppMessage::DirectoryRoomUnhideById(room.room_id))
                            .style(BUTTON_OUTLINE)
                            .padding([SPACE_6, SPACE_12]),
                        )
                        .spacing(SPACE_12)
                        .align_y(Alignment::Center)
                        .into(),
                );
            }
        }
        let hidden_rooms_card = section_card("HIDDEN ROOMS", hidden_room_rows);

        // ── Privacy section (KLIPY-09) ──
        // Concise note about external GIF search: it is optional, and the only
        // data that leaves the device is the search term sent to the KLIPY
        // provider.  Boru never sends identity, messages, contacts, or
        // attachment metadata to KLIPY and adds no behavioural analytics.
        let gif_privacy_row = Column::new()
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Body,
                    "External GIF search",
                )
                .style(move |t| iced::widget::text::Style {
                    color: Some(crate::theme::BoruTheme::for_theme(t).colors.settings_heading_text),
                    ..Default::default()
                }),
            )
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "GIF search is optional and only runs when you search in the GIF picker. \
                     Search terms are sent to the KLIPY GIF service. Boru never sends your \
                     identity, messages, contacts, or attachment metadata to KLIPY, and adds \
                     no behavioural analytics.",
                )
                .style(text_muted_style)
                .wrapping(iced::widget::text::Wrapping::Glyph),
            )
            .spacing(SPACE_2)
            .width(Length::Fill)
            .align_x(Alignment::Start);

        let privacy_card = section_card("PRIVACY", vec![gif_privacy_row.into()]);

        // ── Assemble page ──
        let content = Column::new()
            .push(appearance_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(notifications_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(presence_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(network_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(relay_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(data_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(hidden_rooms_card)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(privacy_card)
            .spacing(SPACE_6)
            .width(Length::Fill);

        let scrollable = crate::ui_components::gutter_scrollable(
            container(content)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill);

        container(scrollable)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(container_primary)
            .into()
    }

    /// State-layer update for settings (BORU-AUDIT-22 spec step 5).
    ///
    /// Handles theme/display preferences (dark mode, accent, nickname, chat
    /// text size), sound and address-sharing toggles, profile image pick/
    /// upload/remove, and home background image pick/remove. The root
    /// `update()` dispatches these variants here via combined match arms.
    pub(crate) fn update_settings(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::ToggleDark(enabled) => {
                self.dark_mode = enabled;
                // Dark mode is part of every screen's dependency snapshot;
                // forget the pre-warmed trees so the next idle cycle rebuilds
                // them with the new theme.
                self.invalidate_prewarm(PREWARM_ORDER);
                if let Some(action_id) = self
                    .gui_action_history
                    .all_actions()
                    .into_iter()
                    .find(|status| {
                        status.state == GuiActionState::AppMessageQueued
                            && matches!(
                                status.expected_state,
                                Some(boru_core::diagnostics::ExpectedState::DarkModeIs(expected))
                                    if expected == enabled
                            )
                    })
                    .map(|status| status.action_id)
                {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                let settings = AppSettings {
                    dark_mode: self.dark_mode,
                    sound_enabled: self.sound_enabled,
                    share_direct_addresses: self.share_direct_addresses,
                    chat_text_size: self.chat_text_size,
                    display_name: Some(self.local_label.clone()),
                    home_background_image: self.home_background_path.clone(),
                    home_menu_item_opacity: self.home_menu_item_opacity,
                    accent_color: self.accent_color,
                    show_presence_indicator: self.show_presence_indicator,
                };
                let data_dir = self.data_dir.clone();
                let _progress_queue = self.download_progress_queue.clone();
                iced::Task::perform(
                    tokio::task::spawn_blocking(move || {
                        settings.save(&data_dir);
                    }),
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::ToggleAccentColorPicker => {
                self.show_accent_picker = !self.show_accent_picker;
                // The ColorPicker renders as an overlay; while it is open the
                // Settings screen must NOT be served from the prewarm cache
                // (Prebuilt drops overlays), so invalidate so the next frame
                // takes the live lazy path.
                self.invalidate_prewarm(&[Screen::Settings]);
                iced::Task::none()
            }

            AppMessage::AccentColorSelected(rgb) => {
                self.accent_color = Some(rgb);
                set_accent_override(Some(rgb));
                self.show_accent_picker = false;
                self.invalidate_prewarm(PREWARM_ORDER);
                // Persist the accent color so it survives restarts.
                let settings = AppSettings {
                    dark_mode: self.dark_mode,
                    sound_enabled: self.sound_enabled,
                    share_direct_addresses: self.share_direct_addresses,
                    chat_text_size: self.chat_text_size,
                    display_name: Some(self.local_label.clone()),
                    home_background_image: self.home_background_path.clone(),
                    home_menu_item_opacity: self.home_menu_item_opacity,
                    accent_color: self.accent_color,
                    show_presence_indicator: self.show_presence_indicator,
                };
                let data_dir = self.data_dir.clone();
                iced::Task::perform(
                    tokio::task::spawn_blocking(move || {
                        settings.save(&data_dir);
                    }),
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::AccentColorCancelled => {
                self.show_accent_picker = false;
                self.invalidate_prewarm(&[Screen::Settings]);
                iced::Task::none()
            }

            AppMessage::SetNickname(name) => {
                self.local_label = name;
                // The local label feeds the Settings identity card.
                self.invalidate_prewarm(&[Screen::Settings]);
                // Persist the display name so it survives restarts.
                let settings = AppSettings {
                    dark_mode: self.dark_mode,
                    sound_enabled: self.sound_enabled,
                    share_direct_addresses: self.share_direct_addresses,
                    chat_text_size: self.chat_text_size,
                    display_name: Some(self.local_label.clone()),
                    home_background_image: self.home_background_path.clone(),
                    home_menu_item_opacity: self.home_menu_item_opacity,
                    accent_color: self.accent_color,
                    show_presence_indicator: self.show_presence_indicator,
                };
                let data_dir = self.data_dir.clone();
                iced::Task::perform(
                    tokio::task::spawn_blocking(move || {
                        settings.save(&data_dir);
                    }),
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::SetChatTextSize(size) => {
                self.chat_text_size = size;
                self.layout_cache.borrow_mut().invalidate_all();
                let settings = AppSettings {
                    dark_mode: self.dark_mode,
                    sound_enabled: self.sound_enabled,
                    share_direct_addresses: self.share_direct_addresses,
                    chat_text_size: self.chat_text_size,
                    display_name: Some(self.local_label.clone()),
                    home_background_image: self.home_background_path.clone(),
                    home_menu_item_opacity: self.home_menu_item_opacity,
                    accent_color: self.accent_color,
                    show_presence_indicator: self.show_presence_indicator,
                };
                let data_dir = self.data_dir.clone();
                let _progress_queue = self.download_progress_queue.clone();
                iced::Task::perform(
                    tokio::task::spawn_blocking(move || {
                        settings.save(&data_dir);
                    }),
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::ToggleSound(enabled) => {
                self.sound_enabled = enabled;
                let settings = AppSettings {
                    dark_mode: self.dark_mode,
                    sound_enabled: self.sound_enabled,
                    share_direct_addresses: self.share_direct_addresses,
                    chat_text_size: self.chat_text_size,
                    display_name: Some(self.local_label.clone()),
                    home_background_image: self.home_background_path.clone(),
                    home_menu_item_opacity: self.home_menu_item_opacity,
                    accent_color: self.accent_color,
                    show_presence_indicator: self.show_presence_indicator,
                };
                let data_dir = self.data_dir.clone();
                let _progress_queue = self.download_progress_queue.clone();
                iced::Task::perform(
                    tokio::task::spawn_blocking(move || {
                        settings.save(&data_dir);
                    }),
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::TogglePresenceIndicator(enabled) => {
                self.show_presence_indicator = enabled;
                // The presence badge is pure presentation — toggling it
                // never touches discovery or reconnection. Bump the
                // sidebar revisions so friend rows re-render with/without
                // the state-machine-derived badge.
                self.mark_friends_sidebar_dirty();
                self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                self.invalidate_prewarm(&[Screen::Settings]);
                let settings = AppSettings {
                    dark_mode: self.dark_mode,
                    sound_enabled: self.sound_enabled,
                    share_direct_addresses: self.share_direct_addresses,
                    chat_text_size: self.chat_text_size,
                    display_name: Some(self.local_label.clone()),
                    home_background_image: self.home_background_path.clone(),
                    home_menu_item_opacity: self.home_menu_item_opacity,
                    accent_color: self.accent_color,
                    show_presence_indicator: self.show_presence_indicator,
                };
                let data_dir = self.data_dir.clone();
                let _progress_queue = self.download_progress_queue.clone();
                iced::Task::perform(
                    tokio::task::spawn_blocking(move || {
                        settings.save(&data_dir);
                    }),
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::ToggleInviteAddressSharing(enabled) => {
                self.share_direct_addresses = enabled;
                let settings = AppSettings {
                    dark_mode: self.dark_mode,
                    sound_enabled: self.sound_enabled,
                    share_direct_addresses: self.share_direct_addresses,
                    chat_text_size: self.chat_text_size,
                    display_name: Some(self.local_label.clone()),
                    home_background_image: self.home_background_path.clone(),
                    home_menu_item_opacity: self.home_menu_item_opacity,
                    accent_color: self.accent_color,
                    show_presence_indicator: self.show_presence_indicator,
                };
                let data_dir = self.data_dir.clone();
                iced::Task::perform(
                    tokio::task::spawn_blocking(move || {
                        settings.save(&data_dir);
                    }),
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::PickProfileImage => iced::Task::perform(
                async {
                    let file = rfd::AsyncFileDialog::new()
                        .set_title("Choose profile image")
                        .pick_file()
                        .await;
                    match file {
                        Some(file) => {
                            if !supported_profile_image(file.path()) {
                                return Err("Unsupported profile image type. Use PNG, JPEG, GIF, WEBP, or BMP.".to_string());
                            }
                            let bytes = file.read().await;
                            if bytes.is_empty() {
                                Err("Profile image is empty.".to_string())
                            } else if bytes.len() > PROFILE_IMAGE_MAX_BYTES {
                                Err("Profile image must be 5 MiB or smaller.".to_string())
                            } else {
                                Ok(bytes)
                            }
                        }
                        None => Err("No profile image selected.".to_string()),
                    }
                },
                AppMessage::ProfileImagePicked,
            ),

            AppMessage::ProfileImagePicked(result) => {
                match result {
                    Ok(bytes) => {
                        // Save to per-user image store and persist the
                        // identifier in a background thread to avoid blocking
                        // the UI thread on blake3 hashing and file I/O.
                        let image_store = self.image_store.clone();
                        let user = self.local_public.to_string();
                        let data_dir = self.data_dir.clone();
                        let _progress_queue = self.download_progress_queue.clone();
                        self.push_system("Saving profile image…");
                        iced::Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    let identifier = match image_store.save_image(
                                        &user,
                                        "profile-image",
                                        &bytes,
                                    ) {
                                        Ok(id) => id,
                                        Err(e) => {
                                            return Err(format!(
                                                "Could not save profile image: {e}"
                                            ));
                                        }
                                    };
                                    // Persist the identifier so it can be reloaded on restart.
                                    let id_file = data_dir.join(".profile-image-id");
                                    let _ = std::fs::write(&id_file, &identifier);
                                    // Return both identifier and the image bytes for
                                    // the UI handle and blob store upload.
                                    Ok((identifier, bytes))
                                })
                                .await
                                .unwrap_or_else(|join_err| Err(format!("Join error: {join_err}")))
                            },
                            |result: Result<(String, Vec<u8>), String>| match result {
                                Ok((identifier, image_bytes)) => {
                                    AppMessage::ProfileImagePersisted {
                                        identifier,
                                        image_bytes,
                                    }
                                }
                                Err(e) => AppMessage::SystemMsg(e),
                            },
                        )
                    }
                    Err(err) if err != "No profile image selected." => {
                        self.push_system(err);
                        iced::Task::none()
                    }
                    Err(_) => iced::Task::none(),
                }
            }

            AppMessage::PickHomeBackgroundImage => iced::Task::perform(
                async {
                    let file = rfd::AsyncFileDialog::new()
                        .set_title("Choose home screen background image")
                        .pick_file()
                        .await;
                    match file {
                        Some(file) => Ok(file.path().to_string_lossy().to_string()),
                        None => Err("No background image selected.".to_string()),
                    }
                },
                AppMessage::HomeBackgroundImagePicked,
            ),

            AppMessage::HomeBackgroundImagePicked(result) => {
                match result {
                    Ok(path) => {
                        if path.is_empty() {
                            return iced::Task::none();
                        }
                        // Read the image bytes and persist the settings entry
                        // on a background thread so a large image file never
                        // blocks the UI thread.
                        let path_for_task = path.clone();
                        let data_dir = self.data_dir.clone();
                        let settings = AppSettings {
                            dark_mode: self.dark_mode,
                            sound_enabled: self.sound_enabled,
                            share_direct_addresses: self.share_direct_addresses,
                            chat_text_size: self.chat_text_size,
                            display_name: Some(self.local_label.clone()),
                            home_background_image: Some(path.clone()),
                            home_menu_item_opacity: self.home_menu_item_opacity,
                            accent_color: self.accent_color,
                            show_presence_indicator: self.show_presence_indicator,
                        };
                        iced::Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    settings.save(&data_dir);
                                    std::fs::read(&path_for_task)
                                        .map_err(|e| format!("Failed to read image: {e}"))
                                        .and_then(|bytes| {
                                            if bytes.is_empty() {
                                                Err("Selected image is empty.".to_string())
                                            } else {
                                                Ok(bytes)
                                            }
                                        })
                                })
                                .await
                                .unwrap_or_else(|e| Err(format!("Task join error: {e}")))
                            },
                            move |read_result: Result<Vec<u8>, String>| match read_result {
                                Ok(image_bytes) => AppMessage::HomeBackgroundImageReady {
                                    path,
                                    image_bytes,
                                },
                                Err(e) => AppMessage::SystemMsg(e),
                            },
                        )
                    }
                    Err(e) if e != "No background image selected." => {
                        self.push_system(e);
                        iced::Task::none()
                    }
                    Err(_) => iced::Task::none(),
                }
            }

            AppMessage::HomeBackgroundImageReady { path, image_bytes } => {
                self.home_background_path = Some(path);
                self.home_background_handle =
                    Some(iced::widget::image::Handle::from_bytes(image_bytes));
                self.invalidate_prewarm(&[Screen::Settings]);
                self.push_system("Home screen background updated.");
                iced::Task::none()
            }

            AppMessage::RemoveHomeBackgroundImage => {
                self.home_background_path = None;
                self.home_background_handle = None;
                self.invalidate_prewarm(&[Screen::Settings]);
                self.push_system("Home screen background removed.");
                Self::persist_home_background(
                    &self.data_dir,
                    self.dark_mode,
                    self.sound_enabled,
                    self.share_direct_addresses,
                    self.chat_text_size,
                    self.local_label.clone(),
                    None,
                    self.home_menu_item_opacity,
                    self.accent_color,
                    self.show_presence_indicator,
                )
            }

            AppMessage::SetHomeMenuItemOpacity(opacity) => {
                self.home_menu_item_opacity = opacity.clamp(0.0, 1.0);
                self.invalidate_prewarm(&[Screen::Settings]);
                // Persist so the value survives restarts (HOME-01).
                let settings = AppSettings {
                    dark_mode: self.dark_mode,
                    sound_enabled: self.sound_enabled,
                    share_direct_addresses: self.share_direct_addresses,
                    chat_text_size: self.chat_text_size,
                    display_name: Some(self.local_label.clone()),
                    home_background_image: self.home_background_path.clone(),
                    home_menu_item_opacity: self.home_menu_item_opacity,
                    accent_color: self.accent_color,
                    show_presence_indicator: self.show_presence_indicator,
                };
                let data_dir = self.data_dir.clone();
                iced::Task::perform(
                    tokio::task::spawn_blocking(move || {
                        settings.save(&data_dir);
                    }),
                    |_| AppMessage::Noop,
                )
            }

            AppMessage::ProfileImagePersisted {
                identifier,
                image_bytes,
            } => {
                self.profile_image_identifier = Some(identifier);
                self.profile_image_handle =
                    Some(iced::widget::image::Handle::from_bytes(image_bytes.clone()));
                self.push_system("Profile image updated.");

                // Upload the image to the local blob store so peers can
                // download it via the BlobTicket advertised in AboutMe.
                let blob_store = self.blob_store.clone();
                let endpoint = self.endpoint.clone();
                iced::Task::perform(
                    async move {
                        let tag = blob_store
                            .blobs()
                            .add_bytes(image_bytes)
                            .await
                            .map_err(|e| format!("Failed to store profile image: {e}"))?;
                        let ticket_str =
                            blob_ticket_string(endpoint.watch_addr().get(), tag.hash, tag.format);
                        Ok(ticket_str)
                    },
                    |r: Result<String, String>| match r {
                        Ok(ticket) => AppMessage::ProfileImageUploaded(ticket),
                        Err(e) => AppMessage::ErrorMsg(e),
                    },
                )
            }

            AppMessage::ProfileImageUploaded(ticket) => {
                self.profile_image_ticket = Some(ticket);
                // Broadcast the updated AboutMe so peers fetch our new image.
                if let Some(ref sender) = self.sender {
                    let sk = self.secret_key.clone();
                    let label = self.local_label.clone();
                    let ticket = self.profile_image_ticket.clone();
                    let s = sender.clone();
                    iced::Task::perform(
                        async move {
                            if let Ok(encoded) = SignedMessage::sign_and_encode(
                                &sk,
                                &crate::Message::AboutMe {
                                    name: label,
                                    profile_image_ticket: ticket,
                                },
                            ) {
                                s.broadcast(encoded).await.ok();
                            }
                        },
                        |_| AppMessage::Noop,
                    )
                } else {
                    iced::Task::none()
                }
            }

            AppMessage::RemoveProfileImage => {
                if self.profile_image_handle.is_some() {
                    // Collect the data needed for the blocking delete,
                    // then spawn it off the UI thread.
                    let user = self.local_public.to_string();
                    let image_store = self.image_store.clone();
                    let identifier = self.profile_image_identifier.clone();
                    let data_dir = self.data_dir.clone();
                    let _progress_queue = self.download_progress_queue.clone();
                    iced::Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                if let Some(ref id) = identifier {
                                    match image_store.delete_image(&user, id) {
                                        Ok(_) => {
                                            let id_file = data_dir.join(".profile-image-id");
                                            let _ = std::fs::remove_file(&id_file);
                                            Ok(())
                                        }
                                        Err(e) => Err(e.to_string()),
                                    }
                                } else {
                                    // Legacy path — remove the old flat file if it exists.
                                    match fs::remove_file(data_dir.join(PROFILE_IMAGE_FILE)) {
                                        Ok(()) => Ok(()),
                                        Err(ref err)
                                            if err.kind() == std::io::ErrorKind::NotFound =>
                                        {
                                            Ok(())
                                        }
                                        Err(e) => Err(e.to_string()),
                                    }
                                }
                            })
                            .await
                            .unwrap_or_else(|join_err| Err(format!("Join error: {join_err}")))
                        },
                        |result: Result<(), String>| match result {
                            Ok(()) => AppMessage::ProfileImageRemoved,
                            Err(e) => AppMessage::SystemMsg(format!(
                                "Could not remove profile image: {e}"
                            )),
                        },
                    )
                } else {
                    iced::Task::none()
                }
            }

            AppMessage::ProfileImageRemoved => {
                self.profile_image_handle = None;
                self.profile_image_ticket = None;
                self.profile_image_identifier = None;
                self.push_system("Profile image removed.");
                // Re-broadcast AboutMe with no ticket so peers stop
                // showing our old image.
                if let Some(ref sender) = self.sender {
                    let sk = self.secret_key.clone();
                    let label = self.local_label.clone();
                    let s = sender.clone();
                    return iced::Task::perform(
                        async move {
                            if let Ok(encoded) = SignedMessage::sign_and_encode(
                                &sk,
                                &crate::Message::AboutMe {
                                    name: label,
                                    profile_image_ticket: None,
                                },
                            ) {
                                s.broadcast(encoded).await.ok();
                            }
                        },
                        |_| AppMessage::Noop,
                    );
                }
                iced::Task::none()
            }

            AppMessage::OpenSettings => {
                if !matches!(self.screen, Screen::Settings) {
                    self.settings_return_to = Some(self.screen.clone());
                    self.screen = Screen::Settings;
                }
                if let Some(action_id) = self.pending_open_settings_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                iced::Task::none()
            }

            AppMessage::CloseSettings => {
                self.screen = self.settings_return_to.take().unwrap_or(Screen::ChatList);
                iced::Task::none()
            }

            #[cfg(feature = "terminal")]
            AppMessage::OpenTerminal => {
                self.screen = Screen::Terminal;
                iced::Task::none()
            }

            #[cfg(feature = "terminal")]
            AppMessage::TerminalEvent(iced_term::Event::BackendCall(_, cmd)) => {
                if let Some(term) = self.terminal.as_mut() {
                    match term.update(cmd) {
                        iced_term::actions::Action::Shutdown => {
                            // The embedded shell exited — leave the terminal tab.
                            if matches!(self.screen, Screen::Terminal) {
                                self.screen = Screen::ChatList;
                            }
                        }
                        iced_term::actions::Action::ChangeTitle(_)
                        | iced_term::actions::Action::Ignore => {}
                    }
                }
                iced::Task::none()
            }
            // update() only dispatches the settings variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
