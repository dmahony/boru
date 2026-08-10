//! File sharing dashboard feature.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the File Sharing
//! dashboard screen: the Hash-compatible screen/card dependency
//! snapshots, the projection/refresh helpers, and the `impl IcedChat`
//! methods that build and render the dashboard tabs (Shared by Me,
//! Downloading, Downloaded, Shared with Me, Activity Log) plus the
//! Files-tab cards. Reads app state via `use super::*`; app.rs
//! re-exports the pub(crate) items it still references with
//! `use files::*`.

use super::*;

/// Dependency for the File Sharing dashboard screen (default Files tab).
/// PERF-4R-A (t_668423a9): the screen-level key snapshots everything the
/// shell + header/search/tab bar + default Files tab grid render — including
/// the four PERF-2 card dependencies — so `iced::widget::lazy` (and the
/// PERF-4R-B pre-warm cache) can serve a fully materialized `Element<'static>`
/// tree while any rendered slice is unchanged. `DashboardTab` is not `Hash`,
/// so `Hash` is implemented manually below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSharingDependency {
    pub(crate) dark_mode: bool,
    /// Responsive band derived from the window width (FS-21 breakpoints), so
    /// the cached tree only rebuilds when the layout tier changes, not on
    /// every pixel of resize.
    pub(crate) responsive_mode: FileSharingResponsiveMode,
    pub(crate) dashboard_search_input: String,
    pub(crate) dashboard_active_tab: crate::dashboard_view_model::DashboardTab,
    /// The FS-19 connectivity notice renders only on the default Files tab,
    /// so its two inputs are part of the snapshot.
    pub(crate) dashboard_connectivity_dismissed: bool,
    pub(crate) mesh_health: MeshHealthSnapshot,
    /// PERF-2 card dependencies, reused as-is.
    pub(crate) shared_by_me: SharedByMeCardDependency,
    pub(crate) peers: PeersCardDependency,
    pub(crate) sharing_summary: SharingSummaryCardDependency,
    pub(crate) recent_activity: RecentActivityCardDependency,
}

impl std::hash::Hash for FileSharingDependency {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dark_mode.hash(state);
        self.responsive_mode.hash(state);
        self.dashboard_search_input.hash(state);
        // DashboardTab is Copy/Eq but not Hash; hash a stable tag so the
        // cache key tracks the active tab (owned tabs miss → live path).
        match self.dashboard_active_tab {
            crate::dashboard_view_model::DashboardTab::SharedByMe => 0u8.hash(state),
            crate::dashboard_view_model::DashboardTab::Downloading => 1u8.hash(state),
            crate::dashboard_view_model::DashboardTab::Downloaded => 2u8.hash(state),
            crate::dashboard_view_model::DashboardTab::SharedWithMe => 3u8.hash(state),
            crate::dashboard_view_model::DashboardTab::ActivityLog => 4u8.hash(state),
        }
        self.dashboard_connectivity_dismissed.hash(state);
        self.mesh_health.hash(state);
        self.shared_by_me.hash(state);
        self.peers.hash(state);
        self.sharing_summary.hash(state);
        self.recent_activity.hash(state);
    }
}

/// Responsive band for the File Sharing shell (FS-21 breakpoints:
/// `VIEWPORT_MIN_WIDTH` / `VIEWPORT_REF_WIDTH` / `VIEWPORT_LG_WIDTH`).
/// Banding the raw width means a resize within a tier keeps the cached tree
/// valid; only a breakpoint flip invalidates the FileSharing cache entry.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) enum FileSharingResponsiveMode {
    /// `width <= VIEWPORT_MIN_WIDTH` — single-column content, scrollable tabs.
    Compact,
    /// `VIEWPORT_MIN_WIDTH < width < VIEWPORT_REF_WIDTH` — two columns,
    /// reduced search width.
    Medium,
    /// `VIEWPORT_REF_WIDTH <= width < VIEWPORT_LG_WIDTH` — reference layout.
    Reference,
    /// `width >= VIEWPORT_LG_WIDTH` — large layout.
    Large,
}

impl FileSharingResponsiveMode {
    fn from_width(width: f32) -> Self {
        use crate::design_tokens::{VIEWPORT_LG_WIDTH, VIEWPORT_MIN_WIDTH, VIEWPORT_REF_WIDTH};
        if width <= VIEWPORT_MIN_WIDTH {
            Self::Compact
        } else if width < VIEWPORT_REF_WIDTH {
            Self::Medium
        } else if width < VIEWPORT_LG_WIDTH {
            Self::Reference
        } else {
            Self::Large
        }
    }

    fn is_compact(self) -> bool {
        matches!(self, Self::Compact)
    }

    fn is_medium(self) -> bool {
        matches!(self, Self::Medium)
    }
}

// ── PERF-2 (t_f6dcbb3a): per-card lazy dependencies for the File Sharing
// dashboard ───────────────────────────────────────────────────────────
// Each struct snapshots exactly the state slice its card renders, so
// `iced::widget::lazy` reuses a card's built subtree unless that card's own
// data changed. The live row types below (SharedByMeRow, RecentActivityRow,
// CompletedDownloadItem, ...) are Eq but not Hash, so Hash is implemented
// manually — hashing every field that participates in PartialEq keeps the
// cache key consistent with change detection.

/// Dependency for the "Downloads" (Downloaded tab) card. `active` mirrors the
/// tab selection so the cached subtree is keyed by whether the tab owns the
/// content area; the remaining fields are the rendered state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DownloadsCardDependency {
    pub(crate) dark_mode: bool,
    pub(crate) active: bool,
    /// Completed-download history rows rendered by the tab.
    pub(crate) history: Vec<crate::dashboard_view_model::CompletedDownloadItem>,
    pub(crate) history_loaded: bool,
    pub(crate) history_error: Option<String>,
    /// Global dashboard search query (filters name + source peer).
    pub(crate) search_query: String,
    pub(crate) sort: crate::dashboard_filters::DownloadedSort,
}

impl std::hash::Hash for DownloadsCardDependency {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dark_mode.hash(state);
        self.active.hash(state);
        self.history_loaded.hash(state);
        self.history_error.hash(state);
        self.search_query.hash(state);
        std::mem::discriminant(&self.sort.key).hash(state);
        self.sort.descending.hash(state);
        for item in &self.history {
            item.id.hash(state);
            item.row_id.hash(state);
            item.content_id.hash(state);
            item.display_name.hash(state);
            item.mime_type.hash(state);
            item.size_bytes.hash(state);
            item.source_peer.hash(state);
            item.completed_at_ms.hash(state);
            std::mem::discriminant(&item.local).hash(state);
            item.destination_path.hash(state);
        }
    }
}

/// Dependency for the "Files I'm Sharing" table card. Includes the search
/// query (drives the search-specific empty state), the filtered+sorted rows
/// actually rendered, the per-row interactive state, the load state, the
/// active sort, and the thumbnail handles keyed by content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SharedByMeCardDependency {
    pub(crate) dark_mode: bool,
    /// Global dashboard search query — checked trimmed-empty for the
    /// search-specific empty state.
    pub(crate) search_query: String,
    /// Number of filtered rows (matches the table count badge).
    pub(crate) items_count: usize,
    /// The filtered+sorted rows rendered by the table.
    pub(crate) rows: Vec<crate::shared_by_me_table::SharedByMeRow>,
    /// Per-row interactive state (open menus/details/confirmations).
    pub(crate) ui: crate::shared_by_me_table::SharedByMeUiState,
    pub(crate) load_state: crate::shared_by_me_table::SharedByMeLoadState,
    pub(crate) sort: crate::dashboard_filters::SharedByMeSort,
    /// Thumbnail handles for image/video rows (hashed by presence only).
    pub(crate) thumbnails: SharedByMeThumbnails,
}

impl std::hash::Hash for SharedByMeCardDependency {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dark_mode.hash(state);
        self.search_query.hash(state);
        self.items_count.hash(state);
        std::mem::discriminant(&self.sort.key).hash(state);
        self.sort.descending.hash(state);
        match &self.load_state {
            crate::shared_by_me_table::SharedByMeLoadState::Loading => 0u8.hash(state),
            crate::shared_by_me_table::SharedByMeLoadState::Ready => 1u8.hash(state),
            crate::shared_by_me_table::SharedByMeLoadState::Error(message) => {
                2u8.hash(state);
                message.hash(state);
            }
        }
        self.ui.menu_open.hash(state);
        self.ui.details_open.hash(state);
        self.ui.confirm_stop.hash(state);
        self.ui.share_menu_open.hash(state);
        self.ui.sharing_status.hash(state);
        self.thumbnails.hash(state);
        for row in &self.rows {
            row.id.hash(state);
            row.content_hash.hash(state);
            row.display_name.hash(state);
            row.mime_type.hash(state);
            row.size_bytes.hash(state);
            row.shared_on_ms.hash(state);
            row.has_explicit_recipients.hash(state);
            row.source_available.hash(state);
            row.downloads.hash(state);
            for recipient in &row.recipients {
                recipient.id.hash(state);
                recipient.label.hash(state);
                std::mem::discriminant(&recipient.access).hash(state);
            }
        }
    }
}

/// Dependency for the "Peers Downloading from Me" card. The card is driven
/// by the live FS-05 outbound projection: rows are projected (with display
/// labels resolved) in `peers_card_dependency()`, and the static renderer
/// draws them. An empty `rows` renders the truthful empty state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeersCardDependency {
    pub(crate) dark_mode: bool,
    /// Live outbound rows, newest first, with peer display labels and online
    /// state resolved by the application layer.
    pub(crate) rows: Vec<crate::dashboard_view_model::PeerDownload>,
}

impl std::hash::Hash for PeersCardDependency {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        self.dark_mode.hash(state);
        for row in &self.rows {
            row.hash_live(state);
        }
    }
}

/// Dependency for the Recent Download Activity card. `tick` is bumped once per
/// second by `ActivityTick` so relative timestamps re-render while idle; `rows`
/// changes only when a real activity event is pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentActivityCardDependency {
    pub(crate) dark_mode: bool,
    pub(crate) tick: u64,
    pub(crate) rows: Vec<crate::recent_activity_view_model::RecentActivityRow>,
}

impl std::hash::Hash for RecentActivityCardDependency {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dark_mode.hash(state);
        self.tick.hash(state);
        for row in &self.rows {
            row.id.hash(state);
            row.occurred_at_ms.hash(state);
            row.peer_label.hash(state);
            row.file_label.hash(state);
            row.action.hash(state);
            std::mem::discriminant(&row.status).hash(state);
            row.detail.hash(state);
            row.bytes.hash(state);
        }
    }
}

/// Dependency for the Sharing Summary card. `summary == None` renders the
/// loading/unknown state (em dashes), so loading is distinct from a real zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SharingSummaryCardDependency {
    pub(crate) dark_mode: bool,
    pub(crate) summary: Option<crate::sharing_summary::SharingSummary>,
}

impl std::hash::Hash for SharingSummaryCardDependency {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dark_mode.hash(state);
        match &self.summary {
            None => 0u8.hash(state),
            Some(summary) => {
                1u8.hash(state);
                summary.files_shared.hash(state);
                summary.total_downloads.hash(state);
                summary.active_downloads.hash(state);
                summary.peers_shared_with.hash(state);
            }
        }
    }
}

/// Thumbnail handle map for the Shared by Me card. `iced::widget::image::Handle`
/// is Eq but not Hash, so the manual Hash impl hashes only each content hash
/// plus whether a handle is present — the presence bit is all the lazy cache
/// key needs, while the actual handles are carried for rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SharedByMeThumbnails(
    pub(crate) std::collections::HashMap<String, Option<iced::widget::image::Handle>>,
);

impl std::hash::Hash for SharedByMeThumbnails {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let mut keys: Vec<&String> = self.0.keys().collect();
        keys.sort();
        for key in keys {
            key.hash(state);
            self.0[key].is_some().hash(state);
        }
    }
}

impl IcedChat {
    pub(crate) fn refresh_sharing_summary(&self) -> iced::Task<AppMessage> {
        let Some(storage) = self.storage.clone() else {
            return iced::Task::done(AppMessage::DashboardSharingSummaryLoaded(None));
        };
        let profile = self.local_public.to_string();
        iced::Task::perform(
            tokio::task::spawn_blocking(move || {
                let shared = storage.list_shared_files(&profile, false).ok()?;
                let downloads = storage.list_downloads().ok()?;
                let peers = storage.list_shared_peer_ids(&profile).ok()?;
                Some(crate::sharing_summary::project_sharing_summary(
                    &shared,
                    &downloads,
                    &peers,
                ))
            }),
            |result| AppMessage::DashboardSharingSummaryLoaded(result.ok().flatten()),
        )
    }

    /// PERF-2: snapshot selector for the Sharing Summary card. `None` renders
    /// em dashes — loading is distinct from zero.
    pub(crate) fn sharing_summary_card_dependency(&self) -> SharingSummaryCardDependency {
        SharingSummaryCardDependency {
            dark_mode: self.dark_mode,
            summary: self.dashboard_sharing_summary,
        }
    }

    /// PERF-2: static renderer for the Sharing Summary card, run inside
    /// `iced::widget::lazy` so it is only re-invoked when the summary or theme
    /// actually changes.
    pub(crate) fn view_sharing_summary_card(
        dep: &SharingSummaryCardDependency,
    ) -> iced::Element<'static, AppMessage> {
        let theme = Self::theme_from_dark(dep.dark_mode);
        crate::sharing_summary::view_sharing_summary_card(dep.summary, theme)
    }

    pub(crate) fn view_shared_with_me(&self) -> iced::Element<'_, AppMessage> {
        use crate::dashboard_view_model::{
            project_validated_remote_shared_file, remote_item_status, RemoteItemStatus,
        };
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(self.dark_mode);

        // UI-30: exit mechanism — the Shared with Me tab owns its full content
        // area (no dashboard header/tab bar), so an explicit back button is the
        // only visible way to return to the file sharing overview.
        let back_button = button(
            Row::new()
                .push(
                    Icon::Back
                        .build()
                        .size(IconSize::Sm)
                        .color_fn(crate::design_tokens::text_secondary)
                        .build(),
                )
                .push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Back to File Sharing",
                    ),
                )
                .spacing(SPACE_4)
                .align_y(Alignment::Center),
        )
        .on_press(AppMessage::DashboardTabSelected(
            crate::dashboard_view_model::DashboardTab::SharedByMe,
        ))
        .padding([SPACE_4, SPACE_8])
        .style(BUTTON_GHOST_BG);

        let header = Row::new()
            .push(back_button)
            .push(Space::new().width(Length::Fill))
            .spacing(SPACE_8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        // Loading state — catalogue fetch in progress.
        if self.catalogue_loading && self.catalogue_error.is_none() {
            return container(
                Column::new()
                    .push(header)
                    .push(Space::new().height(Length::Fixed(SPACE_12)))
                    .push(
                        crate::ui_components::LoadingSkeleton::new(3)
                            .row_height(crate::design_tokens::TABLE_ROW_HEIGHT)
                            .build(&theme),
                    )
                    .spacing(0)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .padding([SPACE_24, SPACE_24])
            .into();
        }

        // Inline error with dismiss — catalogue fetch failed.
        if let Some(error) = &self.catalogue_error {
            let error_el = crate::ui_components::InlineError::new(error).build(&theme);
            let dismiss = button(
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Dismiss"),
            )
            .on_press(AppMessage::CatalogueErrorDismissed)
            .padding([SPACE_4, SPACE_8])
            .style(BUTTON_GHOST_BG);
            return container(
                Column::new()
                    .push(header)
                    .push(Space::new().height(Length::Fixed(SPACE_12)))
                    .push(error_el)
                    .push(Space::new().height(Length::Fixed(SPACE_12)))
                    .push(dismiss)
                    .spacing(0)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .padding([SPACE_24, SPACE_24])
            .into();
        }

        let Some((peer, files)) = self.peer_catalogue_view.as_ref() else {
            return container(
                Column::new()
                    .push(header)
                    .push(Space::new().height(Length::Fixed(SPACE_12)))
                    .push(
                        Column::new()
                            .push(crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Body,
                                "No files have been shared with you yet.",
                            ))
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::SupportingText,
                                    "Validated peer catalogues will appear here.",
                                )
                                .style(text_muted_style),
                            )
                            .spacing(SPACE_8)
                            .align_x(Alignment::Center),
                    )
                    .spacing(0)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .padding(SPACE_24)
            .into();
        };

        let peer_online = self.peer_presence(peer) != PeerPresence::Offline;
        let peer_label = self.resolve_name(peer);
        let mut rows = Column::new().spacing(SPACE_8);
        let mut visible = 0usize;
        for file in files {
            let Some(mut item) =
                project_validated_remote_shared_file(&peer.to_string(), file, peer_online)
            else {
                continue;
            };
            if !self.dashboard_search_input.trim().is_empty()
                && !crate::dashboard_filters::query_matches(
                    &self.dashboard_search_input,
                    &[
                        item.display_name.as_str(),
                        peer_label.as_str(),
                        &peer.fmt_short().to_string(),
                    ],
                )
            {
                continue;
            }
            visible += 1;
            let already_downloaded = self
                .storage
                .as_ref()
                .and_then(|storage| {
                    storage
                        .find_downloads_for_file(&file.content_hash, Some(&peer.to_string()))
                        .ok()
                })
                .is_some_and(|downloads| {
                    downloads
                        .iter()
                        .any(|download| matches!(download.state.as_str(), "complete" | "completed"))
                });
            let status = remote_item_status(
                item.remote_status.is_some(),
                peer_online,
                already_downloaded,
                false,
                false,
            );
            item.remote_status = Some(status);
            let status_label = match status {
                RemoteItemStatus::Available => "Available",
                RemoteItemStatus::OfflineCached => "Offline — fetchable when peer returns",
                RemoteItemStatus::AlreadyDownloaded => "Already downloaded",
                RemoteItemStatus::Expired => "Expired",
                RemoteItemStatus::Revoked => "Revoked",
                RemoteItemStatus::Invalid => "Invalid descriptor",
            };
            let can_download = matches!(
                status,
                RemoteItemStatus::Available | RemoteItemStatus::OfflineCached
            );
            let download_button = if can_download {
                button(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Download"),
                )
                .on_press(AppMessage::RequestFileDownload {
                    peer: *peer,
                    file: file.clone(),
                })
                .padding([SPACE_6, SPACE_12])
                .style(BUTTON_PRIMARY)
            } else {
                button(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        status_label,
                    ),
                )
                .padding([SPACE_6, SPACE_12])
                .style(BUTTON_GHOST_BG)
            };
            // PAPIRUS-11: every Shared with Me row uses the same central
            // FileTypeIcon component/resolver as the chat cards and the
            // Shared by Me table. Remote files have no local bytes (until
            // downloaded), so there is no thumbnail to preserve — the
            // resolved Papirus icon answers "what type of file is this?",
            // while the status button answers "what is happening to it".
            // The row already prints the filename and MIME type as text, so
            // the icon is decorative (PAPIRUS-15).
            let type_icon = crate::download_progress_view::decorative_file_type_icon_element(
                &item.display_name,
                Some(file.mime_type.as_str()),
                None,
                crate::file_type_icon::FileTypeIconSize::List,
                &theme,
            );
            rows = rows.push(
                container(
                    Row::new()
                        .push(type_icon)
                        .push(Space::new().width(Length::Fixed(SPACE_12)))
                        .push(
                            Column::new()
                                .push(crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Body,
                                    item.display_name,
                                ))
                                .push(
                                    crate::fonts::type_role_text(
                                        crate::fonts::TypeRole::Metadata,
                                        format!(
                                            "{} · {} · {}",
                                            item.mime_type.as_deref().unwrap_or("unknown type"),
                                            item.size_bytes
                                                .map(crate::dashboard_view_model::format_bytes)
                                                .unwrap_or_else(|| "size unknown".to_string()),
                                            status_label
                                        ),
                                    )
                                    .style(text_muted_style),
                                )
                                .push(
                                    crate::fonts::type_role_text(
                                        crate::fonts::TypeRole::Metadata,
                                        format!("Shared by {peer_label} · content verified"),
                                    )
                                    .style(text_muted_style),
                                )
                                .spacing(SPACE_4)
                                .width(Length::Fill),
                        )
                        .push(download_button)
                        .spacing(SPACE_12)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding(SPACE_12)
                .style(container_surface),
            );
        }

        if visible == 0 {
            rows = rows.push(
                container(
                    Column::new()
                        .push(crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Body,
                            "No validated shared files match this view.",
                        ))
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::SupportingText,
                                "Malformed or unsigned catalogue entries are not offered for download.",
                            )
                            .style(text_muted_style),
                        )
                        .spacing(SPACE_8),
                )
                .padding(SPACE_16)
                .style(container_surface),
            );
        }

        container(
            Column::new()
                .push(header)
                .push(Space::new().height(Length::Fixed(SPACE_12)))
                .push(crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SectionTitle,
                    "Shared with Me",
                ))
                .push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        format!(
                            "{} · {}",
                            peer_label,
                            if peer_online {
                                "peer online"
                            } else {
                                "cached catalogue"
                            }
                        ),
                    )
                    .style(text_muted_style),
                )
                .push(Space::new().height(SPACE_8))
                .push(crate::ui_components::gutter_scrollable(rows).height(Length::Fill))
                .spacing(SPACE_4),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(SPACE_16)
        .style(container_surface)
        .into()
    }

    /// Load the durable "Files I'm Sharing" projection for the Shared by Me
    /// card.
    ///
    /// Runs off the UI thread against the durable shared-files table plus its
    /// file objects and grantor-side permission grants. The projection never
    /// carries a local source path — only a boolean availability flag — and
    /// recipients are relabelled from the friends store so raw grantee ids
    /// never leak into the table.
    pub(crate) fn refresh_shared_by_me(&self) -> iced::Task<AppMessage> {
        let Some(storage) = self.storage.clone() else {
            return iced::Task::done(AppMessage::SharedByMeLoaded(Err(
                "Storage is not available.".to_string(),
            )));
        };
        let friends = self.friends.clone();
        let names = self.names.clone();
        let local_public = self.local_public;
        iced::Task::perform(
            async move {
                let profile_id = local_public.to_string();
                let rows = storage
                    .list_shared_files(&profile_id, true)
                    .map_err(|e| e.to_string())?;
                let mut objects = std::collections::HashMap::new();
                for row in &rows {
                    if let Some(object) = storage
                        .get_file_object(&row.content_hash)
                        .map_err(|e| e.to_string())?
                    {
                        objects.insert(row.content_hash.clone(), object);
                    }
                }
                let mut permissions: std::collections::HashMap<
                    String,
                    Vec<boru_core::storage::SharedFilePermission>,
                > = std::collections::HashMap::new();
                for permission in storage
                    .list_permissions_for_grantor(&profile_id)
                    .map_err(|e| e.to_string())?
                {
                    permissions
                        .entry(permission.content_hash.clone())
                        .or_default()
                        .push(permission);
                }
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let mut projected =
                    crate::shared_by_me_table::build_shared_by_me(&rows, &objects, &permissions, now_ms);
                // Resolve grantee ids to display labels (friends / announced
                // names / short peer id). Never a local path.
                let mut labels = std::collections::HashMap::new();
                for row in &projected {
                    for recipient in &row.recipients {
                        if !labels.contains_key(&recipient.id) {
                            let label = peer_display_label(&friends, &names, &recipient.id);
                            labels.insert(recipient.id.clone(), label);
                        }
                    }
                }
                projected = crate::shared_by_me_table::relabel_recipients(projected, &labels);
                Ok(projected)
            },
            AppMessage::SharedByMeLoaded,
        )
    }

    /// FS-18: rebuild the Shared by Me tab's filtered+sorted projection under
    /// the active global query and sort. The authoritative `shared_by_me_rows`
    /// buffer is never mutated; only this stable view slice is replaced.
    pub(crate) fn refresh_shared_by_me_filter(&mut self) {
        let search_query = self.dashboard_search_input.as_str();
        let mut filtered: Vec<_> = self
            .shared_by_me_rows
            .iter()
            .filter(|row| {
                let mut haystacks: Vec<&str> = vec![row.display_name.as_str()];
                for recipient in &row.recipients {
                    haystacks.push(recipient.label.as_str());
                    haystacks.push(recipient.id.as_str());
                }
                crate::dashboard_filters::query_matches(search_query, &haystacks)
            })
            .cloned()
            .collect();
        self.dashboard_shared_by_me_sort.apply(&mut filtered);
        self.dashboard_shared_by_me_filter = filtered;
    }

    /// UI-30: spawn uniform thumbnail generation for every image/video row in
    /// the Shared by Me table that doesn't have a handle yet.
    ///
    /// Each row loads its `FileObject` (source path or inline data), then
    /// produces a bounded preview off the UI thread: `image_optimizer` for
    /// pictures, `video_poster` for a poster frame of videos. Results arrive
    /// as [`AppMessage::SharedByMeThumbnailReady`]; failures and unsupported
    /// files map to `None` and fall back to the row's type icon.
    pub(crate) fn kick_shared_by_me_thumbnails(&mut self) -> iced::Task<AppMessage> {
        let Some(storage) = self.storage.clone() else {
            return iced::Task::none();
        };
        let cache_dir = self.data_dir.join("cache").join("video-posters");
        let mut tasks: Vec<iced::Task<AppMessage>> = Vec::new();
        for row in &self.shared_by_me_rows {
            let Some(mime) = row.mime_type.as_deref() else {
                continue;
            };
            let is_image = mime.starts_with("image/");
            let is_video = mime.starts_with("video/");
            if !is_image && !is_video {
                continue;
            }
            if self.shared_by_me_thumbnails.contains_key(&row.content_hash) {
                continue;
            }
            let content_hash = row.content_hash.clone();
            let storage = storage.clone();
            let cache_dir = cache_dir.clone();
            tasks.push(iced::Task::perform(
                async move {
                    let handle = generate_shared_by_me_thumbnail(
                        &storage,
                        &content_hash,
                        is_video,
                        &cache_dir,
                    )
                    .await;
                    (content_hash, handle)
                },
                |(content_hash, handle)| AppMessage::SharedByMeThumbnailReady {
                    content_hash,
                    handle,
                },
            ));
        }
        if tasks.is_empty() {
            iced::Task::none()
        } else {
            iced::Task::batch(tasks)
        }
    }

    /// Load the durable transfer-activity projection for the Recent Download
    /// Activity card.
    ///
    /// Runs off the UI thread: (1) persists any transfer lifecycle events not
    /// yet durably recorded (idempotent `INSERT OR IGNORE`, so replays never
    /// duplicate rows), (2) reads back the newest rows, and (3) enriches them
    /// with safe peer/file display labels resolved from the durable
    /// downloads/shared-files tables.  Removed or pruned rows fall back to
    /// neutral historical labels instead of breaking the list.
    pub(crate) fn refresh_dashboard_activity(&self) -> iced::Task<AppMessage> {
        use boru_core::diagnostics::DiagnosticEventKind;

        let Some(storage) = self.storage.clone() else {
            return iced::Task::done(AppMessage::DashboardRecentActivityLoaded(Vec::new()));
        };
        let friends = self.friends.clone();
        let names = self.names.clone();
        let local_public = self.local_public;

        iced::Task::perform(
            async move {
                let diagnostics = boru_core::chat_core::DIAGNOSTICS.clone();
                for event in diagnostics.events_since(0, 1000, None) {
                    if let DiagnosticEventKind::TransferLifecycle(ev) = &event.kind {
                        let _ = storage.record_transfer_activity(ev);
                    }
                }

                let rows = storage.list_transfer_activity(50).unwrap_or_default();

                let mut enrichment =
                    crate::recent_activity_view_model::ActivityEnrichment::default();
                for row in &rows {
                    let Some(download) = download_for_transfer(&storage, &row.transfer_id) else {
                        continue;
                    };
                    enrichment
                        .peer_labels
                        .entry(row.transfer_id.clone())
                        .or_insert_with(|| {
                            peer_display_label(&friends, &names, &download.remote_peer)
                        });
                    let file_label = storage
                        .get_file_object(&download.content_hash)
                        .ok()
                        .flatten()
                        .map(|object| sanitize_single_line(&object.filename))
                        .or_else(|| {
                            storage
                                .get_shared_file(&local_public.to_string(), &download.content_hash)
                                .ok()
                                .flatten()
                                .map(|shared| sanitize_single_line(&shared.display_filename))
                        });
                    if let Some(label) = file_label {
                        enrichment
                            .file_labels
                            .entry(row.transfer_id.clone())
                            .or_insert(label);
                    }
                }

                crate::recent_activity_view_model::project_recent_activity(rows, &enrichment)
            },
            AppMessage::DashboardRecentActivityLoaded,
        )
    }

    // ── Activity Log tab (FS-17) ─────────────────────────────────────

    /// Load the durable Activity Log projection into the tab.
    ///
    /// Runs off the UI thread: (1) persists any transfer lifecycle events not
    /// yet durably recorded (idempotent `INSERT OR IGNORE`, so replays never
    /// duplicate rows), (2) reads back the newest rows up to the storage
    /// bound, and (3) enriches them with safe peer/file display labels
    /// resolved from the durable downloads/shared-files tables.  Removed or
    /// pruned rows fall back to neutral historical labels instead of breaking
    /// the list. Filtering, search, and pagination happen in the view model
    /// over this in-memory buffer — never by refetching.
    pub(crate) fn refresh_activity_log(&self) -> iced::Task<AppMessage> {
        use boru_core::diagnostics::DiagnosticEventKind;

        let Some(storage) = self.storage.clone() else {
            return iced::Task::done(AppMessage::ActivityLogLoaded(Vec::new()));
        };
        let friends = self.friends.clone();
        let names = self.names.clone();
        let local_public = self.local_public;

        iced::Task::perform(
            async move {
                let diagnostics = boru_core::chat_core::DIAGNOSTICS.clone();
                for event in diagnostics.events_since(0, 1000, None) {
                    if let DiagnosticEventKind::TransferLifecycle(ev) = &event.kind {
                        let _ = storage.record_transfer_activity(ev);
                    }
                }

                let rows = storage
                    .list_transfer_activity(
                        crate::activity_log_view_model::STORAGE_ACTIVITY_LIMIT,
                    )
                    .unwrap_or_default();

                let mut enrichment =
                    crate::activity_log_view_model::ActivityLogEnrichment::default();
                for row in &rows {
                    let Some(download) = download_for_transfer(&storage, &row.transfer_id)
                    else {
                        continue;
                    };
                    enrichment
                        .peer_labels
                        .entry(row.transfer_id.clone())
                        .or_insert_with(|| {
                            peer_display_label(&friends, &names, &download.remote_peer)
                        });
                    let file_label = storage
                        .get_file_object(&download.content_hash)
                        .ok()
                        .flatten()
                        .map(|object| sanitize_single_line(&object.filename))
                        .or_else(|| {
                            storage
                                .get_shared_file(&local_public.to_string(), &download.content_hash)
                                .ok()
                                .flatten()
                                .map(|shared| sanitize_single_line(&shared.display_filename))
                        });
                    if let Some(label) = file_label {
                        enrichment
                            .file_labels
                            .entry(row.transfer_id.clone())
                            .or_insert(label);
                    }
                }

                crate::activity_log_view_model::project_activity_log(rows, &enrichment)
            },
            AppMessage::ActivityLogLoaded,
        )
    }

    /// ── Downloaded tab (FS-15) ────────────────────────────────────────

    /// Load the durable completed-download projection into the Downloaded tab.
    ///
    /// History comes exclusively from the `downloads` table; the dashboard
    /// never scans arbitrary download directories to invent records. The
    /// destination path is resolved to a truthful local state (Verified /
    /// Warning / Missing) so Open/Reveal are only offered while the file
    /// still exists.
    pub(crate) fn refresh_downloaded_history(&self) -> iced::Task<AppMessage> {
        let Some(storage) = self.storage.clone() else {
            return iced::Task::done(AppMessage::DashboardDownloadedLoaded(Err(
                "Storage is not available.".to_string(),
            )));
        };
        let friends = self.friends.clone();
        let names = self.names.clone();

        iced::Task::perform(
            async move {
                let records = storage.list_completed_downloads().map_err(|e| e.to_string())?;
                let mut items = Vec::with_capacity(records.len());
                for record in records {
                    let local = local_file_state(
                        record.destination_path.as_deref(),
                        record.total_bytes,
                    );
                    let peer_label = peer_display_label(&friends, &names, &record.remote_peer);
                    items.push(crate::dashboard_view_model::project_completed_download(
                        &record, &peer_label, local,
                    ));
                }
                crate::dashboard_view_model::sort_completed_downloads(&mut items);
                Ok(items)
            },
            AppMessage::DashboardDownloadedLoaded,
        )
    }

    /// Open a completed download with the native OS handler. Only offered
    /// when the local file still exists; the existence check is re-run here
    /// so a race between render and click cannot open a stale path.
    pub(crate) fn open_downloaded_item(&self, id: i64) -> iced::Task<AppMessage> {
        let Some(item) = self
            .downloaded_history
            .iter()
            .find(|item| item.id.as_str() == format!("download:{id}"))
        else {
            return iced::Task::none();
        };
        let Some(path) = item.destination_path.clone() else {
            return iced::Task::done(AppMessage::ErrorMsg(
                "The local file is no longer available.".to_string(),
            ));
        };
        if !std::path::Path::new(&path).is_file() {
            return iced::Task::done(AppMessage::ErrorMsg(
                "The local file is no longer available.".to_string(),
            ));
        }
        iced::Task::perform(async move { open::that(path) }, |result| {
            if let Err(e) = result {
                AppMessage::ErrorMsg(format!("Could not open file: {e}"))
            } else {
                AppMessage::Noop
            }
        })
    }

    /// Reveal a completed download in the OS file manager. Cross-platform and
    /// only offered while the local file still exists.
    pub(crate) fn reveal_downloaded_item(&self, id: i64) -> iced::Task<AppMessage> {
        let Some(item) = self
            .downloaded_history
            .iter()
            .find(|item| item.id.as_str() == format!("download:{id}"))
        else {
            return iced::Task::none();
        };
        let Some(path) = item.destination_path.clone() else {
            return iced::Task::done(AppMessage::ErrorMsg(
                "The local file is no longer available.".to_string(),
            ));
        };
        if !std::path::Path::new(&path).is_file() {
            return iced::Task::done(AppMessage::ErrorMsg(
                "The local file is no longer available.".to_string(),
            ));
        }
        iced::Task::perform(async move { reveal_in_folder(std::path::Path::new(&path)) }, |result| {
            if let Err(e) = result {
                AppMessage::ErrorMsg(format!("Could not reveal file: {e}"))
            } else {
                AppMessage::Noop
            }
        })
    }

    /// Build the "Recent Download Activity (by Others)" card (FS-12).
    ///
    /// Shows the durable activity projection newest-first: peer identity,
    /// file/folder, normalized action, local timestamp, and a compact
    /// success/error/warning status with an icon plus real text so the state
    /// is never colour-only.  Rows fall back to safe historical labels when
    /// the underlying item was removed or pruned.
    /// PERF-2: snapshot selector for the Recent Download Activity card.
    pub(crate) fn recent_activity_card_dependency(&self) -> RecentActivityCardDependency {
        RecentActivityCardDependency {
            dark_mode: self.dark_mode,
            tick: self.activity_tick,
            rows: self.dashboard_recent_activity.clone(),
        }
    }

    /// PERF-2: static renderer for the Recent Download Activity card, run
    /// inside `iced::widget::lazy` so it is only re-invoked when the activity
    /// rows or the per-second tick actually change.
    pub(crate) fn view_recent_download_activity_card(
        dep: &RecentActivityCardDependency,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Background, Border, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);
        let rows = &dep.rows;

        let activity_rows: Vec<iced::Element<'static, AppMessage>> = rows
            .iter()
            .map(|event| Self::recent_activity_row(event, &theme))
            .collect();

        // Header: uppercase muted title, count badge, "View full activity log"
        // ghost action that selects the Activity Log tab.
        let mut header = Row::new()
            .spacing(SPACE_6)
            .align_y(Alignment::Center)
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Recent Activity")
                    .color(crate::design_tokens::text_muted(&theme)),
            )
            .push(
                container(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        rows.len().to_string(),
                    )
                    .color(crate::design_tokens::primary(&theme)),
                )
                .padding([1.0, SPACE_8])
                .style(move |t| container::Style {
                    background: Some(Background::Color(crate::design_tokens::primary_soft(t))),
                    border: Border {
                        radius: SPACE_12.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            )
            .push(Space::new().width(Length::Fill));

        header = header.push(
            button(
                Row::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "View full activity log",
                        ),
                    )
                    .push(
                        Icon::ChevronRight
                            .build()
                            .size(crate::icon_system::IconSize::Xs)
                            .color_fn(crate::design_tokens::text_secondary)
                            .build(),
                    )
                    .spacing(SPACE_2)
                    .align_y(Alignment::Center),
            )
            .on_press(AppMessage::DashboardTabSelected(
                crate::dashboard_view_model::DashboardTab::ActivityLog,
            ))
            .padding([SPACE_2, SPACE_6])
            .style(|t, status| {
                let color = match status {
                    iced::widget::button::Status::Hovered => crate::design_tokens::primary(t),
                    iced::widget::button::Status::Pressed => {
                        crate::design_tokens::primary_pressed(t)
                    }
                    _ => crate::design_tokens::text_secondary(t),
                };
                button::Style {
                    background: None,
                    text_color: color,
                    ..Default::default()
                }
            }),
        );

        let body: iced::Element<'_, AppMessage> = if activity_rows.is_empty() {
            // Retention-aware empty state: never implies sharing is broken.
            container(
                Column::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Body,
                            "No recent download activity yet.",
                        )
                        .color(crate::design_tokens::text_secondary(&theme)),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Peer requests and completed transfers appear here while kept by the local activity retention window.",
                        )
                        .color(crate::design_tokens::text_muted(&theme)),
                    )
                    .spacing(SPACE_4)
                    .align_x(Alignment::Start),
            )
            .width(Length::Fill)
            .padding([SPACE_6, 0.0])
            .into()
        } else {
            crate::ui_components::gutter_scrollable(
                Column::with_children(activity_rows)
                    .spacing(SPACE_2)
                    .width(Length::Fill),
            )
            .height(Length::Fixed(200.0))
            .width(Length::Fill)
            .into()
        };

        container(
            Column::new()
                .push(header)
                .push(Space::new().height(Length::Fixed(SPACE_6)))
                .push(body)
                .spacing(0)
                .width(Length::Fill),
        )
        .padding([SPACE_12, SPACE_16])
        .width(Length::Fill)
        .style(|t| crate::design_tokens::card_style(t))
        .into()
    }

    /// One compact row in the Recent Download Activity card: status icon,
    /// file label, peer · action · size sub-line, and relative timestamp.
    /// Static (no `&self`) so it can run inside the lazy card builder. The
    /// body clones every field it renders, so the element is fully `'static`.
    pub(crate) fn recent_activity_row(
        event: &crate::recent_activity_view_model::RecentActivityRow,
        theme: &iced::Theme,
    ) -> iced::Element<'static, AppMessage> {
        use crate::recent_activity_view_model::ActivityStatus;
        use iced::widget::{container, row, Column, Space};
        use iced::{Alignment, Length};

        let ago = crate::presentation::relative_time(event.occurred_at_ms);
        let (icon, color_fn): (Icon, fn(&iced::Theme) -> iced::Color) = match event.status {
            ActivityStatus::Success => (
                Icon::Check,
                crate::design_tokens::color_success as fn(&iced::Theme) -> iced::Color,
            ),
            ActivityStatus::Error => (
                Icon::AlertTriangle,
                crate::design_tokens::color_danger as fn(&iced::Theme) -> iced::Color,
            ),
            ActivityStatus::Warning => (
                Icon::AlertTriangle,
                crate::design_tokens::color_warning as fn(&iced::Theme) -> iced::Color,
            ),
            ActivityStatus::Info => (
                Icon::Activity,
                crate::design_tokens::text_muted as fn(&iced::Theme) -> iced::Color,
            ),
        };
        let status_label = event.status.label();

        let size_label = event.bytes.map(crate::dashboard_view_model::format_bytes);
        let sub_line = match (&event.detail, size_label) {
            (Some(detail), Some(size)) => format!("{} · {} · {size}", event.peer_label, detail),
            (Some(detail), None) => format!("{} · {}", event.peer_label, detail),
            (None, Some(size)) => format!("{} · {size}", event.peer_label),
            (None, None) => event.peer_label.clone(),
        };

        container(
            row![
                // Status icon with the accessible status label as real text
                // next to it (colour is never the only signal).
                icon.build()
                    .size(crate::icon_system::IconSize::Xs)
                    .color_fn(color_fn)
                    .build(),
                // PAPIRUS-11: the file-type icon (same central component /
                // resolver as chat cards and the other dashboard rows)
                // answers "what type of file is this?"; the status icon +
                // status label answer "what is happening to it" — status
                // stays separate from the file-type icon (Task 13).
                crate::download_progress_view::file_type_icon_element(
                    &event.file_label,
                    None,
                    None,
                    crate::file_type_icon::FileTypeIconSize::Compact,
                    theme,
                ),
                Column::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Body,
                            crate::presentation::truncate_with_ellipsis(&event.file_label, 42),
                        )
                        .color(crate::design_tokens::text_primary(theme))
                        .wrapping(iced::widget::text::Wrapping::None),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            crate::presentation::truncate_with_ellipsis(&sub_line, 64),
                        )
                        .color(crate::design_tokens::text_muted(theme))
                        .wrapping(iced::widget::text::Wrapping::None),
                    )
                    .spacing(0)
                    .width(Length::Fill),
                Space::new().width(Length::Fixed(SPACE_8)),
                Column::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::BodyEmphasised,
                            event.action.clone(),
                        )
                        .color(match event.status {
                                ActivityStatus::Success =>
                                    crate::design_tokens::color_success(theme),
                                ActivityStatus::Error => crate::design_tokens::color_danger(theme),
                                ActivityStatus::Warning =>
                                    crate::design_tokens::color_warning(theme),
                                ActivityStatus::Info => crate::design_tokens::text_secondary(theme),
                            }),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            format!("{status_label} · {ago}"),
                        )
                        .color(crate::design_tokens::text_muted(theme)),
                    )
                    .spacing(0)
                    .align_x(Alignment::End),
            ]
            .spacing(SPACE_6)
            .align_y(Alignment::Center),
        )
        .height(Length::Fixed(crate::card_shell::CARD_ROW_HEIGHT))
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .into()
    }

    /// Apply one FS-05 projection update to the panel state.
    ///
    /// Outbound records drive the "Peers Downloading from Me" card;
    /// inbound records drive the Downloading tab. Terminal records are
    /// archived exactly once (the projection emits each terminal transition
    /// once); re-applying a terminal update for an already-archived transfer
    /// is a no-op thanks to the id check. Active records overwrite in place,
    /// so a row never duplicates.
    pub(crate) fn apply_transfer_update(&mut self, record: TransferRecord) {
        match record.direction {
            TransferDirection::Outbound => self.apply_outbound_update(record),
            TransferDirection::Inbound => self.apply_inbound_update(record),
        }
    }

    /// Apply one FS-05 projection update to the OUTBOUND panel state.
    ///
    /// New active records push a Recent Activity "started downloading" event
    /// (deduped: only when the transfer id is not already live or archived);
    /// terminal `Completed` records push a "finished downloading" event the
    /// single time they are archived. Progress updates never emit activity.
    pub(crate) fn apply_outbound_update(&mut self, record: TransferRecord) {
        if record.direction != TransferDirection::Outbound {
            return;
        }
        if record.state.is_terminal() {
            let was_active = self.outbound_active.remove(&record.transfer_id).is_some();
            let is_new = was_active
                || !self
                    .outbound_history
                    .iter()
                    .any(|existing| existing.transfer_id == record.transfer_id);
            if is_new {
                if record.state == TransferState::Completed {
                    self.push_outbound_activity(&record, true);
                }
                self.outbound_history.push_front(record);
                self.outbound_history.truncate(MAX_OUTBOUND_HISTORY);
            }
        } else {
            let is_new = !self.outbound_active.contains_key(&record.transfer_id)
                && !self
                    .outbound_history
                    .iter()
                    .any(|existing| existing.transfer_id == record.transfer_id);
            self.outbound_history
                .retain(|existing| existing.transfer_id != record.transfer_id);
            if is_new {
                self.push_outbound_activity(&record, false);
            }
            self.outbound_active
                .insert(record.transfer_id.clone(), record);
        }
    }

    /// Push a Recent Activity entry for an outbound transfer transition.
    ///
    /// `completed=false` emits "started downloading", `completed=true` emits
    /// "finished downloading". The peer is resolved to a verified display
    /// name from the authenticated peer id (never an untrusted string); the
    /// file label comes from the outbound item-label enrichment and falls
    /// back to a short hash prefix rather than a fabricated name.
    pub(crate) fn push_outbound_activity(&mut self, record: &TransferRecord, completed: bool) {
        let peer_display = record
            .peer_id
            .as_deref()
            .and_then(|id| id.parse::<PublicKey>().ok())
            .map(|pk| self.resolve_name(&pk))
            .unwrap_or_else(|| "A peer".to_string());
        let file_label = self
            .outbound_item_labels
            .lock()
            .map(|guard| {
                guard
                    .get(&record.item_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        let prefix: String = record.item_id.chars().take(12).collect();
                        format!("file {prefix}…")
                    })
            })
            .unwrap_or_else(|_| "a file".to_string());
        let description = if completed {
            format!("{peer_display} finished downloading {file_label} from you")
        } else {
            format!("{peer_display} started downloading {file_label} from you")
        };
        self.push_activity(description, ActivityKind::FileShared);
    }

    /// Rebuild the outbound panel maps from a projection snapshot.
    ///
    /// Used after the broadcast receiver lags or restarts (event replay):
    /// the snapshot is authoritative, so the active map and history can never
    /// contain stale or duplicate rows afterwards. Terminal records go to the
    /// bounded history (newest first), everything else stays live.
    pub(crate) fn resync_outbound_panel(&mut self, snapshot: &[TransferRecord]) {
        self.outbound_active.clear();
        self.outbound_history.clear();
        for record in snapshot {
            if record.direction != TransferDirection::Outbound {
                continue;
            }
            if record.state.is_terminal() {
                self.outbound_history.push_back(record.clone());
            } else {
                self.outbound_active
                    .insert(record.transfer_id.clone(), record.clone());
            }
        }
        let mut history: Vec<TransferRecord> = self.outbound_history.drain(..).collect();
        history.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.transfer_id.cmp(&b.transfer_id))
        });
        history.truncate(MAX_OUTBOUND_HISTORY);
        self.outbound_history = history.into();
    }

    /// Apply one FS-05 projection update to the INBOUND panel state
    /// (Downloading tab). Mirrors `apply_transfer_update` for outbound rows.
    ///
    /// Terminal records are archived exactly once; re-applying a terminal
    /// update for an already-archived transfer is a no-op. Active records
    /// overwrite in place, so a row never duplicates.
    pub(crate) fn apply_inbound_update(&mut self, record: TransferRecord) {
        if record.direction != TransferDirection::Inbound {
            return;
        }
        if record.state.is_terminal() {
            if self.inbound_active.remove(&record.transfer_id).is_some()
                || !self
                    .inbound_history
                    .iter()
                    .any(|existing| existing.transfer_id == record.transfer_id)
            {
                self.inbound_history.push_front(record);
                self.inbound_history
                    .truncate(crate::downloading_view_model::MAX_INBOUND_HISTORY);
            }
        } else {
            self.inbound_history
                .retain(|existing| existing.transfer_id != record.transfer_id);
            self.inbound_active
                .insert(record.transfer_id.clone(), record);
        }
    }

    /// Rebuild the inbound panel maps from a projection snapshot.
    ///
    /// Used after the broadcast receiver lags or restarts (event replay):
    /// the snapshot is authoritative, so the active map and history can never
    /// contain stale or duplicate rows afterwards. Terminal records go to the
    /// bounded history (newest first), everything else stays live.
    pub(crate) fn resync_inbound_panel(&mut self, snapshot: &[TransferRecord]) {
        self.inbound_active.clear();
        self.inbound_history.clear();
        for record in snapshot {
            if record.direction != TransferDirection::Inbound {
                continue;
            }
            if record.state.is_terminal() {
                self.inbound_history.push_back(record.clone());
            } else {
                self.inbound_active
                    .insert(record.transfer_id.clone(), record.clone());
            }
        }
        let mut history: Vec<TransferRecord> = self.inbound_history.drain(..).collect();
        history.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.transfer_id.cmp(&b.transfer_id))
        });
        history.truncate(crate::downloading_view_model::MAX_INBOUND_HISTORY);
        self.inbound_history = history.into();
    }

    /// Cancel one inbound transfer from the Downloading tab.
    ///
    /// The transfer id comes from the FS-05 projection. Cancellation follows
    /// the backend's real cancellation flow: a `Cancelled` lifecycle event is
    /// published to the projection (the reducer archives the row exactly
    /// once) and, when a durable download row maps to the same content hash,
    /// `DownloadManager::cancel_download` marks it cancelled in storage and
    /// signals the in-flight worker. A system message explains partial-file
    /// handling (the transfer layer removes the temp file on cancellation; a
    /// partial download is never kept as a final file).
    pub(crate) fn cancel_inbound_transfer(&mut self, transfer_id: &str) {
        let Some(record) = self.inbound_active.get(transfer_id).cloned() else {
            self.push_system(format!("Transfer {transfer_id} is not active."));
            return;
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let cancel_event = TransferEvent {
            event_id: format!("ui-cancel:{transfer_id}:{now_ms}"),
            transfer_id: transfer_id.to_string(),
            item_id: record.item_id.clone(),
            direction: TransferDirection::Inbound,
            peer_id: record.peer_id.clone(),
            sequence: record.updated_at_ms.max(now_ms) + 1,
            attempt: record.attempt,
            occurred_at_ms: now_ms,
            kind: EventName::Cancelled,
            bytes: record.bytes,
            total_bytes: record.total_bytes,
            error: None,
        };
        self.transfer_store.publish(cancel_event);
        // Locally reflect the authoritative transition so the row moves to
        // history even before the broadcast round-trips.
        let mut cancelled = record.clone();
        cancelled.state = TransferState::Cancelled;
        cancelled.updated_at_ms = now_ms;
        self.apply_inbound_update(cancelled);

        // Durable cancellation: find a non-terminal download row for the same
        // content hash and ask the backend to cancel it. If no row exists
        // (legacy chat-path transfer) the projection event above is the
        // cancellation signal and the transfer layer cleans up its temp file
        // when the future is dropped.
        let cancelled_any = match self.download_manager.clone() {
            Some(dm) => match dm.lock() {
                Ok(mut guard) => {
                    let mut cancelled_any = false;
                    for state in
                        ["queued", "active", "paused", "resolving_peer", "downloading"]
                    {
                        let rows = self
                            .storage
                            .as_ref()
                            .and_then(|stg| stg.list_downloads_by_state(state).ok())
                            .unwrap_or_default();
                        for row in rows {
                            if row.content_hash == record.item_id
                                && guard.cancel_download(row.id).is_ok()
                            {
                                cancelled_any = true;
                            }
                        }
                    }
                    cancelled_any
                }
                Err(_) => false,
            },
            None => false,
        };
        if cancelled_any {
            self.push_system(
                "Download cancelled — the partial file was cleaned up; nothing was saved."
                    .to_string(),
            );
        } else {
            self.push_system(
                "Download cancelled — partial bytes were discarded; nothing was saved.".to_string(),
            );
        }
    }

    /// Pause one inbound transfer from the Download Manager.
    ///
    /// The transfer id comes from the FS-05 projection. Pausing is only
    /// supported for durable download rows (matched by content hash): the
    /// backend `DownloadManager::pause_download` signals the in-flight worker
    /// and records the paused state. Transfers without a durable row (legacy
    /// chat-path) have no pause seam — a truthful system message explains
    /// that.
    pub(crate) fn pause_inbound_transfer(&mut self, transfer_id: &str) {
        let Some(record) = self.inbound_active.get(transfer_id).cloned() else {
            self.push_system(format!("Transfer {transfer_id} is not active."));
            return;
        };
        let Some(dm) = self.download_manager.clone() else {
            self.push_system("Pause is not available for this transfer.".to_string());
            return;
        };
        let Ok(mut guard) = dm.lock() else {
            self.push_system("Pause failed — download manager unavailable.".to_string());
            return;
        };
        let mut paused_any = false;
        for state in ["queued", "active", "resolving_peer", "requesting_permission", "downloading", "verifying"] {
            let rows = self
                .storage
                .as_ref()
                .and_then(|stg| stg.list_downloads_by_state(state).ok())
                .unwrap_or_default();
            for row in rows {
                if row.content_hash == record.item_id && guard.pause_download(row.id).is_ok() {
                    paused_any = true;
                }
            }
        }
        if paused_any {
            self.paused_inbound_transfer_ids.insert(transfer_id.to_string());
            self.push_system(
                "Download paused — transfer suspended; use Resume to continue.".to_string(),
            );
        } else {
            self.push_system(
                "Pause is not supported for this transfer (no durable download record)."
                    .to_string(),
            );
        }
    }

    /// Resume a paused inbound transfer from the Download Manager.
    ///
    /// Mirrors [`Self::pause_inbound_transfer`]: the durable download row
    /// (matched by content hash) transitions back to an active state via
    /// `DownloadManager::resume_download`.
    pub(crate) fn resume_inbound_transfer(&mut self, transfer_id: &str) {
        let Some(record) = self.inbound_active.get(transfer_id).cloned() else {
            self.push_system(format!("Transfer {transfer_id} is not active."));
            return;
        };
        let Some(dm) = self.download_manager.clone() else {
            self.push_system("Resume is not available for this transfer.".to_string());
            return;
        };
        let Ok(mut guard) = dm.lock() else {
            self.push_system("Resume failed — download manager unavailable.".to_string());
            return;
        };
        let mut resumed_any = false;
        let rows = self
            .storage
            .as_ref()
            .and_then(|stg| stg.list_downloads_by_state("paused").ok())
            .unwrap_or_default();
        for row in rows {
            if row.content_hash == record.item_id && guard.resume_download(row.id).is_ok() {
                resumed_any = true;
            }
        }
        if resumed_any {
            self.paused_inbound_transfer_ids.remove(transfer_id);
            self.push_system("Download resumed.".to_string());
        } else {
            self.push_system(
                "Nothing to resume — no paused download record for this transfer.".to_string(),
            );
        }
    }

    /// Stop an outbound upload from the Download Manager.
    ///
    /// The outbound side is driven by the blob provider; the app has no
    /// provider-level abort handle, so stopping is expressed through the
    /// authoritative FS-05 projection: a `Cancelled` event is published for
    /// the outbound direction (archived once, exactly like inbound cancel)
    /// and the row leaves the active list immediately.
    pub(crate) fn stop_outbound_transfer(&mut self, transfer_id: &str) {
        let Some(record) = self.outbound_active.get(transfer_id).cloned() else {
            self.push_system(format!("Upload {transfer_id} is not active."));
            return;
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let cancel_event = TransferEvent {
            event_id: format!("ui-stop:{transfer_id}:{now_ms}"),
            transfer_id: transfer_id.to_string(),
            item_id: record.item_id.clone(),
            direction: TransferDirection::Outbound,
            peer_id: record.peer_id.clone(),
            sequence: record.updated_at_ms.max(now_ms) + 1,
            attempt: record.attempt,
            occurred_at_ms: now_ms,
            kind: EventName::Cancelled,
            bytes: record.bytes,
            total_bytes: record.total_bytes,
            error: None,
        };
        self.transfer_store.publish(cancel_event);
        // Locally reflect the authoritative transition so the row leaves the
        // active list immediately.
        let mut stopped = record.clone();
        stopped.state = TransferState::Cancelled;
        stopped.updated_at_ms = now_ms;
        self.apply_outbound_update(stopped);
        self.push_system("Upload stopped — the transfer was removed from active uploads.".to_string());
    }

    /// Live "Peers Downloading from Me" panel — the FS-08 upper-right card.
    ///
    /// Rows come from the FS-05 outbound projection (stable transfer ids);
    /// peer labels are resolved from the authenticated peer id, never from a
    /// display string. Unknown totals render an indeterminate bar plus byte
    /// count; no percentage is fabricated.
    pub(crate) fn view_peers_downloading_from_me(&self, theme: &iced::Theme) -> iced::Element<'_, AppMessage> {
        use crate::card_shell::CardShell;
        use crate::dashboard_view_model::{outbound_row, sort_outbound_rows, PeerDownload};

        let labels = self
            .outbound_item_labels
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let mut rows: Vec<PeerDownload> = self
            .outbound_active
            .values()
            .map(|record| outbound_row(record, &labels))
            .collect();
        sort_outbound_rows(&mut rows);
        let active_count = rows.len();

        let children: Vec<iced::Element<'_, AppMessage>> = rows
            .into_iter()
            .map(|row| self.peer_download_row(row, theme))
            .collect();

        CardShell::new("Peers Downloading from Me", children)
            .count(active_count)
            .on_view_all(AppMessage::DashboardTabSelected(
                crate::dashboard_view_model::DashboardTab::Downloading,
            ))
            .empty_message("No one is downloading from you right now.")
            .max_height(240.0)
            .build(theme)
    }

    /// One compact outbound transfer row. Consumes the row so the returned
    /// element owns its labels (the caller's row vector does not outlive the
    /// view).
    pub(crate) fn peer_download_row<'a>(
        &'a self,
        row: crate::dashboard_view_model::PeerDownload,
        theme: &iced::Theme,
    ) -> iced::Element<'a, AppMessage> {
        use crate::dashboard_view_model::{format_bytes, Progress as VMProgress};
        use crate::ui_components::ProgressBar;
        use iced::widget::{container, Column, Row, Space};
        use iced::{Alignment, Border, Length};

        // Authenticated identity is the only source of the peer label; the
        // projection never carries an untrusted display string for peers.
        let peer_display = row
            .peer_label
            .parse::<PublicKey>()
            .ok()
            .map(|pk| self.resolve_name(&pk))
            .unwrap_or_else(|| "Unknown peer".to_string());
        let online = row
            .peer_label
            .parse::<PublicKey>()
            .ok()
            .map(|pk| matches!(self.peer_presence(&pk), PeerPresence::Online))
            .unwrap_or(false);

        let (state_label, state_color) = match row.state {
            crate::dashboard_view_model::OutboundState::Transferring => {
                ("Transferring", crate::design_tokens::primary(theme))
            }
            crate::dashboard_view_model::OutboundState::Retrying => {
                ("Retrying", crate::design_tokens::color_warning(theme))
            }
            crate::dashboard_view_model::OutboundState::Verifying => {
                ("Verifying", crate::design_tokens::color_warning(theme))
            }
            crate::dashboard_view_model::OutboundState::Completed => {
                ("Completed", crate::design_tokens::color_success(theme))
            }
            crate::dashboard_view_model::OutboundState::Failed => {
                ("Failed", crate::design_tokens::color_danger(theme))
            }
            crate::dashboard_view_model::OutboundState::Cancelled => {
                ("Cancelled", crate::design_tokens::text_muted(theme))
            }
            crate::dashboard_view_model::OutboundState::Disconnected => {
                ("Disconnected", crate::design_tokens::color_danger(theme))
            }
        };

        let (bar, progress_text) = match &row.progress {
            VMProgress::Determinate { bytes, total } if *total > 0 => {
                let pct = ((*bytes as f64 / *total as f64) * 100.0).min(100.0) as u8;
                (
                    ProgressBar::<AppMessage>::new(pct as f32 / 100.0)
                        .show_label(false)
                        .bold()
                        .build(theme),
                    format!("{}%", pct),
                )
            }
            VMProgress::Determinate { bytes, .. } => (
                ProgressBar::<AppMessage>::new(0.0)
                    .indeterminate(true)
                    .bold()
                    .build(theme),
                format!("{} received", format_bytes(*bytes)),
            ),
            VMProgress::Indeterminate { bytes } => (
                ProgressBar::<AppMessage>::new(0.0)
                    .indeterminate(true)
                    .bold()
                    .build(theme),
                format!("{} received", format_bytes(*bytes)),
            ),
            VMProgress::Unknown => (
                ProgressBar::<AppMessage>::new(0.0)
                    .show_label(false)
                    .bold()
                    .build(theme),
                "—".to_string(),
            ),
        };

        let avatar: iced::Element<'_, AppMessage> = Avatar::<AppMessage>::new(&peer_display)
            .size(28.0)
            .online_dot(online)
            .dark_mode(self.dark_mode)
            .build();

        let name_line = Row::new()
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::BodyEmphasised,
                    peer_display,
                )
                .color(crate::design_tokens::text_primary(theme))
                .width(Length::Shrink)
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(Space::new().width(Length::Fill))
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, state_label)
                    .color(state_color)
                    .width(Length::Shrink),
            )
            .align_y(Alignment::Center);

        // PAPIRUS-11: the transferred file leads with the same central
        // FileTypeIcon component/resolver as chat cards — the icon answers
        // "what type of file is this?", the state label + progress answer
        // "what is happening to it".
        let type_icon = crate::download_progress_view::file_type_icon_element(
            &row.display_name,
            None,
            None,
            crate::file_type_icon::FileTypeIconSize::Compact,
            theme,
        );

        let file_line = Row::new()
            .push(type_icon)
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    row.display_name,
                )
                .style(text_muted_style)
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(SPACE_4)
            .align_y(Alignment::Center);

        let progress_line = Row::new()
            .push(bar)
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, progress_text)
                    .style(text_muted_style)
                    .width(Length::Shrink),
            )
            .spacing(SPACE_4)
            .align_y(Alignment::Center);

        let text_col = Column::new()
            .push(name_line)
            .push(file_line)
            .push(progress_line)
            .spacing(SPACE_2)
            .width(Length::Fill);

        let mut row_el = Row::new()
            .push(avatar)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(text_col)
            .spacing(0)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        if let Some(error) = row.error {
            let error_line = Row::new()
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, error)
                        .color(crate::design_tokens::color_danger(theme))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::None),
                )
                .spacing(0);
            row_el = Row::new()
                .push(row_el)
                .push(Space::new().width(Length::Fixed(SPACE_4)))
                .push(error_line)
                .spacing(0)
                .align_y(Alignment::Center);
        }

        container(row_el)
            .width(Length::Fill)
            .padding([SPACE_6, SPACE_4])
            .style(move |t| container::Style {
                background: None,
                border: Border {
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// Render the Downloaded tab: durable completed-download history with
    /// name/type/size, source peer, completed time, integrity state, and safe
    /// local actions (Open / Reveal in Folder only while the file exists;
    /// Remove from history never deletes the file).
    /// PERF-2: the Downloaded tab renders its full content through a lazy
    /// wrapper keyed on [`DownloadsCardDependency`], so the table subtree is
    /// cached unless the history, search query, or sort actually change.
    pub(crate) fn view_downloaded(&self) -> iced::Element<'_, AppMessage> {
        iced::widget::lazy(self.downloads_card_dependency(), Self::view_downloads_card).into()
    }

    /// PERF-2: snapshot selector for the "Downloads" (Downloaded tab) card.
    pub(crate) fn downloads_card_dependency(&self) -> DownloadsCardDependency {
        DownloadsCardDependency {
            dark_mode: self.dark_mode,
            active: self.dashboard_active_tab
                == crate::dashboard_view_model::DashboardTab::Downloaded,
            history: self.downloaded_history.clone(),
            history_loaded: self.downloaded_history_loaded,
            history_error: self.downloaded_history_error.clone(),
            search_query: self.dashboard_search_input.clone(),
            sort: self.dashboard_downloaded_sort,
        }
    }

    /// PERF-2: static renderer for the "Downloads" (Downloaded tab) card, run
    /// inside `iced::widget::lazy` so it is only re-invoked when the history,
    /// load flags, search query, or sort actually change.
    pub(crate) fn view_downloads_card(dep: &DownloadsCardDependency) -> iced::Element<'static, AppMessage> {
        use iced::widget::{Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);

        // Loading skeleton on first open.
        if !dep.history_loaded && dep.history_error.is_none() {
            return crate::ui_components::gutter_scrollable(
                Column::new()
                    .push(dashboard_card(
                        crate::ui_components::LoadingSkeleton::new(4)
                            .row_height(crate::design_tokens::TABLE_ROW_HEIGHT)
                            .build(&theme),
                    ))
                    .spacing(SPACE_16)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        }

        // Inline error with retry.
        if let Some(error) = &dep.history_error {
            let retry = crate::ui_components::InlineError::new(error)
                .on_retry(AppMessage::DashboardDownloadedRefresh)
                .build(&theme);
            return crate::ui_components::gutter_scrollable(
                Column::new()
                    .push(dashboard_card(retry))
                    .spacing(SPACE_16)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        }

        // Empty state.
        if dep.history.is_empty() {
            return crate::ui_components::empty_state(
                Icon::Check,
                "No completed downloads.",
                "Files you receive will appear here with their source peer and verification status.",
                None,
                None,
            )
            .into();
        }

        // Apply the shared search filter to name and source peer label using
        // the FS-18 normalized matcher, then apply the Downloaded tab's active
        // sort to the filtered rows only. Rows stay borrows into the
        // authoritative history buffer — nothing is copied or mutated.
        let query = dep.search_query.as_str();
        let mut filtered: Vec<_> = dep
            .history
            .iter()
            .filter(|item| {
                crate::dashboard_filters::query_matches(
                    query,
                    &[item.display_name.as_str(), item.source_peer.as_str()],
                )
            })
            .collect();
        dep.sort.apply_ref(&mut filtered);

        if filtered.is_empty() {
            return crate::ui_components::empty_state(
                Icon::Search,
                "No matching downloads.",
                "Try a different search term.",
                None,
                None,
            )
            .into();
        }

        // Header row with count.
        let count_label = filtered.len().to_string();
        let header_row = Row::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Downloaded"),
            )
            .push(crate::ui_components::badge_owned(
                count_label,
                crate::ui_components::BadgeKind::Count,
            ))
            .push(Space::new().width(Length::Fill))
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        // FS-18: sort control row (Downloaded: completed time / name / size).
        let sort = dep.sort;
        let mut sort_row = Row::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Sort:")
                    .style(text_muted_style),
            )
            .spacing(SPACE_6)
            .align_y(Alignment::Center);
        for key in crate::dashboard_filters::DownloadedSortKey::ALL.iter() {
            sort_row = sort_row.push(dashboard_sort_chip(
                &theme,
                key.label(),
                sort.key == *key,
                sort.descending,
                AppMessage::DashboardDownloadedSortClicked(*key),
            ));
        }

        // Column headers (Name | Size | Source | Completed | Status | Actions).
        let header = crate::ui_components::TableHeaderRow::new(vec![
            ("Name", None),
            ("Size", Some(72.0)),
            ("Source", Some(120.0)),
            ("Completed", Some(120.0)),
            ("Status", Some(140.0)),
            ("Actions", Some(160.0)),
        ])
        .build(&theme);

        let mut rows = Column::new().spacing(SPACE_4);

        for item in filtered {
            let row_el = Self::downloaded_row(item, &theme);
            rows = rows.push(row_el);
        }

        let body = Column::new()
            .push(header_row)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(sort_row)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(header)
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(rows)
            .spacing(0)
            .width(Length::Fill);

        crate::ui_components::gutter_scrollable(dashboard_card(body.into())).width(Length::Fill).height(Length::Fill).into()
    }

    /// One row of the Downloaded tab. Static (no `&self`) so it can run inside
    /// the lazy card builder. The body clones every field it renders, so the
    /// element is fully `'static`.
    pub(crate) fn downloaded_row(
        item: &crate::dashboard_view_model::CompletedDownloadItem,
        theme: &iced::Theme,
    ) -> iced::Element<'static, AppMessage> {
        use crate::dashboard_view_model::LocalFileState;
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Border, Length};

        let size_label = crate::dashboard_view_model::format_bytes(item.size_bytes);
        let type_label = item
            .mime_type
            .as_deref()
            .map(|m| crate::presentation::truncate_with_ellipsis(m, 24))
            .unwrap_or_else(|| "File".to_string());
        let ago = crate::presentation::relative_time(item.completed_at_ms);

        let (status_label, kind) = match item.local {
            LocalFileState::Verified => ("Verified", crate::ui_components::BadgeKind::Accent),
            LocalFileState::Warning => ("Integrity warning", crate::ui_components::BadgeKind::Danger),
            LocalFileState::Missing => ("File not found", crate::ui_components::BadgeKind::Danger),
            LocalFileState::Unknown => ("Unknown", crate::ui_components::BadgeKind::Default),
        };

        let exists = matches!(item.local, LocalFileState::Verified | LocalFileState::Warning);
        let openable = matches!(item.local, LocalFileState::Verified);

        let id_num = item
            .id
            .as_str()
            .strip_prefix("download:")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(-1);

        let open_btn = button(
            crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Open"),
        )
        .on_press(AppMessage::DownloadedOpen(id_num))
        .padding([SPACE_4, SPACE_8])
        .style(BUTTON_GHOST_BG);
        let reveal_btn = button(
            crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Reveal"),
        )
        .on_press(AppMessage::DownloadedReveal(id_num))
        .padding([SPACE_4, SPACE_8])
        .style(BUTTON_GHOST_BG);
        let remove_btn = button(
            Row::new()
                .push(
                    Icon::Delete
                        .build()
                        .size(crate::icon_system::IconSize::Xs)
                        .color_fn(|_| iced::Color::WHITE)
                        .build(),
                )
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Remove"),
                )
                .spacing(SPACE_4)
                .align_y(Alignment::Center),
        )
        .on_press(AppMessage::DownloadedRemoveHistory(id_num))
        .padding([SPACE_4, SPACE_8])
        .style(BUTTON_DANGER);

        let actions = Row::new()
            .push(open_btn)
            .push(Space::new().width(Length::Fixed(SPACE_4)))
            .push(reveal_btn)
            .push(Space::new().width(Length::Fixed(SPACE_4)))
            .push(remove_btn)
            .spacing(0)
            .align_y(Alignment::Center);

        let status_badge = crate::ui_components::badge(status_label, kind);

        let metadata_label = format!("{type_label} · {size_label}");
        // PAPIRUS-11: the Downloaded row's identity cell leads with the same
        // central FileTypeIcon component/resolver as chat cards. The recorded
        // MIME hint (and the filename extension) select the Papirus icon; the
        // local integrity state answers "what is happening to the file" as a
        // separate badge, never by recolouring the type icon.
        // The row already prints the MIME type in its metadata line, so the
        // icon is decorative (PAPIRUS-15): hidden from assistive technology.
        let type_icon = crate::download_progress_view::decorative_file_type_icon_element(
            &item.display_name,
            item.mime_type.as_deref(),
            None,
            crate::file_type_icon::FileTypeIconSize::List,
            theme,
        );
        // Build the identity cell inline with owned strings: `FileIdentityCell`
        // borrows `&str` that must outlive the returned element, which a
        // stack-local formatted label cannot satisfy.
        let name_cell = Row::new()
            .push(type_icon)
            .push(Space::new().width(Length::Fixed(SPACE_12)))
            .push(
                Column::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Body,
                            item.display_name.clone(),
                        )
                        .color(crate::design_tokens::text_primary(theme)),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            metadata_label,
                        )
                        .color(crate::design_tokens::text_secondary(theme)),
                    )
                    .spacing(SPACE_2)
                    .width(Length::Fill),
            )
            .spacing(0)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        let name_col = if item.local == LocalFileState::Missing {
            Column::new()
                .push(name_cell)
                .push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        "The file was moved or deleted. You can remove this history entry.",
                    )
                    .color(crate::design_tokens::color_danger(theme))
                    .width(Length::Fill)
                    .wrapping(iced::widget::text::Wrapping::None),
                )
                .spacing(SPACE_2)
                .width(Length::Fill)
        } else {
            Column::new().push(name_cell).spacing(0).width(Length::Fill)
        };

        let mut row = Row::new()
            .push(name_col.width(Length::FillPortion(5)))
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, size_label)
                    .color(crate::design_tokens::text_secondary(theme))
                    .width(Length::Fixed(72.0)),
            )
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    item.source_peer.clone(),
                )
                .color(crate::design_tokens::text_secondary(theme))
                .width(Length::Fixed(120.0))
                // FONTS-15: wrap long friend display names inside the fixed
                // Source column instead of letting them spill into the
                // Completed column (a 25+ char name is ~150 px at 12 px IBM
                // Plex Sans, wider than the 120 px column).
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, ago)
                    .color(crate::design_tokens::text_secondary(theme))
                    .width(Length::Fixed(120.0)),
            )
            .push(status_badge)
            .push(Space::new().width(Length::Fill))
            .push(actions)
            .spacing(SPACE_12)
            .align_y(Alignment::Center)
            .padding([SPACE_8, SPACE_4])
            .width(Length::Fill);

        // Ensure the missing-file state never offers Open/Reveal.
        if !exists {
            row = row.push(Space::new().width(Length::Fixed(0.0)));
        }
        let _ = openable;

        container(row)
            .width(Length::Fill)
            .style(move |t| container::Style {
                background: None,
                border: Border {
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// Download Manager screen (DLMGR-02): every active transfer in both
    /// directions.
    ///
    /// - **Downloads** — live inbound transfers from the FS-05 projection
    ///   with name, source peer, byte progress, truthful state, and
    ///   Pause / Resume / Cancel controls.
    /// - **Uploads** — live outbound transfers (peers downloading from us)
    ///   with name, peer, progress, truthful state, and a Stop control.
    ///
    /// Rows reuse the same projection view models as the File Sharing
    /// dashboard (downloading_view_model / peers_downloading_view_model) and
    /// the shared per-entry control widgets from download_progress_view, so
    /// no transfer semantics are duplicated. The header shows a live count
    /// of active downloads and uploads — the same active-transfer totals the
    /// sharing summary card reports.
    pub(crate) fn view_download_manager(&self) -> iced::Element<'_, AppMessage> {
        use crate::downloading_view_model::{incoming_row, sort_incoming_rows};
        use crate::peers_downloading_view_model::{outbound_row, sort_outbound_rows};
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(self.dark_mode);

        // ── Header: back button + title + live counts ──────────────────
        let back_btn = button(crate::fonts::type_role_text(
            crate::fonts::TypeRole::ButtonLabel,
            "←",
        ))
        .on_press(AppMessage::CloseDownloadManager)
        .padding([SPACE_4, SPACE_6])
        .style(BUTTON_ICON);

        let title_col = Column::new()
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::PageTitle,
                    "Download Manager",
                )
                .color(crate::design_tokens::text_primary(&theme)),
            )
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "All active downloads and uploads, with pause / stop controls.",
                )
                .style(text_muted_style),
            )
            .spacing(SPACE_4);

        let header = Row::new()
            .push(back_btn)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(title_col)
            .push(Space::new().width(Length::Fill))
            .align_y(Alignment::Center)
            .padding([SPACE_6, SPACE_10])
            .width(Length::Fill);

        // ── Downloads section ───────────────────────────────────────────
        let inbound_labels = self
            .inbound_item_labels
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let mut inbound_rows: Vec<crate::downloading_view_model::IncomingTransferRow> = self
            .inbound_active
            .values()
            .map(|record| incoming_row(record, &inbound_labels))
            .collect();
        sort_incoming_rows(&mut inbound_rows);
        let download_count = inbound_rows.len();

        let mut downloads_col = Column::new().spacing(SPACE_4);
        for row in inbound_rows {
            downloads_col = downloads_col.push(self.download_manager_incoming_row(&row, &theme));
        }
        let downloads_body: iced::Element<'_, AppMessage> = if download_count == 0 {
            crate::ui_components::empty_state(
                crate::icon_system::Icon::Download,
                "No active downloads.",
                "Files you are receiving will appear here with live progress.",
                None,
                None,
            )
            .into()
        } else {
            downloads_col.into()
        };

        let downloads_header = Row::new()
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::CardTitle,
                "Downloads",
            ))
            .push(crate::ui_components::badge_owned(
                download_count.to_string(),
                crate::ui_components::BadgeKind::Count,
            ))
            .push(Space::new().width(Length::Fill))
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        let downloads_card = dashboard_card(
            Column::new()
                .push(downloads_header)
                .push(Space::new().height(Length::Fixed(SPACE_12)))
                .push(downloads_body)
                .spacing(0)
                .width(Length::Fill)
                .into(),
        );

        // ── Uploads section ─────────────────────────────────────────────
        let outbound_labels = self
            .outbound_item_labels
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let mut outbound_rows: Vec<crate::peers_downloading_view_model::PeersDownloadingRow> = self
            .outbound_active
            .values()
            .map(|record| outbound_row(record, &outbound_labels))
            .collect();
        sort_outbound_rows(&mut outbound_rows);
        let upload_count = outbound_rows.len();

        let mut uploads_col = Column::new().spacing(SPACE_4);
        for row in outbound_rows {
            uploads_col = uploads_col.push(self.download_manager_outbound_row(&row, &theme));
        }
        let uploads_body: iced::Element<'_, AppMessage> = if upload_count == 0 {
            crate::ui_components::empty_state(
                crate::icon_system::Icon::Upload,
                "No active uploads.",
                "Files peers are downloading from you will appear here.",
                None,
                None,
            )
            .into()
        } else {
            uploads_col.into()
        };

        let uploads_header = Row::new()
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::CardTitle,
                "Uploads",
            ))
            .push(crate::ui_components::badge_owned(
                upload_count.to_string(),
                crate::ui_components::BadgeKind::Count,
            ))
            .push(Space::new().width(Length::Fill))
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        let uploads_card = dashboard_card(
            Column::new()
                .push(uploads_header)
                .push(Space::new().height(Length::Fixed(SPACE_12)))
                .push(uploads_body)
                .spacing(0)
                .width(Length::Fill)
                .into(),
        );

        // ── Assemble the full screen ────────────────────────────────────
        let body: iced::Element<'_, AppMessage> = Column::new()
            .push(header)
            .push(container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(move |t| container::Style {
                    background: Some(iced::Background::Color(
                        crate::design_tokens::border_muted(t),
                    )),
                    ..Default::default()
                }))
            .push(Space::new().height(Length::Fixed(SPACE_16)))
            .push(downloads_card)
            .push(Space::new().height(Length::Fixed(SPACE_16)))
            .push(uploads_card)
            .spacing(0)
            .width(Length::Fill)
            .into();

        crate::ui_components::gutter_scrollable(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// One inbound transfer row for the Download Manager.
    pub(crate) fn download_manager_incoming_row<'a>(
        &'a self,
        row: &crate::downloading_view_model::IncomingTransferRow,
        theme: &iced::Theme,
    ) -> iced::Element<'a, AppMessage> {
        use crate::downloading_view_model::{
            format_progress, format_started, IncomingProgress, IncomingState,
        };
        use crate::ui_components::ProgressBar;
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Border, Length};

        let peer_display = row
            .peer_id
            .as_deref()
            .and_then(|id| id.parse::<PublicKey>().ok())
            .map(|pk| self.resolve_name(&pk))
            .unwrap_or_else(|| "Unknown peer".to_string());

        let (state_label, state_color) = match row.state {
            IncomingState::Transferring => ("Transferring", crate::design_tokens::primary(theme)),
            IncomingState::Retrying => ("Retrying", crate::design_tokens::color_warning(theme)),
            IncomingState::Verifying => ("Verifying", crate::design_tokens::color_warning(theme)),
            IncomingState::Completed => ("Completed", crate::design_tokens::color_success(theme)),
            IncomingState::Failed => ("Failed", crate::design_tokens::color_danger(theme)),
            IncomingState::Cancelled => ("Cancelled", crate::design_tokens::text_muted(theme)),
            IncomingState::Disconnected => ("Disconnected", crate::design_tokens::color_danger(theme)),
        };

        let (bar, progress_text) = match &row.progress {
            IncomingProgress::Determinate { bytes, total } if *total > 0 => {
                let pct = ((*bytes as f64 / *total as f64) * 100.0).min(100.0) as u8;
                (
                    ProgressBar::<AppMessage>::new(pct as f32 / 100.0)
                        .show_label(false)
                        .bold()
                        .build(theme),
                    format!("{}% · {}", pct, format_progress(&row.progress)),
                )
            }
            _ => (
                ProgressBar::<AppMessage>::new(0.0)
                    .indeterminate(true)
                    .bold()
                    .build(theme),
                format_progress(&row.progress),
            ),
        };

        let type_icon = crate::download_progress_view::file_type_icon_element(
            &row.display_name,
            None,
            None,
            crate::file_type_icon::FileTypeIconSize::List,
            theme,
        );

        let name_line = Row::new()
            .push(type_icon)
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::BodyEmphasised,
                    row.display_name.clone(),
                )
                .color(crate::design_tokens::text_primary(theme))
                .width(Length::Shrink)
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(SPACE_4)
            .align_y(Alignment::Center);

        let progress_col = Column::new()
            .push(bar)
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, progress_text)
                    .color(crate::design_tokens::text_muted(theme))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(SPACE_2)
            .width(Length::Fill);

        // Controls: Pause/Resume + Cancel for live rows; nothing for
        // terminal/stopped rows.
        let mut controls = Row::new().spacing(SPACE_4).align_y(Alignment::Center);
        if !row.state.is_terminal() && !matches!(row.state, IncomingState::Disconnected) {
            if self.paused_inbound_transfer_ids.contains(&row.id) {
                controls = controls.push(
                    crate::download_progress_view::primary_button(
                        None,
                        "Resume",
                        AppMessage::DownloadingResume(row.id.clone()),
                    ),
                );
            } else {
                controls = controls.push(
                    crate::download_progress_view::secondary_button(
                        None,
                        "Pause",
                        AppMessage::DownloadingPause(row.id.clone()),
                    ),
                );
            }
            controls = controls.push(
                crate::download_progress_view::text_button(
                    "Cancel",
                    AppMessage::DownloadingCancel(row.id.clone()),
                ),
            );
        } else if matches!(row.state, IncomingState::Disconnected) {
            controls = controls.push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Peer disconnected")
                    .color(crate::design_tokens::text_muted(theme)),
            );
        }

        let row_el = Row::new()
            .push(name_line)
            .push(Space::new().width(Length::Fixed(SPACE_12)))
            .push(progress_col)
            .push(Space::new().width(Length::Fixed(SPACE_12)))
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    crate::presentation::truncate_with_ellipsis(&peer_display, 24),
                )
                .color(crate::design_tokens::text_secondary(theme))
                .width(Length::Fixed(140.0))
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    format_started(row.started_at_ms, now_ms() as u64),
                )
                .color(crate::design_tokens::text_secondary(theme))
                .width(Length::Fixed(100.0))
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, state_label)
                    .color(state_color)
                    .width(Length::Fixed(100.0))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(controls)
            .spacing(SPACE_12)
            .align_y(Alignment::Center)
            .padding([SPACE_8, SPACE_4])
            .width(Length::Fill);

        container(row_el)
            .width(Length::Fill)
            .style(move |t| container::Style {
                background: None,
                border: Border {
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// One outbound transfer row for the Download Manager.
    pub(crate) fn download_manager_outbound_row<'a>(
        &'a self,
        row: &crate::peers_downloading_view_model::PeersDownloadingRow,
        theme: &iced::Theme,
    ) -> iced::Element<'a, AppMessage> {
        use crate::peers_downloading_view_model::{
            format_progress, OutboundProgress, OutboundState,
        };
        use crate::ui_components::ProgressBar;
        use iced::widget::{container, Column, Row, Space};
        use iced::{Alignment, Border, Length};

        let peer_display = row
            .peer_id
            .as_deref()
            .and_then(|id| id.parse::<PublicKey>().ok())
            .map(|pk| self.resolve_name(&pk))
            .unwrap_or_else(|| "Unknown peer".to_string());

        let (state_label, state_color) = match row.state {
            OutboundState::Transferring => ("Transferring", crate::design_tokens::primary(theme)),
            OutboundState::Retrying => ("Retrying", crate::design_tokens::color_warning(theme)),
            OutboundState::Verifying => ("Verifying", crate::design_tokens::color_warning(theme)),
            OutboundState::Completed => ("Completed", crate::design_tokens::color_success(theme)),
            OutboundState::Failed => ("Failed", crate::design_tokens::color_danger(theme)),
            OutboundState::Cancelled => ("Cancelled", crate::design_tokens::text_muted(theme)),
            OutboundState::Disconnected => ("Disconnected", crate::design_tokens::color_danger(theme)),
        };

        let (bar, progress_text) = match &row.progress {
            OutboundProgress::Determinate { bytes, total } if *total > 0 => {
                let pct = ((*bytes as f64 / *total as f64) * 100.0).min(100.0) as u8;
                (
                    ProgressBar::<AppMessage>::new(pct as f32 / 100.0)
                        .show_label(false)
                        .bold()
                        .build(theme),
                    format!("{}% · {}", pct, format_progress(&row.progress)),
                )
            }
            _ => (
                ProgressBar::<AppMessage>::new(0.0)
                    .indeterminate(true)
                    .bold()
                    .build(theme),
                format_progress(&row.progress),
            ),
        };

        let type_icon = crate::download_progress_view::file_type_icon_element(
            &row.display_name,
            None,
            None,
            crate::file_type_icon::FileTypeIconSize::List,
            theme,
        );

        let name_line = Row::new()
            .push(type_icon)
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::BodyEmphasised,
                    row.display_name.clone(),
                )
                .color(crate::design_tokens::text_primary(theme))
                .width(Length::Shrink)
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(SPACE_4)
            .align_y(Alignment::Center);

        let progress_col = Column::new()
            .push(bar)
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, progress_text)
                    .color(crate::design_tokens::text_muted(theme))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(SPACE_2)
            .width(Length::Fill);

        // Stop control for live outbound rows (uploads).
        let mut controls = Row::new().spacing(SPACE_4).align_y(Alignment::Center);
        if !row.state.is_terminal() && !matches!(row.state, OutboundState::Disconnected) {
            controls = controls.push(
                crate::download_progress_view::text_button(
                    "Stop",
                    AppMessage::DownloadingStop(row.id.clone()),
                ),
            );
        }

        let row_el = Row::new()
            .push(name_line)
            .push(Space::new().width(Length::Fixed(SPACE_12)))
            .push(progress_col)
            .push(Space::new().width(Length::Fixed(SPACE_12)))
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    crate::presentation::truncate_with_ellipsis(&peer_display, 24),
                )
                .color(crate::design_tokens::text_secondary(theme))
                .width(Length::Fixed(140.0))
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, state_label)
                    .color(state_color)
                    .width(Length::Fixed(100.0))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(controls)
            .spacing(SPACE_12)
            .align_y(Alignment::Center)
            .padding([SPACE_8, SPACE_4])
            .width(Length::Fill);

        container(row_el)
            .width(Length::Fill)
            .style(move |t| container::Style {
                background: None,
                border: Border {
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// Render the Downloading tab: live incoming transfers from the FS-05
    /// projection with name, source peer, byte progress, truthful state,
    /// started time, and (when it can be computed from real observations)
    /// speed/ETA. Restrained actions: Cancel only — pause/resume are not
    /// offered because the projection has no paused state and the backend
    /// cannot honour them for the live inbound path.
    pub(crate) fn view_downloading(&self) -> iced::Element<'_, AppMessage> {
        use crate::downloading_view_model::{incoming_row, sort_incoming_rows};
        use iced::widget::{Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(self.dark_mode);

        let labels = self
            .inbound_item_labels
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();

        let mut rows: Vec<crate::downloading_view_model::IncomingTransferRow> = self
            .inbound_active
            .values()
            .map(|record| incoming_row(record, &labels))
            .collect();
        sort_incoming_rows(&mut rows);

        // FS-18: the global header query filters the live Downloading tab by
        // file display name, peer display label, and short peer id. Filtering
        // happens on the projected clones only — the authoritative inbound
        // store and its live progress updates are untouched, so active
        // transfers keep updating while filtered.
        if !self.dashboard_search_input.trim().is_empty() {
            rows.retain(|row| {
                let peer_label = row
                    .peer_id
                    .as_deref()
                    .and_then(|id| id.parse::<PublicKey>().ok())
                    .map(|pk| self.resolve_name(&pk))
                    .unwrap_or_default();
                crate::dashboard_filters::query_matches(
                    &self.dashboard_search_input,
                    &[
                        row.display_name.as_str(),
                        peer_label.as_str(),
                        row.peer_id.as_deref().unwrap_or(""),
                    ],
                )
            });
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Empty state — no active inbound transfers.
        if rows.is_empty() {
            return crate::ui_components::empty_state(
                Icon::Files,
                "No active downloads.",
                "Files you are receiving will appear here with live progress.",
                None,
                None,
            )
            .into();
        }

        // Header row with count.
        let count_label = rows.len().to_string();
        let header_row = Row::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Downloading"),
            )
            .push(crate::ui_components::badge_owned(
                count_label,
                crate::ui_components::BadgeKind::Count,
            ))
            .push(Space::new().width(Length::Fill))
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        // Column headers (Name | Progress | Source | Started | Status | Actions).
        let header = crate::ui_components::TableHeaderRow::new(vec![
            ("Name", None),
            ("Progress", Some(180.0)),
            ("Source", Some(140.0)),
            ("Started", Some(120.0)),
            ("Status", Some(110.0)),
            ("Actions", Some(90.0)),
        ])
        .build(&theme);

        let mut rows_col = Column::new().spacing(SPACE_4);
        for row in rows {
            rows_col = rows_col.push(self.incoming_download_row(row, now_ms, &theme));
        }

        let body = Column::new()
            .push(header_row)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(header)
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(rows_col)
            .spacing(0)
            .width(Length::Fill);

        crate::ui_components::gutter_scrollable(dashboard_card(body.into()))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// One row of the Downloading tab. Consumes the row so the returned
    /// element owns its labels (the caller's row vector does not outlive the
    /// view).
    pub(crate) fn incoming_download_row<'a>(
        &'a self,
        row: crate::downloading_view_model::IncomingTransferRow,
        now_ms: u64,
        theme: &iced::Theme,
    ) -> iced::Element<'a, AppMessage> {
        use crate::downloading_view_model::{
            format_eta, format_progress, format_speed, format_started, IncomingProgress,
            IncomingState,
        };
        use crate::ui_components::ProgressBar;
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Border, Length};

        // Source peer is resolved from the authenticated peer id — never from
        // a display string carried in the projection.
        let peer_display = row
            .peer_id
            .as_deref()
            .and_then(|id| id.parse::<PublicKey>().ok())
            .map(|pk| self.resolve_name(&pk))
            .unwrap_or_else(|| "Unknown peer".to_string());

        let (state_label, state_color) = match row.state {
            IncomingState::Transferring => ("Transferring", crate::design_tokens::primary(theme)),
            IncomingState::Retrying => ("Retrying", crate::design_tokens::color_warning(theme)),
            IncomingState::Verifying => ("Verifying", crate::design_tokens::color_warning(theme)),
            IncomingState::Completed => ("Completed", crate::design_tokens::color_success(theme)),
            IncomingState::Failed => ("Failed", crate::design_tokens::color_danger(theme)),
            IncomingState::Cancelled => ("Cancelled", crate::design_tokens::text_muted(theme)),
            IncomingState::Disconnected => ("Disconnected", crate::design_tokens::color_danger(theme)),
        };

        // Speed/ETA only when they can be computed from real observations.
        // Previous sample is only used for speed; ETA is derived from the
        // current row's determinate progress and the computed speed.
        let speed_line = match row.speed_bps(None) {
            Some(speed) => {
                let mut line = format_speed(speed);
                if let Some(eta) = row.eta_secs(speed) {
                    line.push_str(&format!(" · {}", format_eta(eta)));
                }
                line
            }
            None => String::new(),
        };

        let (bar, progress_text) = match &row.progress {
            IncomingProgress::Determinate { bytes, total } if *total > 0 => {
                let pct = ((*bytes as f64 / *total as f64) * 100.0).min(100.0) as u8;
                (
                    ProgressBar::<AppMessage>::new(pct as f32 / 100.0)
                        .show_label(false)
                        .bold()
                        .build(theme),
                    format!("{}% · {}", pct, format_progress(&row.progress)),
                )
            }
            IncomingProgress::Determinate { .. } => (
                ProgressBar::<AppMessage>::new(0.0)
                    .indeterminate(true)
                    .bold()
                    .build(theme),
                format_progress(&row.progress),
            ),
            IncomingProgress::Indeterminate { .. } => (
                ProgressBar::<AppMessage>::new(0.0)
                    .indeterminate(true)
                    .bold()
                    .build(theme),
                format_progress(&row.progress),
            ),
            IncomingProgress::Unknown => (
                ProgressBar::<AppMessage>::new(0.0)
                    .show_label(false)
                    .bold()
                    .build(theme),
                "Size unknown".to_string(),
            ),
        };

        // PAPIRUS-11: the Downloading row leads with the same central
        // FileTypeIcon component/resolver as chat cards — the icon answers
        // "what type of file is this?", the state label + progress answer
        // "what is happening to it".
        let type_icon = crate::download_progress_view::file_type_icon_element(
            &row.display_name,
            None,
            None,
            crate::file_type_icon::FileTypeIconSize::List,
            theme,
        );

        let mut name_line = Row::new()
            .push(type_icon)
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::BodyEmphasised, row.display_name)
                    .color(crate::design_tokens::text_primary(theme))
                    .width(Length::Shrink)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(SPACE_4)
            .align_y(Alignment::Center);
        if !speed_line.is_empty() {
            name_line = name_line
                .push(Space::new().width(Length::Fixed(SPACE_8)))
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, speed_line)
                        .color(crate::design_tokens::text_muted(theme))
                        .wrapping(iced::widget::text::Wrapping::None),
                );
        }

        let name_col = Column::new()
            .push(name_line)
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    format_progress(&row.progress),
                )
                .color(crate::design_tokens::text_muted(theme))
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(SPACE_2)
            .width(Length::Shrink);

        let progress_col = Column::new()
            .push(bar)
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, progress_text)
                    .color(crate::design_tokens::text_muted(theme))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .spacing(SPACE_2)
            .width(Length::Fill);

        let started_label = format_started(row.started_at_ms, now_ms);

        // Only show Cancel while the transfer is still live. Completed,
        // failed, and cancelled rows move to the Downloaded/history views;
        // unsupported controls are never shown.
        let cancel_btn: Option<iced::Element<'a, AppMessage>> =
            if row.state.is_terminal() || matches!(row.state, IncomingState::Disconnected) {
                None
            } else {
                Some(
                    button(
                        crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Cancel"),
                    )
                    .on_press(AppMessage::DownloadingCancel(row.id.clone()))
                    .padding([SPACE_4, SPACE_8])
                    .style(BUTTON_GHOST_BG)
                    .into(),
                )
            };

        let mut row_el = Row::new()
            .push(name_col)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(progress_col)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    crate::presentation::truncate_with_ellipsis(&peer_display, 24),
                )
                .color(crate::design_tokens::text_secondary(theme))
                .width(Length::Fixed(140.0))
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, started_label)
                    .color(crate::design_tokens::text_secondary(theme))
                    .width(Length::Fixed(120.0))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, state_label)
                    .color(state_color)
                    .width(Length::Fixed(110.0))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(match cancel_btn {
                Some(btn) => btn,
                None => Space::new().width(Length::Fixed(0.0)).into(),
            })
            .spacing(SPACE_12)
            .align_y(Alignment::Center)
            .padding([SPACE_8, SPACE_4])
            .width(Length::Fill);

        if let Some(error) = &row.error {
            row_el = row_el.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    crate::presentation::truncate_with_ellipsis(error, 48),
                )
                .color(crate::design_tokens::color_danger(theme))
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None),
            );
        }

        container(row_el)
            .width(Length::Fill)
            .style(move |t| container::Style {
                background: None,
                border: Border {
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// Full Activity Log tab (FS-17): filter chips, searchable table,
    /// pagination, raw-error details affordance, and a confirmed, local-only
    /// Clear History action. Rows come from the durable transfer-activity
    /// projection; direction/outcome filters and search are applied by the
    /// view model over the in-memory buffer, so interactions never refetch.
    pub(crate) fn view_activity_log(&self) -> iced::Element<'_, AppMessage> {
        use crate::activity_log_view_model::{filter_activity_log, paginate_activity_log};
        use crate::ui_components::{badge_owned, BadgeKind, TableHeaderRow};
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Background, Border, Length};

        let theme = Self::theme_from_dark(self.dark_mode);

        // Loading skeleton on first open.
        if !self.activity_log_loaded && self.activity_log_error.is_none() {
            return crate::ui_components::gutter_scrollable(
                Column::new()
                    .push(dashboard_card(
                        crate::ui_components::LoadingSkeleton::new(5)
                            .row_height(crate::design_tokens::TABLE_ROW_HEIGHT)
                            .build(&theme),
                    ))
                    .spacing(SPACE_16)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        }

        // Inline error with retry.
        if let Some(error) = &self.activity_log_error {
            let retry = crate::ui_components::InlineError::new(error)
                .on_retry(AppMessage::ActivityLogRefresh)
                .build(&theme);
            return crate::ui_components::gutter_scrollable(
                Column::new()
                    .push(dashboard_card(retry))
                    .spacing(SPACE_16)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        }

        // Header row: title + count badge, Clear History ghost action.
        let count_label = self.activity_log_rows.len().to_string();
        let header_row = Row::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Activity Log"),
            )
            .push(badge_owned(
                count_label,
                BadgeKind::Count,
            ))
            .push(Space::new().width(Length::Fill));

        let clear_btn = button(
            crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Clear History"),
        )
        .on_press(AppMessage::ActivityLogClearRequested)
        .padding([SPACE_4, SPACE_10])
        .style(BUTTON_GHOST_BG);
        let header_row = header_row.push(clear_btn).spacing(SPACE_8).align_y(Alignment::Center);

        // Clear-history confirmation banner (local-only, projection-only).
        let mut confirm_banner: Option<iced::Element<'_, AppMessage>> = None;
        if self.activity_log_clear_confirm {
            let confirm = container(
                Row::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::BodyEmphasised,
                            "Clear the local activity history?",
                        )
                        .color(crate::design_tokens::text_primary(&theme)),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button(
                            crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Cancel"),
                        )
                        .on_press(AppMessage::ActivityLogClearCancelled)
                        .padding([SPACE_4, SPACE_10])
                        .style(BUTTON_GHOST_BG),
                    )
                    .push(
                        button(
                            crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Clear History"),
                        )
                        .on_press(AppMessage::ActivityLogClearConfirmed)
                        .padding([SPACE_4, SPACE_10])
                        .style(BUTTON_DANGER),
                    )
                    .spacing(SPACE_8)
                    .align_y(Alignment::Center),
            )
            .padding([SPACE_10, SPACE_16])
            .width(Length::Fill)
            .style(move |t| container::Style {
                background: Some(Background::Color(crate::design_tokens::color_danger(t).scale_alpha(0.08))),
                border: Border {
                    color: crate::design_tokens::color_danger(t).scale_alpha(0.35),
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    width: 1.0,
                },
                ..Default::default()
            })
            .into();
            confirm_banner = Some(confirm);
        }

        // Filter chips (single-choice segmented control).
        let active_filter = self.activity_log_filter;
        let mut chips = Row::new().spacing(SPACE_6);
        for filter in crate::activity_log_view_model::ActivityLogFilter::ALL.iter() {
            let is_active = *filter == active_filter;
            let chip = button(
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, filter.label()),
            )
            .on_press(AppMessage::ActivityLogFilterSelected(*filter))
            .padding([SPACE_4, SPACE_10])
                .style(move |t, status| {
                    let hovered = matches!(status, iced::widget::button::Status::Hovered);
                    if is_active {
                        button::Style {
                            background: Some(Background::Color(crate::design_tokens::primary(t))),
                            text_color: iced::Color::WHITE,
                            border: Border {
                                radius: SPACE_12.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    } else {
                        button::Style {
                            background: Some(Background::Color(if hovered {
                                crate::design_tokens::surface_hover(t)
                            } else {
                                crate::design_tokens::surface(t)
                            })),
                            text_color: crate::design_tokens::text_secondary(t),
                            border: Border {
                                radius: SPACE_12.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }
                });
            chips = chips.push(chip);
        }

        // Empty history (retention-aware — never implies sharing is broken).
        if self.activity_log_rows.is_empty() {
            let empty = Column::new()
                .push(header_row)
                .push(Space::new().height(Length::Fixed(SPACE_12)))
                .push(dashboard_card(
                    crate::ui_components::empty_state(
                        Icon::Activity,
                        "No activity yet.",
                        "Sharing requests, downloads, and uploads appear here while kept by the local activity retention window.",
                        None,
                        None,
                    )
                    .into(),
                ))
                .spacing(0)
                .width(Length::Fill);
            return crate::ui_components::gutter_scrollable(empty).width(Length::Fill).height(Length::Fill).into();
        }

        // Apply the shared search field (file or peer matching) on top of the
        // active filter, then the FS-18 activity sort (time/status), then
        // paginate. Sorting a filtered clone keeps the authoritative buffer
        // untouched and deterministic across renders.
        let mut filtered = filter_activity_log(
            &self.activity_log_rows,
            active_filter,
            &self.dashboard_search_input,
        );
        self.dashboard_activity_sort.apply(&mut filtered);

        if filtered.is_empty() {
            let empty = Column::new()
                .push(header_row)
                .push(Space::new().height(Length::Fixed(SPACE_12)))
                .push(dashboard_card(
                    crate::ui_components::empty_state(
                        Icon::Search,
                        "No matching activity.",
                        "Try a different filter or search term.",
                        None,
                        None,
                    )
                    .into(),
                ))
                .spacing(0)
                .width(Length::Fill);
            return crate::ui_components::gutter_scrollable(empty).width(Length::Fill).height(Length::Fill).into();
        }

        // FS-18: sort control row (Activity: time / status).
        let sort = self.dashboard_activity_sort;
        let mut sort_row = Row::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Sort:")
                    .style(text_muted_style),
            )
            .spacing(SPACE_6)
            .align_y(Alignment::Center);
        for key in crate::dashboard_filters::ActivitySortKey::ALL.iter() {
            sort_row = sort_row.push(dashboard_sort_chip(
                &theme,
                key.label(),
                sort.key == *key,
                sort.descending,
                AppMessage::DashboardActivitySortClicked(*key),
            ));
        }

        let page = paginate_activity_log(
            filtered,
            self.activity_log_page,
            crate::activity_log_view_model::ACTIVITY_LOG_PAGE_SIZE,
        );

        // Column headers (Direction | Event | Item | Peer | Time | Outcome | Details).
        let table_header = TableHeaderRow::new(vec![
            ("Direction", Some(90.0)),
            ("Event", Some(110.0)),
            ("Item", None),
            ("Peer", Some(140.0)),
            ("Time", Some(110.0)),
            ("Outcome", Some(100.0)),
            ("Details", Some(80.0)),
        ])
        .build(&theme);

        let mut rows = Column::new().spacing(SPACE_4);
        for row in &page.rows {
            rows = rows.push(self.activity_log_row(row, &theme));
        }

        // Pagination footer: "Page X of Y · N events" + Prev/Next.
        let page_label = format!(
            "Page {} of {} · {} event{}",
            page.page + 1,
            page.pages,
            page.total,
            if page.total == 1 { "" } else { "s" },
        );
        let prev_btn = button(
            crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Previous"),
        )
        .on_press_maybe(page.has_previous().then_some(AppMessage::ActivityLogPageSelected(
            page.page.saturating_sub(1),
        )))
        .padding([SPACE_4, SPACE_10])
        .style(BUTTON_GHOST_BG);
        let next_btn = button(
            crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Next"),
        )
        .on_press_maybe(page.has_next().then_some(AppMessage::ActivityLogPageSelected(
            page.page + 1,
        )))
        .padding([SPACE_4, SPACE_10])
        .style(BUTTON_GHOST_BG);
        let footer = Row::new()
            .push(prev_btn)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(next_btn)
            .push(Space::new().width(Length::Fill))
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, page_label)
                    .style(text_muted_style),
            )
            .spacing(0)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        let mut body = Column::new()
            .push(header_row)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(chips)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(sort_row)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(table_header)
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(rows)
            .push(Space::new().height(Length::Fixed(SPACE_12)))
            .push(footer)
            .spacing(0)
            .width(Length::Fill);

        if let Some(banner) = confirm_banner {
            body = Column::new()
                .push(banner)
                .push(Space::new().height(Length::Fixed(SPACE_12)))
                .push(body)
                .spacing(0)
                .width(Length::Fill);
        }

        crate::ui_components::gutter_scrollable(dashboard_card(body.into())).width(Length::Fill).height(Length::Fill).into()
    }

    /// One row of the Activity Log table. Error rows expose a bounded
    /// raw-detail affordance; the table itself only shows safe summaries.
    pub(crate) fn activity_log_row(
        &self,
        row: &crate::activity_log_view_model::ActivityLogRow,
        theme: &iced::Theme,
    ) -> iced::Element<'_, AppMessage> {
        use crate::activity_log_view_model::ActivityDirection as Dir;
        use crate::activity_log_view_model::ActivityOutcome as Outcome;
        use crate::ui_components::{badge, BadgeKind};
        use iced::widget::{button, container, Column, Row, Space};
        use iced::{Alignment, Background, Border, Length};

        let ago = crate::presentation::relative_time(row.occurred_at_ms);
        let size_label = row
            .bytes
            .map(crate::dashboard_view_model::format_bytes)
            .unwrap_or_default();

        let (direction_label, direction_color) = match row.direction {
            Dir::Inbound => ("To me", crate::design_tokens::primary(theme)),
            Dir::Outbound => ("By others", crate::design_tokens::color_success(theme)),
            Dir::Unknown => ("Unknown", crate::design_tokens::text_muted(theme)),
        };

        let (outcome_label, kind) = match row.outcome {
            Outcome::Success => ("Completed", BadgeKind::Accent),
            Outcome::Error => ("Error", BadgeKind::Danger),
            Outcome::Warning => ("Attention", BadgeKind::Default),
            Outcome::Info => ("Info", BadgeKind::Default),
        };

        let item_label = match size_label.as_str() {
            "" => crate::presentation::truncate_with_ellipsis(&row.file_label, 48),
            size => format!(
                "{} · {size}",
                crate::presentation::truncate_with_ellipsis(&row.file_label, 40)
            ),
        };

        // Raw error details affordance: only for rows that carry bounded
        // failure context; toggled inline under the row.
        let mut details_cell: iced::Element<'_, AppMessage> = Space::new()
            .width(Length::Fixed(80.0))
            .into();
        let mut detail_panel: Option<iced::Element<'_, AppMessage>> = None;
        if let Some(raw) = row.raw_detail.as_deref() {
            let is_open = self.activity_log_details_open.as_deref() == Some(row.id.as_str());
            let details_btn = button(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    if is_open { "Hide" } else { "Details" },
                ),
            )
            .on_press(AppMessage::ActivityLogDetailsToggled(row.id.clone()))
            .padding([SPACE_2, SPACE_6])
            .style(BUTTON_GHOST_BG);
            details_cell = details_btn.width(Length::Fixed(80.0)).into();
            if is_open {
                let raw_owned = raw.to_string();
                let panel = container(
                    Row::new()
                        .push(
                            Icon::AlertTriangle
                                .build()
                                .size(crate::icon_system::IconSize::Xs)
                                .color_fn(crate::design_tokens::color_danger)
                                .build(),
                        )
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                raw_owned,
                            )
                            .color(crate::design_tokens::text_secondary(theme))
                            .wrapping(iced::widget::text::Wrapping::None),
                        )
                        .spacing(SPACE_6)
                        .align_y(Alignment::Center),
                )
                .padding([SPACE_6, SPACE_10])
                .width(Length::Fill)
                .style(move |t| container::Style {
                    background: Some(Background::Color(
                        crate::design_tokens::color_danger(t).scale_alpha(0.06),
                    )),
                    border: Border {
                        color: crate::design_tokens::color_danger(t).scale_alpha(0.25),
                        radius: crate::design_tokens::RADIUS_MD.into(),
                        width: 1.0,
                    },
                    ..Default::default()
                })
                .into();
                detail_panel = Some(panel);
            }
        }

        let event_label = format!(
            "{} · attempt {}",
            row.action,
            row.attempt
        );

        let main_row = Row::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, direction_label)
                    .color(direction_color)
                    .width(Length::Fixed(90.0))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    crate::presentation::truncate_with_ellipsis(&event_label, 24),
                )
                    .color(crate::design_tokens::text_primary(theme))
                    .width(Length::Fixed(110.0))
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(
                Row::new()
                    .push(crate::download_progress_view::file_type_icon_element(
                        &row.file_label,
                        None,
                        None,
                        crate::file_type_icon::FileTypeIconSize::Compact,
                        theme,
                    ))
                    .push(Space::new().width(Length::Fixed(SPACE_4)))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            item_label,
                        )
                        .color(crate::design_tokens::text_primary(theme))
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::None),
                    )
                    .spacing(0)
                    .align_y(Alignment::Center)
                    .width(Length::FillPortion(5)),
            )
            .push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    crate::presentation::truncate_with_ellipsis(&row.peer_label, 24),
                )
                .color(crate::design_tokens::text_secondary(theme))
                .width(Length::Fixed(140.0))
                .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, ago)
                    .color(crate::design_tokens::text_secondary(theme))
                    .width(Length::Fixed(110.0)),
            )
            .push(badge(outcome_label, kind))
            .push(Space::new().width(Length::Fixed(4.0)))
            .push(details_cell)
            .spacing(SPACE_10)
            .align_y(Alignment::Center)
            .padding([SPACE_8, SPACE_4])
            .width(Length::Fill);

        let mut body = Column::new().push(main_row).spacing(0).width(Length::Fill);
        if let Some(panel) = detail_panel {
            body = body.push(
                container(panel)
                    .padding([0.0, SPACE_8])
                    .width(Length::Fill),
            );
        }

        container(body)
            .width(Length::Fill)
            .style(move |t| container::Style {
                background: None,
                border: Border {
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// File Sharing screen.
    /// PERF-2: snapshot selector for the "Files I'm Sharing" table card.
    pub(crate) fn shared_by_me_card_dependency(&self) -> SharedByMeCardDependency {
        let load_state = if let Some(message) = &self.shared_by_me_error {
            crate::shared_by_me_table::SharedByMeLoadState::Error(message.clone())
        } else if self.shared_by_me_loading {
            crate::shared_by_me_table::SharedByMeLoadState::Loading
        } else {
            crate::shared_by_me_table::SharedByMeLoadState::Ready
        };
        SharedByMeCardDependency {
            dark_mode: self.dark_mode,
            search_query: self.dashboard_search_input.clone(),
            items_count: self.dashboard_shared_by_me_filter.len(),
            rows: self.dashboard_shared_by_me_filter.clone(),
            ui: self.shared_by_me_ui.clone(),
            load_state,
            sort: self.dashboard_shared_by_me_sort,
            thumbnails: SharedByMeThumbnails(self.shared_by_me_thumbnails.clone()),
        }
    }

    /// PERF-2: static renderer for the "Files I'm Sharing" table card, run
    /// inside `iced::widget::lazy` so it is only re-invoked when the query,
    /// rows, interactive state, load state, sort, or thumbnails change.
    pub(crate) fn view_shared_by_me_card(
        dep: &SharedByMeCardDependency,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{Column, Row, Space};
        use iced::{Alignment, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);

        // FS-18: sort control row (Shared by Me: name / date shared / size /
        // downloads). Real buttons → keyboard accessible.
        let sort = dep.sort;
        let mut sort_row = Row::new()
            .push(
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Sort:")
                    .style(text_muted_style),
            )
            .spacing(SPACE_6)
            .align_y(Alignment::Center);
        for key in crate::dashboard_filters::SharedByMeSortKey::ALL.iter() {
            sort_row = sort_row.push(dashboard_sort_chip(
                &theme,
                key.label(),
                sort.key == *key,
                sort.descending,
                AppMessage::DashboardSharedByMeSortClicked(*key),
            ));
        }

        let file_table_card: iced::Element<'static, AppMessage> =
            if !dep.search_query.trim().is_empty() && dep.rows.is_empty() {
                // The query filtered everything out — a search-specific empty
                // state is more truthful than the card's "haven't shared any
                // files yet" copy.
                crate::ui_components::empty_state(
                    Icon::Search,
                    "No matching files.",
                    "Try a different search term.",
                    None,
                    None,
                )
                .into()
            } else {
                crate::shared_by_me_table::view_shared_by_me_card(
                    &dep.rows,
                    &dep.ui,
                    dep.load_state.clone(),
                    theme,
                    dep.dark_mode,
                    &dep.thumbnails.0,
                )
                .into()
            };

        Column::new()
            .push(sort_row)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(file_table_card)
            .spacing(0)
            .width(Length::Fill)
            .into()
    }

    /// PERF-2: snapshot selector for the "Peers Downloading from Me" card.
    /// Projects the live FS-05 outbound records (already enriched with item
    /// labels by the provider consumer) into UI rows and resolves the peer
    /// display label and online state so the static renderer can draw them
    /// without touching application state.
    pub(crate) fn peers_card_dependency(&self) -> PeersCardDependency {
        use crate::dashboard_view_model::{outbound_row, sort_outbound_rows};
        let labels = self
            .outbound_item_labels
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let mut rows: Vec<crate::dashboard_view_model::PeerDownload> = self
            .outbound_active
            .values()
            .map(|record| outbound_row(record, &labels))
            .collect();
        sort_outbound_rows(&mut rows);
        // Resolve the authenticated peer id to a verified display identity
        // and presence-derived online flag so the static renderer can draw
        // rows without touching application state.
        for row in &mut rows {
            if let Ok(pk) = row.peer_label.parse::<PublicKey>() {
                row.peer_display = self.resolve_name(&pk);
                row.online = matches!(self.peer_presence(&pk), PeerPresence::Online);
            }
        }
        PeersCardDependency {
            dark_mode: self.dark_mode,
            rows,
        }
    }

    /// PERF-2: static renderer for the "Peers Downloading from Me" card.
    /// The dependency carries live outbound rows with resolved peer display
    /// labels; the lazy subtree is rebuilt only when the rows or theme change.
    pub(crate) fn view_peers_card(dep: &PeersCardDependency) -> iced::Element<'static, AppMessage> {
        use crate::card_shell::CardShell;
        use crate::dashboard_view_model::format_bytes;
        use crate::ui_components::ProgressBar;
        use iced::widget::{container, Column, Row, Space};
        use iced::{Alignment, Border, Length};

        let theme = Self::theme_from_dark(dep.dark_mode);

        let children: Vec<iced::Element<'static, AppMessage>> = dep
            .rows
            .iter()
            .map(|row| {
                let (state_label, state_color) = match row.state {
                    crate::dashboard_view_model::OutboundState::Transferring => {
                        ("Transferring", crate::design_tokens::primary(&theme))
                    }
                    crate::dashboard_view_model::OutboundState::Retrying => {
                        ("Retrying", crate::design_tokens::color_warning(&theme))
                    }
                    crate::dashboard_view_model::OutboundState::Verifying => {
                        ("Verifying", crate::design_tokens::color_warning(&theme))
                    }
                    crate::dashboard_view_model::OutboundState::Completed => {
                        ("Completed", crate::design_tokens::color_success(&theme))
                    }
                    crate::dashboard_view_model::OutboundState::Failed => {
                        ("Failed", crate::design_tokens::color_danger(&theme))
                    }
                    crate::dashboard_view_model::OutboundState::Cancelled => {
                        ("Cancelled", crate::design_tokens::text_muted(&theme))
                    }
                    crate::dashboard_view_model::OutboundState::Disconnected => {
                        ("Disconnected", crate::design_tokens::color_danger(&theme))
                    }
                };

                let (bar, progress_text) = match &row.progress {
                    crate::dashboard_view_model::Progress::Determinate { bytes, total }
                        if *total > 0 =>
                    {
                        let pct = ((*bytes as f64 / *total as f64) * 100.0).min(100.0) as u8;
                        (
                            ProgressBar::<AppMessage>::new(pct as f32 / 100.0)
                                .show_label(false)
                                .bold()
                                .build(&theme),
                            format!("{pct}%"),
                        )
                    }
                    crate::dashboard_view_model::Progress::Determinate { bytes, .. }
                    | crate::dashboard_view_model::Progress::Indeterminate { bytes } => (
                        ProgressBar::<AppMessage>::new(0.0)
                            .indeterminate(true)
                            .bold()
                            .build(&theme),
                        format!("{} received", format_bytes(*bytes)),
                    ),
                    crate::dashboard_view_model::Progress::Unknown => (
                        ProgressBar::<AppMessage>::new(0.0)
                            .show_label(false)
                            .bold()
                            .build(&theme),
                        "—".to_string(),
                    ),
                };

                let avatar: iced::Element<'static, AppMessage> =
                    Avatar::<AppMessage>::new(&row.peer_display)
                        .size(28.0)
                        .online_dot(row.online)
                        .dark_mode(dep.dark_mode)
                        .build();

                let name_line = Row::new()
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::BodyEmphasised,
                            row.peer_display.clone(),
                        )
                        .color(crate::design_tokens::text_primary(&theme))
                        .width(Length::Shrink)
                        .wrapping(iced::widget::text::Wrapping::None),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            state_label,
                        )
                        .color(state_color)
                        .width(Length::Shrink),
                    )
                    .align_y(Alignment::Center);

                let type_icon = crate::download_progress_view::file_type_icon_element(
                    &row.display_name,
                    None,
                    None,
                    crate::file_type_icon::FileTypeIconSize::Compact,
                    &theme,
                );

                let file_line = Row::new()
                    .push(type_icon)
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            row.display_name.clone(),
                        )
                        .style(text_muted_style)
                        .width(Length::Fill)
                        .wrapping(iced::widget::text::Wrapping::None),
                    )
                    .spacing(SPACE_4)
                    .align_y(Alignment::Center);

                let progress_line = Row::new()
                    .push(bar)
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            progress_text,
                        )
                        .style(text_muted_style)
                        .width(Length::Shrink),
                    )
                    .spacing(SPACE_4)
                    .align_y(Alignment::Center);

                let text_col = Column::new()
                    .push(name_line)
                    .push(file_line)
                    .push(progress_line)
                    .spacing(SPACE_2)
                    .width(Length::Fill);

                let row_el = Row::new()
                    .push(avatar)
                    .push(Space::new().width(Length::Fixed(SPACE_8)))
                    .push(text_col)
                    .spacing(0)
                    .align_y(Alignment::Center)
                    .width(Length::Fill);

                container(row_el)
                    .width(Length::Fill)
                    .padding([SPACE_6, SPACE_4])
                    .style(move |_t| container::Style {
                        background: None,
                        border: Border {
                            radius: crate::design_tokens::RADIUS_MD.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into()
            })
            .collect();

        CardShell::new("Peers Downloading from Me", children)
            .count(dep.rows.len())
            .on_view_all(AppMessage::DashboardTabSelected(
                crate::dashboard_view_model::DashboardTab::Downloading,
            ))
            .empty_message("No one is downloading from you right now.")
            .max_height(240.0)
            .build(&theme)
    }

    pub(crate) fn view_file_sharing(&self) -> iced::Element<'_, AppMessage> {
        use crate::dashboard_view_model::DashboardTab as Tab;

        use iced::Length;

        // Owned-tab fast path: these tabs render their own full content area
        // (no dashboard header/tab bar), so they stay on the live instance
        // views. PERF-4R-B: the pre-warm cache only holds a FileSharing entry
        // when the active tab is the default Files tab; switching to an owned
        // tab changes the dep hash → cache miss → live path.
        if matches!(
            self.dashboard_active_tab,
            Tab::Downloaded | Tab::ActivityLog | Tab::Downloading | Tab::SharedWithMe
        ) {
            return match self.dashboard_active_tab {
                Tab::Downloaded => crate::ui_components::gutter_scrollable(self.view_downloaded())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                Tab::ActivityLog => self.view_activity_log(),
                Tab::Downloading => crate::ui_components::gutter_scrollable(self.view_downloading())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                Tab::SharedWithMe => crate::ui_components::gutter_scrollable(self.view_shared_with_me())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                Tab::SharedByMe => unreachable!("guarded by the matches! above"),
            };
        }

        // Default Files tab: route through the dependency so the pre-warm
        // cache (PERF-4R-B) can serve a fully materialized tree from `view()`
        // directly. The lazy wrapper keeps today's within-session caching
        // identical; the pre-warm cache bypasses it by serving the stored
        // element from `view()`.
        let dep = self.file_sharing_dependency();
        iced::widget::lazy(dep, Self::view_file_sharing_content).into()
    }

    /// Builds the File Sharing default Files tab's renderable snapshot.
    /// Everything the shell + header/search/tab bar + card grid renders is
    /// captured here, so the tree can be materialized by the pre-warm cache
    /// (PERF-4R-B) during idle and served from `view()` without rebuilding.
    pub(crate) fn file_sharing_dependency(&self) -> FileSharingDependency {
        FileSharingDependency {
            dark_mode: self.dark_mode,
            responsive_mode: FileSharingResponsiveMode::from_width(self.window_width),
            dashboard_search_input: self.dashboard_search_input.clone(),
            dashboard_active_tab: self.dashboard_active_tab,
            dashboard_connectivity_dismissed: self.dashboard_connectivity_dismissed,
            mesh_health: MeshHealthSnapshot::from(&self.mesh_health),
            shared_by_me: self.shared_by_me_card_dependency(),
            peers: self.peers_card_dependency(),
            sharing_summary: self.sharing_summary_card_dependency(),
            recent_activity: self.recent_activity_card_dependency(),
        }
    }

    /// Static renderer for the File Sharing default Files tab, driven by
    /// [`FileSharingDependency`]. CRITICAL: the four cards are built by
    /// calling their static content functions DIRECTLY — never wrapped in
    /// `iced::widget::lazy` — so a pre-warmed tree is fully materialized.
    pub(crate) fn view_file_sharing_content(
        dep: &FileSharingDependency,
    ) -> iced::Element<'static, AppMessage> {
        use crate::dashboard_view_model::DashboardTab as Tab;
        use iced::widget::{button, container, scrollable, text_input, Column, Row, Space};
        use iced::{Alignment, Background, Border, Length};

        // ── FS-21: Responsive breakpoints ──────────────────────────────
        let is_compact = dep.responsive_mode.is_compact();
        let is_medium = dep.responsive_mode.is_medium();

        // Search width adapts: 320 px wide, 240 px medium, Fill compact.
        let search_width: Length = if is_compact {
            Length::Fill
        } else if is_medium {
            Length::Fixed(240.0)
        } else {
            Length::Fixed(320.0)
        };

        let theme = Self::theme_from_dark(dep.dark_mode);

        // ── Header region: title + subtitle (left), search + action (right) ──
        let page_title = Row::new()
            .push(
                Column::new()
                    .push(
                        crate::fonts::type_role_text(crate::fonts::TypeRole::PageTitle, "File Sharing")
                            .color(crate::design_tokens::text_primary(&theme)),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Manage your shared files, downloads, and transfer activity.",
                        )
                        .style(text_muted_style),
                    )
                    .spacing(SPACE_4),
            )
            .width(Length::Fill);

        let search_input = text_input("Search files or peers...", &dep.dashboard_search_input)
            .on_input(|s| AppMessage::DashboardSearchChanged(s))
            .padding([SPACE_6, SPACE_12])
            .size(crate::fonts::TypeRole::Body.size_px())
            .font(crate::fonts::TypeRole::Body.font())
            .width(search_width);

        let search_icon = Icon::Search
            .build()
            .size(crate::icon_system::IconSize::Xs)
            .color_fn(crate::design_tokens::text_muted)
            .build();

        // FS-18: one-action clear for the global query. Keyboard-accessible:
        // it is a real button (Tab focusable) and Escape in the field does the
        // same thing (see Shortcut(Escape) handling). Only rendered while the
        // field has text, so it never crowds the header otherwise.
        let clear_search_button: iced::Element<'static, AppMessage> = if dep
            .dashboard_search_input
            .is_empty()
        {
            let placeholder: iced::Element<'static, AppMessage> = Space::new().into();
            placeholder
        } else {
            button(
                Icon::Close
                    .build()
                    .size(crate::icon_system::IconSize::Xs)
                    .color_fn(crate::design_tokens::text_muted)
                    .build(),
            )
            .on_press(AppMessage::DashboardSearchCleared)
            .padding([SPACE_4, SPACE_6])
            .style(move |t, status| {
                let hovered = matches!(status, iced::widget::button::Status::Hovered);
                button::Style {
                    background: if hovered {
                        Some(Background::Color(crate::design_tokens::surface_hover(t)))
                    } else {
                        None
                    },
                    text_color: if hovered {
                        crate::design_tokens::text_primary(t)
                    } else {
                        crate::design_tokens::text_muted(t)
                    },
                    border: Border {
                        radius: crate::design_tokens::RADIUS_SM.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .into()
        };

        let search_row = Row::new()
            .push(search_icon)
            .push(search_input)
            .push(clear_search_button)
            .spacing(SPACE_4)
            .align_y(Alignment::Center);

        let open_downloads_btn = button(
            Row::new()
                .push(
                    Icon::Files
                        .build()
                        .size(crate::icon_system::IconSize::Xs)
                        .color_fn(|_| iced::Color::WHITE)
                        .build(),
                )
                .push(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Open Downloads Folder"),
                )
                .spacing(SPACE_4)
                .align_y(Alignment::Center),
        )
        .on_press(AppMessage::OpenDownloadsFolder)
        .padding([SPACE_6, SPACE_16])
        .style(BUTTON_PRIMARY_GREEN);

        // SENDME-02: receive a file shared outside the friend graph via a
        // BlobTicket (copy a ticket string → paste here → pre-flight → download).
        let receive_ticket_btn = button(
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
                        "Receive from Ticket",
                    ),
                )
                .spacing(SPACE_4)
                .align_y(Alignment::Center),
        )
        .on_press(AppMessage::OpenReceiveTicketDialog)
        .padding([SPACE_6, SPACE_16])
        .style(BUTTON_OUTLINE);

        // FS-26: receive a file shared outside the friend graph via a short
        // code (type the 7-character code the sharing peer shows, instead of
        // pasting a long ticket).
        let receive_short_code_btn = button(
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
                        "Receive Short Code",
                    ),
                )
                .spacing(SPACE_4)
                .align_y(Alignment::Center),
        )
        .on_press(AppMessage::OpenRedeemCodeDialog)
        .padding([SPACE_6, SPACE_16])
        .style(BUTTON_OUTLINE);

        // DLMGR-01: entry point for the Download Manager screen — every
        // active transfer in both directions with pause/stop controls.
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
        .padding([SPACE_6, SPACE_16])
        .style(BUTTON_OUTLINE);

        let header_actions = Row::new()
            .push(search_row)
            .push(Space::new().width(Length::Fixed(SPACE_16)))
            .push(receive_ticket_btn)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(receive_short_code_btn)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(download_manager_btn)
            .push(Space::new().width(Length::Fixed(SPACE_8)))
            .push(open_downloads_btn)
            .align_y(Alignment::Center);

        let header = Row::new()
            .push(page_title)
            .push(header_actions)
            .align_y(Alignment::Center)
            .spacing(SPACE_16);

        // ── Tab bar ──
        let active_tab = dep.dashboard_active_tab;
        // Build all tab widgets first, then construct the row from the full
        // children list (avoids the incremental `.push()` chain allocating a
        // fresh Row per tab — PERF-3).
        let mut tab_widgets: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        for tab in Tab::ALL.iter() {
            let is_active = *tab == active_tab;
            let tab_label = tab.label();
            let tab_msg = AppMessage::DashboardTabSelected(*tab);

            let tab_btn = button(
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, tab_label),
            )
            .on_press(tab_msg)
            .padding([SPACE_4, SPACE_2])
                .style(move |t, status| {
                    let color = if is_active {
                        crate::design_tokens::text_primary(t)
                    } else if matches!(status, iced::widget::button::Status::Hovered) {
                        crate::design_tokens::primary(t)
                    } else {
                        crate::design_tokens::text_secondary(t)
                    };
                    button::Style {
                        background: None,
                        text_color: color,
                        border: Border::default(),
                        ..Default::default()
                    }
                });

            let underline = container(Space::new().width(Length::Shrink).height(Length::Fixed(2.0)))
                .width(Length::Shrink)
                .height(Length::Fixed(2.0))
                .style(move |t| container::Style {
                    background: if is_active {
                        Some(Background::Color(crate::design_tokens::primary(t)))
                    } else {
                        None
                    },
                    ..Default::default()
                });

            let tab_widget = Column::new()
                .push(tab_btn)
                .push(underline)
                .spacing(0)
                .align_x(Alignment::Center);

            tab_widgets.push(tab_widget.into());
        }

        let tabs_row = Row::with_children(tab_widgets).spacing(SPACE_16);

        let tab_bar_content: iced::Element<'_, AppMessage> = if is_compact {
            scrollable(
                Row::new()
                    .push(tabs_row)
                    .push(Space::new().width(Length::Fixed(SPACE_24)))
                    .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .into()
        } else {
            Row::new()
                .push(tabs_row)
                .push(Space::new().width(Length::Fill))
                .align_y(Alignment::Center)
                .into()
        };

        let tab_bar = container(tab_bar_content)
            .padding([SPACE_8, SPACE_24])
            .width(Length::Fill);

        // Full-width muted separator below tabs.
        let tab_separator = container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .height(Length::Fixed(1.0))
            .style(move |t| container::Style {
                background: Some(Background::Color(crate::design_tokens::border_muted(t))),
                ..Default::default()
            });

        // ── Content grid: left (2/3) + right (1/3) ──
        // FS-21: three-tier responsive:
        //   compact (≤1024): single column, stacked
        //   medium  (1024-1279): two columns, reduced padding
        //   large   (≥1280): full two-column layout

        // Owned-tab branches (Downloading/Downloaded/ActivityLog/SharedWithMe)
        // are handled by the live `view_file_sharing` wrapper; this static
        // renderer only ever runs for the default Files tab.

        // PERF-4R-A: each card is built DIRECTLY from its static content
        // function — no `iced::widget::lazy` inside this renderer — so a
        // pre-warmed tree is fully materialized. The per-card selectors
        // (`*_card_dependency`) still feed those functions via the snapshot.
        let shared_by_me_card = Self::view_shared_by_me_card(&dep.shared_by_me);
        let peers_card = Self::view_peers_card(&dep.peers);
        let sharing_summary_card = Self::view_sharing_summary_card(&dep.sharing_summary);
        let recent_activity_card = Self::view_recent_download_activity_card(&dep.recent_activity);

        // ── FS-19: connectivity notice at the top of the dashboard when the
        // mesh is unhealthy or the user is offline. Dismissible — does not
        // block interaction with unaffected regions.
        let connectivity_notice = dashboard_connectivity_notice(
            dep.dashboard_connectivity_dismissed,
            &dep.mesh_health.as_mesh_health(),
            &theme,
        );

        let content_area: iced::Element<'_, AppMessage> = if !is_compact {
            // Two-column: 2/3 left + 1/3 right.
            let right_column = Column::new()
                .push(peers_card)
                .push(Space::new().height(Length::Fixed(SPACE_20)))
                .push(sharing_summary_card)
                .spacing(0)
                .width(Length::Fill);
            Column::new()
                .push(
                    Row::new()
                        .push(container(shared_by_me_card).width(Length::FillPortion(63)))
                        .push(Space::new().width(Length::Fixed(SPACE_20)))
                        .push(container(right_column).width(Length::FillPortion(34)))
                        .width(Length::Fill),
                )
                .push(Space::new().height(Length::Fixed(SPACE_20)))
                .push(recent_activity_card)
                .spacing(0)
                .width(Length::Fill)
                .into()
        } else {
            // Single column: stack in priority order.
            Column::new()
                .push(shared_by_me_card)
                .push(Space::new().height(Length::Fixed(SPACE_16)))
                .push(sharing_summary_card)
                .push(Space::new().height(Length::Fixed(SPACE_16)))
                .push(peers_card)
                .push(Space::new().height(Length::Fixed(SPACE_16)))
                .push(recent_activity_card)
                .spacing(0)
                .width(Length::Fill)
                .into()
        };

        let scrollable_content = crate::ui_components::gutter_scrollable(content_area)
            .width(Length::Fill)
            .height(Length::Fill);

        // ── Compose full page ──
        let mut page = Column::new()
            .push(header)
            .push(Space::new().height(Length::Fixed(SPACE_20)))
            .push(tab_bar)
            .push(tab_separator);
        if let Some(notice) = connectivity_notice {
            page = page
                .push(Space::new().height(Length::Fixed(SPACE_8)))
                .push(notice);
        }
        page = page
            .push(Space::new().height(Length::Fixed(SPACE_20)))
            .push(scrollable_content)
            .spacing(0)
            .padding([SPACE_24, SPACE_24])
            .width(Length::Fill)
            .height(Length::Fill);

        page.into()
    }

    /// State-layer update for the file-sharing dashboard (BORU-AUDIT-22
    /// spec step 5).
    ///
    /// Handles the file-sharing screen actions: downloads folder, dashboard
    /// search/sort/tab, transfer projection updates, download manager,
    /// shared-by-me, downloaded history, activity log and connectivity
    /// dismissal. The root `update()` dispatches these variants here via
    /// combined match arms.
    pub(crate) fn update_files(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::OpenDownloadsFolder => {
                self.video_card_menu_open = None;
                let dl_dir = self.data_dir.join("downloads");
                let _ = std::fs::create_dir_all(&dl_dir);
                iced::Task::perform(async move { open::that(dl_dir) }, |result| {
                    if let Err(e) = result {
                        AppMessage::ErrorMsg(format!("Could not open downloads folder: {e}"))
                    } else {
                        AppMessage::Noop
                    }
                })
            }
            AppMessage::DashboardSearchChanged(query) => {
                self.dashboard_search_input = query;
                // Close any half-open "Files I'm Sharing" interactions when
                // the user leaves the Shared by Me tab.
                self.shared_by_me_ui.clear();
                // FS-18: keep the Shared by Me filtered projection in sync with
                // the global query immediately (in-memory, no debounce).
                self.refresh_shared_by_me_filter();
                // Refreshing on tab selection keeps the Recent Download
                // Activity card current when the user revisits the dashboard.
                self.refresh_dashboard_activity()
            }
            AppMessage::DashboardSearchCleared => {
                // One-action clear (header × button or Escape). The query is
                // global across tabs, so clearing it restores every tab to its
                // unfiltered rows; authoritative row buffers and summary
                // metrics are untouched.
                self.dashboard_search_input.clear();
                self.shared_by_me_ui.clear();
                self.refresh_shared_by_me_filter();
                iced::Task::none()
            }
            AppMessage::DashboardSharedByMeSortClicked(key) => {
                self.dashboard_shared_by_me_sort = self.dashboard_shared_by_me_sort.on_key_clicked(key);
                self.refresh_shared_by_me_filter();
                iced::Task::none()
            }
            AppMessage::DashboardDownloadedSortClicked(key) => {
                self.dashboard_downloaded_sort = self.dashboard_downloaded_sort.on_key_clicked(key);
                iced::Task::none()
            }
            AppMessage::DashboardActivitySortClicked(key) => {
                self.dashboard_activity_sort = self.dashboard_activity_sort.on_key_clicked(key);
                iced::Task::none()
            }
            AppMessage::TransferProjectionUpdate(update) => {
                self.apply_transfer_update(update.transfer);
                iced::Task::none()
            }
            AppMessage::TransferSnapshotResync => {
                // The broadcast receiver lagged or was restarted: rebuild the
                // panel maps from the projection snapshot so no row is stale
                // or duplicated after event replay.
                let snapshot = self.transfer_store.snapshot();
                self.resync_outbound_panel(&snapshot);
                self.resync_inbound_panel(&snapshot);
                iced::Task::none()
            }
            AppMessage::DownloadingCancel(transfer_id) => {
                self.cancel_inbound_transfer(&transfer_id);
                iced::Task::none()
            }
            AppMessage::DownloadingPause(transfer_id) => {
                self.pause_inbound_transfer(&transfer_id);
                iced::Task::none()
            }
            AppMessage::DownloadingResume(transfer_id) => {
                self.resume_inbound_transfer(&transfer_id);
                iced::Task::none()
            }
            AppMessage::DownloadingStop(transfer_id) => {
                self.stop_outbound_transfer(&transfer_id);
                iced::Task::none()
            }
            AppMessage::OpenDownloadManager => {
                // Navigation only — the shared shell, networking services, and
                // conversation subscriptions stay alive; only the main panel
                // swaps to the Download Manager screen. Remember where we came
                // from so the back button returns to the previous screen.
                if !matches!(self.screen, Screen::DownloadManager) {
                    self.download_manager_return_to = Some(self.screen.clone());
                    self.screen = Screen::DownloadManager;
                }
                iced::Task::none()
            }
            AppMessage::CloseDownloadManager => {
                self.screen = self
                    .download_manager_return_to
                    .take()
                    .unwrap_or(Screen::ChatList);
                iced::Task::none()
            }
            AppMessage::SharedByMeMenuToggle(hash) => {
                self.shared_by_me_ui.toggle_menu(&hash);
                iced::Task::none()
            }
            AppMessage::SharedByMeDetails(hash) => {
                self.shared_by_me_ui.open_details(&hash);
                iced::Task::none()
            }
            AppMessage::SharedByMeCloseDetails => {
                self.shared_by_me_ui.details_open = None;
                iced::Task::none()
            }
            AppMessage::SharedByMeReveal(hash) => {
                // Reveal the source file in the OS file manager. The full
                // local path is used only here — it is never rendered in the
                // table or in error copy.
                let path = self
                    .storage
                    .as_ref()
                    .and_then(|stg| stg.get_file_object(&hash).ok().flatten())
                    .and_then(|object| object.source_path)
                    .map(std::path::PathBuf::from);
                match path {
                    Some(path) => iced::Task::perform(async move { open::that(path) }, |result| {
                        if let Err(e) = result {
                            AppMessage::ErrorMsg(format!("Could not reveal file: {e}"))
                        } else {
                            AppMessage::Noop
                        }
                    }),
                    None => iced::Task::done(AppMessage::ErrorMsg(
                        "The local file is no longer available.".to_string(),
                    )),
                }
            }
            AppMessage::SharedByMeConfirmStopSharing(hash) => {
                // First press opens the inline confirmation; the destructive
                // action is only performed on the second press of the same
                // message once the confirmation row is visible.
                if self.shared_by_me_ui.confirm_stop.as_deref() == Some(hash.as_str()) {
                    self.shared_by_me_ui.clear();
                    self.shared_by_me_loading = true;
                    return iced::Task::done(AppMessage::RemoveSharedFile(hash))
                        .chain(self.refresh_shared_by_me());
                }
                self.shared_by_me_ui.clear();
                self.shared_by_me_ui.confirm_stop = Some(hash);
                iced::Task::none()
            }
            AppMessage::SharedByMeCancelStopSharing => {
                self.shared_by_me_ui.confirm_stop = None;
                iced::Task::none()
            }
            AppMessage::SharedByMeRevokeAccess(hash, grantee) => {
                if let Some(ref stg) = self.storage {
                    let user_id = self.local_public.to_string();
                    match stg.revoke_permission(&hash, &user_id, &grantee, "read") {
                        Ok(true) => {
                            return iced::Task::done(AppMessage::SharedFileRemoved(
                                "Access revoked.".to_string(),
                            ));
                        }
                        Ok(false) => {
                            return iced::Task::done(AppMessage::ErrorMsg(
                                "That recipient no longer has access.".to_string(),
                            ));
                        }
                        Err(e) => {
                            return iced::Task::done(AppMessage::ErrorMsg(format!(
                                "Failed to revoke access: {e}"
                            )));
                        }
                    }
                }
                iced::Task::none()
            }
            AppMessage::SharedByMeLoaded(result) => {
                match result {
                    Ok(rows) => {
                        self.shared_by_me_rows = rows;
                        self.shared_by_me_error = None;
                    }
                    Err(message) => {
                        self.shared_by_me_rows.clear();
                        self.shared_by_me_error = Some(message);
                    }
                }
                self.shared_by_me_loading = false;
                // FS-18: rebuild the filtered/sorted projection from the
                // freshly loaded authoritative rows.
                self.refresh_shared_by_me_filter();
                // UI-30: kick off uniform thumbnail generation for any
                // image/video rows that don't have a handle yet.
                self.kick_shared_by_me_thumbnails()
            }
            AppMessage::SharedByMeThumbnailReady {
                content_hash,
                handle,
            } => {
                self.shared_by_me_thumbnails.insert(content_hash, handle);
                iced::Task::none()
            }
            AppMessage::DashboardRecentActivityLoaded(rows) => {
                self.dashboard_recent_activity = rows;
                iced::Task::none()
            }
            AppMessage::DashboardSharingSummaryLoaded(summary) => {
                // `None` (storage unavailable / load error) keeps the card in
                // its unknown state instead of flashing a fake zero.
                self.dashboard_sharing_summary = summary;
                iced::Task::none()
            }
            AppMessage::DashboardDownloadedRefresh => self.refresh_downloaded_history(),
            AppMessage::DashboardDownloadedLoaded(result) => {
                match result {
                    Ok(rows) => {
                        self.downloaded_history = rows;
                        self.downloaded_history_error = None;
                    }
                    Err(message) => {
                        self.downloaded_history.clear();
                        self.downloaded_history_error = Some(message);
                    }
                }
                self.downloaded_history_loaded = true;
                iced::Task::none()
            }
            AppMessage::DownloadedOpen(id) => self.open_downloaded_item(id),
            AppMessage::DownloadedReveal(id) => self.reveal_downloaded_item(id),
            AppMessage::DownloadedRemoveHistory(id) => {
                if let Some(storage) = self.storage.as_ref() {
                    if let Err(error) = storage.delete_download_history(id) {
                        return iced::Task::done(AppMessage::ErrorMsg(format!(
                            "Could not remove download from history: {error}"
                        )));
                    }
                }
                // Removing history never deletes the local file; refresh the
                // list so the record disappears immediately.
                self.refresh_downloaded_history()
            }
            AppMessage::DashboardTabSelected(tab) => {
                self.dashboard_active_tab = tab;
                // Complete a GUI test action that requested this tab once the
                // dashboard actually shows it.
                if let Some(action_id) = self.pending_dashboard_tab_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                // The Sharing Summary card is only visible on the Shared by Me
                // tab; refresh it there so a freshly completed download or a
                // newly granted share is reflected without a manual reload.
                let mut tasks = Vec::new();
                if tab == crate::dashboard_view_model::DashboardTab::SharedByMe {
                    tasks.push(self.refresh_sharing_summary());
                }
                // Load the Downloaded tab's durable history the first time it
                // is opened (and on every revisit, so newly completed files
                // appear without a manual refresh).
                if tab == crate::dashboard_view_model::DashboardTab::Downloaded {
                    tasks.push(self.refresh_downloaded_history());
                }
                // Load the Activity Log projection whenever the tab is opened
                // so freshly recorded lifecycle events appear immediately.
                if tab == crate::dashboard_view_model::DashboardTab::ActivityLog {
                    tasks.push(self.refresh_activity_log());
                }
                if tasks.is_empty() {
                    iced::Task::none()
                } else {
                    iced::Task::batch(tasks)
                }
            }
            AppMessage::ActivityLogLoaded(rows) => {
                self.activity_log_rows = rows;
                self.activity_log_error = None;
                self.activity_log_loaded = true;
                iced::Task::none()
            }
            AppMessage::ActivityLogRefresh => self.refresh_activity_log(),
            AppMessage::ActivityLogFilterSelected(filter) => {
                self.activity_log_filter = filter;
                // A different filter can change the visible set dramatically;
                // land on the first page so the new result is immediately
                // visible (deterministic, never a stale empty page).
                self.activity_log_page = 0;
                self.activity_log_details_open = None;
                iced::Task::none()
            }
            AppMessage::ActivityLogPageSelected(page) => {
                self.activity_log_page = page;
                iced::Task::none()
            }
            AppMessage::ActivityLogDetailsToggled(event_id) => {
                self.activity_log_details_open = if self
                    .activity_log_details_open
                    .as_deref()
                    == Some(event_id.as_str())
                {
                    None
                } else {
                    Some(event_id)
                };
                iced::Task::none()
            }
            AppMessage::ActivityLogClearRequested => {
                self.activity_log_clear_confirm = true;
                iced::Task::none()
            }
            AppMessage::ActivityLogClearCancelled => {
                self.activity_log_clear_confirm = false;
                iced::Task::none()
            }
            AppMessage::ActivityLogClearConfirmed => {
                self.activity_log_clear_confirm = false;
                if let Some(storage) = self.storage.as_ref() {
                    if let Err(error) = storage.clear_transfer_activity() {
                        return iced::Task::done(AppMessage::ErrorMsg(format!(
                            "Could not clear activity history: {error}"
                        )));
                    }
                }
                // Clear History is projection-only: shared files, downloads,
                // and permissions are untouched by design.
                self.refresh_activity_log()
            }
            AppMessage::DashboardConnectivityDismissed => {
                self.dashboard_connectivity_dismissed = true;
                iced::Task::none()
            }
            AppMessage::DashboardDownloadingRefresh => {
                // The Downloading tab is backed by live subscriptions — a
                // refresh triggers a re-read of the current projection state.
                iced::Task::none()
            }
            // update() only dispatches the files variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
