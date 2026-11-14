import { describe, expect, it } from "vitest";
import {
  WorkspaceDockStore,
  workspaceDockKey,
  type DockContext,
  type DockStorage,
} from "./dock.svelte.ts";

const STORAGE_KEY = "fxg:workspace_dock";

class MemoryStorage implements DockStorage {
  readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
}

function context(sessionId: string, localPath = "/repo"): DockContext {
  return { sessionId, nodeId: "node-1", localPath };
}

function newStore(storage = new MemoryStorage()): {
  store: WorkspaceDockStore;
  storage: MemoryStorage;
} {
  return { store: new WorkspaceDockStore(storage), storage };
}

describe("WorkspaceDockStore", () => {
  it("opens a tool, activates it and expands the dock", () => {
    const { store } = newStore();
    store.setContext(context("s1"));

    expect(store.isOpen).toBe(false);
    store.openTool("changes");

    expect(store.tabs).toHaveLength(1);
    expect(store.activeTool).toBe("changes");
    expect(store.isOpen).toBe(true);
  });

  it("does not duplicate a singleton tool", () => {
    const { store } = newStore();
    store.setContext(context("s1"));

    store.openTool("terminal");
    store.openTool("changes");
    store.openTool("terminal");

    expect(store.tabs).toHaveLength(2);
    expect(store.activeTool).toBe("terminal");
  });

  it("keeps tabs when another session in the same workspace opens", () => {
    const { store } = newStore();
    store.setContext(context("s1", "/repo"));
    store.openTool("terminal");

    store.setContext(context("s2", "/repo"));

    expect(store.tabs).toHaveLength(1);
    expect(store.context?.sessionId).toBe("s2");
    expect(store.isOpen).toBe(true);
  });

  it("swaps the tab set when the workspace changes", () => {
    const { store } = newStore();
    store.setContext(context("s1", "/repo"));
    store.openTool("terminal");

    store.setContext(context("s2", "/worktree"));
    expect(store.tabs).toHaveLength(0);
    expect(store.isOpen).toBe(false);

    store.setContext(context("s3", "/repo"));
    expect(store.tabs).toHaveLength(1);
    expect(store.activeTool).toBe("terminal");
  });

  it("activates a neighbor after closing the active tab", () => {
    const { store } = newStore();
    store.setContext(context("s1"));
    store.openTool("terminal");
    store.openTool("changes");

    const changesId = store.activeTabId;
    expect(changesId).not.toBeNull();
    store.closeTab(changesId ?? "");

    expect(store.activeTool).toBe("terminal");
    expect(store.isOpen).toBe(true);

    store.closeTab(store.activeTabId ?? "");
    expect(store.tabs).toHaveLength(0);
    expect(store.activeTabId).toBeNull();
    expect(store.collapsed).toBe(true);
  });

  it("toggles the dock when the active tool is selected again", () => {
    const { store } = newStore();
    store.setContext(context("s1"));
    store.openTool("terminal");

    store.toggleTool("terminal");
    expect(store.collapsed).toBe(true);
    expect(store.tabs).toHaveLength(1);

    store.toggleTool("terminal");
    expect(store.collapsed).toBe(false);
    expect(store.activeTool).toBe("terminal");
  });

  it("restores tabs, the active tab and the width", () => {
    const { store, storage } = newStore();
    store.setContext(context("s1"));
    store.openTool("terminal");
    store.openTool("changes");
    store.setWidth(60);
    store.commitWidth();

    const restored = new WorkspaceDockStore(storage);
    restored.setContext(context("s2"));

    expect(restored.width).toBe(60);
    expect(restored.tabs.map((tab) => tab.tool)).toEqual(["terminal", "changes"]);
    expect(restored.activeTool).toBe("changes");
    expect(restored.isOpen).toBe(true);
  });

  it("drops unknown and duplicate persisted tabs", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        version: 1,
        width: 42,
        workspaces: {
          [workspaceDockKey("node-1", "/repo")]: {
            tabs: [
              { tool: "unknown", resource: null },
              { tool: "terminal", resource: null },
              { tool: "terminal", resource: null },
            ],
            activeIndex: 99,
            collapsed: false,
            updatedAt: 1,
          },
        },
      }),
    );

    const store = new WorkspaceDockStore(storage);
    store.setContext(context("s1"));

    expect(store.tabs.map((tab) => tab.tool)).toEqual(["terminal"]);
    expect(store.activeTool).toBe("terminal");
    expect(store.isOpen).toBe(true);
  });

  it("ignores storage written by an unknown version", () => {
    const storage = new MemoryStorage();
    storage.setItem(STORAGE_KEY, JSON.stringify({ version: 999, width: 60, workspaces: {} }));

    const store = new WorkspaceDockStore(storage);
    store.setContext(context("s1"));

    expect(store.width).toBe(42);
    expect(store.tabs).toHaveLength(0);
  });

  it("clamps the width", () => {
    const { store } = newStore();

    store.setWidth(95);
    expect(store.width).toBe(75);

    store.setWidth(5);
    expect(store.width).toBe(20);

    store.setWidth(Number.NaN);
    expect(store.width).toBe(20);
  });

  it("keeps only the most recently used workspaces", () => {
    const { store } = newStore();
    for (let index = 0; index < 14; index += 1) {
      store.setContext(context(`s${index}`, `/repo/${index}`));
      store.openTool("changes");
    }

    expect(Object.keys(store.workspaces).length).toBeLessThanOrEqual(12);
    expect(store.workspaces[workspaceDockKey("node-1", "/repo/13")]).toBeDefined();
  });

  it("clears the current workspace when the context is removed", () => {
    const { store } = newStore();
    store.setContext(context("s1"));
    store.openTool("terminal");

    store.setContext(null);
    expect(store.isOpen).toBe(false);

    store.setContext(context("s2"));
    expect(store.tabs).toHaveLength(1);
  });

  it("opens file tabs once and switches their mode", () => {
    const { store } = newStore();
    store.setContext(context("s1"));

    store.openFile("/repo/src/main.ts", "view");
    expect(store.activeTool).toBe("file");
    expect(store.activeTab?.resource).toBe("/repo/src/main.ts");
    expect(store.activeTab?.mode).toBe("view");
    expect(store.activeTab?.scope).toBe("uncommitted");

    store.openFile("/repo/src/main.ts", "diff", "branch_base");
    expect(store.tabs).toHaveLength(1);
    expect(store.activeTab?.mode).toBe("diff");
    expect(store.activeTab?.scope).toBe("branch_base");

    store.openFile("/repo/src/other.ts", "view");
    expect(store.tabs).toHaveLength(2);
    expect(store.activeTab?.resource).toBe("/repo/src/other.ts");

    const first = store.tabs[0];
    expect(first).toBeDefined();
    store.setTabMode(first?.id ?? "", "diff");
    expect(store.tabs[0]?.mode).toBe("diff");
  });

  it("persists file tabs with their mode", () => {
    const { store, storage } = newStore();
    store.setContext(context("s1"));
    store.openFile("/repo/src/main.ts", "diff", "branch_base");

    const restored = new WorkspaceDockStore(storage);
    restored.setContext(context("s2"));

    expect(restored.tabs).toHaveLength(1);
    expect(restored.tabs[0]?.tool).toBe("file");
    expect(restored.tabs[0]?.resource).toBe("/repo/src/main.ts");
    expect(restored.tabs[0]?.mode).toBe("diff");
    expect(restored.tabs[0]?.scope).toBe("branch_base");
  });

  it("drops file tabs without a resource when restoring", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        version: 1,
        width: 42,
        workspaces: {
          [workspaceDockKey("node-1", "/repo")]: {
            tabs: [{ tool: "file", resource: null, mode: "view" }],
            activeIndex: 0,
            collapsed: false,
            updatedAt: 1,
          },
        },
      }),
    );

    const store = new WorkspaceDockStore(storage);
    store.setContext(context("s1"));
    expect(store.tabs).toHaveLength(0);
  });

  it("caps file tabs while keeping singleton tools", () => {
    const { store } = newStore();
    store.setContext(context("s1"));
    store.openTool("terminal");
    for (let index = 0; index < 10; index += 1) {
      store.openFile(`/repo/src/file-${index}.ts`, "view");
    }

    expect(store.tabs.length).toBeLessThanOrEqual(8);
    expect(store.tabs.some((tab) => tab.tool === "terminal")).toBe(true);
    expect(store.tabs.some((tab) => tab.resource === "/repo/src/file-9.ts")).toBe(true);
  });
});
