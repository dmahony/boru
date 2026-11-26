import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

type Apply = (value: unknown) => void;
const { invoke, subscribe } = vi.hoisted(() => ({ invoke: vi.fn(), subscribe: vi.fn((_apply?: (value: unknown) => void) => () => {}) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("react-dom/client", () => ({ createRoot: () => ({ render: () => {} }) }));
vi.mock("./subscriptions", () => ({ appSubscriptions: { subscribe, resync: vi.fn() } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

import { App } from "./main";

const snapshot = { snapshot: { schema_version: 1, identity: { id: "endpoint-public-id", display_name: "Alice" }, rooms: [], active_profile: null, direct_messages: [], unread: [], activity: [], network_health: null, transfers: [] }, cursor: { epoch: "e", revision: 0 } };

describe("startup identity routing", () => {
  afterEach(() => cleanup());
  beforeEach(() => { invoke.mockReset(); subscribe.mockClear(); });

  it("routes a missing identity to onboarding without creating one", async () => {
    invoke.mockResolvedValue({ state: "no_identity", endpointId: null, displayName: null });
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Welcome to Boru" })).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith("get_startup_state");
    expect(invoke).not.toHaveBeenCalledWith("create_identity", expect.anything());
    expect(subscribe).not.toHaveBeenCalled();
  });

  it("lists existing profiles and only enters the workspace once one is opened", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_startup_state") return { state: "existing", endpointId: "endpoint-public-id", displayName: "Alice" };
      if (command === "list_profiles") return [{ endpointId: "endpoint-public-id", displayName: "Alice", hasExistingIdentity: true }];
      if (command === "select_identity") return "endpoint-public-id";
      return snapshot;
    });
    render(<App />);
    subscribe.mockImplementation((apply?: Apply) => { apply?.(snapshot); return () => {}; });
    expect(await screen.findByRole("heading", { name: "Choose a profile" })).toBeTruthy();
    expect(screen.getByText("endpoint-public-id")).toBeTruthy();
    expect(subscribe).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Open profile" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("select_identity", { endpointId: "endpoint-public-id" }));
    await waitFor(() => expect(subscribe).toHaveBeenCalled());
    expect(await screen.findByText("Chats")).toBeTruthy();
    expect(screen.getByLabelText("Endpoint ID").textContent).toBe("endpoint-public-id");
  });

  it("creates a new identity from the onboarding flow without requiring network services", async () => {
    const originalError = console.error;
    const logged: unknown[][] = [];
    const errorSpy = vi.spyOn(console, "error").mockImplementation((...args) => { logged.push(args); });
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_startup_state") return { state: "no_identity", endpointId: null, displayName: null };
      if (command === "create_identity") return "new-public-id";
      return snapshot;
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Boru" });
    fireEvent.change(screen.getByLabelText(/Display name/), { target: { value: "Offline Alice" } });
    fireEvent.click(screen.getByRole("button", { name: "Create profile" }));
    fireEvent.click(await screen.findByRole("button", { name: "Confirm and create" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("create_identity", { displayName: "Offline Alice" }));
    await screen.findByText("Chats");
    expect(invoke.mock.calls.some(([command]) => String(command).match(/network|connect|join/i))).toBe(false);
    expect(document.documentElement.innerHTML).not.toMatch(/secret[_ -]?key|private[_ -]?key|seed phrase/i);
    expect(logged.flat().join(" ")).not.toMatch(/secret[_ -]?key|private[_ -]?key|seed phrase/i);
    errorSpy.mockRestore();
    expect(console.error).toBe(originalError);
  });

  it("creates an identity only after the explicit confirmation step", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "get_startup_state") return { state: "no_identity", endpointId: null, displayName: null };
      if (command === "create_identity") return "new-public-id";
      return snapshot;
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Boru" });
    expect(invoke).not.toHaveBeenCalledWith("create_identity", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: "Create profile" }));
    expect(await screen.findByRole("dialog")).toBeTruthy();
    expect(invoke).not.toHaveBeenCalledWith("create_identity", expect.anything());
    fireEvent.click(screen.getByRole("button", { name: "Confirm and create" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("create_identity", { displayName: null }));
    await waitFor(() => expect(subscribe).toHaveBeenCalled());
  });

  it("surfaces safe IPC errors without rendering backend error details", async () => {
    invoke.mockRejectedValue(new Error("secret key material"));
    render(<App />);
    expect((await screen.findByRole("alert")).textContent).toContain("couldn't check your saved profiles");
    expect(document.body.textContent).not.toContain("secret key material");
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
  });
});
