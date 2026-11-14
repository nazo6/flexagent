import { isDockToolId, type DockLauncherId, type DockToolId } from "#lib/dock/registry.ts";

/** Session context handed to the dock. The workspace, not the session, owns the tools. */
export interface DockContext {
  sessionId: string;
  nodeId: string;
  localPath: string;
}

/** How a file preview is displayed. */
export type DockFileMode = "view" | "diff";

/** Diff scope remembered by a file preview. */
export type DockDiffScope = "uncommitted" | "branch_base";

/** One open file preview inside the Files tool. */
export interface DockFileTab {
  path: string;
  mode: DockFileMode;
  scope: DockDiffScope;
}

export interface DockWorkspace {
  /** Fixed outer tool currently shown (`null` = the dock is closed). */
  activeTool: DockToolId | null;
  collapsed: boolean;
  /** Whether the Files tree is visible next to the preview. */
  treeVisible: boolean;
  /** Open file previews inside Files. */
  files: DockFileTab[];
  activeFile: string | null;
  updatedAt: number;
}

/** The subset of `Storage` the dock needs (injectable for tests). */
export interface DockStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const STORAGE_KEY = "fxg:workspace_dock";
const STORAGE_VERSION = 2;
/** Keep the most recently used workspaces; each one carries a few file previews. */
const MAX_WORKSPACES = 12;
const MAX_FILES = 8;
export const MIN_DOCK_WIDTH = 20;
export const MAX_DOCK_WIDTH = 75;
const DEFAULT_WIDTH = 42;

interface PersistedFile {
  path: string;
  mode: DockFileMode;
  scope: DockDiffScope;
}

interface PersistedWorkspace {
  activeTool: DockToolId | null;
  collapsed: boolean;
  treeVisible: boolean;
  files: PersistedFile[];
  activeFile: string | null;
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

function clampWidth(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_WIDTH;
  return Math.max(MIN_DOCK_WIDTH, Math.min(MAX_DOCK_WIDTH, value));
}

function emptyWorkspace(): DockWorkspace {
  return {
    activeTool: null,
    collapsed: true,
    treeVisible: true,
    files: [],
    activeFile: null,
    updatedAt: Date.now(),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Restores and validates one persisted workspace; unknown tools are dropped. */
function restoreWorkspace(value: unknown): DockWorkspace | null {
  if (!isRecord(value)) return null;

  const activeTool = isDockToolId(value.activeTool) ? value.activeTool : null;
  const files: DockFileTab[] = [];
  if (Array.isArray(value.files)) {
    for (const raw of value.files) {
      if (!isRecord(raw) || typeof raw.path !== "string" || raw.path.length === 0) continue;
      if (files.some((file) => file.path === raw.path)) continue;
      const mode: DockFileMode = raw.mode === "diff" ? "diff" : "view";
      const scope: DockDiffScope = raw.scope === "branch_base" ? "branch_base" : "uncommitted";
      files.push({ path: raw.path, mode, scope });
      if (files.length >= MAX_FILES) break;
    }
  }
  const activeFile =
    typeof value.activeFile === "string" && files.some((file) => file.path === value.activeFile)
      ? value.activeFile
      : (files[0]?.path ?? null);

  const updatedAt =
    typeof value.updatedAt === "number" && Number.isFinite(value.updatedAt)
      ? value.updatedAt
      : Date.now();

  return {
    activeTool,
    collapsed: activeTool === null ? true : value.collapsed !== false,
    treeVisible: value.treeVisible !== false,
    files,
    activeFile,
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
          activeTool: workspace.activeTool,
          collapsed: workspace.collapsed || workspace.activeTool === null,
          treeVisible: workspace.treeVisible,
          files: workspace.files.map((file) => ({
            path: file.path,
            mode: file.mode,
            scope: file.scope,
          })),
          activeFile: workspace.activeFile,
          updatedAt: workspace.updatedAt,
        } satisfies PersistedWorkspace,
      ]),
    ),
  };
  return JSON.stringify(persisted);
}

/**
 * Workspace-scoped state for the session side pane.
 *
 * Outer tools (Files / Changes / Terminal) are fixed. Files owns a tree and a
 * preview, with variable file tabs inside the preview. A session is only the
 * context passed to the active tools; switching sessions inside one workspace
 * keeps the terminal and the Files state alive.
 */
export class WorkspaceDockStore {
  width = $state(DEFAULT_WIDTH);
  workspaces = $state<Record<string, DockWorkspace>>({});
  currentKey = $state<string | null>(null);
  context = $state<DockContext | null>(null);
  /** Increments on every openFile (used by mobile to reveal the preview). */
  fileOpenVersion = $state(0);

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

  get activeTool(): DockToolId | null {
    return this.workspace?.activeTool ?? null;
  }

  get collapsed(): boolean {
    return this.workspace?.collapsed ?? true;
  }

  get isOpen(): boolean {
    const workspace = this.workspace;
    return workspace !== null && !workspace.collapsed && workspace.activeTool !== null;
  }

  get treeVisible(): boolean {
    return this.workspace?.treeVisible ?? true;
  }

  get files(): DockFileTab[] {
    return this.workspace?.files ?? [];
  }

  get activeFile(): string | null {
    return this.workspace?.activeFile ?? null;
  }

  get activeFileTab(): DockFileTab | null {
    const workspace = this.workspace;
    if (workspace === null || workspace.activeFile === null) return null;
    return workspace.files.find((file) => file.path === workspace.activeFile) ?? null;
  }

  /** Points the dock at a session. Only a workspace change swaps the state. */
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

  /** Shows a fixed outer tool. */
  openTool(toolId: DockLauncherId): void {
    if (this.currentKey === null) return;
    this.update((workspace) => ({ ...workspace, activeTool: toolId, collapsed: false }));
  }

  /** Rail behavior: focus the tool, or collapse the dock when it is already active. */
  toggleTool(toolId: DockLauncherId): void {
    if (this.isOpen && this.activeTool === toolId) {
      this.collapse();
      return;
    }
    this.openTool(toolId);
  }

  collapse(): void {
    this.setCollapsed(true);
  }

  setTreeVisible(visible: boolean): void {
    const workspace = this.workspace;
    if (workspace === null || workspace.treeVisible === visible) return;
    this.update((current) => ({ ...current, treeVisible: visible }));
  }

  /** Opens a file preview inside Files (or focuses and updates an existing one). */
  openFile(path: string, mode: DockFileMode = "view", scope: DockDiffScope = "uncommitted"): void {
    if (this.currentKey === null) return;
    this.fileOpenVersion += 1;
    this.update((workspace) => {
      const existing = workspace.files.find((file) => file.path === path);
      const files =
        existing === undefined
          ? trimFiles([...workspace.files, { path, mode, scope }], path)
          : workspace.files.map((file) => (file.path === path ? { ...file, mode, scope } : file));
      return {
        ...workspace,
        files,
        activeFile: path,
        activeTool: "files",
        collapsed: false,
      };
    });
  }

  activateFile(path: string): void {
    const workspace = this.workspace;
    if (workspace === null || !workspace.files.some((file) => file.path === path)) return;
    if (workspace.activeFile === path && workspace.activeTool === "files" && !workspace.collapsed) {
      return;
    }
    this.update((current) => ({
      ...current,
      activeFile: path,
      activeTool: "files",
      collapsed: false,
    }));
  }

  closeFile(path: string): void {
    const workspace = this.workspace;
    if (workspace === null) return;
    const index = workspace.files.findIndex((file) => file.path === path);
    if (index === -1) return;
    const files = workspace.files.filter((file) => file.path !== path);
    const activeFile =
      workspace.activeFile === path
        ? (files[Math.min(index, files.length - 1)]?.path ?? null)
        : workspace.activeFile;
    this.update((current) => ({ ...current, files, activeFile }));
  }

  setFileMode(path: string, mode: DockFileMode): void {
    const workspace = this.workspace;
    if (workspace === null) return;
    const file = workspace.files.find((candidate) => candidate.path === path);
    if (file === undefined || file.mode === mode) return;
    this.update((current) => ({
      ...current,
      files: current.files.map((candidate) =>
        candidate.path === path ? { ...candidate, mode } : candidate,
      ),
    }));
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

/** Keeps file previews under the cap by dropping the oldest non-active one. */
function trimFiles(files: DockFileTab[], keepPath: string): DockFileTab[] {
  let next = files;
  while (next.length > MAX_FILES) {
    const index = next.findIndex((file) => file.path !== keepPath);
    if (index === -1) break;
    next = [...next.slice(0, index), ...next.slice(index + 1)];
  }
  return next;
}

export const dock = new WorkspaceDockStore();
