import React from "react";
import { createRoot } from "react-dom/client";
import { BoruErrorBoundary, ErrorToast } from "./errors";
import "./style.css";

function App() {
  return <main><div className="mark">B</div><h1>Boru</h1><p>Peer-to-peer chat, without a central server.</p><span className="status">Desktop foundation</span></main>;
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode><BoruErrorBoundary><App /><ErrorToast /></BoruErrorBoundary></React.StrictMode>,
);
