import * as React from "react";
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
