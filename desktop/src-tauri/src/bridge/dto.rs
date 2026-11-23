//! Facade over the canonical IPC DTO contract.
//!
//! The authoritative Rust shapes live in [`boru_core::snapshot`] and mirror
//! `desktop/src/contract.ts` plus `tests/fixtures/ipc_contract_v1.json`. This
//! module exists only so Tauri commands reach the contract through the
//! `bridge::dto` path; it must not redefine divergent copies of the DTOs.

pub use boru_core::message_pagination::paginate;
pub use boru_core::snapshot::{
    ActivitySnapshot, AppSnapshot, BinaryHandleDto, ContactDto, ContactStatusDto,
    DeliveryStatusDto, DirectMessageSnapshot, IdentitySnapshot, MessageDto, MessageHistoryCursor,
    MessageHistoryItem, MessageHistoryPage, NetworkHealthSnapshot, ProfileSnapshot, RoomSnapshot,
    TransferSummary, UnreadSnapshot, SNAPSHOT_SCHEMA_VERSION,
};

/// Room list projection serialized as `{ "id", "name", "lastMessageSummary", "unreadCount" }`.
/// `lastMessageSummary` is null when no preview is available; `unreadCount` is numeric.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomSummary {
    /// Canonical conversation/topic identifier.
    pub id: String,
    /// Human-readable room name.
    pub name: String,
    /// Last-message preview, or null when unavailable.
    pub last_message_summary: Option<String>,
    /// Number of unread messages.
    pub unread_count: u32,
}

/// Open-room response serialized as `{ "room", "messages", "nextCursor" }`.
/// Messages use the canonical [`MessageDto`] wire shape; `nextCursor` is null
/// when the first page exhausts history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRoomResponse {
    /// Shared room-list summary.
    pub room: RoomSummary,
    /// First history page.
    pub messages: Vec<MessageDto>,
    /// Opaque cursor for the next page, if any.
    pub next_cursor: Option<String>,
}
