/**
 * Path-based routing helpers for Settings (`/settings/...`).
 *
 * URL structure:
 * - `/settings/hub/<tab>`
 * - `/settings/nodes/<node_id>/<tab>`
 * - `/settings/browser/<tab>`
 */

export type SettingsScope = "hub" | "node" | "browser";
export type SettingsScopeRef = SettingsScope | "current";

export const HUB_TABS = ["general", "process", "tokens", "provisioners", "audit"] as const;
export const NODE_TABS_ALL = [
  "general",
  "process",
  "pairing",
  "agents",
  "opencode",
  "audit",
] as const;
export const NODE_TABS_HUB = ["general", "pairing", "agents"] as const;
export const BROWSER_TABS = ["appearance", "notifications"] as const;

export type HubSettingsTab = (typeof HUB_TABS)[number];
export type NodeSettingsTab = (typeof NODE_TABS_ALL)[number];
export type BrowserSettingsTab = (typeof BROWSER_TABS)[number];
export type SettingsTabId = HubSettingsTab | NodeSettingsTab | BrowserSettingsTab;

const HUB_TAB_SET = new Set<string>(HUB_TABS);
const NODE_TAB_SET = new Set<string>(NODE_TABS_ALL);
const NODE_TAB_HUB_SET = new Set<string>(NODE_TABS_HUB);
const BROWSER_TAB_SET = new Set<string>(BROWSER_TABS);

/** Tabs shown under the Node scope for the current connection role. */
export function nodeTabsForRole(isHub: boolean): readonly NodeSettingsTab[] {
  return isHub ? NODE_TABS_HUB : NODE_TABS_ALL;
}

/** First / default tab for a scope. */
export function defaultTabForScope(scope: SettingsScope): SettingsTabId {
  return scope === "browser" ? "appearance" : "general";
}

export function isHubTab(tab: string): tab is HubSettingsTab {
  return HUB_TAB_SET.has(tab);
}

export function isNodeTab(tab: string): tab is NodeSettingsTab {
  return NODE_TAB_SET.has(tab);
}

export function isNodeTabAvailable(tab: NodeSettingsTab, isHub: boolean): boolean {
  return isHub ? NODE_TAB_HUB_SET.has(tab) : NODE_TAB_SET.has(tab);
}

export function isBrowserTab(tab: string): tab is BrowserSettingsTab {
  return BROWSER_TAB_SET.has(tab);
}

export function resolveSettingsScope(ref: SettingsScopeRef, isHub: boolean): SettingsScope {
  return ref === "current" ? (isHub ? "hub" : "node") : ref;
}

export function hubSettingsPath(tab: HubSettingsTab = "general"): string {
  return `/settings/hub/${tab}`;
}

export function nodeSettingsPath(tab: NodeSettingsTab = "general", nodeId?: string | null): string {
  if (nodeId && nodeId !== "") {
    return `/settings/nodes/${encodeURIComponent(nodeId)}/${tab}`;
  }
  return "/settings/nodes";
}

export function browserSettingsPath(tab: BrowserSettingsTab = "appearance"): string {
  return `/settings/browser/${tab}`;
}

/** Pick the default target node ID for the current connection mode. */
export function resolveDefaultNodeId(
  isHub: boolean,
  nodes: readonly { node_id: string; is_online: boolean }[],
  systemNodeId?: string | null,
): string | null {
  if (!isHub) {
    return systemNodeId && systemNodeId !== "" ? systemNodeId : null;
  }
  return nodes.find((node) => node.is_online)?.node_id ?? null;
}

/** Default settings destination for the current connection mode. */
export function settingsHomePath(isHub: boolean, nodeId?: string | null): string {
  return isHub ? hubSettingsPath("general") : nodeSettingsPath("general", nodeId);
}

/** Build the target URL for a `(scope, tab)` pair (used by search results and deep links). */
export function settingsPathForTarget(
  scopeRef: SettingsScopeRef,
  tab: SettingsTabId,
  ctx: { isHub: boolean; nodeId?: string | null },
): string {
  const scope = resolveSettingsScope(scopeRef, ctx.isHub);
  switch (scope) {
    case "hub":
      return hubSettingsPath(isHubTab(tab) ? tab : "general");
    case "node":
      return nodeSettingsPath(isNodeTab(tab) ? tab : "general", ctx.nodeId);
    case "browser":
      return browserSettingsPath(isBrowserTab(tab) ? tab : "appearance");
  }
}
