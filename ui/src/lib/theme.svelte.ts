import { browser } from "$app/env";

/**
 * Theme palette handling.
 *
 * The design system (type, spacing, radii, layout, component anatomy) is
 * fixed. A palette only swaps the semantic color roles declared in `app.css`.
 * Palettes are curated: arbitrary colors would break contrast and the visual
 * hierarchy that makes Desk recognizable.
 */
export type PaletteId = "bone" | "graphite" | "moss";

export const DEFAULT_PALETTE: PaletteId = "bone";

/**
 * Palette metadata used by the settings preview.
 *
 * The preview colors mirror the light values in `app.css`; the stylesheet stays
 * the source of truth for the actual UI. They only paint the small swatches.
 */
export const PALETTES: readonly {
  id: PaletteId;
  canvas: string;
  surface: string;
  ink: string;
  signal: string;
}[] = [
  {
    id: "bone",
    canvas: "oklch(0.972 0.006 85)",
    surface: "oklch(0.988 0.004 85)",
    ink: "oklch(0.255 0.012 70)",
    signal: "oklch(0.52 0.14 55)",
  },
  {
    id: "graphite",
    canvas: "oklch(0.975 0.004 250)",
    surface: "oklch(0.99 0.002 250)",
    ink: "oklch(0.25 0.015 250)",
    signal: "oklch(0.52 0.17 260)",
  },
  {
    id: "moss",
    canvas: "oklch(0.973 0.008 120)",
    surface: "oklch(0.99 0.005 120)",
    ink: "oklch(0.25 0.015 130)",
    signal: "oklch(0.5 0.11 145)",
  },
];

const STORAGE_KEY = "fxg:palette";

function isPaletteId(value: unknown): value is PaletteId {
  return value === "bone" || value === "graphite" || value === "moss";
}

export function loadPalette(): PaletteId {
  if (!browser) return DEFAULT_PALETTE;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return isPaletteId(raw) ? raw : DEFAULT_PALETTE;
  } catch {
    return DEFAULT_PALETTE;
  }
}

function savePalette(id: PaletteId): void {
  if (!browser) return;
  try {
    localStorage.setItem(STORAGE_KEY, id);
  } catch {
    /* Ignore storage errors (private mode / quota). */
  }
}

function applyPalette(id: PaletteId): void {
  if (!browser) return;
  document.documentElement.dataset.palette = id;
}

function syncThemeColor(): void {
  if (!browser) return;
  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
  if (meta === null) return;
  const canvas = getComputedStyle(document.documentElement).getPropertyValue("--canvas").trim();
  if (canvas !== "") meta.content = canvas;
}

class ThemeStore {
  palette = $state<PaletteId>(loadPalette());

  #observing = false;

  /** Apply the current palette and keep the PWA chrome in sync with dark mode. */
  init(): void {
    if (!browser) return;
    if (!this.#observing) {
      this.#observing = true;
      new MutationObserver(syncThemeColor).observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["class", "data-palette"],
      });
    }
    applyPalette(this.palette);
    // Wait for the stylesheet to resolve the palette variables in dev (where
    // CSS is injected after the module runs) before reading the computed canvas.
    requestAnimationFrame(syncThemeColor);
  }

  setPalette(id: PaletteId): void {
    if (this.palette === id) return;
    this.palette = id;
    savePalette(id);
    applyPalette(id);
    syncThemeColor();
  }

  /** Re-read the computed canvas after a theme switch. */
  refreshChrome(): void {
    syncThemeColor();
  }
}

export const theme = new ThemeStore();
