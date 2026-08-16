//! Tunnel share views.
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the Create Tunnel (share
//! local service) dialog and its discovered-local-service suggestion rows:
//! the `impl IcedChat` methods that build and render them. Reads app state
//! via `use super::*`; app.rs re-exports the pub(crate) items it still
//! references with `use tunnels::*`.

use super::*;

impl IcedChat {
    pub(crate) fn view_share_local_service_dialog<'a>(
        &'a self,
        _peer: PublicKey,
        display_name: String,
        base: iced::widget::Container<'a, AppMessage>,
    ) -> iced::Element<'a, AppMessage> {
        use crate::boru_dialog::{BoruDialog, BORU_DIALOG_WIDTH_STANDARD};
        use crate::form_components::{
            form_label, helper_text, FormSection, SearchableSelect, SelectablePeerRow, TextInput,
        };

        let theme = Self::theme_from_dark(self.dark_mode);
        // Resolve the shared responsive tier once for this dialog.  The
        // dialog body is intentionally section-stacked in the narrow tier;
        // the same tier also keeps its footer reachable by reducing the
        // scroll viewport rather than allowing a dense two-column form.
        let responsive_tier = self
            .boru_layout()
            .responsive
            .tier_for_width(self.window_width);

        // Tunnel Details — the service name the friend sees.
        let mut name_field = TextInput::new(
            crate::i18n::t("tunnels.name"),
            &crate::i18n::t("tunnels.development_server"),
            &self.share_service_name,
            AppMessage::ShareLocalServiceNameChanged,
        )
        .id(SHARE_SERVICE_NAME_INPUT)
        .helper(crate::i18n::t("tunnels.name_helper"));
        let port_valid = self
            .share_service_port
            .trim()
            .parse::<u16>()
            .map(|p| p != 0)
            .unwrap_or(false);
        let share_submitting = self.share_service_submitting;
        if let Some(error) = &self.share_service_error {
            name_field = name_field.error(error.clone());
        }
        if port_valid && !share_submitting {
            name_field = name_field.on_submit(AppMessage::ConfirmShareLocalService);
        }
        let details_section = FormSection::new(crate::i18n::t("tunnels.details"))
            .push(name_field.build())
            .build();

        // Connection Target — who it is shared with + the local port exposed.
        let mut port_field = TextInput::new(
            crate::i18n::t("tunnels.local_port"),
            "3000",
            &self.share_service_port,
            AppMessage::ShareLocalServicePortChanged,
        )
        .id(SHARE_SERVICE_PORT_INPUT)
        .helper(crate::i18n::t("tunnels.port_helper"));
        if let Some(error) = &self.share_service_error {
            port_field = port_field.error(error.clone());
        }
        if port_valid && !share_submitting {
            port_field = port_field.on_submit(AppMessage::ConfirmShareLocalService);
        }
        let target_section = FormSection::new(crate::i18n::t("tunnels.connection_target"))
            .push(form_label(&crate::i18n::t("tunnels.share_with")))
            .push(SelectablePeerRow::new(display_name.clone()).selected(true).build(&theme))
            .push(port_field.build())
            .build();

        // Local Services — discovered running services the user can pick.
        // Suggestions are convenience; manual port entry remains the primary
        // path (the port field above always works).
        let mut suggestions_section = FormSection::new(crate::i18n::t("tunnels.local_services"));
        if self.share_service_scanning {
            suggestions_section =
                suggestions_section.push(helper_text(&crate::i18n::t("tunnels.scanning")));
        } else if self.share_service_suggestions.is_empty() {
            suggestions_section = suggestions_section.push(helper_text(&crate::i18n::t(
                "tunnels.no_local_services",
            )));
        } else {
            for suggestion in &self.share_service_suggestions {
                suggestions_section = suggestions_section.push(
                    self.view_local_service_suggestion_row(suggestion, &theme),
                );
            }
        }
        let suggestions_section = suggestions_section.build();

        // Permissions / Options — access duration.
        let options_section = FormSection::new(crate::i18n::t("tunnels.permissions_options"))
            .push(
                SearchableSelect::new(
                    crate::i18n::t("tunnels.expires_after"),
                    &self.share_expiry_combo,
                    &crate::i18n::t("tunnels.expires_after_placeholder"),
                    Some(&self.share_service_expiry),
                    AppMessage::ShareLocalServiceExpiryChanged,
                )
                .helper(crate::i18n::t("tunnels.expires_after_helper"))
                .build(),
            )
            .build();

        // Status / Guidance — what the tunnel does for the friend.
        let guidance_section = FormSection::new(crate::i18n::t("tunnels.status_guidance"))
            .push(helper_text(&crate::i18n::t_args(
                "tunnels.guidance",
                &[("name", &display_name)],
            )))
            .build();

        // The dialog header/footer labels are borrowed by BoruDialog for
        // the lifetime of the built element, so they must outlive this
        // function. Resolve them once (the active locale is fixed at
        // startup) and cache them in a static.
        let labels = share_dialog_labels();
        let overlay = BoruDialog::new(labels.title)
            .subtitle(labels.subtitle)
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(details_section)
            .push_body(target_section)
            .push_body(suggestions_section)
            .push_body(options_section)
            .push_body(guidance_section)
            .secondary(labels.cancel, AppMessage::CancelShareLocalService)
            .secondary_enabled(!share_submitting)
            .primary(
                if share_submitting {
                    labels.creating
                } else {
                    labels.create
                },
                AppMessage::ConfirmShareLocalService,
            )
            .primary_enabled(port_valid && !share_submitting)
            .on_close(AppMessage::CancelShareLocalService)
            .on_backdrop(AppMessage::CancelShareLocalService)
            // TUN-UI: cap the body so the footer (Cancel / Create Tunnel)
            // stays on screen when the Local Services suggestion list grows
            // the dialog past the window height. Same value as the Create
            // Group Chat dialog.
            .scroll_body(if responsive_tier == crate::layout::ViewportTier::Narrow {
                420.0
            } else {
                520.0
            })
            .build(&theme);

        iced::widget::stack![base, overlay].into()
    }

    /// Render one discovered local service as a clickable suggestion row.
    ///
    /// Clicking the row fills the share dialog's port/name/HTTP fields via
    /// [`AppMessage::SelectShareLocalServiceSuggestion`]. The row shows the
    /// resolved label, the loopback port, and an HTTP badge when the probe
    /// answered.
    pub(crate) fn view_local_service_suggestion_row<'a>(
        &'a self,
        suggestion: &boru_core::local_service_scan::LocalServiceSuggestion,
        theme: &iced::Theme,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{button, container, row, text, Space};
        use iced::{Alignment, Background, Border, Color, Length};

        let port = suggestion.port;
        let is_http = suggestion.is_http;
        let label = suggestion.label.clone();
        let is_selected = self.share_service_port.trim() == port.to_string();

        let mut content = row![
            text(label.clone())
                .font(crate::fonts::TypeRole::Body.font())
                .size(crate::fonts::TypeRole::Body.size_px())
                .style(move |t| text::Style {
                    color: Some(crate::design_tokens::text_primary(t)),
                    ..Default::default()
                }),
            Space::new().width(Length::Fill),
            text(format!(":{port}"))
                .font(crate::fonts::TypeRole::TechnicalValue.font())
                .size(crate::fonts::TypeRole::TechnicalValue.size_px())
                .style(move |t| text::Style {
                    color: Some(crate::design_tokens::text_muted(t)),
                    ..Default::default()
                }),
        ]
        .spacing(SPACE_8)
        .align_y(Alignment::Center)
        .width(Length::Fill);

        if is_http {
            content = content.push(
                container(
                    text("HTTP")
                        .font(crate::fonts::TypeRole::Metadata.font())
                        .size(crate::fonts::TypeRole::Metadata.size_px())
                        .style(move |t| text::Style {
                            color: Some(crate::design_tokens::primary(t)),
                            ..Default::default()
                        }),
                )
                .padding([
                    crate::theme::BoruTheme::default().tunnels.chip_padding_y,
                    crate::theme::BoruTheme::default().tunnels.chip_padding_x,
                ])
                .style(move |t| container::Style {
                    background: Some(Background::Color(crate::design_tokens::primary_soft(t))),
                    border: Border {
                        radius: crate::design_tokens::RADIUS_SM.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        }

        let selected = is_selected;
        button(content)
            .on_press(AppMessage::SelectShareLocalServiceSuggestion(port))
            .padding([SPACE_6, SPACE_8])
            .width(Length::Fill)
            .style(move |t, status| iced::widget::button::Style {
                background: Some(Background::Color(if selected {
                    crate::design_tokens::surface_selected(t)
                } else {
                    match status {
                        iced::widget::button::Status::Hovered => {
                            crate::design_tokens::surface_hover(t)
                        }
                        iced::widget::button::Status::Pressed => {
                            crate::design_tokens::surface_selected(t)
                        }
                        _ => Color::TRANSPARENT,
                    }
                })),
                border: Border {
                    radius: crate::design_tokens::RADIUS_MD.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// State-layer update for tunnels (BORU-AUDIT-22 spec step 5).
    ///
    /// Handles every AppMessage variant owned by the tunnels feature: create
    /// tunnel dialog state, tunnel request accept/decline/close, and the
    /// share-local-service flow (open dialog, field changes, local service
    /// scan, confirm, share result, received-tunnel connect/disconnect/open/
    /// copy). The root `update()` dispatches these variants here via combined
    /// match arms.
    pub(crate) fn update_tunnels(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::ShowCreateTunnelDialog => {
                self.show_create_tunnel_dialog = true;
                self.create_tunnel_port_error = None;
                iced::Task::none()
            }
            AppMessage::CreateTunnelPortChanged(value) => {
                self.create_tunnel_port = value;
                self.create_tunnel_port_error = None;
                iced::Task::none()
            }
            AppMessage::CreateTunnel(peer) => {
                // BORU-CP-12 (PDF Task 4.3): a new client must not attempt
                // an unsupported operation against an old/unknown client.
                // Tunnels require a negotiated TUNNELS capability.
                if !self.feature_offered(&peer, boru_core::control_plane::features::TUNNELS) {
                    tracing::warn!(
                        peer = %peer,
                        feature = boru_core::control_plane::features::TUNNELS,
                        "tunnel creation blocked: peer does not negotiate a compatible tunnel capability"
                    );
                    self.toast_message = Some(
                        "Tunnels unavailable — this peer's client does not support secure tunnels."
                            .to_string(),
                    );
                    self.toast_counter = 160;
                    self.show_create_tunnel_dialog = false;
                    return iced::Task::none();
                }
                tracing::info!(
                    peer = %peer,
                    feature = boru_core::control_plane::features::TUNNELS,
                    negotiated_version = ?self.negotiated_feature_version(
                        &peer,
                        boru_core::control_plane::features::TUNNELS,
                    ),
                    "tunnel creation initiated"
                );
                // Validate the port chosen in the friend-picker dialog before
                // handing off to the share-local-service form. Port `0` is
                // reserved for automatic selection; out-of-range values are
                // rejected so the tunnel never silently binds an unintended
                // listener port.
                let port = self.create_tunnel_port.trim();
                if !port.is_empty() {
                    match port.parse::<u16>() {
                        Ok(parsed) if parsed != 0 => {}
                        _ => {
                            self.create_tunnel_port_error =
                                Some(crate::i18n::t("tunnels.invalid_port"));
                            self.toast_message =
                                Some(crate::i18n::t("tunnels.invalid_port"));
                            self.toast_counter = 160;
                            return iced::Task::none();
                        }
                    }
                }
                // Friend picked from the "Share Tunnel" dialog. Hand off to
                // the existing Share-local-service dialog for that friend,
                // which collects the loopback target + expiry and registers
                // the tunnel with the shared TunnelService on confirm.
                self.show_create_tunnel_dialog = false;
                self.create_tunnel_port_error = None;
                self.screen = Screen::FriendProfile(peer);
                self.friend_profile_menu_open = false;
                self.share_local_service_open = true;
                self.share_service_name = crate::i18n::t("tunnels.development_server");
                self.share_service_port = "3000".to_string();
                self.share_service_expiry = boru_core::tunnel::service::TunnelDuration::OneHour;
                self.share_service_is_http = true;
                self.share_service_submitting = false;
                self.share_service_error = None;
                let scan = self.start_share_service_scan();
                // Auto-focus the first meaningful field (tunnel name).
                iced::Task::batch(vec![
                    scan,
                    iced::widget::operation::focus(SHARE_SERVICE_NAME_INPUT),
                ])
            }
            AppMessage::CancelCreateTunnel => {
                self.show_create_tunnel_dialog = false;
                iced::Task::none()
            }
            AppMessage::TunnelRequestReceived { peer, tunnel_id } => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                // Replace any existing entry for the same tunnel id so a
                // re-sent request does not create duplicates.
                self.tunnel_requests
                    .retain(|req| req.tunnel_id != tunnel_id);
                self.tunnel_requests.push(TunnelRequest {
                    peer,
                    tunnel_id,
                    timestamp,
                });
                // Bump the revision so the lazy sidebar Requests section
                // re-renders with the new tunnel request.
                self.requests_sidebar_revision = self.requests_sidebar_revision.wrapping_add(1);
                iced::Task::none()
            }
            AppMessage::AcceptTunnelRequest(tunnel_id) => {
                // Accepting an incoming tunnel request means connecting to
                // the sharer's service: route into the existing
                // ConnectReceivedTunnel flow (binds a loopback listener
                // through the tunnel) when the received offer is present.
                self.tunnel_requests
                    .retain(|req| req.tunnel_id != tunnel_id);
                self.requests_sidebar_revision = self.requests_sidebar_revision.wrapping_add(1);
                if let Ok(bytes) = hex::decode(&tunnel_id) {
                    if let Ok(id) = <[u8; 32]>::try_from(bytes.as_slice()) {
                        let tid = boru_core::tunnel::TunnelId(id);
                        if self.received_tunnels.contains_key(&tid) {
                            return iced::Task::done(AppMessage::ConnectReceivedTunnel(tid));
                        }
                    }
                }
                self.push_system(crate::i18n::t("tunnels.request_accepted"));
                iced::Task::none()
            }
            AppMessage::DeclineTunnelRequest(tunnel_id) => {
                // Declining drops the request and the stored received offer
                // so it stops being presented in Settings → Secure Tunnels.
                self.tunnel_requests
                    .retain(|req| req.tunnel_id != tunnel_id);
                self.requests_sidebar_revision = self.requests_sidebar_revision.wrapping_add(1);
                if let Ok(bytes) = hex::decode(&tunnel_id) {
                    if let Ok(id) = <[u8; 32]>::try_from(bytes.as_slice()) {
                        self.received_tunnels
                            .remove(&boru_core::tunnel::TunnelId(id));
                    }
                }
                self.push_system(crate::i18n::t("tunnels.request_declined"));
                iced::Task::none()
            }

            AppMessage::CloseTunnel(tunnel_id) => {
                let _ = self.tunnel_service.revoke_tunnel(tunnel_id);
                self.push_system(crate::i18n::t("tunnels.closed"));
                iced::Task::none()
            }

            AppMessage::OpenShareLocalService => {
                self.friend_profile_menu_open = false;
                self.share_local_service_open = true;
                self.share_service_name = crate::i18n::t("tunnels.development_server");
                self.share_service_port = "3000".to_string();
                self.share_service_expiry = boru_core::tunnel::service::TunnelDuration::OneHour;
                self.share_service_is_http = true;
                self.share_service_submitting = false;
                self.share_service_error = None;
                let scan = self.start_share_service_scan();
                // Auto-focus the tunnel name field.
                iced::Task::batch(vec![
                    scan,
                    iced::widget::operation::focus(SHARE_SERVICE_NAME_INPUT),
                ])
            }
            AppMessage::OpenShareVncTunnel => {
                self.friend_profile_menu_open = false;
                self.share_local_service_open = true;
                self.share_service_name = boru_core::vnc_tunnel::SERVICE_NAME.to_string();
                self.share_service_port = "5900".to_string();
                self.share_service_expiry = boru_core::tunnel::service::TunnelDuration::OneHour;
                self.share_service_is_http = false;
                self.share_service_submitting = false;
                self.share_service_error =
                    Some(crate::i18n::t("tunnels.vnc_experimental"));
                self.share_service_scanning = false;
                iced::widget::operation::focus(SHARE_SERVICE_PORT_INPUT)
            }
            AppMessage::ShareLocalServiceNameChanged(value) => {
                self.share_service_name = value;
                self.share_service_error = None;
                iced::Task::none()
            }
            AppMessage::ShareLocalServicePortChanged(value) => {
                self.share_service_port = value;
                self.share_service_error = None;
                iced::Task::none()
            }
            AppMessage::ShareLocalServiceExpiryChanged(value) => {
                self.share_service_expiry = value;
                iced::Task::none()
            }
            AppMessage::ShareLocalServiceHttpToggled(value) => {
                self.share_service_is_http = value;
                iced::Task::none()
            }
            AppMessage::CancelShareLocalService => {
                // Mid-submit: cannot dismiss until the (synchronous) tunnel
                // creation completes.
                if self.share_service_submitting {
                    return iced::Task::none();
                }
                self.share_local_service_open = false;
                self.share_service_error = None;
                iced::Task::none()
            }
            AppMessage::ShareLocalServiceScanDone(suggestions) => {
                self.share_service_scanning = false;
                self.share_service_suggestions = suggestions;
                self.share_service_scan_cached_at = Some(std::time::Instant::now());
                iced::Task::none()
            }
            AppMessage::SelectShareLocalServiceSuggestion(port) => {
                if let Some(suggestion) = self
                    .share_service_suggestions
                    .iter()
                    .find(|s| s.port == port)
                {
                    self.share_service_port = port.to_string();
                    self.share_service_name = suggestion.label.clone();
                    self.share_service_is_http = suggestion.is_http;
                    self.share_service_error = None;
                }
                iced::Task::none()
            }
            AppMessage::ConfirmShareLocalService => {
                // Guard: never re-enter while a submit is in flight.
                if self.share_service_submitting {
                    return iced::Task::none();
                }
                let Screen::FriendProfile(peer) = &self.screen else {
                    self.share_local_service_open = false;
                    return iced::Task::none();
                };
                // BORU-CP-12 (PDF Task 4.3) enforcement point: the tunnel
                // is only actually created when the peer negotiates a
                // compatible TUNNELS capability. This is the authoritative
                // check (guards programmatic/MCP paths that bypass the
                // friend-picker dialog).
                if !self.feature_offered(peer, boru_core::control_plane::features::TUNNELS) {
                    tracing::warn!(
                        peer = %peer,
                        feature = boru_core::control_plane::features::TUNNELS,
                        "tunnel creation blocked at confirm: peer does not negotiate a compatible tunnel capability"
                    );
                    self.toast_message = Some(
                        "Tunnels unavailable — this peer's client does not support secure tunnels."
                            .to_string(),
                    );
                    self.toast_counter = 160;
                    self.share_local_service_open = false;
                    return iced::Task::none();
                }
                tracing::info!(
                    peer = %peer,
                    feature = boru_core::control_plane::features::TUNNELS,
                    negotiated_version = ?self.negotiated_feature_version(
                        peer,
                        boru_core::control_plane::features::TUNNELS,
                    ),
                    "tunnel created (negotiated)"
                );
                // Validate the local port; keep the dialog open and show the
                // error inline under the port field.
                let Ok(port) = self.share_service_port.trim().parse::<u16>() else {
                    self.share_service_error =
                        Some(crate::i18n::t("tunnels.invalid_local_port"));
                    self.toast_message =
                        Some(crate::i18n::t("tunnels.invalid_local_port"));
                    self.toast_counter = 120;
                    return iced::Task::none();
                };
                if port == 0 {
                    self.share_service_error =
                        Some(crate::i18n::t("tunnels.invalid_local_port"));
                    self.toast_message =
                        Some(crate::i18n::t("tunnels.invalid_local_port"));
                    self.toast_counter = 120;
                    return iced::Task::none();
                }
                if self.share_service_name == boru_core::vnc_tunnel::SERVICE_NAME {
                    let source = std::net::SocketAddr::from((
                        std::net::Ipv4Addr::LOCALHOST,
                        port,
                    ));
                    if let Err(error) = (boru_core::vnc_tunnel::VncTunnelConfig {
                        source,
                        preferred_viewer_port: None,
                    })
                    .validate()
                    {
                        self.share_service_error = Some(error.to_string());
                        return iced::Task::none();
                    }
                }
                self.share_service_error = None;
                // Tunnel creation is synchronous; the flag guards against
                // re-entrancy and disables dismissal while processing.
                self.share_service_submitting = true;
                let service_name = self.share_service_name.trim().to_string();
                let service_name = if service_name.is_empty() {
                    crate::i18n::t("tunnels.development_server")
                } else {
                    service_name
                };
                let friend_label = self.resolve_name(peer);
                let expiry = self.share_service_expiry;
                let tunnel_id = boru_core::tunnel::TunnelId(rand::random());
                let target = boru_core::tunnel::service::TunnelTarget::tcp(
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                    port,
                );
                let result = self.tunnel_service.create_tunnel_for_duration(
                    tunnel_id,
                    self.local_public,
                    target,
                    *peer,
                    expiry,
                );
                match result {
                    Ok(def) => {
                        self.share_service_submitting = false;
                        self.share_local_service_open = false;
                        self.share_service_error = None;
                        self.shared_tunnels.insert(
                            tunnel_id,
                            SharedTunnelState {
                                service_name: service_name.clone(),
                                is_http: self.share_service_is_http,
                            },
                        );
                        let offer = boru_core::tunnel::TunnelOffer {
                            tunnel_id,
                            capability: boru_core::tunnel::TunnelCapability::sign(
                                &self.secret_key,
                                *peer,
                                tunnel_id,
                                def.created_at_ms,
                                def.expires_at_ms,
                            ),
                            service_name: service_name.clone(),
                            is_http: self.share_service_is_http,
                            owner_endpoint_addr: self.endpoint.addr(),
                            expires_at_ms: def.expires_at_ms,
                            preferred_local_port: self
                                .create_tunnel_port
                                .trim()
                                .parse::<u16>()
                                .ok()
                                .filter(|&p| p != 0),
                        };
                        // Dispatch the offer over the authenticated whisper
                        // control channel so the friend's GUI can display it.
                        let peer_key = *peer;
                        let whisper_handle = self.whisper_handle.clone();
                        let secret_key = self.secret_key.clone();
                        let send_task = iced::Task::perform(
                            async move {
                                let action =
                                    boru_core::contact::ContactAction::TunnelOffer { offer };
                                let payload = boru_core::contact::SignedContactMessage::sign(
                                    &secret_key,
                                    &action,
                                );
                                match payload {
                                    Ok(payload) => whisper_handle
                                        .send_control(peer_key, payload.into())
                                        .await
                                        .map_err(|e| e.to_string()),
                                    Err(err) => Err(err.to_string()),
                                }
                            },
                            |result| match result {
                                Ok(()) => AppMessage::TunnelOfferSent,
                                Err(message) => AppMessage::TunnelOfferSendFailed { message },
                            },
                        );
                        iced::Task::batch(vec![
                            iced::Task::done(AppMessage::TunnelShared {
                                name: service_name,
                                friend: friend_label,
                                expires_at_ms: def.expires_at_ms,
                            }),
                            send_task,
                        ])
                    }
                    Err(err) => {
                        self.share_service_submitting = false;
                        self.share_service_error = Some(crate::i18n::t_args(
                            "tunnels.create_failed",
                            &[("error", &format!("{err:?}"))],
                        ));
                        iced::Task::done(AppMessage::TunnelShareFailed {
                            message: format!("{err:?}"),
                        })
                    }
                }
            }
            AppMessage::TunnelShared {
                name,
                friend,
                expires_at_ms,
            } => {
                let remaining = expires_at_ms.saturating_sub(now_ms() as u64);
                let when = if remaining >= 24 * 60 * 60 * 1_000 {
                    crate::i18n::t_args(
                        "tunnels.days",
                        &[("count", &(remaining / (24 * 60 * 60 * 1_000)).to_string())],
                    )
                } else if remaining >= 60 * 60 * 1_000 {
                    crate::i18n::t_args(
                        "tunnels.hours",
                        &[("count", &(remaining / (60 * 60 * 1_000)).to_string())],
                    )
                } else if remaining >= 60 * 1_000 {
                    crate::i18n::t_args(
                        "tunnels.minutes",
                        &[("count", &(remaining / (60 * 1_000)).to_string())],
                    )
                } else {
                    crate::i18n::t("tunnels.less_than_minute")
                };
                self.toast_message = Some(crate::i18n::t_args(
                    "tunnels.sharing_with",
                    &[("name", &name), ("friend", &friend), ("when", &when)],
                ));
                self.toast_counter = 160;
                iced::Task::none()
            }
            AppMessage::TunnelShareFailed { message } => {
                self.toast_message = Some(crate::i18n::t_args(
                    "tunnels.share_failed",
                    &[("message", &message)],
                ));
                self.toast_counter = 160;
                iced::Task::none()
            }
            AppMessage::TunnelOfferSent => {
                self.toast_message = Some(crate::i18n::t("tunnels.offer_sent"));
                self.toast_counter = 120;
                iced::Task::none()
            }
            AppMessage::TunnelOfferSendFailed { message } => {
                self.toast_message = Some(crate::i18n::t_args(
                    "tunnels.offer_send_failed",
                    &[("message", &message)],
                ));
                self.toast_counter = 160;
                iced::Task::none()
            }
            AppMessage::ConnectReceivedTunnel(tunnel_id) => {
                // Look up the received offer and start a loopback listener
                // that routes through the tunnel to the sharer's service.
                let Some(state) = self.received_tunnels.get(&tunnel_id) else {
                    return iced::Task::none();
                };
                if state.connected {
                    return iced::Task::none();
                }
                let offer = state.offer.clone();
                let endpoint = self.endpoint.clone();
                let requested_port = offer.preferred_local_port.filter(|&p| p != 0);
                iced::Task::perform(
                    async move {
                        // Bind the sharer's preferred loopback port when one
                        // was chosen; fall back to an ephemeral port with a
                        // clear message when the requested port is already in
                        // use on this machine.
                        let listener =
                            match requested_port {
                                Some(port) => {
                                    match boru_core::tunnel::LocalTunnelListener::bind_loopback(
                                        endpoint.clone(),
                                        offer.owner_endpoint_addr.clone(),
                                        offer.tunnel_id,
                                        offer.capability.clone(),
                                        port,
                                    )
                                    .await
                                    {
                                        Ok(listener) => listener,
                                        Err(_) => {
                                            boru_core::tunnel::LocalTunnelListener::bind_loopback(
                                                endpoint,
                                                offer.owner_endpoint_addr,
                                                offer.tunnel_id,
                                                offer.capability,
                                                0,
                                            )
                                            .await?
                                        }
                                    }
                                }
                                None => {
                                    boru_core::tunnel::LocalTunnelListener::bind_loopback(
                                        endpoint,
                                        offer.owner_endpoint_addr,
                                        offer.tunnel_id,
                                        offer.capability,
                                        0,
                                    )
                                    .await?
                                }
                            };
                        let local_addr = listener.local_addr()?;
                        let live_info = listener.live_info();
                        let cancellation = tokio_util::sync::CancellationToken::new();
                        let run_cancellation = cancellation.clone();
                        tokio::spawn(async move {
                            let _ = listener.run(run_cancellation).await;
                        });
                        Ok::<_, anyhow::Error>((local_addr, cancellation, live_info))
                    },
                    move |result| match result {
                        Ok((local_addr, cancellation, live_info)) => {
                            AppMessage::ReceivedTunnelConnected {
                                tunnel_id,
                                local_addr,
                                cancellation,
                                live_info,
                                requested_port,
                            }
                        }
                        Err(error) => AppMessage::ReceivedTunnelConnectFailed {
                            tunnel_id,
                            message: format!("{error:#}"),
                        },
                    },
                )
            }
            AppMessage::ReceivedTunnelConnected {
                tunnel_id,
                local_addr,
                cancellation,
                live_info,
                requested_port,
            } => {
                if let Some(state) = self.received_tunnels.get_mut(&tunnel_id) {
                    state.connected = true;
                    state.local_addr = Some(local_addr);
                    state.cancellation = Some(cancellation);
                    state.live_info = Some(live_info);
                    state.connection_failed = false;
                }
                // A requested port that could not be bound falls back to an
                // ephemeral port; surface the actual address so the user is
                // not left pointing at a port the tunnel does not use.
                if let Some(requested) = requested_port {
                    if requested != local_addr.port() {
                        self.toast_message = Some(crate::i18n::t_args(
                            "tunnels.port_unavailable",
                            &[
                                ("requested", &requested.to_string()),
                                ("actual", &local_addr.port().to_string()),
                            ],
                        ));
                        self.toast_counter = 200;
                    }
                }
                iced::Task::none()
            }
            AppMessage::ReceivedTunnelConnectFailed { tunnel_id, message } => {
                if let Some(state) = self.received_tunnels.get_mut(&tunnel_id) {
                    state.connected = false;
                    state.local_addr = None;
                    state.cancellation = None;
                    state.connection_failed = true;
                }
                self.toast_message = Some(crate::i18n::t_args(
                    "tunnels.connect_failed",
                    &[("message", &message)],
                ));
                self.toast_counter = 160;
                iced::Task::none()
            }
            AppMessage::DisconnectReceivedTunnel(tunnel_id) => {
                if let Some(state) = self.received_tunnels.get_mut(&tunnel_id) {
                    if let Some(cancellation) = state.cancellation.take() {
                        cancellation.cancel();
                    }
                    state.connected = false;
                    state.local_addr = None;
                    state.live_info = None;
                    state.connection_failed = false;
                }
                iced::Task::none()
            }
            AppMessage::StopSharingTunnel(tunnel_id) => {
                // Revoke the tunnel through the shared backend service; this
                // also cancels any live forwarding streams immediately.
                let name = self
                    .shared_tunnels
                    .get(&tunnel_id)
                    .map(|state| state.service_name.clone())
                    .unwrap_or_else(|| crate::i18n::t("tunnels.service"));
                let revoked = self
                    .tunnel_service
                    .revoke_tunnel_with_termination(tunnel_id, true);
                self.shared_tunnels.remove(&tunnel_id);
                match revoked {
                    Ok(_) => {
                        self.toast_message = Some(crate::i18n::t_args(
                            "tunnels.stopped_sharing",
                            &[("name", &name)],
                        ));
                        self.toast_counter = 160;
                    }
                    Err(error) => {
                        self.toast_message = Some(crate::i18n::t_args(
                            "tunnels.stop_failed",
                            &[("name", &name), ("error", &format!("{error:?}"))],
                        ));
                        self.toast_counter = 160;
                    }
                }
                iced::Task::none()
            }
            AppMessage::OpenReceivedTunnel(tunnel_id) => {
                let Some(state) = self.received_tunnels.get(&tunnel_id) else {
                    return iced::Task::none();
                };
                let Some(local_addr) = state.local_addr else {
                    return iced::Task::none();
                };
                let display = tunnel_local_address(&state.offer, local_addr);
                // Only an explicitly-identified HTTP service is opened in the
                // browser; anything else has no scheme to open.
                if !state.offer.is_http {
                    self.toast_message =
                        Some(crate::i18n::t("tunnels.not_http"));
                    self.toast_counter = 160;
                    return iced::Task::none();
                }
                let url = display.clone();
                iced::Task::perform(
                    async move {
                        let result = open::that(&url);
                        if let Err(e) = result {
                            tracing::warn!(url = %url, error = %e, "failed to open tunnel address");
                        }
                    },
                    |_| AppMessage::Noop,
                )
            }
            AppMessage::CopyReceivedTunnelAddress(tunnel_id) => {
                let Some(state) = self.received_tunnels.get(&tunnel_id) else {
                    return iced::Task::none();
                };
                let Some(local_addr) = state.local_addr else {
                    return iced::Task::none();
                };
                let display = tunnel_local_address(&state.offer, local_addr);
                self.toast_message = Some(crate::i18n::t("tunnels.address_copied"));
                self.toast_counter = 120;
                return iced::clipboard::write(display);
            }
            // update() only dispatches the tunnels variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}

/// Translated labels for the share-local-service dialog.
///
/// `BoruDialog` borrows `&'a str` labels for the lifetime of the built
/// element, so they must outlive the view function that constructs the
/// dialog. The active locale is fixed at startup, so resolving the labels
/// once and caching them in a static is safe and adds no per-frame
/// allocation.
struct ShareDialogLabels {
    title: &'static str,
    subtitle: &'static str,
    cancel: &'static str,
    creating: &'static str,
    create: &'static str,
}

fn share_dialog_labels() -> &'static ShareDialogLabels {
    use std::sync::OnceLock;
    static LABELS: OnceLock<ShareDialogLabels> = OnceLock::new();
    LABELS.get_or_init(|| ShareDialogLabels {
        title: Box::leak(crate::i18n::t("tunnels.create").into_boxed_str()),
        subtitle: Box::leak(crate::i18n::t("tunnels.subtitle").into_boxed_str()),
        cancel: Box::leak(crate::i18n::t("common.cancel").into_boxed_str()),
        creating: Box::leak(crate::i18n::t("tunnels.creating").into_boxed_str()),
        create: Box::leak(crate::i18n::t("tunnels.create").into_boxed_str()),
    })
}
