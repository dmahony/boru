import React, { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { MediaTransferError, uploadMedia } from "./media-client";
import "./style.css";

function MediaPanel() {
  const input = useRef<HTMLInputElement>(null);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [handle, setHandle] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const cancel = useRef<AbortController | null>(null);
  async function selectFile(file?: File) {
    if (!file) return;
    setError(""); setHandle(""); setBusy(true); setProgress({ done: 0, total: file.size });
    const controller = new AbortController(); cancel.current = controller;
    try {
      const result = await uploadMedia(file, ({ transferredBytes, totalBytes }) => setProgress({ done: transferredBytes, total: totalBytes }), controller.signal);
      setHandle(result.handle);
    } catch (cause) {
      setError(cause instanceof MediaTransferError ? `${cause.code}: ${cause.message}` : cause instanceof DOMException && cause.name === "AbortError" ? "Transfer cancelled" : String(cause));
    } finally { setBusy(false); cancel.current = null; }
  }
  const percent = progress?.total ? Math.floor(progress.done * 100 / progress.total) : 0;
  return <section className="media-panel" aria-label="Media transfer">
    <h2>Media transfer</h2>
    <p>Select a media file to upload using bounded, cancellable chunks.</p>
    <input ref={input} hidden type="file" accept="image/*,video/*,audio/*" onChange={event => { void selectFile(event.currentTarget.files?.[0]); event.currentTarget.value = ""; }} />
    <button disabled={busy} onClick={() => input.current?.click()}>Choose media</button>
    {busy && <><progress max={100} value={percent} /><span>{percent}% ({progress?.done.toLocaleString()} / {progress?.total.toLocaleString()} bytes)</span><button onClick={() => cancel.current?.abort()}>Cancel</button></>}
    {handle && <p className="handle">Uploaded. Opaque handle: <code>{handle}</code></p>}
    {error && <p role="alert" className="error">{error}</p>}
  </section>;
}
function App() {
  return <main><div className="mark">B</div><h1>Boru</h1><p>Peer-to-peer chat, without a central server.</p><span className="status">Desktop foundation</span><MediaPanel /></main>;
}

createRoot(document.getElementById("root")!).render(<React.StrictMode><App /></React.StrictMode>);
