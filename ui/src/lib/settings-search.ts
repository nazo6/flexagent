/**
 * Searchable index of the settings screen.
 *
 * Settings are grouped into three scopes — Hub (the server), Node (a machine
 * running sessions) and Browser (this device) — and each scope has its own
 * tabs. `current` resolves to the hub or the node depending on the role the UI
 * is connected to.
 *
 * Each entry points at a scope and a tab and the label a user would look for;
 * keywords cover common synonyms so "dark", "token" or "port" find the right
 * place. The index is navigation only: the value editors stay in their tab, so
 * there is a single place that renders the setting.
 */

export type SettingsScope = "hub" | "node" | "browser";
export type SettingsScopeRef = SettingsScope | "current";

export type SettingsTabId =
  | "process"
  | "general"
  | "pairing"
  | "tokens"
  | "provisioners"
  | "agents"
  | "audit"
  | "appearance";

export type SettingsSearchEntry = {
  scope: SettingsScopeRef;
  tab: SettingsTabId;
  /** Text shown in the results list. */
  label: string;
  /** Extra terms that should match this entry. */
  keywords: string;
};

export const SETTINGS_SCOPE_LABELS: Record<SettingsScope, string> = {
  hub: "Hub",
  node: "Node",
  browser: "Browser",
};

export const SETTINGS_TAB_LABELS: Record<SettingsTabId, string> = {
  process: "Process",
  general: "General",
  pairing: "Hub connection",
  tokens: "Pairing tokens",
  provisioners: "Provisioners",
  agents: "Agents",
  audit: "Audit log",
  appearance: "Appearance",
};

export const SETTINGS_SEARCH_ENTRIES: SettingsSearchEntry[] = [
  {
    scope: "current",
    tab: "process",
    label: "Data directory",
    keywords: "path home flexagent directory",
  },
  { scope: "current", tab: "process", label: "Node ID", keywords: "node identity id" },
  {
    scope: "current",
    tab: "process",
    label: "Listen address",
    keywords: "port host bind address listen",
  },
  {
    scope: "node",
    tab: "pairing",
    label: "Hub pairing",
    keywords: "hub url token pairing connect central sync",
  },
  {
    scope: "current",
    tab: "process",
    label: "Log level",
    keywords: "log level verbosity debug info trace",
  },
  { scope: "current", tab: "process", label: "Log file", keywords: "log file path output" },
  { scope: "current", tab: "process", label: "Log format", keywords: "log format json text" },
  { scope: "current", tab: "process", label: "No color", keywords: "color colour ansi terminal" },
  {
    scope: "current",
    tab: "process",
    label: "Support bundle",
    keywords: "diagnostics export bundle support",
  },
  {
    scope: "current",
    tab: "process",
    label: "Component health",
    keywords: "health status db http sync disk",
  },
  {
    scope: "current",
    tab: "process",
    label: "Emergency stop",
    keywords: "kill switch stop all sessions processes pty emergency",
  },

  { scope: "node", tab: "general", label: "Display name", keywords: "name label node title" },
  { scope: "node", tab: "general", label: "Tray", keywords: "tray menu bar background" },
  {
    scope: "node",
    tab: "general",
    label: "Allow remote Web PTY",
    keywords: "terminal pty remote shell",
  },
  {
    scope: "node",
    tab: "general",
    label: "VAPID subject",
    keywords: "vapid push notification contact email mailto",
  },
  {
    scope: "node",
    tab: "general",
    label: "Project scan directories",
    keywords: "projects scan folders directories workspace",
  },
  { scope: "node", tab: "general", label: "Secrets", keywords: "secret key value environment env" },
  {
    scope: "node",
    tab: "general",
    label: "Web access token",
    keywords: "token auth login password regenerate",
  },

  { scope: "hub", tab: "general", label: "Hub settings", keywords: "hub settings central server" },
  {
    scope: "hub",
    tab: "general",
    label: "VAPID subject",
    keywords: "vapid push notification contact email mailto",
  },
  {
    scope: "hub",
    tab: "general",
    label: "Web access token",
    keywords: "token auth login password regenerate",
  },
  {
    scope: "hub",
    tab: "tokens",
    label: "Node pairing",
    keywords: "pair node token connect hub",
  },
  {
    scope: "hub",
    tab: "tokens",
    label: "Pairing tokens",
    keywords: "token issue revoke node pairing",
  },
  {
    scope: "hub",
    tab: "provisioners",
    label: "Provisioners",
    keywords: "provisioner docker incus colab vm sandbox ephemeral",
  },

  {
    scope: "node",
    tab: "agents",
    label: "ACP agents",
    keywords: "agent acp opencode install path",
  },
  {
    scope: "node",
    tab: "agents",
    label: "Agent catalog",
    keywords: "catalog update agents install",
  },
  { scope: "node", tab: "agents", label: "Custom agent", keywords: "custom agent definition" },

  {
    scope: "browser",
    tab: "appearance",
    label: "Theme",
    keywords: "theme dark light mode appearance",
  },
  {
    scope: "browser",
    tab: "appearance",
    label: "Color palette",
    keywords: "palette colors colour bone graphite moss",
  },
  {
    scope: "browser",
    tab: "appearance",
    label: "Send shortcut",
    keywords: "shortcut keyboard enter send newline",
  },
  {
    scope: "browser",
    tab: "appearance",
    label: "Pinned sessions",
    keywords: "pin pinned sidebar sessions",
  },

  { scope: "current", tab: "audit", label: "Audit log", keywords: "audit log history events" },
];

/** Returns the entries matching `query`, in index order. */
export function searchSettings(query: string): SettingsSearchEntry[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") return [];
  return SETTINGS_SEARCH_ENTRIES.filter((entry) => {
    const haystack = `${entry.label} ${entry.keywords} ${SETTINGS_TAB_LABELS[entry.tab]} ${SETTINGS_SCOPE_LABELS[entry.scope === "current" ? "node" : entry.scope]}`;
    return haystack.toLowerCase().includes(needle);
  });
}
