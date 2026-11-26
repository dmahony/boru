import { invoke } from "@tauri-apps/api/core";

export interface WindowGeometry { x: number; y: number; width: number; height: number }
export interface UiPreferences {
  drafts: Record<string, string>;
  activeConversationId: string | null;
  activeNavigation: string | null;
  sidebarWidth: number | null;
  window: WindowGeometry | null;
  rememberWindowGeometry: boolean;
  reopenLastConversation: boolean;
}
export const defaultUiPreferences: UiPreferences = {
  drafts: {}, activeConversationId: null, activeNavigation: null, sidebarWidth: null, window: null,
  rememberWindowGeometry: true, reopenLastConversation: true,
};
export async function loadUiPreferences(): Promise<UiPreferences> {
  try {
    const saved = await invoke<Partial<UiPreferences>>("load_ui_preferences");
    return { ...defaultUiPreferences, ...saved, drafts: saved.drafts && typeof saved.drafts === "object" ? saved.drafts : {} };
  } catch { return { ...defaultUiPreferences }; }
}
export async function saveUiPreferences(preferences: UiPreferences): Promise<void> {
  await invoke("save_ui_preferences", { request: { preferences } });
}
