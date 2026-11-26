import { afterEach, describe, expect, it, vi } from "vitest";
import { initViewport } from "./viewport.ts";

function stubViewport(options: {
  innerHeight: number;
  height: number;
  offsetTop?: number;
  scale?: number;
}): void {
  vi.stubGlobal("innerHeight", options.innerHeight);
  vi.stubGlobal("visualViewport", {
    height: options.height,
    offsetTop: options.offsetTop ?? 0,
    scale: options.scale ?? 1,
    addEventListener: vi.fn<() => void>(),
    removeEventListener: vi.fn<() => void>(),
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
  const root = document.documentElement;
  root.style.removeProperty("--app-height");
  root.style.removeProperty("--keyboard-inset");
  delete root.dataset.keyboard;
});

describe("initViewport", () => {
  it("tracks the layout viewport when no keyboard is open", () => {
    stubViewport({ innerHeight: 800, height: 800 });

    const dispose = initViewport();

    expect(document.documentElement.style.getPropertyValue("--app-height")).toBe("800px");
    expect(document.documentElement.style.getPropertyValue("--keyboard-inset")).toBe("0px");
    expect(document.documentElement.dataset.keyboard).toBeUndefined();
    dispose();
  });

  it("exposes the inset when only the visual viewport shrinks for the keyboard", () => {
    stubViewport({ innerHeight: 800, height: 500 });

    const dispose = initViewport();

    expect(document.documentElement.style.getPropertyValue("--keyboard-inset")).toBe("300px");
    expect(document.documentElement.dataset.keyboard).toBe("open");
    dispose();
  });

  it("ignores small layout / visual differences such as browser chrome", () => {
    stubViewport({ innerHeight: 800, height: 760 });

    const dispose = initViewport();

    expect(document.documentElement.style.getPropertyValue("--keyboard-inset")).toBe("0px");
    expect(document.documentElement.dataset.keyboard).toBeUndefined();
    dispose();
  });

  it("ignores a visual viewport shrunk by pinch zoom", () => {
    stubViewport({ innerHeight: 800, height: 400, scale: 2 });

    const dispose = initViewport();

    expect(document.documentElement.style.getPropertyValue("--keyboard-inset")).toBe("0px");
    expect(document.documentElement.dataset.keyboard).toBeUndefined();
    dispose();
  });

  it("removes the variables when disposed", () => {
    stubViewport({ innerHeight: 800, height: 500 });

    const dispose = initViewport();
    dispose();

    expect(document.documentElement.style.getPropertyValue("--keyboard-inset")).toBe("");
    expect(document.documentElement.style.getPropertyValue("--app-height")).toBe("");
    expect(document.documentElement.dataset.keyboard).toBeUndefined();
  });
});
