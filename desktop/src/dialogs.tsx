import { rt } from "./i18n";
import * as React from "react";
import { Button, Dialog, DialogContent, DialogDescription, DialogTitle } from "./components/ui";

export type DialogKind = "confirmation" | "security" | "error" | "file";

type SharedDialogProps = {
  open: boolean; onOpenChange(open: boolean): void; title: string; description: string;
  children?: React.ReactNode; cancelLabel?: string; confirmLabel?: string;
  onConfirm?: () => void; destructive?: boolean;
};

/** Reusable, focus-trapped confirmation. Destructive actions require explicit activation. */
export function ConfirmationDialog({ open, onOpenChange, title, description, children, cancelLabel = rt("dialogs.cancel"), confirmLabel = rt("dialogs.confirm"), onConfirm, destructive = false }: SharedDialogProps) {
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent aria-describedby="confirmation-description" className="boru-dialog">
      <DialogTitle>{title}</DialogTitle><DialogDescription id="confirmation-description">{description}</DialogDescription>
      {children && <div className="boru-dialog-body">{children}</div>}
      <footer className="boru-dialog-actions"><Button variant="secondary" onClick={() => onOpenChange(false)}>{cancelLabel}</Button><Button variant={destructive ? "destructive" : "primary"} onClick={() => { onConfirm?.(); onOpenChange(false); }}>{confirmLabel}</Button></footer>
    </DialogContent>
  </Dialog>;
}

/** Security prompts deliberately cannot close via Escape, outside click, or close controls. */
export function SecurityApprovalDialog({ open, onOpenChange, title, description, children, confirmLabel = rt("dialogs.approve"), onConfirm }: Omit<SharedDialogProps, "cancelLabel" | "destructive">) {
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent aria-describedby="security-description" className="boru-dialog boru-dialog-security" onEscapeKeyDown={event => event.preventDefault()} onPointerDownOutside={event => event.preventDefault()} onInteractOutside={event => event.preventDefault()}>
      <DialogTitle>{title}</DialogTitle><DialogDescription id="security-description">{description}</DialogDescription>
      {children && <div className="boru-dialog-body">{children}</div>}
      <footer className="boru-dialog-actions"><Button variant="secondary" onClick={() => onOpenChange(false)}>{rt("dialogs.deny")}</Button><Button variant="destructive" onClick={() => { onConfirm?.(); onOpenChange(false); }}>{confirmLabel}</Button></footer>
    </DialogContent>
  </Dialog>;
}

export function ErrorDialog(props: Omit<SharedDialogProps, "onConfirm" | "confirmLabel" | "destructive">) {
  return <ConfirmationDialog {...props} cancelLabel={rt("dialogs.close")} confirmLabel={rt("dialogs.close")} />;
}
export function FileOperationDialog(props: SharedDialogProps) { return <ConfirmationDialog {...props} />; }

/** Interactive dialog/overlay showcase for design and keyboard regression checks. */
export function DialogGallery() {
  const [active, setActive] = React.useState<DialogKind | null>(null);
  const copy: Record<DialogKind, [string, string]> = {
    confirmation: [rt("dialogs.deleteTitle"), rt("dialogs.deleteDescription")], security: [rt("dialogs.approveTitle"), rt("dialogs.approveDescription")], error: [rt("dialogs.failedTitle"), rt("dialogs.failedDescription")], file: [rt("dialogs.replaceTitle"), rt("dialogs.replaceDescription")],
  };
  const [title, description] = active ? copy[active] : ["", ""];
  return <section className="dialog-gallery" aria-label={rt("dialogs.gallery")}><h2>{rt("dialogs.heading")}</h2><div className="dialog-gallery-actions">{(Object.keys(copy) as DialogKind[]).map(kind => <Button key={kind} variant="secondary" onClick={() => setActive(kind)}>{rt(`dialogs.${kind}`)}</Button>)}</div>
    {active === "security" ? <SecurityApprovalDialog open onOpenChange={open => { if (!open) setActive(null); }} title={title} description={description} confirmLabel={rt("dialogs.approvePeer")} /> : active === "error" ? <ErrorDialog open title={title} description={description} onOpenChange={open => { if (!open) setActive(null); }} /> : active === "file" ? <FileOperationDialog open title={title} description={description} cancelLabel={rt("dialogs.keepBoth")} confirmLabel={rt("dialogs.replace")} onConfirm={() => setActive(null)} onOpenChange={open => { if (!open) setActive(null); }} /> : <ConfirmationDialog open={active === "confirmation"} title={title} description={description} destructive onOpenChange={open => { if (!open) setActive(null); }} onConfirm={() => setActive(null)} confirmLabel={rt("dialogs.delete")} />}
  </section>;
}
