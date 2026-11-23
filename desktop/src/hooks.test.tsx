// @vitest-environment jsdom
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useBoruCommand, useBoruQuery } from "./hooks";
import { uiStore } from "./uiStore";

afterEach(() => { uiStore.reset(); });

describe("Boru hooks", () => {
  it("reports command failures and provides a safe retry prompt", async () => {
    const command = vi.fn().mockRejectedValue(new Error("password=abc123 at /home/dan/private"));
    const { result } = renderHook(() => useBoruCommand(command));
    await act(async () => { await result.current.run("payload"); });
    expect(result.current.pending).toBe(false);
    expect(result.current.error?.message).toContain("[redacted]");
    expect(result.current.error?.message).not.toContain("/home/dan");
    expect(uiStore.getSnapshot().toast?.recovery).toContain("try again");
  });

  it("aborts and ignores a stale query result after the key changes", async () => {
    let resolveFirst!: (value: string) => void;
    const first = vi.fn((_signal: AbortSignal) => new Promise<string>((resolve) => { resolveFirst = resolve; }));
    const second = vi.fn(async () => "new view");
    const { result, rerender } = renderHook(({ key, loader }) => useBoruQuery(key, loader), {
      initialProps: { key: "a", loader: first },
    });
    rerender({ key: "b", loader: second });
    await waitFor(() => expect(result.current.data).toBe("new view"));
    await act(async () => { resolveFirst("stale view"); });
    expect(result.current.data).toBe("new view");
    expect(first.mock.calls[0][0].aborted).toBe(true);
  });
});
