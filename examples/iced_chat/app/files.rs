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
                        crate::i18n::t("common.resume"),
                        AppMessage::DownloadingResume(row.id.clone()),
                    ),
                );
            } else {
                controls = controls.push(
                    crate::download_progress_view::secondary_button(
                        None,
                        crate::i18n::t("common.pause"),
                        AppMessage::DownloadingPause(row.id.clone()),
                    ),
                );
            }
            controls = controls.push(
                crate::download_progress_view::text_button(
                    crate::i18n::t("common.cancel"),
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
                    crate::i18n::t("common.stop"),
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

        let search_input = text_input(&crate::i18n::t("files.search_placeholder"), &dep.dashboard_search_input)
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
            AppMessage::ExecuteFileSend(encoded) => {
                let parts: Vec<&str> = encoded.splitn(3, '|').collect();
                if parts.len() < 3 {
                    tracing::warn!(
                        "ExecuteFileSend: invalid encoded payload ({} parts)",
                        parts.len()
                    );
                    return iced::Task::none();
                }
                let filename = parts[0].to_string();
                let abs_path = parts[1].to_string();
                // Show spinner immediately while the file is uploading.
                let abs_path_buf = std::path::PathBuf::from(&abs_path);
                let file_size = std::fs::metadata(&abs_path_buf)
                    .map(|m| m.len())
                    .unwrap_or(0);
                self.pending_file_upload = Some((filename.clone(), file_size));
                self.file_upload_spinner_frame = 0;

                let is_video = ChatEntry::is_video_file(&filename);
                let transfer_kind = if is_video {
                    TransferKind::Video
                } else {
                    TransferKind::File
                };

                // Create a download card immediately showing upload progress,
                // rather than waiting for the upload to finish.
                let local_label = self.local_label.clone();
                self.download_entry_index = Some(self.entries.len());
                self.entries_push(ChatEntry::system_download(
                    String::new(),
                    transfer_kind,
                    filename.clone(),
                    String::new(),
                    &local_label,
                    None,
                ));
                if let Some(idx) = self.download_entry_index {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(dl) = entry.download.as_mut() {
                            dl.state = DownloadState::Active {
                                bytes: 0,
                                total: Some(file_size),
                            };
                        }
                        entry.body = format!("Uploading: {filename}");
                    }
                }
                // A fresh transfer id binds the upload-progress events to
                // the card created above (Progress → Completed).
                let transfer_id = TransferId::next();
                // Bind the transfer id to the card NOW (before the async task
                // runs) so progress events route deterministically to it. No
                // `Started` event is emitted: the card is already Active, and
                // a queued `Started` drained after `FileDownloaded` resolves
                // the card would flip the terminal `Shared` state back to
                // Active (the Started arm has no terminal guard, unlike
                // Progress/Completed).
                if let Some(idx) = self.download_entry_index {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(dl) = entry.download.as_mut() {
                            dl.transfer_id = Some(transfer_id);
                        }
                    }
                    self.transfer_id_to_index.insert(transfer_id, idx);
                }

                let blob_store = self.blob_store.clone();
                let storage = self.storage.clone();
                let sender = self.sender.clone();
                let secret_key = self.secret_key.clone();
                let endpoint_addr = self.endpoint.addr();
                let poster_cache_dir = self.data_dir.join("cache").join("video-posters");
                let progress_queue = self.download_progress_queue.clone();
                let transfer_name = filename.clone();
                // Cap large file uploads with a generous timeout so a stuck
                // connection doesn't leave the spinner frozen forever.
                let upload_timeout = std::time::Duration::from_secs(3600);
                iced::Task::perform(
                    async move {
                        let result = tokio::time::timeout(upload_timeout, async move {
                            // The upload card already carries this transfer
                            // id (bound at creation), so the Progress and
                            // Completed events below route to it directly.

                            // Fast path: a file previously shared from this
                            // exact source path may already be in the blob
                            // store — skip re-ingesting it entirely.
                            let known_blob = match storage.as_ref() {
                                Some(stg) => {
                                    let hash = stg
                                        .file_object_hash_by_source_path(&abs_path)
                                        .ok()
                                        .flatten()
                                        .and_then(|hash_hex| {
                                            hash_hex.parse::<iroh_blobs::Hash>().ok()
                                        });
                                    match hash {
                                        Some(hash)
                                            if blob_store
                                                .blobs()
                                                .has(hash)
                                                .await
                                                .ok()
                                                .unwrap_or(false) =>
                                        {
                                            Some((hash, iroh_blobs::BlobFormat::Raw))
                                        }
                                        _ => None,
                                    }
                                }
                                None => None,
                            };

                            let (blob_hash, format) = match known_blob {
                                Some(known) => known,
                                None => {
                                    let path_buf = std::path::PathBuf::from(&abs_path);
                                    let metadata = tokio::fs::metadata(&path_buf)
                                        .await
                                        .map_err(|e| format!("Failed to inspect file: {e}"))?;
                                    let _file_size = metadata.len();
                                    // Stream the file into iroh blobs — no
                                    // whole-file memory limit needed.
                                    let file = tokio::fs::File::open(&path_buf)
                                        .await
                                        .map_err(|e| format!("Failed to open file: {e}"))?;
                                    let stream = tokio_util::io::ReaderStream::new(file);
                                    // Walk the import stream so CopyProgress
                                    // events can drive the upload bar.
                                    let import = blob_store
                                        .blobs()
                                        .add_stream(Box::pin(stream))
                                        .await;
                                    let mut add = import.stream().await;
                                    let mut total: Option<u64> = None;
                                    let mut temp_tag: Option<iroh_blobs::api::TempTag> = None;
                                    while let Some(item) = add.next().await {
                                        match item {
                                            iroh_blobs::api::blobs::AddProgressItem::Size(s) => {
                                                total = Some(s);
                                            }
                                            iroh_blobs::api::blobs::AddProgressItem::CopyProgress(
                                                offset,
                                            ) => {
                                                let mut q = progress_queue.lock().unwrap();
                                                q.push_back(TransferProgress::Progress {
                                                    id: transfer_id,
                                                    kind: transfer_kind,
                                                    name: transfer_name.clone(),
                                                    bytes: offset,
                                                    total,
                                                });
                                            }
                                            iroh_blobs::api::blobs::AddProgressItem::OutboardProgress(
                                                _,
                                            )
                                            | iroh_blobs::api::blobs::AddProgressItem::CopyDone => {}
                                            iroh_blobs::api::blobs::AddProgressItem::Done(tt) => {
                                                temp_tag = Some(tt);
                                            }
                                            iroh_blobs::api::blobs::AddProgressItem::Error(e) => {
                                                return Err(format!(
                                                    "Failed to store file: {e}"
                                                ));
                                            }
                                        }
                                    }
                                    let tt = temp_tag.ok_or_else(|| {
                                        "Failed to store file: import ended without a tag"
                                            .to_string()
                                    })?;
                                    (tt.hash(), tt.format())
                                }
                            };
                            let ticket_str = blob_ticket_string(endpoint_addr, blob_hash, format);

                            // ── Announce immediately. The video poster is
                            // generated afterwards and re-announced as a
                            // follow-up, so the share never waits on it. ──
                            let msg = crate::Message::FileShare {
                                name: filename.clone(),
                                ticket: ticket_str.clone(),
                                size: file_size,
                                thumbnail_hash: None,
                                collection_hash: None,
                                collection_entries: 0,
                            };
                            let encoded_msg = SignedMessage::sign_and_encode(&secret_key, &msg)
                                .map_err(|e| format!("Failed to sign: {e}"))?;
                            let _encoded_len = encoded_msg.len();
                            if let Some(ref sender) = sender {
                                match sender.broadcast(encoded_msg).await {
                                    Ok(()) => tracing::info!(
                                        name = %filename,
                                        file_size,
                                        encoded_len = _encoded_len,
                                        "FileShare broadcast OK (poster deferred)"
                                    ),
                                    Err(e) => tracing::error!(
                                        name = %filename,
                                        file_size,
                                        encoded_len = _encoded_len,
                                        error = %e,
                                        "FileShare broadcast FAILED"
                                    ),
                                }
                            }

                            // ── Video poster (off the broadcast critical
                            // path). The cache key is the video's content
                            // hash — known from the ingest — so no second
                            // full-file read is needed. ──
                            let thumbnail_bytes = if is_video {
                                let poster_path = abs_path.clone();
                                let cache_dir = poster_cache_dir.clone();
                                let content_hash = blob_hash;
                                tokio::task::spawn_blocking(move || {
                                    video_poster::generate_with_content_hash(
                                        std::path::Path::new(&poster_path),
                                        &cache_dir,
                                        &content_hash,
                                    )
                                    .ok()
                                    .map(|poster| poster.bytes)
                                })
                                .await
                                .ok()
                                .flatten()
                            } else {
                                None
                            };
                            // Store the poster as a blob so receivers can
                            // fetch it via iroh — keeps gossip messages
                            // small — and re-announce the same ticket with
                            // the hash so their pending card upgrades to
                            // the poster.
                            let thumbnail_hash = match thumbnail_bytes.as_ref() {
                                Some(bytes) => blob_store
                                    .blobs()
                                    .add_bytes(bytes.clone())
                                    .await
                                    .ok()
                                    .map(|tag| MessageHash::from(*tag.hash.as_bytes())),
                                None => None,
                            };
                            if let Some(thumb) = thumbnail_hash {
                                let msg2 = crate::Message::FileShare {
                                    name: filename.clone(),
                                    ticket: ticket_str.clone(),
                                    size: file_size,
                                    thumbnail_hash: Some(thumb),
                                    collection_hash: None,
                                    collection_entries: 0,
                                };
                                if let Ok(encoded2) =
                                    SignedMessage::sign_and_encode(&secret_key, &msg2)
                                {
                                    if let Some(ref sender) = sender {
                                        match sender.broadcast(encoded2).await {
                                            Ok(()) => tracing::info!(
                                                name = %filename,
                                                "FileShare poster follow-up OK"
                                            ),
                                            Err(e) => tracing::error!(
                                                name = %filename,
                                                error = %e,
                                                "FileShare poster follow-up FAILED"
                                            ),
                                        }
                                    }
                                }
                            }

                            // ── Sender-side bookkeeping (off the critical
                            // path): remember path → blob so a re-share of
                            // the same file skips re-ingesting. ──
                            if let Some(stg) = storage.as_ref() {
                                let hash_hex = blob_hash.to_hex().to_string();
                                // SQLite write — defer to the blocking pool
                                // (BORU-AUDIT-18).
                                let stg2 = stg.clone();
                                let hash_hex_cl = hash_hex.clone();
                                let filename_cl = filename.clone();
                                let abs_path_cl = abs_path.clone();
                                if let Err(e) = stg2
                                    .run_blocking("app.record_local_file_object", move |s| {
                                        s.record_local_file_object(
                                            &hash_hex_cl,
                                            file_size,
                                            "application/octet-stream",
                                            &filename_cl,
                                            &abs_path_cl,
                                            &hash_hex_cl,
                                        )
                                    })
                                    .await
                                {
                                    tracing::warn!(
                                        name = %filename,
                                        error = %e,
                                        "record_local_file_object failed after broadcast"
                                    );
                                }
                            }

                            // ── Progress: upload complete ──
                            {
                                let mut q = progress_queue.lock().unwrap();
                                q.push_back(TransferProgress::Completed {
                                    id: transfer_id,
                                    kind: transfer_kind,
                                    name: transfer_name,
                                });
                            }

                            Ok::<_, String>((filename, ticket_str, thumbnail_bytes, abs_path))
                        })
                        .await;
                        match result {
                            Ok(Ok(v)) => Ok(v),
                            Ok(Err(e)) => Err(e),
                            Err(_elapsed) => Err("Upload timed out after 1 hour.".to_string()),
                        }
                    },
                    |r: Result<(String, String, Option<Vec<u8>>, String), String>| match r {
                        Ok((name, ticket, thumbnail, local_path)) => AppMessage::FileDownloaded {
                            name,
                            ticket,
                            thumbnail,
                            local_path: Some(local_path),
                        },
                        Err(e) => AppMessage::FileUploadFailed(e),
                    },
                )
            }

            AppMessage::ExecuteFolderSend(encoded) => {
                let parts: Vec<&str> = encoded.splitn(3, '|').collect();
                if parts.len() < 3 {
                    return iced::Task::none();
                }
                let folder_name = parts[0].to_string();
                let abs_path = parts[1].to_string();
                self.pending_file_upload = Some((folder_name.clone(), 0));
                self.file_upload_spinner_frame = 0;

                // Create a local "sharing" card immediately so the user sees
                // feedback while the directory is imported and broadcast.
                let local_label = self.local_label.clone();
                self.download_entry_index = Some(self.entries.len());
                self.entries_push(ChatEntry::system_download(
                    String::new(),
                    TransferKind::File,
                    folder_name.clone(),
                    String::new(),
                    &local_label,
                    None,
                ));
                if let Some(idx) = self.download_entry_index {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(dl) = entry.download.as_mut() {
                            dl.is_folder = true;
                            dl.state = DownloadState::Active {
                                bytes: 0,
                                total: None,
                            };
                        }
                        entry.body = format!("Sharing folder: {folder_name}");
                    }
                }

                let blob_store = self.blob_store.clone();
                let sender = self.sender.clone();
                let secret_key = self.secret_key.clone();
                let endpoint_addr = self.endpoint.addr();
                let upload_timeout = std::time::Duration::from_secs(3600);
                iced::Task::perform(
                    async move {
                        let result = tokio::time::timeout(upload_timeout, async move {
                            let path_buf = std::path::PathBuf::from(&abs_path);
                            if !path_buf.is_dir() {
                                return Err("Selected path is not a directory".to_string());
                            }
                            // Import the whole directory into a HashSeq
                            // collection (SENDME-01 pipeline).
                            let (temp_tag, total_size, collection) = boru_core::collection_transfer::import_collection(
                                &blob_store,
                                &path_buf,
                                8,
                            )
                            .await
                            .map_err(|e| format!("Failed to import folder: {e}"))?;
                            let hash = temp_tag.hash();
                            let ticket_str = blob_ticket_string(
                                endpoint_addr,
                                hash,
                                iroh_blobs::BlobFormat::HashSeq,
                            );
                            let collection_hash: MessageHash = *hash.as_bytes();
                            let collection_entries = collection.len() as u64;
                            let msg = crate::Message::FileShare {
                                name: folder_name.clone(),
                                ticket: ticket_str.clone(),
                                size: total_size,
                                thumbnail_hash: None,
                                collection_hash: Some(collection_hash),
                                collection_entries,
                            };
                            let encoded_msg =
                                SignedMessage::sign_and_encode(&secret_key, &msg)
                                    .map_err(|e| format!("Failed to sign: {e}"))?;
                            if let Some(ref sender) = sender {
                                match sender.broadcast(encoded_msg).await {
                                    Ok(()) => tracing::info!(
                                        name = %folder_name,
                                        size = total_size,
                                        entries = collection_entries,
                                        "FolderShare broadcast OK"
                                    ),
                                    Err(e) => tracing::error!(
                                        name = %folder_name,
                                        size = total_size,
                                        error = %e,
                                        "FolderShare broadcast FAILED"
                                    ),
                                }
                            }
                            Ok::<_, String>((folder_name.clone(), ticket_str, abs_path))
                        })
                        .await;
                        match result {
                            Ok(Ok(v)) => Ok(v),
                            Ok(Err(e)) => Err(e),
                            Err(_elapsed) => Err("Folder upload timed out after 1 hour.".to_string()),
                        }
                    },
                    |r: Result<(String, String, String), String>| match r {
                        Ok((name, ticket, local_path)) => AppMessage::FileDownloaded {
                            name,
                            ticket,
                            thumbnail: None,
                            local_path: Some(local_path),
                        },
                        Err(e) => AppMessage::FileUploadFailed(e),
                    },
                )
            }

            AppMessage::ExecuteImageSend(encoded) => {
                let parts: Vec<&str> = encoded.splitn(3, '|').collect();
                if parts.len() < 3 {
                    return iced::Task::none();
                }
                let filename = parts[0].to_string();
                let abs_path = parts[1].to_string();
                self.pending_image_upload = Some(filename.clone());
                self.image_upload_spinner_frame = 0;
                // Capture the conversation ownership token when the upload
                // starts. If the user switches rooms while it is in flight,
                // the completion's generation will not match and the stale
                // local entry is caught in debug builds.
                let generation = self.conversation_generation;

                let blob_store = self.blob_store.clone();
                let storage = self.storage.clone();
                let sender = self.sender.clone();
                let secret_key = self.secret_key.clone();
                let _fname = filename.clone();
                let local_pk = self.local_public;

                iced::Task::perform(
                    async move {
                        let path_buf = std::path::PathBuf::from(&abs_path);
                        // Validate file size before reading to avoid loading
                        // a multi-GiB file into memory just to reject it.
                        let metadata = tokio::fs::metadata(&path_buf)
                            .await
                            .map_err(|e| format!("Failed to inspect image: {e}"))?;
                        if metadata.len() > CHAT_IMAGE_MAX_BYTES as u64 {
                            return Err(format!(
                                "Image must be {} MiB or smaller.",
                                CHAT_IMAGE_MAX_BYTES / (1024 * 1024)
                            ));
                        }
                        let full_bytes = tokio::fs::read(&path_buf)
                            .await
                            .map_err(|e| format!("Failed to read image: {e}"))?;
                        // Detect GIF files: skip WebP conversion to preserve
                        // animation frames.  The receiver-side decode_gif_frames
                        // path handles both animated and static GIFs correctly.
                        let is_gif = filename.to_lowercase().ends_with(".gif");
                        let (opt_bytes, wire_name, mime_type, compression_note) = if is_gif {
                            // Transmit GIF bytes unchanged — only enforce the
                            // size cap.  Animated frames survive end-to-end.
                            (
                                full_bytes.clone(),
                                filename.clone(),
                                "image/gif",
                                String::new(),
                            )
                        } else {
                            // Convert to WebP: resize, strip metadata, encode as
                            // lossless WebP.  Errors are reported to the user
                            // rather than silently falling back to the original bytes,
                            // because the original may be many MiB.
                            let orig_size = full_bytes.len();
                            let (opt_bytes, _orig_size, webp_size) =
                                optimize_chat_image_to_webp(&full_bytes)
                                    .map_err(|e| format!("WebP conversion failed: {e}"))?;
                            // Append compression ratio to the image card label
                            let compression_note = if orig_size > 0 && webp_size < orig_size {
                                let saved_pct = (1.0 - webp_size as f64 / orig_size as f64) * 100.0;
                                format!(" ({saved_pct:.0}% smaller)")
                            } else {
                                String::new()
                            };
                            // Rename the file with .webp extension
                            let webp_name = {
                                let path = std::path::Path::new(&filename);
                                if let Some(stem) = path.file_stem() {
                                    format!("{}.webp", stem.to_string_lossy())
                                } else {
                                    format!("{filename}.webp")
                                }
                            };
                            (opt_bytes, webp_name, "image/webp", compression_note)
                        };
                        let fname = wire_name.clone();
                        let display_name = format!("{wire_name}{compression_note}");
                        // Add to blob store.  Both the sender's preview and the
                        // receiver's inline display use these bytes.
                        let tag = blob_store
                            .blobs()
                            .add_bytes(opt_bytes.clone())
                            .await
                            .map_err(|e| format!("Failed to hash image: {e}"))?;
                        #[expect(unused_imports)]
                        use iroh_blobs::api::proto::TagInfo;
                        let hash: MessageHash = *tag.hash.as_bytes();
                        let msg = crate::Message::ImageShare {
                            name: wire_name.clone(),
                            hash,
                        };
                        let encoded = SignedMessage::sign_and_encode(&secret_key, &msg)
                            .map_err(|e| format!("Failed to sign: {e}"))?;
                        if let Some(ref sender) = sender {
                            sender.broadcast(encoded).await.ok();
                        }
                        // Sender-side bookkeeping off the broadcast critical
                        // path: register the upload with the profile only
                        // after the announcement is out. A failure here must
                        // not fail the send — the image is already delivered.
                        if let Some(storage) = storage.as_ref() {
                            // SQLite write — defer to the blocking pool so
                            // the Task::perform worker never blocks on disk
                            // I/O (BORU-AUDIT-18).
                            let stg = storage.clone();
                            let local_pk_str = local_pk.to_string();
                            let wire_name_cl = wire_name.clone();
                            let mime_cl = mime_type.to_string();
                            let bytes_cl = opt_bytes.clone();
                            if let Err(e) = stg
                                .run_blocking("app.register_chat_upload", move |s| {
                                    s.register_chat_upload(
                                        &local_pk_str,
                                        &wire_name_cl,
                                        &mime_cl,
                                        &bytes_cl,
                                    )
                                })
                                .await
                            {
                                tracing::warn!(
                                    name = %wire_name,
                                    error = %e,
                                    "register_chat_upload failed after image broadcast"
                                );
                            }
                        }
                        Ok((local_pk, fname, display_name, opt_bytes, hash))
                    },
                    move |r: Result<(PublicKey, String, String, Vec<u8>, MessageHash), String>| {
                        match r {
                            Ok((sender_pk, name, display_name, bytes, hash)) => {
                                AppMessage::ImageDownloaded {
                                    sender: sender_pk,
                                    name,
                                    display_name,
                                    image_bytes: bytes,
                                    message_hash: hash,
                                    image_identifier: None,
                                    generation,
                                }
                            }
                            Err(e) => AppMessage::ImageUploadFailed(e),
                        }
                    },
                )
            }

            AppMessage::ExecuteDownload => match self.download_entry_index {
                Some(entry_index) => {
                    return self.update(AppMessage::ExecuteDownloadAt(entry_index))
                }
                None => {
                    return iced::Task::done(AppMessage::ErrorMsg(
                        "No pending file to download.".into(),
                    ))
                }
            },
            AppMessage::ExecuteDownloadAt(entry_index) => {
                self.video_card_menu_open = None;
                let Some(entry) = self.entries.get(entry_index) else {
                    return iced::Task::done(AppMessage::ErrorMsg("Entry not found.".into()));
                };
                let Some(dl) = entry.download.clone() else {
                    return iced::Task::done(AppMessage::ErrorMsg("No download attached.".into()));
                };
                // A download may be (re)started from a state where the user
                // explicitly asked for it; see `download_restartable`.
                if !download_restartable(&dl.state) {
                    return iced::Task::none();
                }
                if let Err(error) = validate_attachment_filename(&dl.name) {
                    return iced::Task::done(AppMessage::ErrorMsg(format!(
                        "Download rejected: {error}"
                    )));
                }
                if let Some(e) = self.entries.get_mut(entry_index) {
                    if let Some(ref mut d) = e.download {
                        // Carry forward the total from Ready or a re-download
                        // so the progress bar appears immediately when the
                        // user clicks Download / Retry.
                        let total = match &d.state {
                            DownloadState::Ready { total } => *total,
                            DownloadState::Completed { total_size, .. } => *total_size,
                            _ => None,
                        };
                        d.state = DownloadState::Active { bytes: 0, total };
                    }
                }
                // The Active card is taller than the Ready card. Rebuild the
                // virtualized layout immediately so the card and all later
                // messages keep their correct positions before the first
                // progress event arrives.
                // This is a card reflow, not a new timeline entry.  Do not
                // re-arm the bottom snap here: if the user has scrolled up,
                // the Ready -> Active height change must preserve that reading
                // position.  The append path already arms a single snap when
                // a genuinely new entry arrives while following the latest.
                self.layout_cache.borrow_mut().invalidate_from(entry_index);
                self.download_entry_index = Some(entry_index);
                let blob_store = self.blob_store.clone();
                let endpoint = self.endpoint.clone();
                let neighbors = self.neighbors.clone();
                let _safety = self.public_room_safety.clone();
                let ticket_str = dl.ticket.clone();
                let name = dl.name.clone();
                let kind = dl.kind;
                let is_folder = dl.is_folder;
                let overwrite_policy = dl.overwrite_policy;
                let expected_hash = dl.expected_content_hash.clone();
                let content_hash_fallback = dl
                    .expected_content_hash
                    .clone()
                    .unwrap_or_else(|| "download".to_string());
                let data_dir = self.data_dir.clone();
                let progress_queue = self.download_progress_queue.clone();
                iced::Task::perform(
                    async move {
                        let ticket: iroh_blobs::ticket::BlobTicket = ticket_str
                            .parse()
                            .map_err(|e| format!("Invalid ticket: {e}"))?;
                        let (addr, hash, _format) = ticket.into_parts();
                        let node_id = addr.id;
                        let candidates = download_candidates(node_id, &neighbors);

                        let dl_dir = data_dir.join("downloads");
                        let _ = tokio::fs::create_dir_all(&dl_dir).await;
                        if is_folder {
                            // Whole-directory share (SENDME-01): download the
                            // HashSeq collection and expand it into a folder
                            // tree under the downloads directory.
                            let save_dir = boru_core::collection_transfer::download_collection_to_dir(
                                &blob_store,
                                &endpoint,
                                hash,
                                candidates,
                                &name,
                                &dl_dir,
                            )
                            .await
                            .map_err(|e| format!("Folder download failed: {e}"))?;
                            return Ok::<_, String>((name.clone(), save_dir, false));
                        }
                        // BORU-AUDIT-21: fuse validation + creation into one
                        // atomic reservation (O_EXCL + O_NOFOLLOW) instead of
                        // checking a path and reopening it later.
                        let mut destination =
                            match boru_core::safe_destination::reserve_download_destination(
                                &dl_dir,
                                &name,
                                &content_hash_fallback,
                                overwrite_policy,
                            )
                            .map_err(|e| format!("Unsafe download name: {e}"))?
                            {
                                boru_core::safe_destination::Reservation::Use(dest) => dest,
                                boru_core::safe_destination::Reservation::Skip => {
                                    return Ok::<_, String>((name.clone(), dl_dir.join(&name), true));
                                }
                            };
                        download_blob_to_file(
                            &blob_store,
                            &endpoint,
                            hash,
                            candidates,
                            name.clone(),
                            kind,
                            &mut destination,
                            expected_hash.as_deref(),
                            {
                                let queue = progress_queue.clone();
                                move |ev| {
                                    if let Ok(mut q) = queue.lock() {
                                        q.push_back(ev);
                                    }
                                }
                            },
                            None,
                        )
                        .await
                        .map_err(|e| format!("Download failed: {e}"))?;
                        let save_path = destination
                            .publish()
                            .map_err(|e| format!("Publish failed: {e}"))?;
                        Ok::<_, String>((name.clone(), save_path, false))
                    },
                    move |r| match r {
                        Ok((name, path, skipped)) if skipped => {
                            AppMessage::ErrorMsg(format!(
                                "Skipped — {name} already exists (overwrite policy is Skip)."
                            ))
                        }
                        Ok((name, path, _)) => AppMessage::DownloadDone(name, path),
                        Err(e) => AppMessage::DownloadFailed(e),
                    },
                )
            }

            AppMessage::PauseDownloadAt(entry_index) => {
                self.push_system("Pause requested — transfer suspension not yet implemented.");
                if let Some(entry) = self.entries.get_mut(entry_index) {
                    if let Some(download) = entry.download.as_mut() {
                        if let DownloadState::Active { bytes, total } = &download.state {
                            download.state = DownloadState::Paused {
                                bytes: *bytes,
                                total: *total,
                            };
                            self.layout_cache.borrow_mut().invalidate_from(entry_index);
                        }
                    }
                }
                iced::Task::none()
            }
            AppMessage::ResumeDownloadAt(entry_index) => {
                self.push_system("Resume requested — transfer resumption not yet implemented.");
                if let Some(entry) = self.entries.get_mut(entry_index) {
                    if let Some(download) = entry.download.as_mut() {
                        if matches!(download.state, DownloadState::Paused { .. }) {
                            // Revert to Ready so the user can click Download again.
                            // In a full implementation this would resume the transfer.
                            download.state = DownloadState::Ready { total: None };
                            self.layout_cache.borrow_mut().invalidate_from(entry_index);
                        }
                    }
                }
                iced::Task::none()
            }
            AppMessage::CancelDownloadAt(entry_index) => {
                self.video_card_menu_open = None;
                self.push_system(String::from("Cancel requested."));
                if let Some(entry) = self.entries.get_mut(entry_index) {
                    if let Some(download) = entry.download.as_mut() {
                        if !matches!(download.state, DownloadState::Completed { .. }) {
                            download.state = DownloadState::Cancelled;
                            self.layout_cache.borrow_mut().invalidate_from(entry_index);
                        }
                    }
                }
                iced::Task::none()
            }

            AppMessage::DownloadInitiated {
                content_hash,
                peer,
                download_id,
            } => {
                // Remove from pending set since the operation completed.
                self.pending_downloads.remove(&(content_hash.clone(), peer));
                let label = self
                    .names
                    .get(&peer)
                    .cloned()
                    .unwrap_or_else(|| peer.fmt_short().to_string());
                self.push_system(format!("Download queued for *{label}* (id={download_id})"));
                iced::Task::none()
            }
            AppMessage::DownloadInitiationFailed {
                content_hash,
                peer,
                error,
            } => {
                // Remove from pending set since the operation completed (with error).
                self.pending_downloads.remove(&(content_hash, peer));
                self.push_system(format!("Download failed: {error}"));
                iced::Task::none()
            }



            AppMessage::FileSent(name) => {
                self.push_system(format!("Sharing: {name}"));
                iced::Task::none()
            }
            AppMessage::DownloadDone(name, path) => {
                tracing::info!(%name, path=%path.display(), "DownloadDone received");
                self.push_system(format!("*{name}* is complete"));
                let poster_path = path.clone();
                let mut is_video = false;
                let completed_idx = self
                    .entries
                    .iter()
                    .position(|entry| {
                        entry.download.as_ref().is_some_and(|download| {
                            download.name == name
                                && matches!(
                                    download.state,
                                    DownloadState::Active { .. } | DownloadState::Completed { .. }
                                )
                        })
                    })
                    .or(self.download_entry_index);
                tracing::info!(
                    idx=?completed_idx,
                    download_entry_index=?self.download_entry_index,
                    "DownloadDone: resolved entry index"
                );
                if let Some(idx) = completed_idx {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(download) = entry.download.as_mut() {
                            tracing::info!(
                                idx,
                                prev_state=?download.state,
                                "DownloadDone: before setting Completed"
                            );
                            is_video = download.kind == TransferKind::Video;
                            // VIDCARD-20: a user-initiated Cancel (or
                            // another genuinely user terminal state) must
                            // not be overwritten by the late completion of
                            // the transfer that was still running in the
                            // background — the card would otherwise snap
                            // back to "Ready to play" after the user
                            // cancelled it.
                            //
                            // VID-01: `Completed { saved_path: None }` is
                            // NOT such a state — it is the transient
                            // "Verifying" placeholder set by the queued
                            // TransferProgress::Completed event when it
                            // beats this DownloadDone to the UI. It MUST
                            // be upgraded with the real path, otherwise the
                            // video card stays at "Verifying…" forever even
                            // though the file exists on disk.
                            if !download_done_can_complete(&download.state) {
                                tracing::info!(
                                    idx,
                                    state=?download.state,
                                    "DownloadDone: ignoring completion for user terminal state"
                                );
                                return iced::Task::none();
                            }
                            let total_size = match &download.state {
                                DownloadState::Active { total, .. } => *total,
                                DownloadState::Completed { total_size, .. } => *total_size,
                                _ => None,
                            };
                            download.state = DownloadState::Completed {
                                saved_name: name.clone(),
                                saved_path: Some(path),
                                total_size,
                            };
                            self.layout_cache.borrow_mut().invalidate_from(idx);
                        }
                    }
                }
                self.pending_file = None;
                if is_video {
                    // Mark the async metadata load as in-flight so the card
                    // renders a stable loading placeholder at the bounded
                    // default frame (VIDCARD-09).
                    if let Some(idx) = completed_idx {
                        if let Some(entry) = self.entries.get_mut(idx) {
                            if let Some(download) = entry.download.as_mut() {
                                download.metadata_loading = true;
                            }
                        }
                    }
                    let cache_dir = self.data_dir.join("cache").join("video-posters");
                    let poster_name = name.clone();
                    let metadata_name = name.clone();
                    let probe_path = poster_path.clone();
                    let poster_task = iced::Task::perform(
                        async move {
                            match tokio::task::spawn_blocking(move || {
                                video_poster::generate(&poster_path, &cache_dir)
                                    .map(|poster| (poster.bytes, poster.dimensions))
                            })
                            .await
                            {
                                Ok(result) => result,
                                Err(error) => Err(format!("poster worker failed: {error}")),
                            }
                        },
                        move |poster| AppMessage::PosterGenerated {
                            name: poster_name,
                            poster,
                        },
                    );
                    let metadata_task = iced::Task::perform(
                        async move {
                            match tokio::task::spawn_blocking(move || {
                                boru_core::video_playback::probe_local_video_metadata(&probe_path)
                            })
                            .await
                            {
                                Ok(result) => result,
                                Err(error) => Err(format!("metadata worker failed: {error}")),
                            }
                        },
                        move |metadata| AppMessage::VideoMetadataProbed {
                            name: metadata_name,
                            metadata,
                        },
                    );
                    return iced::Task::batch(vec![poster_task, metadata_task]);
                }
                iced::Task::none()
            }
            AppMessage::DownloadDonePeerFile(name, path) => {
                // Transition to Completed if initiated by a GUI test action.
                if let Some(action_id) = self.pending_download_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                self.push_system(format!("*{name}* is complete"));
                if let Some(content_hash) = self.catalogue_name_to_hash(&name) {
                    self.catalogue_downloads.insert(
                        content_hash,
                        CatalogueDownloadState::Completed { path: path.clone() },
                    );
                }
                let poster_path = path.clone();
                let mut is_video = false;
                if let Some(idx) = self.download_entry_index {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(download) = entry.download.as_mut() {
                            is_video = download.kind == TransferKind::Video;
                            // VIDCARD-20: same terminal-state guard as
                            // DownloadDone — a user-initiated cancel must not
                            // be flipped back to Completed by a late peer-file
                            // completion. VID-01: the transient
                            // `Completed { saved_path: None }` "Verifying"
                            // placeholder is NOT a user terminal state and
                            // must still be upgraded with the real path.
                            if !download_done_can_complete(&download.state) {
                                tracing::info!(
                                    idx,
                                    state=?download.state,
                                    "DownloadDonePeerFile: ignoring completion for user terminal state"
                                );
                                return iced::Task::none();
                            }
                            let total_size = match &download.state {
                                DownloadState::Active { total, .. } => *total,
                                DownloadState::Completed { total_size, .. } => *total_size,
                                _ => None,
                            };
                            download.state = DownloadState::Completed {
                                saved_name: name.clone(),
                                saved_path: Some(path),
                                total_size,
                            };
                            self.layout_cache.borrow_mut().invalidate_from(idx);
                        }
                    }
                }
                if is_video {
                    // Mark the async metadata load as in-flight so the card
                    // renders a stable loading placeholder at the bounded
                    // default frame (VIDCARD-09).
                    if let Some(idx) = self.download_entry_index {
                        if let Some(entry) = self.entries.get_mut(idx) {
                            if let Some(download) = entry.download.as_mut() {
                                download.metadata_loading = true;
                            }
                        }
                    }
                    let cache_dir = self.data_dir.join("cache").join("video-posters");
                    let poster_name = name.clone();
                    let metadata_name = name.clone();
                    let probe_path = poster_path.clone();
                    let poster_task = iced::Task::perform(
                        async move {
                            match tokio::task::spawn_blocking(move || {
                                video_poster::generate(&poster_path, &cache_dir)
                                    .map(|poster| (poster.bytes, poster.dimensions))
                            })
                            .await
                            {
                                Ok(result) => result,
                                Err(error) => Err(format!("poster worker failed: {error}")),
                            }
                        },
                        move |poster| AppMessage::PosterGenerated {
                            name: poster_name,
                            poster,
                        },
                    );
                    let metadata_task = iced::Task::perform(
                        async move {
                            match tokio::task::spawn_blocking(move || {
                                boru_core::video_playback::probe_local_video_metadata(&probe_path)
                            })
                            .await
                            {
                                Ok(result) => result,
                                Err(error) => Err(format!("metadata worker failed: {error}")),
                            }
                        },
                        move |metadata| AppMessage::VideoMetadataProbed {
                            name: metadata_name,
                            metadata,
                        },
                    );
                    return iced::Task::batch(vec![poster_task, metadata_task]);
                }
                iced::Task::none()
            }
            AppMessage::PosterGenerated { name, poster } => {
                match poster {
                    Ok((bytes, dimensions)) => {
                        if let Some(entry) = self.entries.iter_mut().find(|entry| {
                            entry.download.as_ref().is_some_and(|download| {
                                download.name == name && download.kind == TransferKind::Video
                            })
                        }) {
                            if let Some(download) = entry.download.as_mut() {
                                download.poster_dimensions = dimensions;
                                // Recreate the handle so the freshly generated
                                // poster actually renders (the media frame
                                // reads thumbnail_handle, not thumbnail bytes).
                                download.thumbnail_handle = Some(
                                    iced::widget::image::Handle::from_bytes(bytes.clone()),
                                );
                                download.thumbnail = Some(bytes);
                                self.layout_cache.borrow_mut().clear();
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(file = %name, %error, "video poster generation failed; keeping video playable");
                    }
                }
                iced::Task::none()
            }
            AppMessage::VideoMetadataProbed { name, metadata } => {
                // VIDCARD-09: apply real intrinsic dimensions/duration once the
                // async probe resolves. Success carries measurements only; a
                // failed probe keeps the bounded generic contain frame and the
                // problem is logged through the existing diagnostics system.
                // Open File / Open Folder actions remain available.
                match metadata {
                    Ok(meta) => {
                        if let Some(entry) = self.entries.iter_mut().find(|entry| {
                            entry.download.as_ref().is_some_and(|download| {
                                download.name == name && download.kind == TransferKind::Video
                            })
                        }) {
                            if let Some(download) = entry.download.as_mut() {
                                download.metadata_loading = false;
                                download.metadata_failed = false;
                                download.duration_ms = meta.duration_ms;
                                // Prefer the real intrinsic dimensions when the
                                // poster path did not already provide them.
                                if download.poster_dimensions.is_none() {
                                    if let (Some(width), Some(height)) = (meta.width, meta.height)
                                    {
                                        download.poster_dimensions = Some((width, height));
                                    }
                                }
                                self.layout_cache.borrow_mut().clear();
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            file = %name,
                            %error,
                            "video metadata probe failed; keeping bounded generic media frame"
                        );
                        if let Some(entry) = self.entries.iter_mut().find(|entry| {
                            entry.download.as_ref().is_some_and(|download| {
                                download.name == name && download.kind == TransferKind::Video
                            })
                        }) {
                            if let Some(download) = entry.download.as_mut() {
                                download.metadata_loading = false;
                                download.metadata_failed = true;
                                self.layout_cache.borrow_mut().clear();
                            }
                        }
                        // Log the metadata problem through the existing
                        // diagnostics system (shared singleton, surfaced on the
                        // dashboard/activity log like other UI-detectable issues).
                        boru_core::chat_core::DIAGNOSTICS.record(
                            None,
                            boru_core::diagnostics::DiagnosticEventKind::Error(format!(
                                "video metadata probe failed for {name}: {error}"
                            )),
                        );
                    }
                }
                iced::Task::none()
            }
            AppMessage::DownloadFailed(error) => {
                // Transition to Failed if initiated by a GUI test action.
                if let Some(action_id) = self.pending_download_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Failed);
                }
                self.push_system(format!("Download failed: {error}"));
                // If the error carries a catalogue file name (format "name : error"),
                // mark it as failed in the catalogue view.
                if let Some(name_end) = error.find(" : ") {
                    let cat_name = error[..name_end].to_string();
                    if let Some(content_hash) = self.catalogue_name_to_hash(&cat_name) {
                        self.catalogue_downloads
                            .insert(content_hash, CatalogueDownloadState::Failed(error.clone()));
                    }
                }
                let mut updated = false;
                if let Some(idx) =
                    self.current_download_entry_index(self.active_download_transfer_id)
                {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(download) = entry.download.as_mut() {
                            download.state = DownloadState::Failed {
                                failure: DownloadFailure::from_error(error),
                            };
                            self.layout_cache.borrow_mut().invalidate_from(idx);
                            updated = true;
                        }
                    }
                }
                if updated {
                    self.active_download_transfer_id = None;
                }
                iced::Task::none()
            }
            AppMessage::DownloadProgress(progress) => {
                self.handle_download_progress(progress);
                iced::Task::none()
            }
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
            AppMessage::OpenDownloadedFile(name) => {
                self.video_card_menu_open = None;
                if let Err(error) = self.open_downloaded_file(&name) {
                    if error.starts_with("File not found:") {
                        for (idx, entry) in self.entries.iter_mut().enumerate() {
                            if let Some(download) = entry.download.as_mut() {
                                if matches!(download.state, DownloadState::Completed { .. })
                                    && download.name == name
                                {
                                    download.state = DownloadState::Failed {
                                        failure: DownloadFailure::FileRemoved,
                                    };
                                    self.layout_cache.borrow_mut().invalidate_from(idx);
                                    break;
                                }
                            }
                        }
                    }
                    self.push_system(format!("Open failed: {error}"));
                }
                iced::Task::none()
            }
            AppMessage::ReshareFile(entry_index) => {
                self.video_card_menu_open = None;
                if let Some(entry) = self.entries.get(entry_index) {
                    if let Some(dl) = &entry.download {
                        if let DownloadState::Completed {
                            ref saved_name,
                            ref saved_path,
                            ..
                        } = dl.state
                        {
                            if let Some(path) = saved_path {
                                let encoded =
                                    format!("{}|{}|{}", saved_name, path.display(), path.display());
                                return self.update(AppMessage::ExecuteFileSend(encoded));
                            }
                        }
                    }
                }
                iced::Task::none()
            }
            AppMessage::MintShortCode(entry_index) => {
                // Mint a short code for the download card's ticket, subscribe
                // to the code-derived rendezvous topic, and open the dialog
                // showing the code. The subscription is held (sender half
                // stored) so the code's topic stays alive while the dialog is
                // open — the ephemeral subscribe-broadcast-drop pattern is
                // broken (the mesh must form before the receiver subscribes).
                if self.short_code_minting {
                    return iced::Task::none();
                }
                let Some(entry) = self.entries.get(entry_index) else {
                    return iced::Task::none();
                };
                let Some(dl) = &entry.download else {
                    return iced::Task::none();
                };
                let ticket = dl.ticket.clone();
                let name = dl.name.clone();
                let size = match &dl.state {
                    DownloadState::Completed { total_size, .. } => total_size.unwrap_or(0),
                    DownloadState::Shared { size, .. } => size.unwrap_or(0),
                    DownloadState::Ready { total } | DownloadState::Active { total, .. } => {
                        total.unwrap_or(0)
                    }
                    _ => 0,
                };
                if ticket.is_empty() {
                    self.short_code_dialog_error =
                        Some("This card has no share ticket yet.".to_string());
                    self.show_short_code_dialog = true;
                    return iced::Task::none();
                }
                self.short_code_minting = true;
                self.short_code_dialog_error = None;
                let data_dir = self.data_dir.clone();
                let gossip = self.gossip.clone();
                iced::Task::perform(
                    async move {
                        // Load the store (creating the file on first mint).
                        let mut store =
                            boru_core::short_code::ShortCodeStore::load_or_default(&data_dir)
                                .map_err(|e| format!("failed to open short-code store: {e}"))?;
                        let code = store
                            .mint(
                                &ticket,
                                &name,
                                size,
                                boru_core::short_code::DEFAULT_SHORT_CODE_TTL,
                            )
                            .map_err(|e| format!("failed to mint short code: {e}"))?;
                        let topic = boru_core::short_code::derive_shortcode_topic(&code);
                        let sub = gossip
                            .subscribe(topic, Vec::new())
                            .await
                            .map_err(|e| format!("failed to join short-code topic: {e}"))?;
                        let (sender, _receiver) = sub.split();
                        Ok::<_, String>((code, sender))
                    },
                    |result| match result {
                        Ok((code, sender)) => AppMessage::ShortCodeMinted(Ok((code, sender))),
                        Err(e) => AppMessage::ShortCodeMinted(Err(e)),
                    },
                )
            }
            AppMessage::ShortCodeMinted(result) => {
                self.short_code_minting = false;
                match result {
                    Ok((code, sender)) => {
                        let share = self
                            .short_code_active
                            .clone()
                            .or_else(|| {
                                self.entries
                                    .iter()
                                    .find_map(|entry| {
                                        entry.download.as_ref().map(|dl| ShortCodeActiveShare {
                                            code: code.clone(),
                                            ticket: dl.ticket.clone(),
                                            name: dl.name.clone(),
                                            size: match &dl.state {
                                                DownloadState::Completed { total_size, .. } => {
                                                    total_size.unwrap_or(0)
                                                }
                                                DownloadState::Shared { size, .. } => {
                                                    size.unwrap_or(0)
                                                }
                                                _ => 0,
                                            },
                                        })
                                    })
                            });
                        self.short_code_active = share;
                        self.short_code_sender = Some(sender);
                        self.short_code_dialog_code = Some(code.clone());
                        self.show_short_code_dialog = true;
                        tracing::info!(code = %code, "short-code share minted");
                    }
                    Err(e) => {
                        self.short_code_dialog_error = Some(e);
                        self.show_short_code_dialog = true;
                    }
                }
                iced::Task::none()
            }
            AppMessage::CloseShortCodeDialog => {
                // Dropping the sender leaves the code's rendezvous topic,
                // stopping the periodic re-broadcast.
                self.short_code_sender = None;
                self.short_code_active = None;
                self.short_code_dialog_code = None;
                self.short_code_dialog_error = None;
                self.show_short_code_dialog = false;
                iced::Task::none()
            }
            AppMessage::CopyShortCode(code) => {
                self.toast_message = Some("Short code copied to clipboard".to_string());
                iced::clipboard::write(code)
            }
            AppMessage::OpenRedeemCodeDialog => {
                self.show_redeem_code_dialog = true;
                self.redeem_code_input = String::new();
                self.redeem_code_error = None;
                self.redeem_code_busy = false;
                iced::Task::none()
            }
            AppMessage::CloseRedeemCodeDialog => {
                self.show_redeem_code_dialog = false;
                self.redeem_code_input = String::new();
                self.redeem_code_error = None;
                self.redeem_code_busy = false;
                iced::Task::none()
            }
            AppMessage::RedeemCodeInputChanged(text) => {
                self.redeem_code_input = text;
                self.redeem_code_error = None;
                iced::Task::none()
            }
            AppMessage::RedeemShortCode => {
                let input = self.redeem_code_input.trim().to_string();
                let code = boru_core::short_code::normalise_code(&input);
                if code.is_empty() {
                    self.redeem_code_error = Some("Type a short code first.".to_string());
                    return iced::Task::none();
                }
                if code.len() != boru_core::short_code::SHORT_CODE_LEN {
                    self.redeem_code_error = Some(format!(
                        "Short codes are {} characters.",
                        boru_core::short_code::SHORT_CODE_LEN
                    ));
                    return iced::Task::none();
                }
                if self.redeemed_codes.contains(&code) {
                    self.redeem_code_error =
                        Some("This code was already redeemed in this session.".to_string());
                    return iced::Task::none();
                }
                self.redeem_code_busy = true;
                self.redeem_code_error = None;
                let gossip = self.gossip.clone();
                let topic = boru_core::short_code::derive_shortcode_topic(&code);
                iced::Task::perform(
                    async move {
                        // Subscribe to the rendezvous topic and wait for a
                        // signed announcement matching the code. The
                        // subscription is held for the whole wait so the mesh
                        // has time to form.
                        let sub = gossip
                            .subscribe(topic, Vec::new())
                            .await
                            .map_err(|e| format!("failed to join short-code topic: {e}"))?;
                        let (mut _sender, mut receiver) = sub.split();
                        use n0_future::StreamExt;
                        let deadline = std::time::Instant::now()
                            + std::time::Duration::from_secs(60);
                        loop {
                            if std::time::Instant::now() >= deadline {
                                return Err(
                                    "Timed out waiting for the sharing peer. Make sure \
                                     both peers are on the same relay."
                                        .to_string(),
                                );
                            }
                            let remaining =
                                deadline.saturating_duration_since(std::time::Instant::now());
                            let item = tokio::time::timeout(remaining, receiver.next())
                                .await
                                .map_err(|_| {
                                    "Timed out waiting for the sharing peer.".to_string()
                                })?;
                            let Some(Ok(boru_core::api::Event::Received(msg))) = item else {
                                continue;
                            };
                            let Ok((from, announcement)) = boru_core::short_code::
                                SignedShortCodeAnnouncement::verify(&msg.content, &code)
                            else {
                                continue;
                            };
                            let redemption = ShortCodeRedemption {
                                code: announcement.code.clone(),
                                name: announcement.name.clone(),
                                ticket: announcement.ticket.clone(),
                                size: announcement.size,
                                node_short: from.fmt_short().to_string(),
                            };
                            return Ok(redemption);
                        }
                    },
                    AppMessage::ShortCodeRedeemed,
                )
            }
            AppMessage::ShortCodeRedeemed(result) => {
                self.redeem_code_busy = false;
                match result {
                    Ok(redemption) => {
                        self.redeemed_codes.insert(redemption.code.clone());
                        self.show_redeem_code_dialog = false;
                        // Create the same download card as pasting a ticket.
                        self.download_entry_index = Some(self.entries.len());
                        self.entries_push(ChatEntry::system_download(
                            format!("Receiving via short code: {}", redemption.name),
                            TransferKind::File,
                            redemption.name.clone(),
                            redemption.ticket.clone(),
                            &redemption.node_short,
                            None,
                        ));
                        if let Some(idx) = self.download_entry_index {
                            if let Some(entry) = self.entries.get_mut(idx) {
                                if let Some(dl) = entry.download.as_mut() {
                                    dl.state = DownloadState::Ready {
                                        total: Some(redemption.size),
                                    };
                                }
                            }
                        }
                        self.toast_message = Some(format!(
                            "Short code {} resolved — ready to download {}",
                            redemption.code, redemption.name
                        ));
                    }
                    Err(e) => {
                        self.redeem_code_error = Some(e);
                    }
                }
                iced::Task::none()
            }
            AppMessage::SetOverwritePolicy(entry_index, policy) => {
                if let Some(entry) = self.entries.get_mut(entry_index) {
                    if let Some(dl) = entry.download.as_mut() {
                        dl.overwrite_policy = policy;
                        self.layout_cache.borrow_mut().invalidate_from(entry_index);
                    }
                }
                iced::Task::none()
            }
            AppMessage::ImageDownloaded {
                sender,
                name,
                display_name: _,
                image_bytes,
                message_hash,
                image_identifier,
                generation,
            } => {
                // State-safety: an image download started in a previous
                // conversation must not push its entry into the currently
                // active conversation's display. Detect stale completions in
                // debug builds before entries_push mutates the wrong room.
                debug_assert_eq!(
                    self.conversation_generation, generation,
                    "stale ImageDownloaded for {name}: completion generation {generation} \
                     != current conversation generation {}",
                    self.conversation_generation,
                );
                info!(
                    ?sender,
                    name = %name,
                    image_size = image_bytes.len(),
                    ?message_hash,
                    has_identifier = image_identifier.is_some(),
                    "image download completed",
                );
                self.pending_image_upload = None;
                if sender == self.local_public && image_identifier.is_none() {
                    let mut profile_file = SharedFile::new(
                        &name,
                        image_bytes.len() as u64,
                        "image/webp",
                        SystemTime::now(),
                    );
                    profile_file.id = hex::encode(message_hash);
                    profile_file.hash = Some(message_hash);
                    self.profile_store
                        .shared_files_mut()
                        .retain(|file| file.id != profile_file.id);
                    self.profile_store.add_shared_file(profile_file);
                }
                if self.has_message(&message_hash) {
                    return self.drain_pending_transfers();
                }
                let sender_name = if sender == self.local_public {
                    self.local_label.clone()
                } else {
                    self.names
                        .get(&sender)
                        .cloned()
                        .unwrap_or_else(|| sender.fmt_short().to_string())
                };
                // The image was already saved to the per-user store by the
                // async download task. Use the pre-saved identifier.
                let image_error = match &image_identifier {
                    Some(_) => None,
                    None => Some("Image could not be saved to local store".to_string()),
                };
                let kind = Self::image_chat_kind(sender, self.local_public);
                let mut entry = ChatEntry::image(
                    kind,
                    &sender_name,
                    String::new(),
                    image_bytes,
                    Some(message_hash),
                    None,
                    Some(sender),
                    image_identifier,
                    image_error,
                );
                if entry.image_handle.is_none() && entry.image_error.is_none() {
                    entry.image_error = Some("Image preview unavailable".to_string());
                    entry.bump_gen();
                }
                self.entries_push(entry);
                // Persist to chat_history so images survive restarts.
                // Store image_bytes (in-memory, #[serde(skip)]) and
                // image_identifier (persisted) so history replay can
                // reconstruct a renderable ChatEntry.
                {
                    let topic = self.topic;
                    let local_hex = hex::encode(self.local_public.as_bytes());
                    let mut store = self.chat_history.lock().unwrap();
                    let mut hist_entry =
                        HistoryEntry::new(topic, local_hex, Vec::new(), "image", name.clone());
                    // Reference the just-pushed entry's stored bytes and
                    // identifier so the HistoryEntry carries enough data for
                    // history_entry_to_chat_entry to produce a renderable entry.
                    if let Some(last) = self.entries.last() {
                        hist_entry.image_bytes = last.image_bytes.clone();
                        hist_entry.image_identifier = last.image_identifier.clone();
                    }
                    store.push_with_id(hist_entry);
                }
                self.drain_pending_transfers()
            }
            AppMessage::GifMediaFetched {
                sender,
                gif,
                message_hash,
                bytes,
                generation,
            } => {
                // State-safety: a GIF media fetch started in a previous
                // conversation must not push its entry into the currently
                // active conversation's display.  Unlike ImageDownloaded we
                // warn + early-return rather than debug_assert: room switches
                // are fast and a stale fetch is a normal race, not a bug.
                if self.conversation_generation != generation {
                    warn!(
                        ?sender,
                        gif_id = %gif.provider_id,
                        current = self.conversation_generation,
                        expected = generation,
                        "stale GIF media fetch ignored after room switch",
                    );
                    return iced::Task::none();
                }
                if self.has_message(&message_hash) {
                    return self.drain_pending_transfers();
                }
                let sender_name = if sender == self.local_public {
                    self.local_label.clone()
                } else {
                    self.names
                        .get(&sender)
                        .cloned()
                        .unwrap_or_else(|| sender.fmt_short().to_string())
                };
                let kind = Self::image_chat_kind(sender, self.local_public);
                match bytes {
                    Ok(media_bytes) => {
                        info!(
                            ?sender,
                            gif_id = %gif.provider_id,
                            media_size = media_bytes.len(),
                            "external GIF media fetched",
                        );
                        // MP4 playback renditions play through the inline
                        // video player: save the rendition to the managed
                        // downloads dir and render a Ready video card whose
                        // Play button verifies + opens the local file.
                        // GIF/WebP renditions keep the image path below.
                        if gif.format == GifMediaFormat::Mp4 && cfg!(feature = "video-playback") {
                            let hash_hex = blake3::hash(&media_bytes).to_hex().to_string();
                            let file_stem: String = gif
                                .provider_id
                                .chars()
                                .map(|c| {
                                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
                                    {
                                        c
                                    } else {
                                        '-'
                                    }
                                })
                                .collect();
                            let file_name = if file_stem.is_empty() {
                                format!("klipy-gif-{}.mp4", &hash_hex[..12])
                            } else {
                                format!("{file_stem}.mp4")
                            };
                            let dl_dir = self.data_dir.join("downloads");
                            let save_path = dl_dir.join(&file_name);
                            let saved = std::fs::create_dir_all(&dl_dir)
                                .and_then(|_| std::fs::write(&save_path, &media_bytes))
                                .is_ok();
                            let mut entry = ChatEntry::system_download(
                                format!("Video received: {file_name}"),
                                TransferKind::Video,
                                file_name.clone(),
                                String::new(), // content already local — no ticket
                                sender_name.clone(),
                                None,
                            );
                            // Present like a chat message (remote/local), not
                            // a system notice.
                            entry.kind = kind;
                            entry.label = sender_name.clone();
                            entry.message_hash = Some(message_hash);
                            entry.sender_key = Some(sender);
                            if let Some(dl) = entry.download.as_mut() {
                                dl.expected_content_hash = Some(hash_hex);
                                if saved {
                                    dl.state = DownloadState::Shared {
                                        name: file_name,
                                        path: save_path,
                                        size: Some(media_bytes.len() as u64),
                                    };
                                } else {
                                    dl.state = DownloadState::Failed {
                                        failure: DownloadFailure::Other {
                                            detail: "could not save shared MP4 GIF to downloads"
                                                .to_string(),
                                        },
                                    };
                                    warn!(
                                        gif_id = %gif.provider_id,
                                        ?save_path,
                                        "failed to save shared MP4 GIF for video playback",
                                    );
                                }
                            }
                            let entry_index = self.entries_push(entry);
                            // Fetch the Klipy preview rendition (GIF/WebP) as
                            // the card thumbnail, mirroring the file-share
                            // poster path. Best-effort: on failure the card
                            // keeps its video placeholder.
                            if saved {
                                if let Some(preview_url) = gif.preview_url.as_ref() {
                                    let url = preview_url.clone();
                                    return iced::Task::batch(vec![
                                        self.drain_pending_transfers(),
                                        iced::Task::perform(
                                            async move {
                                                fetch_gif_media_bytes(&url)
                                                    .await
                                                    .map(|bytes| (entry_index, bytes))
                                            },
                                            |result| match result {
                                                Ok((idx, bytes)) => {
                                                    AppMessage::ThumbnailFetched {
                                                        entry_index: idx,
                                                        thumbnail_bytes: bytes,
                                                    }
                                                }
                                                Err(error) => {
                                                    warn!(
                                                        %error,
                                                        "klipy preview thumbnail fetch failed",
                                                    );
                                                    AppMessage::Noop
                                                }
                                            },
                                        ),
                                    ]);
                                }
                            }
                            return self.drain_pending_transfers();
                        }
                        // Reuse the standard image rendering path (GIF
                        // frames decode automatically for animated GIFs).
                        let mut entry = ChatEntry::image(
                            kind,
                            &sender_name,
                            String::new(),
                            media_bytes,
                            Some(message_hash),
                            None,
                            Some(sender),
                            None,
                            None,
                        );
                        if entry.image_handle.is_none() && entry.image_error.is_none() {
                            entry.image_error =
                                Some("GIF media could not be decoded".to_string());
                            entry.bump_gen();
                        }
                        self.entries_push(entry);
                        self.drain_pending_transfers()
                    }
                    Err(error) => {
                        // Missing or expired media URL → render a clear
                        // fallback card instead of a broken/blank image.
                        warn!(
                            ?sender,
                            gif_id = %gif.provider_id,
                            %error,
                            "external GIF media unavailable",
                        );
                        let mut entry = ChatEntry::image(
                            kind,
                            &sender_name,
                            String::new(),
                            Vec::new(),
                            Some(message_hash),
                            None,
                            Some(sender),
                            None,
                            Some(format!("GIF unavailable: {error}")),
                        );
                        // No bytes → no decodable handle; the view renders
                        // the image_error fallback placeholder.
                        entry.image_handle = None;
                        entry.image_bytes = None;
                        entry.bump_gen();
                        self.entries_push(entry);
                        self.drain_pending_transfers()
                    }
                }
            }
            AppMessage::ProfileImageDownloaded(peer, image_bytes) => {
                let size = image_bytes.len();
                if image_bytes.is_empty() || size > 2 * 1024 * 1024 {
                    debug!(
                        %peer,
                        size,
                        reason = if image_bytes.is_empty() { "empty" } else { "oversized" },
                        "profile image download rejected",
                    );
                    // Ignore empty or oversized images (>2MB) and clear cached ticket
                    // so the next AboutMe broadcast can retry.
                    self.friend_image_tickets.remove(&peer);
                    return iced::Task::none();
                }
                // Persist to disk cache so the image survives restarts and is
                // available even when the peer is offline.
                save_friend_profile_image(&self.data_dir, &peer, &image_bytes);
                let handle = iced::widget::image::Handle::from_bytes(image_bytes);
                self.friend_image_handles.insert(peer, Some(handle.clone()));
                self.enforce_profile_image_cap();
                // Backfill existing chat entries that were pushed before the async
                // profile image download completed.  Without this, the first message
                // from a peer forever shows the "?" fallback because entries_push
                // only copies avatar_handle at push time, when friend_image_handles
                // still contains `Some(None)` (seeded by record_profile_image_ticket).
                for entry in &mut self.entries {
                    if entry.sender_key == Some(peer) && entry.avatar_handle.is_none() {
                        entry.avatar_handle = Some(handle.clone());
                        entry.bump_gen();
                    }
                }
                // Trigger UI re-draw by marking friends dirty so the sidebar
                // re-renders with the updated profile image.
                self.mark_friends_sidebar_dirty();
                debug!(%peer, size, "profile image loaded and cached");
                iced::Task::none()
            }
            AppMessage::ProfileImageDownloadFailed(peer) => {
                warn!(%peer, "profile image download failed");
                // Download failed (e.g. peer temporarily unreachable).  Remove
                // the cached ticket so the next periodic AboutMe re-broadcast
                // can retry the download.  Without this, the dedup guard in
                // record_profile_image_ticket would skip all future AboutMe
                // messages with the same ticket string, leaving the avatar
                // stuck on the 👤 fallback permanently.
                self.friend_image_tickets.remove(&peer);
                iced::Task::none()
            }
            AppMessage::ImageHydrated {
                index,
                handle,
                error,
            } => {
                if let Some(entry) = self.entries.get_mut(index) {
                    if let Some(h) = handle {
                        entry.image_handle = Some(h);
                        entry.image_error = None;
                    } else if let Some(err) = error {
                        entry.image_error = Some(err);
                    }
                    entry.bump_gen();
                }
                iced::Task::none()
            }
            AppMessage::ImageUploadFailed(error) => {
                info!(%error, "image upload failed");
                self.pending_image_upload = None;
                self.push_system(format!("Image upload failed: {error}"));
                iced::Task::none()
            }
            AppMessage::FileUploadFailed(error) => {
                self.pending_file_upload = None;
                tracing::error!(error = %error, "FileUploadFailed");
                self.push_system(format!("File upload failed: {error}"));
                iced::Task::none()
            }
            AppMessage::FileDownloaded {
                name,
                ticket,
                thumbnail,
                local_path,
            } => {
                self.pending_file_upload = None;
                tracing::info!(
                    name = %name,
                    has_ticket = !ticket.is_empty(),
                    has_thumbnail = thumbnail.is_some(),
                    has_local_path = local_path.is_some(),
                    thumbnail_len = thumbnail.as_ref().map(|b| b.len()).unwrap_or(0),
                    "FileDownloaded"
                );
                // Update the upload-progress entry to Completed.
                //
                // Resolve the uploader's own card by NAME first (same
                // pattern as DownloadDone): the shared download_entry_index
                // can be clobbered while the async upload task is in flight
                // by a remote FileShare (set_pending_file), a user-initiated
                // ExecuteDownload, or a room switch — any of which would
                // otherwise leave the uploader's own card without its
                // thumbnail (VID-02).
                let upload_idx = resolve_upload_card_index(
                    &self.entries,
                    &name,
                    self.download_entry_index,
                );
                if let Some(idx) = upload_idx {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(dl) = entry.download.as_mut() {
                            dl.ticket = ticket.clone();
                            // VIDCARD-fix: the uploader's own card was created
                            // with an empty ticket (ExecuteFileSend), so
                            // expected_content_hash parsed at construction was
                            // None. Re-derive it from the real ticket now —
                            // otherwise PlayInlineVideo bails with "content
                            // identity is missing" for the sender's own
                            // uploads.
                            dl.expected_content_hash = content_hash_from_ticket(&ticket);
                            dl.thumbnail = thumbnail.clone();
                            dl.thumbnail_handle = thumbnail.as_deref().map(|bytes| {
                                iced::widget::image::Handle::from_bytes(bytes.to_vec())
                            });
                            // The uploader card was created with `None`
                            // thumbnail, so poster_dimensions is still unset.
                            // Decode it from the returned poster bytes so the
                            // sender's own card gets the same ratio-exact
                            // frame as the receiver's ThumbnailFetched path.
                            dl.poster_dimensions = thumbnail.as_deref().and_then(|bytes| {
                                image::ImageReader::new(std::io::Cursor::new(bytes))
                                    .with_guessed_format()
                                    .ok()
                                    .and_then(|reader| reader.into_dimensions().ok())
                            });
                            dl.state = DownloadState::Shared {
                                name: name.clone(),
                                path: std::path::PathBuf::from(local_path.unwrap_or_default()),
                                size: None,
                            };
                        }
                        entry.body = format!("Shared: {name} ✓");
                        self.layout_cache.borrow_mut().invalidate_from(idx);
                    }
                }
                // Also set pending_file so the file can be re-downloaded.
                self.pending_file = Some((name, ticket));
                iced::Task::none()
            }
            AppMessage::ThumbnailFetched {
                entry_index,
                thumbnail_bytes,
            } => {
                if !thumbnail_bytes.is_empty() {
                    // VIDCARD-18 guardrail: read the decoded poster dimensions
                    // BEFORE handing the bytes to the image decoder, and
                    // reject anything outside the accepted bounds. The
                    // sender's poster is generated at MAX_POSTER_EDGE
                    // (320 px); a hostile sender must not be able to force a
                    // large surface allocation through the preview path.
                    // Rejection keeps the file-type placeholder in place and
                    // clears the pending hash so the label stays truthful.
                    let dimensions =
                        image::ImageReader::new(std::io::Cursor::new(&thumbnail_bytes))
                            .with_guessed_format()
                            .ok()
                            .and_then(|reader| reader.into_dimensions().ok());
                    if !video_poster::dimensions_within_bounds(dimensions) {
                        tracing::warn!(
                            entry_index,
                            ?dimensions,
                            "video thumbnail rejected: decoded dimensions outside poster bounds; keeping placeholder"
                        );
                        if let Some(entry) = self.entries.get_mut(entry_index) {
                            if let Some(dl) = entry.download.as_mut() {
                                dl.thumbnail_hash = None;
                            }
                        }
                        return iced::Task::none();
                    }
                    if let Some(entry) = self.entries.get_mut(entry_index) {
                        if let Some(dl) = entry.download.as_mut() {
                            dl.thumbnail = Some(thumbnail_bytes.clone());
                            dl.thumbnail_handle =
                                Some(iced::widget::image::Handle::from_bytes(thumbnail_bytes));
                            dl.poster_dimensions = dimensions;
                        }
                        self.layout_cache.borrow_mut().invalidate_from(entry_index);
                    }
                }
                iced::Task::none()
            }
            AppMessage::CopyShareTicket(entry_index) => {
                self.video_card_menu_open = None;
                if let Some(entry) = self.entries.get(entry_index) {
                    if let Some(dl) = &entry.download {
                        // Re-serialize the ticket from the local endpoint so
                        // the recipient fetches from this node (which hosts
                        // the blob after send or download).  If the stored
                        // ticket cannot be parsed, fall back to copying the
                        // raw string — it still grants access to the blob.
                        let ticket_str = match dl.ticket.parse::<iroh_blobs::ticket::BlobTicket>() {
                            Ok(t) => {
                                let (_, hash, format) = t.into_parts();
                                boru_core::ticket_share::make_share_ticket(
                                    self.endpoint.addr(),
                                    hash,
                                    format,
                                    boru_core::ticket_share::AddrInfoOptions::RelayAndAddresses,
                                )
                            }
                            Err(_) => dl.ticket.clone(),
                        };
                        self.toast_message = Some("Share ticket copied to clipboard".to_string());
                        return iced::clipboard::write(ticket_str);
                    }
                }
                iced::Task::none()
            }

            AppMessage::OpenReceiveTicketDialog => {
                self.show_receive_ticket_dialog = true;
                self.receive_ticket_input = String::new();
                self.receive_ticket_preflight = None;
                self.receive_ticket_error = None;
                self.receive_ticket_preflight_busy = false;
                self.receive_ticket_downloading = false;
                iced::Task::none()
            }

            AppMessage::CloseReceiveTicketDialog => {
                self.show_receive_ticket_dialog = false;
                self.receive_ticket_input = String::new();
                self.receive_ticket_preflight = None;
                self.receive_ticket_error = None;
                self.receive_ticket_preflight_busy = false;
                iced::Task::none()
            }

            AppMessage::ReceiveTicketInputChanged(text) => {
                self.receive_ticket_input = text;
                self.receive_ticket_preflight = None;
                self.receive_ticket_error = None;
                iced::Task::none()
            }

            AppMessage::ReceiveTicketPreflight => {
                let input = self.receive_ticket_input.trim().to_string();
                if input.is_empty() {
                    self.receive_ticket_error = Some("Paste a share ticket first.".to_string());
                    return iced::Task::none();
                }
                self.receive_ticket_preflight_busy = true;
                self.receive_ticket_error = None;
                let endpoint = self.endpoint.clone();
                iced::Task::perform(
                    async move {
                        let ticket: iroh_blobs::ticket::BlobTicket = input
                            .parse()
                            .map_err(|e| format!("Invalid share ticket: {e}"))?;
                        let preflight = boru_core::ticket_share::preflight_ticket(&endpoint, &ticket)
                            .await
                            .map_err(|e| format!("Pre-flight failed: {e}"))?;
                        Ok::<_, String>(ReceiveTicketPreflight {
                            ticket: input,
                            content_hash: preflight.hash.to_hex().to_string(),
                            node_short: preflight.node_id.fmt_short().to_string(),
                            total_size: preflight.total_size,
                            is_collection: preflight.format == iroh_blobs::BlobFormat::HashSeq,
                            child_count: preflight.child_count,
                        })
                    },
                    AppMessage::ReceiveTicketPreflightDone,
                )
            }

            AppMessage::ReceiveTicketPreflightDone(result) => {
                self.receive_ticket_preflight_busy = false;
                match result {
                    Ok(preflight) => {
                        self.receive_ticket_preflight = Some(preflight);
                        self.receive_ticket_error = None;
                    }
                    Err(e) => {
                        self.receive_ticket_preflight = None;
                        self.receive_ticket_error = Some(e);
                    }
                }
                iced::Task::none()
            }

            AppMessage::ConfirmReceiveTicket => {
                let Some(preflight) = self.receive_ticket_preflight.clone() else {
                    return iced::Task::none();
                };
                if self.receive_ticket_downloading {
                    return iced::Task::none();
                }
                if preflight.is_collection {
                    self.receive_ticket_error = Some(
                        "Folder tickets are not supported yet — use a single-file ticket.".to_string(),
                    );
                    return iced::Task::none();
                }
                // Close the dialog and start the download through the
                // existing download machinery into a safe destination.
                self.show_receive_ticket_dialog = false;
                self.receive_ticket_downloading = true;
                let name = format!("ticket-{}", &preflight.content_hash[..8]);
                let ticket_str = preflight.ticket.clone();
                let node_short = preflight.node_short.clone();
                let total_size = preflight.total_size;

                // Create the chat download card entry (same shape as a
                // received FileShare card) so progress renders normally.
                self.download_entry_index = Some(self.entries.len());
                self.entries_push(ChatEntry::system_download(
                    format!("Receiving from ticket: {name}"),
                    TransferKind::File,
                    name.clone(),
                    ticket_str.clone(),
                    &node_short,
                    None,
                ));
                if let Some(idx) = self.download_entry_index {
                    if let Some(entry) = self.entries.get_mut(idx) {
                        if let Some(dl) = entry.download.as_mut() {
                            dl.state = DownloadState::Active {
                                bytes: 0,
                                total: Some(total_size),
                            };
                        }
                    }
                }

                let blob_store = self.blob_store.clone();
                let endpoint = self.endpoint.clone();
                let neighbors = self.neighbors.clone();
                let dl_dir = self.boru_downloads_dir.clone();
                let progress_queue = self.download_progress_queue.clone();
                iced::Task::perform(
                    async move {
                        let ticket: iroh_blobs::ticket::BlobTicket = ticket_str
                            .parse()
                            .map_err(|e| format!("Invalid share ticket: {e}"))?;
                        let (addr, hash, _format) = ticket.into_parts();
                        let node_id = addr.id;
                        let candidates = download_candidates(node_id, &neighbors);
                        let _ = tokio::fs::create_dir_all(&dl_dir).await;
                        // BORU-AUDIT-21: reserve atomically instead of
                        // checking a path and reopening it later.
                        let mut destination = match boru_core::safe_destination::reserve_download_destination(
                            &dl_dir,
                            &name,
                            &preflight.content_hash,
                            boru_core::safe_destination::OverwritePolicy::KeepBoth,
                        )
                        .map_err(|e| format!("Unsafe download name: {e}"))?
                        {
                            boru_core::safe_destination::Reservation::Use(dest) => dest,
                            boru_core::safe_destination::Reservation::Skip => {
                                return Err("Download skipped: destination name already exists".into());
                            }
                        };
                        download_blob_to_file(
                            &blob_store,
                            &endpoint,
                            hash,
                            candidates,
                            name.clone(),
                            TransferKind::File,
                            &mut destination,
                            Some(&preflight.content_hash),
                            {
                                let queue = progress_queue.clone();
                                move |ev| {
                                    if let Ok(mut q) = queue.lock() {
                                        q.push_back(ev);
                                    }
                                }
                            },
                            Some(total_size),
                        )
                        .await
                        .map_err(|e| format!("Download failed: {e}"))?;
                        let save_path = destination
                            .publish()
                            .map_err(|e| format!("Publish failed: {e}"))?;
                        Ok::<_, String>((name, save_path))
                    },
                    move |r| match r {
                        Ok((name, path)) => {
                            AppMessage::DownloadDone(name, path)
                        }
                        Err(e) => AppMessage::DownloadFailed(e),
                    },
                )
            }
            AppMessage::SharedByMeToggleShareMenu => {
                self.shared_by_me_ui.toggle_share_menu();
                iced::Task::none()
            }

            AppMessage::AddSharedFile => {
                // Open the file picker — map result to SharedFilePicked(path) or Noop.
                // Cancel is a no-op, never an error.
                iced::Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Select a file to share")
                        .pick_file(),
                    |file| {
                        if let Some(file) = file {
                            AppMessage::SharedFilePicked(file.path().to_string_lossy().to_string())
                        } else {
                            AppMessage::Noop
                        }
                    },
                )
            }

            AppMessage::AddSharedFolder => {
                // Native OS folder picker (FS-10). Cancel is a no-op.
                iced::Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Select a folder to share")
                        .pick_folder(),
                    |folder| {
                        if let Some(folder) = folder {
                            AppMessage::SharedFolderPicked(
                                folder.path().to_string_lossy().to_string(),
                            )
                        } else {
                            AppMessage::Noop
                        }
                    },
                )
            }

            AppMessage::SharedFolderPicked(path) => {
                self.shared_by_me_ui.close_share_menu();
                if path.is_empty() {
                    return iced::Task::none();
                }
                // The secure catalogue is file-based (content-addressed
                // `file_objects` + `shared_files`): there is no folder object
                // in the schema, so a directory cannot enter the signed,
                // authorized share flow without inventing a second sharing
                // subsystem. Make the limitation explicit instead of silently
                // flattening the folder or faking a row. Only the display name
                // is used — never the full local path.
                let name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("folder")
                    .to_string();
                self.push_system(format!(
                    "“{name}” can't be shared as a folder yet — the secure catalogue shares \
                     individual files. Select files inside it instead."
                ));
                iced::Task::none()
            }

            AppMessage::SharedFilePicked(path) => {
                if path.is_empty() {
                    return iced::Task::none();
                }
                // Nonblocking progress: show only the display name while the
                // file is read/hashed off the UI thread.
                let display_name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file")
                    .to_string();
                self.shared_by_me_ui.sharing_status =
                    Some(format!("Registering {display_name}…"));
                self.shared_by_me_ui.close_share_menu();
                // Clone needed resources for the async task
                let storage = self.storage.clone();
                let user_id = self.local_public.to_string();
                iced::Task::perform(
                    async move {
                        let stg = match storage {
                            Some(ref stg) => stg.clone(),
                            None => return Err("Storage is not available".to_string()),
                        };
                        // Read file on blocking thread
                        let abs_path = std::path::PathBuf::from(&path);
                        let (file_data, metadata) = tokio::task::spawn_blocking({
                            let path = abs_path.clone();
                            move || {
                                let meta = std::fs::metadata(&path)
                                    .map_err(|e| format!("Cannot read file: {e}"))?;
                                let data = std::fs::read(&path)
                                    .map_err(|e| format!("Cannot read file: {e}"))?;
                                Ok::<_, String>((data, meta))
                            }
                        })
                        .await
                        .map_err(|e| format!("Task join error: {e}"))??;

                        let filename = abs_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let size = metadata.len();
                        // Compute blake3 content hash
                        let hash = blake3::hash(&file_data);
                        let hash_hex = hash.to_hex().to_string();

                        // Compute metadata_id (same as SharedFile::new does)
                        let modified_time =
                            metadata.modified().unwrap_or(std::time::SystemTime::now());
                        let ts = modified_time
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let mut meta_hasher = blake3::Hasher::new();
                        meta_hasher.update(filename.as_bytes());
                        meta_hasher.update(&size.to_le_bytes());
                        meta_hasher.update(&ts.to_le_bytes());
                        let metadata_id = meta_hasher.finalize().to_hex().to_string();

                        // Detect MIME type from file extension.  PAPIRUS-21:
                        // cover the extensions the central resolver knows so
                        // files shared through the UI are stored with a real
                        // MIME and show the same type icon everywhere (the
                        // MIME strings below are the exact ones
                        // file_type_resolver.rs maps).  Unknown extensions
                        // keep the octet-stream placeholder, which the
                        // resolver treats as "no MIME info" and falls back
                        // to the filename extension.
                        let mime_type = match abs_path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                            .as_str()
                        {
                            // Documents
                            "txt" => "text/plain",
                            "md" | "markdown" => "text/markdown",
                            "log" => "text/x-log",
                            "pdf" => "application/pdf",
                            "doc" => "application/msword",
                            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                            "odt" => "application/vnd.oasis.opendocument.text",
                            "rtf" => "application/rtf",
                            // Spreadsheets
                            "xls" => "application/vnd.ms-excel",
                            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                            "ods" => "application/vnd.oasis.opendocument.spreadsheet",
                            "csv" => "text/csv",
                            "tsv" => "text/tab-separated-values",
                            // Presentations
                            "ppt" => "application/vnd.ms-powerpoint",
                            "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                            "odp" => "application/vnd.oasis.opendocument.presentation",
                            // Images
                            "png" => "image/png",
                            "jpg" | "jpeg" => "image/jpeg",
                            "gif" => "image/gif",
                            "webp" => "image/webp",
                            "svg" => "image/svg+xml",
                            "bmp" => "image/bmp",
                            "tiff" | "tif" => "image/tiff",
                            // Video
                            "mp4" | "m4v" => "video/mp4",
                            "webm" => "video/webm",
                            "mkv" => "video/x-matroska",
                            "avi" => "video/x-msvideo",
                            "mov" => "video/quicktime",
                            "mpeg" | "mpg" => "video/mpeg",
                            "ogv" => "video/ogg",
                            // Audio
                            "mp3" => "audio/mpeg",
                            "flac" => "audio/flac",
                            "wav" => "audio/x-wav",
                            "ogg" => "audio/ogg",
                            "m4a" => "audio/x-m4a",
                            "wma" => "audio/x-ms-wma",
                            "opus" => "audio/opus",
                            "aac" => "audio/aac",
                            // Archives
                            "zip" => "application/zip",
                            "7z" => "application/x-7z-compressed",
                            "rar" => "application/vnd.rar",
                            "tar" => "application/x-tar",
                            "gz" | "gzip" => "application/gzip",
                            "bz2" => "application/x-bzip2",
                            "xz" => "application/x-xz-compressed-tar",
                            "zst" => "application/zstd",
                            // Source code
                            "rs" => "text/x-rust",
                            "py" => "text/x-python",
                            "js" => "application/javascript",
                            "ts" => "text/typescript",
                            "html" | "htm" => "text/html",
                            "css" => "text/css",
                            "json" => "application/json",
                            "xml" => "application/xml",
                            "yaml" | "yml" => "application/yaml",
                            "toml" => "application/toml",
                            "sh" | "bash" | "zsh" | "fish" => "text/x-shellscript",
                            "ps1" | "psm1" | "psd1" => "text/x-powershell",
                            "java" => "text/x-java",
                            "kt" | "kts" => "text/x-kotlin",
                            "c" => "text/x-c",
                            "cpp" | "cc" | "cxx" => "text/x-c++",
                            "cs" => "text/x-csharp",
                            "go" => "text/x-go",
                            "sql" => "application/sql",
                            // Executables / installers / disk images
                            "exe" | "bat" | "cmd" => "application/x-ms-dos-executable",
                            "elf" | "so" | "dll" | "dylib" => "application/x-executable",
                            "deb" => "application/vnd.debian.binary-package",
                            "rpm" => "application/x-rpm",
                            "apk" => "application/vnd.android.package-archive",
                            "msi" => "application/x-msi",
                            "iso" => "application/x-cd-image",
                            "img" => "application/x-raw-disk-image",
                            "dmg" => "application/x-apple-diskimage",
                            // Databases / fonts / keys
                            "sqlite" | "sqlite3" | "db" | "dbf" => "application/x-sqlite3",
                            "ttf" | "otf" | "ttc" => "application/x-font-ttf",
                            "woff" | "woff2" => "font/woff",
                            "pem" => "application/x-pem-key",
                            "key" | "pub" | "asc" | "gpg" | "ppk" => "application/pgp-keys",
                            // Ebooks / torrents / 3D
                            "epub" | "mobi" => "application/epub+zip",
                            "torrent" => "application/x-bittorrent",
                            "stl" | "obj" | "fbx" | "glb" | "gltf" | "blend" | "3ds" => "model/stl",
                            _ => "application/octet-stream",
                        };

                        // Store file object + source path + shared file entry
                        stg.put_file_object(&hash_hex, size, mime_type, &filename, &file_data)
                            .map_err(|e| format!("Failed to store file: {e}"))?;
                        stg.set_file_object_source_path(&hash_hex, Some(&path))
                            .map_err(|e| format!("Failed to set source path: {e}"))?;
                        stg.upsert_shared_file(
                            &hash_hex,
                            &user_id,
                            &metadata_id,
                            &filename,
                            None,
                            true,
                        )
                        .map_err(|e| format!("Failed to register shared file: {e}"))?;

                        Ok(format!("Shared file added: {filename} ({} bytes)", size))
                    },
                    |result: Result<String, String>| match result {
                        Ok(msg) => AppMessage::SharedFileAdded(msg),
                        Err(e) => AppMessage::SharedFileAddFailed(e),
                    },
                )
            }

            AppMessage::SharedFileAddFailed(msg) => {
                self.shared_by_me_ui.sharing_status = None;
                self.shared_by_me_ui.close_share_menu();
                self.push_system(msg);
                iced::Task::none()
            }

            AppMessage::SharedFileAdded(msg) => {
                self.shared_by_me_ui.sharing_status = None;
                self.shared_by_me_ui.close_share_menu();
                // Complete a GUI test share-file action once the file has been
                // registered through the normal sharing path.
                if let Some(action_id) = self.pending_share_file_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                self.push_system(msg);
                // Refresh the shared files list
                if let Some(ref stg) = self.storage {
                    if let Ok(rows) = stg.list_shared_files(&self.local_public.to_string(), true) {
                        self.shared_files = rows;
                    }
                }
                // The dashboard table must update from the authoritative
                // projection, not a manually inserted row.
                self.shared_by_me_loading = true;
                self.refresh_shared_by_me()
            }

            AppMessage::RemoveSharedFile(hash) => {
                if let Some(ref stg) = self.storage {
                    let user_id = self.local_public.to_string();
                    match stg.delete_shared_file(&hash, &user_id) {
                        Ok(true) => {
                            // Refresh the shared files list
                            if let Ok(rows) =
                                stg.list_shared_files(&self.local_public.to_string(), true)
                            {
                                self.shared_files = rows;
                            }
                            return iced::Task::done(AppMessage::SharedFileRemoved(
                                "Shared file removed.".to_string(),
                            ));
                        }
                        Ok(false) => {
                            return iced::Task::done(AppMessage::ErrorMsg(
                                "Shared file not found.".to_string(),
                            ));
                        }
                        Err(e) => {
                            return iced::Task::done(AppMessage::ErrorMsg(format!(
                                "Failed to remove shared file: {e}"
                            )));
                        }
                    }
                }
                iced::Task::done(AppMessage::ErrorMsg(
                    "Storage is not available.".to_string(),
                ))
            }

            AppMessage::SharedFileRemoved(msg) => {
                self.push_system(msg);
                iced::Task::none()
            }
            AppMessage::RequestFileDownload { peer, file } => {
                // Transition to AppMessageHandled if initiated by a GUI test action.
                // Keep the action_id stored — it's consumed when the download
                // completes (DownloadDonePeerFile or DownloadFailed).
                if let Some(ref action_id) = self.pending_download_action {
                    let _ = self
                        .gui_action_history
                        .set_state(action_id, GuiActionState::AppMessageHandled);
                }
                let display_name = file.display_name.clone();
                let content_hash = file.content_hash.clone();
                // FS-20 security hardening: backend-authoritative gate. The
                // verified, stored catalogue decides whether this download may
                // start — not UI state. A stale in-memory row for a file the
                // backend no longer advertises is refused with the backend's
                // reason. (When storage is unavailable the legacy behaviour is
                // preserved; there is no backend state to enforce.)
                if let Some(storage) = self.storage.as_ref() {
                    if let Err(e) = boru_core::download_initiation::validate_download_request(
                        storage,
                        &file.content_hash,
                        &peer.to_string(),
                    ) {
                        self.catalogue_downloads.insert(
                            content_hash,
                            CatalogueDownloadState::Failed(e.to_string()),
                        );
                        self.push_system(format!("Download blocked: {e}"));
                        return iced::Task::none();
                    }
                }
                self.catalogue_downloads
                    .insert(content_hash.clone(), CatalogueDownloadState::Pending);
                // Clone the shared state needed for the async download task.
                let endpoint = self.endpoint.clone();
                let blob_store = self.blob_store.clone();
                let neighbors = self.neighbors.clone();
                let dl_dir = self.boru_downloads_dir.clone();
                let progress_queue = self.download_progress_queue.clone();
                let dn_for_err = display_name.clone();
                let content_hash = file.content_hash.clone();
                let size_bytes = file.size_bytes;
                // If size_bytes is 0 (unknown/uncached metadata), pass None
                // to avoid an immediate "blob too large" failure on the first byte.
                let max_bytes = if size_bytes > 0 {
                    Some(size_bytes)
                } else {
                    None
                };
                iced::Task::perform(
                    async move {
                        let _ = tokio::fs::create_dir_all(&dl_dir).await;
                        let hash: iroh_blobs::Hash = content_hash
                            .parse()
                            .map_err(|e| format!("Invalid content hash: {e}"))?;
                        let candidates =
                            boru_core::chat_core::download_candidates(peer, &neighbors);
                        // FS-20 hardening: derive the destination through the
                        // shared safe-destination helper (strip separators,
                        // reject traversal) even though the catalogue
                        // validation already rejects unsafe names.  Falls
                        // back to a content-hash stem when the display name
                        // is empty or reserved.  BORU-AUDIT-21: reservation
                        // fuses validation + atomic creation (O_EXCL).
                        let mut destination = match boru_core::safe_destination::reserve_download_destination(
                            &dl_dir,
                            &display_name,
                            &content_hash,
                            boru_core::safe_destination::OverwritePolicy::KeepBoth,
                        )
                        .map_err(|e| format!("Unsafe download name: {e}"))?
                        {
                            boru_core::safe_destination::Reservation::Use(dest) => dest,
                            boru_core::safe_destination::Reservation::Skip => {
                                return Err("Download skipped: destination name already exists".into());
                            }
                        };
                        let kind = boru_core::chat_callbacks::TransferKind::File;
                        download_blob_to_file(
                            &blob_store,
                            &endpoint,
                            hash,
                            candidates,
                            display_name.clone(),
                            kind,
                            &mut destination,
                            Some(&content_hash),
                            {
                                let queue = progress_queue.clone();
                                move |ev| {
                                    if let Ok(mut q) = queue.lock() {
                                        q.push_back(ev);
                                    }
                                }
                            },
                            max_bytes,
                        )
                        .await
                        .map_err(|e| format!("Download failed: {e}"))?;
                        let save_path = destination
                            .publish()
                            .map_err(|e| format!("Publish failed: {e}"))?;
                        Ok::<_, String>((display_name, save_path))
                    },
                    move |r| match r {
                        Ok((name, path)) => AppMessage::DownloadDonePeerFile(name, path),
                        Err(e) => AppMessage::DownloadFailed(format!("{} : {e}", dn_for_err)),
                    },
                )
            }
            // update() only dispatches the files variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
