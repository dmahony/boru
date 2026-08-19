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

#[cfg(feature = "screen-sharing")]
/// BORU-SSUI-04: one segment of the sender's quality segmented control.
/// `preset` is the exact value dispatched on press (`None` = Auto /
/// path-derived auto preset); `selected` marks the single visually
/// active segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QualitySegmentSpec {
    /// i18n label key (runtime source labels, never mockup text).
    pub label_key: &'static str,
    /// Preset dispatched via `ScreenShareSetPreset`.
    pub preset: Option<QualityPreset>,
    /// Whether this segment is the selected one.
    pub selected: bool,
}

#[cfg(feature = "screen-sharing")]
/// BORU-SSUI-05: presentation mapping for the sender's remote-control
/// status area. The permission model is consent-gated — remote control
/// is granted only by the sender in response to an explicit viewer
/// request (the consent prompt) and can be revoked while active; there
/// is no direct sender-side toggle. So this maps the authoritative
/// `screen_share_control_active` mirror to a STATE-ONLY display label
/// (ON/OFF) with an input/control icon. The explicit enable/disable
/// actions (grant/deny consent, revoke) stay separate and keep their
/// existing dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RemoteControlStatusSpec {
    /// i18n label key for the status text ("Remote control: ON/OFF").
    pub label_key: &'static str,
    /// Whether remote control is currently granted (ON).
    pub active: bool,
}

#[cfg(feature = "screen-sharing")]
/// BORU-SSUI-06: presentation mapping for the sender's audio toggle row.
/// The switch OFF maps to the current no-audio state, switch ON maps to the
/// current audio-sharing path; both dispatch the SAME `ScreenShareToggleAudio`
/// message the old button used (capture/session path unchanged). The switch
/// binds to the authoritative `screen_share_audio_active` mirror (set by
/// `SessionEvent::AudioState`), and `enabled` goes false only when the host
/// reported a typed unavailable error (audio cannot be shared) — the switch
/// is then rendered disabled with the reason as tooltip/status text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AudioToggleSpec {
    /// Speaker icon reflecting the audio state (Volume2 = on, VolumeX = off).
    pub icon: Icon,
    /// i18n label key for the "Audio" label.
    pub label_key: &'static str,
    /// Whether the switch can be toggled (false when audio cannot be shared).
    pub enabled: bool,
    /// Whether system audio is currently shared (switch ON).
    pub active: bool,
}

#[cfg(feature = "screen-sharing")]
/// BORU-SSUI-09 (PDF Task 9): how the sender control row lays out its
/// logical groups (quality segmented control, remote-control status, audio
/// toggle) for a viewport tier. The tier is resolved by the shared
/// responsive machinery (`LayoutConfig::responsive::tier_for_width`), so
/// boru-layout.toml `[responsive]` thresholds drive the breakpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SenderControlRowLayout {
    /// All groups share one horizontal row (wide window).
    Row,
    /// One row that may wrap into two logical groups without clipping
    /// (medium width).
    Wrap,
    /// Groups stack vertically, every control fully visible (narrow).
    Stack,
}

#[cfg(feature = "screen-sharing")]
impl SenderControlRowLayout {
    /// Map a viewport width tier to the sender control-row layout mode.
    pub(crate) fn for_tier(tier: crate::layout::ViewportTier) -> Self {
        match tier {
            crate::layout::ViewportTier::Narrow => Self::Stack,
            crate::layout::ViewportTier::Desktop => Self::Wrap,
            crate::layout::ViewportTier::UltraWide => Self::Row,
        }
    }
}

impl IcedChat {
    pub(crate) fn view_chat_panel(&self) -> iced::Element<'_, AppMessage> {
        use iced::{widget, Length};

        #[cfg(feature = "dev-ui")]
        let _designer_components = (
            crate::designer::ComponentId::ChatMessageList,
            crate::designer::ComponentId::ChatComposer,
        );

        // Show a loading spinner while the gossip subscription is in flight.
        if self.room_loading {
            const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let spinner = SPINNER_FRAMES[self.splash_spinner_frame % SPINNER_FRAMES.len()];
            let theme = self.theme();
            let btheme = crate::theme::BoruTheme::for_theme(&theme);
            let dark_mode = self.theme() == iced::Theme::Dark;
            return widget::container(
                widget::column![
                    widget::text(spinner)
                        .size(btheme.chat.spinner_size)
                        .color(accent_primary(&theme)),
                    widget::text(crate::i18n::t("chat.loading_conversation"))
                        .size(crate::fonts::TypeRole::Body.size_px())
                        .font(crate::fonts::TypeRole::Body.font())
                        .color(Self::muted_color(dark_mode)),
                    widget::text(crate::i18n::t("chat.setting_up_conversation"))
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
            let btheme = crate::theme::BoruTheme::for_theme(&theme);
            let dark_mode = self.theme() == iced::Theme::Dark;
            return widget::container(
                widget::column![
                    widget::text(spinner)
                        .size(btheme.chat.spinner_size)
                        .color(accent_primary(&theme)),
                    widget::text(crate::i18n::t("status.connecting"))
                        .size(crate::fonts::TypeRole::Body.size_px())
                        .font(crate::fonts::TypeRole::Body.font())
                        .color(Self::muted_color(dark_mode)),
                    widget::text(crate::i18n::t("chat.ready_shortly"))
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
        let timeline_max_width = self.boru_layout().responsive.content_max_width;
        let chat_log = widget::responsive(move |size: iced::Size| {
            // The scrollable viewport spans the FULL chat pane width, so the
            // scrollbar sits flush with the right edge. `timeline_width` stays
            // capped at the readable conversation column so bubble/card sizing
            // and the layout cache are unchanged.
            self.view_chat_log(size.width.min(timeline_max_width), size.height)
                .into()
        });
        // The readable-column cap (content_max_width) is applied to the
        // message content INSIDE the scrollable (see `view_chat_log`),
        // never to the scrollable viewport itself — the scrollbar must
        // hug the far-right edge of the chat pane.
        let chat_log = widget::container(chat_log).width(Length::Fill);
        #[cfg(feature = "dev-ui")]
        let chat_log = crate::designer::overlay(
            crate::designer::ComponentId::ChatMessageList,
            chat_log.into(),
            self.settings_state.designer.enabled,
            self.settings_state.designer.hovered_component,
            self.settings_state.designer.selected_component,
            self.settings_state.designer.resize_operation.as_ref().and_then(|op| {
                (op.component == crate::designer::ComponentId::ChatMessageList)
                    .then_some(self.boru_layout().chat.message_max_width)
            }),
        );
        let composer = self.view_composer();
        let typing_indicator: iced::Element<'_, AppMessage> = match self.screen {
            Screen::Chat { topic } if self.typing_peers.count_for_topic(topic) > 0 => {
                widget::container(widget::text("Someone is typing…").size(TYPO_SM))
                    .padding([2.0, SPACE_8])
                    .width(Length::Fill)
                    .into()
            }
            _ => widget::Space::new().height(Length::Fixed(0.0)).into(),
        };
        #[cfg(feature = "dev-ui")]
        let composer = crate::designer::overlay(
            crate::designer::ComponentId::ChatComposer,
            composer,
            self.settings_state.designer.enabled,
            self.settings_state.designer.hovered_component,
            self.settings_state.designer.selected_component,
            self.settings_state.designer.resize_operation.as_ref().and_then(|op| {
                (op.component == crate::designer::ComponentId::ChatComposer)
                    .then_some(self.boru_layout().chat.bubble_max_width)
            }),
        );
        let content = content
        .push(chat_log)
        .push(typing_indicator)
        .push(composer)
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
                .style(move |t, _status| {
                    let b = crate::theme::BoruTheme::for_theme(t);
                    iced::widget::button::Style {
                        background: Some(iced::Background::Color(
                            b.colors.chat_overlay_backdrop,
                        )),
                        ..Default::default()
                    }
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
                .style(move |t, _status| {
                    let b = crate::theme::BoruTheme::for_theme(t);
                    iced::widget::button::Style {
                        background: Some(iced::Background::Color(
                            b.colors.chat_search_backdrop,
                        )),
                        ..Default::default()
                    }
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
        } else if self.help_overlay.visible() {
            // BORU-APP-002: the overlay composition moved into the
            // help-overlay domain; chat only decides *whether* to show it.
            self.help_overlay.view(inner.into())
        } else if self.show_member_list {
            use iced::widget::Stack;
            use iced::Color;
            let chat_layer = inner;

            let backdrop = widget::button(widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .on_press(AppMessage::ToggleMemberList)
                .style(move |t, _status| {
                    let b = crate::theme::BoruTheme::for_theme(t);
                    iced::widget::button::Style {
                        background: Some(iced::Background::Color(
                            b.colors.chat_overlay_backdrop,
                        )),
                        ..Default::default()
                    }
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
                // ── Emoji / GIF picker overlays ──────────────────────
                // Anchored directly above the composer bar (the row holding
                // the emoji/GIF trigger buttons). The overlay container MUST
                // fill the whole chat panel (height: Fill): iced's Stack
                // places every child at the top-left origin, so a
                // shrink-height container just renders at the top of the
                // chat window and `align_y(Bottom)` has no slack.
                use iced::widget::Stack;

                // Distance from the bottom of the chat panel to the top of
                // the composer bar: inner bottom padding (SPACE_12) +
                // status footer (~21) + spacer (SPACE_8) + composer bar
                // (~46 = 36px send button + 8px row padding + 2px border).
                const COMPOSER_OFFSET: f32 = SPACE_12 + 21.0 + SPACE_8 + 46.0;
                const PICKER_GAP: f32 = SPACE_8;
                // Max emoji picker height: card chrome (58) + search row
                // (42) + category tab row (40) + max scroll region (340).
                const EMOJI_PICKER_HEIGHT: f32 = 58.0 + 42.0 + 40.0 + 340.0;
                // Max GIF picker height: header (~24) + search row (~28) +
                // results scroll 300 + spacing 12 + padding 16 ≈ 380px.
                const GIF_PICKER_HEIGHT: f32 = 380.0;

                let picker_backdrop = |close: AppMessage| {
                    widget::button(widget::Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .on_press(close)
                        .style(|_t, _s| iced::widget::button::Style {
                            background: None,
                            ..Default::default()
                        })
                };

                if self.show_emoji_picker {
                    Stack::new()
                        .push(inner)
                        .push(picker_backdrop(AppMessage::ToggleEmojiPicker))
                        .push(widget::responsive(move |size: iced::Size| {
                            let picker = self.view_emoji_picker();
                            let bottom = (COMPOSER_OFFSET + PICKER_GAP).min(
                                (size.height - EMOJI_PICKER_HEIGHT - PICKER_GAP).max(PICKER_GAP),
                            );
                            widget::container(picker)
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .padding(iced::Padding {
                                    top: 0.0,
                                    right: SPACE_16,
                                    bottom,
                                    left: 0.0,
                                })
                                .align_x(iced::alignment::Horizontal::Right)
                                .align_y(iced::alignment::Vertical::Bottom)
                                .into()
                        }))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                } else if self.show_gif_picker {
                    // ── GIF picker overlay ──────────────────────────
                    Stack::new()
                        .push(inner)
                        .push(picker_backdrop(AppMessage::ToggleGifPicker))
                        .push(widget::responsive(move |size: iced::Size| {
                            let picker = self.view_gif_picker();
                            let bottom = (COMPOSER_OFFSET + PICKER_GAP).min(
                                (size.height - GIF_PICKER_HEIGHT - PICKER_GAP).max(PICKER_GAP),
                            );
                            widget::container(picker)
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .padding(iced::Padding {
                                    top: 0.0,
                                    right: SPACE_16,
                                    bottom,
                                    left: 0.0,
                                })
                                .align_x(iced::alignment::Horizontal::Right)
                                .align_y(iced::alignment::Vertical::Bottom)
                                .into()
                        }))
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
    ///
    /// BORU-SSUI (sender screen-share UI redesign): the sharer controls live
    /// inside ONE card shell below the conversation header. This is a
    /// presentation/interaction redesign — capture, session, network and
    /// permission behavior must not change here (see docs/screen-share-ui/
    /// sender-audit.md). Style values reuse the shared design tokens; the
    /// BORU-SSUI-08 token work migrates the card values into
    /// `screen_share.card.*` TOML tokens afterwards.
    pub(crate) fn view_screen_share_panel(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, responsive, row, text};
        use iced::Length;
        let media_presence = self.calls_state.realtime_media.presence();
        let track_label = |state: boru_core::call::session::TrackState| match state {
            boru_core::call::session::TrackState::Stopped => crate::i18n::t("screenshare.status_off"),
            boru_core::call::session::TrackState::Starting => crate::i18n::t("screenshare.status_starting"),
            boru_core::call::session::TrackState::Active => crate::i18n::t("screenshare.status_active"),
            boru_core::call::session::TrackState::Reconnecting => crate::i18n::t("screenshare.status_reconnecting"),
        };
        let presence_card = container(
            row![
                text(if media_presence.session_active {
                    crate::i18n::t("screenshare.media_session")
                } else {
                    crate::i18n::t("screenshare.media_ready")
                }),
                text(crate::i18n::t_args(
                    "screenshare.voice_status",
                    &[("state", &track_label(media_presence.voice))],
                )),
                text(crate::i18n::t_args(
                    "screenshare.screen_status",
                    &[("state", &track_label(media_presence.screen))],
                )),
            ]
            .spacing(SPACE_12)
            .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .padding([SPACE_6, SPACE_8]);
        // BORU-UI-03: viewer box geometry comes from `ChatTheme::screen_share_*`
        // (640x360 capture aspect; the mouse-area Point maps 1:1 to normalized
        // coordinates only while the box matches the capture aspect).
        let body = if let Some((inviter, _)) = &self.calls_state.screen_share_invite {
            column![
                text(format!("{inviter} wants to share their screen")),
                row![
                    button(text(crate::i18n::t("common.accept"))).on_press(AppMessage::AcceptScreenShare),
                    button(text(crate::i18n::t("common.decline"))).on_press(AppMessage::DeclineScreenShare),
                ].spacing(SPACE_8),
            ].spacing(SPACE_6)
        } else if self.calls_state.screen_share_host_state != ScreenShareHostState::Idle {
            // ── Sharer panel (PDF Phase 13) ────────────────────────────
            // All seven states are displayed: requesting, awaiting
            // acceptance, sharing, paused, reconnecting, stopped, error.
            // Stop Sharing stays reachable in every active state, and the
            // monitor picker lets the sharer choose/switch the source.
            let conversation = self
                .conversation_store
                .active_iter()
                .into_iter()
                .find(|entry| entry.topic == self.topic);
            let peer_name = conversation
                .map(|entry| entry.display_name())
                .unwrap_or_default();

            let state_text = match &self.calls_state.screen_share_host_state {
                ScreenShareHostState::Requesting => crate::i18n::t("screenshare.requesting"),
                ScreenShareHostState::Inviting => crate::i18n::t("screenshare.awaiting_acceptance"),
                ScreenShareHostState::Streaming => {
                    if peer_name.is_empty() {
                        crate::i18n::t("screenshare.sharing")
                    } else {
                        // BORU-SSUI-09: long peer names ellipsize in the
                        // card title. The budget comes from
                        // `screen_share.card.title_max_chars` so TOML can
                        // tune it; the clipped no-wrap title below is the
                        // backstop that prevents any spill at narrow widths.
                        let budget = self.boru_theme().screen_share.card.title_max_chars as usize;
                        let name = crate::presentation::truncate_with_ellipsis(&peer_name, budget);
                        crate::i18n::t_args("screenshare.sharing_with", &[("name", &name)])
                    }
                }
                ScreenShareHostState::Paused => crate::i18n::t("screenshare.paused"),
                ScreenShareHostState::Reconnecting => crate::i18n::t("screenshare.reconnecting"),
                ScreenShareHostState::Stopped => crate::i18n::t("screenshare.stopped"),
                ScreenShareHostState::Error(_) => crate::i18n::t("screenshare.error"),
                ScreenShareHostState::Idle => unreachable!(),
            };
            // BORU-SSUI-02: the card title sits at the top-left of the card
            // shell. `state_text` is the runtime status line — for the active
            // streaming state it resolves to `screenshare.sharing_with`
            // ("Sharing your screen with {name}"), so the peer name is the
            // real conversation display name, never mockup text. Muted
            // supporting-text size matches the approved mockup hierarchy.
            let mut items: Vec<iced::Element<'_, AppMessage>> = vec![container(
                text(state_text)
                    .size(crate::fonts::TypeRole::SupportingText.size_px())
                    .font(crate::fonts::TypeRole::SupportingText.font())
                    .color(Self::muted_color(self.dark_mode))
                    .wrapping(iced::widget::text::Wrapping::None)
                    .width(Length::Fill),
            )
            .width(Length::Fill)
            .clip(true)
            .into()];

            // Error reason (user-safe; never logs media data).
            if let ScreenShareHostState::Error(reason) = &self.calls_state.screen_share_host_state {
                items.push(
                    text(reason)
                        .size(crate::fonts::TypeRole::SupportingText.size_px())
                        .color(Self::muted_color(self.dark_mode))
                        .into(),
                );
            }

            // ── Monitor/source selection (PDF Phase 13) ───────────────
            // The enumerated source list is shown in every active state so
            // the sharer can pick the initial source before the viewer
            // accepts and switch it any time afterwards. The chosen entry
            // is highlighted; picking one sends HostCommand::SwitchSource.
            // BORU-SSUI-03: sources render as selectable CARDS (kind icon +
            // ellipsized title + dimensions + selected state) instead of
            // the old blue text buttons, in a horizontally scrollable row
            // so more sources than fit never wrap into a wall of buttons.
            // The message dispatched is unchanged — `ScreenShareSelectSource`
            // — so capture switching behaviour is preserved exactly.
            if let Some(sources) = &self.calls_state.screen_share_sources {
                if !sources.is_empty() {
                    let selected = self.calls_state.screen_share_selected_source;
                    // BORU-SSUI-10: source cards become inert in the
                    // terminal states (Stopped/Error) — picking a source
                    // on a dead session is impossible. Same gate as the
                    // quality segments and the Stop Sharing action row.
                    let controls_enabled =
                        Self::stop_action_visible(&self.calls_state.screen_share_host_state);
                    let cards: Vec<iced::Element<'_, AppMessage>> = sources
                        .iter()
                        .map(|source| {
                            let is_selected = Self::source_card_is_selected(selected, source.id);
                            self.view_source_card(source, is_selected, controls_enabled)
                        })
                        .collect();
                    // BORU-SSUI-08: the horizontal gap between source cards
                    // comes from `screen_share.source_card.row_spacing`.
                    let source_row_spacing = self.boru_theme().screen_share.source_card.row_spacing;
                    items.push(
                        column![
                            text(crate::i18n::t("screenshare.source"))
                                .size(crate::fonts::TypeRole::SupportingText.size_px())
                                .color(Self::muted_color(self.dark_mode)),
                            iced::widget::scrollable(row(cards).spacing(source_row_spacing))
                                .direction(iced::widget::scrollable::Direction::Horizontal(
                                    iced::widget::scrollable::Scrollbar::default().spacing(SPACE_4),
                                ))
                                .style(crate::ui_components::neutral_scrollbar_style)
                                .width(Length::Fill),
                        ]
                        .spacing(SPACE_6)
                        .into(),
                    );
                }
            }

            // BORU-SSUI-05 (PDF Task 5): dedicated compact status area
            // for remote control — labeled "Remote control: OFF/ON" with
            // an input/control icon (mouse pointer). The current
            // permission model is consent-gated: the sender cannot
            // toggle remote control ON directly, so this is a STATE-ONLY
            // display with no invented toggle. The explicit enable /
            // disable actions (grant/deny consent prompt, revoke) are
            // preserved below with the same compact control language as
            // the rest of the panel. The label + dot bind to
            // `screen_share_control_active`, which is kept live by
            // `SessionEvent::ControlChanged` → `apply_screen_share_event`,
            // so the status updates without reopening the view.
            // BORU-SSUI-09: rendered inside the responsive control row
            // (see `view_screen_share_remote_status_group`).

            // BORU-SS-39: active quality preset + connection path +
            // adaptation state, published ~1 Hz by the host streaming loop.
            // Always visible while streaming (not gated by the dev overlay).
            if let Some(metrics) = &self.calls_state.screen_share_host_metrics {
                let preset_label = match metrics.preset {
                    QualityPreset::LanHigh => crate::i18n::t("screenshare.preset_lan_high"),
                    QualityPreset::Balanced => crate::i18n::t("screenshare.preset_balanced"),
                    QualityPreset::RelayConservative => crate::i18n::t("screenshare.preset_relay"),
                };
                let path_label = match metrics.path_kind {
                    boru_core::screen_share::PathKind::Direct => {
                        crate::i18n::t("screenshare.path_direct")
                    }
                    boru_core::screen_share::PathKind::Relay => {
                        crate::i18n::t("screenshare.path_relay")
                    }
                    boru_core::screen_share::PathKind::Unknown => {
                        crate::i18n::t("screenshare.path_unknown")
                    }
                };
                items.push(
                    text(crate::i18n::t_args(
                        "screenshare.quality_line",
                        &[
                            ("preset", &preset_label),
                            ("path", &path_label),
                            ("level", &metrics.adaptive_level.to_string()),
                        ],
                    ))
                    .size(crate::fonts::TypeRole::SupportingText.size_px())
                    .color(Self::muted_color(self.dark_mode))
                    .into(),
                );
            }
            // BORU-SSUI-04 (PDF Task 4): quality presets as ONE segmented
            // control under a small "Quality" label. The four segments map
            // to the exact same messages the old text buttons dispatched:
            // LAN High → LanHigh, Balanced → Balanced, Relay →
            // RelayConservative, Auto → None (path-derived auto preset).
            // `screen_share_selected_preset` mirrors the user's last choice
            // so exactly one segment shows the accent fill at a time; the
            // host's effective preset remains authoritative (metrics above).
            // No availability signal exists today, so every segment is
            // enabled; the primitive renders disabled segments if a future
            // signal appears (never hidden).
            // BORU-SSUI-09 (PDF Task 9): the quality segmented control,
            // remote-control status and audio toggle are rendered as ONE
            // responsive control row — all three share a single row at wide
            // widths, the row may wrap into two logical groups at medium
            // widths, and the groups stack vertically at narrow widths.
            items.push(self.view_screen_share_control_row());

            // Explicit consent prompt: the host picks the granted
            // capabilities. BORU-SSUI-05: these are the sender's explicit
            // ENABLE action — preserved but rendered with the same compact
            // control language as the rest of the panel (padding([2, 6]),
            // matching the viewer toolbar buttons).
            if let Some((_, viewer, capabilities)) = &self.calls_state.screen_share_control_request
            {
                let caps = capabilities
                    .iter()
                    .map(Self::capability_label)
                    .collect::<Vec<_>>()
                    .join(", ");
                items.push(
                    text(format!("{viewer} requests: {caps}"))
                        .size(crate::fonts::TypeRole::SupportingText.size_px())
                        .color(Self::muted_color(self.dark_mode))
                        .into(),
                );
                let wants_pointer = capabilities
                    .iter()
                    .any(|c| matches!(c, Capability::ControlPointer | Capability::ControlKeyboard));
                let wants_clipboard = capabilities.contains(&Capability::Clipboard);
                let mut grant_buttons: Vec<iced::Element<'_, AppMessage>> = Vec::new();
                // BORU-SSUI-10: consent-prompt actions are keyboard
                // reachable too (Tab + Enter/Space).
                // BORU-SSUI-12: built from the shared `compact_action_button`
                // primitive (same compact language as the viewer toolbar).
                if wants_pointer {
                    grant_buttons.push(compact_action_button(
                        crate::i18n::t("screenshare.grant_pointer"),
                        None,
                        Some(AppMessage::ScreenShareGrantControl(vec![
                            Capability::ControlPointer,
                        ])),
                        Some(crate::design_tokens::RADIUS_SM),
                    ));
                    grant_buttons.push(compact_action_button(
                        crate::i18n::t("screenshare.grant_pointer_keyboard"),
                        None,
                        Some(AppMessage::ScreenShareGrantControl(vec![
                            Capability::ControlPointer,
                            Capability::ControlKeyboard,
                        ])),
                        Some(crate::design_tokens::RADIUS_SM),
                    ));
                }
                // Clipboard is a SEPARATE optional capability (PDF Task 9.3 /
                // BORU-SS-25): the host may grant it on its own, without
                // granting pointer/keyboard control.
                if wants_clipboard {
                    grant_buttons.push(compact_action_button(
                        crate::i18n::t("screenshare.grant_clipboard"),
                        None,
                        Some(AppMessage::ScreenShareGrantControl(vec![
                            Capability::Clipboard,
                        ])),
                        Some(crate::design_tokens::RADIUS_SM),
                    ));
                }
                grant_buttons.push(compact_action_button(
                    crate::i18n::t("common.deny"),
                    None,
                    Some(AppMessage::ScreenShareDenyControl),
                    Some(crate::design_tokens::RADIUS_SM),
                ));
                items.push(row(grant_buttons).spacing(SPACE_8).into());
            }
            // BORU-SSUI-05: explicit DISABLE action (revoke) + the separate
            // clipboard capability — preserved, compact. The verbose
            // "Remote control active" line is superseded by the status row
            // above (icon + ON label + dot).
            if self.calls_state.screen_share_control_active {
                items.push(compact_action_button(
                    crate::i18n::t("screenshare.revoke_control"),
                    None,
                    Some(AppMessage::ScreenShareRevokeControl),
                    Some(crate::design_tokens::RADIUS_SM),
                ));
            }
            if self.calls_state.screen_share_clipboard_active {
                items.push(compact_action_button(
                    crate::i18n::t("screenshare.send_clipboard"),
                    None,
                    Some(AppMessage::ScreenShareHostSendClipboard),
                    Some(crate::design_tokens::RADIUS_SM),
                ));
            }
            // System-audio sharing (BORU-SS-37): a SEPARATE optional
            // capability — the sharer toggles it explicitly (mirroring
            // clipboard, PDF Task 9.3). Enabling grants Capability::Audio
            // and starts capture, disabling stops it.
            // BORU-SSUI-06 (PDF Task 6): rendered as a real toggle row —
            // speaker icon + "Audio" label + switch — instead of the old
            // Audio On/Off label button. Switch OFF = no-audio state,
            // switch ON = the current audio-sharing path; flipping it
            // dispatches the SAME ScreenShareToggleAudio message the old
            // button used, so capture/session behaviour is unchanged. The
            // switch value binds to `screen_share_audio_active` (the
            // authoritative mirror set by SessionEvent::AudioState). When
            // the host reported a typed unavailable error (e.g. no
            // PipeWire runtime), the switch is disabled and the reason
            // shows as a short tooltip + status line — the existing audio
            // capability detection (src/screen_share/audio.rs + the
            // AudioState error) is reused, not reimplemented.
            // BORU-SSUI-09: rendered inside the responsive control row
            // (see `view_screen_share_audio_group`).
            // PDF Phase 12: developer diagnostics overlay — only when the
            // dev-ui gate is on (`--dev-ui` / `BORU_DEV_UI=1` / dev-ui feature).
            if self.calls_state.screen_share_dev_overlay {
                if let Some(metrics) = &self.calls_state.screen_share_host_metrics {
                    for line in screen_share_metrics_lines(metrics) {
                        items.push(
                            text(line)
                                .size(10)
                                .color(Self::muted_color(self.dark_mode))
                                .into(),
                        );
                    }
                }
            }
            // Stop Sharing is permanently accessible while a session is
            // active (requesting → error). Terminal notices offer retry and
            // dismissal instead.
            match &self.calls_state.screen_share_host_state {
                s if !Self::stop_action_visible(s) => {
                    let peer_key =
                        conversation.and_then(|entry| PublicKey::from_str(&entry.peer_id).ok());
                    let mut actions: Vec<iced::Element<'_, AppMessage>> = Vec::new();
                    if let Some(key) = peer_key {
                        // BORU-SSUI-10: terminal action buttons are
                        // keyboard-reachable too (Tab + Enter/Space).
                        actions.push(
                            crate::focusable_button::focusable_button(
                                button(text(crate::i18n::t("screenshare.share_again")))
                                    .on_press(AppMessage::StartScreenShare(key)),
                                Some(AppMessage::StartScreenShare(key)),
                            )
                            .ring_radius(crate::design_tokens::RADIUS_MD)
                            .into(),
                        );
                    }
                    actions.push(
                        crate::focusable_button::focusable_button(
                            button(text(crate::i18n::t("screenshare.dismiss")))
                                .on_press(AppMessage::ScreenShareDismissNotice),
                            Some(AppMessage::ScreenShareDismissNotice),
                        )
                        .ring_radius(crate::design_tokens::RADIUS_MD)
                        .into(),
                    );
                    items.push(row(actions).spacing(SPACE_8).into());
                }
                _ => {
                    // BORU-SSUI-07 (PDF Task 7): bottom action row.
                    //
                    // "Pause Preview" is intentionally OMITTED (the PDF
                    // explicitly allows omitting it "until that behavior is
                    // safely available"): the sender has NO local preview
                    // surface — `screen_share_frame_handle` is populated only
                    // by the VIEWER decode worker (`ScreenShareFrameReceived`),
                    // never by the host capture path — and
                    // `ScreenShareHostState::Paused` is an automatic SESSION
                    // pause entered when the capture source disappears
                    // (monitor unplug), which pauses the remote stream by
                    // design. Binding a button to that state would pause the
                    // remote stream by accident, which the PDF forbids. A
                    // local-only preview pause would need a host-side preview
                    // surface + a capture-side freeze that does not touch the
                    // encode/transport path; there is no such mechanism today,
                    // so the button is omitted rather than inventing one.
                    // BORU-SSUI-08 token work may revisit when a local preview
                    // exists.
                    //
                    // "Stop Sharing" is the ONLY destructive-looking action in
                    // the panel and sits on the far right of the action row
                    // (fill spacer keeps it right-aligned). It dispatches the
                    // SAME `AppMessage::StopScreenShare` the old text button
                    // used — the cleanup path (stop capture, release
                    // resources, EndSession, reset UI, Stopped state) is
                    // unchanged. The red/destructive treatment (solid danger
                    // fill + white stop icon + white label) matches both the
                    // approved mockup and Boru's existing destructive-action
                    // convention (`form_components::destructive_button`), so
                    // the PDF's "reserve solid alarming red fill for
                    // hover/pressed or if that matches Boru destructive-action
                    // conventions" clause is satisfied.
                    // BORU-SSUI-08: the destructive button geometry and the
                    // action-row gap come from `screen_share.destructive.*`
                    // / `screen_share.action.*` TOML tokens (hot-reloadable).
                    let action_theme = self.boru_theme().screen_share.action;
                    let destructive_theme = self.boru_theme().screen_share.destructive;
                    // BORU-SSUI-10: the destructive Stop Sharing button is
                    // keyboard-reachable (Tab + Enter/Space) with a visible
                    // focus ring matching the button radius — the same
                    // FocusableButton wrapper every other Boru action uses.
                    let stop_btn = crate::form_components::destructive_button_icon(
                        Icon::Stop,
                        crate::i18n::t("screenshare.stop_sharing"),
                        Some(AppMessage::StopScreenShare),
                        false,
                        crate::form_components::DestructiveButtonStyle {
                            padding_x: destructive_theme.padding_x,
                            padding_y: destructive_theme.padding_y,
                            radius: destructive_theme.radius,
                            icon_gap: destructive_theme.icon_gap,
                        },
                    );
                    let stop_btn: iced::Element<'_, AppMessage> =
                        crate::focusable_button::focusable_button(
                            stop_btn,
                            Some(AppMessage::StopScreenShare),
                        )
                        .ring_radius(destructive_theme.radius)
                        .into();
                    items.push(
                        row![iced::widget::Space::new().width(Length::Fill), stop_btn]
                            .spacing(action_theme.row_spacing)
                            .align_y(iced::Alignment::Center)
                            .into(),
                    );
                }
            }
            // BORU-SSUI-02: consistent vertical rhythm inside the card shell.
            // BORU-SSUI-08: the rhythm comes from `screen_share.card.spacing`.
            let card_spacing = self.boru_theme().screen_share.card.spacing;
            column(items).spacing(card_spacing)
        } else if self.calls_state.screen_share_viewing {
            // Who is sharing (PDF Phase 13): the viewer always sees the
            // sharer's identity above the surface, plus whether remote
            // control is enabled.
            let mut viewer_lines: Vec<iced::Element<'_, AppMessage>> = Vec::new();
            if let Some(sharer) = &self.calls_state.screen_share_viewing_peer {
                viewer_lines.push(
                    text(crate::i18n::t_args(
                        "screenshare.viewing_peer",
                        &[("name", sharer)],
                    ))
                    .into(),
                );
            }
            let viewer_count = self.calls_state.screen_share_viewers.active_count().max(1);
            let chrome = self.calls_state.screen_share_viewers.viewers().first();
            viewer_lines.push(text(format!(
                "Viewers: {viewer_count}  •  Path: {}  •  Quality: {}",
                chrome.map(|v| format!("{:?}", v.path)).unwrap_or_else(|| "unknown".to_string()),
                chrome.map(|v| v.preset.name()).unwrap_or("auto"),
            )).into());
            viewer_lines.push(status_row(
                None,
                if self.calls_state.screen_share_control_active {
                    crate::i18n::t("screenshare.remote_control_on")
                } else {
                    crate::i18n::t("screenshare.remote_control_off")
                },
                Self::muted_color(self.dark_mode),
                None,
                None,
            ));
            // Dedicated scalable surface (PDF Task 8.2). The surface fills
            // the panel width; its height follows the structural layout
            // model so the chat log remains usable. Fit/100%/zoom/pan are
            // handled by the surface geometry; remote-control input maps
            // through the same geometry so it stays correct under zoom.
            // The layout model owns the reference media size.  Keep the
            // inline surface at that aspect-ratio-friendly size and let the
            // fullscreen overlay use its available height; this avoids a
            // small fixed cap leaving dead space on maximized windows.
            let cap = self.boru_layout().chat.screen_share.height;
            let video: iced::Element<'_, AppMessage> = if let (Some(handle), Some((w, h))) = (
                &self.calls_state.screen_share_frame_handle,
                self.calls_state.screen_share_src_size,
            ) {
                let src_size = iced::Size::new(w as f32, h as f32);
                let mode = self.calls_state.screen_share_view_mode;
                let pan = self.calls_state.screen_share_pan;
                let control_active = self.calls_state.screen_share_control_active;
                let hover = self.calls_state.screen_share_hover;
                let last_pointer_norm = self.calls_state.screen_share_last_pointer_pos;
                let surface = responsive(move |size: iced::Size| {
                    let viewport = iced::Size::new(size.width, cap);
                    view_screen_share_surface(
                        handle,
                        src_size,
                        viewport,
                        mode,
                        pan,
                        control_active,
                        hover,
                        last_pointer_norm,
                    )
                });
                container(surface)
                    .width(Length::Fill)
                    .height(Length::Fixed(cap))
                    .into()
            } else {
                text(crate::i18n::t("screenshare.waiting_frame")).into()
            };
            // PDF Phase 12: developer diagnostics overlay — only when the
            // dev-ui gate is on. The viewer side shows its own measured
            // pipeline stats (decode FPS, dropped frames, queue depth,
            // end-to-end latency) over the surface.
            let video = if self.calls_state.screen_share_dev_overlay {
                if let Some(stats) = self.calls_state.screen_share_viewer_stats {
                    let (w, h) = self.calls_state.screen_share_src_size.unwrap_or((0, 0));
                    let metrics = ScreenShareSessionMetrics {
                        codec: "h264".to_string(),
                        width: w,
                        height: h,
                        fps: 0,
                        bitrate_bps: 0,
                        backend: "viewer".to_string(),
                        // The viewer pipeline has no host-side preset/path
                        // state; the overlay shows the negotiated host values
                        // when the host publishes them.
                        path_kind: boru_core::screen_share::PathKind::Unknown,
                        preset: QualityPreset::Balanced,
                        adaptive_level: 0,
                        snapshot: stats,
                    };
                    iced::widget::stack![video, view_screen_share_metrics_overlay(metrics)].into()
                } else {
                    video
                }
            } else {
                video
            };
            // Compact control row: view mode (fit/100%/−/+/reset) then the
            // existing quality / remote-control / stop actions.
            let scale = self
                .calls_state
                .screen_share_src_size
                .map(|(w, h)| {
                    SurfaceGeometry::new(
                        iced::Size::new(self.window_width, cap),
                        iced::Size::new(w as f32, h as f32),
                        self.calls_state.screen_share_view_mode,
                        self.calls_state.screen_share_pan,
                    )
                    .scale()
                })
                .unwrap_or(1.0);
            let mut actions: Vec<iced::Element<'_, AppMessage>> = vec![
                compact_action_button(
                    crate::i18n::t("screenshare.lower_quality"),
                    None,
                    Some(AppMessage::ScreenShareLowerQuality),
                    None,
                ),
                compact_action_button(
                    crate::i18n::t("screenshare.full_quality"),
                    None,
                    Some(AppMessage::ScreenShareFullQuality),
                    None,
                ),
            ];
            if self.calls_state.screen_share_control_active {
                actions.push(text(crate::i18n::t("screenshare.control_granted")).into());
            } else {
                actions.push(compact_action_button(
                    crate::i18n::t("screenshare.request_control"),
                    None,
                    Some(AppMessage::ScreenShareRequestControl),
                    None,
                ));
            }
            // Clipboard is a SEPARATE optional capability (PDF Task 9.3 /
            // BORU-SS-25): the viewer requests it explicitly, and it is never
            // enabled by granting or requesting remote control.
            if self.calls_state.screen_share_clipboard_active {
                actions.push(compact_action_button(
                    crate::i18n::t("screenshare.send_clipboard"),
                    None,
                    Some(AppMessage::ScreenShareSendClipboard),
                    None,
                ));
            } else {
                actions.push(compact_action_button(
                    crate::i18n::t("screenshare.request_clipboard"),
                    None,
                    Some(AppMessage::ScreenShareRequestClipboard),
                    None,
                ));
            }
            actions.push(compact_action_button(
                crate::i18n::t("screenshare.stop_viewing"),
                None,
                Some(AppMessage::StopScreenShare),
                None,
            ));
            let mut viewer_column: Vec<iced::Element<'_, AppMessage>> = viewer_lines;
            viewer_column.push(video.into());
            viewer_column.push(
                row![
                    view_screen_share_view_controls(
                        scale,
                        self.calls_state.screen_share_fullscreen,
                        self.calls_state.screen_share_cursor_enabled,
                    ),
                    row(actions).spacing(SPACE_6),
                ]
                .spacing(SPACE_8)
                .align_y(iced::Alignment::Center)
                .wrap()
                .into(),
            );
            column(viewer_column).spacing(SPACE_6)
        } else {
            return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
        };
        // BORU-SSUI-02: ONE card shell below the conversation header for all
        // sender sharing controls. Subtle surface distinct from the chat
        // canvas, thin neutral border, medium-large radius and a restrained
        // shadow — the shared Boru card language (design_tokens).
        // BORU-SSUI-08: geometry (padding / radius / border width) comes
        // from `screen_share.card.*` TOML tokens; colours stay mode-aware
        // via `design_tokens` (surface_secondary / border_muted / shadow_card)
        // so light/dark never bake in fixed values.
        // BORU-SSUI-12: the shell is the shared `screen_share_card`
        // primitive (same rounded toolbar/card used by the viewer side).
        let card_theme = self.boru_theme().screen_share.card;
        screen_share_card(
            column![presence_card, body].spacing(SPACE_8).into(),
            card_theme,
        )
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-09 (PDF Task 9): ONE responsive control row combining the
    /// quality segmented control, remote-control status and the audio toggle.
    ///
    /// The row resolves the panel's actual measured width through the shared
    /// responsive tier machinery (`LayoutConfig::responsive::tier_for_width`,
    /// TOML-tunable via `[responsive]` in boru-layout.toml), so:
    /// - **UltraWide** (maximized windows): all three groups share one row;
    /// - **Desktop** (reference 1280x800 and medium split-windows): the same
    ///   row may wrap into two logical groups without clipping (`.wrap()`);
    /// - **Narrow** (very narrow split-windows): the groups stack vertically,
    ///   every control fully visible, no label overlap or spill.
    ///
    /// Groups that only exist while streaming (remote-control status, audio)
    /// return `None` in other states, so the row degrades to quality-only in
    /// requesting/paused states. The quality segmented control is always the
    /// first group, matching the mockup hierarchy (source cards → Quality /
    /// Remote control / Audio row → action row).
    fn view_screen_share_control_row(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{column, responsive, row};
        // BORU-SSUI-09: `Responsive` defaults to `height: Length::Fill`, which
        // inside a Shrink-height flex column makes iced allocate it the
        // REMAINING height and squash the groups (remote/audio collapsed to
        // 0 px, the segmented control to ~10 px in the 640 dump). Forcing
        // Shrink lets the row size to its content's natural height at every
        // tier; only the measured width is used for tier resolution.
        responsive(move |size: iced::Size| {
            let tier = self.boru_layout().responsive.tier_for_width(size.width);
            let mut groups: Vec<iced::Element<'_, AppMessage>> = Vec::new();
            groups.push(self.view_screen_share_quality_group());
            if let Some(group) = self.view_screen_share_remote_status_group() {
                groups.push(group);
            }
            if let Some(group) = self.view_screen_share_audio_group() {
                groups.push(group);
            }
            let row_gap = self.boru_theme().screen_share.card.spacing;
            match SenderControlRowLayout::for_tier(tier) {
                SenderControlRowLayout::Stack => column(groups).spacing(row_gap).into(),
                SenderControlRowLayout::Wrap => row(groups).spacing(row_gap).wrap().into(),
                SenderControlRowLayout::Row => row(groups).spacing(row_gap).into(),
            }
        })
        .height(iced::Length::Shrink)
        .into()
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-09 (PDF Task 9): the quality control group — the small
    /// "Quality" label above the ONE segmented control (BORU-SSUI-04). The
    /// four segments map to the exact same messages the old text buttons
    /// dispatched; `screen_share_selected_preset` mirrors the user's last
    /// choice so exactly one segment shows the accent fill at a time.
    /// Extracted so the responsive control row can place it beside
    /// remote-control status and the audio toggle.
    fn view_screen_share_quality_group(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{column, text};
        let selected_preset = self.calls_state.screen_share_selected_preset;
        // BORU-SSUI-10: controls become inert (disabled + tooltip) in the
        // terminal states (Stopped / Error) so changing quality on a dead
        // session is impossible — the same gate that hides Stop Sharing.
        let enabled = Self::stop_action_visible(&self.calls_state.screen_share_host_state);
        let disabled_tooltip = (!enabled).then(|| crate::i18n::t("screenshare.session_ended"));
        let segments: Vec<crate::ui_components::SegmentedOption<AppMessage>> =
            Self::quality_segment_specs(selected_preset)
                .into_iter()
                .map(|spec| crate::ui_components::SegmentedOption {
                    label: crate::i18n::t(spec.label_key),
                    selected: spec.selected,
                    enabled,
                    on_press: if enabled {
                        Some(AppMessage::ScreenShareSetPreset(spec.preset))
                    } else {
                        None
                    },
                    tooltip: disabled_tooltip.clone(),
                })
                .collect();
        // BORU-SSUI-08: the segmented-control geometry comes from the
        // `screen_share.segmented.*` TOML tokens (hot-reloadable).
        let segmented_theme = self.boru_theme().screen_share.segmented;
        column![
            text(crate::i18n::t("screenshare.preset"))
                .size(crate::fonts::TypeRole::SupportingText.size_px())
                .color(Self::muted_color(self.dark_mode)),
            crate::ui_components::segmented_control(
                segments,
                crate::ui_components::SegmentedControlStyle {
                    radius: segmented_theme.radius,
                    spacing: segmented_theme.spacing,
                    padding_x: segmented_theme.padding_x,
                    padding_y: segmented_theme.padding_y,
                    check_icon_size: segmented_theme.check_icon_size,
                },
            ),
        ]
        .spacing(SPACE_6)
        .into()
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-09 (PDF Task 9): the remote-control status group — the
    /// input/control icon + "Remote control: ON/OFF" label + status dot
    /// (BORU-SSUI-05). State-only: the permission model is consent-gated,
    /// so the sender never gets an invented toggle here. `None` outside the
    /// Streaming state (the status only exists while a session is live).
    fn view_screen_share_remote_status_group(&self) -> Option<iced::Element<'_, AppMessage>> {
        if self.calls_state.screen_share_host_state != ScreenShareHostState::Streaming {
            return None;
        }
        let spec = Self::remote_control_status_spec(self.calls_state.screen_share_control_active);
        let theme = self.theme();
        let icon_color: fn(&iced::Theme) -> iced::Color = if spec.active {
            crate::design_tokens::primary
        } else {
            crate::design_tokens::text_secondary
        };
        let text_color = if spec.active {
            crate::design_tokens::text_primary(&theme)
        } else {
            Self::muted_color(self.dark_mode)
        };
        // BORU-SSUI-12: the status area is the shared `status_row`
        // primitive (icon + label + dot) — the viewer's remote-control
        // line renders through the same primitive.
        Some(status_row(
            Some((Icon::MousePointer, icon_color)),
            crate::i18n::t(spec.label_key),
            text_color,
            Some(if spec.active {
                crate::ui_components::StatusDotKind::Online
            } else {
                crate::ui_components::StatusDotKind::Offline
            }),
            Some(crate::i18n::t(spec.label_key)),
        ))
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-09 (PDF Task 9): the audio toggle group — speaker icon +
    /// "Audio" label + switch (BORU-SSUI-06). `None` outside the Streaming
    /// state. The switch binds to `screen_share_audio_active` (the
    /// authoritative mirror set by `SessionEvent::AudioState`) and dispatches
    /// the SAME `ScreenShareToggleAudio` message as before. When the host
    /// reported a typed unavailable error (e.g. no PipeWire runtime), the
    /// switch is disabled and the reason shows as a short tooltip + status
    /// line.
    fn view_screen_share_audio_group(&self) -> Option<iced::Element<'_, AppMessage>> {
        use iced::widget::{column, row, text, toggler, tooltip};
        if self.calls_state.screen_share_host_state != ScreenShareHostState::Streaming {
            return None;
        }
        let unavailable = self.calls_state.screen_share_audio_error.as_deref();
        let spec = Self::audio_toggle_spec(
            self.calls_state.screen_share_audio_active,
            unavailable.is_some(),
        );
        // BORU-SSUI-08: the audio toggle row geometry (icon size,
        // icon/label/switch gap) comes from `screen_share.toggle.*` TOML
        // tokens (hot-reloadable).
        let toggle_theme = self.boru_theme().screen_share.toggle;
        let icon_color: fn(&iced::Theme) -> iced::Color = if spec.active {
            crate::design_tokens::primary
        } else {
            crate::design_tokens::text_secondary
        };
        // BORU-SSUI-10: the speaker icon always carries a concise tooltip —
        // the volume glyph is the ambiguous part of the row. When audio
        // cannot be shared the tooltip shows the typed reason (existing
        // capability detection); otherwise it names the current state.
        let speaker_tooltip = if let Some(reason) = unavailable {
            crate::i18n::t_args("screenshare.audio_unavailable", &[("reason", reason)])
        } else if spec.active {
            crate::i18n::t("screenshare.audio_on")
        } else {
            crate::i18n::t("screenshare.audio_off")
        };
        let speaker = spec
            .icon
            .build()
            .size(IconSize::from_px(toggle_theme.icon_size))
            .color_fn(icon_color)
            .build();
        let speaker: iced::Element<'_, AppMessage> = tooltip::Tooltip::new(
            speaker,
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, speaker_tooltip),
            tooltip::Position::Bottom,
        )
        .gap(SPACE_2)
        .into();
        // Keep the row neutral; only the switch/icon carry the active state
        // (never blue-wash the whole control).
        let label = text(crate::i18n::t(spec.label_key))
            .size(crate::fonts::TypeRole::SupportingText.size_px())
            .font(crate::fonts::TypeRole::SupportingText.font())
            .color(Self::muted_color(self.dark_mode));
        // iced 0.14 `toggler`: omitting `.on_toggle` renders the switch
        // inert/disabled (Status::Disabled) — exactly what we want when
        // audio cannot be shared.
        let mut switch = toggler(self.calls_state.screen_share_audio_active)
            .style(crate::form_components::toggler_style);
        if spec.enabled {
            switch = switch.on_toggle(|_| AppMessage::ScreenShareToggleAudio);
        }
        // BORU-SSUI-10: iced's Toggler has no `operation::Focusable` impl,
        // so it is unreachable by keyboard on its own — wrap it in the
        // same FocusableButton every other Boru control uses. Enabled: Tab
        // reaches it, Space/Enter toggles, focus ring drawn. Disabled:
        // `None` keeps it out of the tab order entirely.
        let switch = crate::focusable_button::focusable_button(
            switch,
            if spec.enabled {
                Some(AppMessage::ScreenShareToggleAudio)
            } else {
                None
            },
        )
        .ring_radius(crate::design_tokens::RADIUS_MD)
        .build();
        let audio_row = if unavailable.is_some() {
            // Disabled capability: the switch already carries the typed
            // reason tooltip; a short muted status line keeps the state
            // obvious without a hover.
            row![speaker, label, switch]
                .spacing(toggle_theme.row_spacing)
                .align_y(iced::Alignment::Center)
                .into()
        } else {
            row![speaker, label, switch]
                .spacing(toggle_theme.row_spacing)
                .align_y(iced::Alignment::Center)
                .into()
        };
        Some(if let Some(reason) = unavailable {
            column![
                audio_row,
                text(crate::i18n::t_args(
                    "screenshare.audio_unavailable",
                    &[("reason", reason)],
                ))
                .size(crate::fonts::TypeRole::SupportingText.size_px())
                .font(crate::fonts::TypeRole::SupportingText.font())
                .color(Self::muted_color(self.dark_mode)),
            ]
            .spacing(SPACE_4)
            .into()
        } else {
            audio_row
        })
    }

    #[cfg(feature = "screen-sharing")]
    /// Human label for a control capability (consent prompt).
    fn capability_label(capability: &Capability) -> String {
        match capability {
            Capability::ControlPointer => "pointer".to_string(),
            Capability::ControlKeyboard => "keyboard".to_string(),
            Capability::Clipboard => "clipboard".to_string(),
            // BORU-SS-37: system audio is a separate optional capability.
            Capability::Audio => "audio".to_string(),
            Capability::ViewScreen => "view".to_string(),
        }
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-04: map the four quality modes to segmented-control specs.
    /// Exactly one spec is selected for any `selected` value (`None` =
    /// Auto). The dispatch targets mirror the old text buttons exactly:
    /// LAN High → LanHigh, Balanced → Balanced, Relay → RelayConservative,
    /// Auto → None.
    pub(crate) fn quality_segment_specs(
        selected: Option<QualityPreset>,
    ) -> [QualitySegmentSpec; 4] {
        [
            QualitySegmentSpec {
                label_key: "screenshare.preset_lan_high",
                preset: Some(QualityPreset::LanHigh),
                selected: selected == Some(QualityPreset::LanHigh),
            },
            QualitySegmentSpec {
                label_key: "screenshare.preset_balanced",
                preset: Some(QualityPreset::Balanced),
                selected: selected == Some(QualityPreset::Balanced),
            },
            QualitySegmentSpec {
                label_key: "screenshare.preset_relay",
                preset: Some(QualityPreset::RelayConservative),
                selected: selected == Some(QualityPreset::RelayConservative),
            },
            QualitySegmentSpec {
                label_key: "screenshare.preset_auto",
                preset: None,
                selected: selected.is_none(),
            },
        ]
    }

    #[cfg(feature = "screen-sharing")]
    /// Map the selected-source mirror to a source-card state without building
    /// an iced widget tree, keeping the sender selection contract testable.
    pub(crate) fn source_card_is_selected(
        selected: Option<CaptureSourceId>,
        source_id: CaptureSourceId,
    ) -> bool {
        selected == Some(source_id)
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-05: map the authoritative remote-control state to the
    /// status-area presentation. State-only by design — the current
    /// permission model has no direct sender-side toggle (control is
    /// granted via explicit consent and revoked explicitly), so this
    /// only supplies the runtime label ("Remote control: ON/OFF").
    pub(crate) fn remote_control_status_spec(active: bool) -> RemoteControlStatusSpec {
        if active {
            RemoteControlStatusSpec {
                label_key: "screenshare.remote_control_on",
                active: true,
            }
        } else {
            RemoteControlStatusSpec {
                label_key: "screenshare.remote_control_off",
                active: false,
            }
        }
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-06 (PDF Task 6): map the authoritative audio state to the
    /// sender's audio toggle row presentation. The switch value is
    /// `screen_share_audio_active` (mirror of `SessionEvent::AudioState`),
    /// so OFF = no-audio, ON = audio-sharing path — flipping it dispatches
    /// the SAME `ScreenShareToggleAudio` message the old label button used.
    /// When `unavailable` is set (typed unavailable error, e.g. no PipeWire
    /// runtime), the switch is disabled (`enabled = false`) and the reason
    /// is surfaced as tooltip/status text instead of silently failing.
    pub(crate) fn audio_toggle_spec(active: bool, unavailable: bool) -> AudioToggleSpec {
        AudioToggleSpec {
            icon: if active { Icon::Volume2 } else { Icon::VolumeX },
            label_key: "screenshare.audio",
            enabled: !unavailable,
            active,
        }
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-07 (PDF Task 7): whether the sender's destructive
    /// "Stop Sharing" action row is shown for a host state. It is shown
    /// for every active state (requesting → reconnecting); the terminal
    /// states (Stopped / Error) instead show Share Again + Dismiss.
    /// This keeps the action-row branching testable and the destructive
    /// action reachable in exactly the same states as before.
    pub(crate) fn stop_action_visible(state: &ScreenShareHostState) -> bool {
        !matches!(
            state,
            ScreenShareHostState::Stopped | ScreenShareHostState::Error(_)
        )
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-03: map a capture-source kind to a distinct source-picker
    /// icon. `CaptureSourceKind` today emits Monitor/Window/Desktop; there
    /// is no Panel/special-surface kind yet, so `Icon::Panel` stays
    /// reserved (documented gap — see icon_system.rs).
    fn source_kind_icon(kind: boru_core::screen_share::CaptureSourceKind) -> Icon {
        use boru_core::screen_share::CaptureSourceKind;
        match kind {
            CaptureSourceKind::Monitor => Icon::Monitor,
            CaptureSourceKind::Window => Icon::Window,
            CaptureSourceKind::Desktop => Icon::Desktop,
        }
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-03: one selectable source card for the screen-share
    /// source picker.
    ///
    /// Replaces the old blue text buttons. Each card carries a source-type
    /// icon, the ellipsized runtime source/window title, and the native
    /// dimensions on a second line. The selected card gets an accent
    /// border + soft accent background + a check glyph (never colour
    /// alone); unselected cards use a neutral surface with a subtle border
    /// and hover/pressed feedback. Clicking dispatches the SAME
    /// `ScreenShareSelectSource(source.id)` message the text buttons used,
    /// so capture switching behaviour is unchanged.
    /// BORU-SSUI-08: geometry (width / padding / radii / icon sizes /
    /// title budget / selected border) comes from `screen_share.source_card.*`
    /// TOML tokens (hot-reloadable); colours stay mode-aware via
    /// `design_tokens`.
    /// BORU-SSUI-10: the card is wrapped in the app's `FocusableButton`
    /// (Tab-reachable, Enter/Space activates, visible focus ring) and
    /// supports a disabled state — terminal sessions (Stopped/Error)
    /// render cards inert/dimmed instead of letting a click dispatch to a
    /// dead host. The source-kind icon carries a concise tooltip so
    /// monitor/window/desktop glyphs are never ambiguous.
    fn view_source_card(
        &self,
        source: &CaptureSource,
        selected: bool,
        enabled: bool,
    ) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text, tooltip, Space};
        use iced::Length;

        let source_card_theme = self.boru_theme().screen_share.source_card;

        let dark_mode = self.dark_mode;
        let theme = self.theme();

        let kind_icon = Self::source_kind_icon(source.kind);
        let icon_color: fn(&iced::Theme) -> iced::Color = if !enabled {
            crate::design_tokens::text_muted
        } else if selected {
            crate::design_tokens::primary
        } else {
            crate::design_tokens::text_secondary
        };
        // BORU-SSUI-10: concise tooltip on the source-kind icon — the
        // monitor/window/desktop glyph is the one ambiguous part of the
        // card (the title already names the source). Wrapped icon stays
        // inside the button, so clicks still reach the card.
        let kind_tooltip_key = match source.kind {
            boru_core::screen_share::CaptureSourceKind::Monitor => {
                "screenshare.source_kind_monitor"
            }
            boru_core::screen_share::CaptureSourceKind::Window => "screenshare.source_kind_window",
            boru_core::screen_share::CaptureSourceKind::Desktop => {
                "screenshare.source_kind_desktop"
            }
        };
        let icon = kind_icon
            .build()
            .size(IconSize::from_px(source_card_theme.icon_size))
            .color_fn(icon_color)
            .build();
        let icon: iced::Element<'_, AppMessage> = tooltip::Tooltip::new(
            icon,
            crate::fonts::type_role_text(
                crate::fonts::TypeRole::Metadata,
                crate::i18n::t(kind_tooltip_key),
            ),
            tooltip::Position::Bottom,
        )
        .gap(SPACE_2)
        .into();

        let title = crate::presentation::truncate_with_ellipsis(
            &source.title,
            source_card_theme.title_max_chars as usize,
        );
        let dims = format!("{} × {}", source.width, source.height);

        let title_color = if !enabled {
            crate::design_tokens::text_muted(&theme)
        } else if selected {
            crate::design_tokens::text_primary(&theme)
        } else {
            crate::design_tokens::text_secondary(&theme)
        };

        let mut card_row = row![
            icon,
            column![
                container(
                    text(title)
                        .size(crate::fonts::TypeRole::SupportingText.size_px())
                        .font(crate::fonts::TypeRole::SupportingText.font())
                        .color(title_color)
                        .wrapping(iced::widget::text::Wrapping::None)
                        .width(Length::Fill),
                )
                .width(Length::Fill)
                .clip(true),
                text(dims)
                    .size(crate::fonts::TypeRole::Metadata.size_px())
                    .font(crate::fonts::TypeRole::Metadata.font())
                    .color(Self::muted_color(dark_mode))
                    .wrapping(iced::widget::text::Wrapping::None)
                    .width(Length::Fill),
            ]
            .spacing(SPACE_2)
            .width(Length::Fill),
        ]
        .spacing(SPACE_8)
        .align_y(iced::Alignment::Center);

        // Clear selection indicator — a check glyph on the right edge. It is
        // NOT colour-alone: the accent border + soft background + check are
        // all present, so the state reads even for colour-blind users.
        if selected {
            let check = Icon::Check
                .build()
                .size(IconSize::from_px(source_card_theme.check_icon_size))
                .color_fn(if enabled {
                    crate::design_tokens::primary
                } else {
                    crate::design_tokens::text_muted
                })
                .build();
            card_row = card_row.push(check);
        } else {
            // Reserve the same right-edge slot so cards keep an even width
            // whether or not they are selected.
            card_row = card_row.push(
                Space::new()
                    .width(Length::Fixed(source_card_theme.check_icon_size))
                    .height(Length::Fixed(source_card_theme.check_icon_size)),
            );
        }

        let body = container(card_row)
            .padding([source_card_theme.padding_y, source_card_theme.padding_x])
            .width(Length::Fixed(source_card_theme.width));

        let msg = AppMessage::ScreenShareSelectSource(source.id);
        let inner = button(body).padding(0).style(move |t, status| {
            Self::source_card_button_style(t, status, selected, enabled, source_card_theme)
        });
        // BORU-SSUI-10: keyboard reachability — same FocusableButton
        // wrapper the rest of Boru uses. Enabled cards join the Tab order
        // (Enter/Space activates, focus ring drawn); disabled cards pass
        // `None` so Tab never stops on a dead control.
        let inner = if enabled {
            inner.on_press(msg.clone())
        } else {
            inner
        };
        crate::focusable_button::focusable_button(inner, if enabled { Some(msg) } else { None })
            .ring_radius(source_card_theme.radius)
            .into()
    }

    #[cfg(feature = "screen-sharing")]
    /// BORU-SSUI-03: button style for a source card.
    ///
    /// Selected: accent border + `primary_soft` background (plus the check
    /// glyph drawn in the content). Unselected: neutral `surface`
    /// background, subtle `border_muted` border, and hover/pressed
    /// feedback via `surface_hover` / `surface_pressed` with an accent
    /// border on hover — the same interaction language as the rest of Boru.
    /// BORU-SSUI-08: geometry (radius / selected border width) comes from
    /// `screen_share.source_card.*` TOML tokens; colours stay mode-aware.
    /// BORU-SSUI-10: a disabled card (terminal session) renders a muted
    /// surface + muted border with NO hover/pressed feedback and no shadow,
    /// so an inert card is visually unmistakable.
    fn source_card_button_style(
        theme: &iced::Theme,
        status: iced::widget::button::Status,
        selected: bool,
        enabled: bool,
        source_card_theme: crate::theme::ScreenShareSourceCardTheme,
    ) -> iced::widget::button::Style {
        if !enabled {
            return iced::widget::button::Style {
                background: Some(iced::Background::Color(crate::design_tokens::surface(
                    theme,
                ))),
                text_color: crate::design_tokens::text_muted(theme),
                border: iced::Border {
                    color: crate::design_tokens::border_muted(theme),
                    width: crate::design_tokens::BORDER_WIDTH,
                    radius: source_card_theme.radius.into(),
                },
                ..Default::default()
            };
        }
        let bg = if selected {
            crate::design_tokens::primary_soft(theme)
        } else {
            match status {
                iced::widget::button::Status::Hovered => crate::design_tokens::surface_hover(theme),
                iced::widget::button::Status::Pressed => {
                    crate::design_tokens::surface_pressed(theme)
                }
                _ => crate::design_tokens::surface(theme),
            }
        };
        let border_color = if selected {
            crate::design_tokens::primary(theme)
        } else {
            match status {
                iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed => {
                    crate::design_tokens::primary(theme)
                }
                _ => crate::design_tokens::border_muted(theme),
            }
        };
        iced::widget::button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: if selected {
                crate::design_tokens::primary(theme)
            } else {
                crate::design_tokens::text_primary(theme)
            },
            border: iced::Border {
                color: border_color,
                width: if selected {
                    source_card_theme.selected_border_width
                } else {
                    crate::design_tokens::BORDER_WIDTH
                },
                radius: source_card_theme.radius.into(),
            },
            shadow: match status {
                iced::widget::button::Status::Hovered => crate::design_tokens::shadow_card(theme),
                _ => iced::Shadow::default(),
            },
            ..Default::default()
        }
    }

    #[cfg(feature = "screen-sharing")]
    /// Full-window screen-share viewer overlay (PDF Task 8.2 fullscreen).
    ///
    /// Covers the whole app with the scalable surface; a compact control
    /// bar sits below the frame. Escape or the "Inline" button returns to
    /// the normal chat layout.
    pub(crate) fn view_screen_share_fullscreen<'a>(
        &'a self,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{column, container, responsive, row, stack, text};
        use iced::Length;

        let Some(handle) = &self.calls_state.screen_share_frame_handle else {
            return base.into();
        };
        let Some((w, h)) = self.calls_state.screen_share_src_size else {
            return base.into();
        };
        let src_size = iced::Size::new(w as f32, h as f32);
        let mode = self.calls_state.screen_share_view_mode;
        let pan = self.calls_state.screen_share_pan;
        let control_active = self.calls_state.screen_share_control_active;
        let hover = self.calls_state.screen_share_hover;
        let last_pointer_norm = self.calls_state.screen_share_last_pointer_pos;
        let scale = SurfaceGeometry::new(
            iced::Size::new(self.window_width, 600.0),
            src_size,
            mode,
            pan,
        )
        .scale();

        let surface = responsive(move |size: iced::Size| {
            view_screen_share_surface(
                handle,
                src_size,
                size,
                mode,
                pan,
                control_active,
                hover,
                last_pointer_norm,
            )
        });
        // PDF Phase 12: developer diagnostics overlay (dev-ui gate only).
        let surface: iced::Element<'_, AppMessage> = if self.calls_state.screen_share_dev_overlay {
            if let Some(stats) = self.calls_state.screen_share_viewer_stats {
                let (w, h) = self.calls_state.screen_share_src_size.unwrap_or((0, 0));
                let metrics = ScreenShareSessionMetrics {
                    codec: "h264".to_string(),
                    width: w,
                    height: h,
                    fps: 0,
                    bitrate_bps: 0,
                    backend: "viewer".to_string(),
                    path_kind: boru_core::screen_share::PathKind::Unknown,
                    preset: QualityPreset::Balanced,
                    adaptive_level: 0,
                    snapshot: stats,
                };
                iced::widget::stack![surface, view_screen_share_metrics_overlay(metrics)].into()
            } else {
                surface.into()
            }
        } else {
            surface.into()
        };

        let controls = row![
            view_screen_share_view_controls(
                scale,
                true,
                self.calls_state.screen_share_cursor_enabled
            ),
            compact_action_button(
                crate::i18n::t("screenshare.stop_viewing"),
                None,
                Some(AppMessage::StopScreenShare),
                None,
            ),
        ]
        .spacing(SPACE_8)
        .align_y(iced::Alignment::Center);

        let panel = container(
            column![
                row![
                    text(crate::i18n::t("screenshare.fullscreen_active"))
                        .size(crate::fonts::TypeRole::SupportingText.size_px())
                        .color(Self::muted_color(self.dark_mode)),
                    iced::widget::Space::new().width(Length::Fill),
                    compact_action_button(
                        crate::i18n::t("screenshare.inline"),
                        None,
                        Some(AppMessage::ToggleScreenShareFullscreen),
                        None,
                    ),
                ]
                .align_y(iced::Alignment::Center),
                surface,
                controls,
            ]
            .spacing(SPACE_8),
        )
        .padding(SPACE_12)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|t| iced::widget::container::Style {
            background: Some(iced::Background::Color(
                crate::theme::BoruTheme::for_theme(t).colors.expanded_video_backdrop,
            )),
            ..Default::default()
        });

        stack![base, panel].into()
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

        let mut col = column![].spacing(0).width(
            crate::theme::BoruTheme::for_theme(&theme).chat.context_menu_width,
        );

        match kind {
            ContextMenuKind::Text => {
                if let Some(hash) = self.entries.get(idx).and_then(|entry| entry.message_hash) {
                    let pinned = self.pinned_state.is_pinned(self.topic, &hash);
                    let action = if pinned {
                        AppMessage::UnpinMessage(idx)
                    } else {
                        AppMessage::PinMessage(idx)
                    };
                    let label = if pinned { "Unpin message" } else { "Pin message" };
                    let pin_btn = button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        label,
                    ))
                    .on_press(action)
                    .padding([SPACE_4, SPACE_8])
                    .style(|_t, _s| iced::widget::button::Style::default());
                    col = col.push(container(pin_btn).padding(SPACE_2).width(iced::Length::Fill));
                }
                let copy_btn = button(
                    crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, crate::i18n::t("common.copy_text")),
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
                    crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, crate::i18n::t("common.copy_image")),
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
    ///
    /// BORU-TWEMOJI-04: implementation moved to `crate::emoji::picker` so all
    /// emoji/Twemoji concerns live in the dedicated emoji module. External
    /// behaviour is unchanged; the visual swap to SVG happens in
    /// BORU-TWEMOJI-10.
    ///
    /// BORU-TWEMOJI-12: the picker shows the active category's grid; the
    /// category tab row lives inside the picker and emits
    /// `SelectEmojiCategory` to switch.
    pub(crate) fn view_emoji_picker(&self) -> iced::Element<'_, AppMessage> {
        crate::emoji::picker::view_emoji_picker(
            &self.theme(),
            self.emoji_category,
            &self.emoji_search_query,
            &self.recent_emojis,
        )
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
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("common.close")),
            iced::widget::tooltip::Position::Bottom,
        );

        let header = row![
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, crate::i18n::t("gif.search_title"))
                .color(text_muted(&theme)),
            iced::widget::Space::new().width(iced::Length::Fill),
            close_btn,
        ]
        .spacing(SPACE_4)
        .align_y(iced::Alignment::Center);

        // Search input
        let search_input = text_input(&crate::i18n::t("gif.search_placeholder"), &self.gif_search_text)
            .on_input(AppMessage::GifSearchChanged)
            .on_submit(AppMessage::GifSearchSubmit)
            .size(crate::fonts::TypeRole::Body.size_px())
            .font(crate::fonts::TypeRole::Body.font())
            .padding([SPACE_4, SPACE_6]);

        let search_btn =
            button(crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, crate::i18n::t("common.search")))
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
                    let thumb = crate::theme::BoruTheme::for_theme(&theme).chat;

                    let thumbnail: iced::Element<'_, AppMessage> = match preview {
                        Some(bytes) if !bytes.is_empty() => {
                            let handle = iced::widget::image::Handle::from_bytes(bytes);
                            iced::widget::image(handle)
                                .width(iced::Length::Fixed(thumb.gif_thumbnail_width))
                                .height(iced::Length::Fixed(thumb.gif_thumbnail_height))
                                .into()
                        }
                        _ => container(
                            crate::fonts::type_role_text(
                                crate::fonts::TypeRole::Metadata,
                                "...",
                            )
                            .color(text_muted(&theme)),
                        )
                        .width(thumb.gif_thumbnail_width)
                        .height(thumb.gif_thumbnail_height)
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
                        .width(thumb.gif_thumbnail_width),
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

        let scroll = crate::ui_components::gutter_scrollable(results_col).height(iced::Length::Fixed(
            crate::theme::BoruTheme::for_theme(&theme).chat.gif_picker_scroll_height,
        ));

        container(
            column![header, search_row, privacy_note, scroll]
                .spacing(SPACE_6)
                .padding(SPACE_8),
        )
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
        .width(crate::theme::BoruTheme::for_theme(&theme).chat.gif_picker_width)
        .into()
    }

    pub(crate) fn view_chat_header(&self) -> iced::Element<'_, AppMessage> {
        use iced::widget::{button, column, container, row, text};
        use iced::{Alignment, Length};

        let btheme = self.boru_theme();
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
            peer.map(|key| self.ui_presence(&key))
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
            let group_avatar = container(
                text(display_initials)
                    .size(btheme.typography.chat_sender)
                    .color(letter_color),
            )
                .width(Length::Fixed(AVATAR_CHAT_HEADER))
                .height(Length::Fixed(AVATAR_CHAT_HEADER))
                .center_x(Length::Fixed(AVATAR_CHAT_HEADER))
                .center_y(Length::Fixed(AVATAR_CHAT_HEADER))
                .style(move |t| iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg_surface_secondary(
                        &theme_for_initials,
                    ))),
                    border: iced::Border {
                        radius: (AVATAR_CHAT_HEADER / 2.0).into(),
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
                crate::i18n::t_args(
                    "chat.header.member_count",
                    &[("count", &member_count.to_string())]
                )
            } else {
                crate::i18n::t("chat.group")
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
                    container(
                        iced::widget::image(handle)
                            .content_fit(iced::ContentFit::Cover)
                            .width(Length::Fixed(AVATAR_CHAT_HEADER))
                            .height(Length::Fixed(AVATAR_CHAT_HEADER))
                            // Clip to circle — container radius does not
                            // clip children in iced.
                            .border_radius(AVATAR_CHAT_HEADER / 2.0),
                    )
                    .style(|_t| iced::widget::container::Style {
                        border: iced::Border {
                            radius: (AVATAR_CHAT_HEADER / 2.0).into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into()
                })
                .unwrap_or_else(|| {
                    let initials = crate::presentation::initials(&room_name);
                    let theme_for_initials = self.theme();
                    let is_dark = matches!(theme_for_initials, iced::Theme::Dark);
                    let letter_color = crate::presentation::initials_color(&room_name, is_dark);
                    container(
                        text(initials)
                            .size(btheme.typography.chat_sender)
                            .color(letter_color),
                    )
                        .width(Length::Fixed(AVATAR_CHAT_HEADER))
                        .height(Length::Fixed(AVATAR_CHAT_HEADER))
                        .center_x(Length::Fixed(AVATAR_CHAT_HEADER))
                        .center_y(Length::Fixed(AVATAR_CHAT_HEADER))
                        .style(move |t| iced::widget::container::Style {
                            background: Some(iced::Background::Color(bg_surface_secondary(
                                &theme_for_initials,
                            ))),
                            border: iced::Border {
                                radius: (AVATAR_CHAT_HEADER / 2.0).into(),
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("common.confirm_delete")),
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
            self.calls_state.active_call_id.is_some(),
        );

        // BORU-CP-12 (PDF Task 4.3): negotiated capability support. Before
        // offering a peer-facing optional feature the UI checks that the
        // peer's client advertises a compatible version. When the peer is
        // unknown/unsupported the button renders DISABLED with an
        // explanatory tooltip ("feature UI can explain why an action is
        // unavailable") instead of silently attempting the operation.
        let voice_offered = peer
            .map(|key| self.feature_offered(&key, boru_core::control_plane::features::VOICE))
            .unwrap_or(false);
        let video_offered = peer
            .map(|key| self.feature_offered(&key, boru_core::control_plane::features::VIDEO))
            .unwrap_or(false);
        let voice_call: iced::Element<'_, AppMessage> = match peer {
            Some(key) if call_enabled && voice_offered => tool_btn(
                Icon::Phone.build().size(IconSize::Sm).build().into(),
                "Start voice call",
                Some(AppMessage::StartVoiceCall(key)),
            ),
            Some(_) if call_enabled => tool_btn(
                Icon::Phone.build().size(IconSize::Sm).build().into(),
                "Voice calls unavailable — this peer's client does not support voice calls",
                None,
            ),
            _ => iced::widget::Space::new().width(Length::Fixed(0.0)).into(),
        };
        let video_call: iced::Element<'_, AppMessage> = match peer {
            Some(key) if call_enabled && video_offered => tool_btn(
                Icon::VideoCamera.build().size(IconSize::Sm).build().into(),
                "Start video call",
                Some(AppMessage::StartVideoCall(key)),
            ),
            Some(_) if call_enabled => tool_btn(
                Icon::VideoCamera.build().size(IconSize::Sm).build().into(),
                "Video calls unavailable — this peer's client does not support video calls",
                None,
            ),
            _ => iced::widget::Space::new().width(Length::Fixed(0.0)).into(),
        };

        #[cfg(feature = "screen-sharing")]
        let screen_share_offered = peer
            .map(|key| self.feature_offered(&key, boru_core::control_plane::features::SCREEN_SHARE))
            .unwrap_or(false);
        #[cfg(feature = "screen-sharing")]
        let screen_share: iced::Element<'_, AppMessage> = match peer {
            Some(key)
                if !is_group
                    && !is_blocked
                    && matches!(
                        self.calls_state.screen_share_host_state,
                        ScreenShareHostState::Idle
                            | ScreenShareHostState::Stopped
                            | ScreenShareHostState::Error(_)
                    )
                    && screen_share_offered =>
            {
                tool_btn(
                    Icon::Monitor.build().size(IconSize::Sm).build().into(),
                    "Share screen",
                    Some(AppMessage::StartScreenShare(key)),
                )
            }
            Some(_)
                if !is_group
                    && !is_blocked
                    && matches!(
                        self.calls_state.screen_share_host_state,
                        ScreenShareHostState::Idle
                            | ScreenShareHostState::Stopped
                            | ScreenShareHostState::Error(_)
                    ) =>
            {
                tool_btn(
                    Icon::Monitor.build().size(IconSize::Sm).build().into(),
                    "Screen sharing unavailable — this peer's client does not support screen sharing",
                    None,
                )
            }
            _ => iced::widget::Space::new().width(Length::Fixed(0.0)).into(),
        };

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
            .map(|key| self.ui_presence(&key))
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
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("common.close")),
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

        let input = text_input(&crate::i18n::t("chat.search.placeholder"), &self.chat_search_query)
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
                    color: crate::theme::BoruTheme::for_theme(t).colors.panel_shadow,
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

        let btheme = self.boru_theme();

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
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, crate::i18n::t("chat.group_members")),
            Space::new().width(Length::Fill),
            iced::widget::tooltip::Tooltip::new(
                button(icon_svg(ICON_CLOSE, TYPO_SM))
                    .on_press(AppMessage::ToggleMemberList)
                    .padding([SPACE_4, SPACE_6])
                    .style(BUTTON_ICON),
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("common.close")),
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
                            container(
                                text(display_initials)
                                    .size(btheme.typography.chat_metadata)
                                    .color(letter_color),
                            )
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
                    color: crate::theme::BoruTheme::for_theme(t).colors.panel_shadow,
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
        let is_advertised = self.rooms_state.advertised_rooms.contains(&self.topic);

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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("chat.topic_label"))
                    .color(self.color_muted()),
                crate::fonts::type_role_text(crate::fonts::TypeRole::TechnicalValue, topic_hex.clone()),
            ]
            .spacing(SPACE_4),
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("chat.ticket_label"))
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("chat.online_label"))
                    .color(self.color_muted()),
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::Metadata,
                    format!("{}", online_peers),
                ),
            ]
            .spacing(SPACE_4),
            // ── Directory visibility (BORU-DIR-06, PDF 2.3) ──
            // Only the room owner/admin may change directory visibility;
            // non-authorized users get a muted note and no control.
            if self.is_room_directory_owner(self.topic) {
                let visibility_label = match self
                    .conversation_store
                    .find(&self.topic)
                    .map(|e| e.visibility)
                    .unwrap_or(RoomVisibility::Private)
                {
                    RoomVisibility::PublicDiscoverable => "Public — Discoverable",
                    RoomVisibility::PublicUnlisted => "Public — Unlisted",
                    RoomVisibility::Private => "Private",
                };
                let column = column![
                    row![
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            "Directory:",
                        )
                        .color(self.color_muted()),
                        crate::fonts::type_role_text(
                            crate::fonts::TypeRole::Metadata,
                            visibility_label,
                        ),
                    ]
                    .spacing(SPACE_4)
                    .align_y(Alignment::Center),
                    button(crate::fonts::type_role_text(
                        crate::fonts::TypeRole::ButtonLabel,
                        if is_advertised {
                            "✓ Advertised — Change visibility"
                        } else {
                            "Change directory visibility…"
                        },
                    ))
                    .on_press(AppMessage::OpenRoomSettings(self.topic))
                    .style(BUTTON_GHOST_BG)
                    .padding([SPACE_4, SPACE_10])
                    .width(Length::Fill),
                ]
                .spacing(SPACE_4);
                let element: iced::Element<'_, AppMessage> = column.into();
                element
            } else {
                crate::fonts::type_role_text(
                    crate::fonts::TypeRole::SupportingText,
                    "Only the room owner can change directory visibility.",
                )
                .color(self.color_muted())
                .into()
            },
            // ── Separator ──
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "───")
                .color(self.color_muted()),
            // ── Notification policy ──
            row![
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, "Notifications:"),
                button(crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "All"))
                    .on_press(AppMessage::SetConversationNotificationPolicy(self.topic, Some(crate::notification::service::NotificationPolicy::All)))
                    .style(BUTTON_OUTLINE)
                    .padding([SPACE_4, SPACE_8]),
                button(crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Mentions"))
                    .on_press(AppMessage::SetConversationNotificationPolicy(self.topic, Some(crate::notification::service::NotificationPolicy::MentionsOnly)))
                    .style(BUTTON_OUTLINE)
                    .padding([SPACE_4, SPACE_8]),
                button(crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Mute"))
                    .on_press(AppMessage::SetConversationNotificationPolicy(self.topic, Some(crate::notification::service::NotificationPolicy::Muted)))
                    .style(BUTTON_OUTLINE)
                    .padding([SPACE_4, SPACE_8]),
                button(crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, "Global"))
                    .on_press(AppMessage::SetConversationNotificationPolicy(self.topic, None))
                    .style(BUTTON_GHOST_BG)
                    .padding([SPACE_4, SPACE_8]),
            ]
            .spacing(SPACE_4)
            .align_y(Alignment::Center),
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
                    color: crate::theme::BoruTheme::for_theme(t).colors.panel_shadow,
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
            container(crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("chat.group"))
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, crate::i18n::t("groups.invite_member"))
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, crate::i18n::t("groups.notifications")),
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, crate::i18n::t("files.shared")),
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Body, crate::i18n::t("groups.information")),
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
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, crate::i18n::t("common.details")),
            Space::new().height(Length::Fixed(SPACE_8)),
            // Info section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("groups.info_short"))
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(info_items).spacing(SPACE_4),
            divider(&theme),
            // Members section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("groups.members"))
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
            .map(|key| self.ui_presence(&key))
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
            crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("status.direct"))
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, crate::i18n::t("common.copy"))
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("profile.peer_id"))
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
                crate::i18n::t("connection.title"),
                connection_type.to_string(),
                &theme,
            )
            .into(),
        );

        // Relay mode
        let relay_label = fmt_relay_mode(&self.relay_mode);
        conn_items.push(info_row(crate::i18n::t("connection.relay"), relay_label, &theme).into());

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
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, crate::i18n::t("common.copy"))
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
                        crate::i18n::t("tunnels.create"),
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
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, crate::i18n::t("common.details")),
            Space::new().height(Length::Fixed(SPACE_8)),
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("connection.contact"))
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(contact_items).spacing(SPACE_4),
            divider(&theme),
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("connection.title"))
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(conn_items).spacing(SPACE_4),
            divider(&theme),
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("connection.security"))
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(security_items).spacing(SPACE_4),
            divider(&theme),
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("common.tools"))
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
            .unwrap_or_else(|| crate::i18n::t("chat.group"));
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
            crate::i18n::t_args(
                "chat.header.member_count",
                &[("count", &member_count.to_string())]
            )
        } else {
            crate::i18n::t("chat.group")
        };
        info_items.push(info_row(crate::i18n::t("groups.members"), member_label, &theme).into());

        if is_owner {
            info_items.push(info_row(crate::i18n::t("chat.role"), crate::i18n::t("chat.role_owner"), &theme).into());
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::ButtonLabel, crate::i18n::t("groups.leave"))
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
            crate::fonts::type_role_text(crate::fonts::TypeRole::CardTitle, crate::i18n::t("groups.info")),
            Space::new().height(Length::Fixed(SPACE_8)),
            // Group Info section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("groups.about"))
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(info_items).spacing(SPACE_4),
            divider(&theme),
            // Members section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("groups.members"))
                .color(text_secondary(&theme)),
            Space::new().height(Length::Fixed(SPACE_2)),
            column(member_items).spacing(SPACE_4),
            divider(&theme),
            // Advanced section
            crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("common.advanced"))
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::SupportingText, crate::i18n::t("common.actions"))
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

    /// Wrap the chat timeline's message content in the readable-column
    /// layout.
    ///
    /// The scrollable viewport spans the FULL chat pane width so its
    /// scrollbar sits flush with the right edge; this wrapper keeps the
    /// message content (bubbles, cards, separators) within a centered
    /// column capped at `max_width` on wide windows.
    ///
    /// A vertical scrollable draws its content left-anchored, so the cap
    /// lives on an INNER container that a full-width OUTER container
    /// centers — `container::center_x` alone would only center the child
    /// inside the container, leaving the capped column hugging the left
    /// edge (the BORU-RESP-04 regression this replaces).
    fn readable_chat_column<'a>(
        content: impl Into<iced::Element<'a, AppMessage>>,
        max_width: f32,
    ) -> iced::Element<'a, AppMessage> {
        iced::widget::container(
            iced::widget::container(content)
                .width(iced::Length::Fill)
                .max_width(max_width),
        )
        .width(iced::Length::Fill)
        .align_x(iced::Alignment::Center)
        .into()
    }

    pub(crate) fn view_chat_log(
        &self,
        timeline_width: f32,
        viewport_height: f32,
    ) -> iced::widget::Scrollable<'_, AppMessage> {
        #[cfg(feature = "dev-ui")]
        let _designer_component = crate::designer::ComponentId::ChatMessageList;

        use iced::widget::space;
        use iced::widget::text::Wrapping;
        use iced::widget::{button, container, scrollable, text, Column, Row};
        use iced::{Alignment, Length};

        let _start = std::time::Instant::now();

        // Readable-column cap for the message content (never for the
        // scrollable viewport itself). The scrollable spans the full chat
        // pane so the scrollbar sits flush with the right edge; this cap
        // keeps bubbles/cards within a centered readable column on wide
        // windows.
        let content_max_width = self.boru_layout().responsive.content_max_width;

        // ── Ensure layout cache is up-to-date ──
        // Uses the incrementally maintained cache so the height/cumulative passes
        // only run when entries or settings actually change, not on every frame.
        let lc = &mut *self.layout_cache.borrow_mut();
        lc.ensure(&self.entries, self.settings_state.chat_text_size, timeline_width);

        let total_entries = self.entries.len();
        let total_image_bytes = lc.total_image_bytes;
        let image_entry_count = lc.image_entry_count;

        let theme = self.theme();
        let btheme = self.boru_theme();

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
            // Readable-column wrapper: the scrollable viewport spans the
            // full chat pane width (scrollbar flush right), while the
            // message content stays within the capped, centered column.
            let col = Self::readable_chat_column(col, content_max_width);
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
        let chat_layout = self.boru_layout().chat.clone();
        let bubble_max_w = crate::presentation::chat_bubble_max_width_with(
            timeline_width,
            chat_layout.bubble_max_width,
            chat_layout.bubble_width_ratio,
        );

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
                                        .size(btheme.type_size(crate::fonts::TypeRole::ChatSender))
                                        .font(btheme.type_font(crate::fonts::TypeRole::ChatSender))
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
                            .size(btheme.type_size(crate::fonts::TypeRole::ChatSender))
                            .font(btheme.type_font(crate::fonts::TypeRole::ChatSender))
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
                                .size(btheme.type_size(crate::fonts::TypeRole::ChatSender))
                                .font(btheme.type_font(crate::fonts::TypeRole::ChatSender))
                                .color(label_color),
                        )
                        .on_press(AppMessage::RetryOutgoingMessage(event_id))
                        .padding(0)
                        .style(|_t, _s| iced::widget::button::Style::default())
                        .into()
                    } else {
                        text(label_text)
                            .size(btheme.type_size(crate::fonts::TypeRole::ChatSender))
                            .font(btheme.type_font(crate::fonts::TypeRole::ChatSender))
                            .color(label_color)
                            .into()
                    }
                };

            // ── Clickable emoji-aware URL-aware body ──
            let segments = entry.parsed_segments.as_deref().unwrap_or(&[]);
            // BORU-TWEMOJI-17: emoji inside the body render as inline
            // Twemoji SVGs (text runs keep Boru's message typography). The
            // stored/copied message stays the original Unicode string —
            // this is presentation only.
            let emoji_style = crate::emoji::emoji_text::EmojiTextStyle {
                size: self.settings_state.chat_text_size,
                font: btheme.type_font(crate::fonts::TypeRole::ChatMessage),
                line_height: iced::widget::text::LineHeight::Relative(
                    btheme.type_line_height(crate::fonts::TypeRole::ChatMessage),
                ),
                wrapping: Wrapping::WordOrGlyph,
                color: body_color,
            };
            let emoji_renderer = crate::emoji::renderer::TwemojiRenderer;
            let body_el: iced::Element<'_, AppMessage> = if segments.len() == 1
                && matches!(&segments[0], link_preview::TextSegment::Text(_))
            {
                // No URLs — emoji-aware text element. `WordOrGlyph` wraps at
                // word boundaries and falls back to glyph-level breaking for
                // unbreakable words (public keys, pasted tokens, very long
                // single words) so the bubble never overflows its width cap.
                // Emoji-free messages take the fast path inside `emoji_text`
                // and render exactly like the previous plain text element.
                crate::emoji::emoji_text::emoji_text(
                    &emoji_renderer,
                    &entry.body,
                    &emoji_style,
                )
            } else {
                // Mixed text and URLs — build a segmented row. Text segments
                // get the same emoji treatment; URL segments stay clickable.
                let mut row = Row::new().spacing(0);
                for seg in segments {
                    match seg {
                        link_preview::TextSegment::Text(t) => {
                            row = row.push(crate::emoji::emoji_text::emoji_text(
                                &emoji_renderer,
                                t,
                                &emoji_style,
                            ));
                        }
                        link_preview::TextSegment::Url(u) => {
                            let display = link_preview::truncate_url(&u, 80);
                            let url_for_click = u.clone();
                            row = row.push(
                                button(
                                    text(display)
                                        .size(self.settings_state.chat_text_size)
                                        .font(btheme.type_font(crate::fonts::TypeRole::ChatMessage))
                                        .line_height(iced::widget::text::LineHeight::Relative(
                                            btheme.type_line_height(crate::fonts::TypeRole::ChatMessage),
                                        ))
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
                .size(btheme.type_size(crate::fonts::TypeRole::ChatMetadata))
                .font(btheme.type_font(crate::fonts::TypeRole::ChatMetadata))
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
                            .size(btheme.typography.chat_sender)
                            .font(btheme.type_font(crate::fonts::TypeRole::ChatMessage))
                            .wrapping(Wrapping::WordOrGlyph)
                            .color(accent_primary(&theme))
                            .into(),
                    );
                }
                if let Some(ref desc) = preview.description {
                    preview_children.push(
                        text(desc)
                            .size(btheme.typography.chat_metadata)
                            .font(btheme.type_font(crate::fonts::TypeRole::ChatMessage))
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
                            .height(Length::Fixed(
                                self.boru_layout().chat.image_preview_max_height.min(180.0),
                            ))
                            .content_fit(iced::ContentFit::Cover)
                            .into(),
                    );
                } else if let Some(ref img_url) = preview.image_url {
                    let display_url = link_preview::truncate_url(img_url, 60);
                    preview_children.push(
                        text(display_url)
                            .size(btheme.typography.chat_metadata)
                            .font(btheme.type_font(crate::fonts::TypeRole::ChatMessage))
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
                                        .size(btheme.typography.chat_metadata)
                                        .font(btheme.type_font(crate::fonts::TypeRole::ChatMessage))
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
                        .size(btheme.typography.chat_metadata)
                        .font(btheme.type_font(crate::fonts::TypeRole::ChatMessage))
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
                    .width(Length::Fixed(AVATAR_MSG))
                    .height(Length::Fixed(AVATAR_MSG))
                    .into()
            } else if let Some(ref handle) = entry.avatar_handle {
                container(
                    iced::widget::image(handle.clone())
                        .content_fit(iced::ContentFit::Cover)
                        .width(Length::Fixed(AVATAR_MSG))
                        .height(Length::Fixed(AVATAR_MSG))
                        // Clip to circle — container radius does not clip
                        // children in iced.
                        .border_radius(AVATAR_MSG / 2.0),
                )
                .style(|_t| iced::widget::container::Style {
                    border: iced::Border {
                        radius: (AVATAR_MSG / 2.0).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
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
                container(
                    text(initial)
                        .size(btheme.typography.chat_sender)
                        .color(letter_color),
                )
                    .width(Length::Fixed(AVATAR_MSG))
                    .height(Length::Fixed(AVATAR_MSG))
                    .center_x(Length::Fixed(AVATAR_MSG))
                    .center_y(Length::Fixed(AVATAR_MSG))
                    .style(|t| iced::widget::container::Style {
                        background: Some(iced::Background::Color(bg_surface_secondary(t))),
                        border: iced::Border {
                            radius: (AVATAR_MSG / 2.0).into(),
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
                                        .size(btheme.typography.chat_metadata)
                                        .font(btheme.type_font(crate::fonts::TypeRole::ChatMessage))
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
                            .size(btheme.typography.chat_sender)
                            .font(btheme.type_font(crate::fonts::TypeRole::ChatMessage))
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
        // Readable-column wrapper: the scrollable viewport spans the full chat
        // pane width (scrollbar flush right); the message content stays within
        // the capped, centered column.
        let content = Self::readable_chat_column(col, content_max_width);
        crate::ui_components::gutter_scrollable(content)
            .id(CHAT_LOG)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .on_scroll(|v: scrollable::Viewport| {
                AppMessage::Scrolled(v.absolute_offset().y, v.bounds().height)
            })
    }

    pub(crate) fn view_composer(&self) -> iced::Element<'_, AppMessage> {
        #[cfg(feature = "dev-ui")]
        let _designer_component = crate::designer::ComponentId::ChatComposer;

        use crate::design_tokens::SPACE_8;
        use iced::widget::{button, container, row, text, text_input};
        use iced::{Alignment, Length, Padding};

        let btheme = self.boru_theme();
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("chat.composer.attach")),
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("chat.composer.share_folder")),
                iced::widget::tooltip::Position::Bottom,
            )
            .into();

        // ── Center: expandable message input ── transparent bg, fills space
        let input = text_input(&crate::i18n::t("chat.composer.placeholder"), &self.composer_text)
            .id(COMPOSER_INPUT)
            .on_input(AppMessage::InputChanged)
            .on_submit(AppMessage::SendPressed)
            .size(self.settings_state.chat_text_size)
            .font(btheme.type_font(crate::fonts::TypeRole::ComposerText))
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
                            radius: crate::theme::BoruTheme::for_theme(t).radii.xl.into(),
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("chat.composer.emoji")),
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
                iced::Element::from(text("…").size(btheme.typography.composer_text))
            } else {
                iced::Element::from(
                    icon_svg(ICON_SEND, IconSize::Sm.px())
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
                crate::fonts::type_role_text(crate::fonts::TypeRole::Metadata, crate::i18n::t("chat.send_enter")),
                iced::widget::tooltip::Position::Bottom,
            )
            .into();

        // ── Composer row ──
        //  attach | text input (fill) | gif | emoji | send
        let composer_bar = row![attach_btn, folder_btn, input, gif_btn, emoji_btn, send_btn]
            .spacing(self.boru_layout().chat.composer.spacing)
            .align_y(Alignment::Center)
            .padding(Padding::new(self.boru_layout().chat.composer.padding));

        // ── Elevated rounded composer container ──
        //  16 px radius surface with a 1 px border and a very subtle shadow
        //  (plan §4: composer elevation ~0 1 2).  While a window file is
        //  dragged over the app the border adopts the accent colour as a
        //  subtle focus treatment (file-drop routes through the same
        //  attachment pipeline).
        container(composer_bar)
            .width(Length::Fill)
            .padding(Padding::new(0.0))
            .style(move |t: &iced::Theme| {
                let b = crate::theme::BoruTheme::for_theme(t);
                iced::widget::container::Style {
                    background: Some(iced::Background::Color(bg_surface_secondary(t))),
                    border: iced::Border {
                        width: b.borders.hairline,
                        color: if self.composer_drag_over {
                            accent_primary(t)
                        } else {
                            border_muted(t)
                        },
                        radius: b.radii.xl.into(),
                    },
                    shadow: crate::design_tokens::shadow_card(t),
                    ..Default::default()
                }
            })
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
                let was_nonempty = !self.composer_text.trim().is_empty();
                self.composer_text = text;

                // Typing is an authenticated, ephemeral lease.  Throttle the
                // refreshes so holding a key cannot flood gossip; privacy is
                // opt-out and no event is sent when disabled.
                if !self.composer_text.trim().is_empty()
                    && self.settings_state.typing_indicators_enabled
                    && self.typing_emitter.should_emit(std::time::Instant::now())
                {
                    if let Some(sender) = self.sender.clone() {
                        if let Ok(bytes) = SignedMessage::sign_and_encode(
                            &self.secret_key,
                            &crate::Message::Typing { active: true },
                        ) {
                            task::spawn(async move {
                                let _ = sender.broadcast(bytes).await;
                            });
                        }
                    }
                } else if was_nonempty && self.settings_state.typing_indicators_enabled {
                    self.typing_emitter.reset();
                    if let Some(sender) = self.sender.clone() {
                        if let Ok(bytes) = SignedMessage::sign_and_encode(
                            &self.secret_key,
                            &crate::Message::Typing { active: false },
                        ) {
                            task::spawn(async move {
                                let _ = sender.broadcast(bytes).await;
                            });
                        }
                    }
                }

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
                    self.typing_emitter.reset();
                    return iced::Task::none();
                }
                self.typing_emitter.reset();
                if self.settings_state.typing_indicators_enabled {
                    if let Some(sender) = self.sender.clone() {
                    if let Ok(bytes) = SignedMessage::sign_and_encode(
                        &self.secret_key,
                        &crate::Message::Typing { active: false },
                    ) {
                        task::spawn(async move {
                            let _ = sender.broadcast(bytes).await;
                        });
                    }
                }
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
                    // BORU-APP-002: route through the help-overlay domain.
                    self.help_overlay.update(HelpMessage::Toggle);
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
                    let _progress_queue = self.files_state.download_progress_queue.clone();
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

                // Thread reply command: `/reply <root-hex> <text>`.
                // Keeping this as a command gives narrow layouts a usable
                // thread entry point while the focused panel is optional.
                let (text, thread_target) = if let Some(rest) = trimmed.strip_prefix("/reply ") {
                    let mut parts = rest.splitn(2, char::is_whitespace);
                    let root_hex = parts.next().unwrap_or_default();
                    let text = parts.next().unwrap_or_default().trim();
                    let root = match hex::decode(root_hex).ok().and_then(|bytes| bytes.try_into().ok()) {
                        Some(root) => root,
                        None => {
                            self.push_system("Usage: /reply <root-hash-hex> <text>".to_string());
                            return iced::Task::none();
                        }
                    };
                    if text.is_empty() {
                        self.push_system("Usage: /reply <root-hash-hex> <text>".to_string());
                        return iced::Task::none();
                    }
                    (text.to_string(), Some(boru_core::threads::ThreadTarget::root(root)))
                } else {
                    (trimmed.clone(), None)
                };

                // Normal text message
                let _timer = PerfTracker::timer("send_message", "text");
                match self.persist_outgoing_message_with_target(self.topic, &text, thread_target) {
                    Ok((event_id, msg_hash, encoded)) => {
                        self.self_sent_events.insert(msg_hash, event_id);
                        // BORU-CP-13: record the outbound direct broadcast
                        // into the per-peer diagnostics snapshot (direct
                        // conversations only; groups/public rooms have no
                        // single peer). Timestamp-only, never chat content.
                        if let Some(peer) = self.current_direct_peer() {
                            self.report_direct_broadcast(peer);
                        }
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
                // ── BORU-DISC-10 defensive routing guard ────────────────
                // The internal discovery topic is owned by DiscoveryService
                // (joined at startup in main.rs). Discovery payloads must
                // NEVER reach conversation handling: no touch_and_bump, no
                // ConversationLive creation, no preview/unread, no
                // persistence, no rendering. This is the GUI-bridge
                // backstop; the primary guard lives at the forwarder-spawn
                // boundary (spawn_conversation_forwarder).
                if boru_core::discovery_topic::topic_kind(conv_event.topic)
                    == boru_core::discovery_topic::TopicKind::Discovery
                {
                    tracing::warn!(
                        topic = %conv_event.topic,
                        "dropping discovery-topic NetEvent before conversation handling"
                    );
                    return iced::Task::none();
                }
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
                    // Emit only live background messages; backfill restores
                    // history/unread state without causing notifications.
                    if let NetEvent::Message {
                        from,
                        message,
                        backfilled: false,
                        ..
                    } = &event
                    {
                        self.emit_message_notification(&topic, from, message);
                    }
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
                    // on dense public topics.  They are
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
                    self.notifications_state.show_toast_message(format!("Copied: {preview}"));
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
                    self.notifications_state.show_toast_message(format!("Copied: {preview}"));
                    return iced::clipboard::write(entry.body.clone());
                }
                iced::Task::none()
            }

            AppMessage::ContextCopyImage(idx) => {
                self.context_menu = None;
                // Image copy not yet implemented for system clipboard;
                // the context menu still appears for future wiring.
                self.notifications_state
                .show_toast_message("Image copy not yet supported".to_string());
                iced::Task::none()
            }

            message @ (AppMessage::PinMessage(idx) | AppMessage::UnpinMessage(idx)) => {
                self.context_menu = None;
                let Some(hash) = self.entries.get(idx).and_then(|entry| entry.message_hash) else {
                    return iced::Task::none();
                };
                let action = if matches!(message, AppMessage::PinMessage(_)) {
                    boru_core::pinned_messages::PinAction::Pin
                } else {
                    boru_core::pinned_messages::PinAction::Unpin
                };
                self.pinned_state.apply_authenticated(
                    self.topic,
                    hash,
                    action,
                    self.local_public,
                    boru_core::chat_core::now_secs(),
                );
                let wire = match action {
                    boru_core::pinned_messages::PinAction::Pin => crate::Message::PinMessage {
                        topic: self.topic,
                        message_hash: hash,
                    },
                    boru_core::pinned_messages::PinAction::Unpin => crate::Message::UnpinMessage {
                        topic: self.topic,
                        message_hash: hash,
                    },
                };
                if let (Some(sender), Ok(encoded)) = (
                    &self.sender,
                    SignedMessage::sign_and_encode(&self.secret_key, &wire),
                ) {
                    let sender = sender.clone();
                    return iced::Task::perform(
                        async move {
                            sender.broadcast(encoded).await.ok();
                        },
                        |_| AppMessage::Noop,
                    );
                }
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

            AppMessage::SelectEmojiCategory(category) => {
                // BORU-TWEMOJI-12: remember the active category so the next
                // picker view shows the same tab; the grid is rebuilt from
                // the filtered catalog in the view, so no stale items.
                self.emoji_category = category;
                iced::Task::none()
            }

            AppMessage::EmojiSearchChanged(query) => {
                // BORU-TWEMOJI-13: remember the live query; the picker view
                // filters the shared catalog on every frame, so the result
                // list updates immediately as the query changes and an
                // empty query restores the category view.
                self.emoji_search_query = query;
                iced::Task::none()
            }

            AppMessage::InsertEmoji(emoji) => {
                // Insert the emoji at the current cursor position
                self.composer_text.push_str(&emoji);
                // BORU-TWEMOJI-14: record the selection in the recently-used
                // list (move-to-front, deduplicated, capped) and persist it
                // through Boru's normal local settings (settings.json). Only
                // the Unicode string is stored — never an asset key or SVG
                // path — and the list is never transmitted on the wire.
                let recents =
                    crate::emoji::recents::record_recent(&self.recent_emojis, &emoji);
                if recents != self.recent_emojis {
                    self.recent_emojis = recents;
                    self.save_settings();
                }
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
                        self.notifications_state.bump_toast_counter();
                        self.notifications_state.show_toast_message(format!("Failed to send GIF: {e}"));
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
                        let progress_queue = self.files_state.download_progress_queue.clone();
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
                    // Chat renders the video thumbnail only — playback always
                    // happens in the expanded overlay. Open it as soon as a
                    // fresh session starts so the player never renders inline
                    // in the chat card (it appears once the decoder loads).
                    self.inline_video_expanded = true;
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
                    let progress_queue = self.files_state.download_progress_queue.clone();

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
                // Chat surfaces render the video thumbnail only, so there is
                // no inline player to degrade to: closing the expanded
                // overlay stops playback entirely (CloseInlineVideo
                // semantics) instead of leaving an invisible session.
                if self.inline_video_expanded {
                    self.stop_inline_video();
                }
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
                if let Some(tracker) = self.rooms_state.room_trackers.remove(&topic) {
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
                // BORU-DIR-09 (PDF Task 3.3): if the deleted room was
                // advertised in the public room directory, emit a withdrawal
                // so remote directories remove it immediately, and drop the
                // local advertisement entry. TTL expiry remains the safety
                // net if the withdrawal is missed.
                if self.rooms_state.advertised_rooms.remove(&topic) {
                    let local_author = self.local_public;
                    let _ = self.directory_store.lock().map(|mut store| {
                        store.withdraw(topic, local_author)
                    });
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
                    self.broadcast_room_withdrawal(topic);
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
                if let Some(tracker) = self.rooms_state.room_trackers.remove(&topic) {
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
                iced::Task::none()
            }
            // update() only dispatches the chat variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}

#[cfg(all(test, feature = "screen-sharing"))]
mod tests {
    use super::*;

    #[test]
    fn source_kind_icon_maps_every_capture_kind_to_distinct_icon() {
        use boru_core::screen_share::CaptureSourceKind;
        let kinds = [
            (CaptureSourceKind::Monitor, Icon::Monitor),
            (CaptureSourceKind::Window, Icon::Window),
            (CaptureSourceKind::Desktop, Icon::Desktop),
        ];
        for (kind, expected) in kinds {
            assert_eq!(IcedChat::source_kind_icon(kind), expected);
        }
        // Distinct icons for distinct kinds (acceptance: different icons
        // for monitor/desktop/window).
        let icons: Vec<Icon> = kinds
            .iter()
            .map(|(k, _)| IcedChat::source_kind_icon(*k))
            .collect();
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    /// BORU-SSUI-04 (PDF Task 4): the quality segmented control maps each
    /// segment to the exact preset the old text buttons dispatched, and
    /// exactly one segment is visually selected for any chosen preset
    /// (None = Auto / path-derived).
    #[test]
    fn quality_segments_map_presets_and_select_exactly_one() {
        use boru_core::screen_share::QualityPreset;
        // (label key, dispatched preset, is_auto)
        let expectations = [
            (
                "screenshare.preset_lan_high",
                Some(QualityPreset::LanHigh),
                false,
            ),
            (
                "screenshare.preset_balanced",
                Some(QualityPreset::Balanced),
                false,
            ),
            (
                "screenshare.preset_relay",
                Some(QualityPreset::RelayConservative),
                false,
            ),
            ("screenshare.preset_auto", None, true),
        ];
        for selected in [
            None,
            Some(QualityPreset::LanHigh),
            Some(QualityPreset::Balanced),
            Some(QualityPreset::RelayConservative),
        ] {
            let segments = IcedChat::quality_segment_specs(selected);
            assert_eq!(
                segments.len(),
                expectations.len(),
                "one segment per quality mode"
            );
            let selected_count = segments.iter().filter(|s| s.selected).count();
            assert_eq!(
                selected_count, 1,
                "exactly one segment visually selected for {selected:?}"
            );
            for (spec, (label_key, preset, is_auto)) in segments.iter().zip(expectations.iter()) {
                assert_eq!(&spec.label_key, label_key, "runtime source label key");
                assert_eq!(&spec.preset, preset, "dispatch target for {label_key}");
                assert_eq!(
                    spec.selected,
                    if *is_auto {
                        selected.is_none()
                    } else {
                        selected == *preset
                    },
                    "selection state for {label_key}"
                );
            }
        }
    }

    /// BORU-SSUI-05 (PDF Task 5): the remote-control status area maps the
    /// authoritative control state to a STATE-ONLY label — the permission
    /// model has no direct sender-side toggle, so the spec never invents
    /// one, and the runtime label keys are the existing i18n ON/OFF keys.
    #[test]
    fn remote_control_status_spec_maps_state_to_label() {
        let on = IcedChat::remote_control_status_spec(true);
        assert_eq!(on.label_key, "screenshare.remote_control_on");
        assert!(on.active);
        let off = IcedChat::remote_control_status_spec(false);
        assert_eq!(off.label_key, "screenshare.remote_control_off");
        assert!(!off.active);
        // Labels must resolve to real runtime text (never empty keys).
        assert_eq!(crate::i18n::t(on.label_key), "Remote control: ON");
        assert_eq!(crate::i18n::t(off.label_key), "Remote control: OFF");
    }

    /// BORU-SSUI-05: the new input/control icon maps to the mouse-pointer
    /// asset (distinct from the source-picker icons used by Task 3).
    #[test]
    fn mouse_pointer_icon_maps_to_control_asset() {
        let bytes = Icon::MousePointer.bytes();
        let svg = String::from_utf8_lossy(bytes);
        // The lucide mouse-pointer-2 path (distinctive "l6 6.5" pointer body).
        assert!(svg.contains("16 6.5"), "mouse-pointer-2 path data");
        assert!(svg.starts_with("<svg"), "SVG root element");
        assert_ne!(Icon::MousePointer, Icon::Monitor);
        assert_ne!(Icon::MousePointer, Icon::Window);
        assert_ne!(Icon::MousePointer, Icon::Desktop);
    }

    /// BORU-SSUI-06 (PDF Task 6): the audio toggle spec maps the
    /// authoritative audio state to a speaker icon + "Audio" label, and
    /// disables the switch when the host reported audio cannot be shared
    /// (typed unavailable error — existing capability detection).
    #[test]
    fn audio_toggle_spec_maps_state_to_icon_and_enabled() {
        // ON + available → speaker-on icon, switch enabled.
        let on = IcedChat::audio_toggle_spec(true, false);
        assert_eq!(on.icon, Icon::Volume2);
        assert_eq!(on.label_key, "screenshare.audio");
        assert!(on.active);
        assert!(on.enabled);
        // OFF + available → muted speaker icon, switch still enabled
        // (turning it ON maps to the current audio-sharing path).
        let off = IcedChat::audio_toggle_spec(false, false);
        assert_eq!(off.icon, Icon::VolumeX);
        assert!(!off.active);
        assert!(off.enabled);
        // Unavailable → switch disabled regardless of the mirror value;
        // the label stays the same runtime "Audio" text.
        let blocked = IcedChat::audio_toggle_spec(true, true);
        assert_eq!(blocked.icon, Icon::Volume2);
        assert!(!blocked.enabled);
        assert_eq!(blocked.label_key, "screenshare.audio");
        let blocked_off = IcedChat::audio_toggle_spec(false, true);
        assert!(!blocked_off.enabled);
        assert_eq!(blocked_off.icon, Icon::VolumeX);
    }

    /// BORU-SSUI-06: the speaker icons map to distinct lucide volume
    /// assets (on = volume-2, off = volume-x) — never the same glyph for
    /// two states.
    #[test]
    fn audio_speaker_icons_map_to_distinct_volume_assets() {
        let on = String::from_utf8_lossy(Icon::Volume2.bytes());
        let off = String::from_utf8_lossy(Icon::VolumeX.bytes());
        assert!(on.starts_with("<svg"), "volume-2 SVG root");
        assert!(off.starts_with("<svg"), "volume-x SVG root");
        assert_ne!(on, off, "distinct speaker glyphs");
        assert_ne!(Icon::Volume2, Icon::VolumeX);
    }

    /// BORU-SSUI-06: the "Audio" label key resolves to real runtime text
    /// (never the raw key), so the toggle row shows a real label.
    #[test]
    fn audio_label_key_resolves_to_runtime_text() {
        assert_eq!(crate::i18n::t("screenshare.audio"), "Audio");
    }

    /// BORU-SSUI-07 (PDF Task 7): the destructive Stop Sharing action row
    /// is visible in every active host state (requesting → reconnecting)
    /// and hidden only in the terminal states (Stopped / Error) that
    /// instead offer Share Again + Dismiss. This preserves the old
    /// button's reachability exactly — no state loses Stop Sharing.
    #[test]
    fn stop_action_visible_for_all_active_states() {
        for state in [
            ScreenShareHostState::Requesting,
            ScreenShareHostState::Inviting,
            ScreenShareHostState::Streaming,
            ScreenShareHostState::Paused,
            ScreenShareHostState::Reconnecting,
        ] {
            assert!(
                IcedChat::stop_action_visible(&state),
                "Stop Sharing must be reachable in {state:?}"
            );
        }
        assert!(!IcedChat::stop_action_visible(
            &ScreenShareHostState::Stopped
        ));
        assert!(!IcedChat::stop_action_visible(
            &ScreenShareHostState::Error("boom".into())
        ));
    }

    /// BORU-SSUI-07: the stop icon maps to a dedicated filled-square stop
    /// asset, distinct from the pause glyph — the destructive action never
    /// reuses a play/pause control icon.
    #[test]
    fn stop_icon_maps_to_distinct_filled_square_asset() {
        let stop = String::from_utf8_lossy(Icon::Stop.bytes());
        assert!(stop.starts_with("<svg"), "stop SVG root");
        // The filled-square stop glyph (square-fill.svg) carries a rect.
        assert!(stop.contains("rect"), "stop glyph is a square");
        let pause = String::from_utf8_lossy(Icon::Pause.bytes());
        assert_ne!(stop, pause, "stop and pause must be distinct glyphs");
        assert_ne!(Icon::Stop, Icon::Pause);
    }

    /// BORU-SSUI-07: the "Stop Sharing" label key resolves to real runtime
    /// text (never the raw key), so the destructive button shows a real
    /// label from the shared locale.
    #[test]
    fn stop_sharing_label_key_resolves_to_runtime_text() {
        assert_eq!(crate::i18n::t("screenshare.stop_sharing"), "Stop Sharing");
    }

    /// BORU-SSUI-09 (PDF Task 9): the sender control row maps each viewport
    /// tier to the correct layout mode — UltraWide = one row, Desktop =
    /// wrap into two logical groups, Narrow = stack. This is the tier→mode
    /// contract the responsive row uses, so the PDF's wide/medium/narrow
    /// acceptance criteria are pinned by this test.
    #[test]
    fn sender_control_row_layout_maps_tiers_to_modes() {
        use crate::layout::ViewportTier;
        assert_eq!(
            SenderControlRowLayout::for_tier(ViewportTier::UltraWide),
            SenderControlRowLayout::Row
        );
        assert_eq!(
            SenderControlRowLayout::for_tier(ViewportTier::Desktop),
            SenderControlRowLayout::Wrap
        );
        assert_eq!(
            SenderControlRowLayout::for_tier(ViewportTier::Narrow),
            SenderControlRowLayout::Stack
        );
    }

    /// BORU-SSUI-09 (PDF Task 9): long peer names ellipsize in the card
    /// title. The `card.title_max_chars` token bounds the peer name before
    /// it is substituted into the i18n "Sharing your screen with {name}"
    /// string, so a long name cannot blow the card width or overlap the
    /// controls. Short names stay untouched.
    #[test]
    fn sharing_with_title_ellipsizes_long_peer_name() {
        let budget = crate::theme::BoruTheme::default()
            .screen_share
            .card
            .title_max_chars as usize;
        assert_eq!(budget, 32, "title budget default");
        // Short name: untouched.
        let short = crate::presentation::truncate_with_ellipsis("Alice", budget);
        assert_eq!(short, "Alice");
        // Long name: truncated with a Unicode ellipsis and bounded.
        let long = crate::presentation::truncate_with_ellipsis(&"N".repeat(200), budget);
        assert!(long.ends_with('…'));
        assert!(long.chars().count() <= budget);
        // The i18n substitution still resolves to real runtime text.
        let title = crate::i18n::t_args("screenshare.sharing_with", &[("name", &long)]);
        assert!(title.starts_with("Sharing your screen with"));
        assert!(title.chars().count() > long.chars().count());
    }

    /// BORU-SSUI-09 (PDF Task 9): the source-card title uses the same
    /// truncate-with-ellipsis helper (window titles ellipsize gracefully),
    /// and the source-card `title_max_chars` token stays a sensible small
    /// budget so one long window title cannot make a card enormous.
    #[test]
    fn source_card_title_budget_ellipsizes_long_window_titles() {
        let theme = crate::theme::BoruTheme::default();
        let budget = theme.screen_share.source_card.title_max_chars as usize;
        assert!(budget >= 8, "source-card title budget must stay bounded");
        let long = crate::presentation::truncate_with_ellipsis(
            "This is an extremely long window title that should never be shown in full inside a source card",
            budget,
        );
        assert!(long.chars().count() <= budget);
        assert!(long.ends_with('…'));
    }

    /// BORU-SSUI-10 (PDF Task 10): a disabled source card (terminal
    /// session) renders a muted surface + muted border with NO hover or
    /// pressed feedback — an inert card is visually unmistakable and cannot
    /// be confused with an enabled one. The enabled/selected path keeps its
    /// accent treatment unchanged.
    #[test]
    fn source_card_button_style_disabled_is_muted_and_inert() {
        let theme = iced::Theme::Light;
        let card_theme = crate::theme::BoruTheme::default().screen_share.source_card;
        let disabled = IcedChat::source_card_button_style(
            &theme,
            iced::widget::button::Status::Hovered,
            false,
            false,
            card_theme,
        );
        let disabled_hover = IcedChat::source_card_button_style(
            &theme,
            iced::widget::button::Status::Hovered,
            false,
            false,
            card_theme,
        );
        // Inert: hover does not change the background at all.
        assert_eq!(disabled.background, disabled_hover.background);
        // Muted: border stays neutral (not accent) even under hover.
        assert_eq!(
            disabled.border.color,
            crate::design_tokens::border_muted(&theme)
        );
        // Contrasting the enabled hover path: enabled hover uses surface_hover.
        let enabled_hover = IcedChat::source_card_button_style(
            &theme,
            iced::widget::button::Status::Hovered,
            false,
            true,
            card_theme,
        );
        assert_ne!(disabled.background, enabled_hover.background);
        assert_eq!(
            enabled_hover.background,
            Some(iced::Background::Color(
                crate::design_tokens::surface_hover(&theme)
            ))
        );
    }

    /// BORU-SSUI-10: the selected+disabled card keeps its check glyph slot
    /// (the check icon still renders, muted) so a session that ended mid-
    /// selection never loses the selection indicator — only the colour is
    /// muted, never the secondary cue itself.
    #[test]
    fn source_card_button_style_selected_disabled_keeps_muted_treatment() {
        let theme = iced::Theme::Light;
        let card_theme = crate::theme::BoruTheme::default().screen_share.source_card;
        let style = IcedChat::source_card_button_style(
            &theme,
            iced::widget::button::Status::Active,
            true,
            false,
            card_theme,
        );
        // Disabled wins over selected: no accent border, no soft fill.
        assert_eq!(
            style.border.color,
            crate::design_tokens::border_muted(&theme)
        );
        assert_eq!(
            style.background,
            Some(iced::Background::Color(crate::design_tokens::surface(
                &theme
            )))
        );
    }

    /// BORU-SSUI-10: the source-kind tooltip keys resolve to real runtime
    /// text (never the raw key), so the ambiguous monitor/window/desktop
    /// glyphs have a human-readable name.
    #[test]
    fn source_kind_tooltip_keys_resolve_to_runtime_text() {
        assert_eq!(crate::i18n::t("screenshare.source_kind_monitor"), "Monitor");
        assert_eq!(crate::i18n::t("screenshare.source_kind_window"), "Window");
        assert_eq!(crate::i18n::t("screenshare.source_kind_desktop"), "Desktop");
        // Disabled-capability tooltip resolves too.
        assert!(
            crate::i18n::t("screenshare.session_ended").contains("session ended"),
            "{}",
            crate::i18n::t("screenshare.session_ended")
        );
    }
}

/// Layout regression: the chat timeline scrollbar must sit flush with the
/// right edge of the chat pane while the message content stays in a capped,
/// centered readable column.
///
/// Regression from BORU-RESP-04: `view_chat_panel` wrapped the whole
/// scrollable in `container(chat_log).max_width(content_max_width)`, capping
/// the scrollable VIEWPORT at 740px left-anchored in the pane — the scrollbar
/// appeared ~80% across with an empty column to its right. The fix moves the
/// cap INSIDE the scrollable onto the message content (see
/// `IcedChat::readable_chat_column`), so the viewport spans the full pane.

/// Short display form of a peer id for the chat header: the first 8 chars,
/// an ellipsis, and the last 4 chars for ids longer than 16 chars;
/// unchanged for shorter ids. Pure so it can be unit-tested in isolation.
pub(crate) fn peer_id_short_form(full_key: &str) -> String {
    if full_key.len() > 16 {
        format!("{}…{}", &full_key[..8], &full_key[full_key.len() - 4..])
    } else {
        full_key.to_string()
    }
}

/// Compute the display size of a chat image, shared by `view_chat_log`
/// (rendered box) and `LayoutCache` (height estimate) so the two never
/// diverge.  A mismatch between the cached `total_content_height` and the
/// real rendered column height is what makes the scrollbar jump and images
/// jitter as they enter the window — the cache must predict the same box
/// the view will render.
///
/// Clamps to `crate::design_tokens::IMAGE_PREVIEW_MAX_WIDTH` x `crate::design_tokens::IMAGE_PREVIEW_MAX_HEIGHT`,
/// preserving aspect ratio (scale-down only, never upscale).  Unknown
/// dimensions fall back to the full max box, which is exactly what the
/// view renders while a placeholder is shown, so the estimated height
/// stays stable across image decode / hydration.
pub(crate) fn chat_image_display_size(entry: &ChatEntry) -> (f32, f32) {
    let (orig_w, orig_h) = match (entry.image_width, entry.image_height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f32, h as f32),
        _ => (crate::design_tokens::IMAGE_PREVIEW_MAX_WIDTH, crate::design_tokens::IMAGE_PREVIEW_MAX_HEIGHT),
    };
    let scale = (crate::design_tokens::IMAGE_PREVIEW_MAX_WIDTH / orig_w)
        .min(crate::design_tokens::IMAGE_PREVIEW_MAX_HEIGHT / orig_h)
        .min(1.0);
    let display_w = (orig_w * scale).round().max(1.0);
    let display_h = (orig_h * scale).round().max(50.0);
    (display_w, display_h)
}

/// Detect whether a picked/dropped file should be treated as an inline image
/// attachment (routed through the encrypted `ExecuteImageSend` pipeline) vs.
/// a generic file (`ExecuteFileSend`).
///
/// This is the single routing rule shared by the OS file picker
/// (`AttachPressed`) and the drag-and-drop composer path
/// (`ComposerFileDropped`). GIF/WebP/BMP are images so user-uploaded
/// animation files keep flowing through the encrypted attachment pipeline
/// (KLIPY-07); MP4 and other video files are generic files.
pub(crate) fn is_attachment_image(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
        || lower.ends_with(".bmp")
}

/// Case-insensitive substring search over the conversation log. Returns the
/// indices of entries whose body or sender label contains `query`, capped at
/// 50 so the results panel stays cheap to render. Pure and unit-testable.
pub(crate) fn chat_search_matches_in(entries: &[ChatEntry], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.body.to_lowercase().contains(&query) || e.label.to_lowercase().contains(&query)
        })
        .map(|(i, _)| i)
        .take(50)
        .collect()
}

/// Format a unix-ms timestamp into a human-readable relative time string.
pub(crate) fn format_last_seen(last_seen_ms: Option<u64>) -> String {
    let Some(ms) = last_seen_ms else {
        return String::new();
    };
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let elapsed_secs = if now_ms > ms { (now_ms - ms) / 1000 } else { 0 };

    if elapsed_secs < 60 {
        if elapsed_secs <= 5 {
            "just now".to_string()
        } else {
            format!("{}s ago", elapsed_secs)
        }
    } else if elapsed_secs < 3600 {
        let mins = elapsed_secs / 60;
        format!("{}m ago", mins)
    } else if elapsed_secs < 86400 {
        let hours = elapsed_secs / 3600;
        format!("{}h ago", hours)
    } else {
        let days = elapsed_secs / 86400;
        format!("{}d ago", days)
    }
}

/// Format a Unix-millis timestamp into a message time label.
///
/// The API stores message timestamps in UTC; the UI renders them in the
/// user's local timezone before applying the usual "today / this week / older"
/// label rules.
///
/// - Today:    "12:34"
/// - This week: "Mon 12:34"
/// - Older:    "Jan 5"
pub(crate) fn format_message_time(timestamp_ms: i64) -> String {
    use chrono::{Local, TimeZone};

    let now = Local::now();
    let to_local = |ms: i64| Local.timestamp_millis_opt(ms).single();
    format_message_time_with(timestamp_ms, now, to_local)
}

pub(crate) fn format_message_time_with<Tz, F>(
    timestamp_ms: i64,
    now: chrono::DateTime<Tz>,
    mut to_local: F,
) -> String
where
    Tz: chrono::TimeZone,
    F: FnMut(i64) -> Option<chrono::DateTime<Tz>>,
{
    use chrono::{Datelike, Timelike};

    let Some(timestamp) = to_local(timestamp_ms) else {
        return String::new();
    };

    let today = now.date_naive();
    let message_day = timestamp.date_naive();
    let hour = timestamp.hour();
    let minute = timestamp.minute();

    if message_day == today {
        format!("{:02}:{:02}", hour, minute)
    } else if message_day >= today - chrono::TimeDelta::days(6) {
        format!(
            "{} {:02}:{:02}",
            timestamp.naive_local().format("%a"),
            hour,
            minute
        )
    } else {
        format!(
            "{} {}",
            timestamp.naive_local().format("%b"),
            timestamp.day()
        )
    }
}
#[cfg(test)]
mod chat_log_scrollbar_layout_tests {
    use super::*;
    use iced::advanced::layout;
    use iced::advanced::widget::{Tree, Widget};
    use iced::{Font, Pixels, Size};

    /// Lay out the exact chat-log wrapper (scrollable → readable column →
    /// message column) at a given pane width and return the scrollable
    /// viewport width and the message-column (inner capped container) bounds.
    fn measure(pane_width: f32) -> (f32, f32, f32) {
        let max_width = 740.0;
        let message_col =
            iced::widget::column![iced::widget::text("hello"), iced::widget::text("world"),]
                .width(iced::Length::Fill)
                .align_x(iced::Alignment::Start);
        let content = IcedChat::readable_chat_column(message_col, max_width);
        let mut element: iced::Element<'_, AppMessage> =
            crate::ui_components::gutter_scrollable(content)
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .into();
        let mut tree = Tree::new(element.as_widget());
        let renderer =
            iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(Font::default(), Pixels(16.0)));
        let limits = layout::Limits::new(Size::ZERO, Size::new(pane_width, 600.0));
        let node = element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits);
        let scrollable_w = node.bounds().width;
        // scrollable → outer full-width container → inner capped container.
        let outer = &node.children()[0];
        let inner = &outer.children()[0];
        (scrollable_w, inner.bounds().width, inner.bounds().x)
    }

    #[test]
    fn scrollable_viewport_spans_full_pane_width() {
        // Narrow pane (< cap): viewport = pane, content = pane, no centering.
        for (pane, expect_content) in [(500.0, 500.0)] {
            let (sb, cw, _x) = measure(pane);
            assert!(
                (sb - pane).abs() < 1.0,
                "scrollable viewport {sb:.1}px must span the full {pane:.1}px pane \
                 (scrollbar flush right)"
            );
            assert!(
                (cw - expect_content).abs() < 1.0,
                "content {cw:.1}px should match pane {pane:.1}px below the cap"
            );
        }
        // Wide pane: viewport spans the pane, content capped at 740 and centered.
        for pane in [950.0, 1280.0, 1920.0] {
            let (sb, cw, cx) = measure(pane);
            assert!(
                (sb - pane).abs() < 1.0,
                "scrollable viewport {sb:.1}px must span the full {pane:.1}px pane \
                 (scrollbar flush right)"
            );
            assert!(
                (cw - 740.0).abs() < 1.0,
                "message content {cw:.1}px must be capped at 740px at pane {pane:.1}px"
            );
            let expected_x = (pane - 740.0) / 2.0;
            assert!(
                (cx - expected_x).abs() < 1.0,
                "content x {cx:.1}px must be centered (expected {expected_x:.1}px) at pane {pane:.1}px"
            );
        }
    }
}
