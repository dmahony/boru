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
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
    /// BORU-LAYOUT-03: bumps whenever the live layout is replaced so
    /// iced::lazy cannot retain a home subtree built with the previous
    /// LayoutConfig (the home layout itself is captured separately by the
    /// renderer's closure and re-read when this revision changes).
    pub(crate) layout_revision: u64,
    #[cfg(feature = "dev-ui")]
    pub(crate) drag_placeholder: Option<(crate::designer::ComponentId, usize)>,
    /// Designer interaction state is part of the lazy key so enabling
    /// Designer Mode or moving the pointer rebuilds the production Home
    /// surface instead of leaving a cached, stale overlay tree in place.
    #[cfg(feature = "dev-ui")]
    pub(crate) designer_enabled: bool,
    #[cfg(feature = "dev-ui")]
    pub(crate) designer_hovered: Option<crate::designer::ComponentId>,
    #[cfg(feature = "dev-ui")]
    pub(crate) designer_selected: Option<crate::designer::ComponentId>,
    pub(crate) window_width_bits: u32,
    pub(crate) window_height_bits: u32,
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
    pub(crate) people_activity: PeopleActivityCardData,
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
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
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

/// Combined dependency for the People & Activity card: online peers +
/// recent activity in one coherent right-rail card (BORU-HOME-05).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct PeopleActivityCardData {
    pub(crate) online: OnlinePeersCardData,
    pub(crate) activity: RecentActivityCardData,
}

/// Max visible peer rows in the People & Activity combined card (BORU-HOME-05).
/// The combined card shows at most 3 peers; extra peers are accessible via
/// the "View all" header action.
const PEOPLE_PEERS_MAX: usize = 3;

/// Max visible activity rows in the People & Activity combined card (BORU-HOME-05).
/// Rendered inline beneath the peers section with a restrained divider.
const PEOPLE_ACTIVITY_MAX: usize = 4;

/// Minimum Online Peers body height (px). A single 60 px peer row is
/// floored to this so the card keeps a sensible ~220–280 px footprint
/// instead of collapsing into a strip; short lists never stretch it.
///
/// BORU-UI-03: mirrored by `HomeTheme::peers_body_min` (128 px) in the typed
/// theme — `theme.rs`'s `default_matches_audit_source_values` test pins the
/// two sources equal so they cannot drift.
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
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
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
pub(crate) fn online_peers_empty_message() -> String {
    crate::i18n::t("home.online_peers_empty")
}

/// Empty-state copy for the Recent Activity rail card (UI-HOME-16 spec copy).
pub(crate) fn recent_activity_empty_message() -> String {
    crate::i18n::t("home.recent_activity_empty")
}

/// Empty-state copy for the Tunnels rail card (UI-HOME-08 spec copy).
pub(crate) fn tunnels_empty_message() -> String {
    crate::i18n::t("home.tunnels_empty")
}

/// Empty-state copy for the Recent events section of the Mesh Health card
/// (UI-HOME-16: retain the connection summary above, explain the empty feed).
pub(crate) fn mesh_events_empty_message() -> String {
    crate::i18n::t("home.mesh_events_empty")
}

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
    /// BORU-UI-07: bumps whenever the live theme is replaced so iced::lazy
    /// cannot retain a subtree built with the previous theme.
    pub(crate) theme_revision: u64,
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
            theme_revision: self.theme_revision,
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
            theme_revision: self.theme_revision,
            tick: self.activity_tick,
            total: self.recent_activity.len(),
            rows,
            compact_header: self.home_compact_headers(),
            home_menu_item_opacity_bits: self.home_menu_item_opacity.to_bits(),
        }
    }

    /// Combined selector for the People & Activity card (BORU-HOME-05).
    /// Merges online peers + recent activity into one data dependency so the
    /// merged right-rail card can be cached by `iced::widget::lazy`.
    pub(crate) fn people_activity_card_data(&self) -> PeopleActivityCardData {
        PeopleActivityCardData {
            online: self.online_peers_card_data(),
            activity: self.recent_activity_card_data(),
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
            theme_revision: self.theme_revision,
            tick: self.activity_tick,
            rows,
            compact_header: self.home_compact_headers(),
            home_menu_item_opacity_bits: self.home_menu_item_opacity.to_bits(),
        }
    }

    /// True when the home content width is narrow enough that card headers
    /// switch to the two-line compact layout (UI-HOME-15).
    pub(crate) fn home_compact_headers(&self) -> bool {
        self.active_layout.home_content_width(self.window_width)
            < self.active_layout.responsive.home_compact_header_content
    }

    /// Build the Online Peers card subtree. Runs inside `iced::widget::lazy`,
    /// so it is only re-invoked when `OnlinePeersCardData` actually changes.
    pub(crate) fn view_online_peers_card(
        dep: &OnlinePeersCardData,
        btheme: crate::theme::BoruTheme,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        let peer_rows: Vec<iced::Element<'static, AppMessage>> = dep
            .rows
            .iter()
            .map(|row| {
                let mut avatar = Avatar::new(row.name.clone())
                    .size(btheme.avatars.chat_list)
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
                        .color(btheme.colors.text_secondary)
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
                    .spacing(btheme.spacing.space_2)
                    .align_x(Alignment::Start)
                    .width(Length::Fill);
                let row_el = Row::new()
                    // Zero-width spacer enforces the 60 px two-line row
                    // rhythm as a MINIMUM; a wrapped display name grows the
                    // row instead of being clipped (UI-HOME-10).
                    .push(
                        Space::new()
                            .width(Length::Fixed(0.0))
                            .height(Length::Fixed(btheme.lists.peer_row_height)),
                    )
                    .push(avatar.build())
                    .push(Space::new().width(Length::Fixed(btheme.spacing.space_8)))
                    .push(text_col)
                    .spacing(0)
                    .align_y(Alignment::Center);
                button(row_el)
                    .on_press(AppMessage::OpenConversation(row.pk))
                    .width(Length::Fill)
                    .padding([0.0, btheme.spacing.space_8])
                    .style(|t, status| iced::widget::button::Style {
                        // Three-tier interaction ramp (BORU-HOME-10):
                        // default (transparent) → hover → pressed.
                        // Note: iced 0.14 `button::Status` has no `Focused`
                        // variant and buttons are not keyboard-focusable
                        // in this version, so hover/pressed are the
                        // primary pointer affordances.
                        background: match status {
                            iced::widget::button::Status::Pressed => Some(iced::Background::Color(
                                crate::design_tokens::surface_pressed(t),
                            )),
                            iced::widget::button::Status::Hovered => Some(iced::Background::Color(
                                crate::design_tokens::surface_hover(t),
                            )),
                            _ => None,
                        },
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
                            online_peers_empty_message(),
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
            .height(Length::Fixed(btheme.home.peers_body_min))
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
                btheme,
            )))
            .width(Length::Fill)
            .into()
        };

        crate::card_shell::CardShell::new(crate::i18n::t("home.online_peers"), vec![])
            .count(dep.rows.len())
            .count_total(dep.total_friends)
            .on_view_all(AppMessage::OpenFriendRequests)
            .compact_header(dep.compact_header)
            .body(body)
            .background_opacity(f32::from_bits(dep.home_menu_item_opacity_bits))
            .card_radius(btheme.radii.card)
            .build(&theme)
    }

    /// Content-driven height of the Online Peers body (px): the shorter of
    /// the row content and the five-visible-rows cap, floored at
    /// [`PEERS_BODY_MIN`] so a one-peer card stays intentional. BORU-UI-07
    /// reads geometry from the LIVE merged theme so a boru-ui.toml reload
    /// adjusts the card height without a rebuild.
    pub(crate) fn online_peers_body_height(rows: usize, btheme: crate::theme::BoruTheme) -> f32 {
        if rows == 0 {
            return btheme.home.peers_body_min;
        }
        let content = rows as f32 * btheme.lists.peer_row_height
            + (rows as f32 - 1.0) * btheme.spacing.space_2;
        content
            .min(5.0 * btheme.lists.peer_row_height + 4.0 * btheme.spacing.space_2)
            .max(btheme.home.peers_body_min)
    }

    /// Build the Recent Activity card subtree (memoized via lazy).
    pub(crate) fn view_recent_activity_card(
        dep: &RecentActivityCardData,
        btheme: crate::theme::BoruTheme,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{container, row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        // UI-29: recent activity rows are denser than the 48 px peer rows —
        // a compact 32 px row keeps the feed scannable without dead vertical
        // space around the small icon + single-line title (BORU-UI-03: the
        // row height now comes from `HomeTheme::activity_row_height`).
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
                // long descriptions are truncated to ~75 chars (roughly two
                // lines at typical card width) with file-extension preservation
                // so filenames stay identifiable. Wrapped overflow is still
                // allowed for slightly-longer-but-still-reasonable text.
                let description =
                    crate::presentation::truncate_activity_description(&event.description, 75);
                container(
                    row![
                            Space::new()
                                .width(Length::Fixed(0.0))
                                .height(Length::Fixed(btheme.home.activity_row_height)),
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
                                    description,
                                )
                                .color(text_system(&theme))
                                .width(Length::Fill)
                                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                            )
                            .width(Length::Fill),
                            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, ago,)
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

        CardShell::new(crate::i18n::t("home.recent_activity"), activity_rows)
            .count(dep.total)
            .empty_icon(
                icon_svg(ICON_ACTIVITY, TYPO_SM)
                    .style(move |t, _| iced::widget::svg::Style {
                        color: Some(text_muted(t)),
                    })
                    .into(),
            )
            .empty_message(recent_activity_empty_message())
            .compact_header(dep.compact_header)
            .max_height(180.0)
            .background_opacity(f32::from_bits(dep.home_menu_item_opacity_bits))
            .card_radius(btheme.radii.card)
            .build(&theme)
    }

    /// Build the combined People & Activity card (BORU-HOME-05).
    /// Merges online peers + recent activity into one coherent right-rail card.
    /// The peers section shows up to [`PEOPLE_PEERS_MAX`] online friends with
    /// avatar + name + presence; a restrained divider separates it from the
    /// recent activity feed (up to [`PEOPLE_ACTIVITY_MAX`] rows).
    /// BORU-LAYOUT-03: card-sizing constraints (peers body min height,
    /// activity row height, empty-activity height) come from the layout model.
    pub(crate) fn view_people_activity_card(
        dep: &PeopleActivityCardData,
        btheme: crate::theme::BoruTheme,
        layout: crate::layout::HomeLayout,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.online.dark_mode);

        // ── Peers section ──
        let peers_body: iced::Element<'static, AppMessage> = if dep.online.rows.is_empty() {
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
                            online_peers_empty_message(),
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
            .height(Length::Fixed(layout.card_sizing.peers_body_min))
            .align_y(Alignment::Center)
            .into()
        } else {
            let peer_rows: Vec<iced::Element<'static, AppMessage>> = dep
                .online
                .rows
                .iter()
                .take(PEOPLE_PEERS_MAX)
                .map(|row| {
                    let mut avatar = Avatar::new(row.name.clone())
                        .size(crate::design_tokens::AVATAR_CHAT_LIST)
                        .dark_mode(dep.online.dark_mode)
                        .online_dot(true)
                        .fallback_icon(Icon::Friend);
                    if let Some(handle) = row.avatar.handle.clone() {
                        avatar = avatar.image(handle);
                    }
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
                        .push(
                            Space::new()
                                .width(Length::Fixed(0.0))
                                .height(Length::Fixed(crate::card_shell::PEER_ROW_HEIGHT)),
                        )
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
                            background: match status {
                                iced::widget::button::Status::Pressed => {
                                    Some(iced::Background::Color(
                                        crate::design_tokens::surface_pressed(t),
                                    ))
                                }
                                iced::widget::button::Status::Hovered => Some(
                                    iced::Background::Color(crate::design_tokens::surface_hover(t)),
                                ),
                                _ => None,
                            },
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
            let row_count = dep.online.rows.len().min(PEOPLE_PEERS_MAX);
            let body_height = if row_count == 0 {
                layout.card_sizing.peers_body_min
            } else {
                let content = row_count as f32 * btheme.lists.peer_row_height
                    + (row_count as f32 - 1.0) * btheme.spacing.space_2;
                content.max(layout.card_sizing.peers_body_min)
            };
            Column::with_children(peer_rows)
                .spacing(SPACE_2)
                .width(Length::Fill)
                .height(Length::Fixed(body_height))
                .into()
        };

        // ── Divider ──
        let divider = container(Space::new().width(Length::Fill).height(Length::Fixed(
            crate::theme::BoruTheme::for_theme(&theme).borders.hairline,
        )))
        .style(move |t: &iced::Theme| container::Style {
            background: Some(iced::Background::Color(crate::design_tokens::border_muted(
                t,
            ))),
            ..container::Style::default()
        })
        .width(Length::Fill);

        // ── Activity section ──
        let activity_body: iced::Element<'static, AppMessage> = if dep.activity.rows.is_empty() {
            container(
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
                            recent_activity_empty_message(),
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
            .height(Length::Fixed(layout.gaps.hero_gap))
            .align_y(Alignment::Center)
            .into()
        } else {
            let activity_rows: Vec<iced::Element<'static, AppMessage>> = dep
                .activity
                .rows
                .iter()
                .take(PEOPLE_ACTIVITY_MAX)
                .map(|event| {
                    let ago = crate::presentation::relative_time_from_system(event.timestamp);
                    let activity_icon = match event.kind {
                        ActivityKind::Online => ICON_ONLINE,
                        ActivityKind::Offline => ICON_OFFLINE,
                        ActivityKind::FileShared => ICON_FILES,
                        ActivityKind::Message => ICON_CHAT,
                        ActivityKind::Generic => ICON_ACTIVITY,
                    };
                    let kind = event.kind;
                    let description =
                        crate::presentation::truncate_activity_description(&event.description, 75);
                    container(
                        Row::new()
                            .push(
                                Space::new()
                                    .width(Length::Fixed(0.0))
                                    .height(Length::Fixed(layout.card_sizing.activity_row_height)),
                            )
                            .push(icon_svg(activity_icon, TYPO_SM).style(move |t, _| {
                                iced::widget::svg::Style {
                                    color: Some(if kind == ActivityKind::Online {
                                        accent_green(t)
                                    } else {
                                        text_muted(t)
                                    }),
                                }
                            }))
                            .push(Space::new().width(Length::Fixed(SPACE_6)))
                            .push(
                                container(
                                    crate::fonts::type_role_text(
                                        crate::fonts::TypeRole::Body,
                                        description,
                                    )
                                    .color(text_system(&theme))
                                    .width(Length::Fill)
                                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, ago)
                                    .color(text_muted(&theme)),
                            )
                            .spacing(0)
                            .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([0.0, SPACE_8])
                    .align_y(Alignment::Center)
                    .into()
                })
                .collect();
            Column::with_children(activity_rows)
                .spacing(SPACE_2)
                .width(Length::Fill)
                .into()
        };

        // ── Assemble body ──
        // BORU-UI-09: the Recent Activity feed slice is an optional visual
        // feature (`HomeTheme::show_activity_feed`, toggled from the dev UI
        // Inspector). When disabled the People & Activity card shows only the
        // Online Peers section — the baseline UI keeps the feed.
        let body = if btheme.home.show_activity_feed {
            Column::new()
                .push(peers_body)
                .push(Space::new().height(Length::Fixed(SPACE_8)))
                .push(divider)
                .push(Space::new().height(Length::Fixed(SPACE_8)))
                .push(activity_body)
        } else {
            Column::new().push(peers_body)
        }
        .spacing(0)
        .width(Length::Fill);

        CardShell::new("People & Activity", vec![])
            .title_case(false)
            .on_view_all(AppMessage::OpenFriendRequests)
            .count(dep.online.rows.len())
            .count_total(dep.online.total_friends)
            .compact_header(dep.online.compact_header)
            .body(body.into())
            .background_opacity(f32::from_bits(dep.online.home_menu_item_opacity_bits))
            .card_radius(btheme.radii.card)
            .build(&theme)
    }

    /// Build the Tunnels card subtree (memoized via lazy).
    pub(crate) fn view_tunnels_card(
        dep: &TunnelsCardData,
        btheme: crate::theme::BoruTheme,
    ) -> iced::Element<'static, AppMessage> {
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
                        crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, status,)
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
            crate::i18n::t("tunnels.create_action")
        } else {
            crate::i18n::t("common.view_all")
        };

        let mut shell =
            crate::card_shell::CardShell::new(crate::i18n::t("home.tunnels"), tunnel_rows)
                .count(dep.rows.len())
                .header_action(header_action_label, AppMessage::ShowCreateTunnelDialog)
                .empty_icon(
                    icon_svg(ICON_LOCK, TYPO_SM)
                        .style(move |t, _| iced::widget::svg::Style {
                            color: Some(text_muted(t)),
                        })
                        .into(),
                )
                .empty_message(tunnels_empty_message())
                .compact_header(dep.compact_header)
                .card_radius(btheme.radii.card)
                .background_opacity(f32::from_bits(dep.home_menu_item_opacity_bits));

        // BORU-HOME-06: when tunnels exist, size the list body to fit all
        // rows naturally instead of capping at a fixed 120 px (which
        // clipped after ~2 rows). When the list is empty the CardShell
        // empty-state path renders compactly without a fixed list height.
        if !dep.rows.is_empty() {
            let row_count = dep.rows.len() as f32;
            let natural_height = row_count * crate::card_shell::CARD_ROW_HEIGHT
                + (row_count - 1.0) * crate::design_tokens::SPACE_2;
            shell = shell.max_height(natural_height);
        }

        shell.build(&theme)
    }

    /// Header-action label for the Tunnels card: "Create tunnel" when the
    /// list is empty (the dialog the empty copy points at), "View all"
    /// once live tunnels exist. The destination is the same in both cases
    /// (`ShowCreateTunnelDialog`).
    pub(crate) fn tunnels_header_action_label(rows: usize) -> String {
        if rows == 0 {
            crate::i18n::t("tunnels.create_action")
        } else {
            crate::i18n::t("common.view_all")
        }
    }

    // ── Main panel (empty state — landing screen) ─────────────────────

    /// Landing screen shown when no conversation is selected.
    /// Redesigned: connection status first, then actions, then activity.
    pub(crate) fn view_main_empty_state(&self) -> iced::Element<'_, AppMessage> {
        let dep = self.chat_list_dependency();
        let btheme = self.boru_theme();
        // BORU-LAYOUT-03: the live home layout is captured alongside the
        // dependency. `layout_revision` in the lazy key forces a rebuild
        // when the layout is replaced, so the closure re-reads the NEW
        // layout (mirror of how `theme_revision` + `btheme` interact).
        let home_layout = self.boru_layout().home.clone();
        let sidebar_layout = self.boru_layout().sidebar.clone();
        // BORU-LAYOUT-04: the responsive tier (thresholds + per-tier home
        // column counts / padding) is captured the same way so the static
        // renderer can resolve the active breakpoint from the window width.
        let responsive = self.boru_layout().responsive;
        #[cfg(feature = "dev-ui")]
        let (designer_enabled, designer_hovered, designer_selected) = (
            self.settings_state.designer.enabled,
            self.settings_state.designer.hovered_component,
            self.settings_state.designer.selected_component,
        );

        iced::widget::lazy(dep, move |dep| {
            Self::view_chat_list_content(
                dep,
                btheme,
                home_layout.clone(),
                sidebar_layout.clone(),
                responsive,
                #[cfg(feature = "dev-ui")]
                designer_enabled,
                #[cfg(feature = "dev-ui")]
                designer_hovered,
                #[cfg(feature = "dev-ui")]
                designer_selected,
                #[cfg(feature = "dev-ui")]
                dep.drag_placeholder,
            )
        })
        .into()
    }

    /// Builds the ChatList (home / empty-state) screen's renderable snapshot.
    pub(crate) fn chat_list_dependency(&self) -> ChatListDependency {
        let has_peer_connections =
            !self.neighbors.is_empty() || self.relayed_peers > 0 || self.direct_peers > 0;
        let connected_age_secs = self
            .mesh_connected_at
            .map(|t| Instant::now().saturating_duration_since(t).as_secs());
        #[cfg(feature = "dev-ui")]
        let drag_placeholder = self.settings_state.designer.drag_operation.as_ref().and_then(|operation| {
            operation
                .proposed_index
                .map(|index| (operation.component, index))
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
        #[cfg(feature = "dev-ui")]
        let preview_width = if self.settings_state.designer.enabled {
            self.settings_state.designer
                .preview_breakpoint
                .width(self.settings_state.designer.custom_preview_width)
        } else {
            self.window_width
        };
        #[cfg(not(feature = "dev-ui"))]
        let preview_width = self.window_width;
        ChatListDependency {
            dark_mode: self.dark_mode,
            theme_revision: self.theme_revision,
            layout_revision: self.layout_revision,
            window_width_bits: (preview_width * 100.0) as u32,
            window_height_bits: (self.window_height * 100.0) as u32,
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
            people_activity: self.people_activity_card_data(),
            tunnels: self.tunnels_card_data(),
            home_menu_item_opacity_bits: self.home_menu_item_opacity.to_bits(),
            hero_pulse_frame: (self.activity_tick
                % crate::status_card::STATUS_CARD_PULSE_PHASES as u64)
                as u32,
            reduced_motion: self.reduced_motion,
            #[cfg(feature = "dev-ui")]
            drag_placeholder,
            #[cfg(feature = "dev-ui")]
            designer_enabled: self.settings_state.designer.enabled,
            #[cfg(feature = "dev-ui")]
            designer_hovered: self.settings_state.designer.hovered_component,
            #[cfg(feature = "dev-ui")]
            designer_selected: self.settings_state.designer.selected_component,
        }
    }

    /// Static renderer for the ChatList (home / empty-state) screen, driven by
    /// [`ChatListDependency`] so `iced::widget::lazy` can cache the whole
    /// screen while any of its rendered slices is unchanged. BORU-LAYOUT-03:
    /// the structural arrangement comes from the layout model (`home.*`); the
    /// defaults reproduce today's appearance exactly. BORU-LAYOUT-04: the
    /// responsive tier (thresholds + per-tier home columns / padding) comes
    /// from the `responsive.*` model group and is resolved from the window
    /// width, so narrow / desktop / ultra-wide windows apply different column
    /// counts — with the defaults reproducing the pre-responsive behaviour.
    pub(crate) fn view_chat_list_content(
        dep: &ChatListDependency,
        btheme: crate::theme::BoruTheme,
        layout: crate::layout::HomeLayout,
        sidebar: crate::layout::SidebarLayout,
        responsive: crate::layout::ResponsiveLayout,
        #[cfg(feature = "dev-ui")] designer_enabled: bool,
        #[cfg(feature = "dev-ui")] designer_hovered: Option<crate::designer::ComponentId>,
        #[cfg(feature = "dev-ui")] designer_selected: Option<crate::designer::ComponentId>,
        #[cfg(feature = "dev-ui")] drag_placeholder: Option<(crate::designer::ComponentId, usize)>,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, row, Column, Row, Space};
        use iced::{Alignment, Length};

        // Stable semantic anchors for the editable Home sections. These are
        // independent of layout order and widget allocation.
        #[cfg(feature = "dev-ui")]
        let _designer_components = (
            crate::designer::ComponentId::HomeWelcome,
            crate::designer::ComponentId::HomeQuickActions,
            crate::designer::ComponentId::HomePublicRooms,
            crate::designer::ComponentId::HomeFriends,
            crate::designer::ComponentId::HomeRecentActivity,
        );

        let window_width = dep.window_width_bits as f32 / 100.0;
        let window_height = dep.window_height_bits as f32 / 100.0;
        let theme = Self::theme_from_dark(dep.dark_mode);
        // UI-HOME-15: all home breakpoints are based on the dashboard's
        // available *content* width (window minus sidebar, divider and page
        // padding), never the raw window width — the sidebar eats
        // 288–320 px and would otherwise starve the grid on narrow windows.
        let content_width = layout.content_width(window_width, &sidebar, &responsive);
        let compact_header = content_width < responsive.home_compact_header_content;

        // ── BORU-LAYOUT-04: resolve the active viewport tier ──
        // The tier thresholds (narrow_max_width / ultra_wide_min_width) and
        // the per-tier home column counts / padding live in the model, so
        // TOML can move them later. Defaults match the BORU-UI-15 gallery
        // vocabulary: Narrow < 360 px, Desktop 360–1439 px, UltraWide ≥
        // 1440 px — and reproduce the pre-responsive layout exactly.
        let viewport_tier = responsive.tier_for_width(window_width);
        let vertical_scale = responsive.vertical_spacing_scale(window_height);
        let grid_columns = responsive.home_columns.for_tier(viewport_tier);

        // ── BORU-LAYOUT-03: section order / visibility from the model ──
        // `layout.visible_sections()` = `section_order` minus
        // `hidden_sections`. In Grid mode the left (main) column holds the
        // non-rail sections and the right rail holds PeopleActivity/Tunnels;
        // each column renders its sections in model order. In List mode the
        // whole set stacks in model order in one column. The default order
        // (Hero, MeshHealth, QuickActions, PeopleActivity, Tunnels) renders
        // byte-for-byte like the pre-layout code.
        let visible_sections = layout.visible_sections();
        let is_rail_section = |s: crate::layout::HomeSection| {
            matches!(
                s,
                crate::layout::HomeSection::PeopleActivity | crate::layout::HomeSection::Tunnels
            )
        };

        // HOME-01: opacity of home menu/action card backgrounds over the
        // home background image (1.0 = fully opaque; lower = translucent).
        let home_menu_opacity = f32::from_bits(dep.home_menu_item_opacity_bits);

        // ── Connection state (single source of truth) ──
        let has_peer_connections = dep.has_peer_connections;
        let relay_reachable = dep.sender_ready || has_peer_connections;
        let mesh_health = dep.mesh_health.as_mesh_health();
        let variant = home_connection_variant(&mesh_health, has_peer_connections, relay_reachable);

        // ── Hero variant visuals (truthful, from the pure mapping above) ──
        let headline: String = match variant {
            HomeConnectionVariant::Starting => {
                const RECONNECT_DOTS: [&str; 4] = ["\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}"];
                let dot = RECONNECT_DOTS
                    [(dep.main_screen_reconnect_frame as usize) % RECONNECT_DOTS.len()];
                crate::i18n::t_args("home.starting", &[("dot", dot)])
            }
            HomeConnectionVariant::Connecting => crate::i18n::t("home.connecting"),
            HomeConnectionVariant::Ready => crate::i18n::t("home.ready"),
            HomeConnectionVariant::Degraded => {
                let reason = match &mesh_health {
                    MeshHealth::Degraded(r) => r.clone(),
                    _ => String::new(),
                };
                crate::i18n::t_args("home.degraded", &[("reason", &reason)])
            }
            HomeConnectionVariant::Offline => {
                let reason = match &mesh_health {
                    MeshHealth::Offline(r) => r.clone(),
                    _ => String::new(),
                };
                crate::i18n::t_args("home.offline", &[("reason", &reason)])
            }
        };
        let show_retry = matches!(variant, HomeConnectionVariant::Offline);
        let show_details = matches!(
            variant,
            HomeConnectionVariant::Offline | HomeConnectionVariant::Degraded
        );

        // ── Greeting (page header) ──
        // UI-HOME-12: display_heading — Archivo SemiCondensed Bold 32 px,
        // 1.2 line height (via TypeRole::DisplayHeading). BORU-UI-16:
        // family/weight/line-height come from the live theme so the
        // inspector can adjust them; the default matches the approved
        // mapping exactly.
        let greeting = crate::fonts::type_role_text_themed(
            &btheme,
            crate::fonts::TypeRole::DisplayHeading,
            crate::i18n::t_args("home.greeting", &[("time", &dep.time_of_day_greeting)]),
        )
        .color(crate::design_tokens::text_primary(&theme))
        .width(Length::Fill)
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph);
        // Subtitle — IBM Plex Sans Regular at the UI-HOME-02 size token
        // (16 px; the canonical `body` role is 15 px, plan band 15–17 px).
        let welcome_line = crate::fonts::type_role_text(
            crate::fonts::TypeRole::Body,
            crate::i18n::t("home.welcome"),
        )
        .size(btheme.typography.home_subtitle)
        .color(text_secondary(&theme))
        .width(Length::Fill);

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
        // a 2:1 grid (content_width − 24) × 2/3; with the rail stacked it spans the full
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
                card_radius: btheme.radii.card,
                sizing: layout.card_sizing,
            });
        #[cfg(feature = "dev-ui")]
        let hero_card = crate::designer::overlay(
            crate::designer::ComponentId::HomeWelcome,
            hero_card,
            designer_enabled,
            designer_hovered,
            designer_selected,
            None,
        );

        // ── Mesh Health card ──
        // UI-HOME-05: full dashboard card. Header carries a mesh glyph +
        // title + real status badge + the existing "View details" action.
        // Body shows the live status row, three real connection counts
        // (neighbors / direct / relayed), connection state + duration
        // when available, and a short recent-events list fed from the same
        // bounded mesh event log the rest of the app uses — no invented
        // statistics. UI-28 keeps transient startup lines from lingering:
        // the watchdog clears "Starting up...", "Connecting to room...",
        // "Connected to room..." and "Subscribing to..." once the mesh is
        // Good, so the log stays truthful.
        let (health_label, health_color): (String, fn(&iced::Theme) -> Color) = match &mesh_health {
            MeshHealth::Good => (crate::i18n::t("status.healthy"), accent_green),
            MeshHealth::Degraded(_) => (crate::i18n::t("status.degraded"), color_warning),
            MeshHealth::Offline(_) => (crate::i18n::t("status.offline"), color_error),
        };
        let mesh_has_peers = dep.has_peer_connections;
        let mesh_relay_reachable = dep.sender_ready || mesh_has_peers;
        let mesh_variant =
            home_connection_variant(&mesh_health, mesh_has_peers, mesh_relay_reachable);

        let (status_icon, status_color, status_label): (&[u8], fn(&iced::Theme) -> Color, String) =
            match mesh_variant {
                HomeConnectionVariant::Starting => (
                    ICON_RETRY,
                    color_warning,
                    crate::i18n::t("status.starting_up"),
                ),
                HomeConnectionVariant::Connecting => {
                    (ICON_RETRY, color_warning, crate::i18n::t("home.connecting"))
                }
                HomeConnectionVariant::Ready => {
                    (ICON_CHECK, accent_green, crate::i18n::t("common.connected"))
                }
                HomeConnectionVariant::Degraded => {
                    let reason = match &mesh_health {
                        MeshHealth::Degraded(r) => r.clone(),
                        _ => String::new(),
                    };
                    (
                        ICON_MESH,
                        color_warning,
                        crate::i18n::t_args("status.degraded_reason", &[("reason", &reason)]),
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
                        crate::i18n::t_args("status.offline_reason", &[("reason", &reason)]),
                    )
                }
            };

        // Secondary line: current peer counts, plus connection time once the
        // mesh is healthy (mesh_connected_at is maintained by the watchdog).
        let status_detail = match mesh_variant {
            HomeConnectionVariant::Starting => crate::i18n::t("status.establishing_mesh"),
            HomeConnectionVariant::Connecting => crate::i18n::t("status.waiting_for_peers"),
            _ => {
                let mut parts = vec![crate::i18n::t_args(
                    "status.direct_relay_neighbors",
                    &[
                        ("direct", &dep.direct_peers.to_string()),
                        ("relayed", &dep.relayed_peers.to_string()),
                        ("neighbors", &dep.neighbors_len.to_string()),
                    ],
                )];
                if let Some(secs) = dep.connected_age_secs {
                    let duration = if secs < 60 {
                        crate::i18n::t_args("status.connected_secs", &[("secs", &secs.to_string())])
                    } else if secs < 3600 {
                        crate::i18n::t_args(
                            "status.connected_min_sec",
                            &[
                                ("mins", &(secs / 60).to_string()),
                                ("secs", &(secs % 60).to_string()),
                            ],
                        )
                    } else {
                        crate::i18n::t_args(
                            "status.connected_hr_min",
                            &[
                                ("hrs", &(secs / 3600).to_string()),
                                ("mins", &((secs % 3600) / 60).to_string()),
                            ],
                        )
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
            .push(
                icon_svg(status_icon, TYPO_MD).style(move |t, _| iced::widget::svg::Style {
                    color: Some(status_color(t)),
                }),
            )
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

        let mesh_body = mesh_status_row;

        let mesh_card = CardShell::new(crate::i18n::t("home.mesh_health"), vec![])
            .title_case(false)
            .header_icon(
                icon_svg(ICON_MESH, TYPO_MD)
                    .style(move |t, _| iced::widget::svg::Style {
                        color: Some(health_color(t)),
                    })
                    .into(),
            )
            .subtitle(crate::i18n::t("home.mesh_health_subtitle"))
            .status_badge(health_label.as_str(), mesh_badge_kind)
            .header_action(
                crate::i18n::t("home.view_details"),
                AppMessage::OpenConnectionDetails,
            )
            .compact_header(compact_header)
            .body(mesh_body.into())
            .card_radius(btheme.radii.card)
            .background_opacity(home_menu_opacity)
            .build(&theme);
        #[cfg(feature = "dev-ui")]
        let mesh_card = crate::designer::overlay(
            crate::designer::ComponentId::HomePublicRooms,
            mesh_card.into(),
            designer_enabled,
            designer_hovered,
            designer_selected,
            None,
        );

        // ── Quick actions: four equal, full-card targets (Figure 3) ──
        // BORU-UI-03: card radius comes from the LIVE theme
        // (`btheme.radii.card`) so the inspector's "Card" radius slider
        // changes the action cards immediately.
        let action_grid = crate::quick_actions::quick_action_grid(
            content_width,
            &theme,
            home_menu_opacity,
            btheme.radii.card,
            layout.quick_actions,
            layout.card_sizing.quick_action_icon_size,
        );
        #[cfg(feature = "dev-ui")]
        let action_grid = crate::designer::overlay(
            crate::designer::ComponentId::HomeQuickActions,
            action_grid,
            designer_enabled,
            designer_hovered,
            designer_selected,
            None,
        );

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
                .push(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    crate::i18n::t("home.download_manager"),
                ))
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

        // ── Right column: People & Activity / Tunnels ──
        // BORU-HOME-05: Online Peers + Recent Activity merged into one
        // coherent "People & Activity" card with a restrained divider between
        // the peers section and the activity feed. The combined dependency
        // changes when either slice changes, so the merged card rebuilds
        // correctly via `iced::widget::lazy`.
        let people_layout = layout.clone();
        let people_activity_card =
            iced::widget::lazy(dep.people_activity.clone(), move |card_dep| {
                Self::view_people_activity_card(card_dep, btheme, people_layout.clone())
            });
        #[cfg(feature = "dev-ui")]
        let people_activity_card = crate::designer::overlay(
            crate::designer::ComponentId::HomeFriends,
            people_activity_card.into(),
            designer_enabled,
            designer_hovered,
            designer_selected,
            None,
        );
        let tunnels_card = iced::widget::lazy(dep.tunnels.clone(), move |card_dep| {
            Self::view_tunnels_card(card_dep, btheme)
        });
        #[cfg(feature = "dev-ui")]
        let tunnels_card = crate::designer::overlay(
            crate::designer::ComponentId::HomeRecentActivity,
            tunnels_card.into(),
            designer_enabled,
            designer_hovered,
            designer_selected,
            None,
        );

        // ── Page header: greeting + welcome + Download Manager ──
        // UI-HOME-15: on narrow content the Download Manager stacks under
        // the greeting (left-aligned); on wider content it keeps the
        // approved top-right position.
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
                .push(Space::new().height(Length::Fixed(layout.gaps.compact_header_stack_gap)))
                .push(download_manager_btn)
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
                download_manager_btn,
            ]
            .spacing(SPACE_8)
            .align_y(Alignment::Center)
            .into()
        };

        // ── Main content: section order / grid from the layout model ──
        // BORU-LAYOUT-03: every visible section renders exactly once, in
        // `layout.visible_sections()` order. Grid mode splits the set into
        // the main column (Hero/MeshHealth/QuickActions) and the right rail
        // (PeopleActivity/Tunnels), each in model order; below the stack
        // breakpoint the rail stacks under the main column (the pre-layout
        // behaviour). List mode stacks the whole set in one column. The
        // default order + values reproduce today's layout byte-for-byte.
        let main_sections: Vec<crate::layout::HomeSection> = visible_sections
            .iter()
            .copied()
            .filter(|s| !is_rail_section(*s))
            .collect();
        let rail_sections: Vec<crate::layout::HomeSection> = visible_sections
            .iter()
            .copied()
            .filter(|s| is_rail_section(*s))
            .collect();
        let list_mode = matches!(
            layout.mode,
            crate::layout::HomeLayoutMode::List | crate::layout::HomeLayoutMode::Column
        );
        let row_mode = matches!(layout.mode, crate::layout::HomeLayoutMode::Row);

        // Consume the built section elements in model order (each visible
        // section appears in exactly one list below, so `remove` never
        // misses). BTreeMap keeps the type Hash/Eq-free; sections hidden by
        // the model are simply never consumed and dropped.
        let mut section_elements: std::collections::BTreeMap<
            crate::layout::HomeSection,
            iced::Element<'static, AppMessage>,
        > = std::collections::BTreeMap::new();
        section_elements.insert(crate::layout::HomeSection::Hero, hero_card);
        section_elements.insert(crate::layout::HomeSection::MeshHealth, mesh_card);
        section_elements.insert(crate::layout::HomeSection::QuickActions, action_grid);
        section_elements.insert(
            crate::layout::HomeSection::PeopleActivity,
            people_activity_card.into(),
        );
        section_elements.insert(crate::layout::HomeSection::Tunnels, tunnels_card.into());

        let card_gap = layout.gaps.card_gap * vertical_scale;
        let mut column_from_sections = |list: &[crate::layout::HomeSection]| {
            let mut col = Column::new().spacing(0).width(Length::Fill);
            for (i, section) in list.iter().enumerate() {
                if i > 0 {
                    col = col.push(Space::new().height(Length::Fixed(card_gap)));
                }
                col = col.push(
                    section_elements
                        .remove(section)
                        .expect("visible section element built above"),
                );
            }
            col
        };

        // ── BORU-LAYOUT-04: effective column count ──
        // The responsive tier's per-tier column count (`grid_columns`,
        // resolved from the window width above) combines with the
        // pre-responsive content-width stack rule: the dashboard stacks to
        // a single column in List mode, when the tier asks for one column,
        // or when the content width is below the home grid's stack
        // breakpoint. The default tier table (narrow 1 / desktop 2 /
        // ultra-wide 2) with the default stack breakpoint (720) reproduces
        // the pre-responsive behaviour at every window size.
        let effective_columns =
            if list_mode || grid_columns <= 1 || content_width < layout.grid.stack_breakpoint {
                1
            } else {
                grid_columns
            };

        let main_content: iced::Element<'_, AppMessage> = if row_mode {
            // Row mode is intentionally a semantic, responsive flow: it uses
            // the same typed section order but lays cards side-by-side. The
            // content width remains Fill so cards shrink rather than
            // overflowing the responsive canvas.
            let mut row = Row::new().spacing(card_gap).width(Length::Fill);
            for section in &visible_sections {
                if let Some(element) = section_elements.remove(section) {
                    row = row.push(element);
                }
            }
            row.into()
        } else if list_mode {
            // Single stacked column in model order (all visible sections).
            column_from_sections(&visible_sections).into()
        } else if effective_columns <= 1 {
            // Narrow: main-column cards first, then the activity rail below.
            if main_sections.is_empty() {
                // No main-column sections visible — the rail owns the page.
                column_from_sections(&rail_sections).into()
            } else if rail_sections.is_empty() {
                // No rail sections visible — the main column spans full width.
                column_from_sections(&main_sections).into()
            } else {
                let left_col = column_from_sections(&main_sections);
                let right_col = column_from_sections(&rail_sections);
                Column::new()
                    .push(left_col)
                    .push(Space::new().height(Length::Fixed(card_gap)))
                    .push(right_col)
                    .spacing(0)
                    .width(Length::Fill)
                    .into()
            }
        } else if main_sections.is_empty() {
            // No main-column sections visible — the rail owns the page.
            column_from_sections(&rail_sections).into()
        } else if rail_sections.is_empty() {
            // No rail sections visible — the main column spans full width.
            column_from_sections(&main_sections).into()
        } else {
            // Wide: two-column dashboard grid, both columns aligned top.
            // CONN-10 (spec §14 — no parent layout stretching): the left
            // column wrapper is explicitly Shrink-height so the hero card
            // can never be stretched by the (taller) right rail. The Row's
            // `align_y(Start)` is the `align-self: start` equivalent — iced
            // never resizes a Shrink-height child to match a sibling, so
            // opening the rail cannot force the status card taller. The
            // card's own container in status_card.rs pins the same guard.
            // BORU-LAYOUT-03: the main/rail FillPortion split and the
            // column gap come from the layout model (`home.grid`).
            let left_col = column_from_sections(&main_sections);
            let right_col = column_from_sections(&rail_sections);
            Row::new()
                .push(
                    container(left_col)
                        .width(Length::FillPortion(layout.grid.main_portion))
                        .height(Length::Shrink),
                )
                .push(Space::new().width(Length::Fixed(layout.grid.column_gap)))
                .push(
                    container(right_col)
                        .width(Length::FillPortion(layout.grid.rail_portion))
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
            crate::i18n::t("status.quic_encrypted")
        } else {
            crate::i18n::t("status.idle")
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
        // BORU-LAYOUT-03: top/bottom come from the layout model
        // (`home.padding`). BORU-LAYOUT-04: the horizontal padding comes
        // from the responsive tier's per-tier table (`responsive.home_padding_x`,
        // resolved from the window width above) — its defaults are the same
        // values as `home.padding.horizontal_large` / `horizontal_default`,
        // so the default appearance is unchanged.
        let h_padding = responsive.home_padding_x.for_tier(viewport_tier);
        let top_padding = layout.padding.top * vertical_scale;
        let bottom_padding = layout.padding.bottom * vertical_scale;

        // POLISH-05: page header → dashboard gap bumped from SPACE_28 to
        // ~40 px — roughly 12 px more breathing room between the
        // \"Welcome to Boru\" subtitle and the card grid. BORU-LAYOUT-03:
        // the gap comes from the layout model (`home.gaps`).
        #[cfg(feature = "dev-ui")]
        let drag_ghost: Option<iced::Element<'static, AppMessage>> =
            drag_placeholder.map(|(_, index)| {
                container(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        format!("Drop section at position {}", index + 1),
                    )
                    .color(crate::design_tokens::text_muted(&theme)),
                )
                .width(Length::Fill)
                .padding([SPACE_4, SPACE_8])
                .style(|theme| container::Style {
                    background: Some(iced::Background::Color(
                        crate::design_tokens::surface_hover(theme),
                    )),
                    border: iced::Border {
                        color: iced::Color::from_rgb(0.25, 0.68, 1.0),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                })
                .into()
            });
        #[cfg(feature = "dev-ui")]
        let col = Column::new()
            .push(page_header)
            .push(Space::new().height(Length::Fixed(
                layout.gaps.header_dashboard_gap * vertical_scale,
            )))
            .push(drag_ghost)
            .push(main_content)
            .push(Space::new().height(Length::Fixed(layout.gaps.footer_gap * vertical_scale)))
            .push(footer)
            .spacing(0)
            .width(Length::Fill);
        #[cfg(not(feature = "dev-ui"))]
        let col = Column::new()
            .push(page_header)
            .push(Space::new().height(Length::Fixed(
                layout.gaps.header_dashboard_gap * vertical_scale,
            )))
            .push(main_content)
            .push(Space::new().height(Length::Fixed(layout.gaps.footer_gap * vertical_scale)))
            .push(footer)
            .spacing(0)
            .width(Length::Fill);

        // Cap the dashboard width (~1480 px) and centre it in the available
        // content region; vertical page scrolling stays on gutter_scrollable.
        // The max-width only binds on very wide windows (e.g. 1920), where it
        // keeps the grid from stretching edge-to-edge. BORU-LAYOUT-03: the
        // cap comes from the layout model (`home.max_content_width`).
        // UI-HOME-11: the dashboard content container uses Shrink height so
        // the cards + footer take only their natural height instead of
        // stretching to fill the viewport — no giant empty white space on
        // tall/maximized windows. The outer canvas keeps Fill height for
        // scrollable bounds + horizontal centering.
        let canvas = container(
            container(col)
                .padding(iced::Padding::from([top_padding, h_padding]).bottom(bottom_padding))
                .width(Length::Fill)
                .max_width(layout.max_content_width)
                .height(Length::Shrink),
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
