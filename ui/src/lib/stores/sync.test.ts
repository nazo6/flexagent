import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ProjectSummary,
  SessionEventEnvelope,
  SessionListResponse,
  SessionSummary,
} from "#lib/generated/bindings.ts";
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

  emitClose(): void {
    for (const listener of this.#listeners.get("close") ?? []) {
      listener({});
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

function makeSessionSummary(
  sessionId: string,
  lastNodeSeq: number,
  projectId = "local:node-1:demo",
): SessionSummary {
  return {
    session_id: sessionId,
    project_id: projectId,
    node_id: "node-1",
    local_path: "/tmp/demo-repo",
    git_branch: "main",
    is_worktree: false,
    agent_id: "opencode",
    agent_session_id: null,
    parent_session_id: null,
    title: "Demo",
    status: "idle",
    current_mode: null,
    usage: null,
    last_node_seq: lastNodeSeq,
    created_at: 1,
    updated_at: 10,
    archived_at: null,
  };
}

function makeAgentMessageEvent(sessionId: string, nodeSeq: number): SessionEventEnvelope {
  return {
    event_id: `ev-${sessionId}-${nodeSeq}`,
    session_id: sessionId,
    node_seq: nodeSeq,
    created_at: 10 + nodeSeq,
    payload: {
      type: "agent_message",
      data: {
        message_id: `msg-${nodeSeq}`,
        text: "done",
        is_complete: true,
      },
    },
  };
}

function createStore() {
  let sessionsList: SessionSummary[] = [];
  let projectsList: ProjectSummary[] = [];
  const sessionsFn = vi.fn<() => Promise<SessionListResponse>>(async () => ({
    sessions: sessionsList,
    next_cursor: null,
  }));
  const nodesFn = vi.fn<() => Promise<never[]>>(async () => []);
  const projectsFn = vi.fn<() => Promise<ProjectSummary[]>>(async () => projectsList);
  const sessionEventsFn = vi.fn<
    () => Promise<{
      events: SessionEventEnvelope[];
      cursor: number;
      first_cursor: number;
    }>
  >(async () => ({
    events: [] as SessionEventEnvelope[],
    cursor: 0,
    first_cursor: 0,
  }));
  const sessionEventsBeforeFn = vi.fn<
    () => Promise<{
      events: SessionEventEnvelope[];
      cursor: number;
      first_cursor: number;
    }>
  >(async () => ({
    events: [] as SessionEventEnvelope[],
    cursor: 0,
    first_cursor: 0,
  }));

  const connection = {
    tokenRequired: false,
    systemInfo: { latest_event_cursor: 0 },
    client: {
      wsUrl: (path: string) => `ws://localhost${path}`,
      sessions: sessionsFn,
      nodes: nodesFn,
      projects: projectsFn,
      sessionEvents: sessionEventsFn,
      sessionEventsBefore: sessionEventsBeforeFn,
    },
    probe: vi.fn<() => Promise<void>>(async () => {}),
  } as unknown as ConnectionStore;

  const store = new SyncStore(connection);
  return {
    store,
    sessionsFn,
    projectsFn,
    sessionEventsFn,
    sessionEventsBeforeFn,
    setSessions: (next: SessionSummary[]) => {
      sessionsList = next;
    },
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

  it("does not reconnect from a socket replaced by restart", async () => {
    const { store } = createStore();
    store.start();
    await vi.waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const first = MockWebSocket.instances[0];

    store.restart();
    await vi.waitFor(() => expect(MockWebSocket.instances).toHaveLength(2));
    first.emitClose();

    // The reconnect delay is 1s; a stale close must not create a third socket.
    await new Promise((resolve) => setTimeout(resolve, 1_100));
    expect(MockWebSocket.instances).toHaveLength(2);
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

describe("SyncStore unread tracking", () => {
  const originalWebSocket = globalThis.WebSocket;

  beforeEach(() => {
    localStorage.clear();
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  afterEach(() => {
    vi.stubGlobal("WebSocket", originalWebSocket);
  });

  it("flags completed agent messages received while the session is not open", async () => {
    const { store, setSessions } = createStore();
    setSessions([makeSessionSummary("sess-1", 1)]);
    store.start();
    await vi.waitFor(() => expect(store.sessions.length).toBe(1));
    const ws = MockWebSocket.instances[0];

    ws.emitMessage({
      op: "event_batch",
      cursor: 2,
      events: [makeAgentMessageEvent("sess-1", 2)],
    });
    expect(store.hasUnread("sess-1")).toBe(true);

    store.setActiveSession("sess-1");
    expect(store.hasUnread("sess-1")).toBe(false);

    // Output received while the session is open is already being read.
    ws.emitMessage({
      op: "event_batch",
      cursor: 3,
      events: [makeAgentMessageEvent("sess-1", 3)],
    });
    expect(store.hasUnread("sess-1")).toBe(false);

    store.setActiveSession(null);
    ws.emitMessage({
      op: "event_batch",
      cursor: 4,
      events: [makeAgentMessageEvent("sess-1", 4)],
    });
    expect(store.hasUnread("sess-1")).toBe(true);
    store.stop();
  });

  it("ignores replayed messages at or below the read watermark", async () => {
    localStorage.setItem("fxg:session_read", JSON.stringify({ read: { "sess-1": 5 }, unread: [] }));
    const { store, setSessions } = createStore();
    setSessions([makeSessionSummary("sess-1", 5)]);
    store.start();
    await vi.waitFor(() => expect(store.sessions.length).toBe(1));
    const ws = MockWebSocket.instances[0];

    ws.emitMessage({
      op: "event_batch",
      cursor: 5,
      events: [makeAgentMessageEvent("sess-1", 5)],
    });
    expect(store.hasUnread("sess-1")).toBe(false);
    store.stop();
  });
});

describe("SyncStore session list pagination", () => {
  const originalWebSocket = globalThis.WebSocket;

  beforeEach(() => {
    localStorage.clear();
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  afterEach(() => {
    vi.stubGlobal("WebSocket", originalWebSocket);
  });

  it("appends the next page of one project with its own cursor", async () => {
    const { store, sessionsFn } = createStore();
    const projectId = "local:node-1:demo";
    sessionsFn.mockResolvedValueOnce({
      sessions: [makeSessionSummary("sess-1", 1, projectId)],
      next_cursor: "global-cursor",
      project_cursors: [{ project_id: projectId, next_cursor: "project-cursor" }],
    });
    sessionsFn.mockResolvedValueOnce({
      sessions: [makeSessionSummary("sess-2", 1, projectId)],
      next_cursor: null,
      project_cursors: [{ project_id: projectId, next_cursor: null }],
    });

    await store.refreshSessions();
    expect(store.sessions.map((session) => session.session_id)).toEqual(["sess-1"]);
    expect(store.hasMoreSessions(projectId)).toBe(true);

    await store.loadMoreSessions(projectId);
    expect(store.sessions.map((session) => session.session_id)).toEqual(["sess-1", "sess-2"]);
    expect(store.hasMoreSessions(projectId)).toBe(false);
    expect(sessionsFn).toHaveBeenLastCalledWith({
      includeArchived: true,
      limit: 100,
      projectId,
      cursor: "project-cursor",
    });
  });

  it("loads a project's first page when it has no cursor yet", async () => {
    const { store, sessionsFn } = createStore();
    const projectId = "local:node-1:empty";
    sessionsFn.mockResolvedValueOnce({
      sessions: [],
      next_cursor: null,
    });

    expect(store.hasLoadedProjectSessions(projectId)).toBe(false);
    expect(store.hasMoreSessions(projectId)).toBe(true);

    await store.loadMoreSessions(projectId);
    expect(store.hasLoadedProjectSessions(projectId)).toBe(true);
    expect(store.hasMoreSessions(projectId)).toBe(false);
    expect(sessionsFn).toHaveBeenLastCalledWith({
      includeArchived: true,
      limit: 100,
      projectId,
      cursor: null,
    });
  });

  it("does not roll back a loaded project cursor on refresh", async () => {
    const { store, sessionsFn } = createStore();
    const projectId = "local:node-1:demo";
    sessionsFn.mockResolvedValueOnce({
      sessions: [makeSessionSummary("sess-1", 1, projectId)],
      project_cursors: [{ project_id: projectId, next_cursor: "project-cursor" }],
    });
    sessionsFn.mockResolvedValueOnce({
      sessions: [makeSessionSummary("sess-2", 1, projectId)],
      next_cursor: null,
    });
    sessionsFn.mockResolvedValueOnce({
      sessions: [makeSessionSummary("sess-1", 2, projectId)],
      project_cursors: [{ project_id: projectId, next_cursor: "project-cursor" }],
    });

    await store.refreshSessions();
    await store.loadMoreSessions(projectId);
    expect(store.hasMoreSessions(projectId)).toBe(false);

    await store.refreshSessions();
    expect(store.hasMoreSessions(projectId)).toBe(false);
  });
});

describe("SyncStore chat history pagination", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("loads the newest page and exposes older pages through the cursor", async () => {
    const { store, sessionEventsBeforeFn } = createStore();
    const tail = [makeAgentMessageEvent("sess-1", 5), makeAgentMessageEvent("sess-1", 6)];
    sessionEventsBeforeFn.mockResolvedValueOnce({ events: tail, cursor: 60, first_cursor: 50 });

    await store.loadSessionEvents("sess-1");
    const timeline = store.timelineFor("sess-1");
    expect(timeline.events.map((event) => event.node_seq)).toEqual([5, 6]);
    expect(timeline.historyLoaded).toBe(true);
    // A short tail reaches the session start, so nothing older remains.
    expect(timeline.hasOlder).toBe(false);
    expect(sessionEventsBeforeFn).toHaveBeenCalledWith("sess-1", null, 200);

    // Simulate a long session whose tail page is followed by an older page.
    timeline.hasOlder = true;
    timeline.olderCursor = 50;
    sessionEventsBeforeFn.mockResolvedValueOnce({
      events: [makeAgentMessageEvent("sess-1", 3), makeAgentMessageEvent("sess-1", 4)],
      cursor: 40,
      first_cursor: 30,
    });
    await store.loadOlderSessionEvents("sess-1");
    expect(timeline.events.map((event) => event.node_seq)).toEqual([3, 4, 5, 6]);
    expect(sessionEventsBeforeFn).toHaveBeenLastCalledWith("sess-1", 50, 200);
    // Fewer events than a full page means the session start was reached.
    expect(timeline.hasOlder).toBe(false);
    expect(timeline.olderCursor).toBeNull();
  });
});

describe("SyncStore long session history", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("keeps the session head aside and merges it when paging reaches it", async () => {
    const { store, sessionEventsFn, sessionEventsBeforeFn } = createStore();
    const tail = Array.from({ length: 200 }, (_, index) =>
      makeAgentMessageEvent("sess-1", 401 + index),
    );
    sessionEventsBeforeFn.mockResolvedValueOnce({ events: tail, cursor: 600, first_cursor: 401 });
    const head = Array.from({ length: 200 }, (_, index) =>
      makeAgentMessageEvent("sess-1", 1 + index),
    );
    sessionEventsFn.mockResolvedValueOnce({ events: head, cursor: 200, first_cursor: 1 });

    await store.loadSessionEvents("sess-1");
    const timeline = store.timelineFor("sess-1");
    // Only the newest page is rendered; the head supplies session metadata.
    expect(timeline.events).toHaveLength(200);
    expect(timeline.events[0].node_seq).toBe(401);
    expect(timeline.headEvents).toHaveLength(200);
    expect(timeline.metadataEvents()).toHaveLength(400);
    expect(timeline.hasOlder).toBe(true);
    expect(timeline.olderCursor).toBe(401);

    const middle = Array.from({ length: 200 }, (_, index) =>
      makeAgentMessageEvent("sess-1", 201 + index),
    );
    sessionEventsBeforeFn.mockResolvedValueOnce({ events: middle, cursor: 400, first_cursor: 201 });
    await store.loadOlderSessionEvents("sess-1");
    expect(timeline.events.map((event) => event.node_seq)[0]).toBe(1);
    expect(timeline.events).toHaveLength(600);
    expect(timeline.headEvents).toHaveLength(0);
    expect(timeline.hasOlder).toBe(false);
    expect(timeline.olderCursor).toBeNull();
  });
});
