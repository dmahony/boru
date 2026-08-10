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

        // Tunnel Details — the service name the friend sees.
        let mut name_field = TextInput::new(
            "Tunnel name",
            "Development Server",
            &self.share_service_name,
            AppMessage::ShareLocalServiceNameChanged,
        )
        .id(SHARE_SERVICE_NAME_INPUT)
        .helper("A descriptive name so your friend knows what this service is.");
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
        let details_section = FormSection::new("Tunnel Details")
            .push(name_field.build())
            .build();

        // Connection Target — who it is shared with + the local port exposed.
        let mut port_field = TextInput::new(
            "Local port",
            "3000",
            &self.share_service_port,
            AppMessage::ShareLocalServicePortChanged,
        )
        .id(SHARE_SERVICE_PORT_INPUT)
        .helper("Port of the local service on this computer to expose.");
        if let Some(error) = &self.share_service_error {
            port_field = port_field.error(error.clone());
        }
        if port_valid && !share_submitting {
            port_field = port_field.on_submit(AppMessage::ConfirmShareLocalService);
        }
        let target_section = FormSection::new("Connection Target")
            .push(form_label("Share with"))
            .push(SelectablePeerRow::new(display_name.clone()).selected(true).build(&theme))
            .push(port_field.build())
            .build();

        // Local Services — discovered running services the user can pick.
        // Suggestions are convenience; manual port entry remains the primary
        // path (the port field above always works).
        let mut suggestions_section = FormSection::new("Local Services");
        if self.share_service_scanning {
            suggestions_section =
                suggestions_section.push(helper_text("Scanning for local services…"));
        } else if self.share_service_suggestions.is_empty() {
            suggestions_section = suggestions_section.push(helper_text(
                "No local services found. You can still enter a port above.",
            ));
        } else {
            for suggestion in &self.share_service_suggestions {
                suggestions_section = suggestions_section.push(
                    self.view_local_service_suggestion_row(suggestion, &theme),
                );
            }
        }
        let suggestions_section = suggestions_section.build();

        // Permissions / Options — access duration.
        let options_section = FormSection::new("Permissions / Options")
            .push(
                SearchableSelect::new(
                    "Expires after",
                    &self.share_expiry_combo,
                    "Expires after…",
                    Some(&self.share_service_expiry),
                    AppMessage::ShareLocalServiceExpiryChanged,
                )
                .helper("How long the tunnel stays active before it expires.")
                .build(),
            )
            .build();

        // Status / Guidance — what the tunnel does for the friend.
        let guidance_section = FormSection::new("Status / Guidance")
            .push(helper_text(&format!(
                "{display_name} will be able to connect to this local service while the tunnel is active."
            )))
            .build();

        let overlay = BoruDialog::new("Create Tunnel")
            .subtitle("Securely route traffic between peers.")
            .width(self.dialog_width(BORU_DIALOG_WIDTH_STANDARD))
            .push_body(details_section)
            .push_body(target_section)
            .push_body(suggestions_section)
            .push_body(options_section)
            .push_body(guidance_section)
            .secondary("Cancel", AppMessage::CancelShareLocalService)
            .secondary_enabled(!share_submitting)
            .primary(
                if share_submitting { "Creating…" } else { "Create Tunnel" },
                AppMessage::ConfirmShareLocalService,
            )
            .primary_enabled(port_valid && !share_submitting)
            .on_close(AppMessage::CancelShareLocalService)
            .on_backdrop(AppMessage::CancelShareLocalService)
            // TUN-UI: cap the body so the footer (Cancel / Create Tunnel)
            // stays on screen when the Local Services suggestion list grows
            // the dialog past the window height. Same value as the Create
            // Group Chat dialog.
            .scroll_body(520.0)
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
                .padding([2, 6])
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
}
