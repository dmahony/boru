//! Redacted connection-details formatting and reusable dialog rendering.
//!
//! The Iced frontend assembles the current connection snapshot from several live
//! sources (relay mode, room state, discovery state, transport state, peer
//! counts, and the latest technical connection error). This module keeps the
//! formatting, redaction rules, and dialog content data-only so the UI can
//! render and copy the same sanitized text without duplicating privacy logic.
//!
//! Fields that are not available from the current frontend state should be
//! surfaced through the loading / empty / error dialog states and are reported
//! as unavailable in the support summary.

use iced::widget::{button, container, scrollable, text, text_input, Column, Row, Space};
use iced::{Alignment, Length};

use crate::app::{
    accent_primary, bg_surface, border_muted, color_error, text_muted_style, BUTTON_OUTLINE,
    SPACE_12, SPACE_2, SPACE_24, SPACE_4, SPACE_6, SPACE_8, TYPO_MD, TYPO_SM, TYPO_XL, TYPO_XS,
};

const DIALOG_WIDTH: f32 = 680.0;
const DIALOG_MAX_HEIGHT: f32 = 540.0;
const FIRST_VALUE_INPUT_ID: &str = "connection-details-first-value";

/// Copy-ready row for the advanced connection-details view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionDetailRow {
    /// Stable label shown in the UI.
    pub label: &'static str,
    /// Sanitized value shown to the user.
    pub value: String,
    /// Optional copy payload; when `None`, the row is display-only.
    pub copy_text: Option<String>,
}

impl ConnectionDetailRow {
    fn new(label: &'static str, value: String, copy_text: Option<String>) -> Self {
        Self {
            label,
            value,
            copy_text,
        }
    }
}

/// Reusable model for the advanced connection-details surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionDetailsViewModel {
    local_peer_id: String,
    relay_url: Option<String>,
    room_or_mesh_state: String,
    discovery_state: String,
    transport_state: String,
    connected_peers: usize,
    connected_peers_display: String,
    last_connection_error: Option<String>,
}

impl ConnectionDetailsViewModel {
    /// Build a redacted connection snapshot from the current frontend state.
    pub(crate) fn new(
        local_peer_id: impl Into<String>,
        relay_url: Option<String>,
        room_or_mesh_state: impl Into<String>,
        discovery_state: impl Into<String>,
        transport_state: impl Into<String>,
        connected_peers: usize,
        last_connection_error: Option<String>,
    ) -> Self {
        Self {
            local_peer_id: redact_sensitive_text(local_peer_id.into()),
            relay_url: relay_url.map(redact_sensitive_text),
            room_or_mesh_state: redact_sensitive_text(room_or_mesh_state.into()),
            discovery_state: redact_sensitive_text(discovery_state.into()),
            transport_state: redact_sensitive_text(transport_state.into()),
            connected_peers,
            connected_peers_display: connected_peers.to_string(),
            last_connection_error: last_connection_error.map(redact_sensitive_text),
        }
    }

    /// Copy-ready rows for the advanced dialog.
    pub(crate) fn rows(&self) -> Vec<ConnectionDetailRow> {
        vec![
            ConnectionDetailRow::new(
                "Local peer ID",
                self.local_peer_id.clone(),
                Some(self.local_peer_id.clone()),
            ),
            ConnectionDetailRow::new(
                "Relay URL",
                self.relay_url
                    .clone()
                    .unwrap_or_else(|| "Unavailable".to_string()),
                self.relay_url.clone(),
            ),
            ConnectionDetailRow::new("Room or mesh state", self.room_or_mesh_state.clone(), None),
            ConnectionDetailRow::new("Discovery state", self.discovery_state.clone(), None),
            ConnectionDetailRow::new("Transport state", self.transport_state.clone(), None),
            ConnectionDetailRow::new(
                "Connected peers",
                self.connected_peers_display.clone(),
                None,
            ),
            ConnectionDetailRow::new(
                "Last technical connection error",
                self.last_connection_error
                    .clone()
                    .unwrap_or_else(|| "None".to_string()),
                None,
            ),
        ]
    }

    pub(crate) fn local_peer_id(&self) -> &str {
        &self.local_peer_id
    }

    pub(crate) fn relay_url(&self) -> Option<&str> {
        self.relay_url.as_deref()
    }

    pub(crate) fn room_or_mesh_state(&self) -> &str {
        &self.room_or_mesh_state
    }

    pub(crate) fn discovery_state(&self) -> &str {
        &self.discovery_state
    }

    pub(crate) fn transport_state(&self) -> &str {
        &self.transport_state
    }

    pub(crate) fn connected_peers(&self) -> usize {
        self.connected_peers
    }

    pub(crate) fn connected_peers_display(&self) -> &str {
        &self.connected_peers_display
    }

    pub(crate) fn last_connection_error(&self) -> Option<&str> {
        self.last_connection_error.as_deref()
    }

    /// Copy-ready support summary for bug reports and diagnostics.
    pub(crate) fn support_summary(&self) -> String {
        let unavailable = self.unavailable_fields();
        let unavailable_text = if unavailable.is_empty() {
            "none".to_string()
        } else {
            unavailable.join(", ")
        };

        let last_error = self
            .last_connection_error
            .clone()
            .unwrap_or_else(|| "None".to_string());
        let relay_url = self
            .relay_url
            .clone()
            .unwrap_or_else(|| "Unavailable".to_string());

        format!(
            "Support diagnostic summary\nLocal peer ID: {}\nRelay URL: {}\nRoom or mesh state: {}\nDiscovery state: {}\nTransport state: {}\nConnected peers: {}\nLast technical connection error: {}\nUnavailable fields: {}",
            self.local_peer_id,
            relay_url,
            self.room_or_mesh_state,
            self.discovery_state,
            self.transport_state,
            self.connected_peers,
            last_error,
            unavailable_text,
        )
    }

    /// Fields that were unavailable in the current frontend snapshot.
    pub(crate) fn unavailable_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.relay_url.is_none() {
            fields.push("Relay URL");
        }
        if self.last_connection_error.is_none() {
            fields.push("Last technical connection error");
        }
        fields
    }
}

/// Dialog state for the reusable connection-details overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionDetailsDialogState {
    /// Data is still loading.
    Loading { message: String },
    /// No details are available for the current surface.
    Empty { message: String },
    /// A recoverable error occurred while building the details snapshot.
    Error { message: String },
    /// Ready-to-render redacted connection data.
    Ready(ConnectionDetailsViewModel),
}

impl ConnectionDetailsDialogState {
    pub(crate) fn loading(message: impl Into<String>) -> Self {
        Self::Loading {
            message: message.into(),
        }
    }

    pub(crate) fn empty(message: impl Into<String>) -> Self {
        Self::Empty {
            message: message.into(),
        }
    }

    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    pub(crate) fn ready(model: ConnectionDetailsViewModel) -> Self {
        Self::Ready(model)
    }

    /// Stable title shown in the dialog chrome.
    pub(crate) fn title(&self) -> &'static str {
        "Connection details"
    }

    /// Visible announcement message for loading / empty / error states.
    pub(crate) fn body_message(&self) -> Option<&str> {
        match self {
            Self::Loading { message } | Self::Empty { message } | Self::Error { message } => {
                Some(message.as_str())
            }
            Self::Ready(_) => None,
        }
    }

    /// Rows available to render in the ready state.
    pub(crate) fn rows(&self) -> Vec<ConnectionDetailRow> {
        match self {
            Self::Ready(model) => model.rows(),
            _ => Vec::new(),
        }
    }

    /// Support summary to copy in the ready state.
    pub(crate) fn support_summary(&self) -> Option<String> {
        match self {
            Self::Ready(model) => Some(model.support_summary()),
            _ => None,
        }
    }

    /// Whether the support-summary button should be shown.
    pub(crate) fn can_copy_details(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

/// Actions emitted by the reusable dialog component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionDetailsDialogAction {
    Close,
    CopyDetails,
    CopyValue { label: &'static str, value: String },
}

/// Render the advanced connection-details dialog.
pub(crate) fn view<'a, Message>(
    state: &'a ConnectionDetailsDialogState,
    announcement: Option<&'a str>,
    on_action: impl Fn(ConnectionDetailsDialogAction) -> Message + Copy + 'a,
    on_value_edit: impl Fn(String) -> Message + Copy + 'a,
) -> iced::Element<'a, Message>
where
    Message: 'a + Clone,
{
    let dialog = match state {
        ConnectionDetailsDialogState::Loading { message } => dialog_body(
            state,
            announcement,
            vec![text(message.as_str()).size(TYPO_SM).into()],
            on_action,
            on_value_edit,
        ),
        ConnectionDetailsDialogState::Empty { message } => dialog_body(
            state,
            announcement,
            vec![text(message.as_str())
                .size(TYPO_SM)
                .style(text_muted_style)
                .into()],
            on_action,
            on_value_edit,
        ),
        ConnectionDetailsDialogState::Error { message } => dialog_body(
            state,
            announcement,
            vec![text(message.as_str())
                .size(TYPO_SM)
                .style(|theme| iced::widget::text::Style {
                    color: Some(color_error(theme)),
                })
                .into()],
            on_action,
            on_value_edit,
        ),
        ConnectionDetailsDialogState::Ready(model) => dialog_body(
            state,
            announcement,
            vec![
                connection_detail_row(
                    "Local peer ID",
                    model.local_peer_id(),
                    Some(model.local_peer_id()),
                    true,
                    on_action,
                    on_value_edit,
                ),
                connection_detail_row(
                    "Relay URL",
                    model.relay_url().unwrap_or("Unavailable"),
                    model.relay_url(),
                    false,
                    on_action,
                    on_value_edit,
                ),
                connection_detail_row(
                    "Room or mesh state",
                    model.room_or_mesh_state(),
                    None,
                    false,
                    on_action,
                    on_value_edit,
                ),
                connection_detail_row(
                    "Discovery state",
                    model.discovery_state(),
                    None,
                    false,
                    on_action,
                    on_value_edit,
                ),
                connection_detail_row(
                    "Transport state",
                    model.transport_state(),
                    None,
                    false,
                    on_action,
                    on_value_edit,
                ),
                connection_detail_row(
                    "Connected peers",
                    model.connected_peers_display(),
                    None,
                    false,
                    on_action,
                    on_value_edit,
                ),
                connection_detail_row(
                    "Last technical connection error",
                    model.last_connection_error().unwrap_or("None"),
                    None,
                    false,
                    on_action,
                    on_value_edit,
                ),
            ],
            on_action,
            on_value_edit,
        ),
    };

    dialog
}

fn connection_detail_row<'a, Message>(
    label: &'static str,
    value: &'a str,
    copy_text: Option<&'a str>,
    focus_target: bool,
    on_action: impl Fn(ConnectionDetailsDialogAction) -> Message + Copy + 'a,
    on_value_edit: impl Fn(String) -> Message + Copy + 'a,
) -> iced::Element<'a, Message>
where
    Message: 'a + Clone,
{
    let label_widget = text(label)
        .size(TYPO_MD)
        .width(Length::Fill)
        .style(|theme| iced::widget::text::Style {
            color: Some(accent_primary(theme)),
        });

    let value_input = if focus_target {
        text_input("", value)
            .id(FIRST_VALUE_INPUT_ID)
            .on_input(on_value_edit)
            .padding([SPACE_6, SPACE_8])
            .width(Length::Fill)
    } else {
        text_input("", value)
            .on_input(on_value_edit)
            .padding([SPACE_6, SPACE_8])
            .width(Length::Fill)
    };

    let mut line = Row::new()
        .push(
            Column::new()
                .push(label_widget)
                .push(value_input)
                .spacing(SPACE_2)
                .width(Length::Fill)
                .align_x(Alignment::Start),
        )
        .spacing(SPACE_12)
        .align_y(Alignment::Center);

    if let Some(copy_text) = copy_text {
        line = line.push(
            button(text("Copy").size(TYPO_SM))
                .on_press(on_action(ConnectionDetailsDialogAction::CopyValue {
                    label,
                    value: copy_text.to_string(),
                }))
                .style(BUTTON_OUTLINE)
                .padding([SPACE_6, SPACE_12]),
        );
    }

    container(line)
        .width(Length::Fill)
        .padding([SPACE_2, SPACE_2])
        .into()
}

fn dialog_body<'a, Message>(
    state: &'a ConnectionDetailsDialogState,
    announcement: Option<&'a str>,
    body_rows: Vec<iced::Element<'a, Message>>,
    on_action: impl Fn(ConnectionDetailsDialogAction) -> Message + Copy + 'a,
    on_value_edit: impl Fn(String) -> Message + Copy + 'a,
) -> iced::Element<'a, Message>
where
    Message: 'a + Clone,
{
    let title = text(state.title()).size(TYPO_XL).width(Length::Fill);

    let mut header = Column::new().push(title).spacing(SPACE_4);
    if let Some(message) = announcement {
        header = header.push(
            text(message)
                .size(TYPO_XS)
                .style(text_muted_style)
                .width(Length::Fill),
        );
    }

    let mut content = Column::new().push(header).spacing(SPACE_12);

    if !body_rows.is_empty() {
        let rows = body_rows
            .into_iter()
            .fold(Column::new().spacing(SPACE_8), |col, row| col.push(row));
        content = content.push(
            scrollable(rows)
                .height(Length::Fixed(DIALOG_MAX_HEIGHT - 150.0))
                .width(Length::Fill),
        );
    }

    let mut footer = Row::new().spacing(SPACE_8).align_y(Alignment::Center);
    if state.can_copy_details() {
        footer = footer.push(
            button(text("Copy details").size(TYPO_SM))
                .on_press(on_action(ConnectionDetailsDialogAction::CopyDetails))
                .style(BUTTON_OUTLINE)
                .padding([SPACE_6, SPACE_12]),
        );
    }
    footer = footer.push(
        button(text("Close").size(TYPO_SM))
            .on_press(on_action(ConnectionDetailsDialogAction::Close))
            .style(BUTTON_OUTLINE)
            .padding([SPACE_6, SPACE_12]),
    );

    let dialog = Column::new()
        .push(content)
        .push(Space::new().height(Length::Fixed(SPACE_12)))
        .push(footer)
        .spacing(SPACE_12)
        .width(Length::Fixed(DIALOG_WIDTH))
        .align_x(Alignment::Start);

    let overlay = container(dialog)
        .width(Length::Shrink)
        .height(Length::Shrink)
        .padding(SPACE_24)
        .style(move |theme| iced::widget::container::Style {
            background: Some(iced::Background::Color(bg_surface(theme))),
            border: iced::Border {
                radius: 12.0.into(),
                width: 1.0,
                color: border_muted(theme),
            },
            ..Default::default()
        });

    container(overlay)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

/// Sanitize a copy-ready string so credentials, tokens, and query fragments do
/// not leak into the clipboard or summary output.
pub(crate) fn redact_sensitive_text(input: impl AsRef<str>) -> String {
    let trimmed = input.as_ref().trim();
    let without_url_secret = redact_url_like(trimmed);
    redact_key_value_secrets(&without_url_secret)
}

fn redact_url_like(input: &str) -> String {
    let mut value = input.trim().to_string();

    // Drop query and fragment components first so access tokens never survive
    // in support copies.
    for sep in ['?', '#'] {
        if let Some(idx) = value.find(sep) {
            value.truncate(idx);
        }
    }

    let Some(scheme_idx) = value.find("://") else {
        return value;
    };

    let authority_start = scheme_idx + 3;
    let authority_tail = &value[authority_start..];
    let authority_end = authority_tail
        .find('/')
        .map(|idx| authority_start + idx)
        .unwrap_or_else(|| value.len());
    let authority = &value[authority_start..authority_end];
    let host_only = authority.rsplit('@').next().unwrap_or(authority);

    let mut redacted = String::with_capacity(value.len());
    redacted.push_str(&value[..authority_start]);
    redacted.push_str(host_only);
    redacted.push_str(&value[authority_end..]);
    redacted
}

fn redact_key_value_secrets(input: &str) -> String {
    const MARKERS: &[&str] = &[
        "token=",
        "token:",
        "secret=",
        "secret:",
        "password=",
        "password:",
        "credential=",
        "credential:",
        "credentials=",
        "credentials:",
        "private_key=",
        "private_key:",
        "private key=",
        "private key:",
        "bearer ",
    ];

    let mut out = input.to_string();
    let lower = out.to_lowercase();
    let mut replacements: Vec<(usize, usize)> = Vec::new();

    for marker in MARKERS {
        let mut search_start = 0usize;
        while let Some(found) = lower[search_start..].find(marker) {
            let start = search_start + found;
            let value_start = start + marker.len();
            let value_end = find_redaction_end(&out, value_start);
            if value_start < value_end {
                replacements.push((value_start, value_end));
            }
            search_start = value_end.max(start + marker.len());
        }
    }

    replacements.sort_unstable_by(|left, right| right.cmp(left));
    replacements.dedup();
    for (start, end) in replacements {
        out.replace_range(start..end, "[redacted]");
    }
    out
}

fn find_redaction_end(value: &str, start: usize) -> usize {
    let mut end = value.len();
    for sep in [' ', '\t', '\n', '\r', ',', ';'] {
        if let Some(idx) = value[start..].find(sep) {
            end = end.min(start + idx);
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_includes_every_requested_field_and_unavailable_notes() {
        let details = ConnectionDetailsViewModel::new(
            "7d2a5bb3f6f0e12345abcdef67890123",
            Some("https://relay.example.test/relay".to_string()),
            "Joined room (mesh healthy)",
            "2 discovered peers",
            "1 direct, 1 relayed",
            3,
            Some("GATT_CONN_TIMEOUT after peer handshake".to_string()),
        );

        let summary = details.support_summary();
        assert!(summary.contains("Support diagnostic summary"));
        assert!(summary.contains("Local peer ID: 7d2a5bb3f6f0e12345abcdef67890123"));
        assert!(summary.contains("Relay URL: https://relay.example.test/relay"));
        assert!(summary.contains("Room or mesh state: Joined room (mesh healthy)"));
        assert!(summary.contains("Discovery state: 2 discovered peers"));
        assert!(summary.contains("Transport state: 1 direct, 1 relayed"));
        assert!(summary.contains("Connected peers: 3"));
        assert!(summary
            .contains("Last technical connection error: GATT_CONN_TIMEOUT after peer handshake"));
        assert!(summary.contains("Unavailable fields: none"));
    }

    #[test]
    fn rows_offer_copy_text_only_for_identifiers_and_addresses() {
        let details = ConnectionDetailsViewModel::new(
            "peer-id-123",
            Some(
                "https://user:secret@relay.example.test:8443/relay?access_token=abc#frag"
                    .to_string(),
            ),
            "room state",
            "discovery state",
            "transport state",
            7,
            None,
        );

        let rows = details.rows();
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0].label, "Local peer ID");
        assert_eq!(rows[0].copy_text.as_deref(), Some("peer-id-123"));
        assert_eq!(rows[1].label, "Relay URL");
        assert_eq!(
            rows[1].copy_text.as_deref(),
            Some("https://relay.example.test:8443/relay")
        );
        assert!(rows[2].copy_text.is_none());
        assert!(rows[3].copy_text.is_none());
        assert!(rows[4].copy_text.is_none());
        assert!(rows[5].copy_text.is_none());
        assert!(rows[6].copy_text.is_none());
        assert_eq!(rows[6].label, "Last technical connection error");
        assert_eq!(rows[6].value, "None");
    }

    #[test]
    fn redaction_strips_credentials_tokens_and_query_fragments() {
        let redacted = redact_sensitive_text(
            "https://alice:super-secret@relay.example.test/path?token=abcd&password=efgh#frag",
        );
        assert_eq!(redacted, "https://relay.example.test/path");
    }

    #[test]
    fn redaction_preserves_technical_error_words_without_secret_values() {
        let redacted =
            redact_sensitive_text("connection failed: private key=abcd1234, retry later");
        assert_eq!(
            redacted,
            "connection failed: private key=[redacted], retry later"
        );
    }

    #[test]
    fn dialog_state_surfaces_loading_empty_error_and_ready_data() {
        let loading = ConnectionDetailsDialogState::loading("Loading connection details…");
        let empty = ConnectionDetailsDialogState::empty("No connection details are available yet.");
        let error = ConnectionDetailsDialogState::error("Failed to gather connection details.");
        let ready = ConnectionDetailsDialogState::ready(ConnectionDetailsViewModel::new(
            "peer",
            None,
            "Mesh healthy",
            "Discovery ready",
            "Transport ready",
            1,
            None,
        ));

        assert_eq!(loading.body_message(), Some("Loading connection details…"));
        assert_eq!(
            empty.body_message(),
            Some("No connection details are available yet.")
        );
        assert_eq!(
            error.body_message(),
            Some("Failed to gather connection details.")
        );
        assert!(loading.rows().is_empty());
        assert!(empty.rows().is_empty());
        assert!(error.rows().is_empty());
        assert_eq!(ready.rows().len(), 7);
        assert!(ready.can_copy_details());
        assert!(ready
            .support_summary()
            .unwrap()
            .contains("Support diagnostic summary"));
        assert!(!loading.can_copy_details());
        assert!(!empty.can_copy_details());
        assert!(!error.can_copy_details());
    }

    #[test]
    fn unavailable_fields_reports_missing_relay_and_error() {
        let full = ConnectionDetailsViewModel::new(
            "peer",
            Some("url".to_string()),
            "state",
            "disc",
            "trans",
            1,
            Some("err".to_string()),
        );
        assert!(
            full.unavailable_fields().is_empty(),
            "all fields present should yield no unavailable"
        );

        let no_relay = ConnectionDetailsViewModel::new(
            "peer",
            None,
            "state",
            "disc",
            "trans",
            1,
            Some("err".to_string()),
        );
        assert!(no_relay.unavailable_fields().contains(&"Relay URL"));

        let no_error = ConnectionDetailsViewModel::new(
            "peer",
            Some("url".to_string()),
            "state",
            "disc",
            "trans",
            1,
            None,
        );
        assert!(no_error
            .unavailable_fields()
            .contains(&"Last technical connection error"));

        let both_missing =
            ConnectionDetailsViewModel::new("peer", None, "state", "disc", "trans", 1, None);
        assert_eq!(both_missing.unavailable_fields().len(), 2);
    }

    #[test]
    fn rows_format_with_missing_relay_and_no_error_shows_placeholders() {
        let details = ConnectionDetailsViewModel::new(
            "peer-abc",
            None,
            "Mesh idle",
            "Scanning",
            "Listening",
            0,
            None,
        );
        let rows = details.rows();
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[1].value, "Unavailable");
        assert_eq!(rows[1].copy_text, None);
        assert_eq!(rows[6].value, "None");
        assert_eq!(rows[6].copy_text, None);
    }

    #[test]
    fn support_summary_reports_unavailable_fields_when_missing() {
        let details =
            ConnectionDetailsViewModel::new("peer", None, "state", "disc", "trans", 2, None);
        let summary = details.support_summary();
        assert!(summary.contains("Unavailable fields: Relay URL, Last technical connection error"));
    }

    #[test]
    fn support_summary_correctly_lists_none_unavailable() {
        let details = ConnectionDetailsViewModel::new(
            "peer",
            Some("url".to_string()),
            "state",
            "disc",
            "trans",
            2,
            Some("err".to_string()),
        );
        let summary = details.support_summary();
        assert!(summary.contains("Unavailable fields: none"));
    }

    #[test]
    fn rows_copy_text_is_none_for_display_only_fields() {
        let details = ConnectionDetailsViewModel::new(
            "peer-42",
            Some("https://relay.test/".to_string()),
            "joined",
            "discovered",
            "direct",
            3,
            Some("timeout".to_string()),
        );
        let rows = details.rows();
        assert!(
            rows[0].copy_text.is_some(),
            "local peer ID should have copy_text"
        );
        assert!(
            rows[1].copy_text.is_some(),
            "relay URL should have copy_text"
        );
        for i in 2..=6 {
            assert!(
                rows[i].copy_text.is_none(),
                "row {} '{}' should be display-only",
                i,
                rows[i].label
            );
        }
    }

    #[test]
    fn redact_sensitive_text_strips_credentials_urls_and_key_value_secrets() {
        // URL with userinfo, query token, and fragment
        let result = redact_sensitive_text(
            "https://user:pass@relay.test/path?token=abc123&secret=xyz#section",
        );
        assert_eq!(result, "https://relay.test/path");

        // Inline key-value secrets
        assert_eq!(
            redact_sensitive_text("error: private key=deadbeef + token:secret-value"),
            "error: private key=[redacted] + token:[redacted]"
        );

        // Bearer token
        assert_eq!(
            redact_sensitive_text("Authorization: bearer eyJhbGciOiJIUzI1NiJ9"),
            "Authorization: bearer [redacted]"
        );

        // Password marker
        assert_eq!(
            redact_sensitive_text("db:password=supersecret;host=localhost"),
            "db:password=[redacted];host=localhost"
        );

        // No-op for safe input
        assert_eq!(redact_sensitive_text("Hello world"), "Hello world");
        assert_eq!(redact_sensitive_text(""), "");
    }

    #[test]
    fn redact_sensitive_text_handles_untrimmed_and_whitespace_input() {
        assert_eq!(redact_sensitive_text("  "), "");
        assert_eq!(redact_sensitive_text("\t\n"), "");
        assert_eq!(redact_sensitive_text("  plain text  "), "plain text");
    }

    #[test]
    fn dialog_body_message_is_none_when_ready() {
        let ready = ConnectionDetailsDialogState::ready(ConnectionDetailsViewModel::new(
            "peer", None, "state", "disc", "trans", 1, None,
        ));
        assert!(ready.body_message().is_none());
    }

    #[test]
    fn dialog_support_summary_and_can_copy_details_correlate_across_states() {
        let loading = ConnectionDetailsDialogState::loading("loading...");
        let empty = ConnectionDetailsDialogState::empty("empty");
        let error = ConnectionDetailsDialogState::error("error text");

        assert!(loading.support_summary().is_none());
        assert!(empty.support_summary().is_none());
        assert!(error.support_summary().is_none());
        assert!(!loading.can_copy_details());
        assert!(!empty.can_copy_details());
        assert!(!error.can_copy_details());

        let ready = ConnectionDetailsDialogState::ready(ConnectionDetailsViewModel::new(
            "peer", None, "state", "disc", "trans", 1, None,
        ));
        assert!(ready.support_summary().is_some());
        assert!(ready.can_copy_details());
    }

    #[test]
    fn dialog_title_is_consistent_across_all_state_variants() {
        let loading = ConnectionDetailsDialogState::loading("...");
        let empty = ConnectionDetailsDialogState::empty("...");
        let error = ConnectionDetailsDialogState::error("...");
        let ready = ConnectionDetailsDialogState::ready(ConnectionDetailsViewModel::new(
            "peer", None, "state", "disc", "trans", 1, None,
        ));
        assert_eq!(loading.title(), "Connection details");
        assert_eq!(empty.title(), "Connection details");
        assert_eq!(error.title(), "Connection details");
        assert_eq!(ready.title(), "Connection details");
    }
}
