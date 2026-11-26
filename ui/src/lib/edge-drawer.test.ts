import { describe, expect, it } from "vitest";
import { shouldSettleClosed, shouldSettleOpen } from "./edge-drawer.ts";

describe("shouldSettleOpen", () => {
  it("opens past the position threshold", () => {
    expect(shouldSettleOpen({ progress: 0.35, distance: 10, velocity: 0 })).toBe(true);
  });

  it("stays closed below the threshold without a fling", () => {
    expect(shouldSettleOpen({ progress: 0.2, distance: 10, velocity: 0.1 })).toBe(false);
  });

  it("opens on a fast fling past the minimum distance", () => {
    expect(shouldSettleOpen({ progress: 0.2, distance: 40, velocity: 0.6 })).toBe(true);
  });

  it("ignores a fling that barely moved", () => {
    expect(shouldSettleOpen({ progress: 0.2, distance: 10, velocity: 0.9 })).toBe(false);
  });
});

describe("shouldSettleClosed", () => {
  it("closes past the position threshold", () => {
    expect(shouldSettleClosed({ progress: 0.65, distance: 0, velocity: 0 })).toBe(true);
  });

  it("stays open below the threshold without a fling", () => {
    expect(shouldSettleClosed({ progress: 0.8, distance: 5, velocity: -0.1 })).toBe(false);
  });

  it("closes on a fast inward fling", () => {
    expect(shouldSettleClosed({ progress: 0.8, distance: 40, velocity: -0.6 })).toBe(true);
  });
});
