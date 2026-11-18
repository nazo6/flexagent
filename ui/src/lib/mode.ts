import { createEffect, createRoot, createSignal, onCleanup, type Accessor } from "solid-js";

/**
 * Light / dark / system mode.
 *
 * Replaces `mode-watcher`: the same `localStorage` key and values are used, so
 * an existing preference carries over. The `dark` class lands on
 * `<html>` (the Desk stylesheet keys off `.dark`).
 */

export type Mode = "light" | "dark" | "system";
export type ResolvedMode = "light" | "dark";

const STORAGE_KEY = "mode-watcher-mode";
const DARK_QUERY = "(prefers-color-scheme: dark)";

function readStoredMode(): Mode {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    if (value === "light" || value === "dark" || value === "system") return value;
  } catch {
    // localStorage can be unavailable (private mode); fall back to system.
  }
  return "system";
}

function createModeStore() {
  const [userPrefersMode, setUserPrefersMode] = createSignal<Mode>(readStoredMode());
  const query = typeof window === "undefined" ? null : window.matchMedia(DARK_QUERY);
  const [systemPrefersMode, setSystemPrefersMode] = createSignal<ResolvedMode>(
    query?.matches ? "dark" : "light",
  );

  if (query !== null) {
    const onChange = (event: MediaQueryListEvent) => {
      setSystemPrefersMode(event.matches ? "dark" : "light");
    };
    query.addEventListener("change", onChange);
    onCleanup(() => query.removeEventListener("change", onChange));
  }

  const mode: Accessor<ResolvedMode> = () => {
    const preference = userPrefersMode();
    return preference === "system" ? systemPrefersMode() : preference;
  };

  createEffect(
    () => mode(),
    (resolved) => {
      document.documentElement.classList.toggle("dark", resolved === "dark");
      document.documentElement.style.colorScheme = resolved;
    },
  );

  const setMode = (next: Mode) => {
    setUserPrefersMode(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Ignore storage failures; the in-memory preference still applies.
    }
  };

  return {
    userPrefersMode,
    systemPrefersMode,
    mode,
    setMode,
    toggleMode: () => setMode(mode() === "dark" ? "light" : "dark"),
  };
}

/** Singleton; created at import time so the class is applied before first paint. */
export const modeStore = createRoot(createModeStore);

export const mode = modeStore.mode;
export const userPrefersMode = modeStore.userPrefersMode;
export const systemPrefersMode = modeStore.systemPrefersMode;
export const setMode = modeStore.setMode;
export const toggleMode = modeStore.toggleMode;
