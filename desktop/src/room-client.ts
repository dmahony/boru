import { invoke } from "@tauri-apps/api/core";

export interface RoomSummary { id: string; name: string; lastMessageSummary: string | null; unreadCount: number }
export interface MessageDto { id: string; conversationId: string; senderId: string; timestampMs: number; kind: string; body: string; deliveryStatus: string | null; attachmentHandle: string | null }
export interface MessageHistoryPage { items: MessageDto[]; nextCursor: string | null }
export interface OpenRoomResponse { room: RoomSummary; messages: MessageDto[]; nextCursor: string | null }
export interface SendMessageRequest { idempotencyKey: string; conversationId: number[]; text: string; threadTarget: unknown | null }
export interface SendMessageResponse { eventId: number; messageHash: string }
export type AppErrorCode = "INVALID_PARAM" | "NOT_FOUND" | "DB_ERROR" | "INTERNAL" | "CANCELLED" | "CONFLICT";

export class BridgeError extends Error {
  constructor(readonly code: AppErrorCode | "UNKNOWN", message: string) { super(message); this.name = "BridgeError"; }
}

function typedError(error: unknown): BridgeError {
  const message = typeof error === "string" ? error : error instanceof Error ? error.message : "Unknown desktop bridge error";
  const code = message.match(/INVALID_PARAM|NOT_FOUND|DB_ERROR|INTERNAL|CANCELLED|CONFLICT/)?.[0] as AppErrorCode | undefined;
  return new BridgeError(code ?? "UNKNOWN", message);
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try { return await invoke<T>(command, args); } catch (error) { throw typedError(error); }
}

export function listRooms(): Promise<RoomSummary[]> { return call("list_rooms"); }
export function openRoom(conversationId: string): Promise<OpenRoomResponse> { return call("open_room", { conversationId }); }
export function getMessagesPage(conversationId: string, cursor: string | null, limit: number | null, requestId: string): Promise<MessageHistoryPage> {
  return call("get_messages_page", { conversationId, cursor, limit, requestId });
}
export function sendMessage(request: SendMessageRequest): Promise<SendMessageResponse> { return call("send_message", { request }); }
export function markRead(conversationId: number[]): Promise<number | null> { return call("mark_read", { request: { conversationId } }); }
