// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi, type Mock } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { appSubscriptions, type EventNotice } from "./subscriptions";

type EventHandler = (event: { payload: EventNotice }) => void;

describe("event envelope kind discriminator", () => {
  it("deserializes inboundMessage and conversationChanged notices with their ids", () => {
    const inbound = JSON.parse(
      '{"epoch":"e1","revision":7,"eventId":"e1:7","kind":"inboundMessage","messageId":"42"}',
    ) as EventNotice;
    expect(inbound.kind).toBe("inboundMessage");
    expect(inbound.messageId).toBe("42");
    expect(inbound.conversationId).toBeUndefined();

    const conversation = JSON.parse(
      '{"epoch":"e1","revision":8,"eventId":"e1:8","kind":"conversationChanged","conversationId":"abc"}',
    ) as EventNotice;
    expect(conversation.kind).toBe("conversationChanged");
    expect(conversation.conversationId).toBe("abc");
    expect(conversation.messageId).toBeUndefined();
  });

  it("keeps the additive fields optional for envelopes without a kind", () => {
    const legacy = JSON.parse('{"epoch":"e1","revision":9,"eventId":"e1:9"}') as EventNotice;
    expect(legacy.eventId).toBe("e1:9");
    expect(legacy.kind).toBeUndefined();
    expect(legacy.messageId).toBeUndefined();
    expect(legacy.conversationId).toBeUndefined();
  });
});

describe("subscription refetch contract", () => {
  let snapshotCalls = 0;
  let eventHandler: EventHandler | undefined;

  beforeEach(() => {
    snapshotCalls = 0;
    eventHandler = undefined;
    (listen as unknown as Mock).mockImplementation(async (name: string, handler: unknown) => {
      if (name === "boru://ui-event") eventHandler = handler as EventHandler;
      return () => {};
    });
    // The cursor revision tracks the number of authoritative reads so that each
    // incoming notice stays contiguous and triggers exactly one refetch.
    (invoke as unknown as Mock).mockImplementation(async (command: string) => {
      if (command === "get_app_snapshot") {
        snapshotCalls += 1;
        return { snapshot: {}, cursor: { epoch: "e1", revision: snapshotCalls } };
      }
      if (command === "replay_ui_events") return { kind: "events", events: [] };
      throw new Error(`unexpected command ${command}`);
    });
  });

  it("refetches the snapshot on every event regardless of kind", async () => {
    const unsubscribe = appSubscriptions.subscribe(() => {});
    await vi.waitFor(() => expect(snapshotCalls).toBe(1));
    expect(eventHandler).toBeTypeOf("function");

    const notices: EventNotice[] = [
      { epoch: "e1", revision: 2, eventId: "e1:2", kind: "inboundMessage", messageId: "42" },
      { epoch: "e1", revision: 3, eventId: "e1:3", kind: "conversationChanged", conversationId: "abc" },
      { epoch: "e1", revision: 4, eventId: "e1:4", kind: "someFutureKind" },
      { epoch: "e1", revision: 5, eventId: "e1:5" },
    ];

    for (const [index, notice] of notices.entries()) {
      eventHandler!({ payload: notice });
      await vi.waitFor(() => expect(snapshotCalls).toBe(index + 2));
    }

    expect(invoke).toHaveBeenCalledWith("get_app_snapshot");
    expect(invoke).toHaveBeenCalledWith("replay_ui_events", {
      cursor: { epoch: "e1", revision: 1 },
    });
    unsubscribe();
  });
});
