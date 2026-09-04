//! Discover / public chats + peer & friend profile features.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the Discover screen
//! (public room directory), the per-peer catalogue, and the Peer /
//! Friend profile screens: the `impl IcedChat` methods that build and
//! render them. Dependency snapshot structs remain in app.rs for now.
//! Reads app state via `use super::*`; app.rs re-exports the pub(crate)
//! items it still references with `use discover::*`.

use super::{
    AppMessage,
    Arc,
    BUTTON_DANGER,
    BUTTON_GHOST,
    BUTTON_GHOST_BG,
    BUTTON_PRIMARY,
    CatalogueDownloadSnapshot,
    CatalogueRowSnapshot,
    ChatEntry,
    Color,
    ConversationEntry,
    ConversationKind,
    ConversationLive,
    DiscoverDependency,
    DiscoverRoomRow,
    DiscoveredPeersUpdate,
    Duration,
    FriendId,
    FriendProfileDependency,
    FriendProfileServiceRow,
    HashSet,
    ICON_CHAT,
    ICON_CLOSE,
    ICON_FILES,
    IcedChat,
    Icon,
    IconSize,
    Instant,
    Message,
    Ordering,
    PeerCatalogueDependency,
    PeerPresence,
    PeerProfileDependency,
    PublicKey,
    RoomInvitation,
    RoomMetadata,
    RoomStore,
    RoomVisibility,
    SPACE_10,
    SPACE_12,
    SPACE_16,
    SPACE_2,
    SPACE_4,
    SPACE_6,
    SPACE_8,
    Screen,
    SignedMessage,
    StdMutex,
    Storage,
    TYPO_LG,
    TYPO_MD,
    TYPO_SM,
    TYPO_XS,
    Ticket,
    TopicId,
    accent_green,
    accent_primary,
    bg_surface,
    border_muted,
    color_error,
    color_warning,
    container_primary,
    container_surface,
    direct_topic,
    error,
    fetch_paginated_remote_catalogue,
    format_file_size,
    icon_svg,
    info,
    now_ms,
    spawn_conversation_forwarder,
    text_muted,
    text_muted_style,
    text_remote_body,
    trace,
    tunnel_expiry_label,
    tunnel_local_address,
    tunnel_route_label,
    view_toast,
    warn,
};
use boru_core::chat_callbacks::ChatCallbacks;
use iroh::Watcher;
use std::str::FromStr;
#[cfg(feature = "screen-sharing")]
use super::{
    status_row,
};
// ── Room card display helpers (PDF Task 5.2) ─────────────────────────
//
// Display bounds for directory-card text. Every text field is elided to
// these lengths AND wrapped (`WordOrGlyph`), so even a hostile
// advertisement with oversized metadata can never push the card wider
// than its column or split a multi-byte character (PDF Task 5.2
// acceptance: "Oversized text cannot break layout").
const DISCOVER_MAX_NAME_CHARS: usize = 64;
const DISCOVER_MAX_DESC_CHARS: usize = 160;
const DISCOVER_MAX_TAG_CHARS: usize = 24;
const DISCOVER_MAX_TAGS_SHOWN: usize = 4;

/// Elide `text` to at most `max_chars` Unicode characters, appending an
/// ellipsis when truncated. Char-boundary safe: never splits a
/// multi-byte character.
pub(crate) fn discover_elide(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

/// The action-button label for a directory card (PDF Task 5.2: Join /
/// Open). Join wiring itself is BORU-DIR-16; this only names the action.
pub(crate) fn discover_action_label(
    action: boru_core::room_directory::RoomAction,
) -> &'static str {
    match action {
        boru_core::room_directory::RoomAction::Join => "Join",
        boru_core::room_directory::RoomAction::Open => "Open",
        boru_core::room_directory::RoomAction::Hidden => "Hidden",
        boru_core::room_directory::RoomAction::Incompatible => "Incompatible",
    }
}

/// Human-readable compatibility label (PDF Task 5.2 step 4: clearly
/// label incompatible rooms). DIR-17 refines the model later; the
/// cache already exposes Compatible/UpgradeRequired/Unsupported/Unknown.
pub(crate) fn discover_compat_label(
    compat: boru_core::room_directory::RoomCompatibility,
) -> &'static str {
    match compat {
        boru_core::room_directory::RoomCompatibility::Compatible => "Compatible",
        boru_core::room_directory::RoomCompatibility::UpgradeRequired => "Upgrade required",
        boru_core::room_directory::RoomCompatibility::Unsupported => "Not supported",
        boru_core::room_directory::RoomCompatibility::Unknown => "Compatibility unknown",
    }
}

/// Optional-feature hint text for a discovered room card (PDF Task 6.2
/// step 2). Returns `None` when the room advertises no optional features
/// or every advertised feature is supported locally. When some features
/// are missing, returns a muted, informational hint — the room remains
/// joinable; the hint never blocks basic room access.
pub(crate) fn discover_feature_hint(
    feature_compat: &boru_core::room_directory::RoomFeatureCompatibility,
) -> Option<String> {
    match feature_compat {
        boru_core::room_directory::RoomFeatureCompatibility::None
        | boru_core::room_directory::RoomFeatureCompatibility::AllSupported => None,
        boru_core::room_directory::RoomFeatureCompatibility::SomeMissing(missing) => {
            if missing.is_empty() {
                None
            } else {
                Some(format!(
                    "Optional features unavailable: {}",
                    missing.join(", ")
                ))
            }
        }
    }
}

/// Approximate member-count text. The count is an untrusted
/// self-reported hint (PDF Task 7.3 / DIR-21 guardrails), so it is
/// always rendered as clearly approximate ("~N members (approx.)") and
/// omitted entirely when absent or zero — never presented as
/// authoritative, and never used for ranking (the Discover sort orders
/// do not consult it).
pub(crate) fn discover_member_count_text(count: Option<u32>) -> Option<String> {
    count
        .filter(|&c| c > 0)
        .map(|c| format!("~{c} members (approx.)"))
}

// ── Search / filter / sort (PDF Task 5.3) ─────────────────────────────
//
// BORU-DIR-15: the Discover browse surface supports client-side search
// (room name, description, tags), simple filters (Compatible, Not
// Joined, Recently Seen, tags/categories), and sorting — ALL from the
// local RoomDirectory cache snapshot. Nothing here touches the network:
// search queries are never broadcast onto the discovery network (PDF
// Core rule / Task 5.3 acceptance "Search does not leak queries").

/// Sort order for the Discover browse surface (local cached metadata
/// only — never a global or popularity ranking).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum DiscoverSort {
    /// Most recently seen first (by the cache's per-entry `last_seen`).
    #[default]
    RecentlySeen,
    /// Joinable rooms first: Compatible, then UpgradeRequired, Unknown,
    /// Unsupported.
    Compatibility,
    /// Alphabetical by room name (case-insensitive).
    Name,
}

/// The simple filter toggles on the Discover browse surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiscoverFilter {
    Compatible,
    NotJoined,
    RecentlySeen,
}

/// Combined filter-toggle state for [`discover_filter_sort`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct DiscoverFilterState {
    pub(crate) compatible: bool,
    pub(crate) not_joined: bool,
    pub(crate) recently_seen: bool,
}

/// Window for the "Recently Seen" filter: an advertisement must have
/// been received within the last 24h to count as recent.
pub(crate) const DISCOVER_RECENTLY_SEEN_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// The tag filter matches a room when ANY selected tag is present in its
/// tags (OR semantics within the tag/category filter).
fn discover_row_matches_selected_tags(
    tags: &[String],
    selected_tags: &[String],
) -> bool {
    if selected_tags.is_empty() {
        return true;
    }
    tags.iter().any(|tag| selected_tags.iter().any(|s| s == tag))
}

/// Case-insensitive search across room name, description, and tags.
/// Empty query matches everything.
fn discover_row_matches_query(row: &DiscoverRoomRow, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    let name = row.room_name.to_lowercase();
    let desc = row.short_description.to_lowercase();
    let tag_blob = row
        .tags
        .iter()
        .map(|t| t.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let haystack = format!("{name} {desc} {tag_blob}");
    haystack.contains(&q)
}

/// Sort rank for the Compatibility order (lower = more joinable).
fn discover_compat_rank(compat: boru_core::room_directory::RoomCompatibility) -> u8 {
    match compat {
        boru_core::room_directory::RoomCompatibility::Compatible => 0,
        boru_core::room_directory::RoomCompatibility::UpgradeRequired => 1,
        boru_core::room_directory::RoomCompatibility::Unknown => 2,
        boru_core::room_directory::RoomCompatibility::Unsupported => 3,
    }
}

/// Filter + sort local directory rows. Purely functional over the input
/// rows — no cache access, no network, no state. Used by
/// [`IcedChat::discover_dependency`] and by unit tests.
///
/// Each input is `(renderable_row, recency)`; `recency` is `None` when
/// the row came from the legacy directory store (no per-entry `Instant`),
/// in which case the Recently-Seen filter passes it (unknown ≠ stale)
/// and the Recently-Seen sort places it last.
pub(crate) fn discover_filter_sort(
    rows: Vec<(DiscoverRoomRow, Option<Instant>)>,
    query: &str,
    filters: DiscoverFilterState,
    selected_tags: &[String],
    sort: DiscoverSort,
    now: Instant,
) -> Vec<DiscoverRoomRow> {
    let mut out = Vec::with_capacity(rows.len());
    for (row, last_seen) in rows {
        if !discover_row_matches_query(&row, query) {
            continue;
        }
        if filters.compatible
            && row.compatibility != boru_core::room_directory::RoomCompatibility::Compatible
        {
            continue;
        }
        if filters.not_joined
            && row.local_join_state != boru_core::room_directory::LocalJoinState::NotJoined
        {
            continue;
        }
        if filters.recently_seen {
            let recent = match last_seen {
                Some(seen) => now.duration_since(seen) <= DISCOVER_RECENTLY_SEEN_WINDOW,
                None => true,
            };
            if !recent {
                continue;
            }
        }
        if !discover_row_matches_selected_tags(&row.tags, selected_tags) {
            continue;
        }
        out.push((row, last_seen));
    }

    match sort {
        DiscoverSort::RecentlySeen => {
            out.sort_by(|a, b| match (a.1, b.1) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.0.room_name.cmp(&b.0.room_name),
            });
        }
        DiscoverSort::Compatibility => {
            out.sort_by(|a, b| {
                discover_compat_rank(a.0.compatibility)
                    .cmp(&discover_compat_rank(b.0.compatibility))
                    .then_with(|| {
                        a.0.room_name
                            .to_lowercase()
                            .cmp(&b.0.room_name.to_lowercase())
                    })
            });
        }
        DiscoverSort::Name => {
            out.sort_by(|a, b| a.0.room_name.to_lowercase().cmp(&b.0.room_name.to_lowercase()));
        }
    }

    out.into_iter().map(|(row, _)| row).collect()
}

/// Sorted unique tags across all cached rows — the tag/category filter
/// chips. Computed from the FULL cache (before search/filter), so a
/// search that empties the list never hides the category chips.
pub(crate) fn discover_available_tags(rows: &[DiscoverRoomRow]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        for tag in &row.tags {
            seen.insert(tag.clone());
        }
    }
    seen.into_iter().collect()
}

impl IcedChat {
    pub(crate) fn view_peer_profile(&self, peer: PublicKey) -> iced::Element<'_, AppMessage> {
        let profile_data = self.profile_cache.get(&peer);
        let display_name = profile_data
            .as_ref()
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| "Unknown Peer".to_string());
        let dep = PeerProfileDependency {
            dark_mode: self.dark_mode,
            theme_revision: self.theme_revision,
            peer,
            display_name,
        };
        iced::widget::lazy(dep, Self::view_peer_profile_content).into()
    }

    /// Static renderer for the Peer Profile screen, driven by
    /// [`PeerProfileDependency`].
    pub(crate) fn view_peer_profile_content(
        dep: &PeerProfileDependency,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let display_name = dep.display_name.clone();
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
                .on_press(AppMessage::ClosePeerProfile)
                .padding([SPACE_4, SPACE_8])
                .style(BUTTON_GHOST_BG),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::SectionTitle, display_name.clone())
                    .width(Length::Fill),
            )
            .align_y(Alignment::Center)
            .spacing(SPACE_12);

        let mut body = Column::new().spacing(SPACE_8);

        body = body.push(
            container(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, "No shared files.")
                    .style(text_muted_style),
            )
            .width(Length::Fill)
            .padding(SPACE_12)
            .style(container_surface),
        );

        let content = Column::new()
            .push(
                container(header)
                    .width(Length::Fill)
                    .padding(iced::Padding {
                        top: SPACE_12,
                        right: SPACE_12,
                        bottom: SPACE_4,
                        left: SPACE_12,
                    }),
            )
            .push(body)
            .push(Space::new().height(Length::Fill));

        container(crate::ui_components::gutter_scrollable(content))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(container_primary)
            .into()
    }

    /// Placeholder view for the room directory / Discover screen.
    /// Renders a simple "coming soon" message until the full room
    /// browser UI is implemented.
    /// View a remote peer's shared file catalogue with Download buttons,
    /// rich progress, and quick actions.
    /// Look up a content_hash by display_name in the current peer catalogue view.
    pub(crate) fn catalogue_name_to_hash(&self, name: &str) -> Option<String> {
        self.files_state.peer_catalogue_view
            .as_ref()
            .and_then(|(_, files)| files.iter().find(|f| f.display_name == name))
            .map(|f| f.content_hash.clone())
    }

    pub(crate) fn view_peer_catalogue(&self, peer: PublicKey) -> iced::Element<'_, AppMessage> {
        let dep = self.peer_catalogue_dependency(peer);
        let btheme = self.boru_theme();
        iced::widget::lazy(dep, move |dep| Self::view_peer_catalogue_content(dep, peer, btheme)).into()
    }

    /// Build the Hash-compatible snapshot the Peer Catalogue renders from.
    pub(crate) fn peer_catalogue_dependency(&self, peer: PublicKey) -> PeerCatalogueDependency {
        let display_name = self
            .names
            .get(&peer)
            .cloned()
            .unwrap_or_else(|| "Unknown Peer".to_string());
        let rows = match self.files_state.peer_catalogue_view.as_ref() {
            Some((pk, files)) if *pk == peer => files
                .iter()
                .map(|file| {
                    let dl = self.files_state
                        .catalogue_downloads
                        .get(&file.content_hash)
                        .map(CatalogueDownloadSnapshot::from)
                        .unwrap_or(CatalogueDownloadSnapshot::None);
                    let is_pending = self.files_state
                        .pending_downloads
                        .contains(&(file.content_hash.clone(), peer));
                    CatalogueRowSnapshot {
                        shared_file_id: file.shared_file_id.clone(),
                        display_name: file.display_name.clone(),
                        description: file.description.clone(),
                        mime_type: file.mime_type.clone(),
                        size_bytes: file.size_bytes,
                        content_hash: file.content_hash.clone(),
                        version_number: file.version_number,
                        updated_at_ms: file.updated_at_ms,
                        collection_ids: file.collection_ids.clone(),
                        dl,
                        is_pending,
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        PeerCatalogueDependency {
            dark_mode: self.dark_mode,
            theme_revision: self.theme_revision,
            peer,
            display_name,
            catalogue_loading: self.files_state.catalogue_loading,
            rows,
            catalogue_scroll_offset_bits: (self.files_state.catalogue_scroll_offset.max(0.0) * 100.0) as u32,
            catalogue_viewport_height_bits: (self.files_state.catalogue_viewport_height.max(0.0) * 100.0)
                as u32,
        }
    }

    /// Static renderer for the Peer Catalogue screen. Reads only from the
    /// Hash-compatible [`PeerCatalogueDependency`] snapshot. BORU-UI-07
    /// threads the LIVE merged theme in so boru-ui.toml overrides render
    /// immediately after a reload.
    pub(crate) fn view_peer_catalogue_content(
        dep: &PeerCatalogueDependency,
        peer: PublicKey,
        btheme: crate::theme::BoruTheme,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, scrollable, space, Column, Row, Space};
        use iced::{Alignment, Color, Length};

        // BORU-UI-03: row height / overscan come from the typed theme
        // (mode-independent geometry). BORU-UI-07: from the LIVE theme so
        // a reload is reflected without a rebuild.
        let room_theme = btheme.rooms;
        let catalogue_row_height = room_theme.catalogue_row_height;
        let overscan = room_theme.overscan;

        let display_name = &dep.display_name;

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
                .on_press(AppMessage::ClosePeerProfile)
                .padding([SPACE_4, SPACE_8])
                .style(BUTTON_GHOST_BG),
            )
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SectionTitle,
                    format!("{} — Shared Files", display_name),
                )
                .width(Length::Fill),
            )
            .align_y(Alignment::Center)
            .spacing(SPACE_12);

        let mut file_rows = Column::new().spacing(SPACE_4);

        // ── Open Downloads Folder button ──
        file_rows = file_rows.push(
            container(
                button(
                    Row::new()
                        .push(icon_svg(ICON_FILES, TYPO_SM))
                        .push(crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Open Downloads Folder",
                        ))
                        .spacing(SPACE_4)
                        .align_y(Alignment::Center),
                )
                .on_press(AppMessage::OpenDownloadsFolder)
                .padding([SPACE_6, SPACE_12])
                .style(move |theme, status| {
                    let base = match status {
                        iced::widget::button::Status::Hovered => accent_primary(theme),
                        iced::widget::button::Status::Pressed => {
                            let mut c = accent_primary(theme);
                            c.r *= 0.85;
                            c.g *= 0.85;
                            c.b *= 0.85;
                            c
                        }
                        _ => {
                            crate::theme::BoruTheme::for_theme(theme).colors.tag_text
                        }
                    };
                    let b = crate::theme::BoruTheme::for_theme(theme);
                    let bg = match status {
                        iced::widget::button::Status::Hovered => Some(iced::Background::Color(
                            b.colors.tag_bg,
                        )),
                        iced::widget::button::Status::Pressed => Some(iced::Background::Color(
                            b.colors.tag_bg_pressed,
                        )),
                        _ => None,
                    };
                    iced::widget::button::Style {
                        text_color: base,
                        background: bg,
                        border: iced::Border {
                            color: border_muted(theme),
                            width: b.borders.hairline,
                            radius: SPACE_6.into(),
                        },
                        ..Default::default()
                    }
                }),
            )
            .width(Length::Shrink)
            .padding([SPACE_4, SPACE_12]),
        );

        if dep.catalogue_loading {
            file_rows = file_rows.push(
                container(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Body, "Loading catalogue…")
                        .style(text_muted_style),
                )
                .width(Length::Fill)
                .padding(SPACE_12)
                .style(container_surface),
            );
        } else if !dep.rows.is_empty() {
            let files = &dep.rows;
            if files.is_empty() {
                file_rows = file_rows.push(
                    container(
                        crate::fonts::type_role_text(crate::fonts::TypeRole::Body, "No shared files.")
                            .style(text_muted_style),
                    )
                    .width(Length::Fill)
                    .padding(SPACE_12)
                    .style(container_surface),
                );
            } else {
                let total_h = files.len() as f32 * catalogue_row_height;
                let catalogue_scroll_offset = dep.catalogue_scroll_offset_bits as f32 / 100.0;
                let catalogue_viewport_height = dep.catalogue_viewport_height_bits as f32 / 100.0;

                // ── Window calculation (only when viewport is known) ──
                if catalogue_viewport_height > 0.0 && total_h > 0.0 {
                    let so = catalogue_scroll_offset.max(0.0);
                    let view_top = so;
                    let view_bot = so + catalogue_viewport_height.max(200.0);

                    let range_top = (view_top - overscan).max(0.0);
                    let range_bot = (view_bot + overscan).min(total_h);

                    let first_idx = (range_top / catalogue_row_height) as usize;
                    let mut last_idx = (range_bot / catalogue_row_height) as usize;

                    if last_idx >= files.len() {
                        last_idx = files.len().saturating_sub(1);
                    }
                    if last_idx < first_idx {
                        last_idx = first_idx;
                    }

                    let top_space_h = first_idx as f32 * catalogue_row_height;
                    let bottom_start = (last_idx + 1) as f32 * catalogue_row_height;
                    let bottom_h = (total_h - bottom_start).max(0.0);

                    // Top spacer
                    if top_space_h > 0.0 {
                        file_rows = file_rows.push(
                            space::Space::new()
                                .width(Length::Fill)
                                .height(Length::Fixed(top_space_h)),
                        );
                    }

                    // Visible file rows
                    for row in &files[first_idx..=last_idx] {
                        file_rows = file_rows.push(Self::render_catalogue_row(row, dep.dark_mode, peer, btheme));
                    }

                    // Bottom spacer
                    if bottom_h > 0.0 {
                        file_rows = file_rows.push(
                            space::Space::new()
                                .width(Length::Fill)
                                .height(Length::Fixed(bottom_h)),
                        );
                    }
                } else {
                    // Initial render before any viewport event — render first screenful
                    let initial_count = 20.min(files.len());
                    let _top_space_h = 0.0;
                    let bottom_h = (total_h - initial_count as f32 * catalogue_row_height).max(0.0);

                    for row in &files[..initial_count] {
                        file_rows = file_rows.push(Self::render_catalogue_row(row, dep.dark_mode, peer, btheme));
                    }
                    if bottom_h > 0.0 {
                        file_rows = file_rows.push(
                            space::Space::new()
                                .width(Length::Fill)
                                .height(Length::Fixed(bottom_h)),
                        );
                    }
                }
            }
        }

        let content = Column::new()
            .push(
                container(header)
                    .width(Length::Fill)
                    .padding(iced::Padding {
                        top: SPACE_12,
                        right: SPACE_12,
                        bottom: SPACE_4,
                        left: SPACE_12,
                    }),
            )
            .push(file_rows)
            .push(Space::new().height(Length::Fill));

        container(crate::ui_components::gutter_scrollable(content).on_scroll(|v: scrollable::Viewport| {
            AppMessage::CatalogueScrolled(v.absolute_offset().y, v.bounds().height)
        }))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(container_primary)
        .into()
    }
    /// Render one file row in the peer catalogue view.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn render_catalogue_row(
        row: &CatalogueRowSnapshot,
        _dark_mode: bool,
        peer: PublicKey,
        btheme: crate::theme::BoruTheme,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let size_str = format_file_size(row.size_bytes);
        let mime_display = if row.mime_type.len() > 20 {
            format!("{}…", &row.mime_type[..18])
        } else {
            row.mime_type.clone()
        };

        // ── Build file info column ──
        let info_col = Column::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, row.display_name.clone())
                    .width(Length::Fill),
            )
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    format!("{} · {}", size_str, mime_display),
                )
                .style(text_muted_style),
            )
            .spacing(SPACE_2);

        // PAPIRUS-11: every catalogue row leads with the same central
        // FileTypeIcon component/resolver used by chat cards and the other
        // dashboard rows — the icon answers "what type of file is this?",
        // the action button answers "what is happening to it".  The row
        // already prints the filename and the MIME type as text, so the
        // icon is decorative (PAPIRUS-15): hidden from assistive
        // technology, no redundant type tooltip.
        let type_icon = crate::download_progress_view::decorative_file_type_icon_element(
            &row.display_name,
            Some(row.mime_type.as_str()),
            None,
            crate::file_type_icon::FileTypeIconSize::List,
            &Self::theme_from_dark(_dark_mode),
        );

        // ── Action button based on download state ──
        let action: iced::Element<'static, AppMessage> = match row.dl {
            CatalogueDownloadSnapshot::Pending => button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                "…",
            ))
            .padding([SPACE_2, SPACE_6])
            .into(),
            CatalogueDownloadSnapshot::Downloading {
                bytes,
                total,
                speed,
            } => {
                let pct = total
                    .filter(|t| *t > 0)
                    .map(|t| ((bytes as f64 / t as f64) * 100.0) as u8)
                    .unwrap_or(0);
                let speed_str = if speed > 0 {
                    format!("{}/s", format_file_size(speed))
                } else {
                    String::new()
                };
                Column::new()
                    .push(
                        Row::new()
                            .push(
                                iced::widget::progress_bar(0.0..=1.0, pct as f32 / 100.0)
                                    .length(Length::Fixed(
                                        btheme.rooms.progress_length,
                                    ))
                                    .girth(Length::Fixed(
                                        btheme.rooms.progress_girth,
                                    )),
                            )
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Metadata,
                                    format!("{}%", pct),
                                )
                                .color(accent_primary(&iced::Theme::Dark)),
                            )
                            .align_y(Alignment::Center)
                            .spacing(SPACE_4),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            format!("{}{}", format_file_size(bytes), speed_str),
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_2)
                    .align_x(Alignment::End)
                    .into()
            }
            CatalogueDownloadSnapshot::Completed => Row::new()
                .push(
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Open",
                    ))
                    .on_press(AppMessage::OpenDownloadedFile(row.display_name.clone()))
                    .padding([SPACE_2, SPACE_6]),
                )
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "✓")
                        .color(accent_green(&iced::Theme::Dark)),
                )
                .spacing(SPACE_4)
                .align_y(Alignment::Center)
                .into(),
            CatalogueDownloadSnapshot::Failed => Column::new()
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Failed")
                        .color(color_error(&iced::Theme::Dark)),
                )
                .push(
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Retry",
                    ))
                    .on_press(AppMessage::RequestFileDownload {
                        peer,
                        file: row.to_file(),
                    })
                    .padding([SPACE_2, SPACE_6]),
                )
                .spacing(SPACE_2)
                .align_x(Alignment::End)
                .into(),
            CatalogueDownloadSnapshot::Cancelled => Column::new()
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Cancelled")
                        .style(text_muted_style),
                )
                .push(
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Retry",
                    ))
                    .on_press(AppMessage::RequestFileDownload {
                        peer,
                        file: row.to_file(),
                    })
                    .padding([SPACE_2, SPACE_6]),
                )
                .spacing(SPACE_2)
                .align_x(Alignment::End)
                .into(),
            CatalogueDownloadSnapshot::None => {
                if row.is_pending {
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "…",
                    ))
                    .padding([SPACE_2, SPACE_6])
                    .into()
                } else {
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Download",
                    ))
                    .on_press(AppMessage::RequestFileDownload {
                        peer,
                        file: row.to_file(),
                    })
                    .padding([SPACE_2, SPACE_6])
                    .into()
                }
            }
        };

        let file_row = Row::new()
            .push(type_icon)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(info_col.width(Length::Fill))
            .push(action)
            .spacing(SPACE_8)
            .align_y(Alignment::Center)
            .padding([SPACE_4, SPACE_8]);

        container(file_row)
            .width(Length::Fill)
            .style(container_surface)
            .into()
    }
    /// FS-13: recompute the Sharing Summary projection from durable records.
    ///
    /// Runs on a background thread so the GUI loop is never blocked by the
    /// bounded SQLite reads. `None` is delivered when storage is unavailable,
    /// which keeps the card in its unknown (em dash) state — never a zero.

    pub(crate) fn view_discover(&self) -> iced::Element<'_, AppMessage> {
        // Cache the whole Discover screen with `lazy`: switching away and back
        // reuses the built widget tree unless the room list actually changed.
        let dep = self.discover_dependency();
        iced::widget::lazy(dep, Self::view_discover_content).into()
    }

    /// Builds the Discover screen's renderable snapshot.
    ///
    /// BORU-DIR-13 (PDF 5.1): the browse surface reads from the bounded
    /// [`RoomDirectory`] cache (BORU-DIR-10..12) when the discovery service
    /// provided a read handle, falling back to the legacy directory store
    /// (tests / discovery service unavailable). The directory is a pure
    /// browse surface: rows are snapshots of cached metadata plus the local
    /// relationship verdict — no subscription, no membership mutation.
    pub(crate) fn discover_dependency(&self) -> DiscoverDependency {
        // BORU-DIR-15 (PDF Task 5.3): everything below is a pure function
        // of the LOCAL cache snapshot + local UI state. No network call is
        // made here: the search query, filters, and sort only ever read
        // the bounded RoomDirectory cache, and the query string is never
        // broadcast onto the discovery network.
        let now = Instant::now();

        // Build rows with recency from the cache (or the legacy store).
        let mut rows: Vec<(DiscoverRoomRow, Option<Instant>)> = if let Some(dir) = &self.room_directory {
            let guard = dir.lock().unwrap();
            guard
                .snapshot()
                .into_iter()
                .map(|entry| {
                    let row = DiscoverRoomRow {
                        room_id: *entry.advert.room_id.as_bytes(),
                        room_name: entry.advert.room_name.clone(),
                        short_description: entry.advert.short_description.clone(),
                        tags: entry.advert.tags.clone(),
                        room_protocol_version: entry.advert.room_protocol_version,
                        owner_peer_id: entry.advert.owner_peer_id,
                        member_count: entry.advert.approximate_member_count,
                        compatibility: entry.compatibility,
                        feature_compat: entry.feature_compat.clone(),
                        local_join_state: entry.local_join_state,
                        offered_action: entry.offered_action(),
                        conflict: entry.conflict,
                    };
                    (row, Some(entry.last_seen))
                })
                .collect()
        } else {
            // Legacy fallback: the old directory-store advertisements
            // (relay-scoped directory gossip topic). Same row shape so the
            // browse surface renders identically. Recency is unknown for
            // these rows (the legacy store has no per-entry Instant).
            let store = self.directory_store.lock().unwrap();
            store
                .list_active()
                .into_iter()
                .map(|(ad, _author)| {
                    let row = DiscoverRoomRow {
                        room_id: *ad.topic.as_bytes(),
                        room_name: ad.room_name.clone(),
                        short_description: ad.description.clone(),
                        tags: Vec::new(),
                        room_protocol_version: 0,
                        owner_peer_id: [0u8; 32],
                        member_count: Some(ad.member_count),
                        compatibility: boru_core::room_directory::RoomCompatibility::Compatible,
                        feature_compat: boru_core::room_directory::RoomFeatureCompatibility::None,
                        local_join_state: boru_core::room_directory::LocalJoinState::NotJoined,
                        offered_action: boru_core::room_directory::RoomAction::Join,
                        conflict: false,
                    };
                    (row, None)
                })
                .collect()
        };

        // The running application still receives legacy relay-directory
        // advertisements from main.rs while newer control-plane discovery is
        // enabled. Keep both discovery surfaces visible until all publishers
        // have migrated; otherwise an advertisement can be present in the
        // persisted legacy store but absent from the Discover page.
        if self.room_directory.is_some() {
            let known_topics: std::collections::HashSet<[u8; 32]> =
                rows.iter().map(|(row, _)| row.room_id).collect();
            let store = self.directory_store.lock().unwrap();
            for (ad, _author) in store.list_active() {
                if known_topics.contains(ad.topic.as_bytes()) {
                    continue;
                }
                let row = DiscoverRoomRow {
                    room_id: *ad.topic.as_bytes(),
                    room_name: ad.room_name,
                    short_description: ad.description,
                    tags: Vec::new(),
                    room_protocol_version: 0,
                    owner_peer_id: [0u8; 32],
                    member_count: Some(ad.member_count),
                    compatibility: boru_core::room_directory::RoomCompatibility::Compatible,
                    feature_compat: boru_core::room_directory::RoomFeatureCompatibility::None,
                    local_join_state: boru_core::room_directory::LocalJoinState::NotJoined,
                    offered_action: boru_core::room_directory::RoomAction::Join,
                    conflict: false,
                };
                rows.push((row, None));
            }
        }

        let total_count = rows.len();

        // Tag chips come from the FULL cache (before search/filter), so a
        // query that empties the list never hides the category chips.
        let available_tags = discover_available_tags(
            &rows.iter().map(|(row, _)| row.clone()).collect::<Vec<_>>(),
        );

        // Drop selected tags that no longer exist in the cache (e.g. after
        // an advertisement expires) so a stale selection can't silently
        // empty the list with no visible way to clear it.
        let selected_tags: Vec<String> = self
            .discover_selected_tags
            .iter()
            .filter(|tag| available_tags.iter().any(|t| t == *tag))
            .cloned()
            .collect();

        let rooms = discover_filter_sort(
            rows,
            &self.discover_search_query,
            DiscoverFilterState {
                compatible: self.discover_filter_compatible,
                not_joined: self.discover_filter_not_joined,
                recently_seen: self.discover_filter_recently_seen,
            },
            &selected_tags,
            self.discover_sort,
            now,
        );

        DiscoverDependency {
            dark_mode: self.dark_mode,
            theme_revision: self.theme_revision,
            layout_revision: self.layout_revision,
            responsive_mode: {
                let layout = self.boru_layout();
                let sidebar_width = layout
                    .sidebar
                    .width_for_window(self.window_width, &layout.responsive);
                let available_width = (self.window_width - sidebar_width - 1.0).max(0.0);
                if available_width <= layout.responsive.viewport_min_width {
                    crate::layout::ViewportTier::Narrow
                } else {
                    layout.responsive.tier_for_width(available_width)
                }
            },
            max_content_width_bits: self
                .boru_layout()
                .screens
                .get("discover")
                .map(|screen| screen.max_content_width.to_bits())
                .unwrap_or(crate::design_tokens::CONTENT_MAX_WIDTH.to_bits()),
            rooms,
            search_query: self.discover_search_query.clone(),
            filter_compatible: self.discover_filter_compatible,
            filter_not_joined: self.discover_filter_not_joined,
            filter_recently_seen: self.discover_filter_recently_seen,
            selected_tags,
            available_tags,
            sort: self.discover_sort,
            total_count,
            ticket_input: self.discover_ticket_input.clone(),
            ticket_error: self.discover_ticket_error.clone(),
        }
    }

    /// Static renderer for the Discover screen, driven by [`DiscoverDependency`].
    ///
    /// The screen is the public-room directory **browse surface** (PDF Phase
    /// 5). It is deliberately separate from the conversation list: cards show
    /// advertised metadata plus the local relationship verdict
    /// (Join/Open/Incompatible) as a label. Join wiring is BORU-DIR-16 —
    /// opening the directory never subscribes to a room topic or changes
    /// membership (PDF Task 5.1 acceptance).
    pub(crate) fn view_discover_content(dep: &DiscoverDependency) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, text, Column, Row, Space};
        use iced::{Alignment, Length};

        let header = Row::new()
            .push(
                button(
                    Row::new()
                        .push(icon_svg(ICON_CHAT, TYPO_SM))
                        .push(text(crate::i18n::t("common.back")).size(TYPO_SM))
                        .spacing(SPACE_4)
                        .align_y(Alignment::Center),
                )
                .on_press(AppMessage::CloseDiscover)
                .padding([SPACE_6, SPACE_12])
                .style(BUTTON_GHOST_BG),
            )
            .push(text(crate::i18n::t("discover.public_rooms_title")).size(TYPO_LG))
            .push(
                button(
                    Row::new()
                        .push(text("↻").size(TYPO_SM))
                        .push(text(crate::i18n::t("discover.refresh_registry")).size(TYPO_SM))
                        .spacing(SPACE_4)
                        .align_y(Alignment::Center),
                )
                .on_press(AppMessage::RefreshRoomRegistry)
                .padding([SPACE_6, SPACE_12])
                .style(BUTTON_GHOST_BG),
            )
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        // ── BORU-DIR-15 (PDF Task 5.3): search / filter / sort controls ──
        // All controls mutate local UI state only; the search query is
        // applied to the LOCAL RoomDirectory cache and never broadcast.
        let controls = Self::discover_controls(dep);

        let mut main_content = Column::new().spacing(SPACE_8).padding(SPACE_16);

        let ticket_input = iced::widget::text_input(
            "Paste a public room ticket",
            &dep.ticket_input,
        )
        .on_input(AppMessage::DiscoverTicketInputChanged)
        .on_submit(AppMessage::DiscoverJoinFromTicket)
        .width(Length::Fill)
        .padding([SPACE_6, SPACE_8]);
        let mut ticket_section = Column::new()
            .push(text("Join with a ticket").size(TYPO_MD))
            .push(
                Row::new()
                    .push(ticket_input)
                    .push(
                        button(text("Join").size(TYPO_SM))
                            .on_press(AppMessage::DiscoverJoinFromTicket)
                            .padding([SPACE_6, SPACE_12])
                            .style(BUTTON_PRIMARY),
                    )
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center),
            )
            .spacing(SPACE_4);
        if !dep.ticket_error.is_empty() {
            ticket_section = ticket_section.push(
                text(dep.ticket_error.clone())
                    .size(TYPO_SM)
                    .style(text_muted_style),
            );
        }
        main_content = main_content.push(container(ticket_section).width(Length::Fill));

        let rooms = &dep.rooms;

        if dep.total_count == 0 {
            main_content = main_content.push(
                container(
                    Column::new()
                        .push(
                            text(crate::i18n::t("discover.no_public_rooms_yet"))
                                .size(TYPO_MD)
                                .style(text_muted_style),
                        )
                        .push(Space::new().height(SPACE_8))
                        .push(
                            text(crate::i18n::t("discover.rooms_appear_hint"))
                                .size(TYPO_SM)
                                .style(text_muted_style),
                        )
                        .spacing(SPACE_4)
                        .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .center_x(Length::Fill)
                .padding(SPACE_16),
            );
        } else if rooms.is_empty() {
            // The directory has rooms, but search/filters matched none.
            // Deliberately does NOT claim global completeness: this only
            // says the local cache has no match right now.
            main_content = main_content.push(
                container(
                    Column::new()
                        .push(
                            text("No rooms match your search or filters.")
                                .size(TYPO_MD)
                                .style(text_muted_style),
                        )
                        .push(Space::new().height(SPACE_8))
                        .push(
                            text("Try clearing the search or filters to see more of your local directory.")
                                .size(TYPO_SM)
                                .style(text_muted_style),
                        )
                        .spacing(SPACE_4)
                        .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .center_x(Length::Fill)
                .padding(SPACE_16),
            );
        } else {
            for room in rooms {
                main_content =
                    main_content.push(Self::render_discover_room_card(room, dep.dark_mode));
            }
        }

        let body = Column::new()
            .push(header)
            .push(controls)
            .push(
                crate::ui_components::gutter_scrollable(main_content)
                    .height(Length::Fill)
                    .width(Length::Fill),
            )
            .spacing(SPACE_8);

        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .max_width(f32::from_bits(dep.max_content_width_bits))
            .style(container_primary)
            .into()
    }

    /// BORU-DIR-15 (PDF Task 5.3): the search box, filter chips, tag
    /// chips, sort selector, and result-count line for the Discover
    /// browse surface. Pure render over [`DiscoverDependency`]; every
    /// control emits a local-only `AppMessage` that `update_discover`
    /// turns into UI state — never a network op.
    fn discover_controls(dep: &DiscoverDependency) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, text, text_input, Column, Row, Space};
        use iced::{Alignment, Background, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        let accent = accent_primary(&theme);
        let muted = text_muted(&theme);
        let border = border_muted(&theme);

        // Active chip style: accent fill for an engaged filter/sort, ghost
        // otherwise. Active = on-press toggles it OFF, so the chip must
        // clearly read as selected. Only Copy values are captured, so the
        // style closure can be `move` (the rendered element is 'static).
        let chip = |label: String, active: bool, msg: AppMessage| -> iced::Element<'static, AppMessage> {
            button(text(label).size(TYPO_XS).color(if active { Color::WHITE } else { muted }))
                .on_press(msg)
                .padding([SPACE_4, SPACE_8])
                .style(move |_t: &iced::Theme, _status| {
                    if active {
                        iced::widget::button::Style {
                            background: Some(Background::Color(accent)),
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
                            text_color: muted,
                            border: iced::Border {
                                color: border,
                                width: crate::theme::BoruTheme::for_theme(_t).borders.hairline,
                                radius: SPACE_6.into(),
                            },
                            ..Default::default()
                        }
                    }
                })
                .into()
        };

        // Search row: input + clear button when non-empty.
        let clear: iced::Element<'static, AppMessage> = if dep.search_query.is_empty() {
            Space::new().width(Length::Fixed(0.0)).into()
        } else {
            button(text("✕").size(TYPO_XS).color(muted))
                .on_press(AppMessage::DiscoverSearchChanged(String::new()))
                .padding([SPACE_4, SPACE_6])
                .style(BUTTON_GHOST_BG)
                .into()
        };
        let search_row = Row::new()
            .push(
                text_input("Search rooms…", &dep.search_query)
                    .on_input(AppMessage::DiscoverSearchChanged)
                    .padding([SPACE_6, SPACE_10])
                    .size(TYPO_SM)
                    .width(Length::Fill),
            )
            .push(clear)
            .spacing(SPACE_4)
            .align_y(Alignment::Center);

        let narrow = dep.responsive_mode == crate::layout::ViewportTier::Narrow;
        // Filter actions stack in the narrow tier instead of forcing a dense
        // horizontal strip through the available content width.
        let filter_row: iced::Element<'static, AppMessage> = if narrow {
            Column::new()
                .push(chip(
                    "Compatible".to_string(),
                    dep.filter_compatible,
                    AppMessage::DiscoverFilterToggled(DiscoverFilter::Compatible),
                ))
                .push(chip(
                    "Not joined".to_string(),
                    dep.filter_not_joined,
                    AppMessage::DiscoverFilterToggled(DiscoverFilter::NotJoined),
                ))
                .push(chip(
                    "Recently seen".to_string(),
                    dep.filter_recently_seen,
                    AppMessage::DiscoverFilterToggled(DiscoverFilter::RecentlySeen),
                ))
                .spacing(SPACE_4)
                .into()
        } else {
            Row::new()
                .push(chip(
                    "Compatible".to_string(),
                    dep.filter_compatible,
                    AppMessage::DiscoverFilterToggled(DiscoverFilter::Compatible),
                ))
                .push(chip(
                    "Not joined".to_string(),
                    dep.filter_not_joined,
                    AppMessage::DiscoverFilterToggled(DiscoverFilter::NotJoined),
                ))
                .push(chip(
                    "Recently seen".to_string(),
                    dep.filter_recently_seen,
                    AppMessage::DiscoverFilterToggled(DiscoverFilter::RecentlySeen),
                ))
                .spacing(SPACE_4)
                .align_y(Alignment::Center)
                .into()
        };

        let mut controls = Column::new().spacing(SPACE_6).padding(iced::Padding {
            top: 0.0,
            right: SPACE_16,
            bottom: 0.0,
            left: SPACE_16,
        });

        controls = controls.push(search_row);
        controls = controls.push(filter_row);

        // Tag/category chips (only when the cache has any tags).
        if !dep.available_tags.is_empty() {
            let mut tag_row = Row::new().spacing(SPACE_4).align_y(Alignment::Center);
            for tag in dep.available_tags.iter().take(12) {
                let selected = dep.selected_tags.contains(tag);
                tag_row = tag_row.push(chip(
                    format!("#{tag}"),
                    selected,
                    AppMessage::DiscoverTagToggled(tag.clone()),
                ));
            }
            if dep.available_tags.len() > 12 {
                tag_row = tag_row.push(
                    text(format!("+{}", dep.available_tags.len() - 12))
                        .size(TYPO_XS)
                        .style(text_muted_style),
                );
            }
            controls = controls.push(tag_row);
        }

        // Sort selector.
        let sort_row = Row::new()
            .push(text("Sort:").size(TYPO_XS).style(text_muted_style))
            .push(chip(
                "Recently seen".to_string(),
                matches!(dep.sort, DiscoverSort::RecentlySeen),
                AppMessage::DiscoverSortChanged(DiscoverSort::RecentlySeen),
            ))
            .push(chip(
                "Compatibility".to_string(),
                matches!(dep.sort, DiscoverSort::Compatibility),
                AppMessage::DiscoverSortChanged(DiscoverSort::Compatibility),
            ))
            .push(chip(
                "Name".to_string(),
                matches!(dep.sort, DiscoverSort::Name),
                AppMessage::DiscoverSortChanged(DiscoverSort::Name),
            ))
            .spacing(SPACE_4)
            .align_y(Alignment::Center);
        controls = controls.push(sort_row);

        // Result count — "N of M" against the LOCAL cache, never a claim
        // about the whole network (PDF Task 5.3 guardrail: "Do not imply
        // the directory contains every Boru room").
        if dep.total_count > 0 {
            let shown = dep.rooms.len();
            let count_text = if shown == dep.total_count {
                format!("{shown} locally discovered room{}", if shown == 1 { "" } else { "s" })
            } else {
                format!(
                    "Showing {shown} of {} locally discovered rooms",
                    dep.total_count
                )
            };
            controls = controls.push(
                container(
                    text(count_text)
                        .size(TYPO_XS)
                        .style(text_muted_style)
                        .width(Length::Fill),
                )
                .padding(iced::Padding {
                    top: SPACE_2,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                }),
            );
        }

        controls.into()
    }

    // ── Room card (PDF Task 5.2) ─────────────────────────────────────
    // Display bounds for directory-card text. Every text field is elided to
    // these lengths AND wrapped (`WordOrGlyph`), so even a hostile
    // advertisement with oversized metadata can never push the card wider
    // than its column or split a multi-byte character (PDF Task 5.2
    // acceptance: "Oversized text cannot break layout").
    /// Render one room card in the Discover browse surface (PDF Task 5.2).
    ///
    /// Card layout (top to bottom):
    ///   1. room name (elided + wrapped) with the Join/Open action button
    ///      on the right;
    ///   2. short description (elided + wrapped; hidden when empty);
    ///   3. tag pills (hidden when empty, capped with a "+N" overflow pill);
    ///   4. meta row: compatibility label + approximate member count
    ///      (hidden when absent) + "Unverified" marker for contested ads.
    ///
    /// A minimal advertisement (empty description, no tags, no count) still
    /// renders a correct card: every optional field degrades to nothing.
    /// The action button is wired (BORU-DIR-16): Join dispatches the
    /// directory join path, Open dispatches the normal room-open path —
    /// opening the directory itself never changes membership (PDF Task
    /// 5.1).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn render_discover_room_card(
        room: &DiscoverRoomRow,
        dark_mode: bool,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, text, Column, Row};
        use iced::{Alignment, Background, Length};

        let theme = Self::theme_from_dark(dark_mode);

        // ── Header: room name + action button ──
        let name = text(discover_elide(&room.room_name, DISCOVER_MAX_NAME_CHARS))
            .size(TYPO_MD)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .width(Length::Fill);

        // BORU-DIR-16 (PDF Task 6.1): the action button only *appears*
        // here; pressing it is the ONLY way the directory changes local
        // membership. The advertised room_id IS the room's gossip topic
        // (the deterministic identity from topic_derivation), so joining
        // routes through the normal public-room join path (OpenRoom),
        // which creates the conversation record exactly once and
        // subscribes via normal room-topic logic.
        let topic = TopicId::from_bytes(room.room_id);
        let action: iced::Element<'static, AppMessage> = match room.offered_action {
            boru_core::room_directory::RoomAction::Join => button(
                text(discover_action_label(room.offered_action))
                    .size(TYPO_XS)
                    .color(Color::WHITE),
            )
            .on_press(AppMessage::DirectoryRoomJoinById(room.room_id))
            .padding([SPACE_4, SPACE_10])
            .style(BUTTON_PRIMARY)
            .into(),
            boru_core::room_directory::RoomAction::Open => button(
                text(discover_action_label(room.offered_action)).size(TYPO_XS),
            )
            .on_press(AppMessage::OpenRoom(topic))
            .padding([SPACE_4, SPACE_10])
            .style(BUTTON_GHOST_BG)
            .into(),
            boru_core::room_directory::RoomAction::Incompatible => {
                let label = discover_compat_label(room.compatibility);
                button(text(label).size(TYPO_XS).color(Color::WHITE))
                    .padding([SPACE_4, SPACE_10])
                    .style(BUTTON_DANGER)
                    .into()
            }
            boru_core::room_directory::RoomAction::Hidden => {
                text("Hidden").size(TYPO_XS).style(text_muted_style).into()
            }
        };

        let mut header = Row::new()
            .push(name)
            .push(action)
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        // BORU-DIR-20 (PDF Task 7.2): Hide Room control on every offered
        // card. Pressing it persists the hide preference locally and the
        // card disappears from Discover (the cache derives it Blocked on
        // the next sync). Local-only: nothing is broadcast, membership is
        // untouched, and hidden rooms are restored from Settings →
        // Hidden rooms (never by the network).
        if !matches!(room.offered_action, boru_core::room_directory::RoomAction::Hidden) {
            header = header.push(
                button(text("Hide").size(TYPO_XS))
                    .on_press(AppMessage::DirectoryRoomHideById(room.room_id))
                    .padding([SPACE_4, SPACE_10])
                    .style(BUTTON_GHOST),
            );
        }

        let mut body = Column::new().spacing(SPACE_4);
        body = body.push(header);

        // ── Description (optional) ──
        if !room.short_description.is_empty() {
            body = body.push(
                text(discover_elide(&room.short_description, DISCOVER_MAX_DESC_CHARS))
                    .size(TYPO_SM)
                    .style(text_muted_style)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                    .width(Length::Fill),
            );
        }

        // ── Tags (optional) ──
        if !room.tags.is_empty() {
            let mut tags_row = Row::new().spacing(SPACE_4);
            for tag in room.tags.iter().take(DISCOVER_MAX_TAGS_SHOWN) {
                tags_row = tags_row.push(crate::ui_components::badge_owned(
                    format!("#{}", discover_elide(tag, DISCOVER_MAX_TAG_CHARS)),
                    crate::ui_components::BadgeKind::Default,
                ));
            }
            if room.tags.len() > DISCOVER_MAX_TAGS_SHOWN {
                tags_row = tags_row.push(crate::ui_components::badge_owned(
                    format!("+{}", room.tags.len() - DISCOVER_MAX_TAGS_SHOWN),
                    crate::ui_components::BadgeKind::Default,
                ));
            }
            body = body.push(tags_row);
        }

        // ── Meta row: compatibility + approximate member count + conflict ──
        let mut meta = Row::new().spacing(SPACE_8).align_y(Alignment::Center);
        let compat_label = discover_compat_label(room.compatibility);
        let compat_color = match room.compatibility {
            boru_core::room_directory::RoomCompatibility::Compatible => text_muted(&theme),
            boru_core::room_directory::RoomCompatibility::UpgradeRequired => {
                crate::design_tokens::color_warning(&theme)
            }
            boru_core::room_directory::RoomCompatibility::Unsupported => color_error(&theme),
            boru_core::room_directory::RoomCompatibility::Unknown => text_muted(&theme),
        };
        meta = meta.push(text(compat_label).size(TYPO_XS).color(compat_color));
        if let Some(count_text) = discover_member_count_text(room.member_count) {
            meta = meta.push(text(count_text).size(TYPO_XS).style(text_muted_style));
        }
        if room.conflict {
            // BORU-DIR-11: contested metadata must be shown as unverified,
            // never silently trusted.
            meta = meta.push(
                text("Unverified")
                    .size(TYPO_XS)
                    .color(crate::design_tokens::color_warning(&theme)),
            );
        }
        body = body.push(meta);

        // ── Optional-feature hint (PDF Task 6.2 step 2) ──
        // Informational only: a room whose base protocol is Compatible
        // stays joinable even when some advertised optional features are
        // missing locally. The hint is muted and never blocks the action.
        if let Some(hint) = discover_feature_hint(&room.feature_compat) {
            body = body.push(
                text(hint)
                    .size(TYPO_XS)
                    .style(text_muted_style)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            );
        }

        container(body)
            .padding(SPACE_12)
            .width(Length::Fill)
            .style(move |t| container::Style {
                background: Some(Background::Color(bg_surface(t))),
                border: iced::Border {
                    radius: SPACE_8.into(),
                    color: border_muted(&theme),
                    width: crate::theme::BoruTheme::for_theme(t).borders.hairline,
                },
                ..Default::default()
            })
            .into()
    }
    /// Redesigned friend profile view with clean layout, context menu, and action buttons.
    /// Redesigned friend profile view with clean layout, context menu, and action buttons.
    ///
    /// The deterministic base content (header, status, shared files, recent
    /// messages, shared services, actions) is cached with `lazy()` keyed on a
    /// Hash snapshot. The transient overlays (three-dot menu, confirm dialogs,
    /// share dialog, toast) layer on top and re-render every frame because they
    /// contain live text inputs and combo boxes.
    pub(crate) fn view_friend_profile(&self, peer: PublicKey) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, text, Column, Space};
        use iced::Length;

        let dep = self.friend_profile_dependency(peer);
        let display_name = dep.display_name.clone();
        let dark_mode = dep.dark_mode;
        // The base MUST be Fill×Fill (matching every other screen's base in
        // `view()`).  `iced::widget::lazy` reports a Shrink size hint
        // regardless of its content, so without an explicit Fill size the
        // transient overlays stacked over this base (share-local-service
        // dialog, remove/block confirms, toast) get clipped to the base's
        // computed Shrink bounds — the dialog panel renders top-anchored and
        // its lower fields (Local port, expiry, footer) are cut off.
        let base: iced::widget::Container<'_, AppMessage> = iced::widget::container(
            iced::widget::lazy(dep, move |dep| Self::view_friend_profile_content(dep, peer)),
        )
        .width(iced::Length::Fill)
        .height(iced::Length::Fill);

        // ── Three-dot context menu overlay ──
        if self.friend_profile_menu_open {
            let menu_items: Vec<(&str, AppMessage)> = vec![
                ("View Profile", AppMessage::ToggleFriendProfileMenu),
                ("Browse Files", AppMessage::BrowsePeerCatalogue(peer)),
                ("Rename Friend", AppMessage::ShowRenameFriendInput),
                ("Share local service", AppMessage::OpenShareLocalService),
                #[cfg(feature = "experimental-vnc")]
                ("Share desktop using VNC Tunnel", AppMessage::OpenShareVncTunnel),
                ("Copy Public Key", AppMessage::CopyPeerId(peer)),
                ("Remove Friend", AppMessage::ShowRemoveFriendConfirm),
                ("Block Friend", AppMessage::ShowBlockFriendConfirm),
            ];

            let mut menu_col = Column::new()
                .spacing(SPACE_2)
                .padding(SPACE_4)
                .width(Length::Fixed(
                    // BORU-UI-07: live merged theme (boru-ui.toml override
                    // renders immediately after a reload).
                    self.boru_theme().rooms.banner_width,
                ));

            for (label, msg) in &menu_items {
                let is_destructive = *label == "Remove Friend" || *label == "Block Friend";
                let item = button(text(*label).size(TYPO_SM).color(if is_destructive {
                    // BORU-UI-03: the destructive red rgb(0.8,0.2,0.2) is
                    // captured by ColorTokens::request_declined in both modes.
                    crate::theme::BoruTheme::for_theme(&Self::theme_from_dark(dark_mode))
                        .colors
                        .request_declined
                } else {
                    text_remote_body(&Self::theme_from_dark(dark_mode))
                }))
                .on_press(msg.clone())
                .width(Length::Fill)
                .padding([SPACE_6, SPACE_8])
                .style(move |_t, status| {
                    let bg = match status {
                        iced::widget::button::Status::Hovered => {
                            iced::Color::from_rgba(0.3, 0.3, 0.3, 0.3)
                        }
                        _ => iced::Color::TRANSPARENT,
                    };
                    iced::widget::button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            radius: SPACE_4.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                });
                menu_col = menu_col.push(item);
            }

            let menu_panel = container(menu_col)
                .style(move |t| {
                    let b = crate::theme::BoruTheme::for_theme(t);
                    iced::widget::container::Style {
                        background: Some(iced::Background::Color(bg_surface(t))),
                        border: iced::Border {
                            color: border_muted(t),
                            width: b.borders.hairline,
                            radius: b.radii.sm.into(),
                        },
                        ..Default::default()
                    }
                })
                .padding(SPACE_4);

            // Position menu in top-right area — we push it into the header area
            let menu_overlay = container(menu_panel)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::Alignment::End)
                .align_y(iced::Alignment::Start)
                .padding(iced::Padding {
                    top: 60.0,
                    right: 12.0,
                    bottom: 0.0,
                    left: 0.0,
                });

            // Click-outside handler: the full backdrop closes the menu
            let backdrop = button(Space::new().width(Length::Fill).height(Length::Fill))
                .on_press(AppMessage::ToggleFriendProfileMenu)
                .style(|_t, _status| iced::widget::button::Style {
                    background: None,
                    border: iced::Border::default(),
                    text_color: iced::Color::TRANSPARENT,
                    ..Default::default()
                });

            return iced::widget::stack![base, backdrop, menu_overlay].into();
        }

        // ── Confirmation dialogs ──
        if self.friend_remove_confirm {
            return self.view_remove_confirm_overlay(peer, &display_name, base);
        }
        if self.friend_block_confirm {
            return self.view_block_confirm_overlay(peer, &display_name, base);
        }
        if self.tunnels_state.share_local_service_open {
            return self.view_share_local_service_dialog(peer, display_name.clone(), base);
        }

        // ── Toast overlay (BORU-APP-004: notifications-domain view) ──
        if let Some(msg) = &self.notifications_state.toast_message {
            return view_toast(base, msg);
        }

        base.into()
    }

    /// Build the Hash-compatible snapshot the Friend Profile base content
    /// renders from. Every field is owned and Hash so `lazy()` can diff it.
    pub(crate) fn friend_profile_dependency(&self, peer: PublicKey) -> FriendProfileDependency {
        let fid = boru_core::friends::FriendId::from_public_key(peer);
        let friend_record = self.friends.get(&fid);
        let profile_data = self.profile_cache.get(&peer);
        let display_name = profile_data
            .as_ref()
            .map(|p| p.display_name.clone())
            .or_else(|| friend_record.map(|r| r.display_label(&fid, &peer)))
            .unwrap_or_else(|| "Unknown Friend".to_string());

        let presence = self.peer_presence(&peer);
        let has_addrs = friend_record
            .map(|r| !r.known_addrs.is_empty())
            .unwrap_or(false);

        // Check for shared catalogue files
        let has_catalogue = self.files_state
            .peer_catalogue_view
            .as_ref()
            .is_some_and(|(pk, files)| *pk == peer && !files.is_empty());

        // Get recent messages from chat_history for this friend's conversation
        let recent_messages: Vec<String> = {
            let topic = friend_record
                .and_then(|r| r.direct_conversation())
                .map(|dc| dc.topic);
            if let Some(t) = topic {
                let history = self.chat_history.lock().unwrap();
                let entries = history.for_topic(&t);
                entries
                    .iter()
                    .rev()
                    .take(3)
                    .map(|e| {
                        let text = e.text_preview.trim();
                        if text.len() > 80 {
                            format!("{}…", &text[..77])
                        } else {
                            text.to_string()
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            }
        };

        // Received shared services (tunnels) from this friend, pre-rendered
        // into Hash rows so the static content fn needs no live state.
        let shared_services = self
            .tunnels_state
            .received_tunnels
            .values()
            .filter(|state| state.sharer == peer)
            .map(|state| {
                let route_label = state.live_info.as_ref().map(|live| {
                    let snapshot = live.snapshot();
                    tunnel_route_label(snapshot.route).to_string()
                });
                let local_addr = state
                    .local_addr
                    .map(|addr| tunnel_local_address(&state.offer, addr));
                FriendProfileServiceRow {
                    id: state.offer.tunnel_id,
                    service_name: state.offer.service_name.clone(),
                    sharer_label: state.sharer_label.clone(),
                    is_http: state.offer.is_http,
                    expired: state.offer.expires_at_ms <= now_ms().max(0) as u64,
                    connection_failed: state.connection_failed,
                    connected: state.connected,
                    route_label,
                    local_addr,
                    expiry: tunnel_expiry_label(state.offer.expires_at_ms),
                }
            })
            .collect();

        FriendProfileDependency {
            dark_mode: self.dark_mode,
            theme_revision: self.theme_revision,
            peer,
            display_name,
            presence,
            has_addrs,
            friend_profile_rename_input: self.friend_profile_rename_input.clone(),
            friend_profile_renaming: self.friend_profile_renaming,
            has_catalogue,
            recent_messages,
            shared_services,
        }
    }

    /// Static renderer for the Friend Profile base content. Reads only from the
    /// [`FriendProfileDependency`] snapshot (plus the peer key for messages).
    pub(crate) fn view_friend_profile_content(
        dep: &FriendProfileDependency,
        peer: PublicKey,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, row, text, text_input, Column, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        let dark_mode = dep.dark_mode;
        let display_name = dep.display_name.clone();
        let presence = dep.presence;
        let is_online = presence != PeerPresence::Offline;
        let has_addrs = dep.has_addrs;
        let last_seen_str = if is_online {
            if has_addrs {
                "Connected locally.".to_string()
            } else {
                "Online".to_string()
            }
        } else {
            "Offline".to_string()
        };

        let has_catalogue = dep.has_catalogue;
        let recent_messages = &dep.recent_messages;

        // ── Header row: name (or rename input) + three-dot menu + close ──
        let name_element: iced::Element<'static, AppMessage> = if dep.friend_profile_renaming {
            row![]
                .push(
                    text_input(&crate::i18n::t("discover.friend_name_placeholder"), &dep.friend_profile_rename_input)
                        .on_input(AppMessage::FriendRenameInputChanged)
                        .on_submit(AppMessage::FriendRenameConfirm)
                        .size(TYPO_MD)
                        .padding([SPACE_4, SPACE_8])
                        .width(Length::Fill),
                )
                .push(
                    button(text("✓").size(TYPO_SM))
                        .on_press(AppMessage::FriendRenameConfirm)
                        .padding([SPACE_4, SPACE_8])
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
                    button(icon_svg(ICON_CLOSE, TYPO_SM))
                        .on_press(AppMessage::FriendRenameConfirm)
                        .padding([SPACE_4, SPACE_8])
                        .style(move |t, _status| iced::widget::button::Style {
                            text_color: text_muted(t),
                            ..Default::default()
                        }),
                )
                .spacing(SPACE_4)
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .into()
        } else {
            // FONTS-09: the friend's display name (which may be a raw short
            // key like "6c0f88fe9f") is a user-facing name here, so it uses
            // the IBM Plex Sans display-name role (SectionTitle) — matching
            // the peer profile header — never JetBrains Mono.
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::SectionTitle,
                display_name.clone(),
            )
            .width(Length::Fill)
            .into()
        };

        let header = row![]
            // FILES-04: explicit back button returning to the previous
            // screen (File Sharing dashboard when opened from there).
            .push(
                button(
                    row![
                        Icon::Back.build().size(IconSize::Sm).build(),
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Back",
                        ),
                    ]
                    .spacing(SPACE_4)
                    .align_y(Alignment::Center),
                )
                .on_press(AppMessage::CloseFriendProfile)
                .padding([SPACE_4, SPACE_8])
                .style(BUTTON_GHOST_BG),
            )
            .push(name_element)
            .push(
                button(Icon::MoreVertical.build().size(IconSize::Md).build())
                    .on_press(AppMessage::ToggleFriendProfileMenu)
                    .padding([SPACE_4, SPACE_8])
                    .style(move |t, status| iced::widget::button::Style {
                        text_color: if matches!(status, iced::widget::button::Status::Hovered) {
                            accent_primary(t)
                        } else {
                            text_muted(t)
                        },
                        ..Default::default()
                    }),
            )
            .push(
                button(icon_svg(ICON_CLOSE, TYPO_MD))
                    .on_press(AppMessage::CloseFriendProfile)
                    .padding([SPACE_4, SPACE_8])
                    .style(move |t, _status| iced::widget::button::Style {
                        text_color: text_muted(t),
                        ..Default::default()
                    }),
            )
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        let header = container(header)
            .width(Length::Fill)
            .padding(iced::Padding {
                top: SPACE_12,
                right: SPACE_12,
                bottom: SPACE_4,
                left: SPACE_12,
            });

        // ── Status section ──
        let status_color = presence.color(&theme);
        let status_row = row![]
            .push(icon_svg(presence.icon(), TYPO_SM).style(move |_t, _s| {
                iced::widget::svg::Style {
                    color: Some(status_color),
                }
            }))
            .push(
                text(last_seen_str.clone())
                    .size(TYPO_SM)
                    .style(text_muted_style),
            )
            .spacing(SPACE_6)
            .align_y(Alignment::Center);

        let status_section = container(status_row)
            .width(Length::Fill)
            .padding(iced::Padding {
                top: SPACE_2,
                right: SPACE_12,
                bottom: SPACE_8,
                left: SPACE_12,
            });

        // ── Shared Files section ──
        let shared_files_label = row![]
            .push(text(crate::i18n::t("discover.shared_files")).size(TYPO_SM).width(Length::Fill))
            .push(
                button(text(crate::i18n::t("files.browse_short")).size(TYPO_XS))
                    .on_press(AppMessage::BrowsePeerCatalogue(peer))
                    .padding([SPACE_2, SPACE_6])
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
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        let shared_files_body = if has_catalogue {
            row![]
                .push(
                    text(crate::i18n::t("discover.files_available"))
                        .size(TYPO_XS)
                        .style(text_muted_style),
                )
                .spacing(0)
        } else {
            row![]
                .push(
                    text(crate::i18n::t("discover.no_shared_files"))
                        .size(TYPO_XS)
                        .style(text_muted_style),
                )
                .spacing(0)
        };
        let shared_files_section = container(
            Column::new()
                .push(shared_files_label)
                .push(Space::new().height(SPACE_4))
                .push(shared_files_body)
                .spacing(SPACE_2),
        )
        .width(Length::Fill)
        .padding(SPACE_12)
        .style(container_surface);

        // ── Recent Messages section ──
        let recent_header = text(crate::i18n::t("discover.recent_messages")).size(TYPO_SM).width(Length::Fill);

        let recent_body: iced::Element<'static, AppMessage> = if recent_messages.is_empty() {
            text(crate::i18n::t("discover.no_recent_messages"))
                .size(TYPO_XS)
                .style(text_muted_style)
                .into()
        } else {
            let mut col = Column::new().spacing(SPACE_4);
            for msg in recent_messages {
                col = col.push(text(msg.clone()).size(TYPO_XS).style(text_muted_style));
            }
            // Make entire section clickable to open chat
            let section_content = container(col).width(Length::Fill).padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            });
            button(section_content)
                .on_press(AppMessage::OpenFriendChat(peer))
                .width(Length::Fill)
                .padding(0)
                .style(|_t, _status| iced::widget::button::Style {
                    background: None,
                    border: iced::Border::default(),
                    text_color: iced::Color::TRANSPARENT,
                    ..Default::default()
                })
                .into()
        };

        let recent_section = container(
            Column::new()
                .push(recent_header)
                .push(Space::new().height(SPACE_4))
                .push(recent_body)
                .spacing(SPACE_2),
        )
        .width(Length::Fill)
        .padding(SPACE_12)
        .style(container_surface);

        // ── Action buttons ──
        let actions = row![]
            .push(
                button(text(crate::i18n::t("common.message")).size(TYPO_SM))
                    .on_press(AppMessage::OpenFriendChat(peer))
                    .padding([SPACE_8, SPACE_16])
                    .width(Length::Fill)
                    .style(move |t, _status| iced::widget::button::Style {
                        background: Some(iced::Background::Color(accent_primary(t))),
                        text_color: Color::WHITE,
                        border: iced::Border {
                            radius: SPACE_6.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            )
            .push(
                button(text(crate::i18n::t("common.files")).size(TYPO_SM))
                    .on_press(AppMessage::BrowsePeerCatalogue(peer))
                    .padding([SPACE_8, SPACE_16])
                    .width(Length::Fill)
                    .style(move |t, _status| {
                        let b = crate::theme::BoruTheme::for_theme(t);
                        iced::widget::button::Style {
                            background: Some(iced::Background::Color(bg_surface(t))),
                            text_color: text_remote_body(&Self::theme_from_dark(dark_mode)),
                            border: iced::Border {
                                color: border_muted(t),
                                width: b.borders.hairline,
                                radius: SPACE_6.into(),
                            },
                            ..Default::default()
                        }
                    }),
            )
            .push(
                button(text(crate::i18n::t("common.voice")).size(TYPO_SM))
                    .padding([SPACE_8, SPACE_16])
                    .width(Length::Fill)
                    .style(move |t, _status| {
                        let b = crate::theme::BoruTheme::for_theme(t);
                        iced::widget::button::Style {
                            background: Some(iced::Background::Color(bg_surface(t))),
                            text_color: Self::muted_color(dark_mode),
                            border: iced::Border {
                                color: border_muted(t),
                                width: b.borders.hairline,
                                radius: SPACE_6.into(),
                            },
                            ..Default::default()
                        }
                    }),
            )
            .spacing(SPACE_8);

        let actions_section = container(actions)
            .width(Length::Fill)
            .padding(iced::Padding {
                top: SPACE_8,
                right: SPACE_12,
                bottom: SPACE_12,
                left: SPACE_12,
            });

        // ── Build body ──
        let mut body = Column::new().spacing(SPACE_4);
        body = body.push(status_section);

        // ── Shared Services section (received tunnel offers) ──
        let shared_services = &dep.shared_services;
        if !shared_services.is_empty() {
            let mut services_col = Column::new().spacing(SPACE_4);
            for state in shared_services {
                let tunnel_id = state.id;
                let service_name = state.service_name.clone();
                let sharer_label = state.sharer_label.clone();
                let is_http = state.is_http;
                let expired = state.expired;
                let expiry = state.expiry.clone();
                let mut card = Column::new().spacing(SPACE_4);

                // Status badge: Connected (direct/relay), Failed, or Expired
                if expired {
                    card = card.push(text(crate::i18n::t("common.expired")).size(TYPO_XS).color(text_muted(&theme)));
                } else if state.connection_failed {
                    card = card.push(text(crate::i18n::t("common.failed")).size(TYPO_XS).color(color_error(&theme)));
                } else if state.connected {
                    let route = state.route_label.as_deref();
                    card = card.push(
                        text(match route {
                            Some("Direct") => crate::i18n::t("discover.connected_direct"),
                            Some("Relay") => crate::i18n::t("discover.connected_relay"),
                            Some(other) if !other.is_empty() => crate::i18n::t_args("discover.connected_route", &[("route", other)]),
                            _ => crate::i18n::t("common.connected"),
                        })
                        .size(TYPO_XS)
                        .color(accent_green(&theme)),
                    );
                }
                card = card.push(text(sharer_label).size(TYPO_XS).style(text_muted_style));
                card = card.push(text(service_name).size(TYPO_MD));

                if let Some(display) = &state.local_addr {
                    card = card.push(
                        text(crate::i18n::t_args("discover.available_at", &[("addr", display)]))
                            .size(TYPO_XS)
                            .style(text_muted_style),
                    );
                } else if !expired {
                    card = card.push(
                        text(format!("{expiry}"))
                            .size(TYPO_XS)
                            .style(text_muted_style),
                    );
                } else {
                    card = card.push(
                        text(crate::i18n::t("discover.service_expired"))
                            .size(TYPO_XS)
                            .style(text_muted_style),
                    );
                }

                let mut actions = row![].spacing(SPACE_6).align_y(Alignment::Center);
                if state.connected {
                    if is_http {
                        actions = actions.push(
                            button(text(crate::i18n::t("common.open")).size(TYPO_XS))
                                .on_press(AppMessage::OpenReceivedTunnel(tunnel_id))
                                .padding([SPACE_2, SPACE_8])
                                .style(move |t, _status| iced::widget::button::Style {
                                    background: Some(iced::Background::Color(accent_primary(t))),
                                    text_color: Color::WHITE,
                                    border: iced::Border {
                                        radius: SPACE_4.into(),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                        );
                    }
                    actions = actions.push(
                        button(text(crate::i18n::t("discover.copy_address")).size(TYPO_XS))
                            .on_press(AppMessage::CopyReceivedTunnelAddress(tunnel_id))
                            .padding([SPACE_2, SPACE_8]),
                    );
                    actions = actions.push(
                        button(text(crate::i18n::t("common.disconnect")).size(TYPO_XS))
                            .on_press(AppMessage::DisconnectReceivedTunnel(tunnel_id))
                            .padding([SPACE_2, SPACE_8]),
                    );
                } else if !expired {
                    actions = actions.push(
                        button(text(crate::i18n::t("common.connect")).size(TYPO_XS))
                            .on_press(AppMessage::ConnectReceivedTunnel(tunnel_id))
                            .padding([SPACE_2, SPACE_8])
                            .style(move |t, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(accent_primary(t))),
                                text_color: Color::WHITE,
                                border: iced::Border {
                                    radius: SPACE_4.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    );
                }
                card = card.push(actions);

                services_col =
                    services_col.push(container(card).width(Length::Fill).padding(SPACE_8).style(
                        move |t| iced::widget::container::Style {
                            background: Some(iced::Background::Color(bg_surface(t))),
                            border: iced::Border {
                                radius: SPACE_6.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                    ));
            }

            let shared_services_section = container(
                Column::new()
                    .push(text(crate::i18n::t("discover.shared_services")).size(TYPO_SM).width(Length::Fill))
                    .push(Space::new().height(SPACE_4))
                    .push(services_col)
                    .spacing(SPACE_2),
            )
            .width(Length::Fill)
            .padding(SPACE_12)
            .style(container_surface);

            body = body.push(shared_services_section);
        }

        // Separator line
        body = body.push(
            container(Space::new().height(1.0))
                .width(Length::Fill)
                .style(move |t| iced::widget::container::Style {
                    background: Some(iced::Background::Color(border_muted(t))),
                    ..Default::default()
                }),
        );

        body = body.push(shared_files_section);

        body = body.push(
            container(Space::new().height(1.0))
                .width(Length::Fill)
                .style(move |t| iced::widget::container::Style {
                    background: Some(iced::Background::Color(border_muted(t))),
                    ..Default::default()
                }),
        );

        body = body.push(recent_section);

        body = body.push(Space::new().height(Length::Fill));

        // ── Wrap in scrollable ──
        let content = Column::new().push(header).push(body).push(actions_section);

        container(crate::ui_components::gutter_scrollable(content))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(container_primary)
            .into()
    }

    /// State-layer update for discover / public directory (BORU-AUDIT-22
    /// spec step 5).
    ///
    /// Handles peer catalogue browsing, room advertisement/directory
    /// subscription, directory room join/delete/update. The root `update()`
    /// dispatches these variants here via combined match arms.
    pub(crate) fn update_discover(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::BrowsePeerCatalogue(peer) => {
                self.files_state.catalogue_loading = true;
                let endpoint = self.endpoint.clone();
                let storage = self.storage.clone();
                iced::Task::perform(
                    async move {
                        match fetch_paginated_remote_catalogue(&endpoint, peer, 500).await {
                            Ok(catalogue) => {
                                // Persist the validated, signed catalogue so the
                                // backend download gate (validate_download_request /
                                // initiate_download) can re-check the file against
                                // the verified snapshot at download time. This also
                                // makes the durable Downloading/Downloaded views
                                // consistent with what the user browsed.
                                if let Some(storage) = storage.as_ref() {
                                    // SQLite write — defer to the blocking pool
                                    // (BORU-AUDIT-18).
                                    let stg = storage.clone();
                                    let catalogue = catalogue.clone();
                                    if let Err(e) = stg
                                        .run_blocking("app.store_remote_catalogue", move |s| {
                                            boru_core::catalogue_client::process_and_store_remote_catalogue(
                                                s, &catalogue,
                                            )
                                            .map_err(|e| anyhow::anyhow!("{e}"))
                                        })
                                        .await
                                    {
                                        tracing::warn!(
                                            peer = %peer.fmt_short(),
                                            error = %e,
                                            "failed to persist validated remote catalogue"
                                        );
                                    }
                                }
                                let files = catalogue.files;
                                Ok((peer, files))
                            }
                            Err(e) => Err(e.to_string()),
                        }
                    },
                    |result| match result {
                        Ok((peer, files)) => AppMessage::PeerCatalogueReceived { peer, files },
                        Err(e) => AppMessage::PeerCatalogueFailed(e),
                    },
                )
            }
            AppMessage::PeerCatalogueReceived { peer, files } => {
                self.files_state.catalogue_loading = false;
                self.files_state.peer_catalogue_view = Some((peer, files));
                if !matches!(self.screen, Screen::PeerCatalogue(peer) | Screen::PeerProfile(peer)) {
                    self.peer_profile_return_to = Some(self.screen.clone());
                }
                self.screen = Screen::PeerCatalogue(peer);
                iced::Task::none()
            }
            AppMessage::PeerCatalogueFailed(error) => {
                self.files_state.catalogue_loading = false;
                self.push_system(format!("Catalogue fetch failed: {error}"));
                iced::Task::none()
            }
            AppMessage::CatalogueScrolled(offset, vp_h) => {
                self.files_state.catalogue_scroll_offset = offset;
                self.files_state.catalogue_viewport_height = vp_h;
                iced::Task::none()
            }


            AppMessage::ToggleAdvertiseRoom(topic) => {
                // BORU-DIR-06: the advertise toggle is now an owner/admin
                // directory-visibility switch. Compute the target visibility
                // from the current state and delegate to the gated switch
                // (which publishes immediately on discoverable, and stops
                // refreshing on unlisted — TTL expiry applies, no withdrawal
                // message yet).
                let target = if self.rooms_state.advertised_rooms.contains(&topic) {
                    RoomVisibility::PublicUnlisted
                } else {
                    RoomVisibility::PublicDiscoverable
                };
                self.apply_room_directory_visibility(topic, target)
            }
            AppMessage::SubscribeDirectoryTopic => {
                let gossip = self.gossip.clone();
                let topic = self.directory_topic;
                info!(%topic, "subscribing to directory topic");
                iced::Task::perform(
                    async move {
                        let sub = gossip
                            .subscribe(topic, vec![])
                            .await
                            .map_err(|e| e.to_string())?;
                        let (sender, _receiver) = sub.split();
                        Ok::<_, String>(sender)
                    },
                    |result| match result {
                        Ok(sender) => AppMessage::DirectorySubscribed(Some(sender)),
                        Err(e) => {
                            warn!("SubscribeDirectoryTopic failed: {e}");
                            AppMessage::DirectorySubscribed(None)
                        }
                    },
                )
            }
            AppMessage::DirectorySubscribed(sender) => {
                self.directory_sender = sender;
                if self.directory_sender.is_some() {
                    info!("directory topic subscribed");
                    // BORU-DIR-07 (PDF Task 3.1): after the discovery
                    // service is ready, publish one bounded advertisement
                    // per locally authorized PublicDiscoverable room so
                    // they reappear after a client restart. Non-blocking:
                    // failures are logged, not fatal.
                    let startup = self.publish_startup_room_advertisements();
                    // BORU-DIR-07 catch-up: also publish any room that was
                    // marked for advertising *after* the one-shot startup
                    // sweep (e.g. created or made discoverable before this
                    // subscription completed, or after a reconnect). The
                    // shared dedupe fingerprint makes this a no-op for
                    // rooms the startup sweep (or the create-time
                    // broadcast) just published.
                    let catch_up = self.publish_all_advertised_now();
                    return iced::Task::batch(vec![startup, catch_up]);
                } else {
                    warn!("directory topic subscription failed");
                }
                iced::Task::none()
            }
            // ── Room advertisement / Directory ──────────────────────────
            AppMessage::OpenDirectory => {
                if !matches!(self.screen, Screen::Discover) {
                    self.discover_return_to = Some(self.screen.clone());
                }
                self.screen = Screen::Discover;
                iced::Task::none()
            }
            AppMessage::CloseDiscover => {
                self.screen = self.discover_return_to.take().unwrap_or(Screen::ChatList);
                iced::Task::none()
            }
            // ── Manual global-registry refresh ──────────────────────────
            // The PUBLIC ROOMS refresh button: run an immediate DHT registry
            // lookup (instead of waiting for the periodic ~120s tick) and
            // merge any discovered rooms into the local directory.
            AppMessage::RefreshRoomRegistry => self.refresh_room_registry_now(),

            // ── BORU-DIR-15 (PDF Task 5.3): search / filter / sort ────
            // All local-only state mutations. The search query is stored
            // in `discover_search_query` and applied in
            // `discover_dependency()` against the local RoomDirectory
            // snapshot — it is NEVER broadcast onto the discovery network
            // (PDF Core rule; Task 5.3 acceptance "Search does not leak
            // queries to other peers").
            AppMessage::DiscoverSearchChanged(query) => {
                self.discover_search_query = query;
                iced::Task::none()
            }
            AppMessage::DiscoverTicketInputChanged(ticket) => {
                self.discover_ticket_input = ticket;
                self.discover_ticket_error.clear();
                iced::Task::none()
            }
            AppMessage::DiscoverJoinFromTicket => {
                let ticket_input = self.discover_ticket_input.trim();
                if ticket_input.is_empty() {
                    self.discover_ticket_error = "Paste a ticket before joining a room.".to_string();
                    return iced::Task::none();
                }
                match RoomInvitation::parse(ticket_input) {
                    Ok(_) => {
                        self.join_ticket_input = ticket_input.to_string();
                        self.discover_ticket_error.clear();
                        iced::Task::done(AppMessage::JoinFromTicket)
                    }
                    Err(error) => {
                        self.discover_ticket_error = format!("Invalid ticket: {error}");
                        iced::Task::none()
                    }
                }
            }
            AppMessage::DiscoverFilterToggled(filter) => {
                match filter {
                    DiscoverFilter::Compatible => {
                        self.discover_filter_compatible = !self.discover_filter_compatible;
                    }
                    DiscoverFilter::NotJoined => {
                        self.discover_filter_not_joined = !self.discover_filter_not_joined;
                    }
                    DiscoverFilter::RecentlySeen => {
                        self.discover_filter_recently_seen =
                            !self.discover_filter_recently_seen;
                    }
                }
                iced::Task::none()
            }
            AppMessage::DiscoverTagToggled(tag) => {
                if let Some(pos) = self.discover_selected_tags.iter().position(|t| t == &tag) {
                    self.discover_selected_tags.remove(pos);
                } else {
                    self.discover_selected_tags.push(tag);
                }
                iced::Task::none()
            }
            AppMessage::DiscoverSortChanged(sort) => {
                self.discover_sort = sort;
                iced::Task::none()
            }
            AppMessage::DiscoverClearFilters => {
                self.discover_search_query.clear();
                self.discover_filter_compatible = false;
                self.discover_filter_not_joined = false;
                self.discover_filter_recently_seen = false;
                self.discover_selected_tags.clear();
                iced::Task::none()
            }


            AppMessage::DirectoryRoomJoin(ad) => {
                // Parse the ticket from the advertisement and open the room.
                // BORU-DIR-18 (PDF Task 6.3): this legacy path must obey the
                // SAME join gate as DirectoryRoomJoinById — protocol
                // compatibility AND the live local permission state are
                // re-validated before any subscription, so discovery can
                // never bypass room-level security (bans / local hide-block /
                // incompatible protocol) no matter which entry point the
                // join came through.
                match Ticket::from_str(&ad.ticket) {
                    Ok(ticket) => {
                        let topic = ticket.topic;
                        match self.directory_join_target(*topic.as_bytes()) {
                            Ok(_) => {
                                info!(topic = %topic, "joining room from directory");
                                iced::Task::done(AppMessage::OpenRoom(topic))
                            }
                            Err(reason) => {
                                warn!(reason = %reason, "directory join blocked");
                                self.push_system(reason);
                                iced::Task::none()
                            }
                        }
                    }
                    Err(e) => {
                        warn!("failed to parse directory room ticket: {e}");
                        self.push_system("Failed to join room: invalid ticket");
                        iced::Task::none()
                    }
                }
            }
            AppMessage::DirectoryRoomJoinById(room_id) => {
                // BORU-DIR-16 (PDF Task 6.1): explicit Join from the
                // Discover card. The advertisement is metadata, never an
                // authorization (PDF Task 6.1 step 4) — the directory
                // only proves a room was advertised. Joining is initiated
                // only here (the user pressed Join) and only after the
                // cache's protocol-compatibility verdict is checked (PDF
                // Task 6.1 step 2: block or explain known-incompatible
                // rooms before attempting a subscription). The normal
                // public-room join path (OpenRoom) then subscribes via
                // room-topic logic and, on success, creates the local
                // conversation record exactly once.
                match self.directory_join_target(room_id) {
                    Ok(topic) => {
                        info!(topic = %topic, "joining room from directory (explicit user action)");
                        iced::Task::done(AppMessage::OpenRoom(topic))
                    }
                    Err(reason) => {
                        warn!(reason = %reason, "directory join blocked");
                        self.push_system(format!("Cannot join room: {reason}"));
                        iced::Task::none()
                    }
                }
            }
            AppMessage::DirectoryRoomHideById(room_id) => {
                // BORU-DIR-20 (PDF Task 7.2): local Hide Room. The hide
                // preference is persisted through the DIR-12 persistence
                // hook (Storage::set_room_hidden) and the directory cache
                // is re-derived immediately so the card disappears from
                // Discover on the next frame — and stays gone across
                // advertisement refreshes and app restarts. This is a
                // LOCAL moderation choice: nothing is broadcast, no
                // membership changes, and the preference is never sent to
                // the directory topic or any peer (PDF Core rule).
                if let Some(storage) = self.storage.as_ref() {
                    if let Err(err) = storage.set_room_hidden(&room_id, true) {
                        warn!(error = %err, "failed to persist hidden room preference");
                        self.push_system("Failed to hide room: the preference could not be saved.");
                    }
                }
                self.sync_directory_local_states();
                iced::Task::none()
            }
            AppMessage::DirectoryRoomUnhideById(room_id) => {
                // BORU-DIR-20 (PDF Task 7.2): explicit reset of the hide
                // preference for one room (Settings → Hidden rooms). The
                // room is offered again on the next frame. Never
                // broadcast — this is the local undo path the PDF
                // requires.
                if let Some(storage) = self.storage.as_ref() {
                    if let Err(err) = storage.set_room_hidden(&room_id, false) {
                        warn!(error = %err, "failed to persist unhidden room preference");
                        self.push_system("Failed to restore room: the preference could not be saved.");
                    }
                }
                self.sync_directory_local_states();
                iced::Task::none()
            }
            AppMessage::DirectoryRoomUnhideAll => {
                // BORU-DIR-20 (PDF Task 7.2): restore every hidden room
                // (Settings → Hidden rooms → Restore all). Clears the
                // persisted preference set. Never broadcast.
                if let Some(storage) = self.storage.as_ref() {
                    let ids = storage.room_hidden_ids().unwrap_or_default();
                    for id in ids {
                        if let Err(err) = storage.set_room_hidden(&id, false) {
                            warn!(error = %err, "failed to persist unhidden room preference");
                        }
                    }
                }
                self.sync_directory_local_states();
                iced::Task::none()
            }
            AppMessage::DeleteDirectoryRoom(topic) => {
                let local_author = self.local_public;
                let removed = self
                    .directory_store
                    .lock()
                    .map(|mut store| store.remove(topic, local_author))
                    .unwrap_or(false);
                if removed {
                    if let Some(storage) = self.storage.as_ref() {
                        if let Err(err) = storage.with_conn(|conn| {
                            conn.execute(
                                "DELETE FROM directory_ads WHERE topic = ?1 AND author = ?2",
                                rusqlite::params![topic.as_bytes(), local_author.as_bytes()],
                            )
                            .map_err(n0_error::AnyError::from_std)?;
                            Ok(())
                        }) {
                            warn!("failed to delete directory advertisement: {err}");
                        }
                    }
                    self.rooms_state.advertised_rooms.remove(&topic);
                    self.refresh_sidebar_counts();
                }
                iced::Task::none()
            }
            AppMessage::DirectoryRoomUpdate(..) => {
                // Room advertisements from the directory topic are drained
                // directly from directory_room_rx on ConnMonitorTick.
                iced::Task::none()
            }

            AppMessage::NewDiscoveredPeers(peers) => {
                // Capture newly-added peers before the update consumes `peers`.
                let added = peers.added.clone();
                apply_discovered_peers_update(&mut self.discovered_peers, peers);
                // Retroactively join newly discovered peers to all background
                // conversation subscriptions. Without this, a peer discovered
                // after SubscribeStoredConversations ran will never be added
                // to the direct-conversation gossip mesh, and messages will
                // silently queue in the outbox.
                if !added.is_empty() {
                    let pending: Vec<PublicKey> = added
                        .into_iter()
                        .filter(|p| *p != self.local_public)
                        .filter(|p| self.discovered_peers.contains(p))
                        .collect();
                    if !pending.is_empty() {
                        for (_, conv) in &self.conversations {
                            if let Some(ref sender) = conv.sender {
                                let s = sender.clone();
                                let peers = pending.clone();
                                tokio::spawn(async move {
                                    for peer in peers {
                                        if let Err(e) = s.join_peers(vec![peer]).await {
                                            warn!(peer = %peer, error = %e,
                                                "new-discovered join_peers failed");
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                iced::Task::none()
            }
            // ── BORU-CP-07/08: backend reconnection success ──────────────
            AppMessage::ReconnectPeerReady(peer) => {
                // The backend re-established endpoint connectivity to a
                // peer (a reconnect attempt succeeded). BORU-CP-08: restore
                // ONLY the communication state the local user is already
                // entitled to. The required direct topics come from the
                // pure reconcile decision (friend record + conversation
                // store metadata): existing direct conversations are
                // rejoined/resubscribed, while deleted/blocked
                // relationships are never resurrected and group/public
                // topics are never auto-joined from discovery. The
                // data-plane action (subscription) is performed here, never
                // by the discovery service (deterministic topic ownership).
                let topics = self.reconnect_required_topics(peer);
                if topics.is_empty() {
                    trace!(
                        peer = %peer.fmt_short(),
                        "reconnect: no entitled direct topics to restore"
                    );
                    return iced::Task::none();
                }
                info!(
                    peer = %peer.fmt_short(),
                    topics = topics.len(),
                    "reconnect: restoring entitled direct topics"
                );
                // 1. Re-join the peer into live conversation senders so the
                //    direct-topic mesh edges re-form immediately (the same
                //    retroactive-join pattern as NewDiscoveredPeers) instead
                //    of waiting for the gossip dial cooldown. Only senders
                //    for the required direct topics are touched — the peer
                //    is never joined into groups/public chats it is not
                //    entitled to (no authorisation by presence).
                for (conv_topic, conv) in &self.conversations {
                    if !topics.contains(conv_topic) {
                        continue;
                    }
                    if let Some(ref sender) = conv.sender {
                        let s = sender.clone();
                        let peers = vec![peer];
                        tokio::spawn(async move {
                            if let Err(e) = s.join_peers(peers).await {
                                warn!(peer = %peer, error = %e,
                                    "reconnect join_peers failed");
                            }
                        });
                    }
                }
                // 2. Ensure each required direct topic is subscribed.
                //    BackgroundSubscribe is idempotent: it skips when the
                //    topic already has a live sender or a subscription is in
                //    flight, so several reconnect signals can never create
                //    duplicate subscriptions.
                let bootstrap = self.discovered_peers.clone();
                iced::Task::batch(
                    topics
                        .into_iter()
                        .map(|topic| {
                            iced::Task::done(AppMessage::BackgroundSubscribe(
                                topic,
                                bootstrap.clone(),
                            ))
                        })
                        .collect::<Vec<_>>(),
                )
            }
            // ── Background conversation subscriptions (startup auto-subscribe) ──
            AppMessage::SubscribeStoredConversations => {
                // Subscribe to all stored conversations at startup so messages
                // can be received even before the user opens each chat.
                let store_topics: Vec<TopicId> = self
                    .conversation_store
                    .active_iter()
                    .into_iter()
                    .map(|e| e.topic)
                    .filter(|t| !self.conversations.contains_key(t))
                    // BORU-DISC-13: the internal discovery topic is never a
                    // conversation — exclude it from stored-conversation
                    // auto-subscribe so it can never be materialized as a
                    // ConversationLive with a sender in self.conversations.
                    .filter(|t| !boru_core::discovery_topic::is_discovery_topic(*t))
                    .collect();
                if store_topics.is_empty() {
                    return iced::Task::none();
                }
                // Include discovered peers as bootstrap so the gossip mesh has
                // neighbors for the direct topic. Without this, background
                // subscriptions have zero peers and broadcasts go nowhere.
                let bootstrap_peers: Vec<PublicKey> = self.discovered_peers.clone();
                info!(
                    count = store_topics.len(),
                    bootstrap = bootstrap_peers.len(),
                    "startup: subscribing to stored conversations"
                );
                iced::Task::batch(
                    store_topics
                        .into_iter()
                        .map(|topic| {
                            AppMessage::BackgroundSubscribe(topic, bootstrap_peers.clone())
                        })
                        .map(iced::Task::done),
                )
            }
            AppMessage::BackgroundSubscribe(topic, bootstrap_peers) => {
                // Already actively subscribed — skip. Persistent conversation
                // state and live network state are distinct: a conversation
                // loaded from storage has `sender == None` until the gossip
                // subscription completes, so `contains_key` alone must never
                // suppress a subscription.
                let already_subscribed = self
                    .conversations
                    .get(&topic)
                    .is_some_and(|c| c.sender.is_some())
                    || (topic == self.topic && self.sender.is_some());
                let subscribing = self
                    .background_subscriptions_in_flight
                    .contains(&topic);
                if already_subscribed || subscribing {
                    return iced::Task::none();
                }
                // ── BORU-DISC-13 guard ────────────────────────────────
                // The discovery topic is owned by DiscoveryService (joined
                // at startup in main.rs). Never background-subscribe it as a
                // conversation: no ConversationLive, no sender kept in
                // self.conversations, no history replay for the mesh.
                if boru_core::discovery_topic::is_discovery_topic(topic) {
                    tracing::warn!(
                        topic = %topic,
                        "refusing to background-subscribe discovery topic as conversation"
                    );
                    return iced::Task::none();
                }
                self.background_subscriptions_in_flight.insert(topic);
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let _endpoint = self.endpoint.clone();
                let profile_image_ticket = self.settings_state.profile_image_ticket.clone();
                let forward_handle_slot = Arc::new(StdMutex::new(None));
                let forward_handle_slot_task = forward_handle_slot.clone();
                let bootstrap_peers: Vec<PublicKey> = bootstrap_peers
                    .into_iter()
                    .filter(|peer| *peer != self.local_public)
                    .collect();
                let peers_count = bootstrap_peers.len();
                iced::Task::perform(
                    async move {
                        info!(topic=%topic, peers=peers_count, "BackgroundSubscribe: subscribing with bootstrap peers");
                        let sub = gossip
                            .subscribe(topic, bootstrap_peers)
                            .await
                            .map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let receiver_id = format!("{:p}", &receiver as *const _);
                        let neighbors_before = receiver
                            .neighbors()
                            .map(|p| p.fmt_short().to_string())
                            .collect::<Vec<_>>();
                        info!(
                            topic=%topic,
                            receiver_id,
                            neighbors = ?neighbors_before,
                            "BACKGROUND_RX_CREATED: subscription split — receiver created, neighbors snapshot",
                        );
                        let _neighbor_count = neighbors_before.len();
                        let metadata_doc = boru_core::room_docs::create_metadata_doc(
                            topic,
                            &sender,
                            boru_core::room_docs::RoomMetadata {
                                name: Some("boru-chat".to_string()),
                                description: None,
                                rules: None,
                            },
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        let roster_doc = boru_core::room_docs::create_roster_doc(
                            topic,
                            &sender,
                            sk.public().to_string(),
                            label.clone(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        let forward_handle = spawn_conversation_forwarder(
                            topic,
                            metadata_doc,
                            roster_doc,
                            receiver,
                            net_tx,
                            None,
                        );
                        info!(
                            topic=%topic,
                            receiver_id,
                            neighbors = ?neighbors_before,
                            "FORWARDER_SPAWN: permanent forwarder spawned with receiver",
                        );
                        // Broadcast AboutMe so the peer knows we're here
                        if let Ok(msg) = crate::SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe {
                                name: label,
                                profile_image_ticket,
                            },
                        ) {
                            let _ = sender.broadcast(msg).await;
                        }
                        *forward_handle_slot_task.lock().unwrap() = Some(forward_handle);
                        Ok::<_, String>((sender, topic))
                    },
                    move |result| match result {
                        Ok((sender, topic)) => AppMessage::BackgroundSubscribed(
                            topic,
                            Some(sender),
                            Some(forward_handle_slot),
                        ),
                        Err(e) => {
                            AppMessage::BackgroundSubscribeFailed(topic, e)
                        }
                    },
                )
            }
            AppMessage::BackgroundSubscribeFailed(topic, error) => {
                // Release the exact topic's single-flight slot.  Do not route
                // failures through BackgroundSubscribed: that handler owns
                // successful subscription state and cannot safely infer the
                // original topic from an async error.
                self.background_subscriptions_in_flight.remove(&topic);
                warn!(topic = %topic, error = %error, "background subscribe failed");
                iced::Task::none()
            }
            AppMessage::BackgroundSubscribed(topic, sender, forward_handle_slot) => {
                // A freshly background-subscribed conversation (startup
                // auto-subscribe) has never been opened this session, so its
                // in-memory timeline starts empty.  Replay the persisted
                // local history (chat_history.json) into it now, so opening
                // the chat later via the fast path shows the same messages a
                // fresh subscription would have replayed — instead of an
                // empty timeline until a network backfill happens to arrive.
                // This mirrors the RoomOpened JSON replay for the slow path.
                let local_hex = self.local_public.to_string();
                let mut replayed: Vec<ChatEntry> = Vec::new();
                {
                    let store = self.chat_history.lock().unwrap();
                    for hist in store
                        .for_topic(&topic)
                        .into_iter()
                        // Skip placeholder rows that never got a real id
                        // (mirrors the RoomOpened replay guard).
                        .filter(|e| e.event_id != 0)
                    {
                        if let Some(mut chat) =
                            self.history_entry_to_chat_entry(hist, &topic, &local_hex)
                        {
                            // Populate derived caches (label_text,
                            // reactions_text, parsed_segments) exactly like
                            // `entries_push` does — otherwise the windowed
                            // renderer sees an empty segment list and draws
                            // an empty bubble.
                            chat.update_cache();
                            replayed.push(chat);
                        }
                    }
                }
                let conv = self.conversations.entry(topic).or_insert_with(|| {
                    let mut c = ConversationLive::new(topic);
                    if !replayed.is_empty() {
                        c.entries = replayed.clone();
                        c.history_saved_count = replayed.len();
                        c.history_loaded = true;
                        // Follow-latest (the default) with the bottom
                        // sentinel so the windowed renderer shows the
                        // newest messages as soon as the chat opens,
                        // matching the slow-path replay behaviour.
                        c.scroll_offset = f32::MAX;
                    }
                    c
                });
                // The conversation-store loop and the friends loop can both
                // subscribe the same direct topic, so a duplicate
                // BackgroundSubscribed can arrive for a conversation that
                // already replayed its history.  Fill the timeline only if
                // the first completion never did (e.g. a raced first event).
                if !conv.history_loaded {
                    if conv.entries.is_empty() && !replayed.is_empty() {
                        conv.entries = replayed;
                        conv.history_saved_count = conv.entries.len();
                        conv.scroll_offset = f32::MAX;
                    }
                    conv.history_loaded = true;
                }
                if let Some(ref s) = sender {
                    // Retroactively join any discovered peers that were not part
                    // of the bootstrap list at subscription time (e.g. peers that
                    // were discovered via mDNS while the async subscribe was
                    // in-flight, or peers discovered after a background subscribe
                    // that ran before the peer was on any LAN).
                    let pending: Vec<PublicKey> = self
                        .discovered_peers
                        .iter()
                        .filter(|&&pk| pk != self.local_public)
                        .copied()
                        .collect();
                    if !pending.is_empty() {
                        let s = s.clone();
                        tokio::spawn(async move {
                            for peer in pending {
                                if let Err(e) = s.join_peers(vec![peer]).await {
                                    warn!(peer = %peer, error = %e,
                                        "retroactive join_peers after bg subscribe failed");
                                }
                            }
                        });
                    }
                }
                conv.sender = sender.clone();
                // A sender handle only means that the subscription was
                // created. It is not room-ready until NeighborUp is observed.
                // Keep the forwarder alive so that transition can arrive.
                conv.sender_ready = false;
                conv.forward_handle =
                    forward_handle_slot.and_then(|slot| slot.lock().unwrap().take());
                // Subscription creation finished (success or failure) — the
                // single-flight slot is released so a later event can retry
                // if no sender was installed.
                self.background_subscriptions_in_flight.remove(&topic);
                if conv.sender.is_some() {
                    // BORU-DISC-20: log direct/group conversation topic
                    // subscriptions independently and bump the matching
                    // counter, so debugging can prove which conversation
                    // topics were actually joined (separate from the
                    // discovery-topic join in main.rs).
                    let kind = self
                        .conversation_store
                        .find(&topic)
                        .map(|e| e.kind.clone())
                        .unwrap_or(boru_core::conversations::ConversationKind::Group);
                    match kind {
                        boru_core::conversations::ConversationKind::Direct => {
                            boru_core::diagnostics::DIAGNOSTIC_COUNTERS.record_direct_topic_joined();
                            info!(topic=%topic, "background subscribed to direct conversation topic");
                            // BORU-CP-07: a REAL direct-topic readiness
                            // success — report it so the backend clears the
                            // peer's retry/backoff state (acceptance
                            // criterion: successful direct-topic readiness
                            // clears retry/backoff). The report is
                            // friend-scoped: only a deterministic direct
                            // topic of a current friend qualifies. This is
                            // never called for discovery metadata.
                            if let Some(handle) = &self.reconnect_handle {
                                for (fid, _) in self.friends.iter() {
                                    if let Ok(peer_pk) = fid.parse_public_key() {
                                        if direct_topic(&self.local_public, &peer_pk) == topic {
                                            handle.report_topic_ready(peer_pk);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        boru_core::conversations::ConversationKind::Group => {
                            boru_core::diagnostics::DIAGNOSTIC_COUNTERS.record_group_topic_joined();
                            info!(topic=%topic, "background subscribed to group conversation topic");
                        }
                    }
                } else {
                    warn!("background subscribe failed for {topic}");
                }
                iced::Task::none()
            }
            // ── Catalogue errors (state layer) ──
            AppMessage::CatalogueFetchFailed(message) => {
                self.files_state.catalogue_loading = false;
                self.files_state.catalogue_error = Some(message);
                iced::Task::none()
            }
            AppMessage::CatalogueErrorDismissed => {
                self.files_state.catalogue_error = None;
                iced::Task::none()
            }
            // update() only dispatches the discover variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}

impl IcedChat {
    /// BORU-DIR-16 (PDF Task 6.1): resolve a Discover-card Join click
    /// into the normal public-room join path.
    ///
    /// The advertisement itself is never treated as authorization (PDF
    /// Task 6.1 step 4). The Join button only renders for rooms whose
    /// cache verdict is `Join` (compatible + not joined), but the handler
    /// re-validates against the live cache so a stale render can never
    /// subscribe to a known-incompatible room (PDF Task 6.1 step 2).
    ///
    /// The advertised `room_id` IS the room's gossip topic (the
    /// deterministic identity from `topic_derivation::public_room_topic`
    /// — see the advertisement docs), so joining routes through the
    /// existing `OpenRoom` slow path, which subscribes via normal
    /// room-topic logic and creates the conversation record exactly once
    /// on success. No ticket exchange is needed: public discoverable
    /// rooms are joined by subscribing to their advertised topic, and
    /// bootstrap peers come from the normal OpenRoom path (discovered
    /// peers + saved RoomStore).
    ///
    /// Base-room-protocol incompatibility (UpgradeRequired/Unsupported,
    /// PDF Task 6.2 step 5) blocks the join with a useful explanation.
    /// Optional-feature differences are informational only and never
    /// block basic room access (PDF Task 6.2 acceptance).
    ///
    /// Returns `Ok(topic)` when the join may proceed, or `Err(reason)`
    /// with a user-facing explanation when it must be blocked.
    pub(crate) fn directory_join_target(&self, room_id: [u8; 32]) -> Result<TopicId, String> {
        use boru_core::room_directory::{LocalJoinState, RoomCompatibility};

        let topic = TopicId::from_bytes(room_id);

        // Prefer the bounded control-plane cache (the Discover source of
        // truth). The legacy directory-store fallback (tests / discovery
        // service unavailable) carries no compatibility metadata — the
        // browse surface treats those rows as compatible.
        let entry = match &self.room_directory {
            Some(dir) => dir.lock().unwrap().get(&topic).cloned(),
            None => None,
        };

        // BORU-DIR-18 (PDF Task 6.3): room-level permissions are
        // authoritative over the directory. A locally hidden/blocked room
        // (the local "ban" analog, derived from the real room database via
        // LocalRoomFacts.hidden) must never be joinable through discovery —
        // the handler re-validates against the live cache so discovery can
        // never bypass the block, even from a stale render or a direct
        // message. This keeps "directory visibility" and "join
        // authorization" independent: the advertisement may still exist
        // (TTL/refresh continue), but joining is refused until the user
        // unhides the room.
        if let Some(e) = &entry {
            if e.local_join_state == LocalJoinState::Blocked {
                return Err(
                    "Cannot join room: this room is hidden or blocked locally. Unhide it in room settings to join."
                        .to_string(),
                );
            }
        }

        match entry.as_ref() {
            Some(e) => match e.compatibility {
                RoomCompatibility::UpgradeRequired => Err(format!(
                    "Cannot join room: this room requires a newer protocol version (v{}), but this Boru build only supports v{}. Please upgrade Boru to join.",
                    e.advert.room_protocol_version,
                    boru_core::public_room::PROTOCOL_VERSION,
                )),
                RoomCompatibility::Unsupported => Err(format!(
                    "Cannot join room: this room uses protocol v{}, which this Boru build (v{}) does not support.",
                    e.advert.room_protocol_version,
                    boru_core::public_room::PROTOCOL_VERSION,
                )),
                // Compatible, Unknown, or legacy fallback: proceed to the
                // normal join path. Optional-feature differences never block
                // basic room access (PDF Task 6.2 acceptance).
                _ => Ok(topic),
            },
            None => Ok(topic),
        }
    }

    /// Look up the current advertisement for a directory room, preferring
    /// the bounded control-plane cache (the Discover source of truth) and
    /// falling back to the legacy directory store (tests / discovery
    /// service unavailable). Used after a successful join to seed the
    /// local conversation record from advertised metadata (BORU-DIR-16,
    /// PDF Task 6.1 step 5).
    pub(crate) fn directory_advert_for_topic(
        &self,
        topic: &TopicId,
    ) -> Option<boru_core::control_plane::advertisement::PublicRoomAdvertisement> {
        if let Some(dir) = &self.room_directory {
            if let Some(entry) = dir.lock().unwrap().get(topic) {
                return Some(entry.advert.clone());
            }
        }
        // Legacy fallback: the old directory-store advertisement
        // (relay-scoped directory gossip topic). The legacy store has no
        // control-plane advert shape, so rebuild a minimal one.
        let store = self.directory_store.lock().unwrap();
        store
            .list_active()
            .into_iter()
            .find(|(ad, _)| ad.topic == *topic)
            .map(|(ad, _)| {
                let mut advert =
                    boru_core::control_plane::advertisement::PublicRoomAdvertisement::minimal(
                        ad.topic,
                        ad.room_name,
                        [0u8; 32],
                    );
                advert.short_description = ad.description;
                advert.approximate_member_count = Some(ad.member_count);
                advert
            })
    }

    /// BORU-DIR-16 (PDF Task 6.1 step 5): after a successful join, create
    /// or update the local conversation record exactly once using the
    /// advertised metadata. The control-plane directory never materializes
    /// entries on discovery (PDF Core rule), so a directory join has no
    /// prior record to unarchive — the record is created here, and only
    /// here, when `RoomOpened` confirms the subscription succeeded.
    ///
    /// Idempotent by construction: when a record already exists (a
    /// re-open of a joined room, a legacy archived entry, etc.) this is a
    /// no-op, so it can never duplicate a conversation record. Rooms that
    /// are not directory rooms (direct chats, private ticket rooms)
    /// return `false` and leave the store untouched.
    ///
    /// Returns `true` when a record was created.
    pub(crate) fn ensure_directory_joined_record(&mut self, topic: TopicId) -> bool {
        if self.conversation_store.find(&topic).is_some() {
            return false;
        }
        let Some(ad) = self.directory_advert_for_topic(&topic) else {
            return false;
        };
        let mut entry = ConversationEntry::new(topic, "", ad.room_name);
        entry.visibility = ad.visibility;
        entry.description = ad.short_description;
        entry.tags = ad.tags;
        self.conversation_store.upsert(entry);
        self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
        info!(topic = %topic, "created conversation record for directory-joined room");
        true
    }

    /// BORU-CP-08: direct topics the local user is already entitled to
    /// restore after `peer` becomes reachable again.
    ///
    /// Computed from existing local metadata only (friend record +
    /// conversation store) via the pure
    /// [`required_reconnect_topics`](boru_core::control_plane::reconcile::required_reconnect_topics)
    /// decision. Never derives topics from discovery advertisements,
    /// never auto-joins groups/public chats, and never resurrects
    /// deleted/blocked relationships.
    pub(crate) fn reconnect_required_topics(&self, peer: PublicKey) -> Vec<TopicId> {
        let fid = FriendId::from_public_key(peer);
        let friend = self.friends.get(&fid);
        let store_entries: Vec<boru_core::conversations::ConversationEntry> =
            self.conversation_store.iter().cloned().collect();
        boru_core::control_plane::reconcile::required_reconnect_topics(
            &self.local_public,
            &peer,
            friend,
            &store_entries,
        )
    }
}

pub(crate) fn apply_discovered_peers_update(peers: &mut Vec<PublicKey>, update: DiscoveredPeersUpdate) {
    peers.retain(|peer| !update.removed.contains(peer));
    for peer in update.added {
        if update.removed.contains(&peer) {
            continue;
        }
        if !peers.contains(&peer) {
            peers.push(peer);
        }
    }
}
