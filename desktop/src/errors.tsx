import React, { Component, type ErrorInfo, type ReactNode } from "react";
import { redactError, uiStore } from "./uiStore";

interface Props { children: ReactNode }
interface State { error: Error | null }

export class BoruErrorBoundary extends Component<Props, State> {
  state: State = { error: null };
  static getDerivedStateFromError(error: Error): State { return { error }; }
  componentDidCatch(error: Error, _info: ErrorInfo) {
    // Intentionally never log error stacks/component trees: they may contain local paths or user content.
    uiStore.showError(redactError(error), "Reload the view. If the problem persists, restart Boru.");
  }
  render() {
    if (!this.state.error) return this.props.children;
    return <main role="alert" className="error-boundary">
      <h1>Something went wrong</h1><p>This view could not be displayed.</p>
      <button onClick={() => this.setState({ error: null })}>Try again</button>
      <button onClick={() => window.location.reload()}>Reload Boru</button>
    </main>;
  }
}

export function ErrorToast() {
  const toast = React.useSyncExternalStore(uiStore.subscribe, uiStore.getSnapshot, uiStore.getSnapshot).toast;
  if (!toast) return null;
  return <aside className="error-toast" role="alert" aria-live="assertive">
    <span>{toast.message}</span>{toast.recovery && <p>{toast.recovery}</p>}
    <button aria-label="Dismiss error" onClick={uiStore.dismissToast}>Dismiss</button>
  </aside>;
}
