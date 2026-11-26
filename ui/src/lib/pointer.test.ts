import { afterEach, describe, expect, it, vi } from "vitest";
import { isCoarsePointer } from "./pointer.ts";

describe("isCoarsePointer", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("detects a coarse primary pointer", () => {
    const matchMedia = vi.fn<(query: string) => { matches: boolean }>((query) => ({
      matches: query === "(pointer: coarse)",
    }));
    vi.stubGlobal("matchMedia", matchMedia);

    expect(isCoarsePointer()).toBe(true);
    expect(matchMedia).toHaveBeenCalledWith("(pointer: coarse)");
  });

  it("is false for a fine primary pointer", () => {
    vi.stubGlobal("matchMedia", () => ({ matches: false }));

    expect(isCoarsePointer()).toBe(false);
  });

  it("is false when matchMedia is unavailable", () => {
    vi.stubGlobal("matchMedia", undefined);

    expect(isCoarsePointer()).toBe(false);
  });
});
