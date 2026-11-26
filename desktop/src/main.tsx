import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppSnapshotDto, MessageDto, RoomDto } from "./contract";
import { createRoot } from "react-dom/client";
import { appSubscriptions, type SnapshotState } from "./subscriptions";
import { BoruErrorBoundary, ErrorToast } from "./errors";
import { MediaTransferError, readMediaBlob, saveMediaDownload, uploadMedia } from "./media-client";
import { runRealMediaBenchmark } from "./media-benchmark";
import "./style.css";

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
  async function loadRoom(room: RoomDto) {
    const opened = await invoke<{ messages: MessageDto[] }>("open_room", { conversationId: room.id });
    setMessages(opened.messages.slice().reverse());
  }
  async function openRoom(room: RoomDto) {
    setSelected(room); setLoading(true); setError("");
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
      appSubscriptions.resync();
    } catch (cause) { setError(String(cause)); }
    finally { setSending(false); }
  }
  const unread = (id: string) => snapshot.unread.find(item => item.conversation_id === id)?.count ?? 0;
  return <section className="chat-shell" aria-label="Rooms" aria-roledescription="chat">
    <nav className="room-list" aria-label="Room navigation"><h2>Rooms</h2>{rooms.length === 0 ? <p className="empty">No rooms yet.</p> : rooms.map(room => <button key={room.id} className={selected?.id === room.id ? "room active" : "room"} onClick={() => void openRoom(room)}><span>{room.name || room.id.slice(0, 12)}</span>{unread(room.id) > 0 && <b aria-label={`${unread(room.id)} unread`}>{unread(room.id)}</b>}</button>)}</nav>
    <section className="room-chat" aria-label="Room conversation">{selected ? <><header><h2>{selected.name || selected.id}</h2><small>{selected.participant_count ?? ""} participants</small></header><div className="messages" aria-live="polite">{loading ? <p>Loading messages…</p> : messages.length === 0 ? <p className="empty">No messages yet. Start the conversation.</p> : messages.map(message => <article className="message" key={message.id}><p>{message.body}</p><small>{message.sender_id === snapshot.identity?.id ? "You" : message.sender_id.slice(0, 12)} · {new Date(message.timestamp_ms).toLocaleTimeString()} {message.delivery_status && <span className="delivery">{message.delivery_status}</span>}</small></article>)}</div><form className="composer" onSubmit={send}><input aria-label="Message" placeholder="Write a message…" value={draft} onChange={event => setDraft(event.target.value)} onKeyDown={event => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit(); } }} /><button disabled={!draft.trim() || sending}>{sending ? "Sending…" : "Send"}</button></form></> : <div className="room-placeholder">Choose a room to view its history.</div>}{error && <p className="error" role="alert">{error}</p>}</section>
  </section>;
}

function BenchmarkPanel() {
  const canvas = useRef<HTMLCanvasElement>(null), gallery = useRef<HTMLDivElement>(null), video = useRef<HTMLDivElement>(null);
  const [running, setRunning] = useState(false); const [result, setResult] = useState("");
  async function run() { if (!canvas.current || !gallery.current || !video.current) return; setRunning(true); setResult(""); try { setResult(JSON.stringify(await runRealMediaBenchmark(canvas.current, gallery.current, video.current), null, 2)); } catch (error) { setResult(JSON.stringify({ error: String(error), stack: error instanceof Error ? error.stack : null }, null, 2)); } finally { setRunning(false); } }
  function exportJson() { const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([result], { type: "application/json" })); a.download = "media-rendering-linux-webkitgtk.json"; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 1000); }
  return <section aria-label="Benchmark"><h2>WebKitGTK media benchmark (test surface)</h2><button disabled={running} onClick={() => void run()}>{running ? "Benchmark running…" : "Run 5-trial benchmark"}</button>{result && <button onClick={exportJson}>Export raw JSON</button>}<p>One warm-up followed by five trials; media bytes use the real opaque-handle IPC.</p><canvas ref={canvas} width={640} height={360} /><div ref={gallery} className="gallery" /><div ref={video} /><pre id="benchmark-results">{result}</pre></section>;
}
function App() {
  const [state, setState] = useState<SnapshotState>();
  useEffect(() => appSubscriptions.subscribe(setState), []);
  useEffect(() => {
    const resume = () => appSubscriptions.resync();
    window.addEventListener("focus", resume);
    document.addEventListener("visibilitychange", resume);
    return () => { window.removeEventListener("focus", resume); document.removeEventListener("visibilitychange", resume); };
  }, []);
  return <main><div className="mark">B</div><h1>Boru</h1><p>Peer-to-peer chat, without a central server.</p>
    <span className="status">{state ? `Connected · revision ${state.cursor.revision}` : "Connecting…"}</span>
    {state && <RoomView snapshot={state.snapshot} />}
    {import.meta.env.VITE_MEDIA_BENCHMARK === "1" && <BenchmarkPanel />}
    <MediaPanel />
  </main>;
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode><BoruErrorBoundary><App /><ErrorToast /></BoruErrorBoundary></React.StrictMode>,
);
