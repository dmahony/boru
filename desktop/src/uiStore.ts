import { useSyncExternalStore } from "react";

export interface UiState {
  activeConversationId: string | null;
  sidebarOpen: boolean;
  toast: { id: number; message: string; recovery?: string } | null;
}

const initialState: UiState = { activeConversationId: null, sidebarOpen: true, toast: null };
let state = initialState;
let toastSequence = 0;
const listeners = new Set<() => void>();

function emit() { for (const listener of listeners) listener(); }
export const uiStore = {
  getSnapshot: () => state,
  subscribe(listener: () => void) { listeners.add(listener); return () => listeners.delete(listener); },
  setActiveConversation(activeConversationId: string | null) { state = { ...state, activeConversationId }; emit(); },
  setSidebarOpen(sidebarOpen: boolean) { state = { ...state, sidebarOpen }; emit(); },
  showError(message: string, recovery?: string) {
    state = { ...state, toast: { id: ++toastSequence, message: redactError(message), recovery } }; emit();
  },
  dismissToast() { if (state.toast) { state = { ...state, toast: null }; emit(); } },
  reset() { state = initialState; emit(); },
};

export function useUiStore<T>(selector: (value: UiState) => T): T {
  return selector(useSyncExternalStore(uiStore.subscribe, uiStore.getSnapshot, uiStore.getSnapshot));
}

export function redactError(value: unknown): string {
  const text = value instanceof Error ? value.message : String(value);
  return text
    .replace(/(?:[A-Za-z]:\\|\/)(?:[^\s:'"<>]+\/)*[^\s:'"<>]*/g, "[path]")
    .replace(/\b(?:sk|tp)-[A-Za-z0-9_-]{8,}\b/gi, "[redacted]")
    .replace(/(token|password|secret|authorization)\s*[:=]\s*[^\s,;]+/gi, "$1=[redacted]");
}
