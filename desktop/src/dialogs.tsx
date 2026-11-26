import * as React from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Dialog, DialogContent, DialogDescription, DialogTitle } from "./components/ui";

export type DialogKind = "confirmation" | "security" | "error" | "file";

type SharedDialogProps = {
  open: boolean; onOpenChange(open: boolean): void; title: string; description: string;
  children?: React.ReactNode; cancelLabel?: string; confirmLabel?: string;
  onConfirm?: () => void; destructive?: boolean;
};

/** Reusable, focus-trapped confirmation. Destructive actions require explicit activation. */
export function ConfirmationDialog({ open, onOpenChange, title, description, children, cancelLabel = "Cancel", confirmLabel = "Confirm", onConfirm, destructive = false }: SharedDialogProps) {
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent aria-describedby="confirmation-description" className="boru-dialog">
      <DialogTitle>{title}</DialogTitle><DialogDescription id="confirmation-description">{description}</DialogDescription>
      {children && <div className="boru-dialog-body">{children}</div>}
      <footer className="boru-dialog-actions"><Button variant="secondary" onClick={() => onOpenChange(false)}>{cancelLabel}</Button><Button variant={destructive ? "destructive" : "primary"} onClick={() => { onConfirm?.(); onOpenChange(false); }}>{confirmLabel}</Button></footer>
    </DialogContent>
  </Dialog>;
}

/** Security prompts deliberately cannot close via Escape, outside click, or close controls. */
export function SecurityApprovalDialog({ open, onOpenChange, title, description, children, confirmLabel = "Approve", onConfirm }: Omit<SharedDialogProps, "cancelLabel" | "destructive">) {
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent aria-describedby="security-description" className="boru-dialog boru-dialog-security" onEscapeKeyDown={event => event.preventDefault()} onPointerDownOutside={event => event.preventDefault()} onInteractOutside={event => event.preventDefault()}>
      <DialogTitle>{title}</DialogTitle><DialogDescription id="security-description">{description}</DialogDescription>
      {children && <div className="boru-dialog-body">{children}</div>}
      <footer className="boru-dialog-actions"><Button variant="secondary" onClick={() => onOpenChange(false)}>Deny</Button><Button variant="destructive" onClick={() => { onConfirm?.(); onOpenChange(false); }}>{confirmLabel}</Button></footer>
    </DialogContent>
  </Dialog>;
}

export function ErrorDialog(props: Omit<SharedDialogProps, "onConfirm" | "confirmLabel" | "destructive">) {
  return <ConfirmationDialog {...props} cancelLabel="Close" confirmLabel="Close" />;
}
export function FileOperationDialog(props: SharedDialogProps) { return <ConfirmationDialog {...props} />; }

interface LocalServiceDto { port: number; label: string; isHttp: boolean }
interface TrustedPeerDto { peerId: string; alias: string }

/** Create and share a local TCP tunnel with a trusted contact. */
export function CreateShareTunnelDialog({ open, onOpenChange }: Pick<SharedDialogProps, "open" | "onOpenChange">) {
  const [services, setServices] = React.useState<LocalServiceDto[]>([]);
  const [peers, setPeers] = React.useState<TrustedPeerDto[]>([]);
  const [scanState, setScanState] = React.useState<"loading" | "ready" | "error">("loading");
  const [peerState, setPeerState] = React.useState<"loading" | "ready" | "error">("loading");
  const [servicePort, setServicePort] = React.useState("");
  const [peerId, setPeerId] = React.useState("");
  const [peerQuery, setPeerQuery] = React.useState("");
  const [submitState, setSubmitState] = React.useState<"idle" | "submitting" | "success">("idle");
  const [submitError, setSubmitError] = React.useState("");
  const [expiry, setExpiry] = React.useState(3600);
  const requestId = React.useRef(0);

  React.useEffect(() => {
    if (!open) return;
    const id = ++requestId.current;
    setScanState("loading"); setPeerState("loading"); setSubmitState("idle"); setSubmitError("");
    setServices([]); setPeers([]); setServicePort(""); setPeerId(""); setPeerQuery("");
    void invoke<LocalServiceDto[]>("scan_local_services").then(value => {
      if (requestId.current !== id) return;
      setServices(value); setScanState("ready");
    }).catch(() => { if (requestId.current === id) setScanState("error"); });
    void invoke<TrustedPeerDto[]>("list_trusted_peers").then(value => {
      if (requestId.current !== id) return;
      setPeers(value); setPeerState("ready");
    }).catch(() => { if (requestId.current === id) setPeerState("error"); });
    return () => { requestId.current += 1; };
  }, [open]);

  const filteredPeers = peers.filter(peer => `${peer.alias} ${peer.peerId}`.toLowerCase().includes(peerQuery.trim().toLowerCase()));
  const hasService = services.some(service => String(service.port) === servicePort);
  const hasPeer = peers.some(peer => peer.peerId === peerId);
  const invalidService = scanState === "ready" && !hasService;
  const invalidPeer = peerState === "ready" && !hasPeer;
  const busy = submitState === "submitting";

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy || !hasService || !hasPeer) return;
    const service = services.find(item => String(item.port) === servicePort);
    if (!service) return;
    setSubmitState("submitting"); setSubmitError("");
    try {
      await invoke<void>("submit_tunnel_offer", {
        peerId, port: service.port, serviceName: service.label, isHttp: service.isHttp,
        expiresInSeconds: expiry, preferredLocalPort: null,
      });
      setSubmitState("success");
    } catch (error) {
      const reason = typeof error === "string" ? error : "";
      setSubmitError(reason === "untrusted_peer"
        ? "This contact is no longer trusted. Choose a trusted contact and try again."
        : "The tunnel offer could not be sent. Check your connection and try again.");
      setSubmitState("idle");
    }
  }

  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent aria-describedby="tunnel-dialog-description" className="boru-dialog">
      <DialogTitle>Create and share a tunnel</DialogTitle>
      <DialogDescription id="tunnel-dialog-description">Choose a local TCP service and a trusted contact to share it with.</DialogDescription>
      <form className="boru-dialog-body boru-tunnel-form" onSubmit={submit}>
        <label htmlFor="tunnel-service">Local service</label>
        {scanState === "loading" ? <p role="status">Scanning for local TCP services…</p> : scanState === "error" ? <p role="alert">Could not scan local services. Close and reopen this dialog to try again.</p> : services.length === 0 ? <p role="status">No local TCP services were found. Start a service and reopen this dialog.</p> : <>
          <select id="tunnel-service" value={servicePort} onChange={event => setServicePort(event.target.value)} aria-invalid={invalidService} aria-describedby="tunnel-service-error" required>
            <option value="">Select a service</option>{services.map(service => <option key={service.port} value={service.port}>{service.label} — port {service.port}{service.isHttp ? " (HTTP)" : ""}</option>)}
          </select>
          {invalidService && <p id="tunnel-service-error" role="alert">Select a local service to continue.</p>}
        </>}

        <label htmlFor="trusted-peer-search">Find trusted contact</label>
        <input id="trusted-peer-search" type="search" value={peerQuery} onChange={event => setPeerQuery(event.target.value)} placeholder="Search by name or peer ID" />
        {peerState === "loading" ? <p role="status">Loading trusted contacts…</p> : peerState === "error" ? <p role="alert">Could not load trusted contacts. Close and reopen this dialog to try again.</p> : peers.length === 0 ? <p role="status">No trusted contacts are available to share with.</p> : <>
          <label htmlFor="tunnel-peer">Trusted contact</label>
          <select id="tunnel-peer" value={peerId} onChange={event => setPeerId(event.target.value)} aria-invalid={invalidPeer} aria-describedby="tunnel-peer-error" required>
            <option value="">Select a trusted contact</option>{filteredPeers.map(peer => <option key={peer.peerId} value={peer.peerId}>{peer.alias} ({peer.peerId})</option>)}
          </select>
          {filteredPeers.length === 0 && <p role="status">No trusted contact matches that search.</p>}
          {invalidPeer && <p id="tunnel-peer-error" role="alert">Select a trusted contact to continue.</p>}
        </>}

        <label htmlFor="tunnel-expiry">Offer expires after</label>
        <select id="tunnel-expiry" value={expiry} onChange={event => setExpiry(Number(event.target.value))}>
          <option value={600}>10 minutes</option><option value={1800}>30 minutes</option><option value={3600}>1 hour</option><option value={28800}>8 hours</option>
        </select>
        {submitError && <p role="alert">{submitError}</p>}
        {submitState === "success" && <p role="status">Tunnel offer sent successfully.</p>}
        <footer className="boru-dialog-actions"><Button type="button" variant="secondary" onClick={() => onOpenChange(false)}>Cancel</Button><Button type="submit" disabled={!hasService || !hasPeer || busy || scanState !== "ready" || peerState !== "ready"}>{busy ? "Sending…" : "Share tunnel"}</Button></footer>
      </form>
    </DialogContent>
  </Dialog>;
}

/** Interactive dialog/overlay showcase for design and keyboard regression checks. */
export function DialogGallery() {
  const [active, setActive] = React.useState<DialogKind | null>(null);
  const copy: Record<DialogKind, [string, string]> = {
    confirmation: ["Delete conversation?", "This permanently removes the local conversation history."],
    security: ["Approve peer?", "Verify this peer's identity before granting access."],
    error: ["Operation failed", "The file could not be opened. Check that it is available and try again."],
    file: ["Replace existing file?", "A file with this name already exists in the selected destination."],
  };
  const [title, description] = active ? copy[active] : ["", ""];
  return <section className="dialog-gallery" aria-label="Dialog gallery"><h2>Dialogs &amp; overlays</h2><div className="dialog-gallery-actions">{(Object.keys(copy) as DialogKind[]).map(kind => <Button key={kind} variant="secondary" onClick={() => setActive(kind)}>{kind[0].toUpperCase() + kind.slice(1)}</Button>)}</div>
    {active === "security" ? <SecurityApprovalDialog open onOpenChange={open => { if (!open) setActive(null); }} title={title} description={description} confirmLabel="Approve peer" /> : active === "error" ? <ErrorDialog open title={title} description={description} onOpenChange={open => { if (!open) setActive(null); }} /> : active === "file" ? <FileOperationDialog open title={title} description={description} cancelLabel="Keep both" confirmLabel="Replace" onConfirm={() => setActive(null)} onOpenChange={open => { if (!open) setActive(null); }} /> : <ConfirmationDialog open={active === "confirmation"} title={title} description={description} destructive onOpenChange={open => { if (!open) setActive(null); }} onConfirm={() => setActive(null)} confirmLabel="Delete" />}
  </section>;
}
