//! Home screen (chat list / landing) feature.
//!
//! Extracted from app.rs (BORU-AUDIT-22). This child module owns the home /
//! chat-list screen: its Hash-compatible dependency snapshots, the rail-card
//! data structs, and the `impl IcedChat` methods that build and render them.
//! It reads app state via `use super::*` (child modules can see the parent's
//! private items); app.rs re-exports the pub(crate) items it still references
//! with `use home::*`.

use super::*;

/// Hash-compatible snapshot of [`MeshHealth`] for use inside screen
/// dependencies. The reason strings are the only data the renderers read from
/// the enum, so capturing them here lets a static renderer rebuild the hero /
/// mesh cards without borrowing app state.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) enum MeshHealthSnapshot {
    Good,
    Degraded(String),
    Offline(String),
}

impl From<&MeshHealth> for MeshHealthSnapshot {
    fn from(m: &MeshHealth) -> Self {
        match m {
            MeshHealth::Good => MeshHealthSnapshot::Good,
            MeshHealth::Degraded(r) => MeshHealthSnapshot::Degraded(r.clone()),
            MeshHealth::Offline(r) => MeshHealthSnapshot::Offline(r.clone()),
        }
    }
}

impl MeshHealthSnapshot {
    pub(crate) fn as_mesh_health(&self) -> MeshHealth {
        match self {
            MeshHealthSnapshot::Good => MeshHealth::Good,
            MeshHealthSnapshot::Degraded(r) => MeshHealth::Degraded(r.clone()),
            MeshHealthSnapshot::Offline(r) => MeshHealth::Offline(r.clone()),
        }
    }
}

/// Dependency for the ChatList (home / empty-state) screen. Captures the
/// hero / mesh card / action-grid state plus the rail-card selectors so the
/// whole screen rebuilds only when any of its rendered slices change.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct ChatListDependency {
    pub(crate) dark_mode: bool,
    pub(crate) window_width_bits: u32,
    pub(crate) mesh_health: MeshHealthSnapshot,
    pub(crate) main_screen_reconnect_frame: u32,
    pub(crate) local_label: String,
    pub(crate) time_of_day_greeting: String,
    pub(crate) has_peer_connections: bool,
    pub(crate) sender_ready: bool,
    pub(crate) direct_peers: u32,
    pub(crate) relayed_peers: u32,
    pub(crate) neighbors_len: u32,
    pub(crate) connected_age_secs: Option<u64>,
    /// Newest mesh event log rows (message + age at snapshot time) rendered
    /// in the Mesh Health card. `age_secs` is captured when the dependency is
    /// built so the snapshot stays Hash/Eq-compatible (the log stores
    /// `Instant`, which is not Hash); the per-second `ActivityTick` already
    /// rebuilds this screen via the rail-card `tick`, so ages stay fresh.
    pub(crate) mesh_events: Vec<MeshEventRow>,
    pub(crate) online: OnlinePeersCardData,
    pub(crate) activity: RecentActivityCardData,
    pub(crate) tunnels: TunnelsCardData,
    /// f32 bit pattern of the home menu item background opacity — included
    /// so the lazy home screen re-renders when the setting changes.
    pub(crate) home_menu_item_opacity_bits: u32,
    /// Mesh-pulse phase for the status card's network canvas, derived from
    /// the per-second `ActivityTick` so the card's slow node brighten/fade
    /// costs nothing extra (the rail-card `tick` already rebuilds this
    /// screen every second). Always advanced, not gated on connection
    /// state; the status card ignores it unless it may animate.
    pub(crate) hero_pulse_frame: u32,
    /// OS reduced-motion preference — the status card keeps its mesh
    /// static when this is set.
    pub(crate) reduced_motion: bool,
}

/// One mesh event log row, snapshot for the home dependency.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct MeshEventRow {
    /// Event message text (e.g. "Discovered 2 direct, 1 relayed peers").
    pub(crate) message: String,
    /// Whole seconds since the event was recorded, at snapshot time.
    pub(crate) age_secs: u64,
}

/// Dependency for the Online Peers card. Friend presence rows only.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct OnlinePeersCardData {
    pub(crate) dark_mode: bool,
    /// Number of friends the user can message (count-badge denominator).
    pub(crate) total_friends: usize,
    /// Online/Away friend rows (Offline friends are filtered out).
    pub(crate) rows: Vec<OnlinePeerRow>,
    /// UI-HOME-15: two-line compact header on narrow content widths.
    pub(crate) compact_header: bool,
    /// Home menu item background opacity (f32 bit pattern) so the lazy
    /// card rebuilds when the transparency setting changes.
    pub(crate) home_menu_item_opacity_bits: u32,
}

/// One Online Peers row: the peer key (for the open-chat action), the
/// resolved display name, the live presence state (drives the secondary
/// status line), and the avatar handle (keyed so image bytes do not
/// defeat equality checks).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct OnlinePeerRow {
    pub(crate) pk: PublicKey,
    pub(crate) name: String,
    /// Live presence derived from `peer_presence_map` (+ AWAY_THRESHOLD_MS).
    pub(crate) presence: PeerPresence,
    pub(crate) avatar: SidebarAvatarHandle,
}

/// Minimum Online Peers body height (px). A single 60 px peer row is
/// floored to this so the card keeps a sensible ~220–280 px footprint
/// instead of collapsing into a strip; short lists never stretch it.
pub(crate) const PEERS_BODY_MIN: f32 = 128.0;

/// Maximum Online Peers body height (px): exactly five 60 px rows plus
/// four SPACE_2 gaps. The 6th online peer scrolls (same overflow
/// contract as the pre-UI-HOME-07 card).
pub(crate) const PEERS_BODY_MAX: f32 =
    5.0 * crate::card_shell::PEER_ROW_HEIGHT + 4.0 * crate::design_tokens::SPACE_2;

/// Dependency for the Recent Activity card. `tick` is bumped once per second
/// by `ActivityTick` so relative timestamps re-render while idle; `rows`
/// changes only when a real activity event is pushed.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct RecentActivityCardData {
    pub(crate) dark_mode: bool,
    pub(crate) tick: u64,
    /// Full ring-buffer length (drives the count badge).
    pub(crate) total: usize,
    /// The newest activity rows actually rendered (capped at 15).
    pub(crate) rows: Vec<ActivityRow>,
    /// UI-HOME-15: two-line compact header on narrow content widths.
    pub(crate) compact_header: bool,
    /// Home menu item background opacity (f32 bit pattern) so the lazy
    /// card rebuilds when the transparency setting changes.
    pub(crate) home_menu_item_opacity_bits: u32,
}

/// Empty-state copy for the Online Peers rail card (UI-HOME-16 spec copy).
pub(crate) const ONLINE_PEERS_EMPTY_MESSAGE: &str =
    "No peers are online right now. Connected peers will appear here.";

/// Empty-state copy for the Recent Activity rail card (UI-HOME-16 spec copy).
pub(crate) const RECENT_ACTIVITY_EMPTY_MESSAGE: &str =
    "No recent activity. Network events will appear here.";

/// Empty-state copy for the Tunnels rail card (UI-HOME-08 spec copy).
pub(crate) const TUNNELS_EMPTY_MESSAGE: &str =
    "No active tunnels. Create or join a tunnel to securely route traffic.";

/// Empty-state copy for the Recent events section of the Mesh Health card
/// (UI-HOME-16: retain the connection summary above, explain the empty feed).
pub(crate) const MESH_EVENTS_EMPTY_MESSAGE: &str = "No recent mesh events";

/// One Recent Activity row. `timestamp` is kept stable so an unchanged buffer
/// compares equal across frames — only `tick` makes the card rebuild for
/// fresh relative timestamps.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct ActivityRow {
    pub(crate) description: String,
    pub(crate) kind: ActivityKind,
    pub(crate) timestamp: SystemTime,
}

/// Dependency for the Tunnels card. `tick` is included so a tunnel that
/// expires while the app is idle flips to "Expired" within a second; `rows`
/// changes only when the live TunnelService snapshot or the shared-tunnel
/// name map changes.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct TunnelsCardData {
    pub(crate) dark_mode: bool,
    pub(crate) tick: u64,
    pub(crate) rows: Vec<TunnelRow>,
    /// UI-HOME-15: two-line compact header on narrow content widths.
    pub(crate) compact_header: bool,
    /// Home menu item background opacity (f32 bit pattern) so the lazy
    /// card rebuilds when the transparency setting changes.
    pub(crate) home_menu_item_opacity_bits: u32,
}

/// One Tunnels row. `expired` is resolved against the wall clock at selector
/// time so status labels never invent a state; the close action uses `id`.
///
/// `Hash` is implemented manually because `TunnelStatus` does not implement
/// it — the discriminant is hashed, which is all `iced::widget::lazy`'s cache
/// key needs (the actual change detection uses `PartialEq`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TunnelRow {
    pub(crate) id: boru_core::tunnel::TunnelId,
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) status: TunnelStatus,
    pub(crate) expired: bool,
}

impl std::hash::Hash for TunnelRow {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.name.hash(state);
        self.endpoint.hash(state);
        std::mem::discriminant(&self.status).hash(state);
        self.expired.hash(state);
    }
}

impl IcedChat {
    /// Selector for the Online Peers card: friends with live presence plus
    /// the total friend count for the badge denominator.
    pub(crate) fn online_peers_card_data(&self) -> OnlinePeersCardData {
        let total_friends = self
            .friends
            .iter()
            .filter(|(_, r)| r.relationship.can_message())
            .count();
        let rows = self
            .friends
            .iter()
            .filter_map(|(fid, _)| {
                let pk = fid.parse_public_key().ok()?;
                let presence = self.peer_presence(&pk);
                if presence == PeerPresence::Offline {
                    return None;
                }
                Some(OnlinePeerRow {
                    pk,
                    name: self.resolve_name(&pk),
                    presence,
                    avatar: Self::sidebar_avatar_handle(
                        self.friend_image_handles
                            .get(&pk)
                            .and_then(|slot| slot.as_ref()),
                    ),
                })
            })
            .collect();
        OnlinePeersCardData {
            dark_mode: self.dark_mode,
            total_friends,
            rows,
            compact_header: self.home_compact_headers(),
            home_menu_item_opacity_bits: self.home_menu_item_opacity.to_bits(),
        }
    }

    /// Selector for the Recent Activity card: the ring-buffer slice only
    /// (badge total + the newest 15 rendered rows). `tick` is included so the
    /// per-second ActivityTick refreshes relative timestamps while idle.
    pub(crate) fn recent_activity_card_data(&self) -> RecentActivityCardData {
        let rows = self
            .recent_activity
            .iter()
            .take(15)
            .map(|event| ActivityRow {
                description: event.description.clone(),
                kind: event.kind,
                timestamp: event.timestamp,
            })
            .collect();
        RecentActivityCardData {
            dark_mode: self.dark_mode,
            tick: self.activity_tick,
            total: self.recent_activity.len(),
            rows,
            compact_header: self.home_compact_headers(),
            home_menu_item_opacity_bits: self.home_menu_item_opacity.to_bits(),
        }
    }

    /// Selector for the Tunnels card: the live TunnelService snapshot plus
    /// the shared-tunnel name map needed to label rows. `tick` is included so
    /// a tunnel expiring while idle flips to "Expired" within a second.
    pub(crate) fn tunnels_card_data(&self) -> TunnelsCardData {
        let rows = self
            .tunnel_service
            .list_tunnels()
            .into_iter()
            .map(|def| {
                let now = now_ms().max(0) as u64;
                let expired = def.status != TunnelStatus::Revoked && def.expires_at_ms <= now;
                let endpoint = match def.target {
                    boru_core::tunnel::service::TunnelTarget::Tcp { host, port } => {
                        tunnel_target_label(host, port)
                    }
                };
                let name = self
                    .shared_tunnels
                    .get(&def.id)
                    .map(|state| state.service_name.clone())
                    .unwrap_or_else(|| {
                        self.names
                            .get(&def.allowed_peer)
                            .cloned()
                            .unwrap_or_else(|| def.allowed_peer.fmt_short().to_string())
                    });
                TunnelRow {
                    id: def.id,
                    name,
                    endpoint,
                    status: def.status,
                    expired,
                }
            })
            .collect();
        TunnelsCardData {
            dark_mode: self.dark_mode,
            tick: self.activity_tick,
            rows,
            compact_header: self.home_compact_headers(),
            home_menu_item_opacity_bits: self.home_menu_item_opacity.to_bits(),
        }
    }

    /// True when the home content width is narrow enough that card headers
    /// switch to the two-line compact layout (UI-HOME-15).
    pub(crate) fn home_compact_headers(&self) -> bool {
        crate::design_tokens::home_content_width(self.window_width)
            < crate::design_tokens::HOME_COMPACT_HEADER_CONTENT
    }

    /// Build the Online Peers card subtree. Runs inside `iced::widget::lazy`,
    /// so it is only re-invoked when `OnlinePeersCardData` actually changes.
    pub(crate) fn view_online_peers_card(dep: &OnlinePeersCardData) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        let peer_rows: Vec<iced::Element<'static, AppMessage>> = dep
            .rows
            .iter()
            .map(|row| {
                let mut avatar = Avatar::new(row.name.clone())
                    .size(crate::design_tokens::AVATAR_SM)
                    .dark_mode(dep.dark_mode)
                    .online_dot(true)
                    .fallback_icon(Icon::Friend);
                if let Some(handle) = row.avatar.handle.clone() {
                    avatar = avatar.image(handle);
                }
                // Structured row: avatar (with live online dot) + a two-line
                // text column — display name on top, live presence secondary
                // status below (Online / Away / Connecting…, coloured with
                // the status palette).
                let presence_color = row.presence.color(&theme);
                let text_col = Column::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Body,
                            row.name.clone(),
                        )
                        .color(text_system(&theme))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            row.presence.label(),
                        )
                        .color(presence_color),
                    )
                    .spacing(crate::design_tokens::SPACE_2)
                    .align_x(Alignment::Start)
                    .width(Length::Fill);
                let row_el = Row::new()
                    // Zero-width spacer enforces the 60 px two-line row
                    // rhythm as a MINIMUM; a wrapped display name grows the
                    // row instead of being clipped (UI-HOME-10).
                    .push(Space::new().width(Length::Fixed(0.0)).height(Length::Fixed(crate::card_shell::PEER_ROW_HEIGHT)))
                    .push(avatar.build())
                    .push(Space::new().width(Length::Fixed(SPACE_8)))
                    .push(text_col)
                    .spacing(0)
                    .align_y(Alignment::Center);
                button(row_el)
                    .on_press(AppMessage::OpenConversation(row.pk))
                    .width(Length::Fill)
                    .padding([0.0, SPACE_8])
                    .style(|t, status| iced::widget::button::Style {
                        // Hover surface for the interactive row. Note: iced
                        // 0.14 `button::Status` has no `Focused` variant
                        // (Active/Hovered/Pressed/Disabled only) and buttons
                        // are not keyboard-focusable in this version, so
                        // hover is the interaction affordance here.
                        background: matches!(status, iced::widget::button::Status::Hovered).then(
                            || iced::Background::Color(crate::design_tokens::surface_hover(t)),
                        ),
                        border: iced::Border {
                            radius: crate::design_tokens::RADIUS_SM.into(),
                            ..Default::default()
                        },
                        text_color: iced::Color::TRANSPARENT,
                        ..Default::default()
                    })
                    .into()
            })
            .collect();

        // Content-driven body with a floor: the list grows with the number
        // of online peers up to five visible rows (the 6th scrolls) and never
        // collapses below PEERS_BODY_MIN, so a single peer keeps the card at
        // a sensible ~220–280 px footprint instead of a tiny strip or a huge
        // blank panel.
        let body: iced::Element<'static, AppMessage> = if dep.rows.is_empty() {
            // UI-HOME-16: intentional empty state — small muted icon beside
            // the spec copy, vertically centred in the min-height body so the
            // card stays balanced (never a tiny strip, never a huge blank
            // panel). The text has Fill width + word wrapping so the
            // two-sentence copy reflows at narrow rail widths.
            container(
                Row::new()
                    .push(icon_svg(ICON_FRIEND, TYPO_SM).style(move |t, _| {
                        iced::widget::svg::Style {
                            color: Some(text_muted(t)),
                        }
                    }))
                    .push(Space::new().width(Length::Fixed(SPACE_8)))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            ONLINE_PEERS_EMPTY_MESSAGE,
                        )
                        .color(text_muted(&theme))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                    )
                    .spacing(0)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fixed(PEERS_BODY_MIN))
            .align_y(Alignment::Center)
            .into()
        } else {
            crate::ui_components::gutter_scrollable(
                Column::with_children(peer_rows)
                    .spacing(SPACE_2)
                    .width(Length::Fill),
            )
            .height(Length::Fixed(Self::online_peers_body_height(
                dep.rows.len(),
            )))
            .width(Length::Fill)
            .into()
        };

        crate::card_shell::CardShell::new("Online Peers", vec![])
            .count(dep.rows.len())
            .count_total(dep.total_friends)
            .on_view_all(AppMessage::OpenFriendRequests)
            .compact_header(dep.compact_header)
            .body(body)
            .background_opacity(f32::from_bits(dep.home_menu_item_opacity_bits))
            .build(&theme)
    }

    /// Content-driven height of the Online Peers body (px): the shorter of
    /// the row content and the five-visible-rows cap, floored at
    /// [`PEERS_BODY_MIN`] so a one-peer card stays intentional.
    pub(crate) fn online_peers_body_height(rows: usize) -> f32 {
        if rows == 0 {
            return PEERS_BODY_MIN;
        }
        let content =
            rows as f32 * crate::card_shell::PEER_ROW_HEIGHT + (rows as f32 - 1.0) * SPACE_2;
        content.min(PEERS_BODY_MAX).max(PEERS_BODY_MIN)
    }

    /// Build the Recent Activity card subtree (memoized via lazy).
    pub(crate) fn view_recent_activity_card(
        dep: &RecentActivityCardData,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{container, row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        // UI-29: recent activity rows are denser than the 48 px peer rows —
        // a compact 32 px row keeps the feed scannable without dead vertical
        // space around the small icon + single-line title.
        const ACTIVITY_ROW_HEIGHT: f32 = 32.0;
        let activity_rows: Vec<iced::Element<'static, AppMessage>> = dep
            .rows
            .iter()
            .map(|event| {
                let ago = crate::presentation::relative_time_from_system(event.timestamp);
                let activity_icon = match event.kind {
                    ActivityKind::Online => ICON_ONLINE,
                    ActivityKind::Offline => ICON_OFFLINE,
                    ActivityKind::FileShared => ICON_FILES,
                    ActivityKind::Message => ICON_CHAT,
                    ActivityKind::Generic => ICON_ACTIVITY,
                };
                // Copy the kind out of the borrowed row so the icon style
                // closure stays 'static (owned values only) — required for
                // the lazy content builder's `Element<'static, _>` return.
                let kind = event.kind;
                // Min-height floor keeps the dense 32 px single-line rhythm;
                // a wrapped description grows the row instead of being
                // clipped (UI-HOME-10: no fixed row height, no clip, no
                // 40-char truncation).
                container(
                row![
                    Space::new()
                        .width(Length::Fixed(0.0))
                        .height(Length::Fixed(ACTIVITY_ROW_HEIGHT)),
                    icon_svg(activity_icon, TYPO_SM).style(move |t, _| {
                        iced::widget::svg::Style {
                            color: Some(if kind == ActivityKind::Online {
                                accent_green(t)
                            } else {
                                text_muted(t)
                            }),
                        }
                    }),
                    container(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Body,
                            event.description.clone(),
                        )
                        .color(text_system(&theme))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                    )
                    .width(Length::Fill),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        ago,
                    )
                    .color(text_muted(&theme)),
                ]
                .spacing(SPACE_6)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([0.0, SPACE_8])
            .align_y(Alignment::Center)
            .into()
            })
            .collect();

        CardShell::new("Recent Activity", activity_rows)
            .count(dep.total)
            .empty_icon(
                icon_svg(ICON_ACTIVITY, TYPO_SM).style(move |t, _| {
                    iced::widget::svg::Style {
                        color: Some(text_muted(t)),
                    }
                }).into(),
            )
            .empty_message(RECENT_ACTIVITY_EMPTY_MESSAGE)
            .compact_header(dep.compact_header)
            .max_height(180.0)
            .background_opacity(f32::from_bits(dep.home_menu_item_opacity_bits))
            .build(&theme)
    }

    /// Build the Tunnels card subtree (memoized via lazy).
    pub(crate) fn view_tunnels_card(dep: &TunnelsCardData) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, row, Column, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        let tunnel_rows: Vec<iced::Element<'static, AppMessage>> = dep
            .rows
            .iter()
            .map(|tunnel| {
                let status = if tunnel.expired {
                    "Expired"
                } else {
                    tunnel.status.label()
                };
                let status_color = if tunnel.expired {
                    text_muted(&theme)
                } else {
                    match tunnel.status {
                        TunnelStatus::Active => accent_primary(&theme),
                        TunnelStatus::Connecting => color_warning(&theme),
                        TunnelStatus::Connected => accent_green(&theme),
                        TunnelStatus::Revoked => text_muted(&theme),
                        TunnelStatus::Failed => color_error(&theme),
                        TunnelStatus::Disconnected => text_muted(&theme),
                        TunnelStatus::Reconnecting => color_warning(&theme),
                    }
                };
                container(
                    row![
                        // Min-height floor keeps the 48 px single-line rhythm;
                        // a long tunnel name / endpoint wraps and grows the
                        // row instead of being clipped (UI-HOME-10).
                        Space::new()
                            .width(Length::Fixed(0.0))
                            .height(Length::Fixed(crate::card_shell::CARD_ROW_HEIGHT)),
                        icon_svg(ICON_LOCK, TYPO_SM).style(move |t, _| {
                            iced::widget::svg::Style {
                                color: Some(status_color),
                            }
                        }),
                        Column::new()
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Body,
                                    tunnel.name.clone(),
                                )
                                .color(text_system(&theme))
                                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                            )
                            .push(
                                // host:port — genuine technical value → JetBrains Mono.
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::TechnicalValue,
                                    tunnel.endpoint.clone(),
                                )
                                .color(text_muted(&theme))
                                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                            )
                            .spacing(SPACE_2)
                            .align_x(Alignment::Start)
                            .width(Length::Fill),
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            status,
                        )
                        .color(status_color),
                        button(
                            Icon::Close
                                .build()
                                .size(IconSize::Xs)
                                .destructive(true)
                                .build()
                        )
                        .on_press(AppMessage::CloseTunnel(tunnel.id))
                        .padding([SPACE_2, SPACE_6])
                        .style(BUTTON_GHOST_BG),
                    ]
                    .spacing(SPACE_6)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .into()
            })
            .collect();

        // UI-HOME-16: when the list is empty the header action label becomes
        // "Create tunnel" (the dialog the copy points at) instead of the
        // misleading "View all"; the destination is unchanged.
        let header_action_label = if dep.rows.is_empty() {
            "Create tunnel"
        } else {
            "View all"
        };

        crate::card_shell::CardShell::new("Tunnels", tunnel_rows)
            .count(dep.rows.len())
            .header_action(header_action_label, AppMessage::ShowCreateTunnelDialog)
            .empty_icon(
                icon_svg(ICON_LOCK, TYPO_SM).style(move |t, _| {
                    iced::widget::svg::Style {
                        color: Some(text_muted(t)),
                    }
                }).into(),
            )
            .empty_message(TUNNELS_EMPTY_MESSAGE)
            .compact_header(dep.compact_header)
            .max_height(120.0)
            .background_opacity(f32::from_bits(dep.home_menu_item_opacity_bits))
            .build(&theme)
    }

    /// Header-action label for the Tunnels card: "Create tunnel" when the
    /// list is empty (the dialog the empty copy points at), "View all"
    /// once live tunnels exist. The destination is the same in both cases
    /// (`ShowCreateTunnelDialog`).
    pub(crate) fn tunnels_header_action_label(rows: usize) -> &'static str {
        if rows == 0 {
            "Create tunnel"
        } else {
            "View all"
        }
    }

    // ── Main panel (empty state — landing screen) ─────────────────────

    /// Landing screen shown when no conversation is selected.
    /// Redesigned: connection status first, then actions, then activity.
    pub(crate) fn view_main_empty_state(&self) -> iced::Element<'_, AppMessage> {
        let dep = self.chat_list_dependency();
        iced::widget::lazy(dep, Self::view_chat_list_content).into()
    }

    /// Builds the ChatList (home / empty-state) screen's renderable snapshot.
    pub(crate) fn chat_list_dependency(&self) -> ChatListDependency {
        let has_peer_connections =
            !self.neighbors.is_empty() || self.relayed_peers > 0 || self.direct_peers > 0;
        let connected_age_secs = self.mesh_connected_at.map(|t| {
            Instant::now()
                .saturating_duration_since(t)
                .as_secs()
        });
        // Newest mesh events first (the log pushes to the back), capped at the
        // number the card renders. Age is captured here so the snapshot stays
        // Hash/Eq-compatible; the per-second ActivityTick rebuild keeps ages
        // fresh.
        let now = Instant::now();
        let mesh_events: Vec<MeshEventRow> = self
            .mesh_event_log
            .iter()
            .rev()
            .take(4)
            .map(|event| MeshEventRow {
                message: event.message.clone(),
                age_secs: now.saturating_duration_since(event.recorded_at).as_secs(),
            })
            .collect();
        ChatListDependency {
            dark_mode: self.dark_mode,
            window_width_bits: (self.window_width * 100.0) as u32,
            mesh_health: MeshHealthSnapshot::from(&self.mesh_health),
            main_screen_reconnect_frame: self.main_screen_reconnect_frame as u32,
            local_label: self.local_label.clone(),
            time_of_day_greeting: self.time_of_day_greeting().to_string(),
            has_peer_connections,
            sender_ready: self.sender.is_some(),
            direct_peers: self.direct_peers as u32,
            relayed_peers: self.relayed_peers as u32,
            neighbors_len: self.neighbors.len() as u32,
            connected_age_secs,
            mesh_events,
            online: self.online_peers_card_data(),
            activity: self.recent_activity_card_data(),
            tunnels: self.tunnels_card_data(),
            home_menu_item_opacity_bits: self.home_menu_item_opacity.to_bits(),
            hero_pulse_frame: (self.activity_tick % crate::status_card::STATUS_CARD_PULSE_PHASES as u64)
                as u32,
            reduced_motion: self.reduced_motion,
        }
    }

    /// Static renderer for the ChatList (home / empty-state) screen, driven by
    /// [`ChatListDependency`] so `iced::widget::lazy` can cache the whole
    /// screen while any of its rendered slices is unchanged.
    pub(crate) fn view_chat_list_content(dep: &ChatListDependency) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, row, Column, Row, Space};
        use iced::{Alignment, Length};

        let window_width = dep.window_width_bits as f32 / 100.0;
        let theme = Self::theme_from_dark(dep.dark_mode);
        // UI-HOME-15: all home breakpoints are based on the dashboard's
        // available *content* width (window minus sidebar, divider and page
        // padding), never the raw window width — the sidebar eats
        // 288–320 px and would otherwise starve the grid on narrow windows.
        let content_width = crate::design_tokens::home_content_width(window_width);
        let compact_header =
            content_width < crate::design_tokens::HOME_COMPACT_HEADER_CONTENT;

        // HOME-01: opacity of home menu/action card backgrounds over the
        // home background image (1.0 = fully opaque; lower = translucent).
        let home_menu_opacity = f32::from_bits(dep.home_menu_item_opacity_bits);

        // ── Connection state (single source of truth) ──
        let has_peer_connections = dep.has_peer_connections;
        let relay_reachable = dep.sender_ready || has_peer_connections;
        let mesh_health = dep.mesh_health.as_mesh_health();
        let variant =
            home_connection_variant(&mesh_health, has_peer_connections, relay_reachable);

        // ── Hero variant visuals (truthful, from the pure mapping above) ──
        let (hero_icon, hero_color, headline): (&[u8], fn(&iced::Theme) -> iced::Color, String) =
            match variant {
                HomeConnectionVariant::Starting => {
                    const RECONNECT_DOTS: [&str; 4] =
                        ["\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}"];
                    let dot = RECONNECT_DOTS
                        [(dep.main_screen_reconnect_frame as usize) % RECONNECT_DOTS.len()];
                    (ICON_RETRY, color_warning, format!("Starting Boru {dot}"))
                }
                HomeConnectionVariant::Connecting => (
                    ICON_RETRY,
                    color_warning,
                    "Connecting \u{2014} waiting for peers\u{2026}".to_string(),
                ),
                HomeConnectionVariant::Ready => (
                    ICON_CHECK,
                    accent_green,
                    "Boru is connected and ready.".to_string(),
                ),
                HomeConnectionVariant::Degraded => {
                    let reason = match &mesh_health {
                        MeshHealth::Degraded(r) => r.clone(),
                        _ => String::new(),
                    };
                    (
                        ICON_MESH,
                        color_warning,
                        format!("Mesh degraded \u{2014} {reason}"),
                    )
                }
                HomeConnectionVariant::Offline => {
                    let reason = match &mesh_health {
                        MeshHealth::Offline(r) => r.clone(),
                        _ => String::new(),
                    };
                    (
                        ICON_OFFLINE,
                        color_error,
                        format!("Boru is offline \u{2014} {reason}"),
                    )
                }
            };
        let show_retry = matches!(variant, HomeConnectionVariant::Offline);
        let show_details = matches!(
            variant,
            HomeConnectionVariant::Offline | HomeConnectionVariant::Degraded
        );
        // Compact status pill label for the page header (Figure 3).
        let pill_label = match variant {
            HomeConnectionVariant::Starting => "Starting",
            HomeConnectionVariant::Connecting => "Connecting",
            HomeConnectionVariant::Ready => "Connected",
            HomeConnectionVariant::Degraded => "Degraded",
            HomeConnectionVariant::Offline => "Offline",
        };

        // ── Greeting (page header) ──
        // UI-HOME-12: display_heading — Archivo SemiCondensed Bold 32 px,
        // 1.2 line height (via TypeRole::DisplayHeading).
        let display_name = if dep.local_label.is_empty() {
            "there"
        } else {
            &dep.local_label
        };
        let greeting = crate::fonts::type_role_text_lh(
            crate::fonts::TypeRole::DisplayHeading,
            format!("Good {}, {display_name}", dep.time_of_day_greeting),
            1.2,
        )
        .color(crate::design_tokens::text_primary(&theme))
        .width(Length::Fill)
        // A long display name (e.g. a bare peer key) must wrap inside the
        // header instead of overflowing it (UI-HOME-10).
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph);
        // Subtitle — IBM Plex Sans Regular at the UI-HOME-02 size token
        // (16 px; the canonical `body` role is 15 px, plan band 15–17 px).
        let welcome_line = crate::fonts::type_role_text(
            crate::fonts::TypeRole::Body,
            "Welcome to Boru",
        )
        .size(crate::fonts::HOME_SUBTITLE)
        .color(text_secondary(&theme))
        .width(Length::Fill);

        // ── Status pill (page header right, compact) ──
        // POLISH-05: reduced vertical padding from SPACE_12 to SPACE_10
        // (~36 px tall, in the UI-HOME-02 36–40 px band but now visually
        // closer to the Download Manager button at right).
        let status_pill = container(
            row![
                icon_svg(hero_icon, TYPO_SM).style(move |t, _| iced::widget::svg::Style {
                    color: Some(hero_color(t)),
                }),
                Space::new().width(Length::Fixed(SPACE_8)),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, pill_label)
                    .color(hero_color(&theme)),
            ]
            .spacing(0)
            .align_y(Alignment::Center),
        )
        .padding([SPACE_10, SPACE_12])
        .style(move |t| {
            let mut bg = bg_surface(t);
            bg.a *= home_menu_opacity;
            iced::widget::container::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: hero_color(t),
                    width: 1.0,
                    radius: SPACE_16.into(),
                },
                ..Default::default()
            }
        });

        // ── Large connection status card (new dark panel) ──
        // Rendered by the dedicated `status_card` module: dark green
        // gradient panel, outlined status indicator, two-tone heading,
        // privacy pill, and a native canvas peer-to-peer mesh on the
        // right. All connection-state inputs are the same live selectors
        // the previous hero card consumed (variant / headline / actions /
        // width / opacity) — only the presentation changed. The mesh
        // pulses very slowly while Ready and OS reduced-motion is off.
        //
        // CONN-02: the card's responsive tier must respond to the card's
        // REAL container width, not the window-derived dashboard width.
        // With the right rail open the card occupies FillPortion(2) of
        // (content_width − 24); with the rail stacked it spans the full
        // content width. iced has no container queries, so the width is
        // derived here from the same layout rules the grid below builds
        // (see design_tokens::status_card_content_width).
        let card_width = crate::design_tokens::status_card_content_width(content_width);
        let hero_card =
            crate::status_card::view_status_card(&crate::status_card::StatusCardDependency {
                variant,
                content_width: card_width,
                headline: headline.clone(),
                show_retry,
                show_details,
                pulse_frame: dep.hero_pulse_frame,
                animate_mesh: !dep.reduced_motion
                    && matches!(variant, HomeConnectionVariant::Ready),
                dimmed_mesh: !matches!(variant, HomeConnectionVariant::Ready),
                home_menu_opacity,
            });

        // ── Mesh Health card ──
        // UI-HOME-05: full dashboard card. Header carries a mesh glyph +
        // title + real status badge + the existing "View details" action.
        // Body shows the live status row, three real connection counts
        // (neighbors / direct / relayed), lobby state + connection duration
        // when available, and a short recent-events list fed from the same
        // bounded mesh event log the rest of the app uses — no invented
        // statistics. UI-28 keeps transient startup lines from lingering:
        // the watchdog clears "Starting up...", "Connecting to lobby...",
        // "Connected to lobby..." and "Subscribing to..." once the mesh is
        // Good, so the log stays truthful.
        let (health_label, health_color): (&str, fn(&iced::Theme) -> Color) =
            match &mesh_health {
                MeshHealth::Good => ("Healthy", accent_green),
                MeshHealth::Degraded(_) => ("Degraded", color_warning),
                MeshHealth::Offline(_) => ("Offline", color_error),
            };
        let mesh_has_peers = dep.has_peer_connections;
        let mesh_relay_reachable = dep.sender_ready || mesh_has_peers;
        let mesh_variant =
            home_connection_variant(&mesh_health, mesh_has_peers, mesh_relay_reachable);

        let (status_icon, status_color, status_label): (
            &[u8],
            fn(&iced::Theme) -> Color,
            String,
        ) = match mesh_variant {
            HomeConnectionVariant::Starting => {
                (ICON_RETRY, color_warning, "Starting up…".to_string())
            }
            HomeConnectionVariant::Connecting => (
                ICON_RETRY,
                color_warning,
                "Connecting — waiting for peers…".to_string(),
            ),
            HomeConnectionVariant::Ready => (ICON_CHECK, accent_green, "Connected".to_string()),
            HomeConnectionVariant::Degraded => {
                let reason = match &mesh_health {
                    MeshHealth::Degraded(r) => r.clone(),
                    _ => String::new(),
                };
                (ICON_MESH, color_warning, format!("Degraded — {reason}"))
            }
            HomeConnectionVariant::Offline => {
                let reason = match &mesh_health {
                    MeshHealth::Offline(r) => r.clone(),
                    _ => String::new(),
                };
                (ICON_OFFLINE, color_error, format!("Offline — {reason}"))
            }
        };

        // Secondary line: current peer counts, plus connection time once the
        // mesh is healthy (mesh_connected_at is maintained by the watchdog).
        let status_detail = match mesh_variant {
            HomeConnectionVariant::Starting => "Establishing the mesh…".to_string(),
            HomeConnectionVariant::Connecting => {
                "Waiting for peers to join the mesh".to_string()
            }
            _ => {
                let mut parts = vec![format!(
                    "{} direct · {} relayed · {} neighbors",
                    dep.direct_peers,
                    dep.relayed_peers,
                    dep.neighbors_len,
                )];
                if let Some(secs) = dep.connected_age_secs {
                    let duration = if secs < 60 {
                        format!("connected {secs}s")
                    } else if secs < 3600 {
                        format!("connected {}m {}s", secs / 60, secs % 60)
                    } else {
                        format!("connected {}h {}m", secs / 3600, (secs % 3600) / 60)
                    };
                    parts.push(duration);
                }
                parts.join("  ·  ")
            }
        };

        // Status pill in the header reports the mesh health state using the
        // same palette as the footer strip below the dashboard.
        let mesh_badge_kind = match &mesh_health {
            MeshHealth::Good => StatusBadgeKind::Success,
            MeshHealth::Degraded(_) => StatusBadgeKind::Warning,
            MeshHealth::Offline(_) => StatusBadgeKind::Danger,
        };

        // Body: status icon + label + detail (content-driven — grows with
        // the status detail text instead of clipping).
        let mesh_status_row = Row::new()
            .push(icon_svg(status_icon, TYPO_MD).style(move |t, _| {
                iced::widget::svg::Style {
                    color: Some(status_color(t)),
                }
            }))
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(
                Column::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::BodyEmphasised,
                            status_label.clone(),
                        )
                        .color(status_color(&theme))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            status_detail,
                        )
                        .color(text_muted(&theme))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                    )
                    .width(Length::Fill),
            )
            .spacing(0)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        // Live connection-count stat blocks (neighbors / direct / relayed).
        // Values come straight from the dependency snapshot — no fabricated
        // numbers; three labelled tiles make the real counts readable at a
        // glance instead of hiding them inside a sentence.
        let mesh_stat_block = |label: &'static str, value: u32| -> iced::Element<'static, AppMessage> {
            let tile = Column::new()
                .push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::BodyEmphasised,
                        value.to_string(),
                    )
                    .color(crate::design_tokens::text_primary(&theme)),
                )
                .push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        label,
                    )
                    .color(text_muted(&theme)),
                )
                .spacing(SPACE_2)
                .align_x(Alignment::Center)
                .width(Length::Fill);
            container(tile)
                .padding([SPACE_6, SPACE_4])
                .width(Length::Fill)
                .style(move |t| container::Style {
                    background: Some(iced::Background::Color(
                        crate::design_tokens::surface_hover(t),
                    )),
                    border: iced::Border {
                        radius: crate::design_tokens::RADIUS_SM.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        };
        let mesh_stats_row = Row::new()
            .push(mesh_stat_block("Neighbors", dep.neighbors_len))
            // UI-HOME-09: stat-tile gaps on the shared scale (SPACE_8; was
            // SPACE_6, off the scale).
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(mesh_stat_block("Direct", dep.direct_peers))
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(mesh_stat_block("Relayed", dep.relayed_peers))
            .spacing(0)
            .width(Length::Fill);

        // Lobby state + connection duration line. Lobby state derives from
        // the real gossip sender: when the sender subscription is up the app
        // is in the lobby, otherwise it is still joining. Duration is shown
        // only when the watchdog has a connected timestamp.
        let lobby_label = if dep.sender_ready {
            "Lobby: connected".to_string()
        } else {
            "Lobby: connecting…".to_string()
        };
        let mut lobby_parts = vec![lobby_label];
        if let Some(secs) = dep.connected_age_secs {
            let duration = if secs < 60 {
                format!("connected {secs}s")
            } else if secs < 3600 {
                format!("connected {}m {}s", secs / 60, secs % 60)
            } else {
                format!("connected {}h {}m", secs / 3600, (secs % 3600) / 60)
            };
            lobby_parts.push(duration);
        }
        let mesh_lobby_row = Row::new()
            .push(icon_svg(ICON_MESH, TYPO_XS).style(move |t, _| {
                iced::widget::svg::Style {
                    color: Some(text_muted(t)),
                }
            }))
            .push(Space::new().width(Length::Fixed(SPACE_6)))
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    lobby_parts.join("  ·  "),
                )
                .color(text_muted(&theme))
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            )
            .spacing(0)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        // Recent mesh events — a short, readable list from the real bounded
        // log. Each row: small status icon + description + relative time.
        // `dep.mesh_events` is already newest-first and capped at 4 in the
        // dependency selector, so the card stays content-driven (no fixed
        // heights, no hidden overflow).
        let events_header = crate::fonts::type_role_text(
            crate::fonts::TypeRole::Metadata,
            "Recent events".to_uppercase(),
        )
        .color(text_muted(&theme));
        let event_rows: Vec<iced::Element<'static, AppMessage>> = dep
            .mesh_events
            .iter()
            .map(|event| {
                let tone = mesh_event_tone(&event.message);
                let (event_icon, event_color) = mesh_event_visual(tone);
                let ago =
                    crate::presentation::relative_age_secs(event.age_secs, 10);
                container(
                    Row::new()
                        .push(icon_svg(event_icon, TYPO_SM).style(move |t, _| {
                            iced::widget::svg::Style {
                                color: Some(event_color(t)),
                            }
                        }))
                        .push(Space::new().width(Length::Fixed(SPACE_6)))
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                event.message.clone(),
                            )
                            .color(text_system(&theme))
                            .width(Length::Fill)
                            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                        )
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                ago,
                            )
                            .color(text_muted(&theme)),
                        )
                        .spacing(SPACE_6)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([SPACE_2, 0.0])
                .into()
            })
            .collect();
        let events_body: iced::Element<'static, AppMessage> = if event_rows.is_empty() {
            // Intentional no-events state (UI-HOME-16): keep the connection
            // summary above and explain that there are no recent mesh events
            // with the same small-icon + muted-text treatment the rail cards
            // use. The text wraps at narrow content widths.
            Row::new()
                .push(icon_svg(ICON_ACTIVITY, TYPO_SM).style(move |t, _| {
                    iced::widget::svg::Style {
                        color: Some(text_muted(t)),
                    }
                }))
                .push(Space::new().width(Length::Fixed(SPACE_8)))
                .push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        MESH_EVENTS_EMPTY_MESSAGE,
                    )
                    .color(text_muted(&theme))
                    .width(Length::Fill)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                )
                .spacing(0)
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .into()
        } else {
            Column::with_children(event_rows)
                .spacing(SPACE_2)
                .width(Length::Fill)
                .into()
        };

        let mesh_body = Column::new()
            .push(mesh_status_row)
            // UI-HOME-09: structural body gaps on the shared scale (SPACE_12
            // between blocks; was SPACE_10, off the scale).
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(mesh_stats_row)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(mesh_lobby_row)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(crate::ui_components::divider())
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(events_header)
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(events_body)
            .spacing(0)
            .width(Length::Fill);

        let mesh_card = CardShell::new("Mesh Health", vec![])
            .title_case(false)
            .header_icon(icon_svg(ICON_MESH, TYPO_MD).style(move |t, _| {
                iced::widget::svg::Style {
                    color: Some(health_color(t)),
                }
            }).into())
            .subtitle("Current connection status")
            .status_badge(health_label, mesh_badge_kind)
            .header_action("View details", AppMessage::OpenConnectionDetails)
            .compact_header(compact_header)
            .body(mesh_body.into())
            .background_opacity(home_menu_opacity)
            .build(&theme);

        // ── Quick actions: four equal, full-card targets (Figure 3) ──
        let action_grid =
            crate::quick_actions::quick_action_grid(content_width, &theme, home_menu_opacity);

        // DLMGR-01: home entry point — a compact outline button beside the
        // status pill opens the Download Manager (all active transfers in
        // both directions). Static renderer: no dependency data needed, just
        // a message dispatch.
        let download_manager_btn = button(
            Row::new()
                .push(
                    Icon::Download
                        .build()
                        .size(crate::icon_system::IconSize::Xs)
                        .color_fn(crate::design_tokens::text_muted)
                        .build(),
                )
                .push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Download Manager",
                    ),
                )
                .spacing(SPACE_4)
                .align_y(Alignment::Center),
        )
        .on_press(AppMessage::OpenDownloadManager)
        .padding([SPACE_6, SPACE_12])
        .style(BUTTON_OUTLINE);

        // ── Right rail: loading treatment decision (t_0441a1dc) ──
        // No skeleton/shimmer loading is used for the three rail cards, by
        // design. Every data source here is synchronously available at first
        // render: Online Peers reads `self.friends` plus the presence map
        // seeded from persisted friend status during IcedChat::new; Recent
        // Activity reads the in-memory ring buffer; Tunnels reads
        // TunnelService::list_tunnels() (a synchronous RwLock read of the
        // live in-memory registry). There is no mount-time fetch of any of
        // these. The only real asynchronous startup window (endpoint, DHT,
        // protocol handlers, friend load) runs before the Iced window opens
        // and is covered by the native splash window in main.rs; later
        // presence/activity/tunnel updates arrive as event-driven messages
        // that redraw these cards synchronously. A skeleton would therefore
        // only appear by faking an async delay, which the task explicitly
        // forbids — so rows render real data immediately and fill in
        // progressively (e.g. profile images arrive async and replace the
        // initials fallback when downloaded). Full rationale in
        // docs/ui-redesign/evidence/ui-skeletons/README.md.

        // ── Right column: Online Peers / Recent Activity / Tunnels ──
        // Each card is built by a fine-grained selector (see the selector
        // methods above) and wrapped in `iced::widget::lazy`. The lazy widget
        // compares the fresh selector value — a PartialEq snapshot of exactly
        // that card's state slice — with the previous frame and reuses the
        // already-built subtree when nothing in the slice changed. A peer
        // presence change therefore rebuilds only the Online Peers card, an
        // activity push only Recent Activity, and a tunnel status change only
        // Tunnels. The per-second ActivityTick bumps `activity_tick`, which
        // the Recent Activity and Tunnels dependencies include (fresh relative
        // timestamps / truthful expiry flips) but the Online Peers dependency
        // deliberately excludes — the peers card never rebuilds on an idle
        // tick.
        let online_card =
            iced::widget::lazy(dep.online.clone(), Self::view_online_peers_card);
        let activity_card =
            iced::widget::lazy(dep.activity.clone(), Self::view_recent_activity_card);
        let tunnels_card = iced::widget::lazy(dep.tunnels.clone(), Self::view_tunnels_card);

        // Right rail: 20 px vertical card gaps (UI-HOME-02: 20–24 px).
        let right_col = Column::new()
            .push(online_card)
            .push(Space::new().height(Length::Fixed(SPACE_20)))
            .push(activity_card)
            .push(Space::new().height(Length::Fixed(SPACE_20)))
            .push(tunnels_card)
            .spacing(0)
            .width(Length::Fill);

        // ── Page header: greeting + welcome + status pill (Figure 3) ──
        // UI-HOME-15: on narrow content the pill stacks under the greeting
        // (left-aligned) instead of squeezing it; on wider content it keeps
        // the approved top-right position.
        let page_header: iced::Element<'static, AppMessage> = if compact_header {
            Column::new()
                .push(
                    Column::new()
                        .push(greeting)
                        // Greeting → welcome gap. UI-HOME-09: shared-scale
                        // SPACE_4 (was SPACE_2, off the scale).
                        .push(Space::new().height(Length::Fixed(SPACE_4)))
                        .push(welcome_line)
                        .spacing(0)
                        .width(Length::Fill),
                )
                .push(Space::new().height(Length::Fixed(SPACE_12)))
                .push(
                    Row::new()
                        .push(status_pill)
                        .push(Space::new().width(Length::Fixed(SPACE_8)))
                        .push(download_manager_btn)
                        .spacing(0)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                )
                .spacing(0)
                .width(Length::Fill)
                .into()
        } else {
            row![
                Column::new()
                    .push(greeting)
                    // Greeting → welcome gap. UI-HOME-09: shared-scale SPACE_4
                    // (was SPACE_2, off the scale).
                    .push(Space::new().height(Length::Fixed(SPACE_4)))
                    .push(welcome_line)
                    .spacing(0)
                    .width(Length::Fill),
                status_pill,
                Space::new().width(Length::Fixed(SPACE_8)),
                download_manager_btn,
            ]
            .spacing(SPACE_8)
            .align_y(Alignment::Center)
            .into()
        };

        // ── Main content: hero + mesh + actions left, activity rail right ──
        // Two-thirds content + one-third activity rail (plan §4): main
        // ~66.7% / right ~33.3% with a 24 px column gap. Below the stack
        // breakpoint the rail collapses BELOW the left column instead of
        // compressing its cards. UI-HOME-15: the breakpoint is content-width
        // based (window minus sidebar/divider/padding), so the fixed 288 px
        // sidebar never forces an early stack.
        let rail_stacked =
            content_width < crate::design_tokens::HOME_TWO_COL_CONTENT;
        let card_gap = SPACE_20; // 20 px vertical card gap (plan: 20–24 px)

        let left_col = Column::new()
            .push(hero_card)
            .push(Space::new().height(Length::Fixed(card_gap)))
            .push(mesh_card)
            .push(Space::new().height(Length::Fixed(card_gap)))
            .push(action_grid)
            .spacing(0)
            .width(Length::Fill);

        let main_content: iced::Element<'_, AppMessage> = if rail_stacked {
            // Narrow: left-column cards first, then the activity rail below.
            Column::new()
                .push(left_col)
                .push(Space::new().height(Length::Fixed(card_gap)))
                .push(right_col)
                .spacing(0)
                .width(Length::Fill)
                .into()
        } else {
            // Wide: two-column dashboard grid, both columns aligned top.
            // CONN-10 (spec §14 — no parent layout stretching): the left
            // column wrapper is explicitly Shrink-height so the hero card
            // can never be stretched by the (taller) right rail. The Row's
            // `align_y(Start)` is the `align-self: start` equivalent — iced
            // never resizes a Shrink-height child to match a sibling, so
            // opening the rail cannot force the status card taller. The
            // card's own container in status_card.rs pins the same guard.
            Row::new()
                .push(
                    container(left_col)
                        .width(Length::FillPortion(2))
                        .height(Length::Shrink),
                )
                .push(Space::new().width(Length::Fixed(SPACE_24)))
                .push(
                    container(right_col)
                        .width(Length::FillPortion(1))
                        .height(Length::Shrink),
                )
                .spacing(0)
                .align_y(Alignment::Start)
                .width(Length::Fill)
                .into()
        };

        // ── Connection footer: one truthful, compact status strip ──
        // Encryption status is derived from actual connection state: iroh
        // always transports over QUIC (encrypted), so we report "QUIC encrypted"
        // only when a peer connection exists, avoiding a blanket E2E claim.
        let encryption_label = if dep.direct_peers > 0 || dep.relayed_peers > 0 {
            "QUIC encrypted"
        } else {
            "Idle"
        };
        let footer = connection_footer(
            health_label,
            health_color,
            dep.direct_peers as usize,
            dep.relayed_peers as usize,
            dep.neighbors_len as usize,
            encryption_label,
        );

        // ── Assemble: centred dashboard canvas with responsive padding ──
        // Horizontal 32 px at large widths, 28 px elsewhere; top 28 px below
        // the application header; bottom at least 32 px (UI-HOME-02 plan).
        let h_padding = if crate::design_tokens::is_large(window_width) {
            SPACE_32
        } else {
            SPACE_28
        };

        // POLISH-05: page header → dashboard gap bumped from SPACE_28 to
        // ~40 px — roughly 12 px more breathing room between the
        // \"Welcome to Boru\" subtitle and the card grid.
        let col = Column::new()
            .push(page_header)
            .push(Space::new().height(Length::Fixed(SPACE_28 + SPACE_12)))
            .push(main_content)
            .push(Space::new().height(Length::Fixed(SPACE_16)))
            .push(footer)
            .spacing(0)
            .width(Length::Fill);

        // Cap the dashboard width (~1480 px) and centre it in the available
        // content region; vertical page scrolling stays on gutter_scrollable.
        // The max-width only binds on very wide windows (e.g. 1920), where it
        // keeps the grid from stretching edge-to-edge.
        let canvas = container(
            container(col)
                .padding(iced::Padding::from([SPACE_28, h_padding]).bottom(SPACE_32))
                .width(Length::Fill)
                .max_width(crate::design_tokens::DASHBOARD_MAX_WIDTH)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .height(Length::Fill);

        crate::ui_components::gutter_scrollable(canvas)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
    /// State-layer update for the home/chat-list surface (BORU-AUDIT-22 spec step 5).
    ///
    /// Handles the chat-list join-ticket input. The root `update()`
    /// dispatches these variants here via combined match arms.
    pub(crate) fn update_home(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::JoinTicketInputChanged(text) => {
                self.join_ticket_input = text;
                if !self.chat_list_error.is_empty() {
                    self.chat_list_error.clear();
                }
                iced::Task::none()
            }
            // update() only dispatches the home variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
