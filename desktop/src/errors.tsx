import React, { Component, type ErrorInfo, type ReactNode } from "react";
import { redactError, uiStore } from "./uiStore";
import { rt } from "./i18n";

interface Props { children: ReactNode }
interface State { error: Error | null }

export class BoruErrorBoundary extends Component<Props, State> {
  state: State = { error: null };
  static getDerivedStateFromError(error: Error): State { return { error }; }
  componentDidCatch(error: Error, _info: ErrorInfo) {
    // Intentionally never log error stacks/component trees: they may contain local paths or user content.
    uiStore.showError(redactError(error), rt("errors.recovery"));
  }
  render() {
    if (!this.state.error) return this.props.children;
    return <main role="alert" className="error-boundary">
      <h1>{rt("errors.title")}</h1><p>{rt("errors.description")}</p>
      <button onClick={() => this.setState({ error: null })}>{rt("errors.retry")}</button>
      <button onClick={() => window.location.reload()}>{rt("errors.reload")}</button>
    </main>;
  }
}

export function ErrorToast() {
  const toast = React.useSyncExternalStore(uiStore.subscribe, uiStore.getSnapshot, uiStore.getSnapshot).toast;
  if (!toast) return null;
  return <aside className="error-toast" role="alert" aria-live="assertive">
    <span>{toast.message}</span>{toast.recovery && <p>{toast.recovery}</p>}
    <button aria-label={rt("errors.dismissLabel")} onClick={uiStore.dismissToast}>{rt("errors.dismiss")}</button>
  </aside>;
}
