// @vitest-environment jsdom
import React from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("react-dom/client", () => ({ createRoot: () => ({ render: vi.fn() }) }));
vi.mock("./subscriptions", () => ({ appSubscriptions: { resync: vi.fn() } }));
vi.mock("./media-client", () => ({ MediaTransferError: class extends Error {}, readMediaBlob: vi.fn(), saveMediaDownload: vi.fn(), uploadMedia: vi.fn() }));
vi.mock("./media-benchmark", () => ({ runRealMediaBenchmark: vi.fn() }));

import { AppearanceSettings } from "./main";

const saved = { theme: "light", font_size: 20, scale: 120, accent: "#123456", sound_enabled: true, show_presence_indicator: true, typing_indicators_enabled: true, share_direct_addresses: false, keep_running: false, notification_policy: "all", restart_required: ["Keep network host running after window close"] };
type Preferences = { theme: string; font_size: number; scale: number; accent: string | null; sound_enabled?: boolean; show_presence_indicator?: boolean; typing_indicators_enabled?: boolean; share_direct_addresses?: boolean; keep_running?: boolean; notification_policy?: string; restart_required?: string[] };
const summary = (preferences: Preferences = saved) => ({ schema_version: 1, theme: preferences.theme, preferences });

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  document.documentElement.removeAttribute("data-theme");
  for (const name of ["--user-font-size", "--user-scale", "--user-accent"]) document.documentElement.style.removeProperty(name);
});

beforeEach(() => {
  invoke.mockImplementation(async (command: string) => command === "get_settings" ? summary() : summary());
});

describe("AppearanceSettings", () => {
  it("renders persisted preferences and applies them as live CSS variables", async () => {
    render(<AppearanceSettings />);
    expect(screen.getByText(/Loading saved preferences/)).toBeTruthy();
    const theme = (await screen.findByLabelText(/Theme/)) as HTMLSelectElement;
    expect(theme.value).toBe("light");
    expect((screen.getByLabelText(/Font size/) as HTMLInputElement).value).toBe("20");
    expect((screen.getByLabelText(/Scale/) as HTMLInputElement).value).toBe("120");
    expect((screen.getByLabelText(/Accent/) as HTMLInputElement).value).toBe("#123456");
    await waitFor(() => {
      expect(document.documentElement.dataset.theme).toBe("light");
      expect(document.documentElement.style.getPropertyValue("--user-font-size")).toBe("20px");
      expect(document.documentElement.style.getPropertyValue("--user-scale")).toBe("1.2");
      expect(document.documentElement.style.getPropertyValue("--user-accent")).toBe("#123456");
    });
    expect(invoke).toHaveBeenCalledWith("get_settings");
  });

  it("persists theme changes through the bridge and reports rejected writes", async () => {
    render(<AppearanceSettings />);
    const theme = await screen.findByLabelText(/Theme/);
    fireEvent.change(theme, { target: { value: "amoled" } });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("update_settings", { preferences: { ...saved, theme: "amoled" } }));
    expect(document.documentElement.dataset.theme).toBe("amoled");

    invoke.mockRejectedValueOnce(new Error("db offline"));
    fireEvent.change(screen.getByLabelText(/Font size/), { target: { value: "18" } });
    expect(await screen.findByRole("alert")).toBeTruthy();
  });

  it("meets WCAG AA contrast thresholds for dark, light, and AMOLED theme tokens", () => {
    const luminance = (hex: string) => hex.match(/[0-9a-f]{2}/gi)!.map(value => parseInt(value, 16) / 255).map(value => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2).reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0);
    const contrast = (foreground: string, background: string) => { const [light, dark] = [luminance(foreground), luminance(background)].sort((a, b) => b - a); return (light + 0.05) / (dark + 0.05); };
    const themes = [
      ["dark", "#eaf0f6", "#a1afbd", "#111820", "#17212b", "#79d6bd"],
      ["light", "#17211b", "#526158", "#f7f9f8", "#ffffff", "#246b58"],
      ["amoled", "#ffffff", "#c3c3c3", "#000000", "#080808", "#79d6bd"],
    ];
    for (const [_name, text, muted, canvas, surface, accent] of themes) {
      expect(contrast(text, canvas)).toBeGreaterThanOrEqual(4.5);
      expect(contrast(text, surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrast(muted, canvas)).toBeGreaterThanOrEqual(4.5);
      expect(contrast(muted, surface)).toBeGreaterThanOrEqual(4.5);
      expect(contrast(accent, canvas)).toBeGreaterThanOrEqual(3);
    }
  });

  it("falls back to defaults when the first launch has no saved preferences", async () => {
    invoke.mockImplementation(async () => summary({ theme: "dark", font_size: 16, scale: 100, accent: null }));
    render(<AppearanceSettings />);
    const theme = (await screen.findByLabelText(/Theme/)) as HTMLSelectElement;
    expect(theme.value).toBe("dark");
    expect(document.documentElement.style.getPropertyValue("--user-scale")).toBe("1");
    expect(document.documentElement.style.getPropertyValue("--user-accent")).toBe("#79d6bd");
  });
});
