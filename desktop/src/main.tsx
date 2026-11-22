import React, { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { MediaTransferError, readMediaBlob, saveMediaDownload, uploadMedia } from "./media-client";
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
  return <section className="media-panel" aria-label="Media transfer">
    <h2>Media transfer</h2>
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
          {item.busy && <><progress max={item.progress?.total || 1} value={item.progress?.done || 0} /><span>{(item.progress?.done || 0).toLocaleString()} / {(item.progress?.total || 0).toLocaleString()} bytes</span><button onClick={() => cancel.current?.abort()}>Cancel</button></>}
        </div>
        {item.error && <p role="alert" className="error">{item.error}</p>}
      </article>)}
    </div>}
    {error && <p role="alert" className="error">{error}</p>}
  </section>;
}
function App() {
  return <main><div className="mark">B</div><h1>Boru</h1><p>Peer-to-peer chat, without a central server.</p><span className="status">Desktop foundation</span><MediaPanel /></main>;
}

createRoot(document.getElementById("root")!).render(<React.StrictMode><App /></React.StrictMode>);
