import type { ConfigOptionValue } from "#lib/generated/bindings.ts";

/**
 * Per-device memory for the new session screen (localStorage).
 *
 * Remembers the last used agent / initial mode / OpenCode2 run mode per project.
 * This is a per-device preference and is never stored on the server (node or central server).
 */
export type SessionModePreference = "default" | "code" | "plan";

/** Startup settings restored by the new session screen. */
export interface NewSessionPrefs {
  /** Last used agent ID (restored into the custom input when not installed on the node) */
  agent: string;
  /** Initial agent mode */
  mode: SessionModePreference;
}

const KEY_PREFIX = "fxg:new_session_prefs:";
const CONFIG_KEY_PREFIX = "fxg:session_config:";
const SESSION_MODES: readonly SessionModePreference[] = ["default", "code", "plan"];

/** Resolve `localStorage` (SSR / tests can inject a replacement). */
function resolveStorage(storage?: Storage | null): Storage | null {
  if (storage !== undefined) return storage;
  return typeof localStorage === "undefined" ? null : localStorage;
}

function isSessionMode(value: unknown): value is SessionModePreference {
  return SESSION_MODES.includes(value as SessionModePreference);
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
