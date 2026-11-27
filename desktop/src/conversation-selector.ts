import type { AppSnapshotDto } from "./contract";

export type ConversationRow = {
  id: string;
  kind: "room" | "direct";
  title: string;
  subtitle: string | null;
  lastActivityAtMs: number | null;
  unreadCount: number;
};

/** Build a stable, duplicate-free list from the canonical Rust snapshot. */
export function selectConversations(snapshot: AppSnapshotDto): ConversationRow[] {
  const unread = new Map<string, number>();
  for (const item of snapshot.unread) {
    unread.set(item.conversation_id, Math.max(unread.get(item.conversation_id) ?? 0, item.count));
  }

  const rows = new Map<string, ConversationRow>();
  for (const room of snapshot.rooms) {
    rows.set(room.id, {
      id: room.id,
      kind: "room",
      title: room.name || room.id.slice(0, 12),
      subtitle: room.topic,
      lastActivityAtMs: room.last_activity_at_ms,
      unreadCount: unread.get(room.id) ?? 0,
    });
  }
  for (const dm of snapshot.direct_messages) {
    // Prefer the direct-thread projection when stale data contains an ID collision.
    rows.set(dm.id, {
      id: dm.id,
      kind: "direct",
      title: dm.peer_name || dm.peer_id.slice(0, 12),
      subtitle: dm.last_message_preview,
      lastActivityAtMs: dm.last_activity_at_ms,
      unreadCount: unread.get(dm.id) ?? 0,
    });
  }

  return [...rows.values()].sort((left, right) =>
    (right.lastActivityAtMs ?? -1) - (left.lastActivityAtMs ?? -1) || left.id.localeCompare(right.id),
  );
}
