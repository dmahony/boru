import { describe, expect, it } from "vitest";
import { parseRoute, routeUrl } from "./navigation";

describe("deep routes", () => {
  it("round trips conversation ids and recognizes every shell destination", () => {
    const route = { page: "conversations", conversationId: "room/a" } as const;
    expect(parseRoute(routeUrl(route))).toEqual(route);
    for (const page of ["directory", "friends", "shared-files", "tunnels", "settings", "call"] as const) {
      expect(parseRoute(routeUrl({ page }))).toEqual({ page });
    }
  });
  it("falls back safely for unknown routes", () => expect(parseRoute("#/unknown/x")).toEqual({ page: "conversations" }));
});