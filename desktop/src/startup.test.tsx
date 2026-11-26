import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

const { invoke, subscribe } = vi.hoisted(() => ({ invoke: vi.fn(), subscribe: vi.fn(() => () => {}) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("react-dom/client", () => ({ createRoot: () => ({ render: () => {} }) }));
vi.mock("./subscriptions", () => ({ appSubscriptions: { subscribe, resync: vi.fn() } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

import { App } from "./main";

describe("startup identity routing", () => {
  afterEach(() => cleanup());
  beforeEach(() => { invoke.mockReset(); subscribe.mockClear(); });

  it("routes a missing identity to onboarding without creating one", async () => {
    invoke.mockResolvedValue({ state: "no_identity", endpointId: null, displayName: null });
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Welcome to Boru" })).toBeTruthy();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("get_startup_state");
    expect(invoke).not.toHaveBeenCalledWith("create_identity", expect.anything());
    expect(subscribe).not.toHaveBeenCalled();
  });

  it("opens existing identity and starts the workspace subscription", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_startup_state") return { state: "existing", endpointId: "endpoint-public-id", displayName: "Alice" };
      return { snapshot: { schema_version: 1, identity: { id: "endpoint-public-id", display_name: "Alice" }, rooms: [], active_profile: null, direct_messages: [], unread: [], activity: [], network_health: null, transfers: [] }, cursor: { epoch: "e", revision: 0 } };
    });
    render(<App />);
    await waitFor(() => expect(subscribe).toHaveBeenCalled());
    expect(await screen.findByText("Chats")).toBeTruthy();
    expect(screen.getByLabelText("Endpoint ID").textContent).toBe("endpoint-public-id");
    expect(invoke).toHaveBeenCalledWith("get_startup_state");
    expect(invoke).not.toHaveBeenCalledWith("create_identity", expect.anything());
  });

  it("creates an identity only after the explicit onboarding action", async () => {
    invoke.mockImplementation(async (command: string) => command === "get_startup_state" ? { state: "no_identity", endpointId: null, displayName: null } : "new-public-id");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Boru" });
    expect(invoke).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Create profile" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("create_identity", { displayName: null }));
    await waitFor(() => expect(subscribe).toHaveBeenCalled());
    expect(screen.getByLabelText("Endpoint ID").textContent).toBe("new-public-id");
  });

  it("surfaces safe IPC errors without rendering backend error details", async () => {
    invoke.mockRejectedValue(new Error("secret key material"));
    render(<App />);
    expect((await screen.findByRole("alert")).textContent).toContain("couldn't check the saved identity");
    expect(document.body.textContent).not.toContain("secret key material");
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
  });
});
