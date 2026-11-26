import { describe, expect, it } from "vitest";
import { visibleWindowGeometry } from "./uiPreferences";

describe("visibleWindowGeometry", () => {
  it("keeps in-bounds restored geometry", () => {
    expect(visibleWindowGeometry({ x: 100, y: 40, width: 800, height: 600 }, { width: 1600, height: 900 }))
      .toEqual({ x: 100, y: 40, width: 800, height: 600 });
  });
  it("moves off-screen geometry back into the current monitor bounds", () => {
    expect(visibleWindowGeometry({ x: 2500, y: 100, width: 1100, height: 800 }, { width: 1920, height: 1080 }))
      .toEqual({ x: 820, y: 100, width: 1100, height: 800 });
  });
  it("bounds saved size and position", () => {
    expect(visibleWindowGeometry({ x: -100, y: 900, width: 6000, height: 5000 }, { width: 1280, height: 720 }))
      .toEqual({ x: 0, y: 0, width: 1280, height: 720 });
  });
});
