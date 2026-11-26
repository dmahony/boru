import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppSnapshotDto, MessageDto, RoomDto } from "./contract";
import { createRoot } from "react-dom/client";
import { appSubscriptions, type SnapshotState } from "./subscriptions";
import { DialogGallery } from "./dialogs";
import { Toaster } from "./components/ui";
import { BoruErrorBoundary, ErrorToast } from "./errors";
import { MediaTransferError, readMediaBlob, saveMediaDownload, uploadMedia } from "./media-client";
import { runRealMediaBenchmark } from "./media-benchmark";
import { HomeDashboard } from "./HomeDashboard";
import { loadUiPreferences, saveUiPreferences, type UiPreferences } from "./uiPreferences";
import { PrivacyInformation } from "./privacy-onboarding";
import "./style.css";

type DesktopPreferences = { theme: "light" | "dark" | "amoled"; font_size: number; scale: number; accent: string | null; sound_enabled: boolean; show_presence_indicator: boolean; typing_indicators_enabled: boolean; share_direct_addresses: boolean; keep_running: boolean; notification_policy: "all" | "mentions_only" | "muted"; restart_required: string[] };
const defaultPreferences: DesktopPreferences = { theme: "dark", font_size: 16, scale: 100, accent: null, sound_enabled: true, show_presence_indicator: true, typing_indicators_enabled: true, share_direct_addresses: false, keep_running: false, notification_policy: "all", restart_required: ["Keep network host running after window close"] };
export function AppearanceSettings() {
  const [preferences, setPreferences] = useState(defaultPreferences);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => { void invoke<{ preferences: DesktopPreferences }>("get_settings").then(result => { setPreferences({ ...defaultPreferences, ...result.preferences, restart_required: result.preferences.restart_required ?? defaultPreferences.restart_required }); setReady(true); }).catch(cause => { setError(String(cause)); setReady(true); }); }, []);
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = preferences.theme;
    root.style.setProperty("--user-font-size", `${preferences.font_size}px`);
    root.style.setProperty("--user-scale", String(preferences.scale / 100));
    root.style.setProperty("--user-accent", preferences.accent || "#79d6bd");
  }, [preferences]);
  const writeQueue = useRef<Promise<unknown>>(Promise.resolve());
  async function update(next: DesktopPreferences) {
    setPreferences(next);
    const write = writeQueue.current.then(() => invoke("update_settings", { preferences: next }));
    writeQueue.current = write.catch(() => {});
    try { await write; setError(""); }
    catch (cause) { setError(String(cause)); }
  }
  return <section className="appearance-settings" aria-label="Appearance settings"><h2>Appearance</h2>
    {!ready ? <p>Loading saved preferences…</p> : <div className="settings-controls">
      <label>Theme <select value={preferences.theme} onChange={event => void update({ ...preferences, theme: event.target.value as DesktopPreferences["theme"] })}><option value="light">Light</option><option value="dark">Dark</option><option value="amoled">AMOLED</option></select></label>
      <label>Font size <input type="range" min="12" max="24" value={preferences.font_size} onChange={event => void update({ ...preferences, font_size: Number(event.target.value) })} /> {preferences.font_size}px</label>
      <label>Scale <input type="range" min="80" max="140" step="5" value={preferences.scale} onChange={event => void update({ ...preferences, scale: Number(event.target.value) })} /> {preferences.scale}%</label>
      <label>Accent <input type="color" value={preferences.accent || "#79d6bd"} onChange={event => void update({ ...preferences, accent: event.target.value })} /></label>
      <h3>Notifications & privacy</h3>
      <label>Notification policy <select value={preferences.notification_policy} onChange={event => void update({ ...preferences, notification_policy: event.target.value as DesktopPreferences["notification_policy"] })}><option value="all">All messages</option><option value="mentions_only">Mentions only</option><option value="muted">Muted</option></select></label>
      <label><input type="checkbox" checked={preferences.sound_enabled} onChange={event => void update({ ...preferences, sound_enabled: event.target.checked })} /> Notification sounds</label>
      <label><input type="checkbox" checked={preferences.show_presence_indicator} onChange={event => void update({ ...preferences, show_presence_indicator: event.target.checked })} /> Show presence indicator</label>
      <label><input type="checkbox" checked={preferences.typing_indicators_enabled} onChange={event => void update({ ...preferences, typing_indicators_enabled: event.target.checked })} /> Send typing indicators</label>
      <label><input type="checkbox" checked={preferences.share_direct_addresses} onChange={event => void update({ ...preferences, share_direct_addresses: event.target.checked })} /> Share direct addresses in invitations</label>
      <h3>Network</h3>
      <label><input type="checkbox" checked={preferences.keep_running} onChange={event => void update({ ...preferences, keep_running: event.target.checked })} /> Keep network host running after closing the window <small>(restart required)</small></label>
      <p role="note">Restart-required options: {preferences.restart_required.join(", ")}. Command-line and environment overrides take precedence over saved preferences.</p>
    </div>}{error && <p className="error" role="alert">Settings save failed: {error}</p>}<p>Appearance changes apply immediately. Network and identity settings are managed by the existing Boru configuration; changes requiring restart are shown in their respective controls.</p>
  </section>;
}

type MediaItem = { handle: string; contentType: string; size: number; name: string; progress?: { done: number; total: number }; error?: string; busy?: boolean; previewUrl?: string };
function errorText(cause: unknown): string {
  return cause instanceof MediaTransferError ? `${cause.code}: ${cause.message}` : cause instanceof DOMException && cause.name === "AbortError" ? "Transfer cancelled" : String(cause);
}
function MediaPanel() {
  const input = useRef<HTMLInputElement>(null);
  const [items, setItems] = useState<MediaItem[]>([]);
  const [handleInput, setHandleInput] = useState("");
  const [typeInput, setTypeInput] = useState("image/png");
  const [sizeInput, setSizeInput] = useState("0");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const cancel = useRef<AbortController | null>(null);
  const objectUrls = useRef(new Set<string>());
  useEffect(() => () => { for (const url of objectUrls.current) URL.revokeObjectURL(url); }, []);
  function update(handle: string, changes: Partial<MediaItem>) { setItems(current => current.map(item => item.handle === handle ? { ...item, ...changes } : item)); }
  function addItem(item: MediaItem) { setItems(current => [item, ...current.filter(existing => existing.handle !== item.handle)]); }
  async function selectFile(file?: File) {
    if (!file) return;
    setError(""); setBusy(true);
    const controller = new AbortController(); cancel.current = controller;
    try {
      const result = await uploadMedia(file, () => {}, controller.signal);
      addItem({ handle: result.handle, name: file.name, size: result.size, contentType: file.type });
    } catch (cause) { setError(errorText(cause)); }
    finally { setBusy(false); cancel.current = null; }
  }
  function addHandle(event: React.FormEvent) {
    event.preventDefault();
    const handle = handleInput.trim();
    if (!handle) return;
    addItem({ handle, name: `${handle.slice(0, 12)}.bin`, contentType: typeInput.trim() || "application/octet-stream", size: Math.max(0, Number(sizeInput) || 0) });
    setHandleInput("");
  }
  async function run(item: MediaItem, action: "preview" | "download") {
    if (item.busy || items.some(candidate => candidate.busy)) return;
    const controller = new AbortController(); cancel.current = controller;
    update(item.handle, { busy: true, error: undefined, progress: { done: 0, total: item.size } });
    const progress = ({ transferredBytes, totalBytes }: { transferredBytes: number; totalBytes: number }) => update(item.handle, { progress: { done: transferredBytes, total: totalBytes || item.size } });
    try {
      if (action === "preview") {
        const blob = await readMediaBlob(item.handle, item.contentType, progress, controller.signal);
        const url = URL.createObjectURL(blob);
        const previous = items.find(candidate => candidate.handle === item.handle)?.previewUrl;
        if (previous) { URL.revokeObjectURL(previous); objectUrls.current.delete(previous); }
        objectUrls.current.add(url);
        update(item.handle, { previewUrl: url, size: blob.size });
      } else await saveMediaDownload(item.handle, item.name, item.contentType, progress, controller.signal);
    } catch (cause) { update(item.handle, { error: errorText(cause) }); }
    finally { update(item.handle, { busy: false }); cancel.current = null; }
  }
  return <section className="media-panel" aria-label="Media transfer" aria-labelledby="media-transfer-heading">
    <h2 id="media-transfer-heading">Media transfer</h2>
    <p>Upload media or open an opaque handle to preview and download it.</p>
    <input ref={input} hidden type="file" accept="image/*,video/*,audio/*" onChange={event => { void selectFile(event.currentTarget.files?.[0]); event.currentTarget.value = ""; }} />
    <button disabled={busy} onClick={() => input.current?.click()}>Choose media</button>
    <form className="handle-form" onSubmit={addHandle}>
      <input aria-label="Opaque media handle" placeholder="Paste media handle" value={handleInput} onChange={event => setHandleInput(event.target.value)} />
      <input aria-label="Media type" value={typeInput} onChange={event => setTypeInput(event.target.value)} />
      <input aria-label="Media size in bytes" type="number" min="0" value={sizeInput} onChange={event => setSizeInput(event.target.value)} />
      <button disabled={!handleInput.trim() || busy}>Add handle</button>
    </form>
    {items.length === 0 ? <p className="empty">No media handles yet. Upload a file or paste a handle.</p> : <div className="gallery">
      {items.map(item => <article className="media-item" key={item.handle}>
        <div className="preview">{item.previewUrl ? <img src={item.previewUrl} alt={`Preview of ${item.name}`} /> : item.contentType.startsWith("image/") ? <span>Image preview not loaded</span> : <span>▧ {item.contentType}</span>}</div>
        <div className="media-meta"><strong>{item.name}</strong><code>{item.handle}</code><span>{item.contentType} · {item.size.toLocaleString()} bytes</span></div>
        <div className="actions">
          {item.contentType.startsWith("image/") && <button disabled={item.busy || busy || items.some(candidate => candidate.busy)} onClick={() => void run(item, "preview")}>{item.previewUrl ? "Reload preview" : "Preview"}</button>}
          <button disabled={item.busy || busy || items.some(candidate => candidate.busy)} onClick={() => void run(item, "download")}>{item.busy ? "Working…" : "Download"}</button>
          {item.busy && <><progress aria-label={`Download progress for ${item.name}`} max={item.progress?.total || 1} value={item.progress?.done || 0} /><span>{(item.progress?.done || 0).toLocaleString()} / {(item.progress?.total || 0).toLocaleString()} bytes</span><button onClick={() => cancel.current?.abort()}>Cancel</button></>}
        </div>
        {item.error && <p role="alert" className="error">{item.error}</p>}
      </article>)}
    </div>}
    {error && <p role="alert" className="error">{error}</p>}
  </section>;
}
export function RoomView({ snapshot }: { snapshot: AppSnapshotDto }) {
  const rooms = snapshot.rooms;
  const [selected, setSelected] = useState<RoomDto | null>(null);
  const [messages, setMessages] = useState<MessageDto[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  useEffect(() => { void loadUiPreferences().then(saved => {
    const restored = rooms.find(room => room.id === saved.activeConversationId);
    if (restored) { setSelected(restored); void openRoom(restored); }
    setDraft(saved.drafts[saved.activeConversationId ?? ""] ?? "");
  }); }, []);
  function updateDraft(value: string) {
    setDraft(value);
    void loadUiPreferences().then(saved => saveUiPreferences({ ...saved, drafts: { ...saved.drafts, [selected?.id ?? ""]: value }, activeConversationId: selected?.id ?? null })).catch(() => {});
  }
  async function loadRoom(room: RoomDto) {
    const opened = await invoke<{ messages: MessageDto[] }>("open_room", { conversationId: room.id });
    setMessages(opened.messages.slice().reverse());
  }
  async function openRoom(room: RoomDto) {
    setSelected(room); setLoading(true); setError("");
    void loadUiPreferences().then(saved => saveUiPreferences({ ...saved, activeConversationId: room.id })).catch(() => {});
    try {
      await loadRoom(room);
      const conversationId = Array.from(room.id.matchAll(/../g), match => Number.parseInt(match[0], 16));
      await invoke("mark_read", { request: { conversationId } });
      appSubscriptions.resync();
    } catch (cause) { setError(String(cause)); setMessages([]); }
    finally { setLoading(false); }
  }
  useEffect(() => {
    if (!selected) return;
    let current = true;
    void loadRoom(selected).catch(cause => { if (current) setError(String(cause)); });
    return () => { current = false; };
  }, [snapshot]);
  async function send(event: React.FormEvent) {
    event.preventDefault();
    const text = draft.trim(); if (!text || !selected || sending) return;
    setSending(true); setError("");
    try {
      const conversationId = Array.from(selected.id.matchAll(/../g), match => Number.parseInt(match[0], 16));
      await invoke("send_message", { args: { request: { idempotencyKey: crypto.randomUUID(), conversationId, text, threadTarget: null } } });
      setDraft("");
      void loadUiPreferences().then(saved => { const drafts = { ...saved.drafts }; delete drafts[selected.id]; return saveUiPreferences({ ...saved, drafts }); });
      appSubscriptions.resync();
    } catch (cause) { setError(String(cause)); }
    finally { setSending(false); }
  }
  const unread = (id: string) => snapshot.unread.find(item => item.conversation_id === id)?.count ?? 0;
  return <section className="chat-shell" aria-label="Rooms" aria-roledescription="chat">
    <nav className="room-list" aria-label="Room navigation"><h2>Rooms</h2>{rooms.length === 0 ? <p className="empty">No rooms yet.</p> : rooms.map(room => <button key={room.id} className={selected?.id === room.id ? "room active" : "room"} onClick={() => void openRoom(room)}><span>{room.name || room.id.slice(0, 12)}</span>{unread(room.id) > 0 && <b aria-label={`${unread(room.id)} unread`}>{unread(room.id)}</b>}</button>)}</nav>
    <section className="room-chat" aria-label="Room conversation">{selected ? <><header><h2>{selected.name || selected.id}</h2><small>{selected.participant_count ?? ""} participants</small></header><div className="messages" aria-live="polite">{loading ? <p>Loading messages…</p> : messages.length === 0 ? <p className="empty">No messages yet. Start the conversation.</p> : messages.map(message => <article className="message" key={message.id}><p>{message.body}</p><small>{message.sender_id === snapshot.identity?.id ? "You" : message.sender_id.slice(0, 12)} · {new Date(message.timestamp_ms).toLocaleTimeString()} {message.delivery_status && <span className="delivery">{message.delivery_status}</span>}</small></article>)}</div><form className="composer" onSubmit={send}><input aria-label="Message" placeholder="Write a message…" value={draft} onChange={event => updateDraft(event.target.value)} onKeyDown={event => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit(); } }} /><button disabled={!draft.trim() || sending}>{sending ? "Sending…" : "Send"}</button></form></> : <div className="room-placeholder">Choose a room to view its history.</div>}{error && <p className="error" role="alert">{error}</p>}</section>
  </section>;
}

function BenchmarkPanel() {
  const canvas = useRef<HTMLCanvasElement>(null), gallery = useRef<HTMLDivElement>(null), video = useRef<HTMLDivElement>(null);
  const [running, setRunning] = useState(false); const [result, setResult] = useState("");
  async function run() { if (!canvas.current || !gallery.current || !video.current) return; setRunning(true); setResult(""); try { setResult(JSON.stringify(await runRealMediaBenchmark(canvas.current, gallery.current, video.current), null, 2)); } catch (error) { setResult(JSON.stringify({ error: String(error), stack: error instanceof Error ? error.stack : null }, null, 2)); } finally { setRunning(false); } }
  function exportJson() { const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([result], { type: "application/json" })); a.download = "media-rendering-linux-webkitgtk.json"; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 1000); }
  return <section aria-label="Benchmark"><h2>WebKitGTK media benchmark (test surface)</h2><button disabled={running} onClick={() => void run()}>{running ? "Benchmark running…" : "Run 5-trial benchmark"}</button>{result && <button onClick={exportJson}>Export raw JSON</button>}<p>One warm-up followed by five trials; media bytes use the real opaque-handle IPC.</p><canvas ref={canvas} width={640} height={360} /><div ref={gallery} className="gallery" /><div ref={video} /><pre id="benchmark-results">{result}</pre></section>;
}
function WorkspaceApp() {
  const [state, setState] = useState<SnapshotState>();
  const [section, setSection] = useState<"chats" | "media" | "settings">("chats");
  const [prefsReady, setPrefsReady] = useState(false);
  useEffect(() => {
    let mounted = true;
    void loadUiPreferences().then(saved => {
      if (!mounted) return;
      if (saved.activeNavigation === "chats" || saved.activeNavigation === "media" || saved.activeNavigation === "settings") setSection(saved.activeNavigation);
      setPrefsReady(true);
    });
    return () => { mounted = false; };
  }, []);
  useEffect(() => {
    if (!prefsReady) return;
    const persist = () => { void loadUiPreferences().then(saved => saveUiPreferences({ ...saved, activeNavigation: section })); };
    persist();
  }, [section, prefsReady]);
  const endpointId = state?.snapshot?.identity?.id ?? null;
  useEffect(() => appSubscriptions.subscribe(setState), []);
  useEffect(() => {
    const resume = () => appSubscriptions.resync();
    window.addEventListener("focus", resume);
    document.addEventListener("visibilitychange", resume);
    return () => { window.removeEventListener("focus", resume); document.removeEventListener("visibilitychange", resume); };
  }, []);
  return <main className="app-shell">
    <header className="titlebar"><div className="brand"><div className="mark" aria-hidden="true">B</div><span>Boru</span></div><span className="titlebar-caption">Peer-to-peer, by design</span>{endpointId && <code aria-label="Endpoint ID">{endpointId}</code>}<span className="status" role="status">{state ? "Connected" : "Connecting…"}</span></header>
    <div className="workspace">
      <nav className="navigation" aria-label="Main navigation">
        <span className="nav-label">WORKSPACE</span>
        <button className={section === "chats" ? "nav-item active" : "nav-item"} aria-current={section === "chats" ? "page" : undefined} onClick={() => setSection("chats")}><span aria-hidden="true">◉</span> Chats</button>
        <button className={section === "media" ? "nav-item active" : "nav-item"} aria-current={section === "media" ? "page" : undefined} onClick={() => setSection("media")}><span aria-hidden="true">▧</span> Shared media</button>
        <button className={section === "settings" ? "nav-item active" : "nav-item"} aria-current={section === "settings" ? "page" : undefined} onClick={() => setSection("settings")}><span aria-hidden="true">⚙</span> Settings</button>
        <div className="nav-footer"><span className="presence-dot" aria-hidden="true" />{state ? "Connected to Boru" : "Connecting to Boru"}</div>
      </nav>
      <section className="main-content" aria-label={section === "chats" ? "Chats" : section === "media" ? "Shared media" : "Settings"}>
        {section === "chats" ? state ? <><HomeDashboard snapshot={state.snapshot} onOpenRoom={() => document.querySelector(".room-list")?.scrollIntoView({ behavior: "smooth" })} onOpenMedia={() => setSection("media")} /><RoomView snapshot={state.snapshot} /></> : <div className="loading-state" role="status">Connecting to your conversations…</div> : section === "media" ? <MediaPanel /> : <AppearanceSettings />}
        {import.meta.env.VITE_MEDIA_BENCHMARK === "1" && <BenchmarkPanel />}
        <DialogGallery />
      </section>
    </div>
  </main>;
}

function IdentityGate() {
  type Startup = { state: "no_identity" | "existing" | "selected"; endpointId?: string; displayName?: string };
  type Profile = { endpointId: string; displayName: string; hasExistingIdentity: boolean };
  const [startup, setStartup] = useState<Startup>();
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [displayName, setDisplayName] = useState("");
  const [confirmCreate, setConfirmCreate] = useState(false);
  const [active, setActive] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [privacyOpen, setPrivacyOpen] = useState(false);
  async function probe() {
    setError("");
    try {
      const result = await invoke<Startup>("get_startup_state");
      setStartup(result);
      if (result.state === "existing") setProfiles(await invoke<Profile[]>("list_profiles"));
      else if (result.state === "selected") setActive(true);
    } catch {
      // Backend errors can contain key material or paths; surface a safe message only.
      setError("Boru couldn't check your saved profiles.");
    }
  }
  useEffect(() => { void probe(); }, []);
  async function select(endpointId: string) {
    setBusy(true); setError("");
    try { await invoke<string>("select_identity", { endpointId }); setActive(true); }
    catch { setError("Boru couldn't open that profile. Nothing was changed."); }
    finally { setBusy(false); }
  }
  async function create() {
    setBusy(true); setError("");
    try {
      await invoke<string>("create_identity", { displayName: displayName.trim() || null });
      setConfirmCreate(false); setActive(true);
    } catch { setError("Boru couldn't create a profile. Nothing was changed."); }
    finally { setBusy(false); }
  }
  if (active) return <WorkspaceApp />;
  if (!startup) return <main className="identity-screen"><h1>Boru</h1>{error ? <><p role="alert">{error}</p><button onClick={() => void probe()}>Try again</button></> : <p role="status">Checking profiles…</p>}</main>;
  return <main className="identity-screen"><section className="identity-card" aria-labelledby="identity-title">
    <div className="mark" aria-hidden="true">B</div><h1 id="identity-title">{startup.state === "existing" ? "Choose a profile" : "Welcome to Boru"}</h1>
    <p>Private, peer-to-peer messaging. Your identity stays on this device.</p>
    {startup.state === "existing" ? <><h2>Existing profiles</h2>{profiles.length ? <ul className="profile-list">{profiles.map(profile => <li key={profile.endpointId}><div><strong>{profile.displayName || "Boru profile"}</strong><code>{profile.endpointId}</code></div><button disabled={busy} onClick={() => void select(profile.endpointId)}>Open profile</button></li>)}</ul> : <p>No existing profiles found.</p>}</> : <>
      <label htmlFor="profile-name">Display name <span>(optional)</span></label><input id="profile-name" maxLength={80} value={displayName} onChange={event => setDisplayName(event.target.value)} placeholder="How others will see you" />
      <p className="privacy-copy">A new profile creates a local cryptographic identity. Your secret key is stored locally and is never shown in the app.</p>
      <button disabled={busy} onClick={() => setConfirmCreate(true)}>Create profile</button>
      {startup.state === "no_identity" && profiles.length > 0 && <button className="secondary" onClick={() => setStartup({ state: "existing" })}>Back to existing profiles</button>}
    </>}
    <button className="privacy-link" onClick={() => setPrivacyOpen(value => !value)}>{privacyOpen ? "Hide privacy information" : "Privacy information"}</button>
    {privacyOpen && <PrivacyInformation onContinue={() => setPrivacyOpen(false)} />}
    {error && <p className="error" role="alert">{error}</p>}
  </section>
  {confirmCreate && <div className="identity-confirm-backdrop"><section role="dialog" aria-modal="true" aria-labelledby="confirm-title" className="identity-confirm"><h2 id="confirm-title">Create a new identity?</h2><p>This makes a new local identity and cannot be undone. Your profile name is {displayName.trim() ? `“${displayName.trim()}”` : "not set"}.</p><button disabled={busy} onClick={() => void create()}>Confirm and create</button><button className="secondary" disabled={busy} onClick={() => setConfirmCreate(false)}>Cancel</button></section></div>}
  </main>;
}
export function App() { return <BoruErrorBoundary><IdentityGate /><ErrorToast /><Toaster position="top-right" richColors /></BoruErrorBoundary>; }

createRoot(document.getElementById("root")!).render(
  <React.StrictMode><App /></React.StrictMode>,
);
