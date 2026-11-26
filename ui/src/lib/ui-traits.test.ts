import { afterEach, describe, expect, it, vi } from "vitest";

type ChangeListener = (event: { matches: boolean }) => void;

interface MediaQueryListStub {
  matches: boolean;
  media: string;
  onchange: null;
  addEventListener: (type: "change", listener: ChangeListener) => void;
  removeEventListener: (type: "change", listener: ChangeListener) => void;
  addListener: () => void;
  removeListener: () => void;
  dispatchEvent: () => boolean;
}

/** Stub `matchMedia` so the module-level trait stores see the given queries. */
function stubMedia(matches: (query: string) => boolean): void {
  vi.stubGlobal("matchMedia", (query: string): MediaQueryListStub => {
    const listeners = new Set<ChangeListener>();
    return {
      matches: matches(query),
      media: query,
      onchange: null,
      addEventListener: (_type, listener) => listeners.add(listener),
      removeEventListener: (_type, listener) => listeners.delete(listener),
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    };
  });
}

/** Load a fresh `uiTraits` so each case starts from the stubbed queries. */
async function loadTraits() {
  vi.resetModules();
  const module = await import("./ui-traits.svelte.ts");
  return module.uiTraits;
}

describe("uiTraits", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("uses the overlay interaction model on a narrow touch device", async () => {
    stubMedia((query) => query === "(width < 64rem)" || query === "(pointer: coarse)");
    const traits = await loadTraits();

    expect(traits.compact).toBe(true);
    expect(traits.touch).toBe(true);
    expect(traits.overlayNav).toBe(true);
    expect(traits.overlayPanels).toBe(true);
    expect(traits.sheetPickers).toBe(true);
    expect(traits.roomyHeader).toBe(false);
  });

  it("keeps popovers on a narrow desktop window", async () => {
    stubMedia((query) => query === "(width < 64rem)");
    const traits = await loadTraits();

    expect(traits.overlayNav).toBe(true);
    expect(traits.touch).toBe(false);
    expect(traits.sheetPickers).toBe(false);
  });

  it("keeps the wide shell on a tablet with a coarse pointer", async () => {
    stubMedia((query) => query === "(width >= 48rem)" || query === "(pointer: coarse)");
    const traits = await loadTraits();

    expect(traits.compact).toBe(false);
    expect(traits.touch).toBe(true);
    expect(traits.overlayNav).toBe(false);
    expect(traits.overlayPanels).toBe(false);
    expect(traits.sheetPickers).toBe(false);
    expect(traits.roomyHeader).toBe(true);
  });
});
