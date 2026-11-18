/**
 * Path-based routing helpers for Settings (`/settings/...`).
 *
 * URL structure:
 * - `/settings/hub/<tab>`
 * - `/settings/nodes/<node_id>/<tab>`
 * - `/settings/browser/<tab>`
 *
 * This module owns the URL shape. `app/router.tsx` uses the exported route
 * patterns and components use the builders/parsers here, so a path rename only
 * has to be made in one place.
 */

export type SettingsScope = "hub" | "node" | "browser";
export type SettingsScopeRef = SettingsScope | "current";

export const SETTINGS_ROOT = "/settings";

const HUB_SEGMENT = "hub";
const NODES_SEGMENT = "nodes";
const BROWSER_SEGMENT = "browser";

const HUB_BASE = `${SETTINGS_ROOT}/${HUB_SEGMENT}`;
const NODES_BASE = `${SETTINGS_ROOT}/${NODES_SEGMENT}`;
const BROWSER_BASE = `${SETTINGS_ROOT}/${BROWSER_SEGMENT}`;

/**
 * Child route patterns consumed by `app/router.tsx`.
 *
 * They are relative to `SETTINGS_ROOT`, like the other children of the
 * `/settings` route. The absolute bases above drive the URL builders.
 */
export const SETTINGS_HUB_ROUTE = `${HUB_SEGMENT}/:tab?`;
export const SETTINGS_NODES_ROUTE = NODES_SEGMENT;
export const SETTINGS_NODE_ROUTE = `${NODES_SEGMENT}/:node_id/:tab?`;
export const SETTINGS_BROWSER_ROUTE = `${BROWSER_SEGMENT}/:tab?`;

const NODE_INDEX_PATH = new RegExp(`^${NODES_BASE}/[^/]+/?$`);

export const HUB_TABS = ["general", "process", "tokens", "provisioners", "audit"] as const;
export const NODE_TABS_ALL = ["general", "process", "pairing", "agents", "audit"] as const;
export const BROWSER_TABS = ["appearance", "notifications"] as const;

export type HubSettingsTab = (typeof HUB_TABS)[number];
export type NodeSettingsTab = (typeof NODE_TABS_ALL)[number];
export type BrowserSettingsTab = (typeof BROWSER_TABS)[number];
export type SettingsTabId = HubSettingsTab | NodeSettingsTab | BrowserSettingsTab;

const HUB_TAB_SET = new Set<string>(HUB_TABS);
const NODE_TAB_SET = new Set<string>(NODE_TABS_ALL);
const BROWSER_TAB_SET = new Set<string>(BROWSER_TABS);

/** Tabs shown under the Node scope. */
export function nodeTabsForRole(): readonly NodeSettingsTab[] {
  return NODE_TABS_ALL;
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

export function isNodeTabAvailable(tab: NodeSettingsTab): boolean {
  return NODE_TAB_SET.has(tab);
}

export function isBrowserTab(tab: string): tab is BrowserSettingsTab {
  return BROWSER_TAB_SET.has(tab);
}

/** Whether a pathname is `/settings` or lives under it. */
export function isSettingsPath(pathname: string): boolean {
  return pathname === SETTINGS_ROOT || pathname.startsWith(`${SETTINGS_ROOT}/`);
}

/** Settings scope a pathname belongs to (`null` outside `/settings/...`). */
export function settingsScopeFromPath(pathname: string): SettingsScope | null {
  if (pathname === HUB_BASE || pathname.startsWith(`${HUB_BASE}/`)) return "hub";
  if (pathname === NODES_BASE || pathname.startsWith(`${NODES_BASE}/`)) return "node";
  if (pathname === BROWSER_BASE || pathname.startsWith(`${BROWSER_BASE}/`)) return "browser";
  return null;
}

/**
 * Entry paths that redirect to a canonical destination.
 *
 * They render the child route immediately and navigate to the default tab (or
 * default node) without the settings chrome flashing first.
 */
export function isSettingsIndexPath(pathname: string): boolean {
  return (
    pathname === SETTINGS_ROOT ||
    pathname === HUB_BASE ||
    pathname === BROWSER_BASE ||
    pathname === NODES_BASE ||
    NODE_INDEX_PATH.test(pathname)
  );
}

export function resolveSettingsScope(ref: SettingsScopeRef, isHub: boolean): SettingsScope {
  return ref === "current" ? (isHub ? "hub" : "node") : ref;
}

export function hubSettingsPath(tab: HubSettingsTab = "general"): string {
  return `${HUB_BASE}/${tab}`;
}

export function nodeSettingsPath(tab: NodeSettingsTab = "general", nodeId?: string | null): string {
  if (nodeId && nodeId !== "") {
    return `${NODES_BASE}/${encodeURIComponent(nodeId)}/${tab}`;
  }
  return NODES_BASE;
}

export function browserSettingsPath(tab: BrowserSettingsTab = "appearance"): string {
  return `${BROWSER_BASE}/${tab}`;
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
