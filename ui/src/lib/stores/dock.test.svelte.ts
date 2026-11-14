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
  it("opens a fixed tool and expands the dock", () => {
    const { store } = newStore();
    store.setContext(context("s1"));

    expect(store.isOpen).toBe(false);
    store.openTool("files");

    expect(store.activeTool).toBe("files");
    expect(store.isOpen).toBe(true);
  });

  it("toggles the dock when the active tool is selected again", () => {
    const { store } = newStore();
    store.setContext(context("s1"));
    store.openTool("terminal");

    store.toggleTool("terminal");
    expect(store.collapsed).toBe(true);
    expect(store.activeTool).toBe("terminal");

    store.toggleTool("terminal");
    expect(store.collapsed).toBe(false);
    expect(store.activeTool).toBe("terminal");
  });

  it("keeps state when another session in the same workspace opens", () => {
    const { store } = newStore();
    store.setContext(context("s1", "/repo"));
    store.openTool("files");
    store.openFile("/repo/src/main.ts", "view");

    store.setContext(context("s2", "/repo"));

    expect(store.activeTool).toBe("files");
    expect(store.files).toHaveLength(1);
    expect(store.context?.sessionId).toBe("s2");
  });

  it("swaps the state when the workspace changes", () => {
    const { store } = newStore();
    store.setContext(context("s1", "/repo"));
    store.openTool("terminal");

    store.setContext(context("s2", "/worktree"));
    expect(store.activeTool).toBeNull();
    expect(store.isOpen).toBe(false);

    store.setContext(context("s3", "/repo"));
    expect(store.activeTool).toBe("terminal");
    expect(store.isOpen).toBe(true);
  });

  it("opens, focuses and updates file previews inside Files", () => {
    const { store } = newStore();
    store.setContext(context("s1"));

    store.openFile("/repo/src/main.ts", "view");
    expect(store.activeTool).toBe("files");
    expect(store.activeFile).toBe("/repo/src/main.ts");
    expect(store.activeFileTab?.mode).toBe("view");
    expect(store.activeFileTab?.scope).toBe("uncommitted");

    store.openFile("/repo/src/main.ts", "diff", "branch_base");
    expect(store.files).toHaveLength(1);
    expect(store.activeFileTab?.mode).toBe("diff");
    expect(store.activeFileTab?.scope).toBe("branch_base");

    store.openFile("/repo/src/other.ts", "view");
    expect(store.files).toHaveLength(2);
    expect(store.activeFile).toBe("/repo/src/other.ts");

    store.activateFile("/repo/src/main.ts");
    expect(store.activeFile).toBe("/repo/src/main.ts");
    expect(store.activeFileTab?.mode).toBe("diff");

    store.setFileMode("/repo/src/main.ts", "view");
    expect(store.activeFileTab?.mode).toBe("view");
  });

  it("selects a neighbor after closing the active file preview", () => {
    const { store } = newStore();
    store.setContext(context("s1"));
    store.openFile("/repo/a.ts", "view");
    store.openFile("/repo/b.ts", "view");
    store.openFile("/repo/c.ts", "view");

    store.activateFile("/repo/b.ts");
    store.closeFile("/repo/b.ts");
    expect(store.activeFile).toBe("/repo/c.ts");

    store.closeFile("/repo/a.ts");
    store.closeFile("/repo/c.ts");
    expect(store.files).toHaveLength(0);
    expect(store.activeFile).toBeNull();
    expect(store.activeTool).toBe("files");
  });

  it("toggles the Files tree and remembers the choice", () => {
    const { store } = newStore();
    store.setContext(context("s1"));

    expect(store.treeVisible).toBe(true);
    store.setTreeVisible(false);
    expect(store.treeVisible).toBe(false);

    store.openTool("changes");
    expect(store.treeVisible).toBe(false);
  });

  it("restores the active tool, files, tree and width", () => {
    const { store, storage } = newStore();
    store.setContext(context("s1"));
    store.openFile("/repo/src/main.ts", "diff", "branch_base");
    store.openFile("/repo/README.md", "view");
    store.setTreeVisible(false);
    store.setWidth(60);
    store.commitWidth();

    const restored = new WorkspaceDockStore(storage);
    restored.setContext(context("s2"));

    expect(restored.width).toBe(60);
    expect(restored.activeTool).toBe("files");
    expect(restored.treeVisible).toBe(false);
    expect(restored.activeFile).toBe("/repo/README.md");
    expect(restored.files.map((file) => file.path)).toEqual([
      "/repo/src/main.ts",
      "/repo/README.md",
    ]);
    expect(restored.files[0]?.mode).toBe("diff");
    expect(restored.files[0]?.scope).toBe("branch_base");
  });

  it("drops invalid persisted tools and file entries", () => {
    const storage = new MemoryStorage();
    storage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        version: 2,
        width: 42,
        workspaces: {
          [workspaceDockKey("node-1", "/repo")]: {
            activeTool: "file",
            files: [
              { path: "", mode: "view", scope: "uncommitted" },
              { path: "/repo/a.ts", mode: "bogus", scope: "bogus" },
              { path: "/repo/a.ts", mode: "diff", scope: "branch_base" },
            ],
            activeFile: "/repo/missing.ts",
            collapsed: false,
            treeVisible: false,
            updatedAt: 1,
          },
        },
      }),
    );

    const store = new WorkspaceDockStore(storage);
    store.setContext(context("s1"));

    expect(store.activeTool).toBeNull();
    expect(store.isOpen).toBe(false);
    expect(store.files).toHaveLength(1);
    expect(store.files[0]?.path).toBe("/repo/a.ts");
    expect(store.files[0]?.mode).toBe("view");
    expect(store.files[0]?.scope).toBe("uncommitted");
    expect(store.activeFile).toBe("/repo/a.ts");
    expect(store.treeVisible).toBe(false);
  });

  it("ignores storage written by an unknown version", () => {
    const storage = new MemoryStorage();
    storage.setItem(STORAGE_KEY, JSON.stringify({ version: 999, width: 60, workspaces: {} }));

    const store = new WorkspaceDockStore(storage);
    store.setContext(context("s1"));

    expect(store.width).toBe(42);
    expect(store.activeTool).toBeNull();
    expect(store.files).toHaveLength(0);
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

  it("caps open file previews", () => {
    const { store } = newStore();
    store.setContext(context("s1"));
    for (let index = 0; index < 10; index += 1) {
      store.openFile(`/repo/src/file-${index}.ts`, "view");
    }

    expect(store.files.length).toBeLessThanOrEqual(8);
    expect(store.activeFile).toBe("/repo/src/file-9.ts");
    expect(store.files.some((file) => file.path === "/repo/src/file-9.ts")).toBe(true);
  });

  it("clears the current workspace when the context is removed", () => {
    const { store } = newStore();
    store.setContext(context("s1"));
    store.openTool("terminal");

    store.setContext(null);
    expect(store.isOpen).toBe(false);

    store.setContext(context("s2"));
    expect(store.activeTool).toBe("terminal");
  });
});
