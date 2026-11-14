import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProjectSummary, SessionEventEnvelope } from "#lib/generated/bindings.ts";
import type { ConnectionStore } from "./connection.svelte.ts";
import { SyncStore } from "./sync.svelte.ts";

class MockWebSocket {
  static readonly OPEN = 1;
  static instances: MockWebSocket[] = [];

  readyState = MockWebSocket.OPEN;
  #listeners = new Map<string, Array<(event: { data?: string }) => void>>();

  constructor(public readonly url: string) {
    MockWebSocket.instances.push(this);
  }

  addEventListener(type: string, listener: (event: { data?: string }) => void): void {
    const list = this.#listeners.get(type) ?? [];
    list.push(listener);
    this.#listeners.set(type, list);
  }

  send(_data: string): void {}

  close(): void {}

  emitMessage(payload: unknown): void {
    const data = JSON.stringify(payload);
    for (const listener of this.#listeners.get("message") ?? []) {
      listener({ data });
    }
  }
}

function makeProject(projectId: string, name: string): ProjectSummary {
  return {
    project_id: projectId,
    name,
    canonical_git_url: null,
    identity: { kind: "local", key: projectId },
    created_at: 1,
    updated_at: 1,
    bindings: [],
  };
}

function makeSessionCreatedEvent(sessionId: string, projectId: string): SessionEventEnvelope {
  return {
    event_id: "ev-1",
    session_id: sessionId,
    node_seq: 1,
    created_at: 10,
    payload: {
      type: "session_created",
      data: {
        node_id: "node-1",
        project_id: projectId,
        project_name: "demo-repo",
        local_path: "/tmp/demo-repo",
        git_branch: "main",
        is_worktree: false,
        agent_id: "opencode",
        parent_session_id: null,
        title: "Imported",
      },
    },
  };
}

function createStore() {
  let projectsList: ProjectSummary[] = [];
  const sessionsFn = vi.fn<() => Promise<never[]>>(async () => []);
  const nodesFn = vi.fn<() => Promise<never[]>>(async () => []);
  const projectsFn = vi.fn<() => Promise<ProjectSummary[]>>(async () => projectsList);

  const connection = {
    tokenRequired: false,
    client: {
      wsUrl: (path: string) => `ws://localhost${path}`,
      sessions: sessionsFn,
      nodes: nodesFn,
      projects: projectsFn,
    },
    probe: vi.fn<() => Promise<void>>(async () => {}),
  } as unknown as ConnectionStore;

  const store = new SyncStore(connection);
  return {
    store,
    sessionsFn,
    projectsFn,
    setProjects: (next: ProjectSummary[]) => {
      projectsList = next;
    },
  };
}

describe("SyncStore project refresh triggers", () => {
  const originalWebSocket = globalThis.WebSocket;

  beforeEach(() => {
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  afterEach(() => {
    vi.stubGlobal("WebSocket", originalWebSocket);
  });

  it("refreshes projects when a session_created event batch arrives", async () => {
    const { store, sessionsFn, projectsFn, setProjects } = createStore();
    store.start();
    await vi.waitFor(() => expect(projectsFn).toHaveBeenCalledTimes(1));

    const ws = MockWebSocket.instances[0];
    expect(ws).toBeDefined();

    setProjects([makeProject("local:node-1:abc", "demo-repo")]);
    ws.emitMessage({
      op: "event_batch",
      cursor: 1,
      events: [makeSessionCreatedEvent("sess-1", "local:node-1:abc")],
    });

    await vi.waitFor(() => expect(projectsFn).toHaveBeenCalledTimes(2));
    expect(sessionsFn).toHaveBeenCalledTimes(2);
    expect(store.projects.map((p) => p.name)).toEqual(["demo-repo"]);
    store.stop();
  });

  it("refreshes projects when projects_updated arrives over WS", async () => {
    const { store, projectsFn, setProjects } = createStore();
    store.start();
    await vi.waitFor(() => expect(projectsFn).toHaveBeenCalledTimes(1));

    const ws = MockWebSocket.instances[0];
    setProjects([makeProject("local:node-1:xyz", "updated-repo")]);
    ws.emitMessage({ op: "projects_updated" });

    await vi.waitFor(() => expect(projectsFn).toHaveBeenCalledTimes(2));
    expect(store.projects.map((p) => p.name)).toEqual(["updated-repo"]);
    store.stop();
  });
});
