import { describe, expect, it } from "vitest";
import type { AppSnapshotDto, RoomDto } from "./contract";
import { selectConversations } from "./conversation-selector";

const room = (id: string, timestamp: number | null): RoomDto => ({ id, name: id, topic: null, participant_count: null, last_activity_at_ms: timestamp });
const snapshot = (overrides: Partial<AppSnapshotDto> = {}): AppSnapshotDto => ({
  schema_version: 1, identity: null, active_profile: null, rooms: [], direct_messages: [], unread: [], activity: [], network_health: null, transfers: [], ...overrides,
});

describe("selectConversations", () => {
  it("merges room and DM projections in Rust snapshot activity order with unread counts", () => {
    const rows = selectConversations(snapshot({
      rooms: [room("room-old", 10), room("room-new", 30)],
      direct_messages: [{ id: "dm-mid", peer_id: "peer", peer_name: "Pat", last_message_preview: "Hi", last_activity_at_ms: 20 }],
      unread: [{ conversation_id: "room-new", count: 4 }, { conversation_id: "dm-mid", count: 2 }],
    }));
    expect(rows.map(row => [row.id, row.kind, row.unreadCount])).toEqual([["room-new", "room", 4], ["dm-mid", "direct", 2], ["room-old", "room", 0]]);
  });

  it("deduplicates stable IDs, combines duplicate unread projections by maximum and sorts ties deterministically", () => {
    const rows = selectConversations(snapshot({
      rooms: [room("same", 10), room("tie-b", 10)],
      direct_messages: [{ id: "same", peer_id: "peer", peer_name: "DM", last_message_preview: null, last_activity_at_ms: 10 }],
      unread: [{ conversation_id: "same", count: 1 }, { conversation_id: "same", count: 3 }],
    }));
    expect(rows.map(row => [row.id, row.kind, row.title, row.unreadCount])).toEqual([["same", "direct", "DM", 3], ["tie-b", "room", "tie-b", 0]]);
  });

  it("keeps empty and offline snapshot rows without fabricating status or unread state", () => {
    expect(selectConversations(snapshot({ rooms: [room("offline", null)] }))).toEqual([
      { id: "offline", kind: "room", title: "offline", subtitle: null, lastActivityAtMs: null, unreadCount: 0 },
    ]);
    expect(selectConversations(snapshot())).toEqual([]);
  });
});
