import type { AuditLogEntry } from "$lib/generated/AuditLogEntry";
import type { AuditLogsResponse } from "$lib/generated/AuditLogsResponse";
import type { CreateSessionRequest } from "$lib/generated/CreateSessionRequest";
import type { CreateSessionResponse } from "$lib/generated/CreateSessionResponse";
import type { CreateWorktreeRequest } from "$lib/generated/CreateWorktreeRequest";
import type { DiffScope } from "$lib/generated/DiffScope";
import type { ErrorCode } from "$lib/generated/ErrorCode";
import type { FsBrowseResponse } from "$lib/generated/FsBrowseResponse";
import type { InboxResponse } from "$lib/generated/InboxResponse";
import type { KillSwitchResponse } from "$lib/generated/KillSwitchResponse";
import type { NodeSummary } from "$lib/generated/NodeSummary";
import type { NodesResponse } from "$lib/generated/NodesResponse";
import type { PermissionRequestEntry } from "$lib/generated/PermissionRequestEntry";
import type { ProjectSummary } from "$lib/generated/ProjectSummary";
import type { ProjectsResponse } from "$lib/generated/ProjectsResponse";
import type { ProvisionerSummary } from "$lib/generated/ProvisionerSummary";
import type { ProvisionerTestResponse } from "$lib/generated/ProvisionerTestResponse";
import type { ProvisionersResponse } from "$lib/generated/ProvisionersResponse";
import type { PushSubscribeRequest } from "$lib/generated/PushSubscribeRequest";
import type { PushSubscribeResponse } from "$lib/generated/PushSubscribeResponse";
import type { RemoveWorktreeRequest } from "$lib/generated/RemoveWorktreeRequest";
import type { RespondPermissionRequest } from "$lib/generated/RespondPermissionRequest";
import type { RespondPermissionResponse } from "$lib/generated/RespondPermissionResponse";
import type { SearchHit } from "$lib/generated/SearchHit";
import type { SearchResponse } from "$lib/generated/SearchResponse";
import type { SessionEventBatch } from "$lib/generated/SessionEventBatch";
import type { SessionSummary } from "$lib/generated/SessionSummary";
import type { SystemInfoResponse } from "$lib/generated/SystemInfoResponse";
import type { WorkspaceDiffResponse } from "$lib/generated/WorkspaceDiffResponse";
import type { WorktreeInfo } from "$lib/generated/WorktreeInfo";
import type { WorktreesResponse } from "$lib/generated/WorktreesResponse";
import { ApiError } from "./errors";

/** クエリパラメータ (undefined / null は省略される)。 */
export type QueryParams = Record<string, string | number | boolean | null | undefined>;

export interface ApiClientOptions {
  /** ベース URL (`''` = 現在のオリジン。末尾スラッシュなし)。 */
  baseUrl?: string;
  /** Bearer トークン (Cookie 認証のみの場合は null)。 */
  token?: string | null;
}

interface RequestOptions {
  query?: QueryParams;
  /** JSON body (指定時は `Content-Type: application/json` を付与)。 */
  body?: unknown;
}

/**
 * `fxg server` / `fxg daemon` 共通の Client REST API クライアント。
 *
 * 認証は Bearer ヘッダを優先しつつ、同一オリジンでは `fxg_session` Cookie も
 * 自動送信される (`credentials: 'same-origin'`)。接続先の切替はオリジン
 * 単位のナビゲーションで行うため、クロスオリジンのリクエストは発生しない。
 */
export class ApiClient {
  readonly baseUrl: string;
  #token: string | null;

  constructor(options: ApiClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? "").replace(/\/+$/, "");
    this.#token = options.token ?? null;
  }

  setToken(token: string | null): void {
    this.#token = token;
  }

  /** REST API の絶対 URL を組み立てる。 */
  httpUrl(path: string, query?: QueryParams): string {
    const usp = new URLSearchParams();
    for (const [key, value] of Object.entries(query ?? {})) {
      if (value === undefined || value === null || value === "") continue;
      usp.set(key, String(value));
    }
    const suffix = usp.size > 0 ? `?${usp.toString()}` : "";
    return `${this.baseUrl}${path}${suffix}`;
  }

  /** WebSocket の絶対 URL を組み立てる (`http` → `ws`)。 */
  wsUrl(path: string): string {
    const base = this.baseUrl || (typeof location !== "undefined" ? location.origin : "");
    const url = new URL(path, base || "http://localhost");
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    return url.toString();
  }

  async #request<T>(method: string, path: string, options: RequestOptions = {}): Promise<T> {
    const headers: Record<string, string> = { Accept: "application/json" };
    if (options.body !== undefined) headers["Content-Type"] = "application/json";
    if (this.#token) headers.Authorization = `Bearer ${this.#token}`;

    const response = await fetch(this.httpUrl(path, options.query), {
      method,
      headers,
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
      credentials: "same-origin",
      cache: "no-store",
    });

    if (!response.ok) {
      const fallback = `${method} ${path} failed: ${response.status} ${response.statusText}`;
      let body: unknown = null;
      try {
        body = await response.json();
      } catch {
        /* 非 JSON エラーは fallback メッセージを使う */
      }
      throw ApiError.fromBody(response.status, body, fallback);
    }

    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  }

  // ------------------------------------------------------------------
  // 認証 / システム
  // ------------------------------------------------------------------

  /** `GET /api/v1/system/info` */
  systemInfo(): Promise<SystemInfoResponse> {
    return this.#request("GET", "/api/v1/system/info");
  }

  /** `POST /api/v1/auth/login` (成功時に `fxg_session` Cookie が発行される)。 */
  async authLogin(token: string): Promise<void> {
    const previous = this.#token;
    this.#token = token;
    try {
      await this.#request<undefined>("POST", "/api/v1/auth/login", { body: { token } });
    } catch (error) {
      this.#token = previous;
      throw error;
    }
  }

  /** `POST /api/v1/auth/logout` (`fxg_session` Cookie を失効させる)。 */
  authLogout(): Promise<void> {
    return this.#request("POST", "/api/v1/auth/logout", { body: {} });
  }

  /** `POST /api/v1/system/kill-switch` (緊急停止)。 */
  killSwitch(reason: string): Promise<KillSwitchResponse> {
    return this.#request("POST", "/api/v1/system/kill-switch", { body: { reason } });
  }

  // ------------------------------------------------------------------
  // プロジェクト / Worktree / ノード
  // ------------------------------------------------------------------

  /** `GET /api/v1/projects` */
  async projects(): Promise<ProjectSummary[]> {
    const response = await this.#request<ProjectsResponse>("GET", "/api/v1/projects");
    return response.projects;
  }

  /** `GET /api/v1/projects/:id/worktrees` */
  async worktrees(projectId: string): Promise<WorktreeInfo[]> {
    const response = await this.#request<WorktreesResponse>(
      "GET",
      `/api/v1/projects/${encodeURIComponent(projectId)}/worktrees`,
    );
    return response.worktrees;
  }

  /** `POST /api/v1/projects/:id/worktrees` */
  createWorktree(projectId: string, request: CreateWorktreeRequest): Promise<WorktreeInfo> {
    return this.#request("POST", `/api/v1/projects/${encodeURIComponent(projectId)}/worktrees`, {
      body: request,
    });
  }

  /** `DELETE /api/v1/projects/:id/worktrees` */
  removeWorktree(projectId: string, request: RemoveWorktreeRequest): Promise<void> {
    return this.#request("DELETE", `/api/v1/projects/${encodeURIComponent(projectId)}/worktrees`, {
      body: request,
    });
  }

  /** `GET /api/v1/nodes` */
  async nodes(): Promise<NodeSummary[]> {
    const response = await this.#request<NodesResponse>("GET", "/api/v1/nodes");
    return response.nodes;
  }

  /** `GET /api/v1/nodes/:id/fs/browse` */
  browseFs(nodeId: string, path?: string): Promise<FsBrowseResponse> {
    return this.#request<FsBrowseResponse>(
      "GET",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/fs/browse`,
      { query: { path } },
    );
  }

  /** `GET /api/v1/provisioners` */
  async provisioners(): Promise<ProvisionerSummary[]> {
    const response = await this.#request<ProvisionersResponse>("GET", "/api/v1/provisioners");
    return response.provisioners;
  }

  /** `POST /api/v1/provisioners/:name/test` (疎通検証)。 */
  testProvisioner(name: string): Promise<ProvisionerTestResponse> {
    return this.#request<ProvisionerTestResponse>(
      "POST",
      `/api/v1/provisioners/${encodeURIComponent(name)}/test`,
    );
  }

  // ------------------------------------------------------------------
  // セッション / 承認 / 検索 / 監査
  // ------------------------------------------------------------------

  /** `GET /api/v1/sessions` */
  async sessions(
    filter: {
      projectId?: string | null;
      nodeId?: string | null;
      status?: string | null;
      limit?: number;
    } = {},
  ): Promise<SessionSummary[]> {
    const response = await this.#request<{ sessions: SessionSummary[] }>(
      "GET",
      "/api/v1/sessions",
      {
        query: {
          project_id: filter.projectId,
          node_id: filter.nodeId,
          status: filter.status,
          limit: filter.limit,
        },
      },
    );
    return response.sessions;
  }

  /** `POST /api/v1/sessions` (新規セッション開始)。 */
  createSession(request: CreateSessionRequest): Promise<CreateSessionResponse> {
    return this.#request("POST", "/api/v1/sessions", { body: request });
  }

  /** `GET /api/v1/sessions/:id/events?after_cursor=` */
  sessionEvents(sessionId: string, afterCursor = 0, limit?: number): Promise<SessionEventBatch> {
    return this.#request("GET", `/api/v1/sessions/${encodeURIComponent(sessionId)}/events`, {
      query: { after_cursor: afterCursor, limit },
    });
  }

  /** `GET /api/v1/sessions/:id/diff` */
  sessionDiff(
    sessionId: string,
    scope: DiffScope = "uncommitted",
    base?: string | null,
  ): Promise<WorkspaceDiffResponse> {
    return this.#request("GET", `/api/v1/sessions/${encodeURIComponent(sessionId)}/diff`, {
      query: { scope, base },
    });
  }

  /** `GET /api/v1/inbox` */
  async inbox(): Promise<PermissionRequestEntry[]> {
    const response = await this.#request<InboxResponse>("GET", "/api/v1/inbox");
    return response.requests;
  }

  /** `POST /api/v1/sessions/:id/permissions/:req_id/respond` (冪等)。 */
  respondPermission(
    sessionId: string,
    requestId: string,
    request: RespondPermissionRequest,
  ): Promise<RespondPermissionResponse> {
    return this.#request(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/permissions/${encodeURIComponent(requestId)}/respond`,
      { body: request },
    );
  }

  /** `POST /api/v1/search` (FTS5 全文検索)。 */
  async search(query: string, limit?: number): Promise<SearchHit[]> {
    const response = await this.#request<SearchResponse>("POST", "/api/v1/search", {
      query: { q: query, limit },
    });
    return response.hits;
  }

  /** `GET /api/v1/audit/logs` */
  async auditLogs(limit = 100): Promise<AuditLogEntry[]> {
    const response = await this.#request<AuditLogsResponse>("GET", "/api/v1/audit/logs", {
      query: { limit },
    });
    return response.logs;
  }

  /** `POST /api/v1/push/subscribe` (Web Push 購読登録。中央サーバーのみ)。 */
  pushSubscribe(request: PushSubscribeRequest): Promise<PushSubscribeResponse> {
    return this.#request("POST", "/api/v1/push/subscribe", { body: request });
  }
}

/** `CommandResult` のエラーコードを UI 表示向けに整形する。 */
export function commandErrorMessage(code: ErrorCode | null, message: string | null): string {
  return message ?? code ?? "unknown error";
}
