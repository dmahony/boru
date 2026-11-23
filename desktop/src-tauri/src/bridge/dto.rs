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
