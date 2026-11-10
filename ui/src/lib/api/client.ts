import type {
  AgentCapabilitiesPreview,
  AgentOpResponse,
  AgentSummary,
  AgentsResponse,
  AuditLogEntry,
  AuditLogsResponse,
  CreateSessionRequest,
  CreateSessionResponse,
  CreateWorktreeRequest,
  CustomAgentConfig,
  DiffScope,
  ErrorInfo,
  FsBrowseResponse,
  InboxResponse,
  InitialConfig,
  IssueNodeTokenRequest,
  IssueNodeTokenResponse,
  KillSwitchResponse,
  MetaResponse,
  NodeSettingsSnapshot,
  NodeSummary,
  NodeTokenSummary,
  NodeTokensResponse,
  NodesResponse,
  ProjectLinkRequest,
  ProjectLinkResponse,
  ProjectPreview,
  ProjectPreviewRequest,
  ProjectProfile,
  ProjectScanRequest,
  ProjectScanResponse,
  ProjectSummary,
  ProjectsResponse,
  ProvisionerSummary,
  ProvisionerTestResponse,
  ProvisionersResponse,
  PruneWorktreesRequest,
  PushSubscribeRequest,
  PushSubscribeResponse,
  RemoveWorktreeRequest,
  RespondElicitationRequest,
  RespondElicitationResponse,
  RespondPermissionRequest,
  RespondPermissionResponse,
  ResumeSessionRequest,
  ResumeSessionResponse,
  RotateAuthTokenResponse,
  DiagnosticsResponse,
  IncidentListResponse,
  SupportBundle,
  SearchHit,
  SearchResponse,
  SessionArchiveRequest,
  SessionArchiveResponse,
  SessionEventBatch,
  SecretsResponse,
  ServerSettingsSnapshot,
  SessionSummary,
  SystemInfoResponse,
  WorkspaceDiffResponse,
  UpdateAgentSettingsRequest,
  UpdateNodeSettingsRequest,
  UpdateProjectProfileRequest,
  UpdateServerSettingsRequest,
  UpsertSecretRequest,
  WorktreeInfo,
  WorktreesResponse,
} from "#lib/generated/bindings.ts";
import { errorCodeMessage } from "./error-messages";
import { ApiError } from "./errors";

/** Query parameters (undefined / null are omitted). */
export type QueryParams = Record<string, string | number | boolean | null | undefined>;

export interface ApiClientOptions {
  /** Base URL (`''` = current origin, no trailing slash). */
  baseUrl?: string;
  /** Bearer token (null when using Cookie auth only). */
  token?: string | null;
}

interface RequestOptions {
  query?: QueryParams;
  /** JSON body (adds `Content-Type: application/json` when set). */
  body?: unknown;
}

/**
 * Client REST API client shared by `fxg server` / `fxg daemon`.
 *
 * Authentication prefers the Bearer header, while same-origin requests also
 * send the `fxg_session` Cookie automatically (`credentials: 'same-origin'`).
 * Connection targets switch by origin-level navigation, so cross-origin
 * requests never occur.
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

  /** Build an absolute URL for a REST API path. */
  httpUrl(path: string, query?: QueryParams): string {
    const usp = new URLSearchParams();
    for (const [key, value] of Object.entries(query ?? {})) {
      if (value === undefined || value === null || value === "") continue;
      usp.set(key, String(value));
    }
    const suffix = usp.size > 0 ? `?${usp.toString()}` : "";
    return `${this.baseUrl}${path}${suffix}`;
  }

  /** Build an absolute WebSocket URL (`http` → `ws`). */
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
        /* Use the fallback message for non-JSON errors */
      }
      throw ApiError.fromBody(response.status, body, fallback);
    }

    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  }

  // ------------------------------------------------------------------
  // Auth / system
  // ------------------------------------------------------------------

  /** `GET /api/v1/meta` (no auth required; used to detect the connection type). */
  meta(): Promise<MetaResponse> {
    return this.#request("GET", "/api/v1/meta");
  }

  /** `GET /api/v1/system/info` */
  systemInfo(): Promise<SystemInfoResponse> {
    return this.#request("GET", "/api/v1/system/info");
  }

  /** `POST /api/v1/auth/login` (issues the `fxg_session` Cookie on success). */
  async authLogin(token: string): Promise<void> {
    const previous = this.#token;
    this.#token = token;
    try {
      await this.#request<undefined>("POST", "/api/v1/auth/login", {
        body: { token },
      });
    } catch (error) {
      this.#token = previous;
      throw error;
    }
  }

  /** `POST /api/v1/auth/logout` (invalidates the `fxg_session` Cookie). */
  authLogout(): Promise<void> {
    return this.#request("POST", "/api/v1/auth/logout", { body: {} });
  }

  /** `POST /api/v1/system/kill-switch` (emergency stop). */
  killSwitch(reason: string): Promise<KillSwitchResponse> {
    return this.#request("POST", "/api/v1/system/kill-switch", {
      body: { reason },
    });
  }

  // ------------------------------------------------------------------
  // Projects / Worktrees / Nodes
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

  /** `POST /api/v1/projects/:id/worktrees/prune` (clean up metadata for deleted worktrees). */
  async pruneWorktrees(projectId: string, request: PruneWorktreesRequest): Promise<void> {
    await this.#request(
      "POST",
      `/api/v1/projects/${encodeURIComponent(projectId)}/worktrees/prune`,
      { body: request },
    );
  }

  /** `GET /api/v1/nodes` */
  async nodes(): Promise<NodeSummary[]> {
    const response = await this.#request<NodesResponse>("GET", "/api/v1/nodes");
    return response.nodes;
  }

  /** `GET /api/v1/diagnostics` (agent stderr tails, health, recent incidents). */
  diagnostics(): Promise<DiagnosticsResponse> {
    return this.#request("GET", "/api/v1/diagnostics");
  }

  /** `GET /api/v1/diagnostics/incidents` (newest first). */
  diagnosticsIncidents(limit = 50, includeResolved = false): Promise<IncidentListResponse> {
    return this.#request<IncidentListResponse>("GET", "/api/v1/diagnostics/incidents", {
      query: { limit, include_resolved: includeResolved },
    });
  }

  /** `GET /api/v1/diagnostics/bundle` (redacted support bundle). */
  diagnosticsBundle(): Promise<SupportBundle> {
    return this.#request("GET", "/api/v1/diagnostics/bundle");
  }

  /** `POST /api/v1/diagnostics/incidents/:id/resolve`. */
  resolveIncident(id: number): Promise<void> {
    return this.#request("POST", `/api/v1/diagnostics/incidents/${id}/resolve`);
  }

  /** `GET /api/v1/nodes/:id/fs/browse` */
  browseFs(nodeId: string, path?: string): Promise<FsBrowseResponse> {
    return this.#request<FsBrowseResponse>(
      "GET",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/fs/browse`,
      { query: { path } },
    );
  }

  /** `POST /api/v1/nodes/:id/projects/scan` (bulk scan / register Git repositories). */
  scanProjects(nodeId: string, request: ProjectScanRequest): Promise<ProjectScanResponse> {
    return this.#request("POST", `/api/v1/nodes/${encodeURIComponent(nodeId)}/projects/scan`, {
      body: request,
    });
  }

  /** `POST /api/v1/nodes/:id/projects/link` (manually link an arbitrary directory). */
  linkProject(nodeId: string, request: ProjectLinkRequest): Promise<ProjectLinkResponse> {
    return this.#request("POST", `/api/v1/nodes/${encodeURIComponent(nodeId)}/projects/link`, {
      body: request,
    });
  }

  /** `POST /api/v1/nodes/:id/projects/inspect` (read-only identity preview). */
  inspectProject(nodeId: string, localPath: string): Promise<ProjectPreview> {
    return this.#request("POST", `/api/v1/nodes/${encodeURIComponent(nodeId)}/projects/inspect`, {
      body: { local_path: localPath } satisfies ProjectPreviewRequest,
    });
  }

  /** `POST /api/v1/nodes/:id/projects/add` (auto-detect and register). */
  addProject(nodeId: string, localPath: string): Promise<ProjectPreview> {
    return this.#request("POST", `/api/v1/nodes/${encodeURIComponent(nodeId)}/projects/add`, {
      body: { local_path: localPath } satisfies ProjectPreviewRequest,
    });
  }

  /** `GET /api/v1/provisioners` */
  async provisioners(): Promise<ProvisionerSummary[]> {
    const response = await this.#request<ProvisionersResponse>("GET", "/api/v1/provisioners");
    return response.provisioners;
  }

  /** `POST /api/v1/provisioners/:name/test` (connectivity check). */
  testProvisioner(name: string): Promise<ProvisionerTestResponse> {
    return this.#request<ProvisionerTestResponse>(
      "POST",
      `/api/v1/provisioners/${encodeURIComponent(name)}/test`,
    );
  }

  // ------------------------------------------------------------------
  // Sessions / approvals / search / audit
  // ------------------------------------------------------------------

  /** `GET /api/v1/sessions` */
  async sessions(
    filter: {
      projectId?: string | null;
      nodeId?: string | null;
      status?: string | null;
      /** Whether to include archived sessions (default `false` = exclude). */
      includeArchived?: boolean | null;
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
          include_archived: filter.includeArchived,
          limit: filter.limit,
        },
      },
    );
    return response.sessions;
  }

  /** `POST /api/v1/sessions` (start a new session). */
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

  /** `GET /api/v1/inbox` (unresolved approvals / questions) */
  inbox(): Promise<InboxResponse> {
    return this.#request<InboxResponse>("GET", "/api/v1/inbox");
  }

  /** `POST /api/v1/sessions/:id/permissions/:req_id/respond` (idempotent). */
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

  /** `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond` (idempotent). */
  respondElicitation(
    sessionId: string,
    elicitationId: string,
    request: RespondElicitationRequest,
  ): Promise<RespondElicitationResponse> {
    return this.#request(
      "POST",
      `/api/v1/sessions/${encodeURIComponent(sessionId)}/elicitations/${encodeURIComponent(elicitationId)}/respond`,
      { body: request },
    );
  }

  /** `POST /api/v1/sessions/:id/resume` (resume a stopped session). */
  resumeSession(sessionId: string, request: ResumeSessionRequest): Promise<ResumeSessionResponse> {
    return this.#request("POST", `/api/v1/sessions/${encodeURIComponent(sessionId)}/resume`, {
      body: request,
    });
  }

  /** `POST /api/v1/sessions/:id/archive` (archive / restore). */
  archiveSession(
    sessionId: string,
    request: SessionArchiveRequest,
  ): Promise<SessionArchiveResponse> {
    return this.#request("POST", `/api/v1/sessions/${encodeURIComponent(sessionId)}/archive`, {
      body: request,
    });
  }

  /** `DELETE /api/v1/sessions/:id` (delete; erases the conversation log irreversibly). */
  deleteSession(sessionId: string): Promise<void> {
    return this.#request("DELETE", `/api/v1/sessions/${encodeURIComponent(sessionId)}`);
  }

  /** `POST /api/v1/search` (FTS5 full-text search). */
  async search(query: string, limit?: number): Promise<SearchHit[]> {
    const response = await this.#request<SearchResponse>("GET", "/api/v1/search", {
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

  /** `POST /api/v1/push/subscribe` (register a Web Push subscription; central server only). */
  pushSubscribe(request: PushSubscribeRequest): Promise<PushSubscribeResponse> {
    return this.#request("POST", "/api/v1/push/subscribe", { body: request });
  }

  // ------------------------------------------------------------------
  // Agent management / auth tokens (Phase D)
  // ------------------------------------------------------------------

  /** `GET /api/v1/agents` (ACP Registry catalog + node install status). */
  async agents(): Promise<AgentSummary[]> {
    const response = await this.#request<AgentsResponse>("GET", "/api/v1/agents");
    return response.agents;
  }

  /** `POST /api/v1/nodes/:id/agents/refresh` (force-refresh the catalog) */
  refreshAgentsCatalog(nodeId: string): Promise<AgentOpResponse> {
    return this.#request("POST", `/api/v1/nodes/${encodeURIComponent(nodeId)}/agents/refresh`);
  }

  /** `PUT /api/v1/nodes/:id/agents/custom/:agent_id` (create / update a custom definition) */
  upsertCustomAgent(
    nodeId: string,
    agentId: string,
    config: CustomAgentConfig,
  ): Promise<AgentOpResponse> {
    return this.#request(
      "PUT",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/agents/custom/${encodeURIComponent(agentId)}`,
      { body: config },
    );
  }

  /** `DELETE /api/v1/nodes/:id/agents/custom/:agent_id` (delete a custom definition) */
  removeCustomAgent(nodeId: string, agentId: string): Promise<AgentOpResponse> {
    return this.#request(
      "DELETE",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/agents/custom/${encodeURIComponent(agentId)}`,
    );
  }

  /** `POST /api/v1/nodes/:id/agents/:agent_id/package` (enable use via npx/uvx) */
  enablePackageAgent(nodeId: string, agentId: string): Promise<AgentOpResponse> {
    return this.#request(
      "POST",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/agents/${encodeURIComponent(agentId)}/package`,
    );
  }

  /** `DELETE /api/v1/nodes/:id/agents/:agent_id/package` (disable use via npx/uvx) */
  disablePackageAgent(nodeId: string, agentId: string): Promise<AgentOpResponse> {
    return this.#request(
      "DELETE",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/agents/${encodeURIComponent(agentId)}/package`,
    );
  }

  // ------------------------------------------------------------------
  // Settings / secrets / project profiles
  // ------------------------------------------------------------------

  /** `GET /api/v1/settings/server` (central server). */
  serverSettings(): Promise<ServerSettingsSnapshot> {
    return this.#request("GET", "/api/v1/settings/server");
  }

  /** `PATCH /api/v1/settings/server` (central server). */
  updateServerSettings(request: UpdateServerSettingsRequest): Promise<ServerSettingsSnapshot> {
    return this.#request("PATCH", "/api/v1/settings/server", { body: request });
  }

  /** `GET /api/v1/nodes/:id/settings` (node-owned, local or relayed). */
  nodeSettings(nodeId: string): Promise<NodeSettingsSnapshot> {
    return this.#request("GET", `/api/v1/nodes/${encodeURIComponent(nodeId)}/settings`);
  }

  /** `PATCH /api/v1/nodes/:id/settings/node` (requires the observed revision). */
  updateNodeSettings(
    nodeId: string,
    request: UpdateNodeSettingsRequest,
  ): Promise<NodeSettingsSnapshot> {
    return this.#request("PATCH", `/api/v1/nodes/${encodeURIComponent(nodeId)}/settings/node`, {
      body: request,
    });
  }

  /** `PATCH /api/v1/nodes/:id/settings/agents` (requires the observed revision). */
  updateAgentSettings(
    nodeId: string,
    request: UpdateAgentSettingsRequest,
  ): Promise<NodeSettingsSnapshot> {
    return this.#request("PATCH", `/api/v1/nodes/${encodeURIComponent(nodeId)}/settings/agents`, {
      body: request,
    });
  }

  /** `GET /api/v1/nodes/:id/secrets` (node-owned, local or relayed). */
  nodeSecrets(nodeId: string): Promise<SecretsResponse> {
    return this.#request("GET", `/api/v1/nodes/${encodeURIComponent(nodeId)}/secrets`);
  }

  /** `PUT /api/v1/nodes/:id/secrets/:secret_id` (write-only). */
  upsertNodeSecret(
    nodeId: string,
    secretId: string,
    request: UpsertSecretRequest,
  ): Promise<SecretsResponse> {
    return this.#request(
      "PUT",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/secrets/${encodeURIComponent(secretId)}`,
      { body: request },
    );
  }

  /** `DELETE /api/v1/nodes/:id/secrets/:secret_id`. */
  deleteNodeSecret(nodeId: string, secretId: string): Promise<SecretsResponse> {
    return this.#request(
      "DELETE",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/secrets/${encodeURIComponent(secretId)}`,
    );
  }

  /** `GET /api/v1/server/secrets` (server-owned). */
  serverSecrets(): Promise<SecretsResponse> {
    return this.#request("GET", "/api/v1/server/secrets");
  }

  /** `PUT /api/v1/server/secrets/:id` (write-only). */
  upsertServerSecret(secretId: string, request: UpsertSecretRequest): Promise<SecretsResponse> {
    return this.#request("PUT", `/api/v1/server/secrets/${encodeURIComponent(secretId)}`, {
      body: request,
    });
  }

  /** `DELETE /api/v1/server/secrets/:id`. */
  deleteServerSecret(secretId: string): Promise<SecretsResponse> {
    return this.#request("DELETE", `/api/v1/server/secrets/${encodeURIComponent(secretId)}`);
  }

  /** `GET /api/v1/projects/:id/profile` (server-owned). */
  projectProfile(projectId: string): Promise<ProjectProfile> {
    return this.#request("GET", `/api/v1/projects/${encodeURIComponent(projectId)}/profile`);
  }

  /** `PUT /api/v1/projects/:id/profile` (server-owned). */
  updateProjectProfile(
    projectId: string,
    request: UpdateProjectProfileRequest,
  ): Promise<ProjectProfile> {
    return this.#request("PUT", `/api/v1/projects/${encodeURIComponent(projectId)}/profile`, {
      body: request,
    });
  }

  /** `GET /api/v1/nodes/:id/agents` (catalog for a specific node). */
  async nodeAgents(nodeId: string): Promise<AgentSummary[]> {
    const response = await this.#request<AgentsResponse>(
      "GET",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/agents`,
    );
    return response.agents;
  }

  /** `POST /api/v1/nodes/:id/agents/:agentId/probe` (temporary ACP session). */
  probeAgent(
    nodeId: string,
    agentId: string,
    localPath: string,
    force = false,
    initialConfig: InitialConfig[] = [],
  ): Promise<AgentCapabilitiesPreview> {
    return this.#request<AgentCapabilitiesPreview>(
      "POST",
      `/api/v1/nodes/${encodeURIComponent(nodeId)}/agents/${encodeURIComponent(agentId)}/probe`,
      { body: { local_path: localPath, force, initial_config: initialConfig } },
    );
  }

  /** `POST /api/v1/auth/rotate-token` (regenerate the client auth token). */
  rotateAuthToken(): Promise<RotateAuthTokenResponse> {
    return this.#request("POST", "/api/v1/auth/rotate-token");
  }

  /** `GET /api/v1/nodes/tokens` (central server only). */
  async nodeTokens(): Promise<NodeTokenSummary[]> {
    const response = await this.#request<NodeTokensResponse>("GET", "/api/v1/nodes/tokens");
    return response.tokens;
  }

  /** `POST /api/v1/nodes/tokens` (issue a per-node token; central server only). */
  issueNodeToken(request: IssueNodeTokenRequest): Promise<IssueNodeTokenResponse> {
    return this.#request("POST", "/api/v1/nodes/tokens", { body: request });
  }

  /** `DELETE /api/v1/nodes/tokens/:node_id` (revoke a per-node token; central server only). */
  async revokeNodeToken(nodeId: string): Promise<void> {
    await this.#request("DELETE", `/api/v1/nodes/tokens/${encodeURIComponent(nodeId)}`);
  }
}

/** Format a structured `CommandResult` error for UI display (`null` when absent). */
export function commandErrorMessage(error: ErrorInfo | null | undefined): string | null {
  if (!error) return null;
  return errorCodeMessage(error.code) ?? error.message ?? null;
}
