// Boru IPC DTO contract v1. Keep wire values JSON-safe: timestamps/byte counts
// are decimal-safe integers and binary data is represented by opaque handles.
// Rust's serde DTOs and tests/fixtures/ipc_contract_v1.json are authoritative.
export const IPC_SCHEMA_VERSION = 1 as const;

export interface IdentityDto { id: string; display_name: string | null }
export interface RoomDto { id: string; name: string; topic: string | null; participant_count: number | null; last_activity_at_ms: number | null }
export interface ContactDto { id: string; display_name: string; status: ContactStatus }
export type ContactStatus = "pending" | "accepted" | "blocked";
export interface MessageDto { id: string; conversation_id: string; sender_id: string; timestamp_ms: number; kind: string; body: string; delivery_status: DeliveryStatus | null; attachment_handle: string | null }
export interface MessagePageDto { schema_version: typeof IPC_SCHEMA_VERSION; items: MessageDto[]; next_cursor: string | null }
export type DeliveryStatus = "queued" | "sent" | "delivered" | "seen" | "failed";
export interface TransferDto { id: string; direction: "upload" | "download"; file_name: string; state: string; transferred_bytes: number; total_bytes: number | null; handle: string | null }
export interface NetworkStatusDto { ready: boolean; status: string; connected_peer_count: number | null; relay_url: string | null }
export interface SettingsSummaryDto { schema_version: typeof IPC_SCHEMA_VERSION; theme: string | null }

// Readers must reject unsupported schema versions. Unknown additive fields are
// ignored by JS object consumers; Rust's strict DTO deserialization rejects them.
export interface AppSnapshotDto {
  schema_version: typeof IPC_SCHEMA_VERSION;
  identity: IdentityDto | null;
  active_profile: { id: string; display_name: string; description: string | null } | null;
  rooms: RoomDto[];
  direct_messages: { id: string; peer_id: string; peer_name: string; last_message_preview: string | null; last_activity_at_ms: number | null }[];
  unread: { conversation_id: string; count: number }[];
  activity: { id: string; kind: string; summary: string; occurred_at_ms: number; conversation_id: string | null }[];
  network_health: NetworkStatusDto | null;
  transfers: TransferDto[];
}
