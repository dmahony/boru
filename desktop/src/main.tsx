import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { appSubscriptions, type SnapshotState } from "./subscriptions";
import { BoruErrorBoundary, ErrorToast } from "./errors";
import "./style.css";

function App() {
  const [state, setState] = useState<SnapshotState>();
  useEffect(() => appSubscriptions.subscribe(setState), []);
  useEffect(() => {
    const resume = () => appSubscriptions.resync();
    window.addEventListener("focus", resume);
    document.addEventListener("visibilitychange", resume);
    return () => { window.removeEventListener("focus", resume); document.removeEventListener("visibilitychange", resume); };
  }, []);
  return <main><div className="mark">B</div><h1>Boru</h1>
    <p>Peer-to-peer chat, without a central server.</p>
    <span className="status">{state ? `Connected · revision ${state.cursor.revision}` : "Connecting…"}</span>
    {state && <pre aria-label="Authoritative application snapshot">{JSON.stringify(state.snapshot, null, 2)}</pre>}
  </main>;
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode><BoruErrorBoundary><App /><ErrorToast /></BoruErrorBoundary></React.StrictMode>,
);
