import { browser } from "$app/env";

/**
 * Motion preference for this browser.
 *
 * `system` follows the operating system's reduced-motion setting, `full`
 * always keeps the animations, and `reduced` removes decorative motion even
 * when the OS asks for it. The preference only affects this device; it is
 * never sent to the node or the hub.
 */
export type MotionPreference = "system" | "full" | "reduced";

export const DEFAULT_MOTION: MotionPreference = "system";

const STORAGE_KEY = "fxg:motion";

function isMotionPreference(value: unknown): value is MotionPreference {
  return value === "system" || value === "full" || value === "reduced";
}

export function loadMotionPreference(): MotionPreference {
  if (!browser) return DEFAULT_MOTION;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return isMotionPreference(raw) ? raw : DEFAULT_MOTION;
  } catch {
    return DEFAULT_MOTION;
  }
}

function saveMotionPreference(value: MotionPreference): void {
  if (!browser) return;
  try {
    localStorage.setItem(STORAGE_KEY, value);
  } catch {
    /* Ignore storage errors (private mode / quota). */
  }
}

class MotionStore {
  preference = $state<MotionPreference>(loadMotionPreference());
  /** Live value of the operating system's `prefers-reduced-motion`. */
  systemReduced = $state(false);

  /**
   * Whether decorative motion is currently suppressed. Components use this
   * for JS-driven transitions (Svelte's `slide`), while CSS transitions are
   * handled by the `data-motion` rules in `app.css`.
   */
  reduced = $derived(
    this.preference === "reduced" || (this.preference === "system" && this.systemReduced),
  );

  #media: MediaQueryList | null = null;

  #onMediaChange = (event: MediaQueryListEvent): void => {
    this.systemReduced = event.matches;
    this.#apply();
  };

  init(): void {
    if (!browser) return;
    if (this.#media === null) {
      this.#media = window.matchMedia("(prefers-reduced-motion: reduce)");
      this.systemReduced = this.#media.matches;
      this.#media.addEventListener("change", this.#onMediaChange);
    }
    this.#apply();
  }

  setPreference(value: MotionPreference): void {
    this.preference = value;
    saveMotionPreference(value);
    this.#apply();
  }

  /** Transition duration in milliseconds, honouring the current preference. */
  duration(milliseconds: number): number {
    return this.reduced ? 0 : milliseconds;
  }

  /**
   * Effective reduced-motion value, safe before `init()` has run. Cold-start
   * components (the brand draw) can mount before the layout's `onMount`.
   */
  isReducedNow(): boolean {
    if (!browser) return false;
    const system = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    return this.preference === "reduced" || (this.preference === "system" && system);
  }

  #apply(): void {
    if (!browser) return;
    document.documentElement.dataset.motion = this.preference;
  }
}

export const motion = new MotionStore();
