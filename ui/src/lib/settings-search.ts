/**
 * Searchable index of the settings screen.
 *
 * Each entry points at a tab and the label a user would look for; keywords cover
 * common synonyms so "dark", "token" or "port" find the right tab. The index is
 * navigation only: the value editors stay in their tab, so there is a single
 * place that renders the setting.
 */

export type SettingsTab = "bootstrap" | "machine" | "hub" | "agents" | "browser" | "audit";

export type SettingsSearchEntry = {
  tab: SettingsTab;
  /** Text shown in the results list. */
  label: string;
  /** Extra terms that should match this entry. */
  keywords: string;
};

export const SETTINGS_TAB_LABELS: Record<SettingsTab, string> = {
  bootstrap: "Bootstrap",
  machine: "This machine",
  hub: "Hub",
  agents: "Agents",
  browser: "Browser",
  audit: "Audit log",
};

export const SETTINGS_SEARCH_ENTRIES: SettingsSearchEntry[] = [
  { tab: "bootstrap", label: "Data directory", keywords: "path home flexagent directory" },
  { tab: "bootstrap", label: "Node ID", keywords: "node identity id" },
  { tab: "bootstrap", label: "Listen address", keywords: "port host bind address listen" },
  { tab: "bootstrap", label: "Hub", keywords: "hub url connection central" },
  { tab: "bootstrap", label: "Log level", keywords: "log level verbosity debug info trace" },
  { tab: "bootstrap", label: "Log file", keywords: "log file path output" },
  { tab: "bootstrap", label: "Log format", keywords: "log format json text" },
  { tab: "bootstrap", label: "No color", keywords: "color colour ansi terminal" },
  { tab: "bootstrap", label: "Support bundle", keywords: "diagnostics export bundle support" },
  { tab: "bootstrap", label: "Component health", keywords: "health status db http sync disk" },

  { tab: "machine", label: "Display name", keywords: "name label node title" },
  { tab: "machine", label: "Tray", keywords: "tray menu bar background" },
  { tab: "machine", label: "Allow remote Web PTY", keywords: "terminal pty remote shell" },
  {
    tab: "machine",
    label: "Project scan directories",
    keywords: "projects scan folders directories workspace",
  },
  { tab: "machine", label: "Secrets", keywords: "secret key value environment env" },
  { tab: "machine", label: "Client auth token", keywords: "token auth login password regenerate" },

  { tab: "hub", label: "Hub settings", keywords: "hub settings central server" },
  { tab: "hub", label: "Node pairing", keywords: "pair node token connect hub" },
  { tab: "hub", label: "Node tokens", keywords: "token issue revoke node" },

  { tab: "agents", label: "ACP agents", keywords: "agent acp opencode install path" },
  { tab: "agents", label: "Agent catalog", keywords: "catalog update agents install" },
  { tab: "agents", label: "Custom agent", keywords: "custom agent definition" },

  { tab: "browser", label: "Theme", keywords: "theme dark light mode appearance" },
  { tab: "browser", label: "Color palette", keywords: "palette colors colour bone graphite moss" },
  {
    tab: "browser",
    label: "Send shortcut",
    keywords: "shortcut keyboard enter send newline",
  },
  { tab: "browser", label: "Pinned sessions", keywords: "pin pinned sidebar sessions" },

  { tab: "audit", label: "Audit log", keywords: "audit log history events" },
];

/** Returns the entries matching `query`, in index order. */
export function searchSettings(query: string): SettingsSearchEntry[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") return [];
  return SETTINGS_SEARCH_ENTRIES.filter((entry) => {
    const haystack = `${entry.label} ${entry.keywords} ${SETTINGS_TAB_LABELS[entry.tab]}`;
    return haystack.toLowerCase().includes(needle);
  });
}
