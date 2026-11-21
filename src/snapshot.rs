//! Canonical read-only application snapshot schema for frontend adapters.
//!
//! A snapshot is a projection for display, never an authority or write model.
//! Frontend state must not become authoritative. Constructing, serializing, or
//! consuming these DTOs does not change networking, cryptography, identity, or
//! stored user data. Mutations must continue through the existing services.

use serde::{Deserialize, Serialize};

/// Current version of the serialized [`AppSnapshot`] schema.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Complete read-only projection of application state at a point in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSnapshot {
    /// Version of this JSON contract; readers must reject unsupported versions.
    pub schema_version: u32,
    /// Current local identity, if one is available.
    #[serde(default)]
    pub identity: Option<IdentitySnapshot>,
    /// Selected profile, if one is active.
    #[serde(default)]
    pub active_profile: Option<ProfileSnapshot>,
    /// Known rooms and group conversations.
    #[serde(default)]
    pub rooms: Vec<RoomSnapshot>,
    /// Direct-message conversations.
    #[serde(default)]
    pub direct_messages: Vec<DirectMessageSnapshot>,
    /// Unread state by conversation identifier.
    #[serde(default)]
    pub unread: Vec<UnreadSnapshot>,
    /// Recent user-visible activity.
    #[serde(default)]
    pub activity: Vec<ActivitySnapshot>,
    /// Current network health summary, if known.
    #[serde(default)]
    pub network_health: Option<NetworkHealthSnapshot>,
    /// Current transfer summaries.
    #[serde(default)]
    pub transfers: Vec<TransferSummary>,
}

/// Stable identity information suitable for presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitySnapshot {
    /// Stable public identifier; secret key material is never part of this DTO.
    pub id: String,
    /// Display name, if configured.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A user profile suitable for read-only presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSnapshot {
    /// Stable profile identifier.
    pub id: String,
    /// Profile display name.
    pub display_name: String,
    /// Optional user-facing description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Summary of a room conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomSnapshot {
    /// Stable conversation identifier.
    pub id: String,
    /// Room name.
    pub name: String,
    /// Optional topic or description.
    #[serde(default)]
    pub topic: Option<String>,
    /// Number of currently connected participants, when known.
    #[serde(default)]
    pub participant_count: Option<u32>,
    /// Most recent activity time in Unix milliseconds.
    #[serde(default)]
    pub last_activity_at_ms: Option<u64>,
}

/// Summary of a direct-message conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectMessageSnapshot {
    /// Stable conversation identifier.
    pub id: String,
    /// Stable peer identifier.
    pub peer_id: String,
    /// Display label for the peer.
    pub peer_name: String,
    /// Most recent message preview, if available.
    #[serde(default)]
    pub last_message_preview: Option<String>,
    /// Most recent activity time in Unix milliseconds.
    #[serde(default)]
    pub last_activity_at_ms: Option<u64>,
}

/// Unread count for one conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnreadSnapshot {
    /// Stable conversation identifier.
    pub conversation_id: String,
    /// Number of unread messages.
    pub count: u32,
}

/// A recent activity item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivitySnapshot {
    /// Stable activity identifier.
    pub id: String,
    /// Machine-readable activity kind.
    pub kind: String,
    /// Human-readable summary.
    pub summary: String,
    /// Event time in Unix milliseconds.
    pub occurred_at_ms: u64,
    /// Related conversation, if any.
    #[serde(default)]
    pub conversation_id: Option<String>,
}

/// Read-only summary of connectivity and relay health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkHealthSnapshot {
    /// Whether the local endpoint is ready for use.
    pub ready: bool,
    /// Human-readable connection state.
    pub status: String,
    /// Number of currently connected peers, if known.
    #[serde(default)]
    pub connected_peer_count: Option<u32>,
    /// Active relay URL, if applicable and known.
    #[serde(default)]
    pub relay_url: Option<String>,
}

/// Read-only summary of a file transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferSummary {
    /// Stable transfer identifier.
    pub id: String,
    /// Transfer direction (`upload` or `download`).
    pub direction: String,
    /// Human-readable file name.
    pub file_name: String,
    /// Current transfer state.
    pub state: String,
    /// Bytes transferred so far.
    pub transferred_bytes: u64,
    /// Total bytes, if known.
    #[serde(default)]
    pub total_bytes: Option<u64>,
}

/// Cursor opaque to consumers; only the service may interpret its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageHistoryCursor(String);

impl MessageHistoryCursor {
    /// Wrap an opaque service-issued cursor.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Borrow the encoded cursor for forwarding to the service.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One safe message-history item; cryptographic envelopes and local paths are excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageHistoryItem {
    /// Stable message identifier.
    pub id: String,
    /// Conversation identifier.
    pub conversation_id: String,
    /// Sender identifier.
    pub sender_id: String,
    /// Message time in Unix milliseconds.
    pub timestamp_ms: u64,
    /// Machine-readable content kind.
    pub kind: String,
    /// User-visible content.
    pub body: String,
}

/// A bounded page of chronological message history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageHistoryPage {
    /// Version of the frontend IPC contract.
    pub schema_version: u32,
    /// Messages in chronological order.
    #[serde(default)]
    pub items: Vec<MessageHistoryItem>,
    /// Cursor for the next page; absent when history is exhausted.
    #[serde(default)]
    pub next_cursor: Option<MessageHistoryCursor>,
}

/// Contact status included in frontend projections (not a storage model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactStatusDto {
    /// A request has been sent or received but not accepted.
    Pending,
    /// The contact relationship is active.
    Accepted,
    /// The contact is blocked.
    Blocked,
}

/// Contact safe for presentation. No address book secrets or keys are included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactDto {
    /// Stable public contact identifier.
    pub id: String,
    /// User-facing contact label.
    pub display_name: String,
    /// Relationship state.
    pub status: ContactStatusDto,
}

/// Delivery state is an open string at the TypeScript boundary; consumers
/// should render an unknown future value as a neutral state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatusDto {
    /// Accepted locally and awaiting transport.
    Queued,
    /// Written to transport.
    Sent,
    /// A remote peer acknowledged receipt.
    Delivered,
    /// A remote peer marked the message read.
    Seen,
    /// Delivery failed terminally.
    Failed,
}

/// Opaque, runtime-authorized reference for binary content; never a path or bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BinaryHandleDto(String);

impl BinaryHandleDto {
    /// Wrap a service-issued opaque handle.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Borrow the opaque handle for forwarding to an authorized service.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Message item extended with delivery and optional opaque attachment reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDto {
    /// Stable message identifier.
    pub id: String,
    /// Conversation identifier.
    pub conversation_id: String,
    /// Sender identifier.
    pub sender_id: String,
    /// Message time in Unix milliseconds.
    pub timestamp_ms: u64,
    /// Machine-readable content kind.
    pub kind: String,
    /// User-visible content.
    pub body: String,
    /// Current delivery status, absent for incoming messages where not tracked.
    #[serde(default)]
    pub delivery_status: Option<DeliveryStatusDto>,
    /// Optional opaque asset handle; never raw bytes or a filesystem path.
    #[serde(default)]
    pub attachment_handle: Option<BinaryHandleDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_json_fixture_round_trips() {
        let fixture = r#"{
            "schema_version": 1,
            "identity": {"id":"peer-a","display_name":"Alice"},
            "active_profile": null,
            "rooms": [{"id":"room-1","name":"General","topic":null,"participant_count":2,"last_activity_at_ms":1700000000000}],
            "direct_messages": [],
            "unread": [{"conversation_id":"room-1","count":3}],
            "activity": [],
            "network_health": {"ready":true,"status":"connected","connected_peer_count":1,"relay_url":null},
            "transfers": []
        }"#;
        let snapshot: AppSnapshot = serde_json::from_str(fixture).expect("fixture parses");
        assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.identity.as_ref().unwrap().id, "peer-a");
        assert_eq!(snapshot.unread[0].count, 3);
        let encoded = serde_json::to_string(&snapshot).expect("snapshot serializes");
        let decoded: AppSnapshot =
            serde_json::from_str(&encoded).expect("serialized snapshot parses");
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn checked_in_initial_snapshot_fixture_covers_every_section_and_schema_field() {
        let json = include_str!("../tests/fixtures/app_snapshot_initial.json");
        let value: serde_json::Value = serde_json::from_str(json).expect("fixture JSON parses");
        let object = value.as_object().expect("snapshot fixture is an object");
        let expected = [
            "schema_version",
            "identity",
            "active_profile",
            "rooms",
            "direct_messages",
            "unread",
            "activity",
            "network_health",
            "transfers",
        ];
        assert_eq!(object.len(), expected.len(), "top-level schema drift");
        for field in expected {
            assert!(
                object.contains_key(field),
                "missing snapshot field: {field}"
            );
        }
        let snapshot: AppSnapshot = serde_json::from_value(value).expect("fixture matches schema");
        assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert!(snapshot.identity.is_some());
        assert!(snapshot.active_profile.is_some());
        assert_eq!(snapshot.rooms.len(), 1);
        assert_eq!(snapshot.direct_messages.len(), 1);
        assert_eq!(snapshot.unread.len(), 1);
        assert_eq!(snapshot.activity.len(), 1);
        assert!(snapshot.network_health.is_some());
        assert_eq!(snapshot.transfers.len(), 1);
        assert_eq!(
            serde_json::from_str::<AppSnapshot>(&serde_json::to_string(&snapshot).unwrap())
                .unwrap(),
            snapshot
        );
    }

    #[test]
    fn ipc_contract_fixture_round_trips_and_has_no_secret_material() {
        let json = include_str!("../tests/fixtures/ipc_contract_v1.json");
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let message: MessageDto = serde_json::from_value(value["message"].clone()).unwrap();
        let contact: ContactDto = serde_json::from_value(value["contact"].clone()).unwrap();
        assert_eq!(contact.status, ContactStatusDto::Accepted);
        assert_eq!(message.delivery_status, Some(DeliveryStatusDto::Delivered));
        assert_eq!(
            message.attachment_handle.as_ref().unwrap().as_str(),
            "asset:fixture-1"
        );
        let encoded = serde_json::to_string(&message).unwrap();
        assert!(!encoded.contains("secret_key"));
        assert!(!encoded.contains("private_key"));
        assert!(serde_json::from_str::<MessageDto>(
            r#"{"id":"m","conversation_id":"c","sender_id":"s","timestamp_ms":0,"kind":"text","body":"x","secret_key":"bad"}"#
        ).is_err());
        assert!(serde_json::from_str::<DeliveryStatusDto>("\\\"future_state\\\"").is_err());
    }

    #[test]
    fn history_cursor_round_trips_as_opaque_string() {
        let page = MessageHistoryPage {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            items: vec![],
            next_cursor: Some(MessageHistoryCursor::new("opaque:cursor/1".into())),
        };
        let json = serde_json::to_string(&page).unwrap();
        let decoded: MessageHistoryPage = serde_json::from_str(&json).unwrap();
        assert_eq!(
            decoded.next_cursor.as_ref().unwrap().as_str(),
            "opaque:cursor/1"
        );
    }
}
