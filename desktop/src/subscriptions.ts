import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppSnapshotDto } from "./contract";

export interface EventCursor { epoch: string; revision: number }
export interface SnapshotState { snapshot: AppSnapshotDto; cursor: EventCursor }
export type SnapshotListener = (state: SnapshotState) => void;
export interface EventNotice extends EventCursor {
  eventId: string;
  kind?: string;
  messageId?: string;
  conversationId?: string;
}
type Replay = { kind: "events"; events: EventNotice[] } | { kind: "resnapshot"; currentRevision: number };

/** Process-wide, ref-counted event bridge. Route subscribers share one Tauri listener. */
class SubscriptionManager {
  private listeners = new Set<SnapshotListener>();
  private unlisten?: UnlistenFn;
  private generation = 0;
  private cursor?: EventCursor;
  private chain: Promise<void> = Promise.resolve();

  resync(): void { void this.refresh(); }

  subscribe(listener: SnapshotListener): () => void {
    this.listeners.add(listener);
    if (this.listeners.size === 1) void this.start();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) {
        this.generation++;
        this.unlisten?.();
        this.unlisten = undefined;
        this.cursor = undefined;
      }
    };
  }

  private async start(): Promise<void> {
    const generation = ++this.generation;
    try {
      // Attach before snapshot, then replay from its watermark to close the race.
      const stopEvents = await listen<EventNotice>("boru://ui-event", ({ payload }) => {
        this.chain = this.chain.then(() => this.onEvent(payload)).catch(() => this.refresh());
      });
      if (generation !== this.generation || !this.listeners.size) { stopEvents(); return; }
      const stopOverflow = await listen("boru://resnapshot-required", () => { void this.refresh(); });
      if (generation !== this.generation || !this.listeners.size) { stopEvents(); stopOverflow(); return; }
      this.unlisten = () => { stopEvents(); stopOverflow(); };
      await this.refresh();
    } catch {
      if (generation === this.generation) await this.refresh();
    }
  }

  private async refresh(): Promise<void> {
    if (!this.listeners.size) return;
    const generation = this.generation;
    const state = await invoke<SnapshotState>("get_app_snapshot");
    if (generation !== this.generation || !this.listeners.size) return;
    this.cursor = state.cursor;
    this.publish(state);
  }

  private async onEvent(event: EventNotice): Promise<void> {
    if (!this.listeners.size) return;
    if (!this.cursor || event.epoch !== this.cursor.epoch || event.revision > this.cursor.revision + 1) {
      await this.refresh();
      if (!this.cursor || event.epoch !== this.cursor.epoch) return;
    }
    if (event.revision <= this.cursor.revision) return;
    const replay = await invoke<Replay>("replay_ui_events", { cursor: this.cursor });
    if (replay.kind === "resnapshot") { await this.refresh(); return; }
    // Event payloads are state-change hints; always publish authoritative Rust state.
    await this.refresh();
  }

  private publish(state: SnapshotState): void {
    for (const listener of this.listeners) listener(state);
  }
}

export const appSubscriptions = new SubscriptionManager();
