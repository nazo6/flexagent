import { ApiError } from "$lib/api/errors";
import type { ClientWsMessage } from "$lib/generated/ClientWsMessage";
import type { CommandResult } from "$lib/generated/CommandResult";
import type { NodeSummary } from "$lib/generated/NodeSummary";
import type { PermissionRequestEntry } from "$lib/generated/PermissionRequestEntry";
import type { ProjectSummary } from "$lib/generated/ProjectSummary";
import type { ServerWsMessage } from "$lib/generated/ServerWsMessage";
import type { SessionControlAction } from "$lib/generated/SessionControlAction";
import type { SessionEventEnvelope } from "$lib/generated/SessionEventEnvelope";
import type { SessionSummary } from "$lib/generated/SessionSummary";
import { SessionTimeline } from "$lib/sync/timeline.svelte";
import type { PendingPrompt } from "$lib/sync/reducer";
import type { ConnectionStore } from "./connection.svelte";

const CURSOR_KEY = "fxg.sync.cursor";
/** `CommandResult` を待つタイムアウト。 */
const COMMAND_TIMEOUT_MS = 30_000;
/** 再接続バックオフの上限。 */
const MAX_RECONNECT_DELAY_MS = 30_000;

interface PendingCommand {
  sessionId: string | null;
  settle(result: CommandResult): void;
  reject(error: Error): void;
  timer: ReturnType<typeof setTimeout>;
}

function readCursor(): number | null {
  if (typeof localStorage === "undefined") return null;
  const raw = localStorage.getItem(CURSOR_KEY);
  if (raw === null) return null;
  const value = Number.parseInt(raw, 10);
  return Number.isFinite(value) && value >= 0 ? value : null;
}

/**
 * Client WS (`/api/v1/client/ws`) による差分同期ストア。
 *
 * - 接続時に `Subscribe { since_cursor, focused_session_id }` を送信し、
 *   未取得イベントのリプレイとその後はリアルタイムイベントを受信する。
 * - 最後に受信したバッチの `cursor` を保存し、再接続時に差分再開する
 *   (接続先ストアごとに 1 カーソル。接続先の切替はオリジン遷移のため、
 *   localStorage はオリジン単位で自然に分離される)。
 * - セッション投影 (`sessions`)、承認 Inbox、ノード一覧はイベント適用時に
 *   該当 REST API を再取得して同期する (ハブ側投影と整合させる)。
 */
export class SyncStore {
  readonly connection: ConnectionStore;

  sessions = $state<SessionSummary[]>([]);
  nodes = $state<NodeSummary[]>([]);
  projects = $state<ProjectSummary[]>([]);
  inbox = $state<PermissionRequestEntry[]>([]);
  /** Client WS 接続状態。 */
  wsConnected = $state(false);
  /** 中央サーバーへの接続状態 (ローカルノード接続時のみ)。 */
  centralConnected = $state<boolean | null>(null);
  /** 未同期 Outbox イベント件数。 */
  unsyncedEventCount = $state(0);
  /** 最終同期日時 (epoch ms)。 */
  lastSyncedAt = $state<number | null>(null);
  /** 現在開いているセッション (WS の優先配信ヒント)。 */
  activeSessionId = $state<string | null>(null);
  /** 送信済みで `CommandResult` 未着のプロンプト (Pending Queue)。 */
  pendingPrompts = $state(new Map<string, PendingPrompt>());

  #timelines = $state(new Map<string, SessionTimeline>());
  #ws: WebSocket | null = null;
  #stopped = true;
  #reconnectDelayMs = 1_000;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #cursor: number | null = readCursor();
  #pendingCommands = new Map<string, PendingCommand>();
  #sessionsRefreshQueued = false;
  #inboxRefreshQueued = false;

  constructor(connection: ConnectionStore) {
    this.connection = connection;
  }

  // ------------------------------------------------------------------
  // ライフサイクル
  // ------------------------------------------------------------------

  /** Client WS を開始する (認証確立後に呼ぶ)。 */
  start(): void {
    if (!this.#stopped) return;
    this.#stopped = false;
    this.#connect();
    void this.refreshAll();
  }

  /** 切断して再接続を止める (ログアウト時など)。 */
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

  /** 一度停止して再接続する (ログイン直後など)。 */
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
  // 取得 (REST)
  // ------------------------------------------------------------------

  async refreshAll(): Promise<void> {
    await Promise.all([
      this.refreshSessions(),
      this.refreshNodes(),
      this.refreshProjects(),
      this.refreshInbox(),
    ]);
  }

  async refreshSessions(): Promise<void> {
    try {
      this.sessions = await this.connection.client.sessions({ limit: 200 });
    } catch (error) {
      this.#warn("sessions", error);
    }
  }

  async refreshNodes(): Promise<void> {
    try {
      this.nodes = await this.connection.client.nodes();
    } catch (error) {
      this.#warn("nodes", error);
    }
  }

  async refreshProjects(): Promise<void> {
    try {
      this.projects = await this.connection.client.projects();
    } catch (error) {
      this.#warn("projects", error);
    }
  }

  async refreshInbox(): Promise<void> {
    try {
      this.inbox = await this.connection.client.inbox();
    } catch (error) {
      this.#warn("inbox", error);
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

  #queueInboxRefresh(): void {
    if (this.#inboxRefreshQueued) return;
    this.#inboxRefreshQueued = true;
    queueMicrotask(() => {
      this.#inboxRefreshQueued = false;
      void this.refreshInbox();
    });
  }

  #warn(label: string, error: unknown): void {
    console.warn(`[fxg] failed to refresh ${label}`, error);
  }

  // ------------------------------------------------------------------
  // タイムライン
  // ------------------------------------------------------------------

  /** セッションのタイムライン (存在しなければ作成する)。 */
  timelineFor(sessionId: string): SessionTimeline {
    let timeline = this.#timelines.get(sessionId);
    if (!timeline) {
      timeline = new SessionTimeline(sessionId);
      this.#timelines.set(sessionId, timeline);
    }
    return timeline;
  }

  /** セッションを開く (WS の優先配信ヒントを更新する)。 */
  setActiveSession(sessionId: string | null): void {
    this.activeSessionId = sessionId;
    this.#send({
      op: "subscribe",
      since_cursor: this.#cursor,
      focused_session_id: sessionId,
    });
  }

  /**
   * REST 経由でセッション履歴を読み込む (WS 接続前のフォールバック)。
   *
   * 既知イベントは `node_seq` 重複として無害にスキップされる。
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
  // コマンド (Client WS + CommandResult 相関)
  // ------------------------------------------------------------------

  #dispatch(command: ClientWsMessage, sessionId: string): Promise<CommandResult> {
    const commandId = "command_id" in command ? command.command_id : "";
    return new Promise<CommandResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pendingCommands.delete(commandId);
        reject(new Error("コマンドがタイムアウトしました (ノードからの応答がありません)"));
      }, COMMAND_TIMEOUT_MS);
      this.#pendingCommands.set(commandId, {
        sessionId,
        settle: resolve,
        reject,
        timer,
      });
      if (!this.#send(command)) {
        this.#pendingCommands.delete(commandId);
        clearTimeout(timer);
        reject(new ApiError(0, "NODE_OFFLINE", "サーバーに接続されていません"));
      }
    });
  }

  /** プロンプトを送信する (Pending Queue として表示される)。 */
  sendPrompt(sessionId: string, text: string): Promise<CommandResult> {
    const commandId = crypto.randomUUID();
    this.pendingPrompts.set(commandId, {
      commandId,
      sessionId,
      text,
      createdAt: Date.now(),
    });
    const promise = this.#dispatch(
      {
        op: "send_prompt",
        command_id: commandId,
        session_id: sessionId,
        text,
        client_source: "web",
      },
      sessionId,
    );
    return promise.finally(() => {
      this.pendingPrompts.delete(commandId);
    });
  }

  /** 承認リクエストへ応答する (WS 経由。冪等)。 */
  respondPermission(
    sessionId: string,
    requestId: string,
    selectedOptionId: string,
    resolvedBy = "web",
  ): Promise<CommandResult> {
    return this.#dispatch(
      {
        op: "respond_permission",
        command_id: crypto.randomUUID(),
        session_id: sessionId,
        request_id: requestId,
        selected_option_id: selectedOptionId,
        resolved_by: resolvedBy,
      },
      sessionId,
    );
  }

  /** モード切替 / 設定変更 / キャンセル / Kill。 */
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

  /** 緊急停止 (監査ログ付きの REST API)。 */
  async killSwitch(reason: string): Promise<number> {
    const response = await this.connection.client.killSwitch(reason);
    void this.refreshAll();
    return response.notified_nodes;
  }

  #resolveCommand(result: CommandResult): void {
    const pending = this.#pendingCommands.get(result.command_id);
    if (!pending) return;
    this.#pendingCommands.delete(result.command_id);
    clearTimeout(pending.timer);
    pending.settle(result);
  }

  // ------------------------------------------------------------------
  // WS メッセージ処理
  // ------------------------------------------------------------------

  #handleMessage(raw: string): void {
    let message: ServerWsMessage;
    try {
      message = JSON.parse(raw) as ServerWsMessage;
    } catch {
      return;
    }
    switch (message.op) {
      case "event_batch":
        this.#applyBatch(message.events, message.cursor);
        break;
      case "live_stream_delta":
        this.timelineFor(message.session_id).applyDelta(message.delta);
        break;
      case "command_result":
        this.#resolveCommand(message);
        break;
      case "system_status":
        this.centralConnected = message.central_connected;
        this.unsyncedEventCount = message.unsynced_event_count;
        this.lastSyncedAt = message.last_synced_at;
        break;
      case "error":
        if (message.code === "UNAUTHORIZED") {
          // トークン失効 (サーバー再生成など) → 再認証フローへ
          this.stop();
          void this.connection.probe();
        } else {
          console.warn("[fxg] ws error", message.code, message.message);
        }
        break;
      case "pong":
        break;
    }
  }

  #applyBatch(events: SessionEventEnvelope[], cursor: number): void {
    let sessionsDirty = false;
    let inboxDirty = false;
    for (const event of events) {
      const timeline = this.timelineFor(event.session_id);
      if (!timeline.applyEvent(event)) continue;
      switch (event.payload.type) {
        case "session_created":
        case "session_title_changed":
        case "session_agent_bound":
        case "status_changed":
        case "capabilities_updated":
          sessionsDirty = true;
          break;
        case "permission_request":
        case "permission_resolved":
          inboxDirty = true;
          break;
        default:
          break;
      }
    }
    if (cursor > (this.#cursor ?? 0)) {
      this.#cursor = cursor;
      if (typeof localStorage !== "undefined") {
        localStorage.setItem(CURSOR_KEY, String(cursor));
      }
    }
    if (sessionsDirty) this.#queueSessionsRefresh();
    if (inboxDirty) this.#queueInboxRefresh();
  }
}
