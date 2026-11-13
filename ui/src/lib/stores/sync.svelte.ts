import { SvelteMap } from "svelte/reactivity";
import { ApiError } from "#lib/api/errors.ts";
import type {
  ClientWsMessage,
  CommandResult,
  ElicitationAction,
  ElicitationValue,
  NodeSummary,
  ProjectSummary,
  PromptInput,
  RespondElicitationResponse,
  RespondPermissionResponse,
  ResumeSessionResponse,
  HubLinkStatus,
  HubWsMessage,
  SessionArchiveResponse,
  SessionRenameResponse,
  SessionControlAction,
  SessionEventEnvelope,
  SessionSummary,
} from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { problems } from "#lib/stores/problems.svelte.ts";
import { mergeCursor } from "#lib/sync/cursor.ts";
import { SessionTimeline } from "#lib/sync/timeline.svelte.ts";
import type { PendingPrompt } from "#lib/sync/reducer.ts";
import type { ConnectionStore } from "./connection.svelte";

const CURSOR_KEY = "fxg.sync.cursor";
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

interface PendingCommand {
  sessionId: string | null;
  settle(result: CommandResult): void;
  reject(error: Error): void;
  timer: ReturnType<typeof setTimeout>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function readCursor(): number | null {
  if (typeof localStorage === "undefined") return null;
  const raw = localStorage.getItem(CURSOR_KEY);
  if (raw === null) return null;
  const value = Number.parseInt(raw, 10);
  return Number.isFinite(value) && value >= 0 ? value : null;
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

  sessions = $state<SessionSummary[]>([]);
  /** Whether the initial / latest session refresh is in flight. */
  sessionsLoading = $state(false);
  /** Last session refresh error (null when the latest refresh succeeded). */
  sessionsError = $state<string | null>(null);
  nodes = $state<NodeSummary[]>([]);
  nodesLoading = $state(false);
  nodesError = $state<string | null>(null);
  projects = $state<ProjectSummary[]>([]);
  projectsLoading = $state(false);
  projectsError = $state<string | null>(null);
  /** Client WS connection state. */
  wsConnected = $state(false);
  /** Node ⇔ hub link state (only when connected to a local node). */
  hubLink = $state<HubLinkStatus | null>(null);
  /** Number of unsynced Outbox events. */
  unsyncedEventCount = $state(0);
  /** Last sync time (epoch ms). */
  lastSyncedAt = $state<number | null>(null);
  /** Currently open session (WS priority delivery hint). */
  activeSessionId = $state<string | null>(null);
  /** Prompts sent with no `CommandResult` yet (Pending Queue). */
  pendingPrompts = new SvelteMap<string, PendingPrompt>();
  /**
   * Progress logs for ephemeral VM provisioning (ephemeral; not persisted).
   *
   * The server delivers `stderr` output from before the node's `node.db` exists
   * as `HubWsMessage::ProvisioningLog`. It is not restored on reconnect.
   *
   * These (and `pendingPrompts`) are [`SvelteMap`]: Svelte 5 does not proxy
   * `Map` inside `$state`, so mutating one would not re-render its consumers.
   */
  provisioningLogs = new SvelteMap<string, string[]>();

  /** Identity cache for the per-session timelines (not reactive state). */
  #timelines = new Map<string, SessionTimeline>();
  #ws: WebSocket | null = null;
  #stopped = true;
  #reconnectDelayMs = 1_000;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #cursor: number | null = readCursor();
  #pendingCommands = new Map<string, PendingCommand>();
  #sessionsRefreshQueued = false;

  constructor(connection: ConnectionStore) {
    this.connection = connection;
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
      this.#send({
        op: "subscribe",
        since_cursor: this.#cursor,
        focused_session_id: this.activeSessionId,
      });
      void this.refreshAll();
    });
    ws.addEventListener("message", (event: MessageEvent) => {
      if (typeof event.data === "string") this.#handleMessage(event.data);
    });
    ws.addEventListener("close", () => {
      if (this.#ws === ws) this.#ws = null;
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
      // Also fetch archived sessions so the sidebar can split out an "Archived" section
      this.sessions = await this.connection.client.sessions({
        includeArchived: true,
        limit: 200,
      });
      this.sessionsError = null;
    } catch (error) {
      this.sessionsError = errorMessage(error);
      this.#warn("sessions", error);
    } finally {
      this.sessionsLoading = false;
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

  /** Open a session (updates the WS priority delivery hint). */
  setActiveSession(sessionId: string | null): void {
    this.activeSessionId = sessionId;
    this.#send({
      op: "subscribe",
      since_cursor: this.#cursor,
      focused_session_id: sessionId,
    });
  }

  /**
   * Load session history over REST (fallback before the WS connects).
   *
   * Known events are harmlessly skipped as duplicate `node_seq` values.
   */
  async loadSessionEvents(sessionId: string): Promise<void> {
    const timeline = this.timelineFor(sessionId);
    try {
      const batch = await this.connection.client.sessionEvents(sessionId, 0, 5_000);
      for (const event of batch.events) timeline.applyEvent(event);
    } catch (error) {
      this.#warn(`session events (${sessionId})`, error);
      throw error;
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
    const commandId = crypto.randomUUID();
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
        command_id: crypto.randomUUID(),
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

  /**
   * Archive / restore a session (reversible; the event log is retained).
   */
  /** Rename a session; the node emits `SessionTitleChanged` for other clients. */
  async renameSession(sessionId: string, title: string): Promise<SessionRenameResponse> {
    const result = await this.connection.client.renameSession(sessionId, { title });
    await this.refreshSessions();
    return result;
  }

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
    let message: HubWsMessage;
    try {
      message = JSON.parse(raw) as HubWsMessage;
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
      default:
        console.warn("[fxg] unknown client ws message", message);
        break;
    }
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
    for (const event of events) {
      const timeline = this.timelineFor(event.session_id);
      if (!timeline.applyEvent(event)) continue;
      switch (event.payload.type) {
        case "session_created":
        case "session_title_changed":
        case "session_agent_bound":
        case "status_changed":
        case "capabilities_updated":
        case "session_archived":
        case "session_permission_policy_changed":
          sessionsDirty = true;
          break;
        case "session_deleted":
          // Deleted sessions disappear from the list, and the
          // local timeline is discarded (only the tombstone event remains after)
          this.#timelines.delete(event.session_id);
          this.provisioningLogs.delete(event.session_id);
          sessionsDirty = true;
          break;
        default:
          break;
      }
    }
    if (this.#cursor !== update.cursor) {
      this.#cursor = update.cursor;
      if (typeof localStorage !== "undefined") {
        localStorage.setItem(CURSOR_KEY, String(update.cursor));
      }
    }
    if (sessionsDirty) this.#queueSessionsRefresh();
  }
}
