import { untrack } from "solid-js";
import { ApiError } from "#lib/api/errors.ts";
import type {
  ClientWsMessage,
  CommandResult,
  ElicitationAction,
  ElicitationValue,
  HubLinkStatus,
  NodeSummary,
  ProjectSummary,
  PromptInput,
  RespondElicitationResponse,
  RespondPermissionResponse,
  ResumeSessionResponse,
  ServerWsMessage,
  SessionArchiveResponse,
  SessionControlAction,
  SessionEventEnvelope,
  SessionRenameResponse,
  SessionSummary,
} from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { problems } from "#lib/stores/problems.ts";
import { ReactiveCell, ReactiveMap, ReactiveSet } from "#lib/stores/reactive.ts";
import { mergeCursor } from "#lib/sync/cursor.ts";
import { SessionTimeline } from "#lib/sync/timeline.ts";
import type { PendingPrompt } from "#lib/sync/reducer.ts";
import { randomUuid } from "#lib/uuid.ts";
import type { ConnectionStore } from "./connection.ts";

const CURSOR_KEY = "fxg:sync_cursor";
/** Per-device read / unread state for the sidebar. */
const READ_STATE_KEY = "fxg:session_read";
/** Timeout for waiting on a `CommandResult`. */
const COMMAND_TIMEOUT_MS = 30_000;
/**
 * Timeout for `SendPrompt`.
 *
 * Sending to a stopped session triggers a native restore (automatic resume) on
 * the node, so allow for an agent cold start (~25 seconds).
 */
const SEND_PROMPT_TIMEOUT_MS = 120_000;
/** Upper bound for reconnect backoff. */
const MAX_RECONNECT_DELAY_MS = 30_000;
/** Sessions per sidebar page (`GET /api/v1/sessions`). */
const SESSION_PAGE_SIZE = 100;
/** Events per chat history page (`before_cursor` paging). */
export const HISTORY_PAGE_SIZE = 200;
/**
 * Events retained for a session that is not open.
 *
 * Chat history is reloaded from the paginated events API when the session is
 * opened again, so keeping every live event of every session would only grow
 * memory without being displayed.
 */
const INACTIVE_EVENT_LIMIT = 200;

interface PendingCommand {
  sessionId: string | null;
  settle(result: CommandResult): void;
  reject(error: Error): void;
  timer: ReturnType<typeof setTimeout>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Session-list order shared with the server (`updated_at` desc, ID asc). */
function sortSessions(sessions: SessionSummary[]): SessionSummary[] {
  return sessions.toSorted((a, b) => {
    if (a.updated_at !== b.updated_at) return b.updated_at - a.updated_at;
    if (a.session_id === b.session_id) return 0;
    return a.session_id < b.session_id ? -1 : 1;
  });
}

function readCursor(): number | null {
  if (typeof localStorage === "undefined") return null;
  try {
    const raw = localStorage.getItem(CURSOR_KEY);
    if (raw === null) return null;
    const value = Number.parseInt(raw, 10);
    return Number.isFinite(value) && value >= 0 ? value : null;
  } catch {
    return null;
  }
}

function writeCursor(value: number): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(CURSOR_KEY, String(value));
  } catch {
    // Storage may be disabled or full; the cursor is only an optimization.
  }
}

interface ReadState {
  /** Highest `node_seq` the user has seen per session. */
  read: Map<string, number>;
  /** Sessions with a completed agent message the user has not seen yet. */
  unread: string[];
}

/** Load the per-device read state (invalid entries are dropped). */
function readReadState(): ReadState {
  const empty: ReadState = { read: new Map(), unread: [] };
  if (typeof localStorage === "undefined") return empty;
  try {
    const raw = localStorage.getItem(READ_STATE_KEY);
    if (raw === null) return empty;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return empty;
    const record = parsed as Record<string, unknown>;

    const read = new Map<string, number>();
    const storedRead = record.read;
    if (typeof storedRead === "object" && storedRead !== null && !Array.isArray(storedRead)) {
      for (const [sessionId, seq] of Object.entries(storedRead as Record<string, unknown>)) {
        if (sessionId !== "" && typeof seq === "number" && Number.isSafeInteger(seq) && seq >= 0) {
          read.set(sessionId, seq);
        }
      }
    }

    const unread = Array.isArray(record.unread)
      ? record.unread.filter(
          (sessionId): sessionId is string => typeof sessionId === "string" && sessionId !== "",
        )
      : [];
    return { read, unread };
  } catch {
    return empty;
  }
}

/**
 * Incremental sync store over the Client WS (`/api/v1/client/ws`).
 *
 * - On connect it sends `Subscribe { since_cursor, focused_session_id }`,
 *   receives a replay of unfetched events, then real-time events.
 * - It persists the `cursor` of the last received batch and resumes
 *   incrementally on reconnect (one cursor per target store; switching targets
 *   is an origin navigation, so localStorage is naturally isolated per origin).
 * - Session projections (`sessions`) and the node list are
 *   re-fetched from their REST APIs when events are applied (kept consistent
 *   with the hub-side projection).
 */
export class SyncStore {
  readonly connection: ConnectionStore;

  readonly #sessions = new ReactiveCell<SessionSummary[]>([]);
  readonly #sessionsLoading = new ReactiveCell(false);
  readonly #sessionsError = new ReactiveCell<string | null>(null);
  readonly #nodes = new ReactiveCell<NodeSummary[]>([]);
  readonly #nodesLoading = new ReactiveCell(false);
  readonly #nodesError = new ReactiveCell<string | null>(null);
  readonly #projects = new ReactiveCell<ProjectSummary[]>([]);
  readonly #projectsLoading = new ReactiveCell(false);
  readonly #projectsError = new ReactiveCell<string | null>(null);
  readonly #wsConnected = new ReactiveCell(false);
  readonly #hubLink = new ReactiveCell<HubLinkStatus | null>(null);
  readonly #unsyncedEventCount = new ReactiveCell(0);
  readonly #lastSyncedAt = new ReactiveCell<number | null>(null);
  readonly #activeSessionId = new ReactiveCell<string | null>(null);

  /** Prompts sent with no `CommandResult` yet (Pending Queue). */
  readonly pendingPrompts = new ReactiveMap<string, PendingPrompt>();
  /** Sessions with unread completed agent messages (per-device state). */
  readonly unreadSessions = new ReactiveSet<string>();
  /**
   * Progress logs for ephemeral VM provisioning (ephemeral; not persisted).
   *
   * The server delivers `stderr` output from before the node's `node.db` exists
   * as `ServerWsMessage::ProvisioningLog`. It is not restored on reconnect.
   */
  readonly provisioningLogs = new ReactiveMap<string, string[]>();

  /** Identity cache for the per-session timelines (not reactive state). */
  readonly #timelines = new Map<string, SessionTimeline>();
  #ws: WebSocket | null = null;
  #stopped = true;
  #reconnectDelayMs = 1_000;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #cursor: number | null = readCursor();
  /** Whether this page has already connected once (later connections resume from `#cursor`). */
  #hasConnected = false;
  /** Per-project cursor of the next older page (`null` = fully loaded; missing = not loaded). */
  readonly #projectCursors = new ReactiveMap<string, string | null>();
  /** Per-project "load older" in-flight flag. */
  readonly #projectLoading = new ReactiveMap<string, boolean>();
  readonly #pendingCommands = new Map<string, PendingCommand>();
  #sessionsRefreshQueued = false;
  #projectsRefreshQueued = false;
  readonly #readSeqs: Map<string, number>;
  #readStatePersistQueued = false;
  /** Whether the first session listing has initialized the read state. */
  #sessionsInitialized = false;

  constructor(connection: ConnectionStore) {
    this.connection = connection;
    const readState = readReadState();
    this.#readSeqs = readState.read;
    for (const sessionId of readState.unread) this.unreadSessions.add(sessionId);
  }

  get sessions(): SessionSummary[] {
    return this.#sessions.get();
  }

  set sessions(value: SessionSummary[]) {
    this.#sessions.set(value);
  }

  /** Whether the initial / latest session refresh is in flight. */
  get sessionsLoading(): boolean {
    return this.#sessionsLoading.get();
  }

  set sessionsLoading(value: boolean) {
    this.#sessionsLoading.set(value);
  }

  /** Last session refresh error (null when the latest refresh succeeded). */
  get sessionsError(): string | null {
    return this.#sessionsError.get();
  }

  set sessionsError(value: string | null) {
    this.#sessionsError.set(value);
  }

  get nodes(): NodeSummary[] {
    return this.#nodes.get();
  }

  set nodes(value: NodeSummary[]) {
    this.#nodes.set(value);
  }

  get nodesLoading(): boolean {
    return this.#nodesLoading.get();
  }

  set nodesLoading(value: boolean) {
    this.#nodesLoading.set(value);
  }

  get nodesError(): string | null {
    return this.#nodesError.get();
  }

  set nodesError(value: string | null) {
    this.#nodesError.set(value);
  }

  get projects(): ProjectSummary[] {
    return this.#projects.get();
  }

  set projects(value: ProjectSummary[]) {
    this.#projects.set(value);
  }

  get projectsLoading(): boolean {
    return this.#projectsLoading.get();
  }

  set projectsLoading(value: boolean) {
    this.#projectsLoading.set(value);
  }

  get projectsError(): string | null {
    return this.#projectsError.get();
  }

  set projectsError(value: string | null) {
    this.#projectsError.set(value);
  }

  /** Client WS connection state. */
  get wsConnected(): boolean {
    return this.#wsConnected.get();
  }

  set wsConnected(value: boolean) {
    this.#wsConnected.set(value);
  }

  /** Node ⇔ hub link state (only when connected to a local node). */
  get hubLink(): HubLinkStatus | null {
    return this.#hubLink.get();
  }

  set hubLink(value: HubLinkStatus | null) {
    this.#hubLink.set(value);
  }

  /** Number of unsynced Outbox events. */
  get unsyncedEventCount(): number {
    return this.#unsyncedEventCount.get();
  }

  set unsyncedEventCount(value: number) {
    this.#unsyncedEventCount.set(value);
  }

  /** Last sync time (epoch ms). */
  get lastSyncedAt(): number | null {
    return this.#lastSyncedAt.get();
  }

  set lastSyncedAt(value: number | null) {
    this.#lastSyncedAt.set(value);
  }

  /** Currently open session (WS priority delivery hint). */
  get activeSessionId(): string | null {
    return this.#activeSessionId.get();
  }

  set activeSessionId(value: string | null) {
    this.#activeSessionId.set(value);
  }

  // ------------------------------------------------------------------
  // Lifecycle
  // ------------------------------------------------------------------

  /** Start the Client WS (call after authentication is established). */
  start(): void {
    if (!this.#stopped) return;
    this.#stopped = false;
    this.#connect();
    void this.refreshAll();
  }

  /** Disconnect and stop reconnecting (e.g. on logout). */
  stop(): void {
    this.#stopped = true;
    if (this.#reconnectTimer !== null) {
      clearTimeout(this.#reconnectTimer);
      this.#reconnectTimer = null;
    }
    this.#ws?.close();
    this.#ws = null;
    this.wsConnected = false;
  }

  /** Stop once and reconnect (e.g. right after login). */
  restart(): void {
    this.stop();
    this.start();
  }

  #connect(): void {
    if (this.#stopped || this.connection.tokenRequired) return;
    let ws: WebSocket;
    try {
      ws = new WebSocket(this.connection.client.wsUrl("/api/v1/client/ws"));
    } catch {
      this.#scheduleReconnect();
      return;
    }
    this.#ws = ws;
    ws.addEventListener("open", () => {
      this.wsConnected = true;
      this.#reconnectDelayMs = 1_000;
      // A client with no stored cursor starts at the store tail: chat history
      // is loaded per session from the paginated events API, so replaying the
      // whole log would defeat paging and grow memory. A stored cursor is kept
      // so live events and unread state missed while the tab was closed are
      // replayed, and reconnects resume from the in-memory cursor.
      let since = this.#cursor;
      if (!this.#hasConnected && since === null) {
        const latest = this.connection.systemInfo?.latest_event_cursor;
        if (latest !== undefined) {
          since = latest;
          this.#cursor = latest;
          writeCursor(latest);
        }
      }
      this.#hasConnected = true;
      this.#send({
        op: "subscribe",
        since_cursor: since,
        focused_session_id: this.activeSessionId,
      });
      void this.refreshAll();
    });
    ws.addEventListener("message", (event: MessageEvent) => {
      if (typeof event.data === "string") this.#handleMessage(event.data);
    });
    ws.addEventListener("close", () => {
      // `restart()` / `stop()` may already have replaced or cleared this socket.
      // A stale close must not mark the new connection disconnected or schedule
      // another reconnect (which would leak a second WebSocket).
      if (this.#ws !== ws) return;
      this.#ws = null;
      this.wsConnected = false;
      this.#scheduleReconnect();
    });
  }

  #scheduleReconnect(): void {
    if (this.#stopped || this.#reconnectTimer !== null || this.connection.tokenRequired) return;
    const delay = this.#reconnectDelayMs;
    this.#reconnectDelayMs = Math.min(delay * 2, MAX_RECONNECT_DELAY_MS);
    this.#reconnectTimer = setTimeout(() => {
      this.#reconnectTimer = null;
      this.#connect();
    }, delay);
  }

  #send(message: ClientWsMessage): boolean {
    if (!this.#ws || this.#ws.readyState !== WebSocket.OPEN) return false;
    this.#ws.send(JSON.stringify(message));
    return true;
  }

  // ------------------------------------------------------------------
  // Fetch (REST)
  // ------------------------------------------------------------------

  async refreshAll(): Promise<void> {
    await Promise.all([this.refreshSessions(), this.refreshNodes(), this.refreshProjects()]);
  }

  async refreshSessions(): Promise<void> {
    this.sessionsLoading = true;
    try {
      // Always refresh the newest page. Pages the reader loaded explicitly are
      // kept as-is below: sessions that change move to the top (their
      // `updated_at` bumps), so merging keeps their projections current without
      // refetching the whole loaded depth on every status change.
      const page = await this.connection.client.sessions({
        includeArchived: true,
        limit: SESSION_PAGE_SIZE,
      });
      const firstPageIds = new Set(page.sessions.map((session) => session.session_id));
      const older = this.sessions.filter((session) => !firstPageIds.has(session.session_id));
      const next = sortSessions([...page.sessions, ...older]);
      this.sessions = next;
      // Only initialize a project's cursor. A project that already has a cursor
      // keeps the deepest loaded page: rolling it back to the first page would
      // make "Load older" re-fetch pages the reader already loaded.
      for (const cursor of page.project_cursors ?? []) {
        if (!this.#projectCursors.has(cursor.project_id)) {
          this.#projectCursors.set(cursor.project_id, cursor.next_cursor ?? null);
        }
      }
      if (!this.#sessionsInitialized) {
        // Mark sessions that this device has never opened as read, so a first
        // visit does not flag the entire history as unread.
        let changed = false;
        for (const session of next) {
          if (this.#readSeqs.has(session.session_id)) continue;
          this.#readSeqs.set(session.session_id, session.last_node_seq);
          changed = true;
        }
        this.#sessionsInitialized = true;
        if (changed) this.#queueReadStatePersist();
      }
      this.sessionsError = null;
    } catch (error) {
      this.sessionsError = errorMessage(error);
      this.#warn("sessions", error);
    } finally {
      this.sessionsLoading = false;
    }
  }

  /** Whether the project has an older page (unknown projects are treated as having more). */
  hasMoreSessions(projectId: string): boolean {
    const cursor = this.#projectCursors.get(projectId);
    return cursor === undefined ? true : cursor !== null;
  }

  /** Whether the project's session list has been loaded at least once. */
  hasLoadedProjectSessions(projectId: string): boolean {
    return this.#projectCursors.has(projectId);
  }

  /** Whether a per-project "load older" request is in flight. */
  sessionsLoadingMore(projectId: string): boolean {
    return this.#projectLoading.get(projectId) ?? false;
  }

  /**
   * Append the next page of one project's sessions (older entries).
   *
   * A project without a cursor loads its first page. `refreshSessions` keeps
   * the loaded depth, so this only fetches when the reader asks for it.
   */
  async loadMoreSessions(projectId: string): Promise<void> {
    if (this.#projectLoading.get(projectId)) return;
    const cursor = this.#projectCursors.get(projectId);
    if (cursor === null) return;
    this.#projectLoading.set(projectId, true);
    try {
      const page = await this.connection.client.sessions({
        includeArchived: true,
        limit: SESSION_PAGE_SIZE,
        projectId,
        cursor: cursor ?? null,
      });
      const known = new Set(this.sessions.map((session) => session.session_id));
      this.sessions = sortSessions([
        ...this.sessions,
        ...page.sessions.filter((session) => !known.has(session.session_id)),
      ]);
      this.#projectCursors.set(projectId, page.next_cursor ?? null);
      this.sessionsError = null;
    } catch (error) {
      this.sessionsError = errorMessage(error);
      this.#warn("sessions", error);
    } finally {
      this.#projectLoading.set(projectId, false);
    }
  }

  async refreshNodes(): Promise<void> {
    this.nodesLoading = true;
    try {
      this.nodes = await this.connection.client.nodes();
      this.nodesError = null;
    } catch (error) {
      this.nodesError = errorMessage(error);
      this.#warn("nodes", error);
    } finally {
      this.nodesLoading = false;
    }
  }

  async refreshProjects(): Promise<void> {
    this.projectsLoading = true;
    try {
      this.projects = await this.connection.client.projects();
      this.projectsError = null;
    } catch (error) {
      this.projectsError = errorMessage(error);
      this.#warn("projects", error);
    } finally {
      this.projectsLoading = false;
    }
  }

  #queueSessionsRefresh(): void {
    if (this.#sessionsRefreshQueued) return;
    this.#sessionsRefreshQueued = true;
    queueMicrotask(() => {
      this.#sessionsRefreshQueued = false;
      void this.refreshSessions();
    });
  }

  #queueProjectsRefresh(): void {
    if (this.#projectsRefreshQueued) return;
    this.#projectsRefreshQueued = true;
    queueMicrotask(() => {
      this.#projectsRefreshQueued = false;
      void this.refreshProjects();
    });
  }

  #warn(label: string, error: unknown): void {
    // Background refresh failures are persistent (the banner keeps them visible
    // instead of a toast that disappears).
    problems.report(label, error, "warning");
  }

  // ------------------------------------------------------------------
  // Timeline
  // ------------------------------------------------------------------

  /** A session's timeline (created if it does not exist). */
  timelineFor(sessionId: string): SessionTimeline {
    let timeline = this.#timelines.get(sessionId);
    if (!timeline) {
      timeline = new SessionTimeline(sessionId);
      this.#timelines.set(sessionId, timeline);
    }
    return timeline;
  }

  /** Ephemeral provisioning logs (most recent first, at most 500 lines). */
  provisioningLinesFor(sessionId: string): string[] {
    return this.provisioningLogs.get(sessionId) ?? [];
  }

  #appendProvisioningLog(sessionId: string, line: string): void {
    const current = this.provisioningLogs.get(sessionId) ?? [];
    const next = [...current, line];
    if (next.length > 500) next.splice(0, next.length - 500);
    this.provisioningLogs.set(sessionId, next);
  }

  /**
   * Whether a session has unread completed agent messages.
   *
   * Read state is per device (`localStorage`); it is not synced to the node or
   * hub, so another browser may still show the session as unread.
   */
  hasUnread(sessionId: string): boolean {
    return this.unreadSessions.has(sessionId);
  }

  /** Mark a session as read up to its latest known event. */
  markSessionRead(sessionId: string): void {
    const latest = this.#latestSessionSeq(sessionId);
    const previous = this.#readSeqs.get(sessionId) ?? 0;
    if (latest > previous) this.#readSeqs.set(sessionId, latest);
    const removed = this.unreadSessions.delete(sessionId);
    if (latest > previous || removed) this.#queueReadStatePersist();
  }

  /** Highest `node_seq` known for a session (summary or local timeline). */
  #latestSessionSeq(sessionId: string): number {
    const summary = this.sessions.find((session) => session.session_id === sessionId);
    const events = this.#timelines.get(sessionId)?.events;
    const lastEventSeq =
      events !== undefined && events.length > 0 ? events[events.length - 1].node_seq : 0;
    return Math.max(summary?.last_node_seq ?? 0, lastEventSeq);
  }

  /**
   * Update read state from a newly applied persisted event.
   *
   * Only completed agent messages create unread state; other events simply
   * advance the read watermark while the session is open.
   */
  #trackReadState(event: SessionEventEnvelope): void {
    if (event.session_id === this.activeSessionId) {
      const previous = this.#readSeqs.get(event.session_id) ?? 0;
      if (event.node_seq > previous) {
        this.#readSeqs.set(event.session_id, event.node_seq);
        this.#queueReadStatePersist();
      }
      return;
    }
    if (event.payload.type !== "agent_message" || !event.payload.data.is_complete) return;
    const readSeq = this.#readSeqs.get(event.session_id);
    if (readSeq === undefined) {
      // Before the first session listing we cannot tell whether this is an old
      // replayed session or one created while the app was closed.
      if (!this.#sessionsInitialized) return;
    } else if (event.node_seq <= readSeq) {
      return;
    }
    this.#unmarkRead(event.session_id);
  }

  #unmarkRead(sessionId: string): void {
    if (this.unreadSessions.has(sessionId)) return;
    this.unreadSessions.add(sessionId);
    this.#queueReadStatePersist();
  }

  #forgetSessionReadState(sessionId: string): void {
    const removed = this.unreadSessions.delete(sessionId);
    const hadRead = this.#readSeqs.delete(sessionId);
    if (removed || hadRead) this.#queueReadStatePersist();
  }

  #queueReadStatePersist(): void {
    if (this.#readStatePersistQueued) return;
    this.#readStatePersistQueued = true;
    queueMicrotask(() => {
      this.#readStatePersistQueued = false;
      this.#persistReadState();
    });
  }

  #persistReadState(): void {
    if (typeof localStorage === "undefined") return;
    const read: Record<string, number> = {};
    for (const [sessionId, seq] of this.#readSeqs) read[sessionId] = seq;
    try {
      localStorage.setItem(
        READ_STATE_KEY,
        JSON.stringify({ read, unread: [...this.unreadSessions] }),
      );
    } catch {
      // Ignore quota / disabled storage: read state is a convenience only.
    }
  }

  /** Open a session (updates the WS priority delivery hint). */
  setActiveSession(sessionId: string | null): void {
    // This is called from the session page's lifecycle, whose cleanup resets
    // the active session. Tracking the reads below (the previous ID and the
    // read-state bookkeeping) would make that effect depend on the state it
    // writes, so the whole body is untracked.
    untrack(() => {
      const previous = this.activeSessionId;
      if (previous !== null && previous !== sessionId) this.markSessionRead(previous);
      this.activeSessionId = sessionId;
      if (sessionId !== null) this.markSessionRead(sessionId);
      this.#send({
        op: "subscribe",
        since_cursor: this.#cursor,
        focused_session_id: sessionId,
      });
    });
  }

  /**
   * Load the latest history page of a session (fallback before the WS connects
   * and whenever the timeline was pruned).
   *
   * The newest page is fetched with `before_cursor` paging. When the session is
   * longer than one page, the oldest page is also kept aside so session-level
   * metadata (capabilities, provisioning log, ...) stays available without
   * rendering the whole conversation. Older pages are loaded on demand with
   * [`SyncStore.loadOlderSessionEvents`].
   */
  async loadSessionEvents(sessionId: string): Promise<void> {
    const timeline = this.timelineFor(sessionId);
    if (timeline.historyLoading) return;
    if (timeline.historyLoaded && timeline.events.length > 0) return;
    timeline.historyLoading = true;
    timeline.resetWindow();
    try {
      const tail = await this.connection.client.sessionEventsBefore(
        sessionId,
        null,
        HISTORY_PAGE_SIZE,
      );
      for (const event of tail.events) {
        if (timeline.applyEvent(event)) this.#trackReadState(event);
      }
      const oldestSeq = tail.events[0]?.node_seq ?? 1;
      if (tail.events.length < HISTORY_PAGE_SIZE || oldestSeq <= 1) {
        // The tail reaches the session start; nothing older exists.
        timeline.olderCursor = null;
        timeline.hasOlder = false;
        timeline.historyLoaded = true;
        return;
      }
      const head = await this.connection.client.sessionEvents(sessionId, 0, HISTORY_PAGE_SIZE);
      for (const event of head.events) {
        timeline.applyHeadEvent(event);
        this.#trackReadState(event);
      }
      timeline.headEndSeq = head.events.at(-1)?.node_seq ?? 0;
      if (head.events.length < HISTORY_PAGE_SIZE || timeline.headEndSeq >= oldestSeq) {
        // Head and tail overlap (the session fits in two pages).
        timeline.mergeHeadIntoWindow();
        timeline.olderCursor = null;
        timeline.hasOlder = false;
      } else {
        timeline.olderCursor = tail.first_cursor;
        timeline.hasOlder = true;
      }
      timeline.historyLoaded = true;
    } catch (error) {
      this.#warn(`session events (${sessionId})`, error);
      throw error;
    } finally {
      timeline.historyLoading = false;
    }
  }

  /**
   * Prepend the next older page of a session's history.
   *
   * Returns whether anything was added (the caller uses it to restore the
   * scroll position).
   */
  async loadOlderSessionEvents(sessionId: string): Promise<boolean> {
    const timeline = this.#timelines.get(sessionId);
    if (
      timeline === undefined ||
      timeline.historyLoading ||
      !timeline.hasOlder ||
      timeline.olderCursor === null
    ) {
      return false;
    }
    timeline.historyLoading = true;
    try {
      const batch = await this.connection.client.sessionEventsBefore(
        sessionId,
        timeline.olderCursor,
        HISTORY_PAGE_SIZE,
      );
      let added = false;
      for (const event of batch.events) {
        if (timeline.applyEvent(event)) added = true;
        this.#trackReadState(event);
      }
      const oldestSeq = batch.events[0]?.node_seq;
      const reachedHead =
        timeline.headEndSeq > 0 && oldestSeq !== undefined && oldestSeq <= timeline.headEndSeq + 1;
      if (batch.events.length === 0 || batch.events.length < HISTORY_PAGE_SIZE || reachedHead) {
        timeline.mergeHeadIntoWindow();
        timeline.olderCursor = null;
        timeline.hasOlder = false;
      } else {
        timeline.olderCursor = batch.first_cursor;
        timeline.hasOlder = true;
      }
      return added;
    } catch (error) {
      this.#warn(`session history (${sessionId})`, error);
      throw error;
    } finally {
      timeline.historyLoading = false;
    }
  }

  // ------------------------------------------------------------------
  // Commands (Client WS + CommandResult correlation)
  // ------------------------------------------------------------------

  #dispatch(
    command: ClientWsMessage,
    sessionId: string,
    timeoutMs = COMMAND_TIMEOUT_MS,
  ): Promise<CommandResult> {
    const commandId = "command_id" in command ? command.command_id : "";
    return new Promise<CommandResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pendingCommands.delete(commandId);
        reject(new Error(m.lib_command_timeout()));
      }, timeoutMs);
      this.#pendingCommands.set(commandId, {
        sessionId,
        settle: resolve,
        reject,
        timer,
      });
      if (!this.#send(command)) {
        this.#pendingCommands.delete(commandId);
        clearTimeout(timer);
        reject(ApiError.fromCode(0, "NODE_OFFLINE", m.lib_not_connected_to_hub()));
      }
    });
  }

  /**
   * Send a prompt (shown as a Pending Queue entry).
   *
   * Sending to a stopped session is automatically resumed on the node, so the
   * response timeout is extended to [`SEND_PROMPT_TIMEOUT_MS`].
   */
  sendPrompt(sessionId: string, input: PromptInput): Promise<CommandResult> {
    const commandId = randomUuid();
    this.pendingPrompts.set(commandId, {
      commandId,
      sessionId,
      text: input.text,
      createdAt: Date.now(),
    });
    const promise = this.#dispatch(
      {
        op: "send_prompt",
        command_id: commandId,
        session_id: sessionId,
        input,
        client_source: "web",
      },
      sessionId,
      SEND_PROMPT_TIMEOUT_MS,
    );
    return promise.finally(() => {
      this.pendingPrompts.delete(commandId);
    });
  }

  /** Respond to a permission request over REST (idempotent). */
  respondPermission(
    sessionId: string,
    requestId: string,
    selectedOptionId: string,
    resolvedBy = "web",
  ): Promise<RespondPermissionResponse> {
    return this.connection.client.respondPermission(sessionId, requestId, {
      selected_option_id: selectedOptionId,
      // The selected option already carries allow_once / allow_always; `always`
      // would promote an allow_once answer, which the UI does not do implicitly.
      always: false,
      resolved_by: resolvedBy,
    });
  }

  /** Respond to an elicitation (question) over REST (idempotent). */
  respondElicitation(
    sessionId: string,
    elicitationId: string,
    action: ElicitationAction,
    content: Record<string, ElicitationValue> | null,
    resolvedBy = "web",
  ): Promise<RespondElicitationResponse> {
    return this.connection.client.respondElicitation(sessionId, elicitationId, {
      action,
      content,
      resolved_by: resolvedBy,
    });
  }

  /** Mode switch / config change / cancel / kill. */
  controlSession(sessionId: string, action: SessionControlAction): Promise<CommandResult> {
    return this.#dispatch(
      {
        op: "control_session",
        command_id: randomUuid(),
        session_id: sessionId,
        action,
      },
      sessionId,
    );
  }

  /** Emergency stop (REST API with an audit log). */
  async killSwitch(reason: string): Promise<number> {
    const response = await this.connection.client.killSwitch(reason);
    void this.refreshAll();
    return response.notified_nodes;
  }

  /**
   * Resume a stopped session.
   *
   * When native restoration of the agent-side context is unsupported, the
   * session continues via history replay and returns `context_restored = false`
   * (`StatusChanged(Idle)` is reflected automatically by event subscription).
   */
  async resumeSession(sessionId: string): Promise<ResumeSessionResponse> {
    const result = await this.connection.client.resumeSession(sessionId, {
      force_replay: false,
    });
    await this.refreshSessions();
    return result;
  }

  /** Rename a session; the node emits `SessionTitleChanged` for other clients. */
  async renameSession(sessionId: string, title: string): Promise<SessionRenameResponse> {
    const result = await this.connection.client.renameSession(sessionId, { title });
    await this.refreshSessions();
    return result;
  }

  /**
   * Archive / restore a session (reversible; the event log is retained).
   */
  async archiveSession(sessionId: string, archived: boolean): Promise<SessionArchiveResponse> {
    const result = await this.connection.client.archiveSession(sessionId, {
      archived,
    });
    await this.refreshSessions();
    return result;
  }

  /**
   * Delete a session (erases the conversation log irreversibly).
   *
   * The executing node stops a running session, appends `SessionDeleted`
   * (tombstone) and purges the event bodies. The local timeline is discarded
   * immediately as well.
   */
  async deleteSession(sessionId: string): Promise<void> {
    await this.connection.client.deleteSession(sessionId);
    this.#timelines.delete(sessionId);
    this.provisioningLogs.delete(sessionId);
    this.#forgetSessionReadState(sessionId);
    await this.refreshSessions();
  }

  #resolveCommand(result: CommandResult): void {
    const pending = this.#pendingCommands.get(result.command_id);
    if (!pending) return;
    this.#pendingCommands.delete(result.command_id);
    clearTimeout(pending.timer);
    pending.settle(result);
  }

  // ------------------------------------------------------------------
  // WS message handling
  // ------------------------------------------------------------------

  #handleMessage(raw: string): void {
    let message: ServerWsMessage;
    try {
      message = JSON.parse(raw) as ServerWsMessage;
    } catch (error) {
      console.error("[fxg] failed to parse client ws message", error, raw);
      return;
    }
    switch (message.op) {
      case "event_batch":
        this.#applyBatch(message.events, message.cursor);
        break;
      case "live_stream_delta":
        this.timelineFor(message.session_id).applyDelta(message.delta);
        break;
      case "provisioning_log":
        this.#appendProvisioningLog(message.session_id, message.line);
        break;
      case "command_result":
        this.#resolveCommand(message);
        break;
      case "system_status":
        this.hubLink = message.hub_link;
        this.unsyncedEventCount = message.unsynced_event_count;
        this.lastSyncedAt = message.last_synced_at;
        break;
      case "error":
        if (message.error.code === "UNAUTHORIZED") {
          // Token expired (e.g. server regenerated it) → go to the re-auth flow
          this.stop();
          void this.connection.probe();
        } else {
          problems.reportInfo("sync", message.error);
        }
        break;
      case "pong":
        break;
      case "projects_updated":
        this.#queueProjectsRefresh();
        break;
      default:
        console.warn("[fxg] unknown client ws message", message);
        break;
    }
  }

  /** Apply a projection change to a loaded session entry (older pages included). */
  #patchSession(sessionId: string, patch: Partial<SessionSummary>): void {
    this.sessions = this.sessions.map((session) =>
      session.session_id === sessionId ? { ...session, ...patch } : session,
    );
  }

  /**
   * Keep memory bounded for sessions the reader is not looking at.
   *
   * Their timeline is only used for unread tracking (which is seq-based), so
   * the oldest events can be dropped. Opening the session reloads its history
   * from the paginated events API.
   */
  #trimInactiveTimeline(sessionId: string, timeline: SessionTimeline): void {
    if (sessionId === this.activeSessionId) return;
    const overflow = timeline.events.length - INACTIVE_EVENT_LIMIT;
    if (overflow <= 0) return;
    timeline.events = timeline.events.slice(overflow);
    timeline.resetWindow();
  }

  #applyBatch(events: SessionEventEnvelope[], cursor: number): void {
    // After the source store is recreated or reset, the cursor rewinds (docs/03 §3.2).
    // Keeping the old timeline would discard the new store's events as duplicate
    // node_seq / event_id, so discard local state and resync.
    const update = mergeCursor(this.#cursor, cursor);
    if (update.reset) {
      console.warn("[fxg] sync store cursor was rewound; resetting local state", {
        previous: this.#cursor,
        cursor,
      });
      this.#timelines.clear();
      this.provisioningLogs.clear();
      void this.refreshAll();
    }

    let sessionsDirty = false;
    let projectsDirty = false;
    for (const event of events) {
      const timeline = this.timelineFor(event.session_id);
      if (timeline.applyEvent(event)) {
        this.#trackReadState(event);
        this.#trimInactiveTimeline(event.session_id, timeline);
      }
      switch (event.payload.type) {
        case "session_created":
          sessionsDirty = true;
          projectsDirty = true;
          break;
        case "session_title_changed":
        case "session_agent_bound":
        case "status_changed":
        case "capabilities_updated":
          sessionsDirty = true;
          break;
        case "session_archived":
          // Archiving does not bump `updated_at`, so an entry from an older page
          // would otherwise keep its stale visibility until it is refreshed.
          this.#patchSession(event.session_id, {
            archived_at: event.payload.data.archived ? event.created_at : null,
          });
          sessionsDirty = true;
          break;
        case "session_permission_policy_changed":
          this.#patchSession(event.session_id, {
            permission_policy: event.payload.data.policy,
          });
          sessionsDirty = true;
          break;
        case "session_deleted": {
          // Deleted sessions disappear from the list, and the
          // local timeline is discarded (only the tombstone event remains after)
          const deleted = this.sessions.find((session) => session.session_id === event.session_id);
          this.#timelines.delete(event.session_id);
          this.provisioningLogs.delete(event.session_id);
          this.#forgetSessionReadState(event.session_id);
          const remaining = this.sessions.filter(
            (session) => session.session_id !== event.session_id,
          );
          this.sessions = remaining;
          if (deleted && !remaining.some((session) => session.project_id === deleted.project_id)) {
            // No loaded session remains: re-fetch the project's first page
            // instead of continuing from a cursor that may point at a deleted row.
            this.#projectCursors.delete(deleted.project_id);
          }
          sessionsDirty = true;
          projectsDirty = true;
          break;
        }
        default:
          break;
      }
    }
    if (this.#cursor !== update.cursor) {
      this.#cursor = update.cursor;
      writeCursor(update.cursor);
    }
    if (sessionsDirty) this.#queueSessionsRefresh();
    if (projectsDirty) this.#queueProjectsRefresh();
  }
}
