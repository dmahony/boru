// @vitest-environment jsdom
import React from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshotDto, MessageDto, RoomDto } from "./contract";

const { invoke, resync } = vi.hoisted(() => ({ invoke: vi.fn(), resync: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("react-dom/client", () => ({ createRoot: () => ({ render: vi.fn() }) }));
vi.mock("./subscriptions", () => ({ appSubscriptions: { resync } }));
vi.mock("./media-client", () => ({ MediaTransferError: class extends Error {}, readMediaBlob: vi.fn(), saveMediaDownload: vi.fn(), uploadMedia: vi.fn() }));
vi.mock("./media-benchmark", () => ({ runRealMediaBenchmark: vi.fn() }));

import { RoomView } from "./main";

const room: RoomDto = { id: "001122", name: "General", topic: null, participant_count: 2, last_activity_at_ms: null };
const message: MessageDto = { id: "m1", conversation_id: room.id, sender_id: "peer", timestamp_ms: 1_700_000_000_000, kind: "text", body: "Welcome", delivery_status: "delivered", attachment_handle: null };
const snapshot = (rooms: RoomDto[] = [room], unread: AppSnapshotDto["unread"] = [{ conversation_id: room.id, count: 3 }]): AppSnapshotDto => ({
  schema_version: 1, identity: { id: "self", display_name: "Me" }, active_profile: null, rooms, direct_messages: [], unread, activity: [], network_health: null, transfers: [],
});

afterEach(() => { cleanup(); vi.clearAllMocks(); });
beforeEach(() => { invoke.mockImplementation(async (command: string) => command === "open_room" ? { messages: [message] } : undefined); });

describe("RoomView bridge interactions", () => {
  it("renders room list and snapshot-backed unread count", () => {
    render(<RoomView snapshot={snapshot()} />);
    expect(screen.getByRole("button", { name: /General.*3/ })).toBeTruthy();
    expect(screen.getByText("Choose a room to view its history.")).toBeTruthy();
  });

  it("opens room history, marks it read, and shows persisted delivery state", async () => {
    render(<RoomView snapshot={snapshot()} />);
    fireEvent.click(screen.getByRole("button", { name: /General/ }));
    expect(await screen.findByText("Welcome")).toBeTruthy();
    expect(screen.getByText("delivered")).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith("open_room", { conversationId: room.id });
    expect(invoke).toHaveBeenCalledWith("mark_read", { request: { conversationId: [0, 17, 34] } });
    expect(resync).toHaveBeenCalled();
  });

  it("sends a message through IPC and clears the draft on success", async () => {
    render(<RoomView snapshot={snapshot([room], [])} />);
    fireEvent.click(screen.getByRole("button", { name: /General/ }));
    await screen.findByText("Welcome");
    const input = screen.getByRole("textbox", { name: "Message" });
    fireEvent.change(input, { target: { value: "  hello  " } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("send_message", expect.objectContaining({ args: expect.objectContaining({ request: expect.objectContaining({ conversationId: [0, 17, 34], text: "hello", threadTarget: null }) }) })));
    await waitFor(() => expect((input as HTMLInputElement).value).toBe(""));
  });

  it("keeps the draft and displays send failures", async () => {
    invoke.mockImplementation(async (command: string) => { if (command === "open_room") return { messages: [] }; if (command === "send_message") throw new Error("offline"); });
    render(<RoomView snapshot={snapshot()} />);
    fireEvent.click(screen.getByRole("button", { name: /General/ }));
    await screen.findByText("No messages yet. Start the conversation.");
    const input = screen.getByRole("textbox", { name: "Message" });
    fireEvent.change(input, { target: { value: "retry me" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toContain("offline");
    expect((input as HTMLInputElement).value).toBe("retry me");
  });

  it("renders empty, loading, and error states safely", async () => {
    render(<RoomView snapshot={snapshot([])} />);
    expect(screen.getByText("No conversations yet.")).toBeTruthy();
    let finish!: (value: { messages: MessageDto[] }) => void;
    invoke.mockImplementation((command: string) => command === "open_room" ? new Promise(resolve => { finish = resolve; }) : undefined);
    const view = render(<RoomView snapshot={snapshot()} />);
    fireEvent.click(screen.getByRole("button", { name: /General/ }));
    expect(screen.getByText("Loading messages…")).toBeTruthy();
    finish({ messages: [] });
    expect(await screen.findByText("No messages yet. Start the conversation.")).toBeTruthy();
    view.unmount();
    invoke.mockRejectedValue(new Error("history unavailable"));
    render(<RoomView snapshot={snapshot()} />);
    fireEvent.click(screen.getByRole("button", { name: /General/ }));
    expect((await screen.findByRole("alert")).textContent).toContain("history unavailable");
  });

  it("restores a per-conversation draft and persists edits", async () => {
    const onPersist = vi.fn();
    render(<RoomView snapshot={snapshot()} drafts={{ [room.id]: "half typed" }} onPersist={onPersist} />);
    fireEvent.click(screen.getByRole("button", { name: /General/ }));
    const input = screen.getByRole("textbox", { name: "Message" }) as HTMLInputElement;
    expect(input.value).toBe("half typed");
    fireEvent.change(input, { target: { value: "half typed more" } });
    await waitFor(() => expect(onPersist).toHaveBeenCalledWith({ drafts: { [room.id]: "half typed more" } }), { timeout: 1000 });
  });

  it("clears the stored draft after a successful send", async () => {
    const onPersist = vi.fn();
    render(<RoomView snapshot={snapshot([room], [])} drafts={{ [room.id]: "hello" }} onPersist={onPersist} />);
    fireEvent.click(screen.getByRole("button", { name: /General/ }));
    await screen.findByText("Welcome");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(onPersist).toHaveBeenCalledWith({ drafts: {} }));
  });

  it("reopens the remembered conversation on launch without marking it read", async () => {
    render(<RoomView snapshot={snapshot()} initialConversationId={room.id} />);
    expect(await screen.findByText("Welcome")).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith("open_room", { conversationId: room.id });
    expect(invoke).not.toHaveBeenCalledWith("mark_read", expect.anything());
  });
});
