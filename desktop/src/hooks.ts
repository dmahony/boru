import { useCallback, useEffect, useRef, useState } from "react";
import { redactError, uiStore } from "./uiStore";

export type BoruQueryState<T> = { data: T | undefined; loading: boolean; error: Error | null; refetch: () => void };

/** Query state is view-local; callers load authoritative snapshots from Rust. */
export function useBoruQuery<T>(key: string, loader: (signal: AbortSignal) => Promise<T>): BoruQueryState<T> {
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<{ data: T | undefined; loading: boolean; error: Error | null }>({ data: undefined, loading: true, error: null });
  const requestId = useRef(0);
  const loaderRef = useRef(loader);
  loaderRef.current = loader;
  const refetch = useCallback(() => setRevision((n) => n + 1), []);

  useEffect(() => {
    const controller = new AbortController();
    const id = ++requestId.current;
    setState((previous) => ({ ...previous, loading: true, error: null }));
    void loaderRef.current(controller.signal).then((data) => {
      if (!controller.signal.aborted && requestId.current === id) setState({ data, loading: false, error: null });
    }).catch((cause: unknown) => {
      if (controller.signal.aborted || requestId.current !== id) return;
      const error = new Error(redactError(cause));
      setState((previous) => ({ ...previous, loading: false, error }));
      uiStore.showError(error.message, "Retry the request. If it continues, restart Boru.");
    });
    return () => { controller.abort(); requestId.current++; };
  }, [key, revision]);

  return { ...state, refetch };
}

export interface BoruCommandState<R> { pending: boolean; error: Error | null; run: (...args: never[]) => Promise<R | undefined>; reset: () => void }

/** Commands reject via state and toast; callers should still inspect the returned value. */
export function useBoruCommand<A extends unknown[], R>(command: (...args: A) => Promise<R>): {
  pending: boolean; error: Error | null; run: (...args: A) => Promise<R | undefined>; reset: () => void;
} {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const reset = useCallback(() => setError(null), []);
  const run = useCallback(async (...args: A) => {
    setPending(true); setError(null);
    try { return await command(...args); }
    catch (cause) {
      const failure = new Error(redactError(cause));
      if (mounted.current) { setError(failure); setPending(false); uiStore.showError(failure.message, "Check your connection and try again."); }
      return undefined;
    } finally { if (mounted.current) setPending(false); }
  }, [command]);
  return { pending, error, run, reset };
}
