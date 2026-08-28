#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::large_enum_variant,
    clippy::if_same_then_else,
    clippy::doc_lazy_continuation,
    clippy::doc_overindented_list_items,
    clippy::redundant_guards,
    clippy::manual_let_else,
    clippy::vec_init_then_push,
    clippy::let_underscore_future,
    clippy::needless_update,
    clippy::unnecessary_unwrap,
    clippy::single_match,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::question_mark,
    clippy::unnecessary_sort_by,
    clippy::result_large_err,
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::wrong_self_convention,
    missing_debug_implementations,
    unfulfilled_lint_expectations
)]
#![allow(dead_code)]

//! Tunnels domain (BORU-APP-009).
//!
//! Owns the secure-tunnel application responsibilities moved out of the
//! monolithic IcedChat shell (app.rs), following the BORU-ARCH-04 domain
//! pattern (DomainState + DomainMessage + update() + view()).
//!
//! - [`TunnelsState`] owns the tunnel UI state: the create-tunnel
//!   (friend-picker) dialog, the pending incoming tunnel request queue, the
//!   share-local-service dialog (name/port/expiry/HTTP flag/local-service
//!   scan), received secure-tunnel offers, and the shared-tunnel display
//!   metadata map.
//! - [`TunnelsMessage`] covers state-only transitions; heavier arms that
//!   need shell context (capability negotiation, `TunnelService`, whisper
//!   control channel, endpoint, notifications) stay as `AppMessage` variants
//!   dispatched to [`IcedChat::update_tunnels`], reading/writing
//!   `self.tunnels_state.*`.
//! - View builders (`view_share_local_service_dialog`,
//!   `view_local_service_suggestion_row`) render the tunnel dialogs from the
//!   domain state; the friend-picker dialog stays in `app/dialogs.rs` and
//!   the home Tunnels card stays in `app/home.rs` (view layer, read-only).
//!
//! `IcedChat` holds exactly one `tunnels_state: TunnelsState`; there is no
//! mirror of this state anywhere else (PDF §14 "same state in both modules"
//! stop condition).

use super::*;

// ── Domain types (moved from app.rs, BORU-APP-009) ──────────────

/// Stable widget ID used to focus the tunnel-name field in the share-local-service dialog.
pub(crate) const SHARE_SERVICE_NAME_INPUT: &str = "share-service-name-input";
/// Stable widget ID used to focus the tunnel-port field in the share-local-service dialog.
pub(crate) const SHARE_SERVICE_PORT_INPUT: &str = "share-service-port-input";

/// A pending incoming tunnel request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TunnelRequest {
    pub(crate) peer: PublicKey,
    pub(crate) tunnel_id: String,
    pub(crate) timestamp: i64,
}

/// Lifecycle state of a secure-tunnel offer received from a friend.
#[derive(Debug, Clone)]
pub(crate) struct ReceivedTunnelState {
    /// The verified offer payload (capability + display metadata).
    pub(crate) offer: boru_core::tunnel::TunnelOffer,
    /// Endpoint identity of the sharer.
    pub(crate) sharer: PublicKey,
    /// Display name of the sharer at offer time.
    pub(crate) sharer_label: String,
    /// Whether the user has connected a local listener for this tunnel.
    pub(crate) connected: bool,
    /// Local loopback listener address once connected (127.0.0.1:<port>).
    pub(crate) local_addr: Option<std::net::SocketAddr>,
    /// Cancellation token driving the background listener task.
    pub(crate) cancellation: Option<tokio_util::sync::CancellationToken>,
    /// Shared live connection info updated by the listener transport.
    ///
    /// None while disconnected; Some once the listener is running so the
    /// Settings → Secure Tunnels section can display the Iroh-reported route
    /// and lightweight transfer metrics.
    pub(crate) live_info: Option<std::sync::Arc<boru_core::tunnel::service::TunnelLiveInfo>>,
    /// Whether the most recent connection attempt failed.
    pub(crate) connection_failed: bool,
}

/// GUI-side display metadata for a tunnel this user is sharing.
///
/// The backend [`boru_core::tunnel::service::TunnelService`] owns lifecycle
/// state (expiry, connections, revocation); the GUI keeps the human-readable
/// service name and HTTP flag alongside so the Settings → Secure Tunnels
/// section can render a live SHARING list.
#[derive(Debug, Clone)]
pub(crate) struct SharedTunnelState {
    /// Human-readable service name chosen when sharing.
    pub(crate) service_name: String,
    /// Whether the sharer explicitly identified the service as HTTP.
    pub(crate) is_http: bool,
}

// ── Domain state (BORU-ARCH-04 pattern) ─────────────────────────

/// Tunnel UI domain state (BORU-APP-009).
///
/// Moved verbatim from `IcedChat` (app.rs) so the tunnels domain owns its
/// state. Field defaults match the old constructor initializers.
#[derive(Debug)]
pub(crate) struct TunnelsState {
    /// Whether the tunnel creation (friend-picker) dialog is shown.
    pub(crate) show_create_tunnel_dialog: bool,
    /// Port entered in the create-tunnel (friend-picker) dialog for the
    /// tunnel's local listener on the receiving side. Empty means
    /// "automatic" (ephemeral port).
    pub(crate) create_tunnel_port: String,
    /// Inline validation error for the create-tunnel port field.
    pub(crate) create_tunnel_port_error: Option<String>,
    /// Pending incoming tunnel requests, in arrival order.
    pub(crate) tunnel_requests: Vec<TunnelRequest>,
    /// Whether the "Share local service" dialog is open.
    pub(crate) share_local_service_open: bool,
    /// Whether the share-local-service submit is in flight. The tunnel is
    /// created synchronously, but the flag guards Escape/backdrop/Cancel
    /// during processing and disables the primary button.
    pub(crate) share_service_submitting: bool,
    /// Inline error shown inside the share-local-service dialog (port field).
    pub(crate) share_service_error: Option<String>,
    /// Service name entered in the share dialog.
    pub(crate) share_service_name: String,
    /// Local TCP port entered in the share dialog.
    pub(crate) share_service_port: String,
    /// Expiry duration selected in the share dialog.
    pub(crate) share_service_expiry: boru_core::tunnel::service::TunnelDuration,
    /// Combo box state for the expiry picker in the share dialog.
    pub(crate) share_expiry_combo:
        iced::widget::combo_box::State<boru_core::tunnel::service::TunnelDuration>,
    /// Whether the sharer explicitly identified the shared service as HTTP.
    /// Controls whether the receiving side displays `http://` before the
    /// loopback address — never inferred from the port or service name.
    pub(crate) share_service_is_http: bool,
    /// Locally running services discovered for the share dialog suggestion
    /// list. Empty until the first scan completes.
    pub(crate) share_service_suggestions:
        Vec<boru_core::local_service_scan::LocalServiceSuggestion>,
    /// Whether a local-service scan is currently in flight.
    pub(crate) share_service_scanning: bool,
    /// When the last scan finished, used for the ~30s reopen cache so
    /// reopening the dialog is instant.
    pub(crate) share_service_scan_cached_at: Option<std::time::Instant>,
    /// Received secure-tunnel offers, keyed by tunnel id.
    ///
    /// Populated when a friend sends a signed `ContactAction::TunnelOffer`
    /// over the whisper control channel.  Each entry tracks the local
    /// listener once the user connects, so the GUI can show the loopback
    /// address and offer Open / Copy Address / Disconnect actions.
    pub(crate) received_tunnels: HashMap<boru_core::tunnel::TunnelId, ReceivedTunnelState>,
    /// Display metadata for tunnels this user is sharing, keyed by tunnel id.
    ///
    /// Populated when the user shares a local service; removed when they stop
    /// sharing.  Lifecycle state (expiry, active connections, revocation)
    /// stays in the backend [`boru_core::tunnel::service::TunnelService`] and
    /// is read live for the Settings → Secure Tunnels section.
    pub(crate) shared_tunnels: HashMap<boru_core::tunnel::TunnelId, SharedTunnelState>,
}

/// State-only transitions for the tunnels domain (BORU-APP-009).
///
/// Routed through [`TunnelsState::update`] from the shell's
/// [`IcedChat::update_tunnels`] dispatch. Arms that need shell context
/// (capability negotiation, `TunnelService`, whisper control channel,
/// endpoint, notifications, sidebar revision) remain `AppMessage` variants
/// handled inline in `IcedChat::update_tunnels`, reading/writing
/// `self.tunnels_state.<field>`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TunnelsMessage {
    /// Show the tunnel creation (friend-picker) dialog.
    ShowCreateTunnelDialog,
    /// The tunnel port input changed in the create-tunnel dialog.
    CreateTunnelPortChanged(String),
    /// Close the tunnel creation dialog without action.
    CancelCreateTunnel,
    /// Service name changed in the share dialog.
    ShareLocalServiceNameChanged(String),
    /// Local port changed in the share dialog.
    ShareLocalServicePortChanged(String),
    /// Expiry duration selected in the share dialog.
    ShareLocalServiceExpiryChanged(boru_core::tunnel::service::TunnelDuration),
    /// Whether the shared service is explicitly identified as HTTP toggled.
    ShareLocalServiceHttpToggled(bool),
    /// Cancel the share dialog.
    CancelShareLocalService,
    /// A local-service scan finished with discovered suggestions.
    ShareLocalServiceScanDone(Vec<boru_core::local_service_scan::LocalServiceSuggestion>),
    /// The user picked a suggested local service (port) in the share dialog.
    SelectShareLocalServiceSuggestion(u16),
    /// Disconnect a connected received tunnel (cancels its listener task).
    DisconnectReceivedTunnel(boru_core::tunnel::TunnelId),
}

impl TunnelsState {
    /// Create the tunnels domain state with the same defaults the inline
    /// `app.rs` fields used.
    pub(crate) fn new() -> Self {
        Self {
            show_create_tunnel_dialog: false,
            create_tunnel_port: String::new(),
            create_tunnel_port_error: None,
            tunnel_requests: Vec::new(),
            share_local_service_open: false,
            share_service_submitting: false,
            share_service_error: None,
            share_service_name: "Development Server".to_string(),
            share_service_port: "3000".to_string(),
            share_service_expiry: boru_core::tunnel::service::TunnelDuration::OneHour,
            share_expiry_combo: iced::widget::combo_box::State::new(vec![
                boru_core::tunnel::service::TunnelDuration::TenMinutes,
                boru_core::tunnel::service::TunnelDuration::ThirtyMinutes,
                boru_core::tunnel::service::TunnelDuration::OneHour,
                boru_core::tunnel::service::TunnelDuration::EightHours,
                boru_core::tunnel::service::TunnelDuration::UntilExit,
            ]),
            share_service_is_http: true,
            share_service_suggestions: Vec::new(),
            share_service_scanning: false,
            share_service_scan_cached_at: None,
            received_tunnels: HashMap::new(),
            shared_tunnels: HashMap::new(),
        }
    }

    /// Handle a state-only tunnel transition. These arms mutate only this
    /// domain's state, so nothing is returned; the shell just routes the
    /// matching `AppMessage` variant here and returns `Task::none()`.
    pub(crate) fn update(&mut self, msg: TunnelsMessage) {
        match msg {
            TunnelsMessage::ShowCreateTunnelDialog => {
                self.show_create_tunnel_dialog = true;
                self.create_tunnel_port_error = None;
            }
            TunnelsMessage::CreateTunnelPortChanged(value) => {
                self.create_tunnel_port = value;
                self.create_tunnel_port_error = None;
            }
            TunnelsMessage::CancelCreateTunnel => {
                self.show_create_tunnel_dialog = false;
            }
            TunnelsMessage::ShareLocalServiceNameChanged(value) => {
                self.share_service_name = value;
                self.share_service_error = None;
            }
            TunnelsMessage::ShareLocalServicePortChanged(value) => {
                self.share_service_port = value;
                self.share_service_error = None;
            }
            TunnelsMessage::ShareLocalServiceExpiryChanged(value) => {
                self.share_service_expiry = value;
            }
            TunnelsMessage::ShareLocalServiceHttpToggled(value) => {
                self.share_service_is_http = value;
            }
            TunnelsMessage::CancelShareLocalService => {
                // Mid-submit: cannot dismiss until the (synchronous) tunnel
                // creation completes.
                if self.share_service_submitting {
                    return;
                }
                self.share_local_service_open = false;
                self.share_service_error = None;
            }
            TunnelsMessage::ShareLocalServiceScanDone(suggestions) => {
                self.share_service_scanning = false;
                self.share_service_suggestions = suggestions;
                self.share_service_scan_cached_at = Some(std::time::Instant::now());
            }
            TunnelsMessage::SelectShareLocalServiceSuggestion(port) => {
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
            }
            TunnelsMessage::DisconnectReceivedTunnel(tunnel_id) => {
                if let Some(state) = self.received_tunnels.get_mut(&tunnel_id) {
                    if let Some(cancellation) = state.cancellation.take() {
                        cancellation.cancel();
                    }
                    state.connected = false;
                    state.local_addr = None;
                    state.live_info = None;
                    state.connection_failed = false;
                }
            }
        }
    }
}

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

        // Tunnel Details — the service name the friend sees.
        let mut name_field = TextInput::new(
            crate::i18n::t("tunnels.name"),
            &crate::i18n::t("tunnels.development_server"),
            &self.tunnels_state.share_service_name,
            AppMessage::ShareLocalServiceNameChanged,
        )
        .id(SHARE_SERVICE_NAME_INPUT)
        .helper(crate::i18n::t("tunnels.name_helper"));
        let port_valid = self
            .tunnels_state
            .share_service_port
            .trim()
            .parse::<u16>()
            .map(|p| p != 0)
            .unwrap_or(false);
        let share_submitting = self.tunnels_state.share_service_submitting;
        if let Some(error) = &self.tunnels_state.share_service_error {
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
            &self.tunnels_state.share_service_port,
            AppMessage::ShareLocalServicePortChanged,
        )
        .id(SHARE_SERVICE_PORT_INPUT)
        .helper(crate::i18n::t("tunnels.port_helper"));
        if let Some(error) = &self.tunnels_state.share_service_error {
            port_field = port_field.error(error.clone());
        }
        if port_valid && !share_submitting {
            port_field = port_field.on_submit(AppMessage::ConfirmShareLocalService);
        }
        let target_section = FormSection::new(crate::i18n::t("tunnels.connection_target"))
            .push(form_label(&crate::i18n::t("tunnels.share_with")))
            .push(
                SelectablePeerRow::new(display_name.clone())
                    .selected(true)
                    .build(&theme),
            )
            .push(port_field.build())
            .build();

        // Local Services — discovered running services the user can pick.
        // Suggestions are convenience; manual port entry remains the primary
        // path (the port field above always works).
        let mut suggestions_section = FormSection::new(crate::i18n::t("tunnels.local_services"));
        if self.tunnels_state.share_service_scanning {
            suggestions_section =
                suggestions_section.push(helper_text(&crate::i18n::t("tunnels.scanning")));
        } else if self.tunnels_state.share_service_suggestions.is_empty() {
            suggestions_section =
                suggestions_section.push(helper_text(&crate::i18n::t("tunnels.no_local_services")));
        } else {
            for suggestion in &self.tunnels_state.share_service_suggestions {
                suggestions_section = suggestions_section
                    .push(self.view_local_service_suggestion_row(suggestion, &theme));
            }
        }
        let suggestions_section = suggestions_section.build();

        // Permissions / Options — access duration.
        let options_section = FormSection::new(crate::i18n::t("tunnels.permissions_options"))
            .push(
                SearchableSelect::new(
                    crate::i18n::t("tunnels.expires_after"),
                    &self.tunnels_state.share_expiry_combo,
                    &crate::i18n::t("tunnels.expires_after_placeholder"),
                    Some(&self.tunnels_state.share_service_expiry),
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
            // Keep the footer reachable when the local-service list grows;
            // the shared responsive helper owns the tier-specific budget.
            .scroll_body(self.dialog_body_max_height())
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
        _theme: &iced::Theme,
    ) -> iced::Element<'a, AppMessage> {
        use iced::widget::{button, container, row, text, Space};
        use iced::{Alignment, Background, Border, Color, Length};

        let port = suggestion.port;
        let is_http = suggestion.is_http;
        let label = suggestion.label.clone();
        let is_selected = self.tunnels_state.share_service_port.trim() == port.to_string();

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

    /// State-layer update for tunnels (BORU-APP-009).
    ///
    /// Handles every AppMessage variant owned by the tunnels feature: create
    /// tunnel dialog state, tunnel request accept/decline/close, and the
    /// share-local-service flow (open dialog, field changes, local service
    /// scan, confirm, share result, received-tunnel connect/disconnect/open/
    /// copy). State-only transitions are routed through
    /// [`TunnelsState::update`]; arms that need shell context (capability
    /// negotiation, `TunnelService`, whisper control channel, endpoint,
    /// notifications, sidebar revision) read/write `self.tunnels_state.*`
    /// inline. The root `update()` dispatches these variants here via
    /// combined match arms.
    pub(crate) fn update_tunnels(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::ShowCreateTunnelDialog => {
                self.tunnels_state
                    .update(TunnelsMessage::ShowCreateTunnelDialog);
                iced::Task::none()
            }
            AppMessage::CreateTunnelPortChanged(value) => {
                self.tunnels_state
                    .update(TunnelsMessage::CreateTunnelPortChanged(value));
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
                    self.notifications_state.show_toast(
                        "Tunnels unavailable — this peer's client does not support secure tunnels."
                            .to_string(),
                        160,
                    );
                    self.tunnels_state.show_create_tunnel_dialog = false;
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
                let port = self.tunnels_state.create_tunnel_port.trim();
                if !port.is_empty() {
                    match port.parse::<u16>() {
                        Ok(parsed) if parsed != 0 => {}
                        _ => {
                            self.tunnels_state.create_tunnel_port_error =
                                Some(crate::i18n::t("tunnels.invalid_port"));
                            self.notifications_state
                                .show_toast(crate::i18n::t("tunnels.invalid_port"), 160);
                            return iced::Task::none();
                        }
                    }
                }
                // Friend picked from the "Share Tunnel" dialog. Hand off to
                // the existing Share-local-service dialog for that friend,
                // which collects the loopback target + expiry and registers
                // the tunnel with the shared TunnelService on confirm.
                self.tunnels_state.show_create_tunnel_dialog = false;
                self.tunnels_state.create_tunnel_port_error = None;
                self.screen = Screen::FriendProfile(peer);
                self.friend_profile_menu_open = false;
                self.tunnels_state.share_local_service_open = true;
                self.tunnels_state.share_service_name =
                    crate::i18n::t("tunnels.development_server");
                self.tunnels_state.share_service_port = "3000".to_string();
                self.tunnels_state.share_service_expiry =
                    boru_core::tunnel::service::TunnelDuration::OneHour;
                self.tunnels_state.share_service_is_http = true;
                self.tunnels_state.share_service_submitting = false;
                self.tunnels_state.share_service_error = None;
                let scan = self.start_share_service_scan();
                // Auto-focus the first meaningful field (tunnel name).
                iced::Task::batch(vec![
                    scan,
                    iced::widget::operation::focus(SHARE_SERVICE_NAME_INPUT),
                ])
            }
            AppMessage::CancelCreateTunnel => {
                self.tunnels_state
                    .update(TunnelsMessage::CancelCreateTunnel);
                iced::Task::none()
            }
            AppMessage::TunnelRequestReceived { peer, tunnel_id } => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                // Replace any existing entry for the same tunnel id so a
                // re-sent request does not create duplicates.
                self.tunnels_state
                    .tunnel_requests
                    .retain(|req| req.tunnel_id != tunnel_id);
                self.tunnels_state.tunnel_requests.push(TunnelRequest {
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
                self.tunnels_state
                    .tunnel_requests
                    .retain(|req| req.tunnel_id != tunnel_id);
                self.requests_sidebar_revision = self.requests_sidebar_revision.wrapping_add(1);
                if let Ok(bytes) = hex::decode(&tunnel_id) {
                    if let Ok(id) = <[u8; 32]>::try_from(bytes.as_slice()) {
                        let tid = boru_core::tunnel::TunnelId(id);
                        if self.tunnels_state.received_tunnels.contains_key(&tid) {
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
                self.tunnels_state
                    .tunnel_requests
                    .retain(|req| req.tunnel_id != tunnel_id);
                self.requests_sidebar_revision = self.requests_sidebar_revision.wrapping_add(1);
                if let Ok(bytes) = hex::decode(&tunnel_id) {
                    if let Ok(id) = <[u8; 32]>::try_from(bytes.as_slice()) {
                        self.tunnels_state
                            .received_tunnels
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
                self.tunnels_state.share_local_service_open = true;
                self.tunnels_state.share_service_name =
                    crate::i18n::t("tunnels.development_server");
                self.tunnels_state.share_service_port = "3000".to_string();
                self.tunnels_state.share_service_expiry =
                    boru_core::tunnel::service::TunnelDuration::OneHour;
                self.tunnels_state.share_service_is_http = true;
                self.tunnels_state.share_service_submitting = false;
                self.tunnels_state.share_service_error = None;
                let scan = self.start_share_service_scan();
                // Auto-focus the tunnel name field.
                iced::Task::batch(vec![
                    scan,
                    iced::widget::operation::focus(SHARE_SERVICE_NAME_INPUT),
                ])
            }
            AppMessage::OpenShareVncTunnel => {
                self.friend_profile_menu_open = false;
                self.tunnels_state.share_local_service_open = true;
                self.tunnels_state.share_service_name =
                    boru_core::vnc_tunnel::SERVICE_NAME.to_string();
                self.tunnels_state.share_service_port = "5900".to_string();
                self.tunnels_state.share_service_expiry =
                    boru_core::tunnel::service::TunnelDuration::OneHour;
                self.tunnels_state.share_service_is_http = false;
                self.tunnels_state.share_service_submitting = false;
                self.tunnels_state.share_service_error =
                    Some(crate::i18n::t("tunnels.vnc_experimental"));
                self.tunnels_state.share_service_scanning = false;
                iced::widget::operation::focus(SHARE_SERVICE_PORT_INPUT)
            }
            AppMessage::ShareLocalServiceNameChanged(value) => {
                self.tunnels_state
                    .update(TunnelsMessage::ShareLocalServiceNameChanged(value));
                iced::Task::none()
            }
            AppMessage::ShareLocalServicePortChanged(value) => {
                self.tunnels_state
                    .update(TunnelsMessage::ShareLocalServicePortChanged(value));
                iced::Task::none()
            }
            AppMessage::ShareLocalServiceExpiryChanged(value) => {
                self.tunnels_state
                    .update(TunnelsMessage::ShareLocalServiceExpiryChanged(value));
                iced::Task::none()
            }
            AppMessage::ShareLocalServiceHttpToggled(value) => {
                self.tunnels_state
                    .update(TunnelsMessage::ShareLocalServiceHttpToggled(value));
                iced::Task::none()
            }
            AppMessage::CancelShareLocalService => {
                self.tunnels_state
                    .update(TunnelsMessage::CancelShareLocalService);
                iced::Task::none()
            }
            AppMessage::ShareLocalServiceScanDone(suggestions) => {
                self.tunnels_state
                    .update(TunnelsMessage::ShareLocalServiceScanDone(suggestions));
                iced::Task::none()
            }
            AppMessage::SelectShareLocalServiceSuggestion(port) => {
                self.tunnels_state
                    .update(TunnelsMessage::SelectShareLocalServiceSuggestion(port));
                iced::Task::none()
            }
            AppMessage::ConfirmShareLocalService => {
                // Guard: never re-enter while a submit is in flight.
                if self.tunnels_state.share_service_submitting {
                    return iced::Task::none();
                }
                let Screen::FriendProfile(peer) = &self.screen else {
                    self.tunnels_state.share_local_service_open = false;
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
                    self.notifications_state.show_toast(
                        "Tunnels unavailable — this peer's client does not support secure tunnels."
                            .to_string(),
                        160,
                    );
                    self.tunnels_state.share_local_service_open = false;
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
                let Ok(port) = self.tunnels_state.share_service_port.trim().parse::<u16>() else {
                    self.tunnels_state.share_service_error =
                        Some(crate::i18n::t("tunnels.invalid_local_port"));
                    self.notifications_state
                        .show_toast(crate::i18n::t("tunnels.invalid_local_port"), 120);
                    return iced::Task::none();
                };
                if port == 0 {
                    self.tunnels_state.share_service_error =
                        Some(crate::i18n::t("tunnels.invalid_local_port"));
                    self.notifications_state
                        .show_toast(crate::i18n::t("tunnels.invalid_local_port"), 120);
                    return iced::Task::none();
                }
                if self.tunnels_state.share_service_name == boru_core::vnc_tunnel::SERVICE_NAME {
                    let source = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
                    if let Err(error) = (boru_core::vnc_tunnel::VncTunnelConfig {
                        source,
                        preferred_viewer_port: None,
                    })
                    .validate()
                    {
                        self.tunnels_state.share_service_error = Some(error.to_string());
                        return iced::Task::none();
                    }
                }
                self.tunnels_state.share_service_error = None;
                // Tunnel creation is synchronous; the flag guards against
                // re-entrancy and disables dismissal while processing.
                self.tunnels_state.share_service_submitting = true;
                let service_name = self.tunnels_state.share_service_name.trim().to_string();
                let service_name = if service_name.is_empty() {
                    crate::i18n::t("tunnels.development_server")
                } else {
                    service_name
                };
                let friend_label = self.resolve_name(peer);
                let expiry = self.tunnels_state.share_service_expiry;
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
                        self.tunnels_state.share_service_submitting = false;
                        self.tunnels_state.share_local_service_open = false;
                        self.tunnels_state.share_service_error = None;
                        self.tunnels_state.shared_tunnels.insert(
                            tunnel_id,
                            SharedTunnelState {
                                service_name: service_name.clone(),
                                is_http: self.tunnels_state.share_service_is_http,
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
                            is_http: self.tunnels_state.share_service_is_http,
                            owner_endpoint_addr: self.endpoint.addr(),
                            expires_at_ms: def.expires_at_ms,
                            preferred_local_port: self
                                .tunnels_state
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
                        self.tunnels_state.share_service_submitting = false;
                        self.tunnels_state.share_service_error = Some(crate::i18n::t_args(
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
                self.notifications_state.show_toast(
                    crate::i18n::t_args(
                        "tunnels.sharing_with",
                        &[("name", &name), ("friend", &friend), ("when", &when)],
                    ),
                    160,
                );
                iced::Task::none()
            }
            AppMessage::TunnelShareFailed { message } => {
                self.notifications_state.show_toast(
                    crate::i18n::t_args("tunnels.share_failed", &[("message", &message)]),
                    160,
                );
                iced::Task::none()
            }
            AppMessage::TunnelOfferSent => {
                self.notifications_state
                    .show_toast(crate::i18n::t("tunnels.offer_sent"), 120);
                iced::Task::none()
            }
            AppMessage::TunnelOfferSendFailed { message } => {
                self.notifications_state.show_toast(
                    crate::i18n::t_args("tunnels.offer_send_failed", &[("message", &message)]),
                    160,
                );
                iced::Task::none()
            }
            AppMessage::ConnectReceivedTunnel(tunnel_id) => {
                // Look up the received offer and start a loopback listener
                // that routes through the tunnel to the sharer's service.
                let Some(state) = self.tunnels_state.received_tunnels.get(&tunnel_id) else {
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
                        let listener = match requested_port {
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
                if let Some(state) = self.tunnels_state.received_tunnels.get_mut(&tunnel_id) {
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
                        self.notifications_state.show_toast(
                            crate::i18n::t_args(
                                "tunnels.port_unavailable",
                                &[
                                    ("requested", &requested.to_string()),
                                    ("actual", &local_addr.port().to_string()),
                                ],
                            ),
                            200,
                        );
                    }
                }
                iced::Task::none()
            }
            AppMessage::ReceivedTunnelConnectFailed { tunnel_id, message } => {
                if let Some(state) = self.tunnels_state.received_tunnels.get_mut(&tunnel_id) {
                    state.connected = false;
                    state.local_addr = None;
                    state.cancellation = None;
                    state.connection_failed = true;
                }
                self.notifications_state.show_toast(
                    crate::i18n::t_args("tunnels.connect_failed", &[("message", &message)]),
                    160,
                );
                iced::Task::none()
            }
            AppMessage::DisconnectReceivedTunnel(tunnel_id) => {
                self.tunnels_state
                    .update(TunnelsMessage::DisconnectReceivedTunnel(tunnel_id));
                iced::Task::none()
            }
            AppMessage::StopSharingTunnel(tunnel_id) => {
                // Revoke the tunnel through the shared backend service; this
                // also cancels any live forwarding streams immediately.
                let name = self
                    .tunnels_state
                    .shared_tunnels
                    .get(&tunnel_id)
                    .map(|state| state.service_name.clone())
                    .unwrap_or_else(|| crate::i18n::t("tunnels.service"));
                let revoked = self
                    .tunnel_service
                    .revoke_tunnel_with_termination(tunnel_id, true);
                self.tunnels_state.shared_tunnels.remove(&tunnel_id);
                match revoked {
                    Ok(_) => {
                        self.notifications_state.show_toast(
                            crate::i18n::t_args("tunnels.stopped_sharing", &[("name", &name)]),
                            160,
                        );
                    }
                    Err(error) => {
                        self.notifications_state.show_toast(
                            crate::i18n::t_args(
                                "tunnels.stop_failed",
                                &[("name", &name), ("error", &format!("{error:?}"))],
                            ),
                            160,
                        );
                    }
                }
                iced::Task::none()
            }
            AppMessage::OpenReceivedTunnel(tunnel_id) => {
                let Some(state) = self.tunnels_state.received_tunnels.get(&tunnel_id) else {
                    return iced::Task::none();
                };
                let Some(local_addr) = state.local_addr else {
                    return iced::Task::none();
                };
                let display = tunnel_local_address(&state.offer, local_addr);
                // Only an explicitly-identified HTTP service is opened in the
                // browser; anything else has no scheme to open.
                if !state.offer.is_http {
                    self.notifications_state
                        .show_toast(crate::i18n::t("tunnels.not_http"), 160);
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
                let Some(state) = self.tunnels_state.received_tunnels.get(&tunnel_id) else {
                    return iced::Task::none();
                };
                let Some(local_addr) = state.local_addr else {
                    return iced::Task::none();
                };
                let display = tunnel_local_address(&state.offer, local_addr);
                self.notifications_state
                    .show_toast(crate::i18n::t("tunnels.address_copied"), 120);
                iced::clipboard::write(display)
            }
            // update() only dispatches the tunnels variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }

    /// Kick off an asynchronous local-service scan for the Share Local
    /// Service dialog, respecting the ~30s reopen cache. Runs off the UI
    /// thread via `iced::Task::perform`; results arrive as
    /// `AppMessage::ShareLocalServiceScanDone`.
    fn start_share_service_scan(&mut self) -> iced::Task<AppMessage> {
        // ~30s cache: reopening the dialog within the TTL reuses the last
        // scan so the suggestion list appears instantly.
        if let Some(at) = self.tunnels_state.share_service_scan_cached_at {
            if at.elapsed() < boru_core::local_service_scan::SCAN_CACHE_TTL {
                return iced::Task::none();
            }
        }
        self.tunnels_state.share_service_scanning = true;
        let own_pid = std::process::id();
        // Exclude Boru's own received-tunnel loopback listeners so the app
        // never suggests its internal tunnel listener as a shareable service.
        let excluded_ports: Vec<u16> = self
            .tunnels_state
            .received_tunnels
            .values()
            .filter_map(|s| s.local_addr.map(|a| a.port()))
            .collect();
        iced::Task::perform(
            boru_core::local_service_scan::scan_local_services(Some(own_pid), excluded_ports),
            AppMessage::ShareLocalServiceScanDone,
        )
    }

    /// Handle a signed secure-tunnel offer received over the whisper control
    /// channel.
    ///
    /// The offer is verified before it is presented: it must name a valid
    /// recipient-bound capability signed by the sender and not be expired.
    /// Rejected or expired offers are ignored rather than shown to the user.
    pub(crate) fn handle_received_tunnel_offer(
        &mut self,
        sender: PublicKey,
        offer: boru_core::tunnel::TunnelOffer,
    ) -> Option<boru_core::tunnel::TunnelId> {
        let now = now_ms().max(0) as u64;
        let valid =
            offer
                .capability
                .verify_for(&sender, &self.local_public, offer.tunnel_id, now, true);
        if let Err(error) = valid {
            info!(
                from = %sender.fmt_short(),
                ?error,
                "ignoring invalid received tunnel offer"
            );
            return None;
        }
        if offer.expires_at_ms <= now {
            info!(
                from = %sender.fmt_short(),
                "ignoring expired received tunnel offer"
            );
            return None;
        }
        let sharer_label = self.resolve_name(&sender);
        let service_name = offer.service_name.clone();
        let expiry = tunnel_expiry_label(offer.expires_at_ms);
        let tunnel_id = offer.tunnel_id;
        self.tunnels_state.received_tunnels.insert(
            tunnel_id,
            ReceivedTunnelState {
                offer,
                sharer: sender,
                sharer_label: sharer_label.clone(),
                connected: false,
                local_addr: None,
                cancellation: None,
                live_info: None,
                connection_failed: false,
            },
        );
        self.notifications_state.show_toast(
            format!("{sharer_label} shared {service_name} with you ({expiry})"),
            200,
        );
        info!(
            from = %sender.fmt_short(),
            service = %service_name,
            "received tunnel offer"
        );
        Some(tunnel_id)
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

// ── Formatting helpers (moved from app.rs, BORU-APP-009) ────────

/// Human-readable remaining tunnel lifetime, e.g. "38 minutes remaining".
pub(crate) fn tunnel_remaining_label(expires_at_ms: u64) -> String {
    let remaining = expires_at_ms.saturating_sub(now_ms() as u64);
    if remaining >= 24 * 60 * 60 * 1_000 {
        format!("{} days remaining", remaining / (24 * 60 * 60 * 1_000))
    } else if remaining >= 60 * 60 * 1_000 {
        format!("{} hours remaining", remaining / (60 * 60 * 1_000))
    } else if remaining >= 60 * 1_000 {
        format!("{} minutes remaining", remaining / (60 * 1_000))
    } else if remaining > 0 {
        "less than a minute remaining".to_string()
    } else {
        "Expired".to_string()
    }
}

/// Render a tunnel target's TCP host:port in user-friendly form, using
/// `localhost` when the target is loopback.
pub(crate) fn tunnel_target_label(host: std::net::IpAddr, port: u16) -> String {
    if host.is_loopback() {
        format!("localhost:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Format a connected received-tunnel loopback address for display.
///
/// Only an explicitly HTTP-identified service gets the `http://` scheme
/// prefix; other TCP services are shown as a bare host:port.
pub(crate) fn tunnel_local_address(
    offer: &boru_core::tunnel::TunnelOffer,
    addr: std::net::SocketAddr,
) -> String {
    if offer.is_http {
        format!("http://{addr}")
    } else {
        addr.to_string()
    }
}

/// Human-readable tunnel expiry countdown, e.g. "Expires in 42 minutes".
pub(crate) fn tunnel_expiry_label(expires_at_ms: u64) -> String {
    let remaining = expires_at_ms.saturating_sub(now_ms() as u64);
    if remaining >= 24 * 60 * 60 * 1_000 {
        format!("Expires in {} days", remaining / (24 * 60 * 60 * 1_000))
    } else if remaining >= 60 * 60 * 1_000 {
        format!("Expires in {} hours", remaining / (60 * 60 * 1_000))
    } else if remaining >= 60 * 1_000 {
        format!("Expires in {} minutes", remaining / (60 * 1_000))
    } else if remaining > 0 {
        "Expires in less than a minute".to_string()
    } else {
        "Expired".to_string()
    }
}

/// Human-readable connection route label from Iroh path data.
///
/// The backend only records what Iroh reliably reports; unknown routes map to
/// the neutral "Connected" label rather than inventing a Direct/Relay guess.
pub(crate) fn tunnel_route_label(route: boru_core::tunnel::service::TunnelRoute) -> &'static str {
    route.label()
}

/// Human-readable transfer summary for a tunnel, e.g. "12.4 MB transferred".
fn tunnel_transfer_label(info: boru_core::tunnel::service::TunnelConnectionInfo) -> Option<String> {
    let total = info.bytes_sent.saturating_add(info.bytes_received);
    if total == 0 {
        None
    } else {
        Some(format!("{} transferred", format_file_size(total)))
    }
}

/// Human-readable connection duration for a tunnel, e.g. "3m 12s".
fn tunnel_duration_label(connected_at_ms: u64) -> Option<String> {
    if connected_at_ms == 0 {
        return None;
    }
    let elapsed_ms = (now_ms() as u64).saturating_sub(connected_at_ms);
    let seconds = elapsed_ms / 1_000;
    if seconds == 0 {
        return Some("connected just now".to_string());
    }
    if seconds < 60 {
        return Some(format!("{seconds}s"));
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return Some(format!("{minutes}m {}s", seconds % 60));
    }
    let hours = minutes / 60;
    Some(format!("{hours}h {}m", minutes % 60))
}

/// Compact one-line connection info for a tunnel row, e.g.
/// "Direct · 12.4 MB transferred · 3m 12s". Metrics are only included when
/// available; the route label is always shown once a connection exists.
/// While the link is reconnecting, the label leads with "Reconnecting".
pub(crate) fn tunnel_connection_info_label(
    info: boru_core::tunnel::service::TunnelConnectionInfo,
) -> String {
    let mut parts = vec![tunnel_route_label(info.route).to_string()];
    if info.reconnecting {
        parts.insert(0, "Reconnecting".to_string());
    }
    if let Some(transfer) = tunnel_transfer_label(info) {
        parts.push(transfer);
    }
    if let Some(duration) = tunnel_duration_label(info.connected_at_ms) {
        parts.push(duration);
    }
    parts.join("  ·  ")
}

/// Human-readable tunnel status for the GUI, mapping backend states to
/// user-friendly labels: "Available", "Connecting", "Connected", "Failed",
/// "Disconnected", "Expired", "Revoked".
pub(crate) fn tunnel_status_label(
    def: &boru_core::tunnel::service::TunnelDefinition,
) -> &'static str {
    let now = now_ms().max(0) as u64;
    // Expired tunnels (past their expiry) show as Expired regardless of
    // their lifecycle state, unless they were already revoked.
    if def.status != boru_core::tunnel::service::TunnelStatus::Revoked && def.expires_at_ms <= now {
        return "Expired";
    }
    def.status.label()
}

/// Return a themed color for a tunnel status badge.
pub(crate) fn tunnel_status_color(
    theme: &iced::Theme,
    def: &boru_core::tunnel::service::TunnelDefinition,
) -> iced::Color {
    use boru_core::tunnel::service::TunnelStatus;
    let now = now_ms().max(0) as u64;
    if def.status != TunnelStatus::Revoked && def.expires_at_ms <= now {
        return text_muted(theme);
    }
    match def.status {
        TunnelStatus::Active => accent_primary(theme),
        TunnelStatus::Connecting => color_warning(theme),
        TunnelStatus::Connected => accent_green(theme),
        TunnelStatus::Revoked => text_muted(theme),
        TunnelStatus::Failed => color_error(theme),
        TunnelStatus::Disconnected => text_muted(theme),
        TunnelStatus::Reconnecting => color_warning(theme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnels_state_new_defaults_match_previous_inline_fields() {
        let state = TunnelsState::new();
        assert!(!state.show_create_tunnel_dialog);
        assert!(state.create_tunnel_port.is_empty());
        assert!(state.create_tunnel_port_error.is_none());
        assert!(state.tunnel_requests.is_empty());
        assert!(!state.share_local_service_open);
        assert!(!state.share_service_submitting);
        assert!(state.share_service_error.is_none());
        assert_eq!(state.share_service_name, "Development Server");
        assert_eq!(state.share_service_port, "3000");
        assert_eq!(
            state.share_service_expiry,
            boru_core::tunnel::service::TunnelDuration::OneHour
        );
        assert!(state.share_service_is_http);
        assert!(state.share_service_suggestions.is_empty());
        assert!(!state.share_service_scanning);
        assert!(state.share_service_scan_cached_at.is_none());
        assert!(state.received_tunnels.is_empty());
        assert!(state.shared_tunnels.is_empty());
    }

    #[test]
    fn create_tunnel_dialog_transitions_are_state_only() {
        let mut state = TunnelsState::new();
        state.update(TunnelsMessage::ShowCreateTunnelDialog);
        assert!(state.show_create_tunnel_dialog);
        state.update(TunnelsMessage::CreateTunnelPortChanged("8080".to_string()));
        assert_eq!(state.create_tunnel_port, "8080");
        state.update(TunnelsMessage::CancelCreateTunnel);
        assert!(!state.show_create_tunnel_dialog);
    }

    #[test]
    fn share_dialog_field_changes_are_state_only() {
        let mut state = TunnelsState::new();
        state.update(TunnelsMessage::ShareLocalServiceNameChanged(
            "Media".to_string(),
        ));
        assert_eq!(state.share_service_name, "Media");
        state.update(TunnelsMessage::ShareLocalServicePortChanged(
            "8443".to_string(),
        ));
        assert_eq!(state.share_service_port, "8443");
        state.update(TunnelsMessage::ShareLocalServiceExpiryChanged(
            boru_core::tunnel::service::TunnelDuration::EightHours,
        ));
        assert_eq!(
            state.share_service_expiry,
            boru_core::tunnel::service::TunnelDuration::EightHours
        );
        state.update(TunnelsMessage::ShareLocalServiceHttpToggled(false));
        assert!(!state.share_service_is_http);
        // Field edits clear the inline error (mirrors the old inline arms).
        state.share_service_error = Some("boom".to_string());
        state.update(TunnelsMessage::ShareLocalServiceNameChanged(
            "Video".to_string(),
        ));
        assert!(state.share_service_error.is_none());
    }

    #[test]
    fn cancel_share_dialog_is_noop_mid_submit_and_closes_otherwise() {
        let mut state = TunnelsState::new();
        // Mid-submit: cannot dismiss until the (synchronous) tunnel
        // creation completes.
        state.share_service_submitting = true;
        state.share_local_service_open = true;
        state.update(TunnelsMessage::CancelShareLocalService);
        assert!(state.share_local_service_open, "no-op mid-submit");

        state.share_service_submitting = false;
        state.update(TunnelsMessage::CancelShareLocalService);
        assert!(!state.share_local_service_open);
        assert!(state.share_service_error.is_none());
    }

    #[test]
    fn suggestion_selection_fills_port_name_and_http_flag() {
        let mut state = TunnelsState::new();
        state.share_service_suggestions =
            vec![boru_core::local_service_scan::LocalServiceSuggestion {
                label: "My Web App".to_string(),
                port: 5173,
                is_http: true,
            }];
        state.update(TunnelsMessage::SelectShareLocalServiceSuggestion(5173));
        assert_eq!(state.share_service_port, "5173");
        assert_eq!(state.share_service_name, "My Web App");
        assert!(state.share_service_is_http);
        assert!(state.share_service_error.is_none());
    }

    #[test]
    fn scan_done_stores_suggestions_and_cache_timestamp() {
        let mut state = TunnelsState::new();
        state.share_service_scanning = true;
        let suggestions = vec![boru_core::local_service_scan::LocalServiceSuggestion {
            label: "Dev API".to_string(),
            port: 3000,
            is_http: false,
        }];
        state.update(TunnelsMessage::ShareLocalServiceScanDone(
            suggestions.clone(),
        ));
        assert!(!state.share_service_scanning);
        assert_eq!(state.share_service_suggestions, suggestions);
        assert!(state.share_service_scan_cached_at.is_some());
    }

    #[test]
    fn disconnect_received_tunnel_clears_listener_state() {
        let mut state = TunnelsState::new();
        let id = boru_core::tunnel::TunnelId([3u8; 32]);
        let owner = iroh::SecretKey::generate();
        let sharer_key = owner.public();
        state.received_tunnels.insert(
            id,
            ReceivedTunnelState {
                offer: boru_core::tunnel::TunnelOffer {
                    tunnel_id: id,
                    capability: boru_core::tunnel::TunnelCapability::sign(
                        &owner,
                        sharer_key,
                        id,
                        0,
                        u64::MAX,
                    ),
                    service_name: "svc".to_string(),
                    is_http: true,
                    owner_endpoint_addr: iroh::EndpointAddr::new(sharer_key),
                    expires_at_ms: 0,
                    preferred_local_port: None,
                },
                sharer: sharer_key,
                sharer_label: "alice".to_string(),
                connected: true,
                local_addr: Some("127.0.0.1:8080".parse().unwrap()),
                cancellation: None,
                live_info: None,
                connection_failed: false,
            },
        );
        state.update(TunnelsMessage::DisconnectReceivedTunnel(id));
        let entry = state.received_tunnels.get(&id).unwrap();
        assert!(!entry.connected);
        assert!(entry.local_addr.is_none());
        assert!(!entry.connection_failed);
    }
}
