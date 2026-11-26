import React from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { CreateShareTunnelDialog } from "./dialogs";

afterEach(() => cleanup());
beforeEach(() => invoke.mockReset());

const services = [{ port: 8080, label: "Web server", isHttp: true }, { port: 5432, label: "Database", isHttp: false }];
const peers = [{ peerId: "peer-abc", alias: "Alice" }, { peerId: "peer-def", alias: "Bob" }];
function setup() {
  const onOpenChange = vi.fn();
  render(<CreateShareTunnelDialog open onOpenChange={onOpenChange} />);
  return onOpenChange;
}
function replyDefaults() {
  invoke.mockImplementation(async (command: string) => {
    if (command === "scan_local_services") return services;
    if (command === "list_trusted_peers") return peers;
    return undefined;
  });
}
async function chooseBoth() {
  await screen.findByRole("option", { name: "Web server — port 8080 (HTTP)" });
  fireEvent.change(screen.getByLabelText("Local service"), { target: { value: "8080" } });
  fireEvent.change(screen.getByLabelText("Trusted contact"), { target: { value: "peer-abc" } });
}

describe("CreateShareTunnelDialog", () => {
  it("shows service scanning and trusted-peer loading, then an accessible service/peer selector", async () => {
    let finishServices!: (value: typeof services) => void;
    let finishPeers!: (value: typeof peers) => void;
    invoke.mockImplementation((command: string) => command === "scan_local_services" ? new Promise(resolve => { finishServices = resolve; }) : new Promise(resolve => { finishPeers = resolve; }));
    setup();
    expect(screen.getByText("Scanning for local TCP services…")).toBeTruthy();
    expect(screen.getByText("Loading trusted contacts…")).toBeTruthy();
    await waitFor(() => { expect(finishServices).toBeTypeOf("function"); expect(finishPeers).toBeTypeOf("function"); });
    finishServices(services); finishPeers(peers);
    expect(await screen.findByRole("option", { name: "Web server — port 8080 (HTTP)" })).toBeTruthy();
    expect(screen.getByRole("option", { name: /Alice \(peer-abc\)/ })).toBeTruthy();
    expect(screen.getByLabelText("Local service").getAttribute("aria-invalid")).toBe("true");
    fireEvent.change(screen.getByLabelText("Find trusted contact"), { target: { value: "bob" } });
    expect(screen.getByRole("option", { name: /Bob \(peer-def\)/ })).toBeTruthy();
    expect(screen.queryByRole("option", { name: /Alice \(peer-abc\)/ })).toBeNull();
  });

  it("reports empty service and trusted-peer results", async () => {
    invoke.mockImplementation(async (command: string) => command === "scan_local_services" ? [] : []);
    setup();
    expect(await screen.findByText("No local TCP services were found. Start a service and reopen this dialog.")).toBeTruthy();
    expect(screen.getByText("No trusted contacts are available to share with.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Share tunnel" }).hasAttribute("disabled")).toBe(true);
  });

  it("reports scan and trusted-peer lookup failures without native calls", async () => {
    invoke.mockImplementation(async (command: string) => { if (command === "scan_local_services") throw new Error("scanner down"); if (command === "list_trusted_peers") throw new Error("lookup down"); });
    setup();
    expect(await screen.findByText("Could not scan local services. Close and reopen this dialog to try again.")).toBeTruthy();
    expect(screen.getByText("Could not load trusted contacts. Close and reopen this dialog to try again.")).toBeTruthy();
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("validates missing choices and submits a typed IPC offer successfully", async () => {
    replyDefaults(); setup();
    await screen.findByRole("option", { name: "Web server — port 8080 (HTTP)" });
    expect(screen.getByRole("button", { name: "Share tunnel" }).hasAttribute("disabled")).toBe(true);
    fireEvent.submit(screen.getByRole("button", { name: "Share tunnel" }).closest("form")!);
    expect(invoke).not.toHaveBeenCalledWith("submit_tunnel_offer", expect.anything());
    await chooseBoth();
    fireEvent.change(screen.getByLabelText("Trusted contact"), { target: { value: "" } });
    fireEvent.submit(screen.getByRole("button", { name: "Share tunnel" }).closest("form")!);
    expect(screen.getByText("Select a trusted contact to continue.")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Trusted contact"), { target: { value: "peer-abc" } });
    await userEvent.setup().click(screen.getByRole("button", { name: "Share tunnel" }));
    expect(await screen.findByText("Tunnel offer sent successfully.")).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith("submit_tunnel_offer", { peerId: "peer-abc", port: 8080, serviceName: "Web server", isHttp: true, expiresInSeconds: 3600, preferredLocalPort: null });
  });

  it("reports missing service and rejects an untrusted peer with a clear message", async () => {
    replyDefaults(); setup();
    await chooseBoth();
    fireEvent.change(screen.getByLabelText("Local service"), { target: { value: "" } });
    fireEvent.submit(screen.getByRole("button", { name: "Share tunnel" }).closest("form")!);
    expect(screen.getByText("Select a local service to continue.")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Local service"), { target: { value: "8080" } });
    invoke.mockImplementation(async (command: string) => { if (command === "scan_local_services") return services; if (command === "list_trusted_peers") return peers; if (command === "submit_tunnel_offer") throw "untrusted_peer"; });
    await userEvent.setup().click(screen.getByRole("button", { name: "Share tunnel" }));
    expect(await screen.findByText("This contact is no longer trusted. Choose a trusted contact and try again.")).toBeTruthy();
  });

  it("prevents a duplicate submission while the first offer is pending", async () => {
    let resolveOffer!: () => void;
    invoke.mockImplementation((command: string) => command === "scan_local_services" ? Promise.resolve(services) : command === "list_trusted_peers" ? Promise.resolve(peers) : new Promise<void>(resolve => { resolveOffer = resolve; }));
    setup(); await chooseBoth();
    const button = screen.getByRole("button", { name: "Share tunnel" });
    fireEvent.click(button);
    expect(await screen.findByRole("button", { name: "Sending…" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Sending…" }).hasAttribute("disabled")).toBe(true);
    fireEvent.submit(screen.getByRole("button", { name: "Sending…" }).closest("form")!);
    expect(invoke.mock.calls.filter(([command]) => command === "submit_tunnel_offer")).toHaveLength(1);
    resolveOffer();
    await waitFor(() => expect(screen.getByText("Tunnel offer sent successfully.")).toBeTruthy());
  });
});
