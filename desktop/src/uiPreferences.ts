import { invoke } from "@tauri-apps/api/core";

export interface WindowGeometry { x: number; y: number; width: number; height: number }
export interface UiPreferences {
  drafts: Record<string, string>;
  activeConversationId: string | null;
  activeNavigation: string | null;
  sidebarWidth: number | null;
  window: WindowGeometry | null;
}
export const defaultUiPreferences: UiPreferences = {
  drafts: {}, activeConversationId: null, activeNavigation: null, sidebarWidth: null, window: null,
};

export async function loadUiPreferences(): Promise<UiPreferences> {
  try {
    const saved = await invoke<Partial<UiPreferences>>("load_ui_preferences");
    return {
      ...defaultUiPreferences, ...saved,
      drafts: saved.drafts && typeof saved.drafts === "object" ? saved.drafts : {},
    };
  } catch { return { ...defaultUiPreferences }; }
}
export async function saveUiPreferences(preferences: UiPreferences): Promise<void> {
  await invoke("save_ui_preferences", { request: { preferences } });
}

// Clamp the restored position to the current available work area. Browsers do
// not expose all monitor rectangles, so use the native viewport as a safe bound.
export function visibleWindowGeometry(saved: WindowGeometry, viewport: { width: number; height: number }): WindowGeometry {
  const width = Math.max(640, Math.min(saved.width, Math.max(640, viewport.width)));
  const height = Math.max(420, Math.min(saved.height, Math.max(420, viewport.height)));
  const maxX = Math.max(0, viewport.width - width);
  const maxY = Math.max(0, viewport.height - height);
  return { x: Math.max(0, Math.min(saved.x, maxX)), y: Math.max(0, Math.min(saved.y, maxY)), width, height };
}
