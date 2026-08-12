//! Discover / public chats + peer & friend profile features.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the Discover screen
//! (public room directory), the per-peer catalogue, and the Peer /
//! Friend profile screens: the `impl IcedChat` methods that build and
//! render them. Dependency snapshot structs remain in app.rs for now.
//! Reads app state via `use super::*`; app.rs re-exports the pub(crate)
//! items it still references with `use discover::*`.

use super::*;

impl IcedChat {
    pub(crate) fn view_peer_profile(&self, peer: PublicKey) -> iced::Element<'_, AppMessage> {
        let profile_data = self.profile_cache.get(&peer);
        let display_name = profile_data
            .as_ref()
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| "Unknown Peer".to_string());
        let dep = PeerProfileDependency {
            dark_mode: self.dark_mode,
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
        self.peer_catalogue_view
            .as_ref()
            .and_then(|(_, files)| files.iter().find(|f| f.display_name == name))
            .map(|f| f.content_hash.clone())
    }

    pub(crate) fn view_peer_catalogue(&self, peer: PublicKey) -> iced::Element<'_, AppMessage> {
        let dep = self.peer_catalogue_dependency(peer);
        iced::widget::lazy(dep, move |dep| Self::view_peer_catalogue_content(dep, peer)).into()
    }

    /// Build the Hash-compatible snapshot the Peer Catalogue renders from.
    pub(crate) fn peer_catalogue_dependency(&self, peer: PublicKey) -> PeerCatalogueDependency {
        let display_name = self
            .names
            .get(&peer)
            .cloned()
            .unwrap_or_else(|| "Unknown Peer".to_string());
        let rows = match self.peer_catalogue_view.as_ref() {
            Some((pk, files)) if *pk == peer => files
                .iter()
                .map(|file| {
                    let dl = self
                        .catalogue_downloads
                        .get(&file.content_hash)
                        .map(CatalogueDownloadSnapshot::from)
                        .unwrap_or(CatalogueDownloadSnapshot::None);
                    let is_pending = self
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
            peer,
            display_name,
            catalogue_loading: self.catalogue_loading,
            rows,
            catalogue_scroll_offset_bits: (self.catalogue_scroll_offset.max(0.0) * 100.0) as u32,
            catalogue_viewport_height_bits: (self.catalogue_viewport_height.max(0.0) * 100.0)
                as u32,
        }
    }

    /// Static renderer for the Peer Catalogue screen. Reads only from the
    /// Hash-compatible [`PeerCatalogueDependency`] snapshot.
    pub(crate) fn view_peer_catalogue_content(
        dep: &PeerCatalogueDependency,
        peer: PublicKey,
    ) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, scrollable, space, Column, Row, Space};
        use iced::{Alignment, Color, Length};

        const CATALOGUE_ROW_HEIGHT: f32 = 52.0;
        const OVERSCAN: f32 = 800.0;

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
                        _ => Color::from_rgb(0.4, 0.4, 0.4),
                    };
                    let bg = match status {
                        iced::widget::button::Status::Hovered => Some(iced::Background::Color(
                            Color::from_rgba(0.3, 0.3, 0.3, 0.06),
                        )),
                        iced::widget::button::Status::Pressed => Some(iced::Background::Color(
                            Color::from_rgba(0.3, 0.3, 0.3, 0.12),
                        )),
                        _ => None,
                    };
                    iced::widget::button::Style {
                        text_color: base,
                        background: bg,
                        border: iced::Border {
                            color: border_muted(theme),
                            width: 1.0,
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
                let total_h = files.len() as f32 * CATALOGUE_ROW_HEIGHT;
                let catalogue_scroll_offset = dep.catalogue_scroll_offset_bits as f32 / 100.0;
                let catalogue_viewport_height = dep.catalogue_viewport_height_bits as f32 / 100.0;

                // ── Window calculation (only when viewport is known) ──
                if catalogue_viewport_height > 0.0 && total_h > 0.0 {
                    let so = catalogue_scroll_offset.max(0.0);
                    let view_top = so;
                    let view_bot = so + catalogue_viewport_height.max(200.0);

                    let range_top = (view_top - OVERSCAN).max(0.0);
                    let range_bot = (view_bot + OVERSCAN).min(total_h);

                    let first_idx = (range_top / CATALOGUE_ROW_HEIGHT) as usize;
                    let mut last_idx = (range_bot / CATALOGUE_ROW_HEIGHT) as usize;

                    if last_idx >= files.len() {
                        last_idx = files.len().saturating_sub(1);
                    }
                    if last_idx < first_idx {
                        last_idx = first_idx;
                    }

                    let top_space_h = first_idx as f32 * CATALOGUE_ROW_HEIGHT;
                    let bottom_start = (last_idx + 1) as f32 * CATALOGUE_ROW_HEIGHT;
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
                        file_rows = file_rows.push(Self::render_catalogue_row(row, dep.dark_mode, peer));
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
                    let bottom_h = (total_h - initial_count as f32 * CATALOGUE_ROW_HEIGHT).max(0.0);

                    for row in &files[..initial_count] {
                        file_rows = file_rows.push(Self::render_catalogue_row(row, dep.dark_mode, peer));
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
                                    .length(Length::Fixed(80.0))
                                    .girth(Length::Fixed(6.0)),
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
    pub(crate) fn discover_dependency(&self) -> DiscoverDependency {
        let ads: Vec<(RoomAdvertisement, PublicKey)> = {
            let store = self.directory_store.lock().unwrap();
            let mut list = store.list_active();
            list.sort_by(|(a, _), (b, _)| b.last_activity.cmp(&a.last_activity));
            list
        };
        DiscoverDependency {
            dark_mode: self.dark_mode,
            ads,
        }
    }

    /// Static renderer for the Discover screen, driven by [`DiscoverDependency`].
    pub(crate) fn view_discover_content(dep: &DiscoverDependency) -> iced::Element<'static, AppMessage> {
        use iced::widget::{button, container, text, Column, Row, Space};
        use iced::{Alignment, Background, Length};

        let header = Row::new()
            .push(
                button(
                    Row::new()
                        .push(icon_svg(ICON_CHAT, TYPO_SM))
                        .push(text(" Back").size(TYPO_SM))
                        .spacing(SPACE_4)
                        .align_y(Alignment::Center),
                )
                .on_press(AppMessage::CloseDiscover)
                .padding([SPACE_6, SPACE_12])
                .style(BUTTON_GHOST_BG),
            )
            .push(text("Public Rooms").size(TYPO_LG))
            .spacing(SPACE_8)
            .align_y(Alignment::Center);

        let mut main_content = Column::new().spacing(SPACE_8).padding(SPACE_16);

        let ads = &dep.ads;

        if ads.is_empty() {
            main_content = main_content.push(
                container(
                    Column::new()
                        .push(
                            text("No public rooms discovered yet.")
                                .size(TYPO_MD)
                                .style(text_muted_style),
                        )
                        .push(Space::new().height(SPACE_8))
                        .push(
                            text("Rooms advertised on your relay will appear here.")
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
            let theme = Self::theme_from_dark(dep.dark_mode);
            for (ad, _author) in ads {
                let theme = theme.clone();
                let ad_for_join = ad.clone();
                let room_name = ad.room_name.clone();
                let member_count = ad.member_count;
                let desc = if ad.description.len() > 100 {
                    format!("{}…", &ad.description[..100])
                } else {
                    ad.description.clone()
                };
                let last_active = crate::presentation::relative_time(ad.last_activity);

                let room_card = container(
                    Row::new()
                        .push(
                            Column::new()
                                .push(text(room_name).size(TYPO_MD))
                                .push(text(desc).size(TYPO_SM).style(text_muted_style))
                                .push(
                                    Row::new()
                                        .push(
                                            text(format!("{} members", member_count))
                                                .size(TYPO_XS)
                                                .style(text_muted_style),
                                        )
                                        .push(
                                            text(last_active).size(TYPO_XS).style(text_muted_style),
                                        )
                                        .spacing(SPACE_12),
                                )
                                .spacing(SPACE_4)
                                .width(Length::Fill),
                        )
                        .push(
                            button(text("Join").size(TYPO_SM))
                                .on_press(AppMessage::DirectoryRoomJoin(ad_for_join))
                                .padding([SPACE_6, SPACE_12])
                                .style(BUTTON_PRIMARY),
                        )
                        .spacing(SPACE_12)
                        .align_y(Alignment::Center),
                )
                .padding(SPACE_12)
                .width(Length::Fill)
                .style(move |t| container::Style {
                    background: Some(Background::Color(bg_surface(t))),
                    border: iced::Border {
                        radius: SPACE_8.into(),
                        color: border_muted(&theme),
                        width: 1.0,
                    },
                    ..Default::default()
                });
                main_content = main_content.push(room_card);
            }
        }

        let body = Column::new()
            .push(header)
            .push(
                crate::ui_components::gutter_scrollable(main_content)
                    .height(Length::Fill)
                    .width(Length::Fill),
            )
            .spacing(SPACE_8);

        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(container_primary)
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
                .width(Length::Fixed(200.0));

            for (label, msg) in &menu_items {
                let is_destructive = *label == "Remove Friend" || *label == "Block Friend";
                let item = button(text(*label).size(TYPO_SM).color(if is_destructive {
                    Color::from_rgb(0.8, 0.2, 0.2)
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
                .style(move |t| iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg_surface(t))),
                    border: iced::Border {
                        color: border_muted(t),
                        width: 1.0,
                        radius: SPACE_8.into(),
                    },
                    ..Default::default()
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
        if self.share_local_service_open {
            return self.view_share_local_service_dialog(peer, display_name.clone(), base);
        }

        // ── Toast overlay ──
        if let Some(msg) = &self.toast_message {
            // FONTS-15: the toast text now renders in the wider IBM Plex Sans
            // default font, so long messages (e.g. "Alice shared a very long
            // tunnel service name with you (2h)") can exceed the window on
            // narrow layouts. Cap the toast width and let the text wrap
            // instead of spilling past the window edge.
            let toast = container(
                text(msg)
                    .size(TYPO_SM)
                    .color(Color::WHITE)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            )
            .max_width(480.0)
            .padding(iced::Padding {
                top: SPACE_8,
                right: SPACE_16,
                bottom: SPACE_8,
                left: SPACE_16,
            })
            .style(move |t| iced::widget::container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.1, 0.1, 0.1, 0.85,
                ))),
                border: iced::Border {
                    radius: SPACE_8.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

            return iced::widget::stack![
                base,
                container(toast)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(iced::Padding {
                        top: 16.0,
                        right: 0.0,
                        bottom: 0.0,
                        left: 0.0,
                    }),
            ]
            .into();
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
        let has_catalogue = self
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
                    text_input("Friend's name…", &dep.friend_profile_rename_input)
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
            .push(text("Shared Files").size(TYPO_SM).width(Length::Fill))
            .push(
                button(text("Browse").size(TYPO_XS))
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
                    text("Files available")
                        .size(TYPO_XS)
                        .style(text_muted_style),
                )
                .spacing(0)
        } else {
            row![]
                .push(
                    text("No shared files.")
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
        let recent_header = text("Recent Messages").size(TYPO_SM).width(Length::Fill);

        let recent_body: iced::Element<'static, AppMessage> = if recent_messages.is_empty() {
            text("No recent messages.")
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
                button(text("Message").size(TYPO_SM))
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
                button(text("Files").size(TYPO_SM))
                    .on_press(AppMessage::BrowsePeerCatalogue(peer))
                    .padding([SPACE_8, SPACE_16])
                    .width(Length::Fill)
                    .style(move |t, _status| iced::widget::button::Style {
                        background: Some(iced::Background::Color(bg_surface(t))),
                        text_color: text_remote_body(&Self::theme_from_dark(dark_mode)),
                        border: iced::Border {
                            color: border_muted(t),
                            width: 1.0,
                            radius: SPACE_6.into(),
                        },
                        ..Default::default()
                    }),
            )
            .push(
                button(text("Voice").size(TYPO_SM))
                    .padding([SPACE_8, SPACE_16])
                    .width(Length::Fill)
                    .style(move |t, _status| iced::widget::button::Style {
                        background: Some(iced::Background::Color(bg_surface(t))),
                        text_color: Self::muted_color(dark_mode),
                        border: iced::Border {
                            color: border_muted(t),
                            width: 1.0,
                            radius: SPACE_6.into(),
                        },
                        ..Default::default()
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
                    card = card.push(text("Expired").size(TYPO_XS).color(text_muted(&theme)));
                } else if state.connection_failed {
                    card = card.push(text("Failed").size(TYPO_XS).color(color_error(&theme)));
                } else if state.connected {
                    let route = state.route_label.as_deref();
                    card = card.push(
                        text(match route {
                            Some("Direct") => "Connected · Direct".to_string(),
                            Some("Relay") => "Connected · Relay".to_string(),
                            Some(other) if !other.is_empty() => format!("Connected · {other}"),
                            _ => "Connected".to_string(),
                        })
                        .size(TYPO_XS)
                        .color(accent_green(&theme)),
                    );
                }
                card = card.push(text(sharer_label).size(TYPO_XS).style(text_muted_style));
                card = card.push(text(service_name).size(TYPO_MD));

                if let Some(display) = &state.local_addr {
                    card = card.push(
                        text(format!("Available at: {display}"))
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
                        text("This shared service has expired.")
                            .size(TYPO_XS)
                            .style(text_muted_style),
                    );
                }

                let mut actions = row![].spacing(SPACE_6).align_y(Alignment::Center);
                if state.connected {
                    if is_http {
                        actions = actions.push(
                            button(text("Open").size(TYPO_XS))
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
                        button(text("Copy Address").size(TYPO_XS))
                            .on_press(AppMessage::CopyReceivedTunnelAddress(tunnel_id))
                            .padding([SPACE_2, SPACE_8]),
                    );
                    actions = actions.push(
                        button(text("Disconnect").size(TYPO_XS))
                            .on_press(AppMessage::DisconnectReceivedTunnel(tunnel_id))
                            .padding([SPACE_2, SPACE_8]),
                    );
                } else if !expired {
                    actions = actions.push(
                        button(text("Connect").size(TYPO_XS))
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
                    .push(text("Shared Services").size(TYPO_SM).width(Length::Fill))
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
                self.catalogue_loading = true;
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
                self.catalogue_loading = false;
                self.peer_catalogue_view = Some((peer, files));
                if !matches!(self.screen, Screen::PeerCatalogue(peer) | Screen::PeerProfile(peer)) {
                    self.peer_profile_return_to = Some(self.screen.clone());
                }
                self.screen = Screen::PeerCatalogue(peer);
                iced::Task::none()
            }
            AppMessage::PeerCatalogueFailed(error) => {
                self.catalogue_loading = false;
                self.push_system(format!("Catalogue fetch failed: {error}"));
                iced::Task::none()
            }
            AppMessage::CatalogueScrolled(offset, vp_h) => {
                self.catalogue_scroll_offset = offset;
                self.catalogue_viewport_height = vp_h;
                iced::Task::none()
            }


            AppMessage::ToggleAdvertiseRoom(topic) => {
                // Toggle advertising for this room.
                if self.advertised_rooms.contains(&topic) {
                    self.advertised_rooms.remove(&topic);
                    info!(%topic, "room advertising disabled");
                    iced::Task::none()
                } else {
                    self.advertised_rooms.insert(topic);
                    info!(%topic, "room advertising enabled");
                    let room_name = self
                        .conversation_store
                        .find(&topic)
                        .map(|e| {
                            if e.name.is_empty() {
                                topic.to_string()
                            } else {
                                e.name.clone()
                            }
                        })
                        .unwrap_or_else(|| topic.to_string());
                    // PUBLIC-02: making a room public is a local
                    // announcement — surface it in the Recent Activity feed.
                    self.push_activity(
                        format!("You announced public room \"{room_name}\""),
                        ActivityKind::Generic,
                    );
                    // Broadcast an immediate RoomAdvertisement so the room
                    // appears in the directory without waiting for the next
                    // ~60s periodic tick.
                    if let Some(ref dir_sender) = self.directory_sender {
                        let sk = self.secret_key.clone();
                        let s = dir_sender.clone();
                        let neighbor_count = self
                            .room_neighbor_counts
                            .get(&topic)
                            .copied()
                            .unwrap_or_default();
                        let ticket = self.room_ticket(topic, &[]).to_string();
                        iced::Task::perform(
                            async move {
                                let ad = boru_core::chat_core::RoomAdvertisement {
                                    room_name,
                                    description: String::new(),
                                    topic,
                                    ticket,
                                    member_count: neighbor_count,
                                    last_activity: std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis()
                                        as u64,
                                };
                                let ad_bytes = postcard::to_stdvec(&ad).unwrap_or_default();
                                let signature = sk.sign(&ad_bytes);
                                let msg = crate::Message::RoomAdvertisement {
                                    ad,
                                    signature: signature.to_bytes().to_vec(),
                                };
                                let ok = match SignedMessage::sign_and_encode(&sk, &msg) {
                                    Ok(encoded) => s.broadcast(encoded).await.is_ok(),
                                    Err(_) => false,
                                };
                                ok
                            },
                            |ok| {
                                if ok {
                                    tracing::debug!("immediate room advertisement broadcast");
                                } else {
                                    tracing::warn!("immediate room advertisement broadcast failed");
                                }
                                AppMessage::Noop
                            },
                        )
                    } else {
                        iced::Task::done(AppMessage::SubscribeDirectoryTopic)
                    }
                }
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


            AppMessage::DirectoryRoomJoin(ad) => {
                // Parse the ticket from the advertisement and open the room.
                match Ticket::from_str(&ad.ticket) {
                    Ok(ticket) => {
                        let topic = ticket.topic;
                        info!(topic = %topic, "joining room from directory");
                        iced::Task::done(AppMessage::OpenRoom(topic))
                    }
                    Err(e) => {
                        warn!("failed to parse directory room ticket: {e}");
                        self.push_system("Failed to join room: invalid ticket");
                        iced::Task::none()
                    }
                }
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
                    self.advertised_rooms.remove(&topic);
                    self.public_rooms_sidebar_revision =
                        self.public_rooms_sidebar_revision.wrapping_add(1);
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
                let profile_image_ticket = self.profile_image_ticket.clone();
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
                    |result| match result {
                        Ok((sender, topic)) => AppMessage::BackgroundSubscribed(
                            topic,
                            Some(sender),
                            Some(forward_handle_slot),
                        ),
                        Err(e) => {
                            let fallback_topic = TopicId::from_bytes([0u8; 32]);
                            warn!("BackgroundSubscribe failed: {e}");
                            AppMessage::BackgroundSubscribed(fallback_topic, None, None)
                        }
                    },
                )
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
                self.catalogue_loading = false;
                self.catalogue_error = Some(message);
                iced::Task::none()
            }
            AppMessage::CatalogueErrorDismissed => {
                self.catalogue_error = None;
                iced::Task::none()
            }
            // update() only dispatches the discover variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
