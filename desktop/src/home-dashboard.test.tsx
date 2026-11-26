// @vitest-environment jsdom
import React from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { AppSnapshotDto } from "./contract";
import { HomeDashboard } from "./HomeDashboard";

const fixture: AppSnapshotDto = {
  schema_version: 1,
  identity: { id: "me", display_name: "Ada" },
  active_profile: null,
  rooms: [{ id: "room-1", name: "General", topic: null, participant_count: 2, last_activity_at_ms: 1_700_000_000_000 }],
  direct_messages: [{ id: "dm-1", peer_id: "peer", peer_name: "Lin", last_message_preview: "Hi", last_activity_at_ms: 1_700_000_000_000 }],
  unread: [{ conversation_id: "room-1", count: 3 }],
  activity: [{ id: "activity-1", kind: "message", summary: "Lin sent a message", occurred_at_ms: 1_700_000_000_000, conversation_id: "room-1" }],
  network_health: { ready: true, status: "Connected", connected_peer_count: 2, relay_url: null },
  transfers: [{ id: "transfer-1", direction: "upload", file_name: "report.pdf", state: "active", transferred_bytes: 20, total_bytes: 100, handle: null }],
};

describe("HomeDashboard", () => {
  it("projects snapshot values and exposes working quick actions", () => {
    const onOpenRoom = vi.fn();
    const onOpenMedia = vi.fn();
    render(<HomeDashboard snapshot={fixture} onOpenRoom={onOpenRoom} onOpenMedia={onOpenMedia} />);
    expect(screen.getByText("Welcome, Ada")).toBeTruthy();
    expect(screen.getByText("2", { selector: "strong" })).toBeTruthy();
    expect(screen.getByText("1 rooms · 3 unread")).toBeTruthy();
    expect(screen.getByText("Lin sent a message")).toBeTruthy();
    expect(screen.getByText("Tunnels").parentElement?.textContent).toContain("snapshot unavailable");
    fireEvent.click(screen.getByRole("button", { name: /open conversations/i }));
    fireEvent.click(screen.getByRole("button", { name: /share a file/i }));
    expect(onOpenRoom).toHaveBeenCalledOnce();
    expect(onOpenMedia).toHaveBeenCalledOnce();
  });

  it("renders explicit offline and empty states without inventing values", () => {
    const empty = { ...fixture, direct_messages: [], rooms: [], unread: [], activity: [], network_health: null, transfers: [] };
    render(<HomeDashboard snapshot={empty} onOpenRoom={() => {}} onOpenMedia={() => {}} />);
    expect(screen.getByText("Network status unavailable")).toBeTruthy();
    expect(screen.getByText("No recent activity yet.")).toBeTruthy();
    expect(screen.getByText("Waiting for network snapshot")).toBeTruthy();
  });
});
