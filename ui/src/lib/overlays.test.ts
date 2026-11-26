import { describe, expect, it, vi } from "vitest";
import { closeTopOverlay, hasOpenOverlay, registerOverlay } from "./overlays.ts";

describe("overlay stack", () => {
  it("closes the most recently opened overlay first", () => {
    const order: string[] = [];
    const disposeFirst = registerOverlay("first", () => order.push("first"));
    const disposeSecond = registerOverlay("second", () => order.push("second"));

    expect(hasOpenOverlay()).toBe(true);
    expect(closeTopOverlay()).toBe(true);
    expect(order).toEqual(["second"]);
    expect(closeTopOverlay()).toBe(true);
    expect(order).toEqual(["second", "first"]);
    expect(closeTopOverlay()).toBe(false);
    expect(hasOpenOverlay()).toBe(false);

    disposeFirst();
    disposeSecond();
  });

  it("removes an overlay when its effect is cleaned up", () => {
    const close = vi.fn<() => void>();
    const dispose = registerOverlay("dock", close);
    dispose();

    expect(closeTopOverlay()).toBe(false);
    expect(close).not.toHaveBeenCalled();
  });

  it("replaces a duplicate registration of the same id", () => {
    const first = vi.fn<() => void>();
    const second = vi.fn<() => void>();
    const disposeFirst = registerOverlay("dock", first);
    const disposeSecond = registerOverlay("dock", second);

    expect(closeTopOverlay()).toBe(true);
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();

    disposeFirst();
    disposeSecond();
  });
});
