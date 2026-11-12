import type { ConfigOptionValue } from "#lib/generated/bindings.ts";

/**
 * Per-device memory for the new session screen (localStorage).
 *
 * Remembers the last used agent / initial mode per project.
 * This is a per-device preference and is never stored on the server (node or central server).
 */

/**
 * Initial mode preference: `default` (let the agent decide) or any mode ID the
 * selected agent reports (for example OpenCode's `build` / `plan`).
 */
export type SessionModePreference = string;

/** Startup settings restored by the new session screen. */
export interface NewSessionPrefs {
  /** Last used agent ID (restored into the custom input when not installed on the node) */
  agent: string;
  /** Initial agent mode */
  mode: SessionModePreference;
}

const KEY_PREFIX = "fxg:new_session_prefs:";
const CONFIG_KEY_PREFIX = "fxg:session_config:";

/** Resolve `localStorage` (SSR / tests can inject a replacement). */
function resolveStorage(storage?: Storage | null): Storage | null {
  if (storage !== undefined) return storage;
  return typeof localStorage === "undefined" ? null : localStorage;
}

function isSessionMode(value: unknown): value is SessionModePreference {
  return typeof value === "string" && value !== "";
}

/**
 * Load the saved startup settings.
 *
 * Returns `null` when nothing is saved, the JSON is invalid, or no meaningful
 * value is present. Invalid enum values are clamped to the default (`default`).
 */
export function loadNewSessionPrefs(
  projectId: string,
  storage?: Storage | null,
): NewSessionPrefs | null {
  const store = resolveStorage(storage);
  if (store === null || projectId === "") return null;
  const raw = store.getItem(KEY_PREFIX + projectId);
  if (raw === null) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    const record = parsed as Record<string, unknown>;
    const prefs: NewSessionPrefs = {
      agent: typeof record.agent === "string" ? record.agent : "",
      mode: isSessionMode(record.mode) ? record.mode : "default",
    };
    if (prefs.agent === "" && prefs.mode === "default") {
      return null;
    }
    return prefs;
  } catch {
    return null;
  }
}

/** Save the startup settings (failures such as quota exceeded are ignored). */
export function saveNewSessionPrefs(
  projectId: string,
  prefs: NewSessionPrefs,
  storage?: Storage | null,
): void {
  const store = resolveStorage(storage);
  if (store === null || projectId === "") return;
  try {
    store.setItem(KEY_PREFIX + projectId, JSON.stringify(prefs));
  } catch {
    // Do not persist when localStorage is disabled or over quota
  }
}

/** Load the last used session configuration (model / thinking level) for a project. */
export function loadSessionConfig(
  projectId: string,
  storage?: Storage | null,
): Record<string, ConfigOptionValue> {
  const store = resolveStorage(storage);
  if (store === null || projectId === "") return {};
  const raw = store.getItem(CONFIG_KEY_PREFIX + projectId);
  if (raw === null) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    return parsed as Record<string, ConfigOptionValue>;
  } catch {
    return {};
  }
}

/** Persist one configuration value for a project (per-device preference). */
export function saveSessionConfigOption(
  projectId: string,
  key: string,
  value: ConfigOptionValue,
  storage?: Storage | null,
): void {
  const store = resolveStorage(storage);
  if (store === null || projectId === "" || key === "") return;
  try {
    const config = loadSessionConfig(projectId, store);
    config[key] = value;
    store.setItem(CONFIG_KEY_PREFIX + projectId, JSON.stringify(config));
  } catch {
    // Do not persist when localStorage is disabled or over quota
  }
}

const MODEL_FAVORITES_KEY_PREFIX = "fxg:model_favorites:";

/**
 * Load the favorite model keys for one agent.
 *
 * Keys are [`modelChoiceKey`](../config-options.ts) values. Unknown or
 * malformed entries are dropped so a corrupt preference cannot break the
 * picker.
 */
export function loadModelFavorites(agentId: string, storage?: Storage | null): string[] {
  const store = resolveStorage(storage);
  if (store === null || agentId === "") return [];
  const raw = store.getItem(MODEL_FAVORITES_KEY_PREFIX + agentId);
  if (raw === null) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    const seen = new Set<string>();
    const result: string[] = [];
    for (const value of parsed) {
      if (typeof value !== "string" || value === "" || seen.has(value)) continue;
      seen.add(value);
      result.push(value);
    }
    return result;
  } catch {
    return [];
  }
}

/** Persist the favorite model keys for one agent (per-device preference). */
export function saveModelFavorites(
  agentId: string,
  favorites: readonly string[],
  storage?: Storage | null,
): void {
  const store = resolveStorage(storage);
  if (store === null || agentId === "") return;
  try {
    store.setItem(MODEL_FAVORITES_KEY_PREFIX + agentId, JSON.stringify(favorites));
  } catch {
    // Do not persist when localStorage is disabled or over quota
  }
}

/**
 * Toggle one model favorite and return the updated list.
 *
 * Disappeared models stay in the list so the favorite is restored if the
 * agent offers the model again.
 */
export function toggleModelFavorite(
  agentId: string,
  key: string,
  storage?: Storage | null,
): string[] {
  if (agentId === "" || key === "") return loadModelFavorites(agentId, storage);
  const current = loadModelFavorites(agentId, storage);
  const next = current.includes(key)
    ? current.filter((favorite) => favorite !== key)
    : [...current, key];
  saveModelFavorites(agentId, next, storage);
  return next;
}

export type SendShortcut = "enter" | "modifier_enter";

const SHORTCUT_KEY = "fxg:send_shortcut";

/**
 * Load the send shortcut preference (defaults to "enter").
 */
export function loadSendShortcutPreference(storage?: Storage | null): SendShortcut {
  const store = resolveStorage(storage);
  if (store === null) return "enter";
  const raw = store.getItem(SHORTCUT_KEY);
  if (raw === "modifier_enter" || raw === "enter") return raw;
  return "enter";
}

/**
 * Save the send shortcut preference.
 */
export function saveSendShortcutPreference(shortcut: SendShortcut, storage?: Storage | null): void {
  const store = resolveStorage(storage);
  if (store === null) return;
  try {
    store.setItem(SHORTCUT_KEY, shortcut);
  } catch {
    // Do not persist when localStorage is disabled or over quota
  }
}

/**
 * Checks whether a keydown event should trigger a form submit based on the preference.
 * Always guards against IME composition (`isComposing` and keyCode 229).
 */
export function shouldSubmitOnEnter(
  event: KeyboardEvent,
  shortcut: SendShortcut = "enter",
): boolean {
  if (event.isComposing || event.keyCode === 229) return false;
  if (event.key !== "Enter") return false;
  if (shortcut === "modifier_enter") {
    return event.ctrlKey || event.metaKey;
  }
  // "enter" mode: Enter sends unless Shift is pressed (newline). Ctrl/Cmd+Enter also sends.
  return event.ctrlKey || event.metaKey || !event.shiftKey;
}
