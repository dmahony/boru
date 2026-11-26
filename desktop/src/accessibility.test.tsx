import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import React from "react";
import { axe, toHaveNoViolations } from "jest-axe";
import { App, AppearanceSettings, RoomView } from "./main";
import { DialogGallery } from "./dialogs";
expect.extend(toHaveNoViolations);

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => ({ messages: [] })) }));
vi.mock("react-dom/client", () => ({ createRoot: () => ({ render: vi.fn() }) }));
vi.mock("./subscriptions", () => ({ appSubscriptions: { resync: vi.fn(), subscribe: vi.fn(() => () => {}) } }));
vi.mock("./media-client", () => ({ MediaTransferError: class extends Error {}, readMediaBlob: vi.fn(), saveMediaDownload: vi.fn(), uploadMedia: vi.fn() }));
vi.mock("./media-benchmark", () => ({ runRealMediaBenchmark: vi.fn() }));

afterEach(() => { cleanup(); document.documentElement.removeAttribute("data-theme"); });
const snapshot = { rooms: [], unread: [], identity: null } as never;

describe("BRT-033 axe audit", () => {
  it.each(["shell", "room view", "settings", "dialogs"]) ("has no serious or critical axe violations in %s", async surface => {
    const view = surface === "shell" ? <App />
      : surface === "room view" ? <RoomView snapshot={snapshot} />
      : surface === "settings" ? <AppearanceSettings /> : <DialogGallery />;
    const { container } = render(view);
    if (surface === "settings") await screen.findByRole("heading", { name: "Appearance" });
    if (surface === "dialogs") { await screen.findByRole("heading", { name: "Dialogs & overlays" }); fireEvent.click(screen.getByRole("button", { name: "Confirmation" })); await screen.findByRole("dialog"); }
    const results = await axe(container, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"] } });
    const seriousOrCritical = results.violations.filter(item => item.impact === "serious" || item.impact === "critical");
    expect(seriousOrCritical, JSON.stringify(seriousOrCritical.map(item => ({ id: item.id, nodes: item.nodes.map(node => node.target) })), null, 2)).toEqual([]);
  });
});

function luminance(hex: string): number {
  const rgb = hex.match(/[\da-f]{2}/gi)!.map(value => parseInt(value, 16) / 255).map(value => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2);
  return 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
}
function ratio(fg: string, bg: string): number { const [a, b] = [luminance(fg), luminance(bg)].sort((x, y) => y - x); return (a + 0.05) / (b + 0.05); }

describe("WCAG AA theme token contrast", () => {
  it.each([
    ["dark", "#eaf0f6", "#a1afbd", "#111820", "#17212b", "#79d6bd"],
    ["light", "#17211b", "#526158", "#f7f9f8", "#ffffff", "#246b58"],
    ["amoled", "#ffffff", "#c3c3c3", "#000000", "#080808", "#79d6bd"],
  ])("meets text and UI contrast tokens for %s", (_name, text, muted, canvas, surface, accent) => {
    expect(ratio(text, canvas)).toBeGreaterThanOrEqual(4.5);
    expect(ratio(text, surface)).toBeGreaterThanOrEqual(4.5);
    expect(ratio(muted, canvas)).toBeGreaterThanOrEqual(4.5);
    expect(ratio(muted, surface)).toBeGreaterThanOrEqual(4.5);
    expect(ratio(accent, canvas)).toBeGreaterThanOrEqual(3);
  });
});
