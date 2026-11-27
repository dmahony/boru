import React from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshotDto, RoomDto } from "./contract";

const { invoke, resync } = vi.hoisted(() => ({ invoke: vi.fn(), resync: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("react-dom/client", () => ({ createRoot: () => ({ render: vi.fn() }) }));
vi.mock("./subscriptions", () => ({ appSubscriptions: { resync } }));
vi.mock("./media-client", () => ({ MediaTransferError: class extends Error {}, readMediaBlob: vi.fn(), saveMediaDownload: vi.fn(), uploadMedia: vi.fn() }));
vi.mock("./media-benchmark", () => ({ runRealMediaBenchmark: vi.fn() }));

import { RoomView } from "./main";
const room: RoomDto = { id: "001122", name: "General", topic: null, participant_count: 2, last_activity_at_ms: 10 };
const snapshot: AppSnapshotDto = {
  schema_version: 1, identity: { id: "self", display_name: "Me" }, active_profile: null, rooms: [room],
  direct_messages: [{ id: "dm-aabb", peer_id: "peer", peer_name: "Pat", last_message_preview: "See you", last_activity_at_ms: 20 }],
  unread: [{ conversation_id: "001122", count: 3 }, { conversation_id: "dm-aabb", count: 5 }], activity: [], network_health: null, transfers: [],
};
afterEach(() => { cleanup(); vi.clearAllMocks(); });

describe("conversation sidebar", () => {
  it("renders snapshot rooms and DMs by activity with unread badges and no duplicate IDs", () => {
    render(<RoomView snapshot={snapshot} />);
    const rows = screen.getAllByRole("button");
    expect(rows.map(row => row.getAttribute("data-conversation-id"))).toEqual(["dm-aabb", "001122"]);
    expect(screen.getByRole("button", { name: /Pat.*5 unread/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /General.*3 unread/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Pat.*Direct message.*5 unread/ })).toBeTruthy();
  });
  it("selects a room row and opens that exact room thread", async () => {
    invoke.mockImplementation(async (command: string) => command === "open_room" ? { messages: [] } : undefined);
    render(<RoomView snapshot={snapshot} />);
    fireEvent.click(screen.getByRole("button", { name: /General/ }));
    expect(await screen.findByText("No messages yet. Start the conversation.")).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith("open_room", { conversationId: room.id });
  });
});
