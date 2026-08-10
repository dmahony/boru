//! Chat (active room) feature.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the active-room chat
//! surface: the chat panel/header/footer, message log (with its
//! incremental layout cache), composer, emoji/gif pickers, search,
//! context menu, details panels and the help overlay — the
//! `impl IcedChat` methods that build and render them. Reads app state
//! via `use super::*`; app.rs re-exports the pub(crate) items it still
//! references with `use chat::*`.

use super::*;

impl IcedChat {
    pub(crate) fn view_chat_panel(&self) -> iced::Element<'_, AppMessage> {
        use iced::{widget, Length};

        // Show a loading spinner while the gossip subscription is in flight.
        if self.room_loading {
            const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let spinner = SPINNER_FRAMES[self.splash_spinner_frame % SPINNER_FRAMES.len()];
            let theme = self.theme();
            let dark_mode = self.theme() == iced::Theme::Dark;
            return widget::container(
                widget::column![
                    widget::text(spinner)
                        .size(40.0)
                        .color(accent_primary(&theme)),
                    widget::text("Loading conversation\u{2026}")
                        .size(crate::fonts::TypeRole::Body.size_px())
                        .font(crate::fonts::TypeRole::Body.font())
                        .color(Self::muted_color(dark_mode)),
                    widget::text("Setting up your conversation")
                        .size(crate::fonts::TypeRole::SupportingText.size_px())
                        .font(crate::fonts::TypeRole::SupportingText.font())
                        .color(Self::muted_color(dark_mode)),
                ]
                .spacing(SPACE_12)
                .align_x(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        }

        // Show a connecting animation when the subscription completed but the
        // gossip sender isn't available yet — the mesh peer hasn't connected.
        if self.sender.is_none() {
            const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let spinner = SPINNER_FRAMES[self.connecting_spinner_frame % SPINNER_FRAMES.len()];
            let theme = self.theme();
            let dark_mode = self.theme() == iced::Theme::Dark;
            return widget::container(
                widget::column![
                    widget::text(spinner)
                        .size(40.0)
                        .color(accent_primary(&theme)),
                    widget::text("Connecting…")
                        .size(crate::fonts::TypeRole::Body.size_px())
                        .font(crate::fonts::TypeRole::Body.font())
                        .color(Self::muted_color(dark_mode)),
                    widget::text("The conversation will be ready shortly")
                        .size(crate::fonts::TypeRole::SupportingText.size_px())
                        .font(crate::fonts::TypeRole::SupportingText.font())
                        .color(Self::muted_color(dark_mode)),
                ]
                .spacing(SPACE_8)
                .align_x(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        }

        // Keep the header and composer outside the scrollable message log so
        // navigation and sending remain available while reading history. A
        // subtle divider separates the header from the message region.
        //
        // The timeline is the ONLY vertically expanding region: it fills the
        // space between the fixed header and the pinned composer. It is wrapped
        // in a `responsive` widget so `view_chat_log` knows its exact region
        // height — iced only emits `Scrolled` events once content overflows,
        // so a short timeline would otherwise never learn its viewport size
        // and could not bottom-align its content (leaving a dead area below
        // the last message).
        //
        // The restrained footer/status line (plan UI-16) sits below the
        // composer, separated by a small gap, and reports complementary
        // route/peer state — the header already owns presence + encryption
        // (direct) or member count (group), so nothing is duplicated.
        let content = widget::column![
            self.view_chat_header(),
            divider(&self.theme()),
            widget::responsive(|size: iced::Size| {
                self.view_chat_log(size.width, size.height).into()
            }),
            self.view_composer(),
            widget::Space::new().height(Length::Fixed(SPACE_8)),
            self.view_chat_footer(),
        ]
        // Make the column itself participate in the parent height
        // negotiation. The responsive timeline can then consume exactly the
        // remaining space after the fixed header and composer have been
        // measured, instead of falling back to the column's intrinsic height.
        .spacing(0)
        .width(Length::Fill)
        .height(Length::Fill);

        let inner = widget::container(content)
            .padding(iced::Padding {
                top: 0.0,
                right: SPACE_16,
                bottom: SPACE_12,
                left: SPACE_16,
            })
            .width(Length::Fill)
            .height(Length::Fill);

        // ── Chat options popover overlay ────────────────────────────
        if self.show_chat_options {
            use iced::widget::Stack;
            use iced::Color;

            let backdrop = widget::button(widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .on_press(AppMessage::ToggleChatOptions)
                .style(move |t, _status| iced::widget::button::Style {
                    background: Some(iced::Background::Color(if matches!(t, iced::Theme::Dark) {
                        Color::from_rgba(0.0, 0.0, 0.0, 0.45)
                    } else {
                        Color::from_rgba(0.0, 0.0, 0.0, 0.25)
                    })),
                    ..Default::default()
                });

            let options_panel = self.view_chat_options_popover();

            Stack::new()
                .push(inner)
                .push(backdrop)
                .push(
                    widget::container(options_panel)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else if self.show_chat_search {
            use iced::widget::Stack;
            use iced::Color;

            let backdrop = widget::button(widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .on_press(AppMessage::ToggleChatSearch)
                .style(move |t, _status| iced::widget::button::Style {
                    background: Some(iced::Background::Color(if matches!(t, iced::Theme::Dark) {
                        Color::from_rgba(0.0, 0.0, 0.0, 0.35)
                    } else {
                        Color::from_rgba(0.0, 0.0, 0.0, 0.15)
                    })),
                    ..Default::default()
                });

            let search_panel = self.view_chat_search_panel();

            Stack::new()
                .push(inner)
                .push(backdrop)
                .push(
                    widget::container(search_panel)
                        .width(Length::Fill)
                        .padding(iced::Padding {
                            top: 72.0, // below the fixed header
                            right: SPACE_16,
                            bottom: 0.0,
                            left: 0.0,
                        })
                        .align_x(iced::alignment::Horizontal::Right)
                        .align_y(iced::alignment::Vertical::Top),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else if self.help_visible {
            use iced::widget::Stack;
            use iced::Color;
            let chat_layer = inner;

            let backdrop = widget::button(widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .on_press(AppMessage::ToggleHelp)
                .style(move |t, _status| iced::widget::button::Style {
                    background: Some(iced::Background::Color(if matches!(t, iced::Theme::Dark) {
                        Color::from_rgba(0.0, 0.0, 0.0, 0.55)
                    } else {
                        Color::from_rgba(0.0, 0.0, 0.0, 0.35)
                    })),
                    ..Default::default()
                });

            let help_panel = widget::container(self.view_help())
                .width(Length::Shrink)
                .height(Length::Shrink)
                .max_width(480.0)
                .max_height(600.0)
                .style(move |t| iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg_surface(t))),
                    border: iced::Border {
                        radius: SPACE_12.into(),
                        ..Default::default()
                    },
                    shadow: iced::Shadow {
                        color: Color::from_rgba(0.0, 0.0, 0.0, 0.3),
                        offset: iced::Vector::new(0.0, 4.0),
                        blur_radius: 24.0,
                    },
                    ..Default::default()
                });

            Stack::new()
                .push(chat_layer)
                .push(backdrop)
                .push(
                    widget::container(help_panel)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else if self.show_member_list {
            use iced::widget::Stack;
            use iced::Color;
            let chat_layer = inner;

            let backdrop = widget::button(widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .on_press(AppMessage::ToggleMemberList)
                .style(move |t, _status| iced::widget::button::Style {
                    background: Some(iced::Background::Color(if matches!(t, iced::Theme::Dark) {
                        Color::from_rgba(0.0, 0.0, 0.0, 0.45)
                    } else {
                        Color::from_rgba(0.0, 0.0, 0.0, 0.25)
                    })),
                    ..Default::default()
                });

            let member_list_panel = widget::container(self.view_group_member_list())
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill);

            Stack::new()
                .push(chat_layer)
                .push(backdrop)
                .push(member_list_panel)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            // ── Right-click context menu overlay ────────────────────
            if let Some((idx, _, _, kind)) = self.context_menu {
                use iced::widget::Stack;
                let menu = self.view_context_menu(idx, kind);
                Stack::new()
                    .push(inner)
                    .push(
                        // Position near top-right of chat area
                        widget::container(menu)
                            .width(Length::Fill)
                            .padding(iced::Padding {
                                top: SPACE_8,
                                right: SPACE_16,
                                bottom: 0.0,
                                left: 0.0,
                            })
                            .align_x(iced::alignment::Horizontal::Right)
                            .align_y(iced::alignment::Vertical::Top),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            } else {
                // ── Emoji picker overlay ──────────────────────────
                if self.show_emoji_picker {
                    use iced::widget::Stack;
                    let picker = self.view_emoji_picker();
                    Stack::new()
                        .push(inner)
                        .push(
                            widget::container(picker)
                                .width(Length::Fill)
                                .padding(iced::Padding {
                                    top: 0.0,
                                    right: SPACE_16,
                                    bottom: 48.0,
                                    left: 0.0,
                                })
                                .align_x(iced::alignment::Horizontal::Right)
                                .align_y(iced::alignment::Vertical::Bottom),
                        )
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                } else if self.show_gif_picker {
                    // ── GIF picker overlay ──────────────────────────
                    use iced::widget::Stack;
                    let picker = self.view_gif_picker();
                    Stack::new()
                        .push(inner)
                        .push(
                            widget::container(picker)
                                .width(Length::Fill)
                                .padding(iced::Padding {
                                    top: 0.0,
                                    right: SPACE_16,
                                    bottom: 48.0,
                                    left: 0.0,
                                })
                                .align_x(iced::alignment::Horizontal::Right)
                                .align_y(iced::alignment::Vertical::Bottom),
                        )
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                } else {
                    inner.into()
                }
            }
        }
    }

    // ── Chat screen view ─────────────────────────────────────────────

    /// Render the right-click context menu overlay.
    pub(crate) fn view_context_menu(
        &self,
        idx: usize,
        kind: ContextMenuKind,
    ) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container};

        let theme = self.theme();
        let close_btn = button(Icon::Close.build().size(IconSize::Xs).build())
            .on_press(AppMessage::CloseContextMenu)
            .padding([SPACE_2, SPACE_6])
            .style(|_t, _s| iced::widget::button::Style::default());

        let mut col = column![].spacing(0).width(180.0);

        match kind {
            ContextMenuKind::Text => {
                let copy_btn = button(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Copy Text"),
                )
                .on_press(AppMessage::ContextCopyText(idx))
                .padding([SPACE_4, SPACE_8])
                .style(|_t, _s| iced::widget::button::Style::default());
                col = col.push(
                    container(copy_btn)
                        .padding(SPACE_2)
                        .width(iced::Length::Fill),
                );
            }
            ContextMenuKind::Image => {
                let copy_img = button(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Copy Image"),
                )
                .on_press(AppMessage::ContextCopyImage(idx))
                .padding([SPACE_4, SPACE_8])
                .style(|_t, _s| iced::widget::button::Style::default());
                col = col.push(
                    container(copy_img)
                        .padding(SPACE_2)
                        .width(iced::Length::Fill),
                );
            }
        }

        let header = container(iced::widget::row![
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                match kind {
                    ContextMenuKind::Text => "Message",
                    ContextMenuKind::Image => "Image",
                },
            )
            .color(text_muted(&theme)),
            iced::widget::Space::new().width(iced::Length::Fill),
            close_btn,
        ])
        .padding([SPACE_4, SPACE_8]);

        container(column![header, col])
            .style(move |t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    color: border_muted(t),
                    width: 1.0,
                    radius: (8.0_f32).into(),
                },
                ..Default::default()
            })
            .width(200.0)
            .into()
    }

    /// Render the emoji picker panel with commonly used emojis.
    ///
    /// ICEDAW-01: migrated from the hand-rolled `container` overlay panel to
    /// `iced_aw::Card`. The Card provides the head row (title + built-in
    /// close button via `on_close`) and the body (scrollable grid), matching
    /// the previous layout exactly: 280px wide, `bg_surface` background,
    /// 1px `border_muted` border, 8px corner radius.
    pub(crate) fn view_emoji_picker(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, row, text};

        let theme = self.theme();
        const EMOJIS: &[&str] = &[
            "😀", "😂", "🤣", "😊", "😍", "🥰", "😘", "😜", "🤔", "🙄", "😢", "😭", "😤", "😡",
            "🥺", "😎", "🤩", "👍", "👎", "👏", "🙌", "💪", "🤝", "❤️", "🔥", "⭐", "🎉", "✨",
            "💯", "✅", "❌", "⚠️", "💡", "📌", "🎵", "🌈", "🍕", "☕", "🕐", "💤",
        ];

        let head = crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Emojis")
            .color(text_muted(&theme));

        let mut grid = column![].spacing(SPACE_2);
        for chunk in EMOJIS.chunks(8) {
            let mut r = row![].spacing(SPACE_2);
            for &emoji in chunk {
                let c = emoji.chars().next().unwrap();
                r = r.push(
                    button(text(emoji).size(20.0))
                        .on_press(AppMessage::InsertEmoji(c))
                        .padding([SPACE_2, SPACE_4])
                        .style(|_t, _s| iced::widget::button::Style::default()),
                );
            }
            grid = grid.push(r);
        }

        let scroll = crate::ui_components::gutter_scrollable(grid).height(iced::Length::Fixed(160.0));

        iced_aw::Card::new(head, scroll)
            .width(280.0)
            .padding_head(iced::Padding::new(SPACE_8))
            .padding_body(iced::Padding::new(SPACE_8))
            .on_close(AppMessage::ToggleEmojiPicker)
            .style(move |t, _status| iced_aw::style::card::Style {
                background: iced::Background::Color(bg_surface(t)),
                border_radius: 8.0,
                border_width: 1.0,
                border_color: border_muted(t),
                head_background: iced::Background::Color(bg_surface(t)),
                head_text_color: text_muted(t),
                body_background: iced::Background::Color(iced::Color::TRANSPARENT),
                body_text_color: text_muted(t),
                foot_background: iced::Background::Color(iced::Color::TRANSPARENT),
                foot_text_color: text_muted(t),
                close_color: text_muted(t),
            })
            .into()
    }

    // ── GIF picker async helpers ─────────────────────────────────────────
    //
    // All GIF picker network work goes through the provider-neutral
    // `GifProvider` trait object (obtained via `boru_core::default_gif_provider()`),
    // never a concrete KLIPY type.  Responses carry a monotonic request seq;
    // `update()` discards stale completions so an older search can never
    // overwrite newer results.

    /// Start a GIF search through the configured provider.
    pub(crate) fn start_gif_search(&mut self, query: String, cursor: Option<String>) -> iced::Task<AppMessage> {
        let Some(provider) = boru_core::default_gif_provider().ok() else {
            self.gif_not_configured = true;
            self.gif_loading = false;
            return iced::Task::none();
        };
        let seq = self.gif_request_seq.wrapping_add(1);
        self.gif_request_seq = seq;
        self.gif_loading = true;
        self.gif_error = None;
        self.gif_append_error = None;
        let task = iced::Task::perform(
            async move {
                let result = provider
                    .search(GifSearchRequest {
                        query,
                        cursor,
                        limit: 24,
                        content_rating: Some(GifContentRating::G),
                    })
                    .await;
                (seq, result)
            },
            |(seq, result)| match result {
                Ok(page) => AppMessage::GifSearchResults { seq, page },
                Err(error) => AppMessage::GifSearchFailed {
                    seq,
                    message: gif_provider_error_message(&error),
                },
            },
        );
        task
    }

    /// Start a trending-GIF request through the configured provider.
    pub(crate) fn start_gif_trending(&mut self, cursor: Option<String>) -> iced::Task<AppMessage> {
        let Some(provider) = boru_core::default_gif_provider().ok() else {
            self.gif_not_configured = true;
            self.gif_loading = false;
            return iced::Task::none();
        };
        let seq = self.gif_request_seq.wrapping_add(1);
        self.gif_request_seq = seq;
        self.gif_loading = true;
        self.gif_error = None;
        self.gif_append_error = None;
        let task = iced::Task::perform(
            async move {
                let result = provider
                    .trending(GifTrendingRequest {
                        cursor,
                        limit: 24,
                        content_rating: Some(GifContentRating::G),
                    })
                    .await;
                (seq, result)
            },
            |(seq, result)| match result {
                Ok(page) => AppMessage::GifTrendingResults { seq, page },
                Err(error) => AppMessage::GifSearchFailed {
                    seq,
                    message: gif_provider_error_message(&error),
                },
            },
        );
        task
    }

    /// Fire one small preview-thumbnail download per result that does not
    /// already have cached bytes.  Only the small `preview` rendition
    /// (WebP/GIF) is fetched — never a full-size original.
    pub(crate) fn gif_preview_download_tasks(&self) -> iced::Task<AppMessage> {
        let mut tasks: Vec<iced::Task<AppMessage>> = Vec::new();
        for result in &self.gif_results {
            if self.gif_preview_cache.contains_key(&result.provider_id) {
                continue;
            }
            // MP4 previews cannot be rendered by iced's image widget; skip them.
            if result.preview.format == GifMediaFormat::Mp4 {
                continue;
            }
            let url = result.preview.url.clone();
            let provider_id = result.provider_id.clone();
            tasks.push(iced::Task::perform(
                async move {
                    // Bound every preview fetch: an 8s timeout and a 5 MiB
                    // cap so a dead or oversized media URL degrades to the
                    // placeholder instead of hanging or exhausting memory.
                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(8))
                        .build()
                        .ok()?;
                    let resp = client.get(&url).send().await.ok()?;
                    if !resp.status().is_success() {
                        return None;
                    }
                    let bytes = resp.bytes().await.ok()?;
                    if bytes.len() > 5 * 1024 * 1024 {
                        return None;
                    }
                    Some((provider_id, bytes.to_vec()))
                },
                |opt| match opt {
                    Some((provider_id, bytes)) => AppMessage::GifPreviewLoaded(provider_id, bytes),
                    None => AppMessage::Noop,
                },
            ));
        }
        if tasks.is_empty() {
            iced::Task::none()
        } else {
            iced::Task::batch(tasks)
        }
    }

    /// Render the GIF picker panel with common GIF URLs and search/custom input.
    pub(crate) fn view_gif_picker(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text_input};

        let theme = self.theme();
        let close_btn = iced::widget::tooltip::Tooltip::new(
            button(Icon::Close.build().size(IconSize::Xs).build())
                .on_press(AppMessage::ToggleGifPicker)
                .padding([SPACE_2, SPACE_4])
                .style(|_t, _s| iced::widget::button::Style::default()),
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Close"),
            iced::widget::tooltip::Position::Bottom,
        );

        let header = row![
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "GIF Search")
                .color(text_muted(&theme)),
            iced::widget::Space::new().width(iced::Length::Fill),
            close_btn,
        ]
        .spacing(SPACE_4)
        .align_y(iced::Alignment::Center);

        // Search input
        let search_input = text_input("Search KLIPY", &self.gif_search_text)
            .on_input(AppMessage::GifSearchChanged)
            .on_submit(AppMessage::GifSearchSubmit)
            .size(crate::fonts::TypeRole::Body.size_px())
            .font(crate::fonts::TypeRole::Body.font())
            .padding([SPACE_4, SPACE_6]);

        let search_btn =
            button(crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Search"))
                .on_press_maybe(if !self.gif_search_text.is_empty() {
                    Some(AppMessage::GifSearchSubmit)
                } else {
                    None
                })
                .padding([SPACE_4, SPACE_8]);

        let search_row = row![search_input, search_btn]
            .spacing(SPACE_4)
            .align_y(iced::Alignment::Center);

        // KLIPY-09 privacy: make it explicit that external search is optional
        // and that search terms leave the device for the KLIPY service.  No
        // Boru identity, messages, or contacts are ever sent.
        let privacy_note = crate::fonts::type_role_text(
            crate::fonts::TypeRole::Metadata,
            "Optional — search terms are sent to the KLIPY GIF service. Your identity, messages, and contacts never leave Boru.",
        )
        .color(text_muted(&theme))
        .wrapping(iced::widget::text::Wrapping::Glyph);

        // ── Results area ── state machine: not-configured / loading /
        // error / no-results / empty / grid (+ load more).
        let mut results_col = column![].spacing(SPACE_4);

        if self.gif_not_configured {
            results_col = results_col.push(
                column![
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        "GIF search is not configured",
                    )
                    .color(text_muted(&theme)),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        "Set the KLIPY_API_KEY environment variable to enable external GIF search.",
                    )
                    .color(text_muted(&theme)),
                ]
                .spacing(SPACE_2),
            );
        } else if self.gif_loading && self.gif_results.is_empty() {
            // Loading spinner.
            const SPINNER_FRAMES: [&str; 10] =
                ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let spinner = SPINNER_FRAMES[self.gif_spinner_frame % SPINNER_FRAMES.len()];
            results_col = results_col.push(
                row![
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Body,
                        spinner,
                    )
                    .color(text_muted(&theme)),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        if self.gif_showing_trending {
                            "Loading trending GIFs…"
                        } else {
                            "Searching GIFs…"
                        },
                    )
                    .color(text_muted(&theme)),
                ]
                .spacing(SPACE_6)
                .align_y(iced::Alignment::Center),
            );
        } else if let Some(error) = &self.gif_error {
            results_col = results_col.push(
                column![
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        "Couldn't load GIFs",
                    )
                    .color(text_muted(&theme)),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        error.as_str(),
                    )
                    .color(text_muted(&theme)),
                    button(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            "Retry",
                        )
                    )
                    .on_press(AppMessage::GifRetry)
                    .padding([SPACE_4, SPACE_8]),
                ]
                .spacing(SPACE_2),
            );
        } else if self.gif_results.is_empty() {
            if self.gif_has_searched {
                results_col = results_col.push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        "No GIFs found — try a different search term",
                    )
                    .color(text_muted(&theme)),
                );
            } else {
                results_col = results_col.push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        "Type a search term and press Enter or Search",
                    )
                    .color(text_muted(&theme)),
                );
            }
        } else {
            // Render in rows of 2 thumbnails each.
            for chunk in self.gif_results.chunks(2) {
                let mut row_widgets = row![].spacing(SPACE_4);
                for gif in chunk {
                    let title = gif.title.as_deref().filter(|s| !s.is_empty()).unwrap_or("GIF");
                    let preview = self.gif_preview_cache.get(&gif.provider_id).cloned();

                    let thumbnail: iced::Element<'_, AppMessage> = match preview {
                        Some(bytes) if !bytes.is_empty() => {
                            let handle = iced::widget::image::Handle::from_bytes(bytes);
                            iced::widget::image(handle)
                                .width(iced::Length::Fixed(150.0))
                                .height(iced::Length::Fixed(100.0))
                                .into()
                        }
                        _ => container(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                "...",
                            )
                            .color(text_muted(&theme)),
                        )
                        .width(150.0)
                        .height(100.0)
                        .center_x(iced::Length::Fill)
                        .center_y(iced::Length::Fill)
                        .style(move |t| iced::widget::container::Style {
                            background: Some(iced::Background::Color(bg_surface_secondary(t))),
                            ..Default::default()
                        })
                        .into(),
                    };

                    let card = button(
                        column![
                            thumbnail,
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                title,
                            )
                            .color(text_muted(&theme)),
                        ]
                        .spacing(SPACE_2)
                        .width(iced::Length::Fixed(150.0)),
                    )
                    .on_press(AppMessage::SendGif(gif.clone()))
                    .padding(SPACE_4)
                    .style(|_t, _s| iced::widget::button::Style::default());

                    row_widgets = row_widgets.push(card);
                }
                results_col = results_col.push(row_widgets);
            }
            // Load-more button when another page exists.
            if self.gif_next_cursor.is_some() {
                results_col = results_col.push(
                    button(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::ButtonLabel,
                            if self.gif_loading { "Loading…" } else { "Load more" },
                        )
                    )
                    .on_press_maybe(if self.gif_loading {
                        None
                    } else {
                        Some(AppMessage::GifLoadMore)
                    })
                    .padding([SPACE_4, SPACE_8]),
                );
            }
            // A failed load-more keeps the already-loaded grid visible; show
            // the error as a compact note so the user can retry without
            // losing results.
            if let Some(append_error) = &self.gif_append_error {
                results_col = results_col.push(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        append_error.as_str(),
                    )
                    .color(text_muted(&theme)),
                );
            }
        }

        let scroll = crate::ui_components::gutter_scrollable(results_col).height(iced::Length::Fixed(300.0));

        container(
            column![header, search_row, privacy_note, scroll]
                .spacing(SPACE_6)
                .padding(SPACE_8),
        )
        .style(move |t| iced::widget::container::Style {
            background: Some(iced::Background::Color(bg_surface(t))),
            border: iced::Border {
                color: border_muted(t),
                width: 1.0,
                radius: (8.0_f32).into(),
            },
            ..Default::default()
        })
        .width(320.0)
        .into()
    }

    pub(crate) fn view_chat_header(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text};
        use iced::{Alignment, Length};

        let topic_hex = self.topic.to_string();
        let short_topic = &topic_hex[..8.min(topic_hex.len())];
        let conversation = self
            .conversation_store
            .active_iter()
            .into_iter()
            .find(|entry| entry.topic == self.topic);
        let room_name = conversation
            .map(|entry| entry.display_name())
            .unwrap_or_else(|| format!("Room {short_topic}"));
        let is_group = conversation
            .as_ref()
            .map(|entry| {
                matches!(
                    entry.kind,
                    boru_core::conversations::ConversationKind::Group
                )
            })
            .unwrap_or(false);
        let peer = conversation.and_then(|entry| PublicKey::from_str(&entry.peer_id).ok());

        // Presence: while the subscription or gossip sender is still coming
        // up we show Connecting; if no peer identity can be resolved we show
        // Unknown instead of guessing.
        let presence = if self.room_loading || self.sender.is_none() {
            PeerPresence::Connecting
        } else {
            peer.map(|key| self.peer_presence(&key))
                .unwrap_or(PeerPresence::Unknown)
        };

        // ── Shared ghost icon toolbar button ─────────────────────────
        // Consistent padding, tooltip and BUTTON_ICON (transparent, themed
        // hover) for every header action so the toolbar reads as one system.
        fn tool_btn<'a>(
            icon: iced::widget::svg::Svg<'a, iced::Theme>,
            tip: &'static str,
            msg: Option<AppMessage>,
        ) -> iced::Element<'a, AppMessage> {
            let mut b = button(icon).padding([SPACE_4, SPACE_6]).style(BUTTON_ICON);
            if let Some(m) = msg {
                b = b.on_press(m);
            }
            iced::widget::tooltip::Tooltip::new(
                b,
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, tip),
                iced::widget::tooltip::Position::Bottom,
            )
            .into()
        };

        // Group chat header: show name + member count
        // Direct chat header: show name + online/offline status + encryption cue
        let (avatar, identity) = if is_group {
            // Group avatar: initials from group name
            let initials = crate::presentation::initials(&room_name);
            let display_initials = if initials.is_empty() {
                "G".to_string()
            } else {
                initials
            };
            let theme_for_initials = self.theme();
            let is_dark = matches!(theme_for_initials, iced::Theme::Dark);
            let letter_color = crate::presentation::initials_color(&room_name, is_dark);
            let group_avatar = container(text(display_initials).size(TYPO_SM).color(letter_color))
                .width(Length::Fixed(AVATAR_SM))
                .height(Length::Fixed(AVATAR_SM))
                .center_x(Length::Fixed(AVATAR_SM))
                .center_y(Length::Fixed(AVATAR_SM))
                .style(move |t| iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg_surface_secondary(
                        &theme_for_initials,
                    ))),
                    border: iced::Border {
                        radius: SPACE_8.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into();

            let member_count = self
                .room_history
                .find(&self.topic)
                .map(|r| r.member_count)
                .unwrap_or(0);
            let member_label = if member_count > 0 {
                format!(
                    "{member_count} member{}",
                    if member_count == 1 { "" } else { "s" }
                )
            } else {
                "Group".to_string()
            };

            let group_identity = column![
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::BodyEmphasised,
                    room_name.clone(),
                )
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, member_label)
                    .style(move |t| iced::widget::text::Style {
                        color: Some(text_secondary(t)),
                    }),
            ]
            .spacing(SPACE_2)
            .width(Length::Fill);

            (group_avatar, group_identity)
        } else {
            let peer_avatar: iced::Element<'_, AppMessage> = peer
                .and_then(|key| self.friend_image_handles.get(&key).and_then(|h| h.clone()))
                .map(|handle| {
                    iced::widget::image(handle)
                        .content_fit(iced::ContentFit::ScaleDown)
                        .width(Length::Fixed(AVATAR_SM))
                        .height(Length::Fixed(AVATAR_SM))
                        .into()
                })
                .unwrap_or_else(|| {
                    let initials = crate::presentation::initials(&room_name);
                    let theme_for_initials = self.theme();
                    let is_dark = matches!(theme_for_initials, iced::Theme::Dark);
                    let letter_color = crate::presentation::initials_color(&room_name, is_dark);
                    container(text(initials).size(TYPO_SM).color(letter_color))
                        .width(Length::Fixed(AVATAR_SM))
                        .height(Length::Fixed(AVATAR_SM))
                        .center_x(Length::Fixed(AVATAR_SM))
                        .center_y(Length::Fixed(AVATAR_SM))
                        .style(move |t| iced::widget::container::Style {
                            background: Some(iced::Background::Color(bg_surface_secondary(
                                &theme_for_initials,
                            ))),
                            border: iced::Border {
                                radius: SPACE_8.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .into()
                });

            let status_text = presence.label();
            let status_dot =
                icon_svg(presence.icon(), TYPO_XS).style(move |t, _| iced::widget::svg::Style {
                    color: Some(presence.color(t)),
                });

            // CHAT-03: combined "Name | peerid" header element. The peer's
            // short ID sits next to the name with a pipe separator, and the
            // whole combined element is the single copy affordance — clicking
            // it copies the FULL peer id (toast + clipboard via CopyPeerId).
            // A tooltip reveals the full value on hover.
            let name_peer_row: iced::Element<'_, AppMessage> = match peer {
                Some(key) => {
                    let full_key = key.to_string();
                    let short_key = peer_id_short_form(&full_key);
                    let combined = row![
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::BodyEmphasised,
                            room_name.clone(),
                        )
                        .wrapping(iced::widget::text::Wrapping::None),
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::TechnicalValue,
                            format!(" | {short_key}"),
                        )
                        .style(move |t| iced::widget::text::Style {
                            color: Some(text_secondary(t)),
                        }),
                    ]
                    .spacing(SPACE_2)
                    .align_y(Alignment::Center);
                    iced::widget::tooltip::Tooltip::new(
                        iced::widget::mouse_area(combined)
                            .on_press(AppMessage::CopyPeerId(key))
                            .interaction(iced::mouse::Interaction::Pointer),
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            format!("Copy peer ID · {full_key}"),
                        ),
                        iced::widget::tooltip::Position::Bottom,
                    )
                    .into()
                }
                None => crate::fonts::type_role_text(
                    crate::fonts::TypeRole::BodyEmphasised,
                    room_name.clone(),
                )
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None)
                .into(),
            };

            // Security / connection cue derived from real state: iroh always
            // transports over QUIC (encrypted); the connection type mirrors the
            // details panel (direct mesh vs relay).
            let is_mesh_neighbor = peer.is_some_and(|pk| self.neighbors.contains(&pk));
            let connection_type = if is_mesh_neighbor {
                "Direct (mesh)"
            } else if presence != PeerPresence::Offline && presence != PeerPresence::Unknown {
                "Relay"
            } else {
                "Not connected"
            };
            let lock_icon = iced::widget::tooltip::Tooltip::new(
                container(icon_svg(ICON_LOCK, TYPO_XS).style(move |t, _| {
                    iced::widget::svg::Style {
                        color: Some(text_secondary(t)),
                    }
                }))
                .padding([0.0, SPACE_2]),
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    format!("QUIC encrypted · {connection_type}"),
                ),
                iced::widget::tooltip::Position::Bottom,
            );

            let peer_identity = column![
                name_peer_row,
                row![
                    status_dot,
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, status_text)
                        .style(move |t| iced::widget::text::Style {
                            color: Some(presence.color(t))
                        }),
                    lock_icon,
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::Metadata,
                        "End-to-end encrypted",
                    )
                    .style(move |t| {
                        iced::widget::text::Style {
                            color: Some(text_secondary(t)),
                        }
                    }),
                ]
                .spacing(SPACE_4)
                .align_y(Alignment::Center),
            ]
            .spacing(SPACE_2)
            .width(Length::Fill);

            (peer_avatar, peer_identity)
        };

        // ── Toolbar (right side) ─────────────────────────────────────
        // Ghost icon buttons for: search, delete, copy, share, overflow.
        // All actions use the shared tool_btn helper with consistent padding,
        // tooltips, and BUTTON_ICON style.
        let search = tool_btn(
            Icon::Search.build().size(IconSize::Sm).build().into(),
            "Search",
            Some(AppMessage::ToggleChatSearch),
        );

        // Delete: uses the existing ClearHistoryRequested/ConfirmClearHistory
        // confirmation flow. First press toggles a destructive "Delete?"
        // state; second press confirms and clears the conversation.
        let is_deleting = self.history_confirm_clear;
        let delete_icon = Icon::Delete
            .build()
            .size(IconSize::Sm)
            .destructive(true)
            .build();
        let delete: iced::Element<'_, AppMessage> = if is_deleting {
            let mut b = button(delete_icon).padding([SPACE_4, SPACE_6]);
            b = b.on_press(AppMessage::ConfirmClearHistory);
            iced::widget::tooltip::Tooltip::new(
                b,
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Confirm delete"),
                iced::widget::tooltip::Position::Bottom,
            )
            .into()
        } else {
            tool_btn(
                Icon::Delete.build().size(IconSize::Sm).build().into(),
                "Clear conversation",
                Some(AppMessage::ClearHistoryRequested),
            )
        };

        // Copy: peer-ID copy moved into the combined "Name | peerid" header
        // element (CHAT-03), so the toolbar copy button only remains for
        // groups, where it copies the room ticket (invite link).
        let copy: iced::Element<'_, AppMessage> = match peer {
            Some(_key) => iced::widget::Space::new().width(Length::Fixed(0.0)).into(),
            None => {
                let ticket = self.ticket_str.clone();
                if ticket.is_empty() {
                    iced::widget::Space::new().width(Length::Fixed(0.0)).into()
                } else {
                    tool_btn(
                        Icon::Copy.build().size(IconSize::Sm).build().into(),
                        "Copy invite link",
                        Some(AppMessage::CopyToClipboard(ticket)),
                    )
                }
            }
        };

        // Share: opens the shared files catalogue for direct chats.
        let share: iced::Element<'_, AppMessage> = match peer {
            Some(key) => tool_btn(
                Icon::Share.build().size(IconSize::Sm).build().into(),
                "Shared files",
                Some(AppMessage::BrowsePeerCatalogue(key)),
            ),
            None => iced::widget::Space::new().width(Length::Fixed(0.0)).into(),
        };

        // Overflow: opens the chat options popover with room info, advertise
        // toggle, delete, and settings.
        let overflow = tool_btn(
            Icon::More.build().size(IconSize::Sm).build().into(),
            "More options",
            Some(AppMessage::ToggleChatOptions),
        );

        // Calls are available only for direct, unblocked friends and only
        // while no other call is active.  Groups and public rooms get no call
        // buttons in the header.
        let is_blocked = peer.is_some_and(|key| {
            self.friends
                .get(&FriendId::from_public_key(key))
                .is_some_and(|record| record.relationship == FriendRelationship::Blocked)
        });
        let call_enabled = call_buttons_enabled(
            peer.is_some() && !is_group,
            is_blocked,
            self.active_call_id.is_some(),
        );
        let voice_call: iced::Element<'_, AppMessage> = peer
            .filter(|_| call_enabled)
            .map(|key| {
                tool_btn(
                    Icon::Phone.build().size(IconSize::Sm).build().into(),
                    "Start voice call",
                    Some(AppMessage::StartVoiceCall(key)),
                )
            })
            .unwrap_or_else(|| iced::widget::Space::new().width(Length::Fixed(0.0)).into());
        let video_call: iced::Element<'_, AppMessage> = peer
            .filter(|_| call_enabled)
            .map(|key| {
                tool_btn(
                    Icon::VideoCamera.build().size(IconSize::Sm).build().into(),
                    "Start video call",
                    Some(AppMessage::StartVideoCall(key)),
                )
            })
            .unwrap_or_else(|| iced::widget::Space::new().width(Length::Fixed(0.0)).into());

        // ── Header area (left): back button, avatar, identity ─────────
        // Identity receives Fill so it shrinks when the toolbar needs
        // space. Wrapping in a clipping container ensures long peer IDs
        // are visually truncated rather than overflowing the header bar.
        let back_btn = tool_btn(
            Icon::Back.build().size(IconSize::Md).build().into(),
            "Back to chats",
            Some(AppMessage::GoToChatList),
        );
        let header_area = row![
            back_btn,
            avatar,
            container(identity).width(Length::Fill).clip(true),
        ]
        .spacing(SPACE_4)
        .width(Length::Fill)
        .align_y(Alignment::Center);

        // ── Toolbar (right): fixed natural width, never shrinks ──────
        // Shrink ensures action buttons stay fully visible at any window
        // size. The header area absorbs the remaining space instead.
        let toolbar = row![voice_call, video_call, search, delete, copy, share, overflow,]
            .spacing(SPACE_4)
            .width(Length::Shrink)
            .align_y(Alignment::Center);

        container(
            row![header_area, toolbar]
                .spacing(SPACE_8)
                .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(60.0))
        .padding([SPACE_6, SPACE_10])
        .style(container_header)
        .into()
    }

    /// Return the indices of conversation entries matching the live search
    /// query (case-insensitive substring over body and sender label), capped
    /// so the results panel stays cheap to render.
    pub(crate) fn chat_search_matches(&self) -> Vec<usize> {
        chat_search_matches_in(&self.entries, &self.chat_search_query)
    }

    /// The restrained footer/status line below the chat composer (plan UI-16).
    ///
    /// Reports the active conversation's connection route and, when connected,
    /// the peer count. The header already owns presence + encryption (direct
    /// chats) and member count (group chats), so this footer shows only the
    /// complementary route/peer state — no status text is duplicated.
    pub(crate) fn view_chat_footer(&self) -> iced::Element<'_, AppMessage> {
        let conversation = self
            .conversation_store
            .active_iter()
            .into_iter()
            .find(|entry| entry.topic == self.topic);
        let peer = conversation.and_then(|entry| PublicKey::from_str(&entry.peer_id).ok());
        let is_group = conversation
            .map(|entry| {
                matches!(
                    entry.kind,
                    boru_core::conversations::ConversationKind::Group
                )
            })
            .unwrap_or(false);
        let presence = peer
            .map(|key| self.peer_presence(&key))
            .unwrap_or(PeerPresence::Unknown);
        let (route_label, connected, peer_label) =
            chat_footer_status(is_group, &self.neighbors, peer, presence);
        chat_status_footer(route_label, connected, peer_label)
    }

    /// In-conversation search panel — a compact popover listing messages that
    /// match the current query. Each result copies the full message text.
    pub(crate) fn view_chat_search_panel(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text_input};
        use iced::{Alignment, Length};

        let theme = self.theme();
        let matches = self.chat_search_matches();
        let total = self.entries.len();

        let close_btn = iced::widget::tooltip::Tooltip::new(
            button(Icon::Close.build().size(IconSize::Xs).build())
                .on_press(AppMessage::ToggleChatSearch)
                .padding([SPACE_2, SPACE_4])
                .style(BUTTON_ICON),
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Close"),
            iced::widget::tooltip::Position::Bottom,
        );

        let header = row![
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::CardTitle,
                "Search in conversation",
            ),
            iced::widget::Space::new().width(Length::Fill),
            close_btn,
        ]
        .spacing(SPACE_4)
        .align_y(Alignment::Center);

        let input = text_input("Search messages…", &self.chat_search_query)
            .on_input(AppMessage::ChatSearchQueryChanged)
            .on_submit(AppMessage::ToggleChatSearch)
            .size(crate::fonts::TypeRole::Body.size_px())
            .font(crate::fonts::TypeRole::Body.font())
            .padding([SPACE_4, SPACE_6]);

        let summary = if self.chat_search_query.trim().is_empty() {
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!("{total} messages loaded"),
            )
            .color(text_muted(&theme))
        } else {
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!(
                    "{} match{}",
                    matches.len(),
                    if matches.len() == 1 { "" } else { "es" }
                ),
            )
            .color(accent_primary(&theme))
        };

        let mut results = column![].spacing(SPACE_2);
        if matches.is_empty() && !self.chat_search_query.trim().is_empty() {
            results = results.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "No matching messages",
                )
                .color(text_muted(&theme)),
            );
        } else {
            for idx in &matches {
                let entry = &self.entries[*idx];
                let body = if entry.body.len() > 140 {
                    format!("{}…", &entry.body[..140])
                } else {
                    entry.body.clone()
                };
                let result_row: iced::Element<'_, AppMessage> = button(
                    column![
                        row![
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                entry.label.clone(),
                            )
                            .color(text_muted(&theme)),
                            iced::widget::Space::new().width(Length::Fill),
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                entry.timestamp.map(format_message_time).unwrap_or_default(),
                            )
                            .color(text_muted(&theme)),
                        ]
                        .spacing(SPACE_4),
                        crate::fonts::type_role_text(crate::fonts::TypeRole::Body, body)
                            .wrapping(iced::widget::text::Wrapping::None)
                            .color(crate::design_tokens::text(&theme)),
                    ]
                    .spacing(SPACE_2)
                    .align_x(Alignment::Start),
                )
                .on_press(AppMessage::CopyToClipboard(entry.body.clone()))
                .padding([SPACE_4, SPACE_6])
                .style(BUTTON_GHOST_BG)
                .width(Length::Fill)
                .into();
                results = results.push(result_row);
            }
        }

        let content = column![header, input, summary, crate::ui_components::gutter_scrollable(results)]
            .spacing(SPACE_6)
            .padding(SPACE_10);

        container(content)
            .style(move |t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    color: border_muted(t),
                    width: 1.0,
                    radius: SPACE_12.into(),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.3),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            })
            .width(Length::Fixed(380.0))
            .max_height(460.0)
            .into()
    }

    /// Render the group member list overlay — showing avatar, display name, role, and presence.
    pub(crate) fn view_group_member_list(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text, Space};
        use iced::{Alignment, Length};

        // Resolve the group via conversation store -> group_id -> storage -> list_group_members.
        let group_members: Option<Vec<(String, String, bool)>> = (|| {
            let conversation = self
                .conversation_store
                .active_iter()
                .into_iter()
                .find(|e| e.topic == self.topic)?;
            if !matches!(
                conversation.kind,
                boru_core::conversations::ConversationKind::Group
            ) {
                return None;
            }
            let group_id = conversation.group_id?;
            let storage = self.storage.as_ref()?;
            let rows = storage.list_group_members(group_id.as_bytes()).ok()?;
            Some(
                rows.iter()
                    .filter(|r| r.state == "Active" || r.state == "Member" || r.state == "Owner")
                    .map(|r| {
                        let pk_opt: Option<iroh::PublicKey> = if r.public_key.len() == 32 {
                            let mut arr = [0u8; 32];
                            arr.copy_from_slice(&r.public_key);
                            iroh::PublicKey::from_bytes(&arr).ok()
                        } else {
                            None
                        };
                        let display_name = pk_opt
                            .as_ref()
                            .map(|pk| {
                                self.names.get(pk).cloned().unwrap_or_else(|| {
                                    let pk_str = pk.to_string();
                                    self.conversation_store
                                        .active_iter()
                                        .into_iter()
                                        .find_map(|e| {
                                            if e.peer_id == pk_str {
                                                Some(e.name.clone())
                                            } else {
                                                None
                                            }
                                        })
                                        .unwrap_or_else(|| {
                                            let s = pk.to_string();
                                            format!("{}..{}", &s[..6], &s[s.len() - 4..])
                                        })
                                })
                            })
                            .unwrap_or_else(|| "Unknown".to_string());
                        let role = r.role.clone();
                        let online = pk_opt.is_some_and(|k| self.neighbors.contains(&k));
                        (display_name, role, online)
                    })
                    .collect::<Vec<_>>(),
            )
        })();

        let theme = self.theme();
        let dark = matches!(theme, iced::Theme::Dark);
        let bg = bg_surface(&theme);

        let header = row![
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Group Members"),
            Space::new().width(Length::Fill),
            iced::widget::tooltip::Tooltip::new(
                button(icon_svg(ICON_CLOSE, TYPO_SM))
                    .on_press(AppMessage::ToggleMemberList)
                    .padding([SPACE_4, SPACE_6])
                    .style(BUTTON_ICON),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Close"),
                iced::widget::tooltip::Position::Bottom,
            ),
        ]
        .spacing(SPACE_4)
        .align_y(Alignment::Center)
        .padding([SPACE_8, SPACE_10]);

        let list_body: iced::Element<'_, AppMessage> = match group_members {
            Some(members) if !members.is_empty() => {
                let member_rows: Vec<iced::Element<'_, AppMessage>> = members
                    .into_iter()
                    .map(|(name, role, online)| {
                        let initials = crate::presentation::initials(&name);
                        let display_initials = if initials.is_empty() {
                            "?".to_string()
                        } else {
                            initials
                        };
                        let letter_color = crate::presentation::initials_color(&name, dark);

                        let avatar =
                            container(text(display_initials).size(TYPO_XS).color(letter_color))
                                .width(Length::Fixed(28.0))
                                .height(Length::Fixed(28.0))
                                .center_x(Length::Fixed(28.0))
                                .center_y(Length::Fixed(28.0))
                                .style(move |t| iced::widget::container::Style {
                                    background: Some(iced::Background::Color(
                                        bg_surface_secondary(&t),
                                    )),
                                    border: iced::Border {
                                        radius: SPACE_6.into(),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                });

                        let status_dot =
                            icon_svg(if online { ICON_ONLINE } else { ICON_OFFLINE }, TYPO_XS)
                                .style(move |t, _| iced::widget::svg::Style {
                                    color: Some(if online {
                                        accent_green(&t)
                                    } else {
                                        text_muted(&t)
                                    }),
                                });

                        let role_label = if role == "Owner" { "Owner" } else { "" };

                        row![
                            avatar,
                            crate::fonts::type_role_text(crate::fonts::TypeRole::Body, name)
                                .width(Length::FillPortion(3)),
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                role_label,
                            )
                            .style(move |t| iced::widget::text::Style {
                                color: Some(text_secondary(t))
                            })
                            .width(Length::FillPortion(1)),
                            status_dot,
                        ]
                        .spacing(SPACE_6)
                        .align_y(Alignment::Center)
                        .padding([SPACE_4, SPACE_10])
                        .into()
                    })
                    .collect::<Vec<iced::Element<'_, AppMessage>>>();

                crate::ui_components::gutter_scrollable(column(member_rows).spacing(SPACE_2))
                    .height(Length::Fill)
                    .into()
            }
            _ => crate::fonts::type_role_text(
                crate::fonts::TypeRole::SupportingText,
                "No members found",
            )
            .style(move |t| iced::widget::text::Style {
                color: Some(text_secondary(t)),
            })
            .width(Length::Fill)
            .into(),
        };

        container(column![header, list_body].spacing(SPACE_4))
            .width(Length::Fixed(300.0))
            .height(Length::FillPortion(3))
            .max_height(500.0)
            .style(move |t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: SPACE_12.into(),
                    ..Default::default()
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.3),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            })
            .into()
    }

    /// Build the chat options popover — a compact card with room info,
    /// navigation, and management actions.
    pub(crate) fn view_chat_options_popover(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row};
        use iced::{Alignment, Length};

        let topic_hex = self.topic.to_string();
        let short_topic = if topic_hex.len() > 8 {
            format!("{}…", &topic_hex[..8])
        } else {
            topic_hex.clone()
        };

        let room_name = self
            .room_history
            .find(&self.topic)
            .map(|r| r.display_name())
            .unwrap_or_else(|| format!("Room {}", short_topic));

        let is_deleting = self.room_delete_confirm_topic == Some(self.topic);
        let delete_label = if is_deleting {
            "Delete?"
        } else {
            "Delete Chat"
        };

        let ticket_short = if self.ticket_str.len() > 12 {
            format!(
                "{}…{}",
                &self.ticket_str[..6],
                &self.ticket_str[self.ticket_str.len() - 6..]
            )
        } else if !self.ticket_str.is_empty() {
            self.ticket_str.clone()
        } else {
            "—".to_string()
        };

        let online_peers = self.peer_presence_map.len();
        let is_advertised = self.advertised_rooms.contains(&self.topic);

        let content = column![
            // ── Room name ──
            crate::fonts::type_role_text(crate::fonts::TypeRole::SectionTitle, room_name.clone()),
            // ── Back button ──
            button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                "← Back to chats",
            ))
            .on_press(AppMessage::GoToChatList)
            .style(BUTTON_GHOST_BG)
            .padding([SPACE_6, SPACE_12])
            .width(Length::Fill),
            // ── Separator ──
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "───")
                .color(self.color_muted()),
            // ── Room info ──
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Topic: ")
                    .color(self.color_muted()),
                crate::fonts::type_role_text(crate::fonts::TypeRole::TechnicalValue, topic_hex.clone()),
            ]
            .spacing(SPACE_4),
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Ticket: ")
                    .color(self.color_muted()),
                button(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, ticket_short.clone())
                        .color(self.color_muted())
                )
                .on_press(AppMessage::CopyToClipboard(self.ticket_str.clone()))
                .style(BUTTON_GHOST_BG)
                .padding([SPACE_2, SPACE_6]),
            ]
            .spacing(SPACE_4),
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Online: ")
                    .color(self.color_muted()),
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    format!("{}", online_peers),
                ),
            ]
            .spacing(SPACE_4),
            // ── Advertise toggle ──
            button(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    if is_advertised {
                        "✓ Advertised"
                    } else {
                        "Advertise in Directory"
                    },
                )
            )
            .on_press(AppMessage::ToggleAdvertiseRoom(self.topic))
            .style(BUTTON_GHOST_BG)
            .padding([SPACE_4, SPACE_10])
            .width(Length::Fill),
            // ── Separator ──
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "───")
                .color(self.color_muted()),
            // ── Actions ──
            button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                delete_label,
            ))
            .on_press(if is_deleting {
                AppMessage::ConfirmDeleteRoom(self.topic)
            } else {
                AppMessage::DeleteRoomRequested(self.topic)
            })
                .style(if is_deleting {
                    |t: &iced::Theme, _s: iced::widget::button::Status| {
                        iced::widget::button::Style {
                            background: Some(iced::Background::Color(color_error(t))),
                            text_color: iced::Color::WHITE,
                            border: iced::Border {
                                radius: SPACE_6.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }
                } else {
                    BUTTON_GHOST_BG
                })
                .padding([SPACE_6, SPACE_12])
                .width(Length::Fill),
            button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                "Settings",
            ))
            .on_press(AppMessage::OpenSettings)
            .style(BUTTON_GHOST_BG)
            .padding([SPACE_6, SPACE_12])
            .width(Length::Fill),
        ]
        .spacing(SPACE_6)
        .align_x(Alignment::Start)
        .padding(SPACE_16)
        .max_width(360.0);

        container(content)
            .style(|t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    color: border_muted(t),
                    width: 1.0,
                    radius: SPACE_12.into(),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.3),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            })
            .width(Length::Shrink)
            .into()
    }

    /// Right-side details panel — shows conversation metadata and actions.
    /// For direct conversations: contact info, connection, security, tools.
    /// For groups: group info panel with name, description, members, actions.
    pub(crate) fn view_details_panel(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text, Space};
        use iced::{Alignment, Length};

        let theme = self.theme().clone();

        // ── Look up current conversation entry ──
        let conversation = self.conversation_store.find(&self.topic);
        let is_direct = conversation
            .as_ref()
            .map(|entry| entry.kind == boru_core::conversations::ConversationKind::Direct)
            .unwrap_or(true);

        if is_direct {
            return self.view_details_panel_direct();
        }

        // ── Group details panel ─────────────────────────────────────────
        let display_name = conversation
            .as_ref()
            .map(|entry| entry.display_name())
            .unwrap_or_else(|| "Unknown".to_string());

        let member_count = self.neighbors.len();

        // Common badge
        let kind_badge =
            container(crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Group")
                .color(accent_primary(&theme)))
            .padding([SPACE_2, SPACE_8])
            .style(move |t| container::Style {
                background: Some(iced::Background::Color({
                    let mut c = accent_primary(t);
                    c.a = 0.12;
                    c
                })),
                border: iced::Border {
                    color: {
                        let mut c = accent_primary(t);
                        c.a = 0.25;
                        c
                    },
                    width: 1.0,
                    radius: SPACE_12.into(),
                },
                ..Default::default()
            });

        // ── Group info section ──
        let mut info_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        // Display name with badge
        let dn = display_name.clone();
        info_items.push(
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::BodyEmphasised, dn),
                Space::new().width(Length::Fixed(SPACE_8)),
                kind_badge,
            ]
            .align_y(Alignment::Center)
            .into(),
        );

        // Member count
        info_items.push(
            row![
                icon_svg(ICON_ONLINE, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(text_muted(t))
                }),
                Space::new().width(Length::Fixed(SPACE_4)),
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Body,
                    format!("Members · {}", member_count),
                )
                .color(text_secondary(&theme)),
            ]
            .align_y(Alignment::Center)
            .into(),
        );

        // ── Members section ──
        let mut member_rows: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        // Sort neighbors for stable order — use fmt_short for display
        let mut sorted_neighbors: Vec<PublicKey> = self.neighbors.iter().copied().collect();
        sorted_neighbors.sort_by(|a, b| a.fmt_short().to_string().cmp(&b.fmt_short().to_string()));

        for neighbor in sorted_neighbors.iter().take(12) {
            let theme = theme.clone();
            let short_name = neighbor.fmt_short().to_string();
            let display_label =
                boru_core::peer_names::resolve_peer_name(neighbor, None, None, None, None);
            let is_friend = self.peer_presence(neighbor) != PeerPresence::Offline;

            let row_element = row![
                // Avatar dot
                container(Space::new())
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(8.0))
                    .style({
                        let theme = theme.clone();
                        move |_t| container::Style {
                            background: Some(iced::Background::Color(if is_friend {
                                accent_green(&theme)
                            } else {
                                text_muted(&theme)
                            })),
                            border: iced::Border {
                                radius: 4.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
                Space::new().width(Length::Fixed(SPACE_8)),
                text(display_label.clone())
                    .size(crate::fonts::TypeRole::Body.size_px())
                    .font(crate::fonts::TypeRole::Body.font())
                    .width(Length::Fill),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, short_name.clone())
                    .color(text_muted(&theme)),
            ]
            .spacing(SPACE_4)
            .align_y(Alignment::Center)
            .width(Length::Fill);
            member_rows.push(row_element.into());
        }

        if self.neighbors.len() > 12 {
            member_rows.push(
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    format!("+ {} more", self.neighbors.len() - 12),
                )
                .color(text_muted(&theme))
                .into(),
            );
        }

        // Invite member button
        let invite_btn = button(
            row![
                icon_svg(ICON_USER_PLUS, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(accent_primary(t))
                }),
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Invite member")
                    .color(accent_primary(&theme)),
            ]
            .spacing(SPACE_6)
            .align_y(Alignment::Center),
        )
        .on_press(AppMessage::ToggleInviteMenu)
        .padding([SPACE_6, SPACE_12])
        .width(Length::Fill)
        .style(BUTTON_OUTLINE);

        // ── Settings section ──
        let settings_items: Vec<iced::Element<'_, AppMessage>> = vec![
            row![
                icon_svg(ICON_NOTIFICATION, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(text_secondary(t))
                }),
                Space::new().width(Length::Fixed(SPACE_8)),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, "Notifications"),
                Space::new().width(Length::Fill),
            ]
            .spacing(SPACE_4)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into(),
            row![
                icon_svg(ICON_FILES, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(text_secondary(t))
                }),
                Space::new().width(Length::Fixed(SPACE_8)),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, "Shared files"),
                Space::new().width(Length::Fill),
            ]
            .spacing(SPACE_4)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into(),
            row![
                icon_svg(ICON_MORE, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(text_secondary(t))
                }),
                Space::new().width(Length::Fixed(SPACE_8)),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, "Group information"),
                Space::new().width(Length::Fill),
            ]
            .spacing(SPACE_4)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into(),
        ];

        // ── Leave Group button (owner view would additionally show Edit group, Manage members) ──
        let leave_btn = button(crate::fonts::type_role_text(
            crate::fonts::TypeRole::ButtonLabel,
            "Leave Group",
        ))
        .padding([SPACE_6, SPACE_12])
        .width(Length::Fill)
        .style(BUTTON_DANGER);

        // ── Assemble the panel ──
        let panel_body = column![
            // Heading
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Details"),
            Space::new().height(Length::Fixed(SPACE_8)),
            // Info section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Group info")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(info_items).spacing(SPACE_4),
            divider(&theme),
            // Members section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Members")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(member_rows).spacing(SPACE_8),
            Space::new().height(Length::Fixed(SPACE_4)),
            invite_btn,
            divider(&theme),
            // Settings section
            column(settings_items).spacing(SPACE_10),
            divider(&theme),
            // Leave
            leave_btn,
            Space::new().height(Length::Fill),
        ]
        .spacing(SPACE_4);

        container(crate::ui_components::gutter_scrollable(panel_body))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([SPACE_8, SPACE_8])
            .style(container_surface)
            .into()
    }

    /// Direct-chat details panel — contact info, connection, security, tools.
    pub(crate) fn view_details_panel_direct(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, Space};
        use iced::{Alignment, Length};

        let theme = self.theme();

        // ── Look up current conversation entry ──
        let conversation = self.conversation_store.find(&self.topic);
        let peer = conversation
            .as_ref()
            .and_then(|entry| entry.peer_id.parse::<PublicKey>().ok());
        let presence = peer
            .map(|key| self.peer_presence(&key))
            .unwrap_or(PeerPresence::Offline);
        let is_online = presence != PeerPresence::Offline;
        let display_name = conversation
            .as_ref()
            .map(|entry| entry.display_name())
            .unwrap_or_else(|| "Unknown".to_string());
        let last_seen = conversation
            .as_ref()
            .map(|entry| {
                if presence == PeerPresence::Online {
                    "Online now".to_string()
                } else if presence == PeerPresence::Away {
                    "Away".to_string()
                } else if entry.last_seen_at_unix_ms > 0 {
                    format_last_seen(Some(entry.last_seen_at_unix_ms))
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();

        // ── Determine connection type for this peer ──
        let is_mesh_neighbor = peer.is_some_and(|pk| self.neighbors.contains(&pk));
        let connection_type = if is_mesh_neighbor {
            "Direct (mesh)"
        } else if is_online {
            "Relay"
        } else {
            "Not connected"
        };
        let connection_label = if is_online {
            "Connected"
        } else {
            "Disconnected"
        };

        // ── Section: Contact ──
        let mut contact_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        // Presence row: status dot + label
        contact_items.push(
            row![
                icon_svg(presence.icon(), TYPO_SM,).style(move |t, _| iced::widget::svg::Style {
                    color: Some(presence.color(t))
                }),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, presence.label())
                    .style(move |t| iced::widget::text::Style {
                        color: Some(presence.color(t))
                    }),
            ]
            .spacing(SPACE_6)
            .align_y(Alignment::Center)
            .into(),
        );

        // Kind badge
        let kind_badge = container(
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Direct")
                .color(accent_primary(&theme)),
        )
            .padding([SPACE_2, SPACE_8])
            .style(move |t| container::Style {
                background: Some(iced::Background::Color({
                    let mut c = accent_primary(t);
                    c.a = 0.12;
                    c
                })),
                border: iced::Border {
                    color: {
                        let mut c = accent_primary(t);
                        c.a = 0.25;
                        c
                    },
                    width: 1.0,
                    radius: SPACE_12.into(),
                },
                ..Default::default()
            });
        // Display name with badge
        let dn = display_name.clone();
        contact_items.push(
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::BodyEmphasised, dn),
                Space::new().width(Length::Fixed(SPACE_8)),
                kind_badge,
            ]
            .align_y(Alignment::Center)
            .into(),
        );

        if !last_seen.is_empty() {
            contact_items.push(info_row("Last seen".to_string(), last_seen, &theme).into());
        }

        // Peer ID with copy button
        if let Some(pk) = peer {
            let full_id = pk.to_string();
            let fid = full_id.clone();
            let copy_btn = button(
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Copy")
                    .color(text_muted(&theme)),
            )
            .on_press(AppMessage::CopyToClipboard(fid))
            .padding([SPACE_2, SPACE_4])
            .style(BUTTON_GHOST_BG);

            let truncated = if full_id.len() > 32 {
                format!("{}…", &full_id[..32])
            } else {
                full_id.clone()
            };
            let peer_id_row = row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Peer ID")
                    .color(text_secondary(&theme)),
                Space::new().width(Length::Fill),
                crate::fonts::type_role_text(crate::fonts::TypeRole::TechnicalValue, truncated)
                    .color(crate::design_tokens::text(&theme)),
                copy_btn,
            ]
            .spacing(SPACE_4)
            .align_y(Alignment::Center)
            .width(Length::Fill);
            contact_items.push(peer_id_row.into());
        }

        // First-seen / created date
        if let Some(entry) = conversation.as_ref() {
            if entry.created_at_unix_ms > 0 {
                let created = crate::presentation::relative_time(entry.created_at_unix_ms);
                contact_items.push(info_row("First seen".to_string(), created, &theme).into());
            }
        }

        // ── Section: Connection ──
        let mut conn_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        // Connection state indicator
        let conn_state_color = if is_online {
            accent_green(&theme)
        } else {
            text_muted(&theme)
        };
        let conn_state_dot = icon_svg(if is_online { ICON_ONLINE } else { ICON_OFFLINE }, TYPO_SM)
            .style(move |_t, _| iced::widget::svg::Style {
                color: Some(conn_state_color),
            });
        let conn_state_row = row![
            conn_state_dot,
            crate::fonts::type_role_text(crate::fonts::TypeRole::Body, connection_label)
                .style(move |t| iced::widget::text::Style {
                    color: Some(if is_online {
                        accent_green(t)
                    } else {
                        text_muted(t)
                    }),
                }),
        ]
        .spacing(SPACE_6)
        .align_y(Alignment::Center);
        conn_items.push(conn_state_row.into());

        conn_items.push(
            info_row(
                "Connection".to_string(),
                connection_type.to_string(),
                &theme,
            )
            .into(),
        );

        // Relay mode
        let relay_label = fmt_relay_mode(&self.relay_mode);
        conn_items.push(info_row("Relay".to_string(), relay_label, &theme).into());

        // Latency
        if let Some(pk) = peer {
            if let Some(latency) = self.peer_latencies.get(&pk) {
                let ms = latency.as_millis();
                conn_items.push(info_row("Latency".to_string(), format!("{ms} ms"), &theme).into());
            }
        }

        // ── Section: Security ──
        let mut security_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();
        security_items.push(
            info_row(
                "Encryption".to_string(),
                "QUIC (encrypted)".to_string(),
                &theme,
            )
            .into(),
        );

        if let Some(pk) = peer {
            let fingerprint = pk.fmt_short().to_string();
            let full_key = pk.to_string();
            let fpr = fingerprint.clone();
            let copy_btn = button(
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Copy")
                    .color(text_muted(&theme)),
            )
            .on_press(AppMessage::CopyToClipboard(full_key.clone()))
            .padding([SPACE_2, SPACE_4])
            .style(BUTTON_GHOST_BG);

            let key_row = row![
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "Key fingerprint",
                )
                .color(text_secondary(&theme)),
                Space::new().width(Length::Fill),
                crate::fonts::type_role_text(crate::fonts::TypeRole::TechnicalValue, fpr)
                    .color(crate::design_tokens::text(&theme)),
                copy_btn,
            ]
            .spacing(SPACE_4)
            .align_y(Alignment::Center)
            .width(Length::Fill);
            security_items.push(key_row.into());
        }

        // ── Section: Tools ──
        let mut tool_btns: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        if let Some(pk) = peer {
            let shared_files_btn = button(
                row![
                    icon_svg(ICON_FILES, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                        color: Some(accent_primary(t))
                    }),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Shared files",
                    )
                    .color(accent_primary(&theme)),
                ]
                .spacing(SPACE_6)
                .align_y(Alignment::Center),
            )
            .on_press(AppMessage::BrowsePeerCatalogue(pk))
            .padding([SPACE_6, SPACE_12])
            .width(Length::Fill)
            .style(BUTTON_OUTLINE);
            tool_btns.push(shared_files_btn.into());
        }

        let connection_btn = button(
            row![
                icon_svg(ICON_ACTIVITY, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(accent_primary(t))
                }),
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Connection details",
                )
                .color(accent_primary(&theme)),
            ]
            .spacing(SPACE_6)
            .align_y(Alignment::Center),
        )
        .on_press(AppMessage::OpenConnectionDetails)
        .padding([SPACE_6, SPACE_12])
        .width(Length::Fill)
        .style(BUTTON_OUTLINE);
        tool_btns.push(connection_btn.into());

        // Only show if we have a valid peer key
        if let Some(pk) = peer {
            let tunnel_btn = button(
                row![
                    icon_svg(ICON_ACTIVITY, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                        color: Some(accent_primary(t))
                    }),
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        "Create Tunnel",
                    )
                    .color(accent_primary(&theme)),
                ]
                .spacing(SPACE_6)
                .align_y(Alignment::Center),
            )
            .on_press(AppMessage::CreateTunnel(pk))
            .padding([SPACE_6, SPACE_12])
            .width(Length::Fill)
            .style(BUTTON_OUTLINE);
            tool_btns.push(tunnel_btn.into());
        }

        // ── Assemble the panel ──
        let panel_body = column![
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Details"),
            Space::new().height(Length::Fixed(SPACE_8)),
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Contact")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(contact_items).spacing(SPACE_4),
            divider(&theme),
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Connection")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(conn_items).spacing(SPACE_4),
            divider(&theme),
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Security")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(security_items).spacing(SPACE_4),
            divider(&theme),
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Tools")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(tool_btns).spacing(SPACE_4),
            Space::new().height(Length::Fill),
        ]
        .spacing(SPACE_4);

        container(crate::ui_components::gutter_scrollable(panel_body))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([SPACE_8, SPACE_8])
            .style(container_surface)
            .into()
    }

    /// Right-side group info panel — shown when the active conversation is a group.
    pub(crate) fn view_group_info_panel(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, Space};
        use iced::{Alignment, Length};

        let theme = self.theme();
        let conversation = self.conversation_store.find(&self.topic);
        let display_name = conversation
            .as_ref()
            .map(|entry| entry.display_name())
            .unwrap_or_else(|| "Group".to_string());
        let room_entry = self.room_history.find(&self.topic);
        let description = room_entry
            .and_then(|r| {
                // Use room metadata description from room_history or room_docs
                // For now derive it (stored in room history through the group creation path)
                None::<String>
            })
            .unwrap_or_default();

        let member_count = room_entry.map(|r| r.member_count).unwrap_or(0);
        let is_owner = room_entry.map(|r| r.is_owner).unwrap_or(true);

        // ── Section: Group Info ──
        let mut info_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        // Group name
        info_items.push(
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::BodyEmphasised, display_name.clone()),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Group")
                    .color(accent_primary(&theme)),
            ]
            .spacing(SPACE_8)
            .align_y(Alignment::Center)
            .into(),
        );

        // Member count
        let member_label = if member_count > 0 {
            format!(
                "{} member{}",
                member_count,
                if member_count == 1 { "" } else { "s" }
            )
        } else {
            "Group".to_string()
        };
        info_items.push(info_row("Members".to_string(), member_label, &theme).into());

        if is_owner {
            info_items.push(info_row("Role".to_string(), "Owner".to_string(), &theme).into());
        }

        // ── Section: Members ──
        let mut member_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        // List the local user
        let local_label = format!("{} (you)", self.local_label.clone());
        member_items.push(
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, local_label),
                Space::new().width(Length::Fill),
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    if is_owner { "Owner" } else { "Member" },
                )
                .color(text_secondary(&theme)),
            ]
            .spacing(SPACE_4)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into(),
        );

        // List friends who are in the group (from selected_members during creation)
        // For now show a minimal members list — full roster requires RosterDoc handle
        member_items.push(
            row![crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                format!("{} online", self.neighbors.len()),
            )
            .color(text_muted(&theme)),]
            .into(),
        );

        // ── Section: Advanced ──
        let topic_hex = self.topic.to_string();
        let short_topic = if topic_hex.len() > 16 {
            format!("{}…", &topic_hex[..16])
        } else {
            topic_hex.clone()
        };

        let mut advanced_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();
        advanced_items.push(info_row("Group ID".to_string(), short_topic, &theme).into());

        // ── Owner-only controls ──
        let mut owner_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();
        if is_owner {
            owner_items.push(
                container(
                    crate::fonts::type_role_text(
                        crate::fonts::TypeRole::SupportingText,
                        "Owner Controls",
                    )
                    .color(text_secondary(&theme)),
                )
                .padding([SPACE_4, 0.0])
                .into(),
            );
        }

        // ── Actions ──
        let mut action_items: Vec<iced::Element<'_, AppMessage>> = Vec::new();

        // Invite member button (owner only)
        let invite_btn = button(
            row![
                icon_svg(ICON_PLUS, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(accent_primary(t))
                }),
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Invite Member",
                )
                .color(accent_primary(&theme)),
            ]
            .spacing(SPACE_6)
            .align_y(Alignment::Center),
        )
        .on_press(AppMessage::ShowInviteMemberDialog)
        .padding([SPACE_6, SPACE_12])
        .width(Length::Fill)
        .style(BUTTON_OUTLINE);
        action_items.push(invite_btn.into());

        // Leave group button (wired but actual leave logic in Phase 16)
        let leave_btn = button(
            row![
                icon_svg(ICON_CLOSE, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(color_error(t))
                }),
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Leave Group")
                    .color(color_error(&theme)),
            ]
            .spacing(SPACE_6)
            .align_y(Alignment::Center),
        )
        .padding([SPACE_6, SPACE_12])
        .width(Length::Fill)
        .style(move |t: &iced::Theme, _s| iced::widget::button::Style {
            border: iced::Border {
                color: {
                    let mut c = color_error(t);
                    c.a = 0.3;
                    c
                },
                width: 1.0,
                radius: SPACE_6.into(),
            },
            ..iced::widget::button::Style::default()
        });
        action_items.push(leave_btn.into());

        // Connection details button
        let connection_btn = button(
            row![
                icon_svg(ICON_ACTIVITY, TYPO_SM).style(|t, _| iced::widget::svg::Style {
                    color: Some(accent_primary(t))
                }),
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::ButtonLabel,
                    "Connection details",
                )
                .color(accent_primary(&theme)),
            ]
            .spacing(SPACE_6)
            .align_y(Alignment::Center),
        )
        .on_press(AppMessage::OpenConnectionDetails)
        .padding([SPACE_6, SPACE_12])
        .width(Length::Fill)
        .style(BUTTON_OUTLINE);
        action_items.push(connection_btn.into());

        // ── Assemble the panel ──
        let panel_body = column![
            // Heading
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Group Info"),
            Space::new().height(Length::Fixed(SPACE_8)),
            // Group Info section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "About")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(info_items).spacing(SPACE_4),
            divider(&theme),
            // Members section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Members")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(member_items).spacing(SPACE_4),
            divider(&theme),
            // Advanced section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Advanced")
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(advanced_items).spacing(SPACE_4),
        ]
        .spacing(SPACE_4);

        // Build the full panel with owner controls and actions at the bottom
        let mut full_panel = column![panel_body].spacing(0);

        if !owner_items.is_empty() {
            full_panel = full_panel.push(divider(&theme));
            full_panel = full_panel.push(column(owner_items).spacing(SPACE_4));
        }

        full_panel = full_panel.push(divider(&theme));
        full_panel = full_panel.push(
            column![
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, "Actions")
                    .color(text_secondary(&theme)),
                Space::new().height(Length::Fixed(SPACE_2)),
                column(action_items).spacing(SPACE_4),
            ]
            .spacing(SPACE_4),
        );

        full_panel = full_panel.push(Space::new().height(Length::Fill));

        container(crate::ui_components::gutter_scrollable(full_panel))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([SPACE_8, SPACE_8])
            .style(container_surface)
            .into()
    }

    pub(crate) fn view_chat_log(
        &self,
        timeline_width: f32,
        viewport_height: f32,
    ) -> iced::widget::Scrollable<'_, AppMessage> {
        use iced::widget::space;
        use iced::widget::text::Wrapping;
        use iced::widget::{button, container, scrollable, text, Column, Row};
        use iced::{Alignment, Length};

        let _start = std::time::Instant::now();

        // ── Ensure layout cache is up-to-date ──
        // Uses the incrementally maintained cache so the height/cumulative passes
        // only run when entries or settings actually change, not on every frame.
        let lc = &mut *self.layout_cache.borrow_mut();
        lc.ensure(&self.entries, self.chat_text_size, timeline_width);

        let total_entries = self.entries.len();
        let total_image_bytes = lc.total_image_bytes;
        let image_entry_count = lc.image_entry_count;

        let theme = self.theme();

        // ── Empty state ──
        if self.entries.is_empty() {
            let col = if self.sender.is_none() {
                // Still connecting — the subscription completed but the
                // gossip sender isn't ready. Show an inline spinner.
                const SPINNER_FRAMES: [&str; 10] =
                    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let spinner = SPINNER_FRAMES[self.connecting_spinner_frame % SPINNER_FRAMES.len()];
                Column::new().push(
                    container(
                        Column::new()
                            .push(text(spinner).size(28.0).color(accent_primary(&theme)))
                            .push(
                                crate::fonts::type_role_text(
                                    crate::fonts::TypeRole::Body,
                                    "Connecting…",
                                )
                                .color(self.color_muted()),
                            )
                            .spacing(SPACE_8)
                            .align_x(iced::Alignment::Center),
                    )
                    .padding([0.0, SPACE_8])
                    .width(Length::Fill)
                    .center_x(Length::Fill),
                )
            } else {
                Column::new().push(
                    container(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Body,
                            "No messages yet.",
                        )
                        .color(self.color_muted()),
                    )
                    .padding([0.0, SPACE_8])
                    .width(Length::Fill),
                )
            };
            self.total_content_height.set(0.0);
            // Empty-state render — record perf snapshot
            self.perf.replace(PerfMetrics {
                last_render_time_ns: _start.elapsed().as_nanos() as u64,
                window_size: 0,
                total_entries,
                total_image_bytes,
                image_entry_count,
            });
            return crate::ui_components::gutter_scrollable(col)
                .id(CHAT_LOG)
                .anchor_bottom()
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .on_scroll(|v: scrollable::Viewport| {
                    AppMessage::Scrolled(v.absolute_offset().y, v.bounds().height)
                });
        }

        // ── Use cached layout data for window computation (O(log n)) ──
        let total_height = lc.total_height;
        self.total_content_height.set(total_height);

        // Effective bubble width cap: 560 px or 68 % of the timeline width,
        // whichever is smaller (plan §4).  Supplied by the responsive wrapper
        // in `view_chat_panel` so bubbles never exceed the conversation
        // column and long content wraps instead of overflowing.
        let bubble_max_w = crate::presentation::chat_bubble_max_width(timeline_width);

        let (first_idx, last_idx, top_space_h, bottom_h) =
            lc.window(self.scroll_offset, viewport_height);

        // Bottom-align a short timeline. The scrollable keeps its Fill height
        // (so the timeline is always the sole expanding region between the
        // fixed header and the pinned composer); when the message content is
        // shorter than the viewport, a leading spacer pushes the content to
        // the bottom so it hugs the composer. Whitespace then sits above the
        // messages (balanced, chat convention) instead of leaving a giant dead
        // area below them. When content overflows the viewport the spacer is
        // zero and the anchored-to-bottom scrolling takes over unchanged.
        //
        // The cache's `total_height` sums entry heights only; the rendered
        // column also inserts `SPACE_4` between every child (leading spacer,
        // date separators, entries, bottom spacer). Count the children we are
        // about to push inside the visible window and subtract their gap
        // overhead from the lead, so a short timeline fills the viewport
        // exactly and iced does not paint a phantom near-full-height
        // scrollbar for content that already fits.
        //
        // `viewport_height` is the measured timeline region height supplied by
        // the `responsive` wrapper in `view_chat_panel` — it cannot come from
        // `self.viewport_height`, because iced only emits `Scrolled` events
        // when content overflows (short content would leave it at 0).
        let visible_count = last_idx.saturating_sub(first_idx).saturating_add(1);
        let mut date_seps_in_window = 0usize;
        {
            let mut prev_day = if first_idx > 0 {
                self.entries[first_idx - 1]
                    .timestamp
                    .map(|ts| ts / 86400000)
            } else {
                None
            };
            for i in first_idx..=last_idx {
                if let Some(day) = self.entries[i].timestamp.map(|ts| ts / 86400000) {
                    if prev_day != Some(day) {
                        date_seps_in_window += 1;
                    }
                    prev_day = Some(day);
                }
            }
        }
        // Children in the short-content layout: lead spacer + visible entries
        // + date separators (+ top/bottom spacers only when overflowing,
        // where the lead is zero anyway). Gaps = children - 1.
        let gap_overhead = SPACE_4 * (visible_count + date_seps_in_window) as f32;
        let timeline_lead = (viewport_height - total_height - gap_overhead).max(0.0);

        // ── Build windowed content column ──
        let mut col = Column::new()
            .spacing(SPACE_4)
            .width(Length::Fill)
            .align_x(Alignment::Start);

        if timeline_lead > 0.0 {
            col = col.push(
                space::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(timeline_lead)),
            );
        }

        if top_space_h > 0.0 {
            col = col.push(
                space::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(top_space_h)),
            );
        }

        let mut prev_day: Option<i64> = if first_idx > 0 {
            self.entries[first_idx - 1]
                .timestamp
                .map(|ts| ts / 86400000)
        } else {
            None
        };

        for i in first_idx..=last_idx {
            let entry = &self.entries[i];

            // ── Date divider ──
            let entry_day = crate::presentation::day_key(entry.timestamp);
            if let Some(day) = entry_day {
                if prev_day != Some(day) {
                    let today_day = crate::presentation::day_key(Some(now_ms())).unwrap_or(day);
                    let divider_label = crate::presentation::date_divider_label(
                        entry.timestamp.unwrap_or(0),
                        today_day,
                    );
                    col = col.push(crate::ui_components::date_separator(divider_label, &theme));
                }
                prev_day = Some(day);
            }

            let previous = i.checked_sub(1).map(|index| &self.entries[index]);
            let group_continues = previous.is_some_and(|previous| {
                let kind = |kind| match kind {
                    ChatKind::System => crate::presentation::MessageKind::System,
                    ChatKind::Local => crate::presentation::MessageKind::Local,
                    ChatKind::Remote => crate::presentation::MessageKind::Remote,
                };
                let previous_sender = previous.sender_key.map(|key| key.to_string());
                let current_sender = entry.sender_key.map(|key| key.to_string());
                crate::presentation::continues_message_group(
                    kind(previous.kind),
                    kind(entry.kind),
                    previous_sender.as_deref(),
                    current_sender.as_deref(),
                    previous.timestamp,
                    entry.timestamp,
                )
            });

            // Consecutive plain system notices (no download attachment — those
            // render as attachment cards, not chips) form a tight visual group:
            // their chip-to-chip gap is smaller than the spacing around user
            // message bubbles. Grouping is purely visual; entries are never
            // reordered or filtered based on display type.
            let system_group_continues = {
                let is_plain_system = |entry: &ChatEntry| {
                    matches!(entry.kind, ChatKind::System) && entry.download.is_none()
                };
                is_plain_system(entry) && previous.is_some_and(is_plain_system)
            };

            // Whether the NEXT entry continues this same visual group.
            // Used to show delivery state only on the last message of a group.
            let next_continues = i + 1 < total_entries && {
                let next = &self.entries[i + 1];
                let kind = |kind| match kind {
                    ChatKind::System => crate::presentation::MessageKind::System,
                    ChatKind::Local => crate::presentation::MessageKind::Local,
                    ChatKind::Remote => crate::presentation::MessageKind::Remote,
                };
                let current_sender = entry.sender_key.map(|key| key.to_string());
                let next_sender = next.sender_key.map(|key| key.to_string());
                crate::presentation::continues_message_group(
                    kind(entry.kind),
                    kind(next.kind),
                    current_sender.as_deref(),
                    next_sender.as_deref(),
                    entry.timestamp,
                    next.timestamp,
                )
            };

            // ── Local / Remote / System-with-download messages ──
            let label_color = match entry.kind {
                ChatKind::Local => text_local_label(&theme),
                ChatKind::Remote => text_remote_label(&theme),
                ChatKind::System => text_muted(&theme),
            };
            let body_color = match entry.kind {
                ChatKind::Local => text_local_body(&theme),
                ChatKind::Remote => text_remote_body(&theme),
                ChatKind::System => text_muted(&theme),
            };

            let label_text = entry.label_text.as_deref().unwrap_or(&entry.label);
            let is_friend_online = entry
                .sender_key
                .map_or(false, |k| self.peer_presence(&k) != PeerPresence::Offline);
            let label_el: iced::Element<'_, AppMessage> =
                if matches!(entry.kind, ChatKind::System) && entry.download.is_none() {
                    // System notices have no label — just the centred text
                    space::Space::new().height(0.0).into()
                } else if group_continues {
                    // No label inside a group — the inter-bubble gap is the
                    // plan's 6 px message-group gap.
                    space::Space::new().height(Length::Fixed(0.0)).into()
                } else if matches!(entry.kind, ChatKind::Remote) {
                    if let Some(sender_key) = entry.sender_key {
                        let status_icon = icon_svg(
                            if is_friend_online {
                                ICON_ONLINE
                            } else {
                                ICON_OFFLINE
                            },
                            TYPO_XXS,
                        )
                        .style(move |t, _| iced::widget::svg::Style {
                            color: Some(if is_friend_online {
                                accent_green(t)
                            } else {
                                Self::muted_color(false)
                            }),
                        });
                        button(
                            Row::new()
                                .push(status_icon)
                                .push(
                                    text(label_text)
                                        .size(crate::fonts::TypeRole::ChatSender.size_px())
                                        .font(crate::fonts::TypeRole::ChatSender.font())
                                        .color(label_color),
                                )
                                .spacing(SPACE_4)
                                .align_y(Alignment::Center),
                        )
                        .on_press(AppMessage::OpenPeerProfile(sender_key))
                        .padding(0)
                        .style(|_t, _s| iced::widget::button::Style::default())
                        .into()
                    } else {
                        text(label_text)
                            .size(crate::fonts::TypeRole::ChatSender.size_px())
                            .font(crate::fonts::TypeRole::ChatSender.font())
                            .color(label_color)
                            .into()
                    }
                } else {
                    // Local messages: make label clickable for retry when Failed
                    if matches!(entry.kind, ChatKind::Local)
                        && entry.delivery_state == DeliveryState::Failed
                    {
                        let event_id = entry.event_id;
                        button(
                            text(label_text)
                                .size(crate::fonts::TypeRole::ChatSender.size_px())
                                .font(crate::fonts::TypeRole::ChatSender.font())
                                .color(label_color),
                        )
                        .on_press(AppMessage::RetryOutgoingMessage(event_id))
                        .padding(0)
                        .style(|_t, _s| iced::widget::button::Style::default())
                        .into()
                    } else {
                        text(label_text)
                            .size(crate::fonts::TypeRole::ChatSender.size_px())
                            .font(crate::fonts::TypeRole::ChatSender.font())
                            .color(label_color)
                            .into()
                    }
                };

            // ── Clickable URL-aware body ──
            let segments = entry.parsed_segments.as_deref().unwrap_or(&[]);
            let body_el: iced::Element<'_, AppMessage> = if segments.len() == 1
                && matches!(&segments[0], link_preview::TextSegment::Text(_))
            {
                // No URLs — simple text element. `WordOrGlyph` wraps at word
                // boundaries and falls back to glyph-level breaking for
                // unbreakable words (public keys, pasted tokens, very long
                // single words) so the bubble never overflows its width cap.
                text(&entry.body)
                    .size(self.chat_text_size)
                    .font(crate::fonts::TypeRole::ChatMessage.font())
                    .line_height(iced::widget::text::LineHeight::Relative(1.45))
                    .wrapping(Wrapping::WordOrGlyph)
                    .color(body_color)
                    .into()
            } else {
                // Mixed text and URLs — build a segmented row
                let mut row = Row::new().spacing(0);
                for seg in segments {
                    match seg {
                        link_preview::TextSegment::Text(t) => {
                            row = row.push(
                                text(t)
                                    .size(self.chat_text_size)
                                    .font(crate::fonts::TypeRole::ChatMessage.font())
                                    .line_height(iced::widget::text::LineHeight::Relative(1.45))
                                    .wrapping(Wrapping::WordOrGlyph)
                                    .color(body_color),
                            );
                        }
                        link_preview::TextSegment::Url(u) => {
                            let display = link_preview::truncate_url(&u, 80);
                            let url_for_click = u.clone();
                            row = row.push(
                                button(
                                    text(display)
                                        .size(self.chat_text_size)
                                        .font(crate::fonts::TypeRole::ChatMessage.font())
                                        .line_height(iced::widget::text::LineHeight::Relative(1.45))
                                        .wrapping(Wrapping::WordOrGlyph)
                                        .color(accent_primary(&theme)),
                                )
                                .on_press(AppMessage::OpenUrl(url_for_click))
                                .padding(0)
                                .style(|_t, _s| iced::widget::button::Style::default()),
                            );
                        }
                    }
                }
                // Keep URL segments clickable while allowing the row to
                // create additional lines when the bubble reaches its
                // available width.
                row.wrap().into()
            };

            let bubble =
                container(body_el)
                    .padding([SPACE_10, SPACE_16])
                    .style(move |t: &iced::Theme| {
                        let mut s = iced::widget::container::Style {
                            border: crate::design_tokens::bubble_border(
                                t,
                                entry.kind == ChatKind::Local,
                                entry.kind == ChatKind::System,
                                matches!(entry.kind, ChatKind::Local)
                                    && entry.delivery_state == DeliveryState::Failed,
                            )
                            .unwrap_or_default(),
                            ..Default::default()
                        };
                        if let Some(bg) = bubble_bg(t, entry.kind) {
                            s.background = Some(bg);
                        }
                        s
                    });

            // Wrap non-system bubbles in a button so clicking copies the
            // message body to the clipboard with a toast confirmation.
            // Also wrap in a mouse_area so right-click opens a context menu.
            let clickable_bubble: iced::Element<'_, AppMessage> =
                if !matches!(entry.kind, ChatKind::System) && !entry.body.is_empty() {
                    let idx = i;
                    iced::widget::mouse_area(
                        button(bubble)
                            .on_press(AppMessage::CopyMessage(i))
                            .padding(0)
                            .style(|_t, _s| iced::widget::button::Style::default()),
                    )
                    .on_right_press(AppMessage::RightClickText(idx))
                    .into()
                } else {
                    bubble.into()
                };

            let ts_text = entry.formatted_time.as_deref().unwrap_or("");
            let metadata = if matches!(entry.kind, ChatKind::Local) && !next_continues {
                format!(
                    "{} · {}",
                    ts_text,
                    crate::presentation::delivery_label(&entry.delivery_state)
                )
            } else {
                ts_text.to_string()
            };
            let ts_el = text(metadata)
                .size(crate::fonts::TypeRole::ChatMetadata.size_px())
                .font(crate::fonts::TypeRole::ChatMetadata.font())
                .color(text_muted(&theme));

            let mut bubble_col = Column::new()
                .spacing(SPACE_2)
                .max_width(bubble_max_w)
                .width(Length::Fill)
                // Outgoing groups hug the right edge (avatar trailing), so
                // their bubble + timestamp align right inside the reserved
                // column; incoming groups hug the left edge.
                .align_x(if matches!(entry.kind, ChatKind::Local) {
                    iced::Alignment::End
                } else {
                    iced::Alignment::Start
                });
            // Skip the body bubble for image-only entries (empty body + image present)
            if entry.body.is_empty() && entry.image_handle.is_some() {
                bubble_col = bubble_col.push(ts_el);
            } else {
                bubble_col = bubble_col.push(clickable_bubble).push(ts_el);
            }

            // ── Link preview card ──
            if let Some(ref preview) = entry.link_preview {
                let mut preview_children: Vec<iced::Element<'_, AppMessage>> = Vec::new();
                if let Some(ref title) = preview.title {
                    preview_children.push(
                        text(title)
                            .size(TYPO_SM)
                            .font(crate::fonts::TypeRole::ChatMessage.font())
                            .wrapping(Wrapping::WordOrGlyph)
                            .color(accent_primary(&theme))
                            .into(),
                    );
                }
                if let Some(ref desc) = preview.description {
                    preview_children.push(
                        text(desc)
                            .size(TYPO_XS)
                            .font(crate::fonts::TypeRole::ChatMessage.font())
                            .wrapping(Wrapping::WordOrGlyph)
                            .color(text_muted(&theme))
                            .into(),
                    );
                }
                if let Some(ref bytes) = preview.image_bytes {
                    let handle = iced::widget::image::Handle::from_bytes(bytes.clone());
                    preview_children.push(
                        iced::widget::image(handle)
                            .width(Length::Fill)
                            .height(Length::Fixed(180.0))
                            .content_fit(iced::ContentFit::Cover)
                            .into(),
                    );
                } else if let Some(ref img_url) = preview.image_url {
                    let display_url = link_preview::truncate_url(img_url, 60);
                    preview_children.push(
                        text(display_url)
                            .size(TYPO_XXS)
                            .font(crate::fonts::TypeRole::ChatMessage.font())
                            .wrapping(Wrapping::WordOrGlyph)
                            .color(text_muted(&theme))
                            .into(),
                    );
                }
                if !preview_children.is_empty() {
                    let prv_url = preview.url.clone();
                    let preview_card = button(
                        container(
                            Column::new()
                                .push(
                                    text(link_preview::truncate_url(&preview.url, 60))
                                        .size(TYPO_XXS)
                                        .font(crate::fonts::TypeRole::ChatMessage.font())
                                        .color(text_muted(&theme)),
                                )
                                .push(Column::with_children(preview_children).spacing(SPACE_2))
                                .spacing(SPACE_2),
                        )
                        .padding([SPACE_6, SPACE_8])
                        .width(Length::Fill)
                        .style(container_card),
                    )
                    .on_press(AppMessage::OpenUrl(prv_url))
                    .padding(0)
                    .style(|_t, _s| iced::widget::button::Style::default());
                    bubble_col = bubble_col.push(preview_card);
                }
            } else if entry.link_preview_loading {
                bubble_col = bubble_col.push({
                    const SP: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    let s = SP[self.splash_spinner_frame % SP.len()];
                    text(format!("{s} Loading preview…"))
                        .size(TYPO_XS)
                        .font(crate::fonts::TypeRole::ChatMessage.font())
                        .color(text_muted(&theme))
                });
            }

            // ── Avatar column ──
            // UI-14 rule: the sender avatar appears once per visual group, on
            // the group's FIRST bubble.  Subsequent bubbles in the same group
            // reserve the same-width slot so every bubble in the group shares
            // one edge.  The avatar sits at the leading edge for incoming
            // groups (left) and the trailing edge for outgoing groups (right).
            let avatar_el: iced::Element<'_, AppMessage> = if group_continues {
                space::Space::new()
                    .width(Length::Fixed(AVATAR_SM))
                    .height(Length::Fixed(AVATAR_SM))
                    .into()
            } else if let Some(ref handle) = entry.avatar_handle {
                iced::widget::image(handle.clone())
                    .content_fit(iced::ContentFit::ScaleDown)
                    .width(Length::Fixed(AVATAR_SM))
                    .height(Length::Fixed(AVATAR_SM))
                    .into()
            } else {
                // Coloured circle fallback with the sender's initial, so an
                // entry without a profile image never renders a bare "?".
                let name = entry.label.as_str();
                let initial = name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_string());
                let dark = matches!(self.theme(), iced::Theme::Dark);
                let letter_color = crate::presentation::initials_color(name, dark);
                container(text(initial).size(TYPO_SM).color(letter_color))
                    .width(Length::Fixed(AVATAR_SM))
                    .height(Length::Fixed(AVATAR_SM))
                    .center_x(Length::Fixed(AVATAR_SM))
                    .center_y(Length::Fixed(AVATAR_SM))
                    .style(|t| iced::widget::container::Style {
                        background: Some(iced::Background::Color(bg_surface_secondary(t))),
                        border: iced::Border {
                            radius: (AVATAR_SM / 2.0).into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into()
            };

            let msg_row = match entry.kind {
                ChatKind::Remote => Row::new()
                    .push(avatar_el)
                    .push(bubble_col)
                    .align_y(iced::Alignment::Center)
                    .spacing(SPACE_8),
                ChatKind::Local => Row::new()
                    .push(bubble_col)
                    .push(avatar_el)
                    .align_y(iced::Alignment::Center)
                    .spacing(SPACE_8),
                // System entries with a download attachment render the
                // download card directly (upload progress, received file).
                // Plain text system events render as small centred notices.
                ChatKind::System => {
                    let download = entry
                        .download
                        .as_ref()
                        .map(|dl| self.view_download_attachment(i, dl, timeline_width));
                    if let Some(dl_el) = download {
                        // Right padding keeps the card clear of the
                        // scrollable's overlay scrollbar (~12 px).
                        Row::new()
                            .push(dl_el)
                            .width(Length::Shrink)
                            .padding(iced::Padding::default().right(SPACE_12))
                    } else {
                        // UI-29: plain, subtle inline text for system notices.
                        // No bubble surface, no label chip, no icon slot — just
                        // the muted message, centred like the date separators,
                        // so it reads as a system annotation rather than a
                        // participant message.
                        Row::new()
                            .push(
                                container(
                                    text(&entry.body)
                                        .size(TYPO_XS)
                                        .font(crate::fonts::TypeRole::ChatMessage.font())
                                        .color(text_muted(&theme))
                                        .wrapping(Wrapping::WordOrGlyph),
                                )
                                .width(Length::Fill)
                                .center_x(Length::Fill)
                                .max_width(720.0)
                                .padding([0.0, SPACE_12]),
                            )
                            .width(Length::Fill)
                    }
                }
            }
            .width(Length::Fill);

            // CHAT-02: anchor the sender name to the same side as the message
            // body. The wrapping column defaults to align_x(Start), which
            // pinned own-message usernames to the LEFT edge while their bubble
            // hugged the right. Own messages anchor the label right (End),
            // received/system entries keep it left (Start) — the same side as
            // their bubble, regardless of message length.
            let label_align = if matches!(entry.kind, ChatKind::Local) {
                iced::Alignment::End
            } else {
                iced::Alignment::Start
            };
            col = col.push(
                Column::new()
                    .push(label_el)
                    .push(msg_row)
                    .align_x(label_align)
                    .spacing(
                        if system_group_continues {
                            // Consecutive system chips are grouped tightly: the
                            // gap between chips is smaller than the spacing
                            // around user message bubbles (normal spacing
                            // below).
                            SPACE_2
                        } else if group_continues {
                            // 6 px gap between bubbles inside one sender group
                            // (plan §4).
                            SPACE_6
                        } else if matches!(entry.kind, ChatKind::System) {
                            SPACE_4
                        } else {
                            // 18 px group-to-group gap between different sender
                            // groups (plan §4).
                            SPACE_18
                        },
                    ),
            );

            // ── Image card header (PAPIRUS-10) ────────────────────────────
            // Image messages carry the central Papirus image-type icon beside
            // the filename in the card header; the preview itself stays the
            // main visual.  Live entries drop the original filename from the
            // protocol (body is empty), so the header shows a generic "Image"
            // label; history replay restores the stored filename into `body`.
            let is_image_entry = entry.image_bytes.is_some()
                || entry.image_identifier.is_some()
                || entry.image_error.is_some()
                || entry.gif_frames.is_some();
            if is_image_entry {
                let icon_name = if !entry.body.is_empty() {
                    entry.body.clone()
                } else if let Some(id) = entry.image_identifier.as_deref() {
                    id.rsplit('/').next().unwrap_or("image").to_string()
                } else {
                    "image".to_string()
                };
                let header_label = if entry.body.is_empty() {
                    "Image".to_string()
                } else {
                    entry.body.clone()
                };
                let image_header = Row::new()
                    .push(crate::download_progress_view::file_type_icon_element_with_tooltip(
                        &icon_name,
                        None,
                        None,
                        crate::file_type_icon::FileTypeIconSize::List,
                        &theme,
                    ))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            header_label,
                        )
                        .color(text_muted(&theme)),
                    )
                    .spacing(SPACE_6)
                    .align_y(Alignment::Center);
                col = col.push(image_header);
            }

            // ── Image / animated GIF (decoded once at construction) ──
            // Display size is computed by the shared helper used by the
            // LayoutCache too, so the rendered box always matches the
            // cached height — that is what keeps the scrollbar stable as
            // images enter the window and prevents decode-driven reflow.
            let (display_w, display_h) = chat_image_display_size(entry);

            if let Some(frames) = entry.gif_frames.as_deref() {
                // Animated GIF: the iced-moving-picture Gif widget manages its
                // own state tree and advances frames via per-frame delays +
                // request_redraw_at, so each GIF animates independently at the
                // correct speed (no global 100ms tick, no PNG re-encode).
                let img = iced_moving_picture::widget::gif::Gif::new(frames)
                    .content_fit(iced::ContentFit::ScaleDown)
                    .width(Length::Fixed(display_w))
                    .height(Length::Fixed(display_h));
                // Keep the preview edge consistent.
                let framed = container(img)
                    .width(Length::Fixed(display_w))
                    .height(Length::Fixed(display_h))
                    .style(|t| iced::widget::container::Style {
                        border: iced::Border {
                            color: border_muted(t),
                            width: 1.0,
                            radius: ATTACHMENT_RADIUS.into(),
                        },
                        ..Default::default()
                    });
                let thumbnail = iced::widget::button(framed)
                    .on_press(AppMessage::OpenImageLightbox(i))
                    .padding(0)
                    .style(|_t, _s| iced::widget::button::Style::default());
                let thumb_with_right_click = iced::widget::mouse_area(thumbnail)
                    .on_right_press(AppMessage::RightClickImage(i));
                col = col.push(thumb_with_right_click);
            } else if let Some(handle) = self.image_handle_for_entry(entry) {
                let img = iced::widget::image(handle)
                    .content_fit(iced::ContentFit::ScaleDown)
                    .width(Length::Fixed(display_w))
                    .height(Length::Fixed(display_h));
                // Keep the preview edge consistent.
                let framed = container(img)
                    .width(Length::Fixed(display_w))
                    .height(Length::Fixed(display_h))
                    .style(|t| iced::widget::container::Style {
                        border: iced::Border {
                            color: border_muted(t),
                            width: 1.0,
                            radius: ATTACHMENT_RADIUS.into(),
                        },
                        ..Default::default()
                    });
                let thumbnail = iced::widget::button(framed)
                    .on_press(AppMessage::OpenImageLightbox(i))
                    .padding(0)
                    .style(|_t, _s| iced::widget::button::Style::default());
                let thumb_with_right_click = iced::widget::mouse_area(thumbnail)
                    .on_right_press(AppMessage::RightClickImage(i));
                col = col.push(thumb_with_right_click);
            } else if entry.image_error.is_some() || entry.image_identifier.is_some() {
                use iced::widget::{container, Column};
                let error_text = entry
                    .image_error
                    .as_deref()
                    .unwrap_or("Image preview unavailable");
                // PAPIRUS-10: the image-unavailable placeholder uses the
                // central Papirus image icon (Large) as its main visual —
                // no emoji as a file-type icon.
                let icon_name = if !entry.body.is_empty() {
                    entry.body.clone()
                } else if let Some(id) = entry.image_identifier.as_deref() {
                    id.rsplit('/').next().unwrap_or("image").to_string()
                } else {
                    "image".to_string()
                };
                let placeholder = Column::new()
                    .push(crate::download_progress_view::file_type_icon_element_with_tooltip(
                        &icon_name,
                        None,
                        None,
                        crate::file_type_icon::FileTypeIconSize::Large,
                        &theme,
                    ))
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::SupportingText,
                            "Image unavailable",
                        )
                        .color(text_system(&theme)),
                    )
                    .push(
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            error_text,
                        )
                        .color(color_error(&theme))
                        .wrapping(Wrapping::WordOrGlyph),
                    )
                    .spacing(SPACE_2);
                // The placeholder occupies the SAME fixed box the decoded
                // image will use (display_w × display_h), so the entry
                // height never changes when the image hydrates/decodes —
                // a variable-height placeholder reflowed the windowed list
                // and made images jitter while loading.
                col = col.push(
                    container(placeholder)
                        .width(Length::Fixed(display_w))
                        .height(Length::Fixed(display_h))
                        .center_x(Length::Fill)
                        .center_y(Length::Fill)
                        .padding([SPACE_8, SPACE_10])
                        .style(container_card),
                );
            }

            // ── Reactions ──
            if let Some(ref reactions_text) = entry.reactions_text {
                let reactions_line = Row::new()
                    .push(
                        text(reactions_text)
                            .color(text_muted(&theme))
                            .size(TYPO_SM)
                            .font(crate::fonts::TypeRole::ChatMessage.font())
                            .wrapping(Wrapping::WordOrGlyph)
                            .width(Length::Fill),
                    )
                    .spacing(0)
                    .padding([0.0, SPACE_8])
                    .width(Length::Fill);
                col = col.push(reactions_line);
            }
        }

        if let Some(filename) = &self.pending_image_upload {
            const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let spinner = SPINNER_FRAMES[self.image_upload_spinner_frame % SPINNER_FRAMES.len()];
            col = col.push(
                container(
                    Row::new()
                        .push(text(spinner).size(TYPO_LG).color(text_muted(&theme)))
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::SupportingText,
                                format!("Processing {filename}…"),
                            )
                            .color(text_muted(&theme)),
                        )
                        .spacing(SPACE_8)
                        .align_y(iced::Alignment::Center),
                )
                .padding([SPACE_8, SPACE_10])
                .style(container_card),
            );
        }

        if let Some((filename, file_size)) = &self.pending_file_upload {
            const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let spinner = SPINNER_FRAMES[self.file_upload_spinner_frame % SPINNER_FRAMES.len()];
            let size_label = {
                let bytes = *file_size;
                const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
                let mut value = bytes as f64;
                let mut unit_idx = 0usize;
                while value >= 1024.0 && unit_idx < UNITS.len() - 1 {
                    value /= 1024.0;
                    unit_idx += 1;
                }
                if unit_idx == 0 {
                    format!("{bytes} {} ", UNITS[unit_idx])
                } else {
                    format!("{value:.1} {} ", UNITS[unit_idx])
                }
            };
            col = col.push(
                container(
                    Row::new()
                        .push(text(spinner).size(TYPO_LG).color(text_muted(&theme)))
                        .push(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::SupportingText,
                                format!("Uploading {filename} ({size_label})…"),
                            )
                            .color(text_muted(&theme)),
                        )
                        .spacing(SPACE_8)
                        .align_y(iced::Alignment::Center),
                )
                .padding([SPACE_8, SPACE_10])
                .style(container_card),
            );
        }

        // Bottom spacer
        // Bottom spacer (precomputed by layout cache)
        if bottom_h > 0.0 {
            col = col.push(
                container(space::Space::new().height(Length::Fixed(bottom_h))).width(Length::Fill),
            );
        }

        // ── Record render perf metrics ──
        let window_size = if total_entries > 0 {
            last_idx.saturating_sub(first_idx) + 1
        } else {
            0
        };
        self.perf.replace(PerfMetrics {
            last_render_time_ns: _start.elapsed().as_nanos() as u64,
            window_size,
            total_entries,
            total_image_bytes,
            image_entry_count,
        });

        // Top-anchored scrollable: `scroll_offset` (mirrored from the Scrolled
        // event) is a top-relative offset, which matches the windowed layout
        // cache exactly.  When following the latest message the app snaps the
        // scrollable back to the bottom via `scroll_to_bottom_pending`; when
        // the user has scrolled up, a top anchor keeps the reading position
        // fixed while live entries append below the viewport.  The empty-state
        // scrollable above keeps `anchor_bottom` because it has no content.
        crate::ui_components::gutter_scrollable(col)
            .id(CHAT_LOG)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .on_scroll(|v: scrollable::Viewport| {
                AppMessage::Scrolled(v.absolute_offset().y, v.bounds().height)
            })
    }

    pub(crate) fn view_composer(&self) -> iced::Element<'_, AppMessage> {
        use crate::design_tokens::{RADIUS_XL, SPACE_8};
        use iced::widget::{button, container, row, text, text_input};
        use iced::{Alignment, Length, Padding};

        let has_text = !self.composer_text.is_empty();
        // A send in flight wins over the empty-text appearance: the button
        // shows a clear "sending" state until the broadcast task completes.
        let sending = self.composer_sending;

        // ── Attach button (paperclip icon) ── leading edge, left of input
        // Tooltip label so the icon-only control is identifiable without
        // relying on the glyph alone (UI-19).
        let attach_btn: iced::Element<'_, AppMessage> =
            iced::widget::tooltip::Tooltip::new(
                button(icon_svg(ICON_PAPERCLIP, TYPO_SM))
                    .on_press(AppMessage::AttachPressed)
                    .style(BUTTON_ICON)
                    .padding([SPACE_4, SPACE_6]),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Attach a file"),
                iced::widget::tooltip::Position::Bottom,
            )
            .into();

        // ── Folder button (folder icon) ── whole-directory share (SENDME-01)
        let folder_btn: iced::Element<'_, AppMessage> =
            iced::widget::tooltip::Tooltip::new(
                button(icon_svg(ICON_FOLDER, TYPO_SM))
                    .on_press(AppMessage::AttachFolderPressed)
                    .style(BUTTON_ICON)
                    .padding([SPACE_4, SPACE_6]),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Share a folder"),
                iced::widget::tooltip::Position::Bottom,
            )
            .into();

        // ── Center: expandable message input ── transparent bg, fills space
        let input = text_input("Type a message…", &self.composer_text)
            .id(COMPOSER_INPUT)
            .on_input(AppMessage::InputChanged)
            .on_submit(AppMessage::SendPressed)
            .size(self.chat_text_size)
            .font(crate::fonts::TypeRole::ComposerText.font())
            .width(Length::Fill)
            .padding(Padding::new(SPACE_8))
            .style(
                move |t: &iced::Theme, status: iced::widget::text_input::Status| {
                    let is_focused =
                        matches!(status, iced::widget::text_input::Status::Focused { .. });
                    iced::widget::text_input::Style {
                        background: iced::Background::Color(iced::Color::TRANSPARENT),
                        border: iced::Border {
                            // UI-19: focus ring uses the shared focus token
                            // (2 px, plan §4) so keyboard focus is visible on
                            // the composer exactly like every other input.
                            color: if is_focused {
                                crate::design_tokens::color_focus(t)
                            } else {
                                iced::Color::TRANSPARENT
                            },
                            width: if is_focused {
                                crate::design_tokens::FOCUS_WIDTH
                            } else {
                                0.0
                            },
                            radius: RADIUS_XL.into(),
                        },
                        icon: iced::Color::TRANSPARENT,
                        placeholder: crate::design_tokens::text_muted(t),
                        value: crate::design_tokens::text(t),
                        selection: accent_primary(t),
                    }
                },
            );

        // ── GIF picker toggle button ── trailing actions, after input
        let gif_btn = button(crate::fonts::type_role_text(
            crate::fonts::TypeRole::ButtonLabel,
            "GIF",
        ))
            .on_press(AppMessage::ToggleGifPicker)
            .style(BUTTON_ICON)
            .padding([SPACE_4, SPACE_6]);

        // ── Emoji picker toggle button ── next to GIF
        let emoji_btn: iced::Element<'_, AppMessage> =
            iced::widget::tooltip::Tooltip::new(
                button(Icon::Smile.build().size(IconSize::Sm).build())
                    .on_press(AppMessage::ToggleEmojiPicker)
                    .style(BUTTON_ICON)
                    .padding([SPACE_4, SPACE_6]),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Emoji"),
                iced::widget::tooltip::Position::Bottom,
            )
            .into();

        // ── Right: circular green send button ──
        //  * empty composer → muted transparent circle (disabled)
        //  * text present   → filled accent-green circle with send icon
        //  * send in flight → filled green circle with a brief spinner glyph
        // The shortcut (Enter to send) is documented in the help overlay; the
        // circular affordance matches Figure 4.
        let send_btn = button(
            if sending {
                iced::Element::from(text("…").size(TYPO_MD))
            } else {
                iced::Element::from(
                    icon_svg(ICON_SEND, 18.0)
                        .style(|_t, _s| iced::widget::svg::Style {
                            color: Some(iced::Color::WHITE),
                        }),
                )
            },
        )
        .width(Length::Fixed(SPACE_18 * 2.0))
        .height(Length::Fixed(SPACE_18 * 2.0))
        .padding(0)
        .style(move |t: &iced::Theme, status: iced::widget::button::Status| {
            if sending {
                // Sending: keep the green fill but dim it and disable press.
                let mut s = BUTTON_PRIMARY_GREEN(t, iced::widget::button::Status::Disabled);
                s.border.radius = SPACE_18.into();
                s
            } else if has_text {
                let mut s = BUTTON_PRIMARY_GREEN(t, status);
                s.border.radius = SPACE_18.into();
                s
            } else {
                // Disabled: transparent circle with a muted send icon.
                let mut s = BUTTON_MUTED(t, iced::widget::button::Status::Disabled);
                s.background = None;
                s.text_color = crate::design_tokens::text_muted(t);
                s.border.radius = SPACE_18.into();
                s
            }
        });
        let send_btn = if sending || !has_text {
            send_btn
        } else {
            send_btn.on_press(AppMessage::SendPressed)
        };
        // Tooltip label so the icon-only send control is identifiable
        // without relying on the glyph alone (UI-19). Enter also sends.
        let send_btn: iced::Element<'_, AppMessage> =
            iced::widget::tooltip::Tooltip::new(
                send_btn,
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Send (Enter)"),
                iced::widget::tooltip::Position::Bottom,
            )
            .into();

        // ── Composer row ──
        //  attach | text input (fill) | gif | emoji | send
        let composer_bar = row![attach_btn, folder_btn, input, gif_btn, emoji_btn, send_btn]
            .spacing(SPACE_6)
            .align_y(Alignment::Center)
            .padding(Padding::new(SPACE_4));

        // ── Elevated rounded composer container ──
        //  16 px radius surface with a 1 px border and a very subtle shadow
        //  (plan §4: composer elevation ~0 1 2).  While a window file is
        //  dragged over the app the border adopts the accent colour as a
        //  subtle focus treatment (file-drop routes through the same
        //  attachment pipeline).
        container(composer_bar)
            .width(Length::Fill)
            .padding(Padding::new(0.0))
            .style(move |t: &iced::Theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(t))),
                border: iced::Border {
                    width: 1.0,
                    color: if self.composer_drag_over {
                        accent_primary(t)
                    } else {
                        border_muted(t)
                    },
                    radius: RADIUS_XL.into(),
                },
                shadow: crate::design_tokens::shadow_card(t),
                ..Default::default()
            })
            .into()
    }

    pub(crate) fn view_help(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, container, text, Column, Space};
        use iced::{Alignment, Length};

        // ── Header: title + accessible close button ──
        let header = iced::widget::row![
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, "Help")
                .width(Length::Fill),
            button(crate::fonts::type_role_text(
                crate::fonts::TypeRole::ButtonLabel,
                "Close",
            ))
            .on_press(AppMessage::ToggleHelp)
            .padding(SPACE_4)
            .style(BUTTON_GHOST),
        ]
        .align_y(Alignment::Center)
        .spacing(SPACE_8);

        // ── Command reference sections ──
        let commands = Column::new()
            .spacing(SPACE_6)
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::SupportingText,
                "── Commands ──",
            )
            .style(text_muted_style))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/send <path>    Share a file with peers",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/image <path>   Share an image inline",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/download       Fetch the last shared file",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/leave          Leave this room and delete from history",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/help           Toggle this menu",
            ))
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::SupportingText,
                "── Friends ──",
            )
            .style(text_muted_style))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/friend add <pk> [alias]  Track a friend's online status",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/friend remove <pk|alias> Stop tracking a friend",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/friend list    List tracked friends and their status",
            ))
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::SupportingText,
                "── Messages ──",
            )
            .style(text_muted_style))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/react <idx> <emoji>  Add a reaction to a message",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/edit <idx> <text>   Edit a message",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "/delete <idx>        Delete a message",
            ))
            .push(Space::new().height(Length::Fixed(SPACE_4)))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::SupportingText,
                "── Tips ──",
            )
            .style(text_muted_style))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "Type a message and press Enter to send.",
            ))
            .push(crate::fonts::type_role_text(
                crate::fonts::TypeRole::Body,
                "Click Remove on a room in the chat list to remove it.",
            ));

        // ── Footer ──
        let report_bug_btn = button(crate::fonts::type_role_text(
            crate::fonts::TypeRole::ButtonLabel,
            "Report Bug",
        ))
            .on_press(AppMessage::ReportBug)
            .padding([SPACE_6, SPACE_12])
            .style(|t, status| iced::widget::button::Style {
                background: Some(iced::Background::Color(bg_surface(t))),
                border: iced::Border {
                    color: border_muted(t),
                    width: 1.0,
                    radius: SPACE_8.into(),
                },
                text_color: text_muted_style(t)
                    .color
                    .unwrap_or(iced::Color::from_rgb(0.6, 0.6, 0.6)),
                ..Default::default()
            });

        let footer = Column::new()
            .push(report_bug_btn)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(
                text("Press Esc to close")
                    .size(crate::fonts::TypeRole::SupportingText.size_px())
                    .style(text_muted_style),
            );

        let dialog_content = Column::new()
            .push(header)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(commands)
            .push(Space::new().height(Length::Fixed(SPACE_8)))
            .push(footer)
            .spacing(SPACE_4)
            .padding(SPACE_24)
            .width(Length::Fill);

        container(
            crate::ui_components::gutter_scrollable(dialog_content)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| iced::widget::container::Style::default())
        .into()
    }
}
