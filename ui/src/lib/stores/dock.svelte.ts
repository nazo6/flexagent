import { DOCK_TOOLS, isDockToolId, dockTool, type DockToolId } from "#lib/dock/registry.ts";

/** Session context handed to the dock. The workspace, not the session, owns the tabs. */
export interface DockContext {
  sessionId: string;
  nodeId: string;
  localPath: string;
}

/** One open tool instance. `resource` is a file path once file tabs exist. */
export interface DockTab {
  id: string;
  tool: DockToolId;
  resource: string | null;
}

export interface DockWorkspace {
  tabs: DockTab[];
  activeTabId: string | null;
  collapsed: boolean;
  updatedAt: number;
}

/** The subset of `Storage` the dock needs (injectable for tests). */
export interface DockStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const STORAGE_KEY = "fxg:workspace_dock";
const STORAGE_VERSION = 1;
/** Keep the most recently used workspaces; each one carries a few tabs. */
const MAX_WORKSPACES = 12;
const MAX_TABS = 8;
export const MIN_DOCK_WIDTH = 20;
export const MAX_DOCK_WIDTH = 75;
const DEFAULT_WIDTH = 42;

interface PersistedTab {
  tool: DockToolId;
  resource: string | null;
}

interface PersistedWorkspace {
  tabs: PersistedTab[];
  activeIndex: number;
  collapsed: boolean;
  updatedAt: number;
}

interface PersistedDock {
  version: number;
  width: number;
  workspaces: Record<string, PersistedWorkspace>;
}

function defaultStorage(): DockStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** Stable key for a workspace; the same identity the PTY registry uses. */
export function workspaceDockKey(nodeId: string, localPath: string): string {
  return `${nodeId}\u0000${localPath}`;
}

function tabKey(tool: DockToolId, resource: string | null): string {
  return `${tool}\u0000${resource ?? ""}`;
}

function isSingleton(tool: DockToolId): boolean {
  return DOCK_TOOLS.find((candidate) => candidate.id === tool)?.singleton ?? true;
}

function clampWidth(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_WIDTH;
  return Math.max(MIN_DOCK_WIDTH, Math.min(MAX_DOCK_WIDTH, value));
}

function emptyWorkspace(): DockWorkspace {
  return { tabs: [], activeTabId: null, collapsed: true, updatedAt: Date.now() };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Restores and validates one persisted workspace; unknown tools are dropped. */
function restoreWorkspace(value: unknown): DockWorkspace | null {
  if (!isRecord(value) || !Array.isArray(value.tabs)) return null;

  const tabs: DockTab[] = [];
  for (const raw of value.tabs) {
    if (!isRecord(raw)) continue;
    const tool = raw.tool;
    if (!isDockToolId(tool)) continue;
    const resource = typeof raw.resource === "string" ? raw.resource : null;
    const id = tabKey(tool, resource);
    if (tabs.some((tab) => tab.id === id)) continue;
    if (isSingleton(tool) && tabs.some((tab) => tab.tool === tool)) continue;
    tabs.push({ id, tool, resource });
    if (tabs.length >= MAX_TABS) break;
  }

  const updatedAt =
    typeof value.updatedAt === "number" && Number.isFinite(value.updatedAt)
      ? value.updatedAt
      : Date.now();
  if (tabs.length === 0) {
    return { ...emptyWorkspace(), updatedAt };
  }

  const activeIndex =
    typeof value.activeIndex === "number" && Number.isInteger(value.activeIndex)
      ? value.activeIndex
      : 0;
  const activeTab = tabs[Math.max(0, Math.min(activeIndex, tabs.length - 1))] ?? tabs[0];
  return {
    tabs,
    activeTabId: activeTab?.id ?? null,
    collapsed: value.collapsed !== false,
    updatedAt,
  };
}

function loadDock(storage: DockStorage | null): {
  width: number;
  workspaces: Record<string, DockWorkspace>;
} {
  if (storage === null) {
    return { width: DEFAULT_WIDTH, workspaces: {} };
  }
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (raw === null) {
      return { width: DEFAULT_WIDTH, workspaces: {} };
    }
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed) || parsed.version !== STORAGE_VERSION) {
      return { width: DEFAULT_WIDTH, workspaces: {} };
    }

    const width = clampWidth(typeof parsed.width === "number" ? parsed.width : DEFAULT_WIDTH);
    const workspaces: Record<string, DockWorkspace> = {};
    if (isRecord(parsed.workspaces)) {
      for (const [key, value] of Object.entries(parsed.workspaces)) {
        const workspace = restoreWorkspace(value);
        if (workspace !== null) workspaces[key] = workspace;
      }
    }
    return { width, workspaces: pruneWorkspaces(workspaces, null) };
  } catch {
    return { width: DEFAULT_WIDTH, workspaces: {} };
  }
}

function pruneWorkspaces(
  workspaces: Record<string, DockWorkspace>,
  keepKey: string | null,
): Record<string, DockWorkspace> {
  const keys = Object.keys(workspaces);
  if (keys.length <= MAX_WORKSPACES) return workspaces;

  const removable = keys
    .filter((key) => key !== keepKey)
    .toSorted((a, b) => (workspaces[a]?.updatedAt ?? 0) - (workspaces[b]?.updatedAt ?? 0));
  const pruned = { ...workspaces };
  for (const key of removable.slice(0, keys.length - MAX_WORKSPACES)) {
    delete pruned[key];
  }
  return pruned;
}

function serialize(workspaces: Record<string, DockWorkspace>, width: number): string {
  const entries = Object.entries(workspaces)
    .toSorted((a, b) => b[1].updatedAt - a[1].updatedAt)
    .slice(0, MAX_WORKSPACES);
  const persisted: PersistedDock = {
    version: STORAGE_VERSION,
    width,
    workspaces: Object.fromEntries(
      entries.map(([key, workspace]) => [
        key,
        {
          tabs: workspace.tabs.map((tab) => ({ tool: tab.tool, resource: tab.resource })),
          activeIndex: Math.max(
            0,
            workspace.tabs.findIndex((tab) => tab.id === workspace.activeTabId),
          ),
          collapsed: workspace.collapsed || workspace.tabs.length === 0,
          updatedAt: workspace.updatedAt,
        } satisfies PersistedWorkspace,
      ]),
    ),
  };
  return JSON.stringify(persisted);
}

/**
 * Workspace-scoped tab state for the session side pane.
 *
 * The workspace (`node_id` + canonical local path) owns the tabs; a session is
 * only the context passed to the active tools. Switching sessions inside one
 * workspace therefore keeps the terminal and any open tool alive.
 */
export class WorkspaceDockStore {
  width = $state(DEFAULT_WIDTH);
  workspaces = $state<Record<string, DockWorkspace>>({});
  currentKey = $state<string | null>(null);
  context = $state<DockContext | null>(null);

  private storage: DockStorage | null;

  constructor(storage: DockStorage | null = defaultStorage()) {
    this.storage = storage;
    const loaded = loadDock(storage);
    this.width = loaded.width;
    this.workspaces = loaded.workspaces;
  }

  get workspace(): DockWorkspace | null {
    return this.currentKey === null ? null : (this.workspaces[this.currentKey] ?? null);
  }

  get tabs(): DockTab[] {
    return this.workspace?.tabs ?? [];
  }

  get activeTabId(): string | null {
    return this.workspace?.activeTabId ?? null;
  }

  get activeTab(): DockTab | null {
    const workspace = this.workspace;
    if (workspace === null) return null;
    return workspace.tabs.find((tab) => tab.id === workspace.activeTabId) ?? null;
  }

  get activeTool(): DockToolId | null {
    return this.activeTab?.tool ?? null;
  }

  get collapsed(): boolean {
    return this.workspace?.collapsed ?? true;
  }

  get isOpen(): boolean {
    const workspace = this.workspace;
    return workspace !== null && !workspace.collapsed && workspace.tabs.length > 0;
  }

  /** Points the dock at a session. Only a workspace change swaps the tab set. */
  setContext(context: DockContext | null): void {
    this.context = context;
    if (context === null) {
      this.currentKey = null;
      return;
    }
    const key = workspaceDockKey(context.nodeId, context.localPath);
    if (key === this.currentKey) return;
    this.currentKey = key;
    if (this.workspaces[key] === undefined) {
      this.workspaces = pruneWorkspaces({ ...this.workspaces, [key]: emptyWorkspace() }, key);
    }
  }

  /** Opens a tool or focuses its existing tab. */
  openTool(toolId: DockToolId): void {
    if (this.currentKey === null) return;
    const definition = dockTool(toolId);
    this.update((workspace) => {
      const existing = definition.singleton
        ? workspace.tabs.find((tab) => tab.tool === toolId)
        : undefined;
      if (existing !== undefined) {
        return { ...workspace, activeTabId: existing.id, collapsed: false };
      }

      const tab: DockTab = { id: tabKey(toolId, null), tool: toolId, resource: null };
      const tabs = trimTabs([...workspace.tabs, tab], tab.id, workspace.activeTabId);
      return { ...workspace, tabs, activeTabId: tab.id, collapsed: false };
    });
  }

  activateTab(id: string): void {
    const workspace = this.workspace;
    if (workspace === null || !workspace.tabs.some((tab) => tab.id === id)) return;
    if (workspace.activeTabId === id && !workspace.collapsed) return;
    this.update((current) => ({ ...current, activeTabId: id, collapsed: false }));
  }

  closeTab(id: string): void {
    const workspace = this.workspace;
    if (workspace === null) return;
    const index = workspace.tabs.findIndex((tab) => tab.id === id);
    if (index === -1) return;

    const tabs = workspace.tabs.filter((tab) => tab.id !== id);
    if (tabs.length === 0) {
      this.update((current) => ({ ...current, tabs: [], activeTabId: null, collapsed: true }));
      return;
    }
    const next = tabs[Math.min(index, tabs.length - 1)] ?? tabs[0];
    const activeTabId = workspace.activeTabId === id ? (next?.id ?? null) : workspace.activeTabId;
    this.update((current) => ({ ...current, tabs, activeTabId, collapsed: false }));
  }

  /** Rail behavior: focus the tool, or collapse the dock when it is already active. */
  toggleTool(toolId: DockToolId): void {
    if (this.isOpen && this.activeTool === toolId) {
      this.collapse();
      return;
    }
    this.openTool(toolId);
  }

  collapse(): void {
    this.setCollapsed(true);
  }

  setWidth(percent: number): void {
    if (!Number.isFinite(percent)) return;
    const next = clampWidth(percent);
    if (next === this.width) return;
    this.width = next;
  }

  /** Persists the width after a drag; moves update the live value only. */
  commitWidth(): void {
    this.persist();
  }

  private setCollapsed(collapsed: boolean): void {
    const workspace = this.workspace;
    if (workspace === null || workspace.collapsed === collapsed) return;
    this.update((current) => ({ ...current, collapsed }));
  }

  private update(change: (workspace: DockWorkspace) => DockWorkspace): void {
    const key = this.currentKey;
    if (key === null) return;
    const current = this.workspaces[key] ?? emptyWorkspace();
    const next: DockWorkspace = { ...change(current), updatedAt: Date.now() };
    const workspaces = { ...this.workspaces, [key]: next };
    this.workspaces = pruneWorkspaces(workspaces, key);
    this.persist();
  }

  private persist(): void {
    if (this.storage === null) return;
    try {
      this.storage.setItem(STORAGE_KEY, serialize(this.workspaces, this.width));
    } catch {
      /* Ignore quota / storage errors. */
    }
  }
}

/** Keeps tabs under the cap by dropping the oldest inactive, non-singleton tab. */
function trimTabs(tabs: DockTab[], keepId: string, activeId: string | null): DockTab[] {
  let next = tabs;
  while (next.length > MAX_TABS) {
    const index = next.findIndex(
      (tab) => tab.id !== keepId && tab.id !== activeId && !isSingleton(tab.tool),
    );
    if (index === -1) break;
    next = [...next.slice(0, index), ...next.slice(index + 1)];
  }
  return next;
}

export const dock = new WorkspaceDockStore();
