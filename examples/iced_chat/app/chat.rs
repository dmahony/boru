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
        let mut content = widget::column![self.view_chat_header(), divider(&self.theme())];
        #[cfg(feature = "screen-sharing")]
        {
            content = content.push(self.view_screen_share_panel());
        }
        let content = content
        .push(widget::responsive(|size: iced::Size| {
            self.view_chat_log(size.width, size.height).into()
        }))
        .push(self.view_composer())
        .push(widget::Space::new().height(Length::Fixed(SPACE_8)))
        .push(self.view_chat_footer())
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

    #[cfg(feature = "screen-sharing")]
    /// Conversation-local screen-share controls. Playback pauses when the
    /// viewer navigates away; no media is retained in the background.
    pub(crate) fn view_screen_share_panel(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text};
        use iced::Length;
        let body = if let Some((inviter, _)) = &self.screen_share_invite {
            column![
                text(format!("{inviter} wants to share their screen")),
                row![
                    button(text("Accept")).on_press(AppMessage::AcceptScreenShare),
                    button(text("Decline")).on_press(AppMessage::DeclineScreenShare),
                ].spacing(SPACE_8),
            ].spacing(SPACE_6)
        } else if self.screen_share_host_state != ScreenShareHostState::Idle {
            let state = match self.screen_share_host_state {
                ScreenShareHostState::Inviting => "Waiting for the viewer to accept…",
                ScreenShareHostState::Streaming => "Screen sharing active",
                ScreenShareHostState::Idle => unreachable!(),
            };
            let mut items: Vec<iced::Element<'_, AppMessage>> = vec![text(state).into()];
            // Explicit consent prompt: the host picks the granted capabilities.
            if let Some((_, viewer, capabilities)) = &self.screen_share_control_request {
                let caps = capabilities
                    .iter()
                    .map(Self::capability_label)
                    .collect::<Vec<_>>()
                    .join(", ");
                items.push(text(format!("{viewer} requests control: {caps}")).into());
                items.push(
                    row![
                        button(text("Grant Pointer"))
                            .on_press(AppMessage::ScreenShareGrantControl(vec![
                                Capability::ControlPointer,
                            ])),
                        button(text("Grant Pointer + Keyboard"))
                            .on_press(AppMessage::ScreenShareGrantControl(vec![
                                Capability::ControlPointer,
                                Capability::ControlKeyboard,
                            ])),
                        button(text("Deny")).on_press(AppMessage::ScreenShareDenyControl),
                    ].spacing(SPACE_8).into(),
                );
            }
            if self.screen_share_control_active {
                items.push(
                    text("Remote control active — the viewer is driving the pointer/keyboard")
                        .into(),
                );
                items.push(
                    button(text("Revoke Control"))
                        .on_press(AppMessage::ScreenShareRevokeControl)
                        .into(),
                );
            }
            items.push(button(text("Stop Sharing")).on_press(AppMessage::StopScreenShare).into());
            column(items).spacing(SPACE_6)
        } else if self.screen_share_viewing {
            let video: iced::Element<'_, AppMessage> = if let Some(handle) = &self.screen_share_frame_handle {
                if self.screen_share_control_active {
                    // Fixed 640x360 box matches the capture aspect exactly, so
                    // the mouse_area Point maps 1:1 to normalized coordinates.
                    let image = iced::widget::Image::new(handle.clone())
                        .width(Length::Fixed(Self::SCREEN_SHARE_VIDEO_W))
                        .height(Length::Fixed(Self::SCREEN_SHARE_VIDEO_H))
                        .content_fit(iced::ContentFit::Contain);
                    let last = self.screen_share_last_pointer_pos.unwrap_or((0.0, 0.0));
                    iced::widget::mouse_area(image)
                        .on_move(|pos| AppMessage::ScreenSharePointerMove {
                            x: (pos.x / Self::SCREEN_SHARE_VIDEO_W).clamp(0.0, 1.0),
                            y: (pos.y / Self::SCREEN_SHARE_VIDEO_H).clamp(0.0, 1.0),
                        })
                        .on_press(AppMessage::ScreenSharePointerButton {
                            x: last.0,
                            y: last.1,
                            button: 1,
                            pressed: true,
                        })
                        .on_release(AppMessage::ScreenSharePointerButton {
                            x: last.0,
                            y: last.1,
                            button: 1,
                            pressed: false,
                        })
                        .into()
                } else {
                    iced::widget::Image::new(handle.clone())
                        .width(Length::Fill)
                        .height(Length::Fixed(if self.screen_share_fullscreen { 480.0 } else { 240.0 }))
                        .content_fit(iced::ContentFit::Contain)
                        .into()
                }
            } else {
                text("Waiting for the next decoded frame…").into()
            };
            let mut actions: Vec<iced::Element<'_, AppMessage>> = vec![
                button(text(if self.screen_share_fullscreen { "Inline" } else { "Fullscreen" }))
                    .on_press(AppMessage::ToggleScreenShareFullscreen)
                    .into(),
            ];
            if self.screen_share_control_active {
                actions.push(
                    text("Control granted — move the mouse or type to control the host").into(),
                );
            } else {
                actions.push(
                    button(text("Request Control"))
                        .on_press(AppMessage::ScreenShareRequestControl)
                        .into(),
                );
            }
            actions.push(button(text("Stop Viewing")).on_press(AppMessage::StopScreenShare).into());
            column![
                video,
                row(actions).spacing(SPACE_8),
            ].spacing(SPACE_6)
        } else {
            return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
        };
        container(body)
            .padding(SPACE_8)
            .width(Length::Fill)
            .style(|t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg_surface_secondary(t))),
                border: iced::Border { color: border_muted(t), width: 1.0, radius: SPACE_8.into() },
                ..Default::default()
            })
            .into()
    }

    #[cfg(feature = "screen-sharing")]
    /// Human label for a control capability (consent prompt).
    fn capability_label(capability: &Capability) -> String {
        match capability {
            Capability::ControlPointer => "pointer".to_string(),
            Capability::ControlKeyboard => "keyboard".to_string(),
            _ => "other".to_string(),
        }
    }

    #[cfg(feature = "screen-sharing")]
    /// Fixed viewer video box size (matches the 640x360 capture aspect so the
    /// mouse_area Point maps 1:1 to normalized coordinates).
    const SCREEN_SHARE_VIDEO_W: f32 = 640.0;

    #[cfg(feature = "screen-sharing")]
    const SCREEN_SHARE_VIDEO_H: f32 = 360.0;

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

        #[cfg(feature = "screen-sharing")]
        let screen_share: iced::Element<'_, AppMessage> = peer
            .filter(|_| {
                !is_group && !is_blocked && self.screen_share_host_state == ScreenShareHostState::Idle
            })
            .map(|key| tool_btn(
                Icon::Monitor.build().size(IconSize::Sm).build().into(),
                "Share screen",
                Some(AppMessage::StartScreenShare(key)),
            ))
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
        let mut toolbar = row![voice_call, video_call];
        #[cfg(feature = "screen-sharing")]
        {
            toolbar = toolbar.push(screen_share);
        }
        let toolbar = toolbar
            .push(search)
            .push(delete)
            .push(copy)
            .push(share)
            .push(overflow)
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
    /// State-layer update for the active-room chat surface (BORU-AUDIT-22 spec step 5).
    ///
    /// Handles composer input/send, attach flows, chat options/search,
    /// clear conversation, details/member panels, the whisper invite menu
    /// and the slash-command (react/edit/delete/whisper) arms. The root
    /// `update()` dispatches these variants here via combined match arms.
    pub(crate) fn update_chat(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::InputChanged(text) => {
                self.composer_text = text;

                // SetComposerText completes only after the normal input path
                // has updated the actual composer state.
                if let Some((action_id, expected)) = self.pending_set_composer_action.take() {
                    if self.composer_text == expected {
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::AppMessageHandled);
                        let _ = self
                            .gui_action_history
                            .set_state(&action_id, GuiActionState::Completed);
                    } else {
                        self.pending_set_composer_action = Some((action_id, expected));
                    }
                }

                iced::Task::none()
            }

            AppMessage::SendPressed => {
                // Never send while an input-method composition is active: the
                // Enter that confirms the composition must not also submit the
                // message.  The IME state is tracked from `InputMethod` window
                // events (see the event subscription).
                if self.composer_ime_active {
                    return iced::Task::none();
                }
                let trimmed = self.composer_text.trim().to_string();
                if trimmed.is_empty() {
                    return iced::Task::none();
                }
                self.composer_text.clear();

                if let Some(path) = trimmed.strip_prefix("/send ") {
                    let path = path.trim().to_string();
                    return iced::Task::perform(
                        async move {
                            let path_buf = std::path::PathBuf::from(&path);
                            let abs_path = std::path::absolute(&path_buf)
                                .map_err(|_| format!("Invalid path: {path}"))?;
                            if !abs_path.exists() {
                                return Err(format!("File not found: {path}"));
                            }
                            let filename = path_buf
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            if filename.is_empty() {
                                return Err("Invalid file path.".to_string());
                            }
                            Ok(format!("{filename}|{}|{path}", abs_path.display()))
                        },
                        |r: Result<String, String>| match r {
                            Ok(v) => AppMessage::ExecuteFileSend(v),
                            Err(e) => AppMessage::ErrorMsg(e),
                        },
                    );
                }

                if let Some(path) = trimmed.strip_prefix("/image ") {
                    let path = path.trim().to_string();
                    return iced::Task::perform(
                        async move {
                            let path_buf = std::path::PathBuf::from(&path);
                            let abs_path = std::path::absolute(&path_buf)
                                .map_err(|_| format!("Invalid path: {path}"))?;
                            if !abs_path.exists() {
                                return Err(format!("File not found: {path}"));
                            }
                            let filename = path_buf
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            if filename.is_empty() {
                                return Err("Invalid file path.".to_string());
                            }
                            Ok(format!("{filename}|{}|{path}", abs_path.display()))
                        },
                        |r: Result<String, String>| match r {
                            Ok(v) => AppMessage::ExecuteImageSend(v),
                            Err(e) => AppMessage::ErrorMsg(e),
                        },
                    );
                }

                if trimmed == "/download" {
                    return iced::Task::done(AppMessage::ExecuteDownload);
                }
                if trimmed == "/help" {
                    self.help_visible = !self.help_visible;
                    return iced::Task::none();
                }
                if trimmed == "/settings" {
                    self.settings_return_to = Some(self.screen.clone());
                    self.screen = Screen::Settings;
                    return iced::Task::none();
                }

                // ── Leave room / delete from history ──
                if trimmed == "/leave" {
                    let topic = self.topic;
                    // Broadcast Leave (best-effort)
                    if let Some(ref sender) = self.sender {
                        if let Ok(encoded) =
                            SignedMessage::sign_and_encode(&self.secret_key, &crate::Message::Leave)
                        {
                            let sender = sender.clone();
                            task::spawn(async move {
                                sender.broadcast(encoded).await.ok();
                            });
                        }
                    }
                    // Remove room and chat history (not just go back — delete it)
                    self.room_history.remove(&topic);
                    self.room_history_dirty = true;
                    self.chat_history.lock().unwrap().remove_topic(&topic);
                    self.persist_room_history();
                    // Leave the room and go back to chat list
                    self.leave_current_room();
                    self.screen = Screen::ChatList;
                    return iced::Task::none();
                }

                // ── Friend commands ──────────────────
                if let Some(pubkey_str) = trimmed.strip_prefix("/friend add ") {
                    let pubkey_str = pubkey_str.trim().to_string();
                    let (key_part, alias) = if let Some((key_part, rest)) =
                        pubkey_str.split_once(char::is_whitespace)
                    {
                        (key_part.to_string(), Some(rest.trim().to_string()))
                    } else {
                        (pubkey_str, None)
                    };
                    let mgr = self.friend_mgr.clone();
                    // Parse key and lookup address outside async block (avoids capturing self)
                    let peer = key_part.parse::<PublicKey>().ok();
                    let addr = peer.as_ref().and_then(|p| {
                        let fid = FriendId::from_public_key(*p);
                        self.friends
                            .get(&fid)
                            .and_then(|record| record.known_addrs.first().cloned())
                    });
                    return iced::Task::perform(
                        async move {
                            match peer {
                                Some(peer) => {
                                    let fid = FriendId::from_public_key(peer);
                                    let label = alias
                                        .clone()
                                        .unwrap_or_else(|| peer.fmt_short().to_string());
                                    let was_new = mgr.add_friend(peer, addr).await.unwrap_or(false);
                                    AppMessage::FriendAdded {
                                        fid: fid.as_str().to_string(),
                                        label,
                                        was_new,
                                    }
                                }
                                None => {
                                    AppMessage::ErrorMsg(format!("Invalid public key: {key_part}"))
                                }
                            }
                        },
                        |msg| msg,
                    );
                }

                if let Some(target) = trimmed.strip_prefix("/friend remove ") {
                    let target = target.trim().to_string();
                    let mgr = self.friend_mgr.clone();
                    return iced::Task::perform(
                        async move {
                            match target.parse::<PublicKey>() {
                                Ok(peer) => {
                                    let removed = mgr.remove_friend(&peer).await.unwrap_or(false);
                                    let label = if removed {
                                        peer.fmt_short().to_string()
                                    } else {
                                        target.clone()
                                    };
                                    AppMessage::FriendRemoved { label }
                                }
                                Err(_) => {
                                    AppMessage::ErrorMsg(format!("Friend not found: {target}"))
                                }
                            }
                        },
                        |msg| msg,
                    );
                }

                if trimmed == "/friend list" {
                    let mgr = self.friend_mgr.clone();
                    return iced::Task::perform(
                        async move {
                            match mgr.list_friends().await {
                                Ok(list) => {
                                    let items: Vec<(String, String)> = list
                                        .into_iter()
                                        .map(|(pk, status)| {
                                            let status_str = match status {
                                                FriendStatus::Unknown => "?".to_string(),
                                                FriendStatus::Online => "ONLINE".to_string(),
                                                FriendStatus::Offline => "offline".to_string(),
                                            };
                                            (pk.fmt_short().to_string(), status_str)
                                        })
                                        .collect();
                                    AppMessage::FriendListResult(items)
                                }
                                Err(e) => {
                                    AppMessage::ErrorMsg(format!("Failed to list friends: {e}"))
                                }
                            }
                        },
                        |msg| msg,
                    );
                }

                if trimmed == "/connections" {
                    use boru_core::chat_core::check_peer_connection_type;
                    let neighbors: Vec<iroh::PublicKey> = self.neighbors.iter().copied().collect();
                    let peer_lat = self.peer_latencies.clone();
                    if neighbors.is_empty() {
                        self.push_system("No known peers to inspect.");
                    } else {
                        let ep = self.endpoint.clone();
                        let names = self.names.clone();
                        return iced::Task::perform(
                            async move {
                                let mut lines = vec![format!("Connections ({}):", neighbors.len())];
                                for pk in &neighbors {
                                    let ctype = check_peer_connection_type(&ep, *pk).await;
                                    let label = names
                                        .get(pk)
                                        .cloned()
                                        .unwrap_or_else(|| pk.fmt_short().to_string());
                                    let lat_str = peer_lat
                                        .get(pk)
                                        .map(|d| format!(" {}ms", d.as_millis()))
                                        .unwrap_or_default();
                                    lines.push(format!(
                                        "  {label} - {} ({}){lat_str}",
                                        match ctype {
                                            boru_core::chat_core::ConnectionType::Direct => {
                                                "direct"
                                            }
                                            boru_core::chat_core::ConnectionType::Relayed => {
                                                "relayed"
                                            }
                                            boru_core::chat_core::ConnectionType::Unknown => {
                                                "unknown"
                                            }
                                        },
                                        pk.fmt_short(),
                                    ));
                                }
                                AppMessage::ConnectionsResult(lines)
                            },
                            |msg| msg,
                        );
                    }
                    return iced::Task::none();
                }

                // ── Reactions ──
                if let Some(rest) = trimmed.strip_prefix("/react ") {
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                    if parts.len() < 2 {
                        self.push_system("Usage: /react <msg_index> <emoji>".to_string());
                        return iced::Task::none();
                    }
                    let idx: usize = match parts[0].parse() {
                        Ok(i) => i,
                        Err(_) => {
                            self.push_system("Usage: /react <msg_index> <emoji>".to_string());
                            return iced::Task::none();
                        }
                    };
                    let emoji = parts[1].to_string();
                    if idx == 0 || idx > self.entries.len() {
                        self.push_system(format!("No message at index {idx}"));
                        return iced::Task::none();
                    }
                    let Some(hash) = self.entries[idx - 1].message_hash else {
                        self.push_system("Cannot react to a system message".to_string());
                        return iced::Task::none();
                    };
                    // Apply locally first
                    self.add_reaction(&hash, emoji.clone());
                    // Broadcast
                    match SignedMessage::sign_and_encode(
                        &self.secret_key,
                        &crate::Message::Reaction {
                            message_hash: hash,
                            emoji,
                        },
                    ) {
                        Ok(encoded) => {
                            if let Some(ref sender) = self.sender {
                                let sender = sender.clone();
                                return iced::Task::perform(
                                    async move {
                                        sender.broadcast(encoded).await.ok();
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                        }
                        Err(e) => {
                            return iced::Task::done(AppMessage::ErrorMsg(e.to_string()));
                        }
                    }
                    return iced::Task::done(AppMessage::ErrorMsg(
                        "Not connected to any room.".into(),
                    ));
                }

                // ── Edit ──
                if let Some(rest) = trimmed.strip_prefix("/edit ") {
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                    if parts.len() < 2 {
                        self.push_system("Usage: /edit <msg_index> <new_text>".to_string());
                        return iced::Task::none();
                    }
                    let idx: usize = match parts[0].parse() {
                        Ok(i) => i,
                        Err(_) => {
                            self.push_system("Usage: /edit <msg_index> <new_text>".to_string());
                            return iced::Task::none();
                        }
                    };
                    let new_text = parts[1].to_string();
                    if idx == 0 || idx > self.entries.len() {
                        self.push_system(format!("No message at index {idx}"));
                        return iced::Task::none();
                    }
                    let Some(hash) = self.entries[idx - 1].message_hash else {
                        self.push_system("Cannot edit a system message".to_string());
                        return iced::Task::none();
                    };
                    // Apply locally first
                    self.edit_message(&hash, new_text.clone());
                    // Broadcast
                    match SignedMessage::sign_and_encode(
                        &self.secret_key,
                        &crate::Message::Edit {
                            original_hash: hash,
                            new_text,
                        },
                    ) {
                        Ok(encoded) => {
                            if let Some(ref sender) = self.sender {
                                let sender = sender.clone();
                                return iced::Task::perform(
                                    async move {
                                        sender.broadcast(encoded).await.ok();
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                        }
                        Err(e) => {
                            return iced::Task::done(AppMessage::ErrorMsg(e.to_string()));
                        }
                    }
                    return iced::Task::done(AppMessage::ErrorMsg(
                        "Not connected to any room.".into(),
                    ));
                }

                // ── Delete ──
                if let Some(idx_str) = trimmed.strip_prefix("/delete ") {
                    let idx_str = idx_str.trim().to_string();
                    let idx: usize = match idx_str.parse() {
                        Ok(i) => i,
                        Err(_) => {
                            self.push_system("Usage: /delete <msg_index>".to_string());
                            return iced::Task::none();
                        }
                    };
                    if idx == 0 || idx > self.entries.len() {
                        self.push_system(format!("No message at index {idx}"));
                        return iced::Task::none();
                    }
                    let Some(hash) = self.entries[idx - 1].message_hash else {
                        self.push_system("Cannot delete a system message".to_string());
                        return iced::Task::none();
                    };
                    // Apply locally first
                    self.delete_message(&hash);
                    // Broadcast
                    match SignedMessage::sign_and_encode(
                        &self.secret_key,
                        &crate::Message::Delete { message_hash: hash },
                    ) {
                        Ok(encoded) => {
                            if let Some(ref sender) = self.sender {
                                let sender = sender.clone();
                                return iced::Task::perform(
                                    async move {
                                        sender.broadcast(encoded).await.ok();
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                        }
                        Err(e) => {
                            return iced::Task::done(AppMessage::ErrorMsg(e.to_string()));
                        }
                    }
                    return iced::Task::done(AppMessage::ErrorMsg(
                        "Not connected to any room.".into(),
                    ));
                }

                // Normal text message — check for whisper commands first
                if let Some(rest) = trimmed.strip_prefix("/whisper ") {
                    // ── Whisper DM ──────────────────────────────────────────
                    let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                    if parts.len() < 2 {
                        self.push_system("Usage: /whisper <peer-key|friend-alias> <message>");
                        return iced::Task::none();
                    }
                    let target = parts[0].trim().to_string();
                    let message = parts[1].trim().to_string();
                    // Resolve peer key from alias or direct public key
                    let peer_key = self.resolve_peer_key(&target);
                    let peer_key = match peer_key {
                        Some(pk) => pk,
                        None => {
                            self.push_system(format!(
                                "Unknown peer: {target}. Use a public key or friend alias."
                            ));
                            return iced::Task::none();
                        }
                    };
                    let whisper_handle = self.whisper_handle.clone();
                    let text = message.clone();
                    let label = self
                        .names
                        .get(&peer_key)
                        .cloned()
                        .unwrap_or_else(|| peer_key.fmt_short().to_string());
                    self.push_system(format!("[Whisper to {label}] {message}"));
                    // Check if this peer has a mailbox key for offline delivery.
                    let mailbox_pk = {
                        let fid = FriendId::from_public_key(peer_key);
                        self.friends.get(&fid).and_then(|r| r.mailbox_public_key)
                    };
                    let secret_key = self.secret_key.clone();
                    let data_dir = self.data_dir.clone();
                    let _progress_queue = self.download_progress_queue.clone();
                    let endpoint = self.endpoint.clone();
                    return iced::Task::perform(
                        async move {
                            match whisper_handle.send_dm(peer_key, text.clone()).await {
                                Ok(()) => AppMessage::Noop,
                                Err(_) if mailbox_pk.is_some() => {
                                    let pk = mailbox_pk.unwrap();
                                    match seal_for(&secret_key, pk, text.as_bytes()) {
                                        Ok(envelope) => {
                                            let mut store = MailboxStore::load(&data_dir)
                                                .ok()
                                                .flatten()
                                                .unwrap_or_else(|| {
                                                    MailboxStore::for_recipient(
                                                        &data_dir,
                                                        secret_key.public(),
                                                    )
                                                });
                                            let delivery_envelope = envelope.clone();
                                            match store.enqueue_outgoing(envelope) {
                                                Ok(msg_id) => {
                                                    // Persist the envelope first (fallback for offline peers).
                                                    // Save is a no-op — SQLite unified storage handles persistence.
                                                    // Attempt proactive direct QUIC delivery.
                                                    match send_deliver(
                                                        &endpoint,
                                                        &secret_key,
                                                        peer_key,
                                                        delivery_envelope,
                                                    )
                                                    .await
                                                    {
                                                        Ok(()) => AppMessage::OfflineDMStatus {
                                                            message_id: msg_id,
                                                            label,
                                                            status:
                                                                OfflineDeliveryStatus::Delivered,
                                                        },
                                                        Err(_) => {
                                                            // Peer offline; envelope is already stored for later
                                                            // sync-based delivery.
                                                            AppMessage::OfflineDMStatus {
                                                                message_id: msg_id,
                                                                label,
                                                                status:
                                                                    OfflineDeliveryStatus::Queued,
                                                            }
                                                        }
                                                    }
                                                }
                                                Err(enq_err) => AppMessage::ErrorMsg(format!(
                                                    "Failed to queue offline message: {enq_err}"
                                                )),
                                            }
                                        }
                                        Err(seal_err) => AppMessage::ErrorMsg(format!(
                                            "Failed to encrypt offline message: {seal_err}"
                                        )),
                                    }
                                }
                                Err(e) => AppMessage::ErrorMsg(format!("Whisper failed: {e}")),
                            }
                        },
                        |msg| msg,
                    );
                }

                if let Some(rest) = trimmed.strip_prefix("/whisper-file ") {
                    let _ = rest;
                    self.push_system(
                        "Direct file transfer is disabled; use the authorised file catalogue."
                            .to_string(),
                    );
                    return iced::Task::none();
                }

                // Normal text message
                let _timer = PerfTracker::timer("send_message", "text");
                let text = trimmed.clone();
                match self.persist_outgoing_message(self.topic, &trimmed) {
                    Ok((event_id, msg_hash, encoded)) => {
                        self.self_sent_events.insert(msg_hash, event_id);
                        let mut local_entry = ChatEntry::local(&self.local_label, &text);
                        local_entry.event_id = event_id;
                        local_entry.message_hash = Some(msg_hash);
                        let entry_idx = self.entries_push(local_entry);
                        let preview_task = self.maybe_fetch_link_preview(entry_idx);
                        if let Some(action_id) = self.pending_submit_composer_action.take() {
                            let _ = self
                                .gui_action_history
                                .set_state(&action_id, GuiActionState::AppMessageHandled);
                            let _ = self
                                .gui_action_history
                                .set_state(&action_id, GuiActionState::Completed);
                        }
                        // Show the transient "sending" state on the send button
                        // while the broadcast task is in flight.  The flag is
                        // cleared by the completion task chained below (after
                        // every output of the send task, including the
                        // `MessageSent` acceptance).
                        self.composer_sending = true;
                        let send_task = Self::broadcast_or_queue(
                            encoded,
                            self.sender.clone(),
                            self.sender_ready,
                            self.neighbors.len(),
                            text,
                            event_id,
                            msg_hash,
                            preview_task,
                        );
                        send_task.chain(iced::Task::done(AppMessage::ComposerSendFinished))
                    }
                    Err(e) => iced::Task::done(AppMessage::ErrorMsg(e)),
                }
            }

            AppMessage::AttachPressed => {
                iced::Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Select a file to share")
                        .pick_file(),
                    |file| {
                        if let Some(file) = file {
                            let name = file.file_name().to_string();
                            let path = file.path().to_string_lossy().to_string();
                            let encoded = format!("{name}|{path}|{path}");
                            // Auto-detect image files by extension for inline display
                            let is_image = is_attachment_image(&name);
                            if is_image {
                                AppMessage::ExecuteImageSend(encoded)
                            } else {
                                AppMessage::ExecuteFileSend(encoded)
                            }
                        } else {
                            AppMessage::Noop
                        }
                    },
                )
            }

            AppMessage::AttachFolderPressed => {
                iced::Task::perform(
                    rfd::AsyncFileDialog::new()
                        .set_title("Select a folder to share")
                        .pick_folder(),
                    |dir| {
                        if let Some(dir) = dir {
                            // rfd 0.15 `FileHandle::file_name()` returns the
                            // name directly (a `String`), not an `Option`.
                            let name = dir.file_name();
                            if name.is_empty() {
                                return AppMessage::ErrorMsg(
                                    "Could not determine folder name".to_string(),
                                );
                            }
                            let path = dir.path().to_string_lossy().to_string();
                            let encoded = format!("{name}|{path}|{path}");
                            AppMessage::ExecuteFolderSend(encoded)
                        } else {
                            AppMessage::Noop
                        }
                    },
                )
            }

            AppMessage::ToggleHelp => {
                self.help_visible = !self.help_visible;
                if let Some(action_id) = self.pending_toggle_help_action.take() {
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::AppMessageHandled);
                    let _ = self
                        .gui_action_history
                        .set_state(&action_id, GuiActionState::Completed);
                }
                iced::Task::none()
            }
            AppMessage::ComposerSendFinished => {
                self.composer_sending = false;
                iced::Task::none()
            }
            AppMessage::ComposerDragOver(over) => {
                self.composer_drag_over = over;
                iced::Task::none()
            }
            AppMessage::ComposerFileDropped(path) => {
                self.composer_drag_over = false;
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if name.is_empty() {
                    return iced::Task::none();
                }
                let path_str = path.to_string_lossy().to_string();
                let encoded = format!("{name}|{path_str}|{path_str}");
                // Auto-detect image files by extension for inline display (same
                // rule as the AttachPressed file dialog result).
                let is_image = is_attachment_image(&name);
                if is_image {
                    iced::Task::done(AppMessage::ExecuteImageSend(encoded))
                } else {
                    iced::Task::done(AppMessage::ExecuteFileSend(encoded))
                }
            }
            AppMessage::ComposerImeActive(active) => {
                self.composer_ime_active = active;
                iced::Task::none()
            }
            AppMessage::ToggleChatOptions => {
                self.show_chat_options = !self.show_chat_options;
                // Close the details panel when opening the options popover
                self.details_panel_open = false;
                iced::Task::none()
            }
            AppMessage::ToggleChatSearch => {
                self.show_chat_search = !self.show_chat_search;
                if !self.show_chat_search {
                    self.chat_search_query.clear();
                }
                iced::Task::none()
            }
            AppMessage::ChatSearchQueryChanged(query) => {
                self.chat_search_query = query;
                iced::Task::none()
            }
            AppMessage::ClearConversation => {
                let topic = self.topic;
                #[cfg(feature = "video-playback")]
                self.stop_inline_video();
                // Clear screen entries and indexes
                self.entries.clear();
                self.event_id_to_index.clear();
                self.message_hash_to_index.clear();
                self.layout_cache.borrow_mut().clear();
                self.history_saved_count = 0;

                // Clear from in-memory chat history store
                {
                    let mut chat_history = self.chat_history.lock().unwrap();
                    let _ =
                        clear_room_history(topic, &mut self.room_history, &mut chat_history, None);
                }

                // Clear from SQLite outgoing_messages
                if let Some(storage) = &self.storage {
                    let _ = storage.delete_outgoing_for_topic(&topic);
                }

                self.push_system("Conversation cleared.");
                iced::Task::none()
            }
            AppMessage::ToggleDetailsPanel => {
                self.details_panel_open = !self.details_panel_open;
                iced::Task::none()
            }
            AppMessage::ToggleMemberList => {
                self.show_member_list = !self.show_member_list;
                iced::Task::none()
            }

            // ── Invite menu ────────────────────────────────────────
            AppMessage::ToggleInviteMenu => {
                self.show_invite_menu = !self.show_invite_menu;
                if !self.show_invite_menu {
                    self.invite_whisper_input.clear();
                }
                iced::Task::none()
            }
            AppMessage::InviteWhisperInputChanged(text) => {
                self.invite_whisper_input = text;
                iced::Task::none()
            }
            AppMessage::InviteSendWhisper => {
                let peer_key_str = self.invite_whisper_input.clone();
                let whisper_handle = self.whisper_handle.clone();
                let ticket_str = self.ticket_str.clone();

                // Parse the peer public key and send the invite
                let result = match peer_key_str.parse::<PublicKey>() {
                    Ok(peer_key) => {
                        let invite_text = format!("\x00PRIVATE_CHAT:{ticket_str}");
                        iced::Task::perform(
                            async move { whisper_handle.send_dm(peer_key, invite_text).await },
                            move |result| match result {
                                Ok(()) => AppMessage::Noop,
                                Err(e) => AppMessage::ErrorMsg(format!("Invite failed: {e}")),
                            },
                        )
                    }
                    Err(_) => iced::Task::done(AppMessage::ErrorMsg(
                        "Invalid public key. Enter a valid peer key.".to_string(),
                    )),
                };

                // Close the invite menu
                self.show_invite_menu = false;
                self.invite_whisper_input.clear();

                // Push a system message showing that we sent an invite
                self.push_system(format!("Room invite sent via whisper to {peer_key_str}"));

                result
            }

            AppMessage::NetEvent(conv_event) => {
                let _timer = PerfTracker::timer("net_event", format!("topic={}", conv_event.topic));
                let topic = conv_event.topic;
                let event = conv_event.event;
                // Only bump conversation to the top of the sidebar when there
                // is an actual user-visible message (text, file, image).
                // NeighborUp/Down, Presence, AboutMe, Closed, and Error events
                // are network/gossip noise and should not reorder the list.
                if Self::_is_user_visible_event(&event) {
                    self.conversation_store.touch_and_bump(&topic);
                    self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                }
                // Update the sidebar preview BEFORE taking the mutable borrow
                // on self.conversations (avoids borrow conflict).
                let is_inactive =
                    topic != self.topic || !matches!(self.screen, Screen::Chat { .. });
                if is_inactive {
                    self.update_room_preview(&topic, &event);
                }
                let conversation = self
                    .conversations
                    .entry(topic)
                    .or_insert_with(|| ConversationLive::new(topic));
                // Update per-conversation neighbor sets when NeighborUp/Down
                // events arrive for any topic, not just the active one. This
                // ensures that switching to a background conversation restores
                // an accurate neighbor set rather than an empty one.
                match &event {
                    NetEvent::NeighborUp { peer } => {
                        conversation.neighbors.insert(*peer);
                        self.room_neighbor_counts
                            .entry(topic)
                            .and_modify(|count| *count = count.saturating_add(1))
                            .or_insert(1);
                        conversation.sender_ready = conversation.sender.is_some();
                        if topic == self.topic {
                            let was_ready = self.sender_ready;
                            // Keep the active conversation's neighbor set in
                            // sync with the per-conversation set. switch_to_
                            // conversation only copies conversation.neighbors
                            // once; a NeighborUp arriving after the switch
                            // (e.g. direct chat opened via the fast path
                            // before the direct-topic mesh formed) would
                            // otherwise leave self.neighbors empty forever,
                            // silently queueing every send (broadcast_or_queue
                            // requires sender_ready && neighbor_count > 0) and
                            // never retrying them (the retry loop also gates on
                            // self.neighbors.len() for the active topic).
                            self.neighbors.insert(*peer);
                            self.sender_ready = self.sender.is_some();
                            info!(
                                %peer,
                                topic = %topic,
                                sender_some = self.sender.is_some(),
                                sender_ready = self.sender_ready,
                                was_ready,
                                "NeighborUp: sender_ready updated"
                            );
                        }
                    }
                    NetEvent::NeighborDown { peer } => {
                        conversation.neighbors.remove(peer);
                        if let Some(count) = self.room_neighbor_counts.get_mut(&topic) {
                            *count = count.saturating_sub(1);
                        }
                        conversation.sender_ready =
                            !conversation.neighbors.is_empty() && conversation.sender.is_some();
                        if topic == self.topic {
                            self.neighbors.remove(peer);
                            self.sender_ready =
                                !conversation.neighbors.is_empty() && self.sender.is_some();
                        }
                        // NeighborDown is transient: the endpoint may still
                        // have a usable relay/address record, so ask the
                        // gossip actor to retry this peer instead of waiting
                        // for the next DHT publication cycle.
                        if let Some(sender) = conversation.sender.clone() {
                            let peer = *peer;
                            tokio::spawn(async move {
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                if let Err(error) = sender.join_peers(vec![peer]).await {
                                    debug!(%peer, %error, "room neighbor retry failed");
                                }
                            });
                        }
                    }
                    _ => {}
                }
                if is_inactive {
                    // Only queue user-visible messages (chat text, file shares)
                    // into the pending replay buffer.  Gossip protocol events
                    // (AboutMe, Presence, Heartbeat, NeighborUp/Down,
                    // announcements) are not renderable chat history — queueing
                    // them fills the 256-event cap and triggers a warning storm
                    // on dense public topics like the shared lobby.  They are
                    // also excluded from the unread counter below.
                    let should_count = Self::_is_user_visible_event(&event);
                    if !should_count {
                        return iced::Task::none();
                    }
                    // Emit a notification for user-visible messages on inactive
                    // conversations — includes group-aware rendering.
                    if let NetEvent::Message { .. } = &event {
                        // Notification emit deferred — requires refactor to avoid
                        // double-borrow with conversation entry above.
                    }
                    conversation.pending_events.push_back(event);
                    // Cap the pending queue to prevent unbounded memory growth
                    // and Iced event-loop starvation during replay.  When the
                    // cap is exceeded we discard the oldest queued event and
                    // adjust the unread counter if that event was visible.
                    while conversation.pending_events.len() > MAX_PENDING_EVENTS {
                        if let Some(dropped) = conversation.pending_events.pop_front() {
                            if Self::_is_user_visible_event(&dropped) {
                                conversation.unread = conversation.unread.saturating_sub(1);
                            }
                        }
                        tracing::warn!(
                            topic=%topic,
                            total=conversation.pending_events.len(),
                            "pending events cap reached, oldest event dropped"
                        );
                    }
                    conversation.unread = conversation.unread.saturating_add(1);
                    tracing::info!(topic=%topic, unread=conversation.unread, "queued event for inactive room");
                    return iced::Task::none();
                }
                conversation.unread = 0;
                let mut tasks: Vec<iced::Task<AppMessage>> = Vec::new();
                if let Some(read_receipt_task) = self.process_net_event_sync(&topic, &event) {
                    tasks.push(read_receipt_task);
                }
                if !self.pending_image.is_empty()
                    || !self.pending_thumbnail_fetch.is_empty()
                    || !self.pending_gif.is_empty()
                {
                    tasks.push(self.drain_pending_transfers());
                }
                // Check if a profile image ticket arrived from a remote peer
                if let Some((peer, ticket_str)) = self.pending_profile_image_tickets.pop_front() {
                    tasks.push(Self::download_profile_image_task(
                        &self.blob_store,
                        &self.endpoint,
                        &self.memory_lookup,
                        &self.neighbors,
                        &self.public_room_safety,
                        peer,
                        ticket_str,
                    ));
                }
                // Check if the last pushed entry has a URL that needs link preview
                if !self.entries.is_empty() {
                    let last_idx = self.entries.len() - 1;
                    if let Some(pt) = self.maybe_fetch_link_preview(last_idx) {
                        tasks.push(pt);
                    }
                }
                if tasks.is_empty() {
                    iced::Task::none()
                } else {
                    iced::Task::batch(tasks)
                }
            }


            AppMessage::WhisperEvent(event) => {
                info!(variant = ?event, "WhisperEvent received in iced handler");
                match event {
                    boru_core::whisper::WhisperEvent::Control { from, content } => {
                        match SignedContactMessage::verify(&content, Some(from)) {
                            Ok((sender, ContactAction::FriendRequest { name })) => {
                                // Keep the request pending until the user explicitly
                                // accepts or declines it.  Auto-accepting here made
                                // incoming requests disappear from the sidebar.
                                let local_str = self.local_public.to_string();
                                if let Some(name) = name {
                                    let record = self
                                        .friends
                                        .ensure_friend(FriendId::from_public_key(sender));
                                    record.last_announced_name = Some(name);
                                    self.try_save_friends();
                                }
                                match self.friend_request_store.send_request(
                                    sender.to_string(),
                                    local_str,
                                    None,
                                ) {
                                    Ok(req) => {
                                        info!(
                                            request_id = %req.id,
                                            from = %sender.fmt_short(),
                                            "incoming friend request stored"
                                        );
                                        self.requests_sidebar_revision =
                                            self.requests_sidebar_revision.wrapping_add(1);
                                        self.refresh_sidebar_counts();
                                        self.send_save_friend_requests();
                                        // Notification emit deferred — requires name lifetime fix.
                                    }
                                    Err(FriendRequestError::DuplicatePending { existing_id }) => {
                                        info!(
                                            %existing_id,
                                            from = %sender.fmt_short(),
                                            "incoming friend request is duplicate pending — ignored"
                                        );
                                    }
                                    Err(err) => {
                                        info!(
                                            error = %err,
                                            from = %sender.fmt_short(),
                                            "failed to store incoming friend request"
                                        );
                                    }
                                }
                            }
                            Ok((sender, ContactAction::FriendRequestAccepted)) => {
                                self.outgoing_request_states
                                    .insert(sender, OutgoingRequestState::Accepted);
                                self.rebuild_join_request_list();
                                // Update the friend request store to reflect remote acceptance.
                                let local_str = self.local_public.to_string();
                                let sender_str = sender.to_string();
                                if let Some(pending_id) = self
                                    .friend_request_store
                                    .list_outgoing_by_status(
                                        &local_str,
                                        FriendRequestStatus::Pending,
                                    )
                                    .into_iter()
                                    .find(|r| r.recipient == sender_str)
                                    .map(|r| r.id.clone())
                                {
                                    let _ = self
                                        .friend_request_store
                                        .confirm_outgoing_accepted(&pending_id, &local_str);
                                }
                                let fid = FriendId::from_public_key(sender);
                                let record = self.friends.ensure_friend(fid);
                                record.relationship = FriendRelationship::Friends;
                                self.call_handle.set_peer_authorized(sender, true);
                                if let Some(conversation) = record.direct_conversation.as_mut() {
                                    conversation.state = DirectConversationState::Active;
                                }
                                // Show the accepted friend immediately in the sidebar.
                                self.peer_presence_map
                                    .insert(sender, now_ms().max(0) as u64);
                                self.chats_sidebar_revision =
                                    self.chats_sidebar_revision.wrapping_add(1);
                                self.mark_friends_sidebar_dirty();
                                self.try_save_friends();
                                // Auto-subscribe to the deterministic direct-chat topic
                                // so both peers are on the same gossip topic without
                                // waiting for a whisper ConversationInvite.
                                let friend_topic = direct_topic(&self.local_public, &sender);
                                if !self.conversations.contains_key(&friend_topic) {
                                    let bootstrap = self.discovered_peers.clone();
                                    return iced::Task::done(AppMessage::BackgroundSubscribe(
                                        friend_topic,
                                        bootstrap,
                                    ));
                                }
                            }
                            Ok((sender, ContactAction::FriendRequestRejected)) => {
                                self.outgoing_request_states
                                    .insert(sender, OutgoingRequestState::Declined);
                                self.rebuild_join_request_list();
                            }
                            Ok((sender, ContactAction::ConversationInvite { topic, addrs }))
                                if addrs.iter().all(|addr| addr.id == sender) =>
                            {
                                // ConversationInvite is an authenticated, explicit
                                // Chat click. Validate the stable topic before any
                                // durable mutation, then auto-accept and open it.
                                let Some(persisted_addrs) = confirmed_direct_invite_addrs(
                                    self.local_public,
                                    &self.friends,
                                    sender,
                                    topic,
                                    &addrs,
                                ) else {
                                    debug!("ignoring contact invite with invalid direct topic");
                                    return iced::Task::none();
                                };
                                let fid = FriendId::from_public_key(sender);
                                let label = self
                                    .friends
                                    .get(&fid)
                                    .map(|record| record.display_label(&fid, &sender))
                                    .unwrap_or_else(|| sender.fmt_short().to_string());
                                let record = self.friends.ensure_friend(fid.clone());
                                record.record_addrs(persisted_addrs.clone());
                                record.set_direct_conversation(
                                    topic,
                                    DirectConversationState::Active,
                                );
                                // Only establish the friendship when this invite is
                                // the acceptance reply to a friend request WE sent
                                // (pending outgoing request to this sender).  A bare
                                // Chat click invite from a non-friend must NOT
                                // auto-friend — the sender stays in Discover until
                                // both sides explicitly accept a friend request.
                                let is_acceptance_reply = self
                                    .friend_request_store
                                    .list_outgoing_by_status(
                                        &self.local_public.to_string(),
                                        FriendRequestStatus::Pending,
                                    )
                                    .into_iter()
                                    .any(|r| r.recipient == sender.to_string());
                                if is_acceptance_reply {
                                    record.relationship = FriendRelationship::Friends;
                                    self.call_handle.set_peer_authorized(sender, true);
                                }
                                self.conversation_store.upsert(ConversationEntry::new(
                                    topic,
                                    sender.to_string(),
                                    label,
                                ));
                                self.chats_sidebar_revision =
                                    self.chats_sidebar_revision.wrapping_add(1);
                                let _room =
                                    RoomStore::with_peers(&self.data_dir, topic, persisted_addrs);
                                self.try_save_friends();
                                self.peer_presence_map
                                    .insert(sender, now_ms().max(0) as u64);
                                self.chats_sidebar_revision =
                                    self.chats_sidebar_revision.wrapping_add(1);
                                self.mark_friends_sidebar_dirty();
                                self.outgoing_request_states
                                    .insert(sender, OutgoingRequestState::Accepted);
                                self.rebuild_join_request_list();
                                // Update the friend request store to reflect remote
                                // acceptance — the ConversationInvite signals acceptance.
                                let local_str = self.local_public.to_string();
                                let sender_str = sender.to_string();
                                if let Some(pending_id) = self
                                    .friend_request_store
                                    .list_outgoing_by_status(
                                        &local_str,
                                        FriendRequestStatus::Pending,
                                    )
                                    .into_iter()
                                    .find(|r| r.recipient == sender_str)
                                    .map(|r| r.id.clone())
                                {
                                    let _ = self
                                        .friend_request_store
                                        .confirm_outgoing_accepted(&pending_id, &local_str);
                                }
                                // Use BackgroundSubscribe instead of OpenRoom to avoid
                                // slow-path gossip subscription with WAL replay storm.
                                // The conversation appears in the sidebar; user clicks
                                // to open it when ready.
                                let bootstrap = self.discovered_peers.clone();
                                return iced::Task::done(AppMessage::BackgroundSubscribe(
                                    topic, bootstrap,
                                ));
                            }
                            Ok((sender, ContactAction::AddressUpdate { addrs }))
                                if addrs.iter().all(|addr| addr.id == sender) =>
                            {
                                let record = self
                                    .friends
                                    .ensure_friend(FriendId::from_public_key(sender));
                                record.record_addrs(addrs);
                                self.try_save_friends();
                            }
                            Ok((sender, ContactAction::MailboxAdvertise { mailbox })) => {
                                let fid = FriendId::from_public_key(sender);
                                let record = self.friends.ensure_friend(fid);
                                record.set_mailbox_public_key(mailbox);
                                self.try_save_friends();
                            }
                            Ok((sender, ContactAction::TunnelOffer { offer })) => {
                                // Forward the incoming tunnel offer to the
                                // sidebar Requests section so the user can
                                // accept or decline it. The tunnel id is
                                // hex-encoded so Accept/Decline can map it
                                // back to a TunnelId.
                                if let Some(tunnel_id) =
                                    self.handle_received_tunnel_offer(sender, offer)
                                {
                                    return iced::Task::done(AppMessage::TunnelRequestReceived {
                                        peer: sender,
                                        tunnel_id: hex::encode(tunnel_id.0),
                                    });
                                }
                            }
                            Ok((_sender, _action)) => {
                                self.push_system(
                                    "Rejected invalid contact control message.".to_string(),
                                );
                            }
                            Err(err) => {
                                info!(
                                    error = %err,
                                    from = %from.fmt_short(),
                                    "invalid contact control message"
                                );
                            }
                        }
                    }
                    boru_core::whisper::WhisperEvent::Message { from, content } => {
                        // The whisper session manager sends an empty DM for
                        // connection establishment / address discovery; these
                        // contain no user-readable text and should not create
                        // chat entries or increment the unread counter.
                        if content.is_empty() {
                            return iced::Task::none();
                        }
                        let text = String::from_utf8_lossy(&content).to_string();
                        let label = self
                            .names
                            .get(&from)
                            .cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());

                        // A ticket-bearing invite gives the recipient the
                        // route needed to bootstrap the deterministic private room.
                        let invite_ticket = text
                            .strip_prefix("\x00PRIVATE_CHAT:")
                            .and_then(|raw| raw.parse::<Ticket>().ok());
                        let is_invite = invite_ticket.is_some() || text == "\x00PRIVATE_CHAT";
                        if is_invite {
                            if let Some(ticket) = &invite_ticket {
                                let room_label = self
                                    .room_history
                                    .find(&ticket.topic)
                                    .map(|r| r.display_name())
                                    .unwrap_or_else(|| {
                                        let hex = ticket.topic.to_string();
                                        format!("room {}", &hex[..8])
                                    });
                                self.push_system(format!("{label} invited you to {room_label}"));
                            } else {
                                self.push_system(format!(
                                    "{label} opened a private chat with you."
                                ));
                            }
                        }

                        // ── Group invite parsing ────────────────────────
                        let is_group_invite = text.starts_with("INVITE:");
                        if is_group_invite {
                            let parts: Vec<&str> = text.split(':').collect();
                            if parts.len() >= 5 && parts[0] == "INVITE" {
                                let invite_id_hex = parts[1];
                                let inviter_pk_str = parts[2];
                                let group_id_hex = parts[3];
                                let group_name = parts[4];
                                let ticket_str = parts.get(5).copied().unwrap_or("");

                                // Decode invite_id from hex
                                use data_encoding::HEXLOWER;
                                let mut invite_id = [0u8; 32];
                                if let Ok(decoded) = HEXLOWER.decode(invite_id_hex.as_bytes()) {
                                    if decoded.len() == 32 {
                                        invite_id.copy_from_slice(&decoded);
                                    }
                                }

                                // Decode group_id from hex
                                let mut group_id = [0u8; 32];
                                if let Ok(decoded) = HEXLOWER.decode(group_id_hex.as_bytes()) {
                                    if decoded.len() == 32 {
                                        group_id.copy_from_slice(&decoded);
                                    }
                                }

                                if let Ok(inviter_pk) = PublicKey::from_str(inviter_pk_str) {
                                    self.push_system(format!(
                                        "{label} invited you to group \"{group_name}\" (see REQUESTS section to accept)"
                                    ));

                                    // Persist in local invite inbox
                                    if let Some(ref st) = self.storage {
                                        let now_ms = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis()
                                            as u64;
                                        let expire_ms = now_ms + 7 * 24 * 60 * 60 * 1000;
                                        let invite_row = boru_core::storage::GroupInviteRow {
                                            invite_id,
                                            group_id,
                                            inviter_public_key: inviter_pk.to_vec(),
                                            recipient_public_key: self.secret_key.public().to_vec(),
                                            epoch: 1,
                                            status: "Pending".into(),
                                            created_at_ms: now_ms,
                                            expires_at_ms: expire_ms,
                                            ticket: ticket_str.to_string(),
                                            group_name: group_name.to_string(),
                                        };
                                        let _ = st.create_group_invite(&invite_row);
                                    }

                                    // Bump sidebar so the REQUESTS section updates
                                    self.requests_sidebar_revision =
                                        self.requests_sidebar_revision.wrapping_add(1);
                                    self.refresh_sidebar_counts();
                                }
                            }
                        }

                        let fid = FriendId::from_public_key(from);
                        // Don't auto-open a private chat for group invites —
                        // the user should explicitly accept the invite from the
                        // REQUESTS sidebar to join the actual group room.
                        let should_open_private =
                            is_invite || (self.friends.get(&fid).is_some() && !is_group_invite);
                        if should_open_private {
                            let private_topic = private_topic(&self.local_public, &from);
                            if let Some(ticket) = invite_ticket {
                                let _room = RoomStore::with_peers(
                                    &self.data_dir,
                                    private_topic,
                                    ticket.peers,
                                );
                            }
                            let already_on_topic = matches!(
                                self.screen,
                                Screen::Chat { topic } if topic == private_topic
                            );
                            if !already_on_topic {
                                self.save_room_to_history();
                                // Use BackgroundSubscribe to avoid slow-path
                                // subscription with WAL replay storm.
                                let bootstrap = self.discovered_peers.clone();
                                return iced::Task::done(AppMessage::BackgroundSubscribe(
                                    private_topic,
                                    bootstrap,
                                ));
                            }
                        }

                        if !is_invite {
                            let entry = ChatEntry::remote(
                                format!("Whisper from {label}"),
                                text,
                                None,
                                None, // whisper events carry no sent_at
                                Some(from),
                            );
                            self.entries_push(entry);
                        }
                    }

                    boru_core::whisper::WhisperEvent::Connected { peer } => {
                        let label = self
                            .names
                            .get(&peer)
                            .cloned()
                            .unwrap_or_else(|| peer.fmt_short().to_string());
                        self.push_system(format!("[Whisper] Connected to {label}"));

                        // On reconnect, sync any offline mailbox envelopes.
                        let has_mailbox = self
                            .friends
                            .get(&FriendId::from_public_key(peer))
                            .and_then(|r| r.mailbox_public_key)
                            .is_some();
                        if has_mailbox {
                            let endpoint = self.endpoint.clone();
                            let sk = self.secret_key.clone();
                            let dd = self.data_dir.clone();
                            let peer2 = peer;
                            return iced::Task::perform(
                                async move {
                                    // Open storage + read the last-synced cursor
                                    // position on the blocking pool — Storage::open
                                    // and get_sync_cursor are synchronous SQLite
                                    // I/O that must not run on a Tokio worker
                                    // (BORU-AUDIT-18).
                                    let dd_cursor = dd.clone();
                                    let since_ms = tokio::task::spawn_blocking(move || {
                                        let storage = Storage::open(&dd_cursor).ok();
                                        storage
                                            .as_ref()
                                            .and_then(|s| s.get_sync_cursor(&peer2).ok().flatten())
                                            .map(|c| c.last_sync_at_ms)
                                            .unwrap_or(0)
                                    })
                                    .await
                                    .unwrap_or(0);

                                    let identity = MailboxIdentity::from_secret(&sk);
                                    let mut store =
                                        MailboxStore::load(&dd).ok().flatten().unwrap_or_else(
                                            || MailboxStore::for_recipient(&dd, sk.public()),
                                        );
                                    let mut texts = Vec::new();
                                    let mut ack_ids = Vec::new();
                                    let mut cursor = since_ms;

                                    loop {
                                        match send_sync_request(&endpoint, &sk, peer2, cursor).await
                                        {
                                            Ok(page) => {
                                                for env in page.envelopes {
                                                    if let Ok((msg_id, plaintext, acceptance)) =
                                                        store.accept_incoming_with_status(
                                                            &identity,
                                                            env,
                                                            &[peer2],
                                                        )
                                                    {
                                                        // Replayed envelopes must still be ACKed, but
                                                        // only newly inserted messages may be surfaced
                                                        // in the conversation UI.  Sync is a backfill
                                                        // path, not permission to duplicate history.
                                                        ack_ids.push(msg_id.clone());
                                                        if acceptance
                                                            == IncomingAcceptance::Inserted
                                                        {
                                                            if let Ok(text) =
                                                                String::from_utf8(plaintext)
                                                            {
                                                                texts.push((msg_id, text));
                                                            }
                                                        }
                                                    }
                                                }

                                                if page.has_more {
                                                    cursor =
                                                        page.last_created_at_ms.unwrap_or(cursor);
                                                } else {
                                                    break;
                                                }
                                            }
                                            Err(e) => {
                                                return AppMessage::ErrorMsg(format!(
                                                    "Mailbox sync failed: {e}"
                                                ));
                                            }
                                        }
                                    }

                                    // Save is a no-op — SQLite unified storage handles persistence.

                                    // Persist the cursor so subsequent reconnects resume from here.
                                    // Storage::open + upsert_sync_cursor are synchronous SQLite
                                    // I/O — run on the blocking pool (BORU-AUDIT-18).
                                    let _ = tokio::task::spawn_blocking(move || {
                                        if let Some(stg) = Storage::open(&dd).ok() {
                                            let _ = stg.upsert_sync_cursor(
                                                &peer2,
                                                None,
                                                now_ms().max(0) as u64,
                                            );
                                        }
                                    })
                                    .await;
                                    // Send acks for all processed envelopes (new + replayed).
                                    for msg_id in &ack_ids {
                                        let ack = MailboxAck::sign(&sk, msg_id, peer2);
                                        let _ = send_ack(&endpoint, &sk, peer2, ack).await;
                                    }
                                    AppMessage::MailboxReplayed { peer: peer2, texts }
                                },
                                std::convert::identity,
                            );
                        }
                    }
                    boru_core::whisper::WhisperEvent::Disconnected { peer } => {
                        let label = self
                            .names
                            .get(&peer)
                            .cloned()
                            .unwrap_or_else(|| peer.fmt_short().to_string());
                        self.push_system(format!("[Whisper] Disconnected from {label}"));
                    }
                    boru_core::whisper::WhisperEvent::MailboxEnvelope { .. } => {
                        // Mailbox envelopes are encrypted and processed by the mailbox
                        // store — the GUI chat does not interpret them.
                    }
                    boru_core::whisper::WhisperEvent::MailboxAck { .. } => {
                        // Mailbox acknowledgements are verified and removed by the
                        // mailbox store — the GUI chat does not interpret them.
                    }
                }
                iced::Task::none()
            }

            AppMessage::OfflineDMStatus {
                message_id,
                label,
                status,
            } => {
                let status_text = match status {
                    OfflineDeliveryStatus::Queued => "queued",
                    OfflineDeliveryStatus::Delivered => "delivered",
                };
                let entry = ChatEntry::local(
                    &self.local_label,
                    format!("[Offline DM {status_text}] {label}"),
                );
                let idx = self.entries.len();
                self.entries_push(entry);
                self.pending_offline_ids.insert(message_id, idx);
                iced::Task::none()
            }

            AppMessage::InboxEvent(event) => {
                match event {
                    InboxEvent::EnvelopeReceived { from, envelope } => {
                        let label = self
                            .names
                            .get(&from)
                            .cloned()
                            .unwrap_or_else(|| from.fmt_short().to_string());

                        // Load mailbox store, accept incoming (validates + persists).
                        let s = MailboxStore::load(&self.data_dir)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| {
                                MailboxStore::for_recipient(
                                    &self.data_dir,
                                    self.secret_key.public(),
                                )
                            });
                        let mut store = s;
                        let identity = MailboxIdentity::from_secret(&self.secret_key);
                        match store.accept_incoming_with_status(&identity, envelope, &[from]) {
                            Ok((msg_id, plaintext, acceptance)) => {
                                if acceptance == IncomingAcceptance::Duplicate {
                                    let peer = from.to_string();
                                    DIAGNOSTICS.record_with_peer(
                                        None,
                                        Some(&peer),
                                        DiagnosticEventKind::DuplicateReceived {
                                            message_id_short: Some(
                                                msg_id.chars().take(12).collect(),
                                            ),
                                            conversation_id_prefix: None,
                                            peer_id: Some(peer.clone()),
                                        },
                                    );
                                } else if let Ok(text) = String::from_utf8(plaintext) {
                                    let entry = ChatEntry::remote(
                                        format!("Offline DM from {label}"),
                                        text,
                                        None,
                                        None,
                                        Some(from),
                                    );
                                    self.entries_push(entry);
                                }
                                // Persist accepted state. Duplicates remain
                                // unchanged, but are acknowledged below.
                                // Save is a no-op — SQLite unified storage handles persistence.
                                // Send an acknowledgement for both new and
                                // duplicate deliveries: the prior ack may have
                                // been lost after durable acceptance.
                                let endpoint = self.endpoint.clone();
                                let sk = self.secret_key.clone();
                                return iced::Task::perform(
                                    async move {
                                        let ack = MailboxAck::sign(&sk, &msg_id, from);
                                        let _ = send_ack(&endpoint, &sk, from, ack).await;
                                    },
                                    |_| AppMessage::Noop,
                                );
                            }
                            Err(e) => {
                                self.push_system(format!(
                                    "[Mailbox] Failed to accept envelope from {label}: {e}"
                                ));
                            }
                        }
                        iced::Task::none()
                    }
                    InboxEvent::AckReceived {
                        from: _from,
                        ack: _ack,
                    } => {
                        // Remove acknowledged envelope from local store.
                        let s = MailboxStore::load(&self.data_dir)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| MailboxStore::empty_at(&self.data_dir));
                        let mut store = s;
                        if let Ok(true) = store.acknowledge_outgoing_and_save(&_ack) {
                            debug!(
                                "mailbox: peer {} acknowledged envelope {}",
                                _from.fmt_short(),
                                _ack.message_id
                            );
                            // Update the in-memory ChatEntry to show delivered status.
                            if let Some(&idx) = self.pending_offline_ids.get(&_ack.message_id) {
                                if idx < self.entries.len() {
                                    self.entries[idx].body = "[Offline DM acked]".to_string();
                                    self.entries[idx].bump_gen();
                                }
                            }
                        }
                        iced::Task::none()
                    }
                    InboxEvent::SyncRequested { from, since_ms } => {
                        debug!(
                            "inbox: sync requested by {} since_ms={}",
                            from.fmt_short(),
                            since_ms
                        );
                        iced::Task::none()
                    }
                    InboxEvent::DeleteTombstoneReceived { from, proof } => {
                        // A remote peer forwarded a signed deletion authorisation
                        // from the original message author.  Apply the tombstone
                        // to the local message store to remove the inbox row and
                        // prevent resurrection by backfill/duplicates.
                        let store_path = self.data_dir.join("message_store.db");
                        match MessageStore::open(&store_path) {
                            Ok(store) => {
                                match store.insert_tombstone(
                                    &proof.msg_id,
                                    &proof.conversation_id,
                                    &proof.author,
                                    &*proof.author_signature,
                                ) {
                                    Ok(true) => {
                                        debug!(
                                            "inbox: applied delete tombstone from {} for msg {:?}",
                                            from.fmt_short(),
                                            proof.msg_id
                                        );
                                    }
                                    Ok(false) => {
                                        debug!(
                                            "inbox: delete tombstone from {} for msg {:?} was already tombstoned",
                                            from.fmt_short(),
                                            proof.msg_id
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            "inbox: failed to apply delete tombstone from {}: {e}",
                                            from.fmt_short()
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "inbox: failed to open message store for delete tombstone from {}: {e}",
                                    from.fmt_short()
                                );
                            }
                        }
                        iced::Task::none()
                    }
                }
            }

            AppMessage::OutboxRetryResult(results) => {
                // Only successful broadcasts advance a queued message. Failed
                // attempts remain queued for the next periodic retry.
                let mut changed = false;
                {
                    let mut history = self.chat_history.lock().unwrap();
                    for (topic, event_id, delivered) in results {
                        if let Some(storage) = &self.storage {
                            let _ = storage.increment_outgoing_retry(event_id);
                            if delivered {
                                let _ = storage.update_outgoing_delivery_state(event_id, "sent");
                            }
                        }
                        let message_hash = "unknown".to_string();
                        info!(
                            event_id,
                            message_hash = %message_hash,
                            topic = %topic,
                            local_peer = %self.local_public.fmt_short(),
                            neighbor_count = self.neighbors.len(),
                            sender_ready = self.sender_ready,
                            broadcast_result = if delivered { "accepted" } else { "failed" },
                            persistence_result = "queued",
                            "message delivery telemetry"
                        );
                        if delivered {
                            let _ = history.update_delivery_state(event_id, DeliveryState::Sent);
                            if let Some(&index) = self.event_id_to_index.get(&event_id) {
                                if let Some(entry) = self.entries.get_mut(index) {
                                    entry.delivery_state = DeliveryState::Sent;
                                    entry.bump_gen();
                                    changed = true;
                                }
                            }
                        }
                    }
                }
                if changed {
                    self.layout_cache.borrow_mut().clear();
                }
                iced::Task::none()
            }

            AppMessage::MessageSent(_text, event_id, msg_hash) => {
                if let Some(&index) = self.event_id_to_index.get(&event_id) {
                    if let Some(entry) = self.entries.get_mut(index) {
                        entry.delivery_state = DeliveryState::Sent;
                        entry.message_hash = Some(msg_hash);
                        entry.bump_gen();
                    }
                    self.message_hash_to_index.insert(msg_hash, index);
                }
                // Persist delivery state update in SQLite synchronously
                // (quick UPDATE query) and offload chat_history.json save to
                // background to avoid blocking the UI thread.
                if let Some(storage) = &self.storage {
                    let _ = storage.update_outgoing_delivery_state(event_id, "sent");
                }
                iced::Task::none()
            }

            AppMessage::RetryOutgoingMessage(event_id) => {
                // User tapped a failed outgoing message to retry it.
                // Transition state from "failed" back to "queued" in SQLite
                // so the periodic retry loop picks it up.
                if let Some(storage) = &self.storage {
                    let _ = storage.update_outgoing_delivery_state(event_id, "queued");
                }
                // Update in-memory entry
                if let Some(&index) = self.event_id_to_index.get(&event_id) {
                    if let Some(entry) = self.entries.get_mut(index) {
                        if entry.delivery_state == DeliveryState::Failed {
                            entry.delivery_state = DeliveryState::Queued;
                            entry.bump_gen();
                        }
                    }
                }
                iced::Task::none()
            }
            AppMessage::CopyMessage(idx) => {
                if let Some(entry) = self.entries.get(idx) {
                    // Truncate long messages in the toast
                    let preview: String = if entry.body.len() > 60 {
                        format!("{}…", &entry.body[..60])
                    } else {
                        entry.body.clone()
                    };
                    self.toast_message = Some(format!("Copied: {preview}"));
                    return iced::clipboard::write(entry.body.clone());
                }
                iced::Task::none()
            }

            AppMessage::RightClickText(idx) => {
                self.context_menu = Some((idx, 0.0, 0.0, ContextMenuKind::Text));
                iced::Task::none()
            }

            AppMessage::RightClickImage(idx) => {
                self.context_menu = Some((idx, 0.0, 0.0, ContextMenuKind::Image));
                iced::Task::none()
            }

            AppMessage::ContextCopyText(idx) => {
                self.context_menu = None;
                if let Some(entry) = self.entries.get(idx) {
                    let preview: String = if entry.body.len() > 60 {
                        format!("{}…", &entry.body[..60])
                    } else {
                        entry.body.clone()
                    };
                    self.toast_message = Some(format!("Copied: {preview}"));
                    return iced::clipboard::write(entry.body.clone());
                }
                iced::Task::none()
            }

            AppMessage::ContextCopyImage(idx) => {
                self.context_menu = None;
                // Image copy not yet implemented for system clipboard;
                // the context menu still appears for future wiring.
                self.toast_message = Some("Image copy not yet supported".to_string());
                iced::Task::none()
            }

            AppMessage::CloseContextMenu => {
                self.context_menu = None;
                iced::Task::none()
            }

            AppMessage::ToggleVideoCardMenu(entry_index) => {
                self.video_card_menu_open =
                    if self.video_card_menu_open == Some(entry_index) {
                        None
                    } else {
                        Some(entry_index)
                    };
                self.layout_cache.borrow_mut().invalidate_from(entry_index);
                iced::Task::none()
            }

            AppMessage::ToggleEmojiPicker => {
                self.show_emoji_picker = !self.show_emoji_picker;
                iced::Task::none()
            }

            AppMessage::InsertEmoji(ch) => {
                // Insert the emoji at the current cursor position
                self.composer_text.push(ch);
                iced::Task::none()
            }

            AppMessage::ToggleGifPicker => {
                self.show_gif_picker = !self.show_gif_picker;
                if self.show_gif_picker {
                    // Reflect current provider configuration when the picker
                    // opens, so the provider-not-configured state shows
                    // immediately instead of on first search.
                    self.gif_not_configured = boru_core::default_gif_provider().is_err();
                    self.gif_results.clear();
                    self.gif_preview_cache.clear();
                    self.gif_error = None;
                    self.gif_append_error = None;
                    self.gif_has_searched = false;
                    self.gif_next_cursor = None;
                    if self.gif_not_configured {
                        self.gif_loading = false;
                        return iced::Task::none();
                    }
                    // Show trending GIFs as suggestions before any search.
                    self.gif_showing_trending = true;
                    return self.start_gif_trending(None);
                }
                self.gif_loading = false;
                // User closed the picker while a request was in flight:
                // invalidate the request sequence so the late response is
                // dropped by the stale-guard instead of mutating hidden
                // state (results, errors, or preview-download tasks) after
                // the panel closed.
                self.gif_request_seq = self.gif_request_seq.wrapping_add(1);
                iced::Task::none()
            }

            AppMessage::GifSearchChanged(text) => {
                self.gif_search_text = text;
                if self.gif_search_text.trim().is_empty() {
                    // Empty query: cancel pending work and show trending again.
                    self.gif_has_searched = false;
                    self.gif_results.clear();
                    self.gif_preview_cache.clear();
                    self.gif_error = None;
                    self.gif_append_error = None;
                    self.gif_next_cursor = None;
                    if self.gif_not_configured {
                        return iced::Task::none();
                    }
                    self.gif_showing_trending = true;
                    return self.start_gif_trending(None);
                }
                // Debounce: schedule a search after a quiet period.  Each
                // keystroke bumps the debounce seq so only the latest timer
                // fires (older timers are ignored by the seq guard).
                let seq = self.gif_debounce_seq.wrapping_add(1);
                self.gif_debounce_seq = seq;
                let task = iced::Task::perform(
                    tokio::time::sleep(std::time::Duration::from_millis(400)),
                    move |_| AppMessage::GifSearchDebounced(seq),
                );
                task
            }

            AppMessage::GifSearchDebounced(seq) => {
                if seq != self.gif_debounce_seq {
                    // A newer keystroke superseded this debounce timer.
                    return iced::Task::none();
                }
                let query = self.gif_search_text.trim().to_string();
                if query.is_empty() {
                    return iced::Task::none();
                }
                self.gif_showing_trending = false;
                self.gif_has_searched = true;
                return self.start_gif_search(query, None);
            }

            AppMessage::GifSearchSubmit => {
                let query = self.gif_search_text.trim().to_string();
                if query.is_empty() {
                    return iced::Task::none();
                }
                // Cancel any pending debounce timer.
                self.gif_debounce_seq = self.gif_debounce_seq.wrapping_add(1);
                self.gif_showing_trending = false;
                self.gif_has_searched = true;
                return self.start_gif_search(query, None);
            }

            AppMessage::GifRetry => {
                // Re-run the request that failed.  `GifSearchSubmit` is a
                // no-op for empty queries, so a dedicated retry is needed
                // when a trending request failed before any query existed.
                if self.gif_not_configured {
                    return iced::Task::none();
                }
                self.gif_error = None;
                self.gif_append_error = None;
                self.gif_loading = false;
                let query = self.gif_search_text.trim().to_string();
                if query.is_empty() || self.gif_showing_trending {
                    self.gif_showing_trending = true;
                    return self.start_gif_trending(None);
                }
                self.gif_showing_trending = false;
                self.gif_has_searched = true;
                return self.start_gif_search(query, None);
            }

            AppMessage::GifSearchResults { seq, page } => {
                // Stale-response guard: an older request completing late must
                // not replace newer results.
                if seq != self.gif_request_seq {
                    return iced::Task::none();
                }
                self.gif_loading = false;
                self.gif_error = None;
                self.gif_append_error = None;
                self.gif_showing_trending = false;
                self.gif_has_searched = true;
                if self.gif_appending {
                    // Pagination: append, deduplicating by provider_id.
                    let mut seen: HashSet<String> =
                        self.gif_results.iter().map(|r| r.provider_id.clone()).collect();
                    for item in page.items {
                        if seen.insert(item.provider_id.clone()) {
                            self.gif_results.push(item);
                        }
                    }
                    self.gif_appending = false;
                } else {
                    self.gif_results = page.items;
                }
                self.gif_next_cursor = page.next_cursor;
                return self.gif_preview_download_tasks();
            }

            AppMessage::GifTrendingResults { seq, page } => {
                if seq != self.gif_request_seq {
                    return iced::Task::none();
                }
                self.gif_loading = false;
                self.gif_error = None;
                self.gif_append_error = None;
                self.gif_showing_trending = true;
                if self.gif_appending {
                    let mut seen: HashSet<String> =
                        self.gif_results.iter().map(|r| r.provider_id.clone()).collect();
                    for item in page.items {
                        if seen.insert(item.provider_id.clone()) {
                            self.gif_results.push(item);
                        }
                    }
                    self.gif_appending = false;
                } else {
                    self.gif_results = page.items;
                }
                self.gif_next_cursor = page.next_cursor;
                return self.gif_preview_download_tasks();
            }

            AppMessage::GifSearchFailed { seq, message } => {
                if seq != self.gif_request_seq {
                    return iced::Task::none();
                }
                self.gif_loading = false;
                let was_appending = self.gif_appending;
                self.gif_appending = false;
                if was_appending {
                    // Load-more failure: keep the already-loaded grid and
                    // surface a compact note under it instead of replacing
                    // results with the full-screen error state.
                    self.gif_append_error = Some(message);
                } else {
                    self.gif_error = Some(message);
                }
                iced::Task::none()
            }

            AppMessage::GifPreviewLoaded(provider_id, bytes) => {
                self.gif_preview_cache.insert(provider_id, bytes);
                iced::Task::none()
            }

            AppMessage::GifLoadMore => {
                if self.gif_loading {
                    return iced::Task::none();
                }
                let Some(cursor) = self.gif_next_cursor.clone() else {
                    return iced::Task::none();
                };
                self.gif_appending = true;
                if self.gif_showing_trending {
                    return self.start_gif_trending(Some(cursor));
                }
                let query = self.gif_search_text.trim().to_string();
                if query.is_empty() {
                    self.gif_appending = false;
                    return iced::Task::none();
                }
                return self.start_gif_search(query, Some(cursor));
            }

            AppMessage::SendGif(gif) => {
                // Provider-neutral handoff: `gif` is a GifSearchResult (not a
                // KLIPY type).  Build KLIPY-06's SharedGif chat payload from
                // it and broadcast the signed message — receivers fetch the
                // rendition URLs directly (no sender-side full-size download,
                // no API key, no search query on the wire).  The sender's own
                // bubble renders through the same pending-GIF fetch path used
                // for remote receipts (gossip does not echo own broadcasts).
                let shared_gif =
                    boru_core::gif_provider::SharedGif::from_search_result(&gif);
                let message = crate::Message::SharedGif {
                    gif: shared_gif.clone(),
                };
                let message_hash = message_hash(&message);
                self.show_gif_picker = false;
                self.gif_search_text.clear();
                let encoded = match SignedMessage::sign_and_encode(&self.secret_key, &message) {
                    Ok(e) => e,
                    Err(e) => {
                        self.toast_counter = self.toast_counter.wrapping_add(1);
                        self.toast_message = Some(format!("Failed to send GIF: {e}"));
                        return iced::Task::none();
                    }
                };
                let sender = self.sender.clone();
                let sender_ready = self.sender_ready;
                let neighbor_count = self.neighbors.len();
                let broadcast_task = iced::Task::perform(
                    async move {
                        if sender_ready && neighbor_count > 0 {
                            if let Some(sender) = sender {
                                if sender.broadcast(encoded).await.is_err() {
                                    warn!("SharedGif broadcast failed");
                                }
                            }
                        } else {
                            info!(
                                sender_ready,
                                neighbor_count,
                                "SharedGif queued for retry (no mesh yet)"
                            );
                        }
                    },
                    |_| AppMessage::Noop,
                );
                // Local echo: render the sender's own bubble via the standard
                // pending-GIF fetch path (same as a remote receipt).
                self.set_pending_gif(shared_gif, self.local_public, message_hash);
                let fetch_task = self.start_next_pending_gif_fetch();
                iced::Task::batch([broadcast_task, fetch_task])
            }
            AppMessage::PlayInlineVideo(entry_index) => {
                #[cfg(feature = "video-playback")]
                {
                    tracing::info!(entry_index, "PlayInlineVideo called");
                    if !self.video_runtime.available {
                        tracing::warn!("PlayInlineVideo: video runtime unavailable");
                        self.push_system(self.video_runtime.unavailable_message());
                        return iced::Task::none();
                    }
                    let Some(entry) = self.entries.get(entry_index) else {
                        tracing::warn!("PlayInlineVideo: entry not found");
                        return iced::Task::none();
                    };
                    let Some(download) = entry.download.as_ref() else {
                        tracing::warn!("PlayInlineVideo: no download attached");
                        return iced::Task::none();
                    };
                    tracing::info!(
                        state=?download.state,
                        name=%download.name,
                        has_ticket=!download.ticket.is_empty(),
                        has_hash=download.expected_content_hash.is_some(),
                        "PlayInlineVideo: download state",
                    );
                    // Determine play source: only play from a fully downloaded file.
                    // `shared_path` marks the sender's own card
                    // (DownloadState::Shared) whose path is the user-selected
                    // source file outside the managed downloads directory —
                    // identity is still verified, containment is relaxed.
                    let (play_path, play_total_size, shared_path) =
                        if let DownloadState::Completed {
                            saved_path: Some(path),
                            total_size,
                            ..
                        } = &download.state
                        {
                            (path.clone(), *total_size, false)
                        } else if let DownloadState::Shared { path, .. } = &download.state {
                            if path.exists() {
                                (path.clone(), None, true)
                            } else {
                                self.push_system("Shared file is no longer available.");
                                return iced::Task::none();
                            }
                        } else if !download.ticket.is_empty() {
                        // Not yet downloaded — start the download and
                        // inform the user to click play again when
                        // complete.  We intentionally avoid streaming
                        // the file over a local HTTP server because that
                        // machinery adds fragility; download-then-play is
                        // simpler and avoids breaking the gossip
                        // transport with oversized messages.
                        let total_size = match &download.state {
                            DownloadState::Ready { total } => total.unwrap_or(0),
                            DownloadState::Active { total, .. } => total.unwrap_or(0),
                            DownloadState::Completed { total_size, .. } => total_size.unwrap_or(0),
                            _ => 0,
                        };
                        if total_size == 0 {
                            self.push_system("Cannot download video: unknown size.");
                            return iced::Task::none();
                        }
                        // If a download is already active, just inform
                        // the user to wait.
                        if matches!(download.state, DownloadState::Active { .. }) {
                            self.push_system(
                                "Download in progress — click play again when complete.",
                            );
                            return iced::Task::none();
                        }
                        let name = download.name.clone();
                        let expected_hash = download.expected_content_hash.clone();
                        let data_dir = self.data_dir.clone();
                        let blob_store = self.blob_store.clone();
                        let endpoint = self.endpoint.clone();
                        let neighbors = self.neighbors.clone();
                        let progress_queue = self.download_progress_queue.clone();
                        let kind = download.kind;
                        let ticket = download.ticket.clone();

                        // Mark download as Active so the UI shows progress.
                        if let Some(download) = self
                            .entries
                            .get_mut(entry_index)
                            .and_then(|entry| entry.download.as_mut())
                        {
                            download.state = DownloadState::Active {
                                bytes: 0,
                                total: Some(total_size),
                            };
                        }
                        self.layout_cache.borrow_mut().invalidate_from(entry_index);

                        // VIDCARD-fix: run the download through
                        // iced::Task::perform and dispatch DownloadDone on
                        // completion. The previous tokio::spawn pushed only
                        // TransferProgress events, so the queued Completed
                        // event flipped the card to the "Verifying"
                        // placeholder (Completed { saved_path: None }) and
                        // nothing ever upgraded it with the real path — the
                        // video stayed stuck at Verifying forever even
                        // though the file existed on disk.
                        let task_name = name.clone();
                        return iced::Task::perform(
                            async move {
                                let dl_dir = data_dir.join("downloads");
                                let _ = tokio::fs::create_dir_all(&dl_dir).await;
                                // BORU-AUDIT-21: reserve atomically instead of
                                // checking a path and reopening it later.
                                let mut destination = match boru_core::safe_destination::reserve_download_destination(
                                    &dl_dir,
                                    &task_name,
                                    "download",
                                    boru_core::safe_destination::OverwritePolicy::KeepBoth,
                                )
                                .map_err(|e| format!("Unsafe download name: {e}"))?
                                {
                                    boru_core::safe_destination::Reservation::Use(dest) => dest,
                                    boru_core::safe_destination::Reservation::Skip => {
                                        return Err("Download skipped: destination name already exists".into());
                                    }
                                };

                                let parsed: iroh_blobs::ticket::BlobTicket = ticket
                                    .parse()
                                    .map_err(|e| format!("Invalid ticket: {e}"))?;
                                let (addr, hash, _format) = parsed.into_parts();
                                let candidates = download_candidates(addr.id, &neighbors);

                                download_blob_to_file(
                                    &blob_store,
                                    &endpoint,
                                    hash,
                                    candidates,
                                    task_name.clone(),
                                    kind,
                                    &mut destination,
                                    expected_hash.as_deref(),
                                    move |ev| {
                                        if let Ok(mut q) = progress_queue.lock() {
                                            q.push_back(ev);
                                        }
                                    },
                                    Some(total_size),
                                )
                                .await
                                .map_err(|e| format!("Download failed: {e}"))?;
                                let save_path = destination
                                    .publish()
                                    .map_err(|e| format!("Publish failed: {e}"))?;
                                Ok::<_, String>((task_name, save_path))
                            },
                            |result| match result {
                                Ok((name, save_path)) => {
                                    AppMessage::DownloadDone(name, save_path)
                                }
                                Err(e) => AppMessage::DownloadFailed(e),
                            },
                        );
                    } else {
                        self.push_system("Video is not ready to play yet.");
                        return iced::Task::none();
                    };
                    let path = play_path.clone();
                    let Some(expected_hash) = download.expected_content_hash.clone() else {
                        self.push_system(
                            "Video cannot be played because its content identity is missing.",
                        );
                        return iced::Task::none();
                    };
                    let expected_size = play_total_size;
                    let downloads_root = self.data_dir.join("downloads");
                    let message_id = entry.event_id;
                    let attachment_id = download.name.clone();
                    if let Err(error) = validate_attachment_filename(&download.name) {
                        self.push_system(format!("Video verification failed: {error}"));
                        return iced::Task::none();
                    }
                    let verify_result = if shared_path {
                        // Sender's own upload: the user-selected source file
                        // lives outside the managed downloads directory.
                        // Identity (hash + size) is still fully checked.
                        verify_local_attachment_unmanaged(
                            &path,
                            &downloads_root,
                            &expected_hash,
                            expected_size,
                        )
                    } else {
                        verify_local_attachment(
                            &path,
                            &downloads_root,
                            &expected_hash,
                            expected_size,
                        )
                    };
                    if let Err(error) = verify_result {
                        self.push_system(format!("Video verification failed: {error}"));
                        return iced::Task::none();
                    }
                    if let Some(download) = self
                        .entries
                        .get_mut(entry_index)
                        .and_then(|entry| entry.download.as_mut())
                    {
                        // Retry only recreates the decoder; the verified local
                        // attachment is not downloaded again.
                        download.playback_error = None;
                    }
                    let key = VideoInstanceKey::new(self.topic, message_id, attachment_id);
                    if self.playback_coordinator.active_video() == Some(&key) {
                        if let Some(session) = self.inline_video.as_mut().filter(|s| s.key == key) {
                            if let Some(video) = session.video.as_mut().and_then(Arc::get_mut) {
                                video.set_paused(!video.paused());
                                if video.paused() {
                                    // Manual pause ends the current talkspurt:
                                    // raise the keepalive floor so stale frames
                                    // from before the pause are dropped on
                                    // resume.
                                    let framerate = video.framerate();
                                    if framerate.is_finite() && framerate > 0.0 {
                                        let position = video.position();
                                        let floor =
                                            (position.as_secs_f64() * framerate).floor() as u32;
                                        session.jitter.reset_after_keepalive(floor);
                                    }
                                }
                                self.layout_cache.borrow_mut().clear();
                                return iced::Task::none();
                            }
                        }
                    }
                    let _previous = self.playback_coordinator.request_play(key.clone());
                    self.inline_video = Some(InlineVideoSession {
                        key: key.clone(),
                        video: None,
                        error: None,
                        // Fresh talkspurt: the first observed frame anchors
                        // playout after the default jitter delay.
                        jitter: VideoJitterBuffer::default(),
                        resume_position: self
                            .inline_video_resume
                            .as_ref()
                            .filter(|(resume_key, _)| resume_key == &key)
                            .map(|(_, position)| *position)
                            .unwrap_or_default(),
                        last_near_viewport: Instant::now(),
                        streaming_server: None,
                        controls_visible: true,
                        controls_last_interaction: Instant::now(),
                        controls_focused: false,
                    });
                    self.inline_video_resume = None;
                    self.layout_cache.borrow_mut().invalidate_from(entry_index);
                    return iced::Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                // Same relaxed containment for the sender's
                                // own Shared file as the pre-flight check
                                // above; identity is still fully verified.
                                let canonical = if shared_path {
                                    verify_local_attachment_unmanaged(
                                        &path,
                                        &downloads_root,
                                        &expected_hash,
                                        expected_size,
                                    )?
                                } else {
                                    verify_local_attachment(
                                        &path,
                                        &downloads_root,
                                        &expected_hash,
                                        expected_size,
                                    )?
                                };
                                let uri = url::Url::from_file_path(&canonical)
                                    .map_err(|()| "cannot create file URI".to_string())?;
                                Video::new(&uri).map_err(|e| e.to_string())
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|result| result)
                        },
                        move |result| match result {
                            Ok(mut video) => {
                                video.set_paused(false);
                                AppMessage::InlineVideoEvent(InlineVideoEvent::Loaded {
                                    key,
                                    video: Arc::new(video),
                                })
                            }
                            Err(error) => AppMessage::InlineVideoEvent(InlineVideoEvent::Failed {
                                key,
                                error,
                            }),
                        },
                    );
                }
                #[cfg(not(feature = "video-playback"))]
                {
                    let Some(entry) = self.entries.get(entry_index) else {
                        return iced::Task::none();
                    };
                    let Some(download) = entry.download.as_ref() else {
                        return iced::Task::none();
                    };
                    // If already downloaded, open externally
                    if let DownloadState::Completed {
                        saved_path: Some(_),
                        ..
                    } = &download.state
                    {
                        return self.update(AppMessage::OpenDownloadedFile(download.name.clone()));
                    }
                    // If undownloaded but has ticket, stream it
                    if !download.ticket.is_empty() {
                        if let Some(task) = self.stream_for_external_play(entry_index, download) {
                            return task;
                        }
                    }
                    self.push_system("Video is not ready to play yet.");
                }
                iced::Task::none()
            }
            AppMessage::StreamInlineVideo(entry_index) => {
                #[cfg(feature = "video-playback")]
                {
                    tracing::info!(entry_index, "StreamInlineVideo called");
                    if !self.video_runtime.available {
                        tracing::warn!("StreamInlineVideo: video runtime unavailable");
                        self.push_system(self.video_runtime.unavailable_message());
                        return iced::Task::none();
                    }
                    let Some(entry) = self.entries.get(entry_index) else {
                        tracing::warn!("StreamInlineVideo: entry not found");
                        return iced::Task::none();
                    };
                    let Some(download) = entry.download.as_ref() else {
                        tracing::warn!("StreamInlineVideo: no download attached");
                        return iced::Task::none();
                    };
                    tracing::info!(
                        state=?download.state,
                        name=%download.name,
                        has_ticket=!download.ticket.is_empty(),
                        has_hash=download.expected_content_hash.is_some(),
                        "StreamInlineVideo: download state",
                    );
                    // If the video is already fully downloaded, progressive
                    // streaming adds nothing — just play the local file.
                    let fully_downloaded = match &download.state {
                        DownloadState::Completed {
                            saved_path: Some(path),
                            ..
                        } => path.exists(),
                        DownloadState::Shared { path, .. } => path.exists(),
                        _ => false,
                    };
                    if fully_downloaded {
                        return self.update(AppMessage::PlayInlineVideo(entry_index));
                    }
                    // A known total size is required for Content-Length.
                    let total_size = match &download.state {
                        DownloadState::Ready { total } => total.unwrap_or(0),
                        DownloadState::Active { total, .. } => total.unwrap_or(0),
                        DownloadState::Paused { total, .. } => total.unwrap_or(0),
                        DownloadState::Completed { total_size, .. } => total_size.unwrap_or(0),
                        _ => 0,
                    };
                    if total_size == 0 {
                        self.push_system("Cannot stream video: unknown file size.");
                        return iced::Task::none();
                    }
                    let content_hash = match download.expected_content_hash.clone() {
                        Some(hash) => hash,
                        None => {
                            self.push_system(
                                "Cannot stream video: missing content identity.",
                            );
                            return iced::Task::none();
                        }
                    };
                    let task_content_hash = content_hash.clone();
                    let name = download.name.clone();
                    let kind = download.kind;
                    let ticket_str = download.ticket.clone();
                    let is_folder = download.is_folder;
                    let data_dir = self.data_dir.clone();
                    let blob_store = self.blob_store.clone();
                    let endpoint = self.endpoint.clone();
                    let neighbors = self.neighbors.clone();
                    let progress_queue = self.download_progress_queue.clone();

                    // If the download hasn't started yet, begin it now so the
                    // blob-store file (which the streaming server serves)
                    // starts growing immediately.
                    let mut tasks = Vec::new();
                    if matches!(download.state, DownloadState::Ready { .. }) {
                        if let Some(e) = self.entries.get_mut(entry_index) {
                            if let Some(ref mut d) = e.download {
                                let total = match &d.state {
                                    DownloadState::Ready { total } => *total,
                                    _ => None,
                                };
                                d.state = DownloadState::Active { bytes: 0, total };
                            }
                        }
                        self.layout_cache.borrow_mut().invalidate_from(entry_index);
                        self.download_entry_index = Some(entry_index);
                        let task_data_dir = data_dir.clone();
                        let task_name = name.clone();
                        let task_ticket = ticket_str.clone();
                        let task_kind = kind;
                        let task_is_folder = is_folder;
                        let task_blob_store = blob_store.clone();
                        let task_endpoint = endpoint.clone();
                        let task_neighbors = neighbors.clone();
                        let task_progress_queue = progress_queue.clone();
                        // VIDCARD-fix: the previous tokio::spawn discarded the
                        // download result and never dispatched DownloadDone, so
                        // when the stream-triggered download finished, the
                        // queued TransferProgress::Completed left the card at
                        // the "Verifying" placeholder forever. Route the
                        // completion through DownloadDone (same as the
                        // non-stream path) so the card leaves Verifying and
                        // becomes playable once the file is on disk.
                        tasks.push(iced::Task::perform(
                            async move {
                                let ticket: iroh_blobs::ticket::BlobTicket = task_ticket
                                    .parse()
                                    .map_err(|e| format!("Invalid ticket: {e}"))?;
                                let (addr, hash, _format) = ticket.into_parts();
                                let candidates =
                                    download_candidates(addr.id, &task_neighbors);
                                let dl_dir = task_data_dir.join("downloads");
                                let _ = tokio::fs::create_dir_all(&dl_dir).await;
                                if task_is_folder {
                                    let save_dir = boru_core::collection_transfer::download_collection_to_dir(
                                        &task_blob_store,
                                        &task_endpoint,
                                        hash,
                                        candidates,
                                        &task_name,
                                        &dl_dir,
                                    )
                                    .await
                                    .map_err(|e| format!("Folder download failed: {e}"))?;
                                    return Ok::<_, String>((task_name, save_dir));
                                }
                                // BORU-AUDIT-21: reserve atomically instead of
                                // checking a path and reopening it later.
                                let mut destination = match boru_core::safe_destination::reserve_download_destination(
                                    &dl_dir,
                                    &task_name,
                                    &task_content_hash,
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
                                    &task_blob_store,
                                    &task_endpoint,
                                    hash,
                                    candidates,
                                    task_name.clone(),
                                    task_kind,
                                    &mut destination,
                                    Some(&task_content_hash),
                                    move |ev| {
                                        if let Ok(mut q) = task_progress_queue.lock() {
                                            q.push_back(ev);
                                        }
                                    },
                                    None,
                                )
                                .await
                                .map_err(|e| format!("Download failed: {e}"))?;
                                let save_path = destination
                                    .publish()
                                    .map_err(|e| format!("Publish failed: {e}"))?;
                                Ok::<_, String>((task_name, save_path))
                            },
                            |result| match result {
                                Ok((name, save_path)) => {
                                    AppMessage::DownloadDone(name, save_path)
                                }
                                Err(e) => AppMessage::DownloadFailed(e),
                            },
                        ));
                    }

                    // The growing file lives in the FsStore data directory:
                    // <data_dir>/blobs/data/<hex>.data. The downloader writes
                    // into this file progressively as chunks arrive, so a
                    // Range-capable HTTP server can serve playback before the
                    // download completes.
                    let store_data_path = data_dir
                        .join("blobs")
                        .join("data")
                        .join(format!("{content_hash}.data"));
                    let content_type = Self::content_type_for_filename(&name);
                    tasks.push(iced::Task::perform(
                        async move {
                            StreamingServer::start(store_data_path, total_size, content_type)
                                .await
                                .map(|server| (server.url(), Arc::new(server)))
                                .map_err(|e| e.to_string())
                        },
                        move |result| match result {
                            Ok((url, server)) => AppMessage::StreamingServerReady {
                                entry_index,
                                url,
                                server,
                            },
                            Err(error) => AppMessage::StreamingServerFailed {
                                entry_index,
                                error,
                            },
                        },
                    ));
                    iced::Task::batch(tasks)
                }
                #[cfg(not(feature = "video-playback"))]
                {
                    // No inline runtime: fall back to download + external open.
                    let Some(entry) = self.entries.get(entry_index) else {
                        return iced::Task::none();
                    };
                    let Some(download) = entry.download.as_ref() else {
                        return iced::Task::none();
                    };
                    if !download.ticket.is_empty() {
                        if let Some(task) = self.stream_for_external_play(entry_index, download) {
                            return task;
                        }
                    }
                    self.push_system("Video is not ready to play yet.");
                    iced::Task::none()
                }
            }
            #[cfg(feature = "video-playback")]
            AppMessage::StreamingServerReady {
                entry_index,
                url,
                server,
            } => {
                let Some(entry) = self.entries.get(entry_index) else {
                    tracing::warn!("StreamingServerReady: entry not found");
                    return iced::Task::none();
                };
                let Some(download) = entry.download.as_ref() else {
                    tracing::warn!("StreamingServerReady: no download attached");
                    return iced::Task::none();
                };
                tracing::info!(entry_index, url = %url, "StreamingServerReady: opening player");
                let message_id = entry.event_id;
                let attachment_id = download.name.clone();
                // The stream is intentionally NOT content-verified: the file
                // is still growing by design. Clear any stale error state.
                if let Some(download) = self
                    .entries
                    .get_mut(entry_index)
                    .and_then(|entry| entry.download.as_mut())
                {
                    download.playback_error = None;
                }
                let key = VideoInstanceKey::new(self.topic, message_id, attachment_id);
                let _previous = self.playback_coordinator.request_play(key.clone());
                self.inline_video = Some(InlineVideoSession {
                    key: key.clone(),
                    video: None,
                    error: None,
                    // Fresh talkspurt: the first observed frame anchors
                    // playout after the default jitter delay.
                    jitter: VideoJitterBuffer::default(),
                    controls_visible: true,
                    controls_last_interaction: Instant::now(),
                    controls_focused: false,
                    resume_position: self
                        .inline_video_resume
                        .as_ref()
                        .filter(|(resume_key, _)| resume_key == &key)
                        .map(|(_, position)| *position)
                        .unwrap_or_default(),
                    last_near_viewport: Instant::now(),
                    streaming_server: Some(server),
                });
                self.inline_video_resume = None;
                self.layout_cache.borrow_mut().invalidate_from(entry_index);
                iced::Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let uri = match url::Url::parse(&url) {
                                Ok(uri) => uri,
                                Err(e) => return Err(format!("invalid stream URL: {e}")),
                            };
                            Video::new(&uri).map_err(|e| e.to_string())
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|result| result)
                    },
                    move |result| match result {
                        Ok(mut video) => {
                            video.set_paused(false);
                            AppMessage::InlineVideoEvent(InlineVideoEvent::Loaded {
                                key,
                                video: Arc::new(video),
                            })
                        }
                        Err(error) => AppMessage::InlineVideoEvent(InlineVideoEvent::Failed {
                            key,
                            error,
                        }),
                    },
                )
            }
            #[cfg(feature = "video-playback")]
            AppMessage::StreamingServerFailed {
                entry_index,
                error,
            } => {
                tracing::warn!(entry_index, %error, "StreamingServerFailed");
                self.push_system(format!("Could not start video stream: {error}"));
                iced::Task::none()
            }
            AppMessage::StreamUrl(url) => {
                self.push_system(Self::external_stream_hint(&url));
                // Open the stream in the OS default player (VLC/browser).
                // Non-fatal: the hint above carries the URL so the user can
                // paste it manually if no default handler is registered.
                let url2 = url.clone();
                iced::Task::perform(
                    async move {
                        if let Err(e) = open::that(&url2) {
                            tracing::warn!(url = %url2, error = %e, "failed to open external stream URL");
                        }
                    },
                    |_| AppMessage::Noop,
                )
            }
            #[cfg(feature = "video-playback")]
            AppMessage::CloseInlineVideo => {
                #[cfg(feature = "video-playback")]
                {
                    self.stop_inline_video();
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoTick => {
                let now = Instant::now();
                if let Some(session) = self.inline_video.as_mut() {
                    if let Some(video) = session.video.as_ref() {
                        // While paused the playhead is frozen; the pause path
                        // already raised the keepalive floor, so there is
                        // nothing to schedule until playback resumes.
                        if !video.paused() {
                            if !session.controls_focused
                                && now.duration_since(session.controls_last_interaction)
                                    >= Duration::from_millis(2800)
                            {
                                session.controls_visible = false;
                            }
                            // Feed the playhead into the deadline-driven
                            // jitter buffer.  The first frame of a talkspurt
                            // (start, resume, or seek) anchors playout after
                            // the jitter delay; every later frame is scheduled
                            // relative to that anchor using the source frame
                            // duration.
                            let framerate = video.framerate();
                            if framerate.is_finite() && framerate > 0.0 {
                                let position = video.position();
                                let seq = (position.as_secs_f64() * framerate).floor() as u32;
                                session.jitter.observe_playhead(seq, now);
                            }
                            // Present every frame whose wall-clock deadline
                            // has arrived.  Losses (deadline passed without
                            // the frame) are counted by the buffer; only
                            // repaint when a frame is actually due instead of
                            // on a fixed timer.
                            let mut present = false;
                            while let Some(due) = session.jitter.pop_due(now) {
                                match due {
                                    Some(_seq) => present = true,
                                    None => {
                                        tracing::debug!(
                                            losses = session.jitter.total_losses(),
                                            "inline video frame deadline missed"
                                        );
                                    }
                                }
                            }
                            if present {
                                self.layout_cache.borrow_mut().clear();
                            }
                        }
                    }
                }
                iced::Task::none()
            }
            #[cfg(not(feature = "video-playback"))]
            AppMessage::InlineVideoShowControls => iced::Task::none(),
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoShowControls => {
                if let Some(session) = self.inline_video.as_mut() {
                    session.controls_visible = true;
                    session.controls_last_interaction = Instant::now();
                    self.layout_cache.borrow_mut().clear();
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoControlsFocused(focused) => {
                if let Some(session) = self.inline_video.as_mut() {
                    session.controls_focused = focused;
                    if focused {
                        // Keyboard focus entered the controls: show them
                        // and reset the idle deadline so they stay visible
                        // (PDF task 18 / AC9).
                        session.controls_visible = true;
                        session.controls_last_interaction = Instant::now();
                    }
                    self.layout_cache.borrow_mut().clear();
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoSeekChanged(value) => {
                self.inline_video_seek = Some(value.clamp(0.0, 1.0));
                if let Some(session) = self.inline_video.as_mut() {
                    session.controls_visible = true;
                    session.controls_last_interaction = Instant::now();
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoSeekReleased => {
                if let (Some(position), Some(session)) =
                    (self.inline_video_seek.take(), self.inline_video.as_mut())
                {
                    session.controls_visible = true;
                    session.controls_last_interaction = Instant::now();
                    if let Some(video) = session.video.as_mut().and_then(Arc::get_mut) {
                        let duration = video.duration();
                        let target = duration.mul_f32(position.clamp(0.0, 1.0));
                        let _ = video.seek(target, false);
                        // A seek starts a fresh talkspurt: drop the previous
                        // anchor, floor, and buffered frames so the first
                        // frame at the target anchors new playout.
                        session.jitter.reset();
                    }
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoSeekRelative(delta_seconds) => {
                if let Some(session) = self.inline_video.as_mut() {
                    if let Some(video) = session.video.as_mut().and_then(Arc::get_mut) {
                        let duration = video.duration();
                        let position = video.position();
                        let target = if delta_seconds.is_sign_negative() {
                            position.saturating_sub(Duration::from_secs_f32(delta_seconds.abs()))
                        } else {
                            position.saturating_add(Duration::from_secs_f32(delta_seconds))
                        }
                        .min(duration);
                        let _ = video.seek(target, false);
                        session.jitter.reset();
                        session.controls_visible = true;
                        session.controls_last_interaction = Instant::now();
                        self.layout_cache.borrow_mut().clear();
                    }
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoToggleMute => {
                if let Some(video) = self
                    .inline_video
                    .as_mut()
                    .and_then(|s| s.video.as_mut())
                    .and_then(Arc::get_mut)
                {
                    video.set_muted(!video.muted());
                    if let Some(session) = self.inline_video.as_mut() {
                        session.controls_visible = true;
                        session.controls_last_interaction = Instant::now();
                    }
                    self.layout_cache.borrow_mut().clear();
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoAdjustVolume(delta) => {
                if let Some(video) = self
                    .inline_video
                    .as_mut()
                    .and_then(|s| s.video.as_mut())
                    .and_then(Arc::get_mut)
                {
                    let value = (video.volume() as f32 + delta).clamp(0.0, 1.0);
                    video.set_volume(value as f64);
                    if let Some(session) = self.inline_video.as_mut() {
                        session.controls_visible = true;
                        session.controls_last_interaction = Instant::now();
                    }
                    self.layout_cache.borrow_mut().clear();
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoSetVolume(value) => {
                if let Some(video) = self
                    .inline_video
                    .as_mut()
                    .and_then(|s| s.video.as_mut())
                    .and_then(Arc::get_mut)
                {
                    video.set_volume(value.clamp(0.0, 1.0) as f64);
                    if let Some(session) = self.inline_video.as_mut() {
                        session.controls_visible = true;
                        session.controls_last_interaction = Instant::now();
                    }
                    self.layout_cache.borrow_mut().clear();
                }
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoToggleExpanded => {
                self.inline_video_expanded = !self.inline_video_expanded;
                if let Some(session) = self.inline_video.as_mut() {
                    session.controls_visible = true;
                    session.controls_last_interaction = Instant::now();
                }
                self.layout_cache.borrow_mut().clear();
                iced::Task::none()
            }
            #[cfg(feature = "video-playback")]
            AppMessage::InlineVideoEvent(event) => {
                match event {
                    InlineVideoEvent::Loaded { key, video } => {
                        if let Some(session) = self.inline_video.as_mut().filter(|s| s.key == key) {
                            let resume_position = session.resume_position;
                            let mut video = video;
                            if let Some(video) = Arc::get_mut(&mut video) {
                                if resume_position > Duration::ZERO {
                                    let _ = video.seek(resume_position, false);
                                }
                                video.set_paused(false);
                            }
                            // Adopt the real source frame duration once the
                            // decoder reports its framerate.
                            let framerate = video.framerate();
                            if framerate.is_finite() && framerate > 0.0 {
                                session
                                    .jitter
                                    .set_frame_duration(Duration::from_secs_f64(1.0 / framerate));
                            }
                            session.video = Some(video);
                            session.error = None;
                            self.layout_cache.borrow_mut().clear();
                        }
                    }
                    InlineVideoEvent::Failed { key, error }
                    | InlineVideoEvent::Error { key, error } => {
                        if self.inline_video.as_ref().is_some_and(|s| s.key == key) {
                            let playback_error = InlinePlaybackError::from_backend(&error);
                            tracing::warn!(
                                message_id = key.message_id,
                                attachment_id = %key.attachment_id,
                                category = ?playback_error.kind,
                                diagnostic = %playback_error.detail,
                                "inline video playback failed"
                            );
                            if let Some(entry) = self.entries.iter_mut().find(|entry| {
                                entry.event_id == key.message_id
                                    && entry
                                        .download
                                        .as_ref()
                                        .is_some_and(|d| d.kind == TransferKind::Video)
                            }) {
                                if let Some(download) = entry.download.as_mut() {
                                    download.playback_error = Some(playback_error);
                                }
                            }
                            self.inline_video = None;
                            // The error card remains available for retry, but
                            // the failed player must no longer reserve the
                            // coordinator's active slot.
                            self.playback_coordinator.clear(Some(&key));
                            self.inline_video_seek = None;
                            self.inline_video_expanded = false;
                            self.layout_cache.borrow_mut().clear();
                        }
                    }
                    InlineVideoEvent::Ended { key } => {
                        if self.inline_video.as_ref().is_some_and(|s| s.key == key) {
                            self.stop_inline_video();
                        }
                    }
                }
                iced::Task::none()
            }
            AppMessage::OpenImageLightbox(entry_index) => {
                self.lightbox_image = Some(entry_index);
                iced::Task::none()
            }
            AppMessage::CloseImageLightbox => {
                self.lightbox_image = None;
                // Requirement (CHAT-SCROLL): closing the lightbox must
                // ALWAYS return the chat log to the latest message (bottom),
                // like a normal messenger — even when the user had scrolled
                // up before opening the image.  Force follow-latest, re-arm
                // the f32::MAX bottom sentinel, and queue the snap.
                self.follow_latest = true;
                self.scroll_offset = f32::MAX;
                self.scroll_to_bottom_pending = true;
                // The lightbox overlay is a `stack![base, overlay]` wrapper;
                // removing it re-creates the windowed scrollable, whose
                // first `Scrolled(0, vp)` event would clobber the sentinel
                // before the snap task lands.  Arm the stale-event guard so
                // non-bottom events keep the sentinel and re-queue the snap
                // until a bottom event confirms arrival.
                self.lightbox_close_snap_guard = 3;
                iced::Task::none()
            }
            AppMessage::ClearHistoryRequested => {
                if self.history_clear_pending {
                    return iced::Task::none();
                }
                self.history_clear_feedback = None;
                self.history_clear_feedback_is_error = false;
                self.history_confirm_clear = !self.history_confirm_clear;
                if !self.history_confirm_clear {
                    self.complete_close_dialog_action();
                }
                iced::Task::none()
            }

            AppMessage::ConfirmClearHistory => {
                if self.history_clear_pending {
                    return iced::Task::none();
                }
                self.history_clear_pending = true;
                self.history_clear_feedback = None;
                self.history_clear_feedback_is_error = false;
                let topic = self.topic;
                let room_history = self.room_history.clone();
                let chat_history = self.chat_history.clone();
                if let Some(storage) = &self.storage {
                    let _ = storage.delete_outgoing_for_topic(&topic);
                }
                iced::Task::perform(
                    async move {
                        let mut room_history = room_history;
                        let mut chat_history = chat_history.lock().unwrap();
                        let report =
                            clear_room_history(topic, &mut room_history, &mut chat_history, None)
                                .map_err(|err| err.to_string())?;
                        Ok::<_, String>((topic, room_history, report))
                    },
                    move |result| match result {
                        Ok((topic, room_history, report)) => AppMessage::ClearHistoryFinished {
                            topic,
                            room_history,
                            report,
                        },
                        Err(error) => AppMessage::ClearHistoryFailed { topic, error },
                    },
                )
            }

            AppMessage::ClearHistoryFinished {
                topic,
                room_history: _,
                report,
            } => {
                self.history_clear_pending = false;
                self.history_confirm_clear = false;
                self.history_clear_feedback_is_error = false;
                self.history_clear_feedback = Some(format!(
                    "Cleared {} messages from this chat.",
                    report.chat_entries_removed
                ));
                self.clear_current_room_history_runtime(topic, &report);
                iced::Task::none()
            }

            AppMessage::ClearHistoryFailed { topic: _, error } => {
                self.history_clear_pending = false;
                self.history_confirm_clear = true;
                self.history_clear_feedback_is_error = true;
                self.history_clear_feedback = Some(error.clone());
                iced::Task::none()
            }

            AppMessage::DeleteRoomRequested(topic) => {
                // Toggle confirmation for this topic.
                self.room_delete_confirm_topic = if self.room_delete_confirm_topic == Some(topic) {
                    None
                } else {
                    Some(topic)
                };
                if self.room_delete_confirm_topic.is_none() {
                    self.complete_close_dialog_action();
                }
                iced::Task::none()
            }

            AppMessage::ConfirmDeleteRoom(topic) => {
                self.room_delete_confirm_topic = None;
                // Shutdown continuous DHT tracker for this room if one exists.
                if let Some(tracker) = self.room_trackers.remove(&topic) {
                    tracker.shutdown_shared();
                }
                if let Err(err) = self.purge_room_history(topic) {
                    self.push_system(format!("Could not delete room history: {err}"));
                }
                // Remove from conversation store and persist so the deletion
                // survives a restart.
                self.conversations.remove(&topic);
                self.conversation_store.remove(&topic);
                self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                self.refresh_sidebar_counts();
                // Also remove from the SQLite message store so the chat
                // messages and conversation metadata don't linger on disk.
                let store_path = self.data_dir.join("message_store.db");
                if store_path.exists() {
                    match MessageStore::open(&store_path) {
                        Ok(store) => {
                            let topic_bytes = topic.as_bytes();
                            if let Err(err) = store.delete_messages_for_topic(topic_bytes) {
                                warn!("failed to delete messages for topic: {err}");
                            }
                            // Remove conversation metadata so it cannot
                            // resurrect after a backfill or restart.
                            if let Err(err) = store.hard_delete_conversation(topic_bytes) {
                                warn!("failed to delete conversation meta: {err}");
                            }
                        }
                        Err(err) => {
                            warn!("failed to open message store for cleanup: {err}");
                        }
                    }
                }
                if matches!(&self.screen, Screen::Chat { topic: t } if t == &topic) {
                    self.screen = Screen::ChatList;
                }
                iced::Task::none()
            }

            AppMessage::MailboxReplayed { peer, texts } => {
                let n = texts.len();
                let label = self
                    .names
                    .get(&peer)
                    .cloned()
                    .unwrap_or_else(|| peer.fmt_short().to_string());
                for (_msg_id, text) in texts {
                    let entry = ChatEntry::remote(
                        format!("Offline DM from {label}"),
                        text,
                        None,
                        None,
                        Some(peer),
                    );
                    self.entries_push(entry);
                }
                if n > 0 {
                    self.push_system(format!(
                        "[Offline DM sync: received {n} message{} from {label}]",
                        if n == 1 { "" } else { "s" }
                    ));
                }
                iced::Task::none()
            }

            // ── Conversation selection / management ─────────────────
            AppMessage::OpenConversation(peer) => {
                // Derive topic, ensure conversation record exists, and select.
                let topic = direct_topic(&self.local_public, &peer);
                let fid = FriendId::from_public_key(peer);
                let record = self.friends.ensure_friend(fid);
                record.set_direct_conversation(topic, DirectConversationState::Active);
                self.conversation_store
                    .upsert(boru_core::conversations::ConversationEntry::new(
                        topic,
                        peer.to_string(),
                        peer.fmt_short().to_string(),
                    ));
                self.try_save_friends();
                iced::Task::done(AppMessage::OpenRoom(topic))
            }

            AppMessage::SelectConversation(topic) => {
                // UI-only switch — does NOT create or subscribe.
                iced::Task::done(AppMessage::OpenRoom(topic))
            }

            AppMessage::CloseConversation(topic) => {
                // Remove conversation from local list without affecting friendship,
                // subscriptions, or the live forwarder. The conversation stays
                // subscribed in the background.
                self.save_room_to_history();
                self.room_history.remove(&topic);
                self.room_history_dirty = true;
                self.persist_room_history();
                // Archive in conversation store
                if let Some(entry) = self.conversation_store.find_mut(&topic) {
                    entry.archived = true;
                    self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);
                }
                // If this was the displayed conversation, go back to chat list
                if topic == self.topic {
                    self.screen = Screen::ChatList;
                }
                iced::Task::none()
            }
            AppMessage::Scrolled(offset, vp_h) => {
                // Mirror the scrollable's offset into the windowed-renderer
                // state and track whether the user is at the bottom.
                // total_content_height is set during view_chat_log() each
                // frame via Cell interior mutability (allows &self reads in
                // view()).
                let total = self.total_content_height.get();
                if total > 0.0 {
                    self.viewport_height = vp_h;
                    // Detect whether the user is at the bottom of the chat
                    // log.  The 10px epsilon absorbs sub-pixel rounding and
                    // viewport re-measurement during resize.
                    if offset + vp_h >= total - 10.0 {
                        self.follow_latest = true;
                        self.scroll_offset = offset;
                        // A bottom event confirms the lightbox-close snap
                        // landed (or the user reached the bottom); the
                        // stale-event guard is no longer needed.
                        self.lightbox_close_snap_guard = 0;
                    } else if self.lightbox_close_snap_guard > 0 {
                        // Stale event from the freshly re-created scrollable
                        // right after the lightbox overlay disappeared: the
                        // fresh widget starts at the TOP (offset 0) and this
                        // event would clobber the f32::MAX bottom sentinel
                        // before the snap task lands.  Keep the sentinel
                        // armed, stay in follow-latest, and re-queue the snap
                        // (the update tail consumes the flag and re-emits
                        // snap_to_end).  A genuine user scroll cannot arrive
                        // in this window because the overlay removal and the
                        // first layout happen in the same frame.
                        self.scroll_offset = f32::MAX;
                        self.scroll_to_bottom_pending = true;
                        self.lightbox_close_snap_guard -= 1;
                    } else {
                        self.scroll_offset = offset;
                        self.follow_latest = false;
                        // A manual scroll away from the bottom cancels any
                        // queued snap-to-bottom so a stale snap can never
                        // steal the user's reading position.
                        self.scroll_to_bottom_pending = false;
                    }
                } else {
                    // Empty timeline: the anchor-bottom empty-state
                    // scrollable reports offset 0 with no content.  Clobbering
                    // `scroll_offset` here would destroy the `f32::MAX`
                    // bottom sentinel that keeps follow-latest armed until the
                    // first entry renders (RoomOpened history replay / live
                    // append) — which is what landed fresh conversations at
                    // the TOP of history instead of the latest message.  Only
                    // learn the viewport height; leave the sentinel untouched.
                    self.viewport_height = vp_h;
                }
                #[cfg(feature = "video-playback")]
                self.reconcile_inline_video_viewport();
                iced::Task::none()
            }
            AppMessage::SendMessage {
                conversation_topic,
                content,
            } => {
                // Validate that this conversation exists
                if !self.conversations.contains_key(&conversation_topic) {
                    warn!("SendMessage: unknown conversation {conversation_topic:?}");
                    return iced::Task::none();
                }
                // If sending to the active conversation, use the normal flow
                if conversation_topic == self.topic {
                    self.composer_text = content;
                    // Fall through to SendPressed logic
                    let trimmed = self.composer_text.trim().to_string();
                    if trimmed.is_empty() {
                        return iced::Task::none();
                    }
                    self.composer_text.clear();
                    let text = trimmed.clone();
                    match self.persist_outgoing_message(self.topic, &trimmed) {
                        Ok((event_id, msg_hash, encoded)) => {
                            self.self_sent_events.insert(msg_hash, event_id);
                            let mut local_entry = ChatEntry::local(&self.local_label, &text);
                            local_entry.event_id = event_id;
                            local_entry.message_hash = Some(msg_hash);
                            let _entry_idx = self.entries_push(local_entry);
                            Self::broadcast_or_queue(
                                encoded,
                                self.sender.clone(),
                                self.sender_ready,
                                self.neighbors.len(),
                                text,
                                event_id,
                                msg_hash,
                                None,
                            )
                        }
                        Err(e) => iced::Task::done(AppMessage::ErrorMsg(e)),
                    }
                } else {
                    // For background conversations, use the ConversationLive's sender
                    let text = content;
                    match self.persist_outgoing_message(conversation_topic, &text) {
                        Ok((event_id, msg_hash, encoded)) => {
                            if let Some(conv) = self.conversations.get_mut(&conversation_topic) {
                                conv.self_sent_events.insert(msg_hash, event_id);
                                let mut local_entry = ChatEntry::local(&self.local_label, &text);
                                local_entry.event_id = event_id;
                                local_entry.message_hash = Some(msg_hash);
                                conv.entries.push(local_entry);
                                conv.unread = conv.unread.saturating_add(1);
                                Self::broadcast_or_queue(
                                    encoded,
                                    conv.sender.clone(),
                                    conv.sender_ready,
                                    conv.neighbors.len(),
                                    text,
                                    event_id,
                                    msg_hash,
                                    None,
                                )
                            } else {
                                iced::Task::none()
                            }
                        }
                        Err(e) => iced::Task::done(AppMessage::ErrorMsg(e)),
                    }
                }
            }
            AppMessage::DeleteRoom(topic) => {
                #[cfg(feature = "video-playback")]
                if self.topic == topic {
                    self.stop_inline_video();
                }
                // Shutdown continuous DHT tracker for this room if one exists.
                if let Some(tracker) = self.room_trackers.remove(&topic) {
                    tracker.shutdown_shared();
                }
                if let Err(err) = self.purge_room_history(topic) {
                    self.push_system(format!("Could not delete room history: {err}"));
                }
                iced::Task::none()
            }
            // ── Link preview result (state layer) ──
            AppMessage::LinkPreviewLoaded(idx, result) => {
                tracing::info!(entry_index = idx, result = ?result, "LinkPreviewLoaded fired");
                if idx >= self.entries.len() {
                    tracing::warn!(entry_index = idx, "LinkPreviewLoaded: out of bounds");
                    return iced::Task::none();
                }
                let entry = &mut self.entries[idx];
                match result {
                    link_preview::LinkPreviewResult::Success(data) => {
                        entry.link_preview = Some(data);
                        entry.link_preview_loading = false;
                        entry.link_preview_error = false;
                    }
                    link_preview::LinkPreviewResult::Error(e) => {
                        tracing::info!(entry_index = idx, error = %e, "link preview fetch failed");
                        entry.link_preview_loading = false;
                        entry.link_preview_error = true;
                    }
                    link_preview::LinkPreviewResult::Pending => {
                        // Another task is already fetching this URL.
                        // The first fetch will populate the cache and send
                        // its own `LinkPreviewLoaded` message.
                        return iced::Task::none();
                    }
                }
                entry.bump_gen();
                self.link_preview_fetch_index = None;
                iced::Task::none()
            }
            // update() only dispatches the chat variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
