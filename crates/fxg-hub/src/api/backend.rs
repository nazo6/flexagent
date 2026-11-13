//! Client-facing backend trait and the types exchanged with it.

use std::net::SocketAddr;

use async_trait::async_trait;
use axum::http::{HeaderMap, header};
use fxg_db::Db;
use fxg_protocol::client_api::{
    AgentCapabilitiesPreview, AgentDiagnostics, AgentOpResponse, AgentsResponse, ConnectionRole,
    CreateSessionRequest, CreateSessionResponse, CreateWorktreeRequest, DiagnosticsResponse,
    HubLinkStatus, IssueNodeTokenRequest, IssueNodeTokenResponse, KillSwitchResponse,
    NodeTokensResponse, ProbeAgentRequest, ProjectLinkRequest, ProjectLinkResponse, ProjectPreview,
    ProjectPreviewRequest, ProjectScanRequest, ProjectScanResponse, ProjectUnlinkRequest,
    ProjectUnlinkResponse, ProvisionersResponse, PruneWorktreesRequest, PushSubscribeRequest,
    PushSubscribeResponse, RemoveWorktreeRequest, RespondElicitationRequest,
    RespondElicitationResponse, RespondPermissionRequest, RespondPermissionResponse,
    ResumeSessionRequest, ResumeSessionResponse, RotateAuthTokenResponse, SessionArchiveRequest,
    SessionArchiveResponse, SessionImportRequest, SessionImportResponse, SessionRenameRequest,
    SessionRenameResponse, SupportBundle, SupportBundleConfig, WorktreeInfo, WorktreesResponse,
};
use fxg_protocol::common::{
    AgentAction, ClientSource, CommandResult, ComponentHealth, DiffScope, ErrorCode,
    ProjectSummary, PromptInput, SessionControlAction, SystemStatus, WorkspaceDiffResponse,
};
use fxg_protocol::events::SessionEventEnvelope;
use fxg_protocol::node_hub::{NodeRpcRequest, NodeRpcResponse};
use fxg_protocol::settings::{
    HubSettingsSnapshot, ProvisionerDefinition, SecretsResponse, UpdateHubSettingsRequest,
    UpsertSecretRequest,
};
use fxg_protocol::{CommandId, NodeId, SecretId, SessionId};
use tokio::sync::broadcast;

use super::{ApiError, PtyChannelError, PtyChannelEvent, PtySpawnParams, header_str};

/// Information about the connected client (for audit logs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    /// Source IP address
    pub ip: String,
    /// User-Agent
    pub user_agent: Option<String>,
    /// Auth subject (`bearer` / `cookie` / `local`)
    pub auth_subject: String,
}

impl ClientInfo {
    /// Builds from request headers and the peer address (`local` when ConnectInfo is absent).
    pub fn from_request(headers: &HeaderMap, addr: Option<SocketAddr>) -> Self {
        let auth_subject = if headers.contains_key(header::AUTHORIZATION) {
            "bearer"
        } else if headers.contains_key(header::COOKIE) {
            "cookie"
        } else {
            "local"
        };
        Self {
            ip: addr
                .map(|addr| addr.ip().to_string())
                .unwrap_or_else(|| "local".to_owned()),
            user_agent: header_str(headers, header::USER_AGENT).map(str::to_owned),
            auth_subject: auth_subject.to_owned(),
        }
    }

    /// Value stored in `audit_logs.auth_subject`.
    pub fn audit_subject(&self) -> String {
        format!("{}:{}", self.auth_subject, self.ip)
    }
}

/// Role-specific fields of `GET /api/v1/system/info`.
#[derive(Debug, Clone, Default)]
pub struct SystemExtras {
    /// VAPID public key for Web Push (hub only)
    pub vapid_public_key: Option<String>,
    /// Number of unsynced Outbox events (only when connected to a local node)
    pub unsynced_event_count: Option<u64>,
    /// Node ⇔ hub link state (only when connected to a local node)
    pub hub_link: Option<HubLinkStatus>,
    /// Last successful synchronization time (Unix epoch ms; local node only)
    pub last_synced_at: Option<i64>,
}

/// Live event delivered over Client WS.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// Event already persisted (cursor committed in the source store)
    Persisted {
        /// Event body
        event: Box<SessionEventEnvelope>,
        /// `session_events.cursor` (delta resume cursor)
        cursor: u64,
    },
    /// Ephemeral chunk mid-stream (not persisted)
    StreamDelta {
        /// Target session ID
        session_id: SessionId,
        /// Delta body
        delta: fxg_protocol::common::StreamDeltaPayload,
    },
    /// Progress log from temporary VM provisioning (`stderr`) (not persisted)
    ProvisioningLog {
        /// Target session ID
        session_id: SessionId,
        /// One log line
        line: String,
    },
}

/// Session operation command over Client WS (excluding `Subscribe` / `Ping`).
#[derive(Debug, Clone, PartialEq)]
pub enum ClientCommand {
    /// Prompt submission
    SendPrompt {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// Prompt body and attachments
        input: PromptInput,
        /// Source (`cli` / `web` / `android`)
        client_source: ClientSource,
    },
    /// Mode switch / configuration change / cancel / kill
    ControlSession {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// Operation to perform
        action: SessionControlAction,
    },
}

impl ClientCommand {
    /// Extracts a command from a WS message (`Subscribe` / `Ping` yield `None`).
    pub fn from_message(message: &fxg_protocol::client_api::ClientWsMessage) -> Option<Self> {
        use fxg_protocol::client_api::ClientWsMessage;
        match message {
            ClientWsMessage::SendPrompt {
                command_id,
                session_id,
                input,
                client_source,
            } => Some(Self::SendPrompt {
                command_id: command_id.clone(),
                session_id: session_id.clone(),
                input: input.clone(),
                client_source: client_source.clone(),
            }),
            ClientWsMessage::ControlSession {
                command_id,
                session_id,
                action,
            } => Some(Self::ControlSession {
                command_id: command_id.clone(),
                session_id: session_id.clone(),
                action: action.clone(),
            }),
            ClientWsMessage::Subscribe { .. } | ClientWsMessage::Ping => None,
        }
    }

    /// Correlation ID.
    pub fn command_id(&self) -> &CommandId {
        match self {
            Self::SendPrompt { command_id, .. } | Self::ControlSession { command_id, .. } => {
                command_id
            }
        }
    }
}

// ----------------------------------------------------------------------
// Backend trait
// ----------------------------------------------------------------------

/// Backend abstraction used by the shared router (node / hub).
///
/// DB read endpoints are self-contained in [`Self::db`], so implementations only supply
/// role-specific behavior (command execution, relaying).
#[async_trait]
pub trait ClientApiBackend: Clone + Send + Sync + 'static {
    /// The target DB (`node.db` for a local node, `hub.db` for the hub).
    fn db(&self) -> &Db;

    /// `GET /api/v1/projects` (list of logical projects).
    ///
    /// The default implementation returns the DB projection as-is. A local node overrides it to
    /// return a list with live existence checks of binding paths.
    async fn list_projects(&self) -> Result<Vec<ProjectSummary>, ApiError> {
        self.db().list_projects().await.map_err(ApiError::from)
    }

    /// Diagnostics (`GET /api/v1/diagnostics`).
    ///
    /// The default implementation combines component health, recent incidents and
    /// the role-specific hooks below. Endpoints only override
    /// [`Self::agent_diagnostics`] / [`Self::role_health`].
    async fn diagnostics(&self) -> Result<DiagnosticsResponse, ApiError> {
        super::diagnostics::build_diagnostics(self).await
    }

    /// Process-local delivery counters (exposed by diagnostics).
    fn diagnostics_counters(&self) -> Option<&super::DiagnosticsCounters> {
        None
    }

    /// Role data directory when the endpoint owns one.
    fn data_dir(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// Database file path of the endpoint.
    fn db_path(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// Diagnostic log path of the endpoint.
    fn log_path(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// Role-specific components (sync / hub).
    async fn role_health(&self, _now: i64) -> Vec<ComponentHealth> {
        Vec::new()
    }

    /// Live node ⇔ hub link updates (local node only; `None` on the hub).
    fn hub_link_updates(&self) -> Option<tokio::sync::watch::Receiver<HubLinkStatus>> {
        None
    }

    /// Agent stderr tails for diagnostics.
    fn agent_diagnostics(&self) -> Vec<AgentDiagnostics> {
        Vec::new()
    }

    /// Redacted bootstrap configuration included in a support bundle.
    fn support_bundle_config(&self) -> SupportBundleConfig {
        SupportBundleConfig::default()
    }

    /// Tail of the process log included in a support bundle (unredacted; the
    /// diagnostics layer applies redaction).
    fn support_bundle_logs(&self, _lines: usize) -> Vec<String> {
        Vec::new()
    }

    /// Component health snapshot (`GET /api/v1/diagnostics` / bundle).
    async fn system_status(&self) -> Result<SystemStatus, ApiError> {
        Ok(super::diagnostics::build_system_status(self).await)
    }

    /// Redacted support bundle (`GET /api/v1/diagnostics/bundle` /
    /// `fxg support bundle`).
    async fn support_bundle(&self) -> Result<SupportBundle, ApiError> {
        super::diagnostics::build_support_bundle(self).await
    }

    /// Requests a graceful process shutdown (`POST /api/v1/system/shutdown`).
    fn request_shutdown(&self) -> bool {
        false
    }

    /// Web access token.
    fn auth_token(&self) -> String;

    /// Kind of connection target (returned by `GET /api/v1/system/info`).
    fn connection_role(&self) -> ConnectionRole;

    /// Node ID when this endpoint is a local node (`None` on the hub).
    fn local_node_id(&self) -> Option<NodeId> {
        None
    }

    /// Executes a generic node operation.
    ///
    /// A local node executes the operation in-process; the hub relays
    /// it through [`NodeHub`](crate::hub::NodeHub). `client` is used by a local
    /// node as the audit source and ignored by the hub (the node records the
    /// relayed operation itself).
    async fn execute_node_rpc(
        &self,
        node_id: &NodeId,
        request: NodeRpcRequest,
        client: Option<ClientInfo>,
    ) -> Result<NodeRpcResponse, ApiError>;

    /// fxg version.
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Role-specific fields of `GET /api/v1/system/info`.
    async fn system_extras(&self) -> Result<SystemExtras, ApiError>;

    /// Subscribes to live events for Client WS.
    fn subscribe_client_events(&self) -> broadcast::Receiver<ClientEvent>;

    /// Executes a command from Client WS and returns a `CommandResult`.
    async fn dispatch_command(&self, command: ClientCommand) -> CommandResult;

    /// `POST /api/v1/sessions` (start a new session).
    async fn create_session(
        &self,
        request: CreateSessionRequest,
        client: ClientInfo,
    ) -> Result<CreateSessionResponse, ApiError>;

    /// `POST /api/v1/sessions/import` (take over an existing agent session).
    ///
    /// The default implementation is for endpoints that cannot own an agent
    /// process (the hub overrides it to relay to the target node).
    async fn import_session(
        &self,
        _request: SessionImportRequest,
        _client: ClientInfo,
    ) -> Result<SessionImportResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "session import is not supported by this backend",
        ))
    }

    /// `GET /api/v1/sessions/external` (sessions that already exist on the agent side).
    ///
    /// The default implementation is for endpoints that cannot own an agent
    /// process (the hub overrides it to relay to the target node).
    async fn list_external_sessions(
        &self,
        _node_id: Option<&str>,
        _agents: &[String],
        _force: bool,
    ) -> Result<fxg_protocol::client_api::ExternalSessionListResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "external session discovery is not supported by this backend",
        ))
    }

    /// `POST /api/v1/sessions/:id/permissions/:req_id/respond` (idempotent).
    async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        request: RespondPermissionRequest,
        client: ClientInfo,
    ) -> Result<RespondPermissionResponse, ApiError>;

    /// `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond` (idempotent).
    async fn respond_elicitation(
        &self,
        session_id: &str,
        elicitation_id: &str,
        request: RespondElicitationRequest,
        client: ClientInfo,
    ) -> Result<RespondElicitationResponse, ApiError>;

    /// `POST /api/v1/sessions/:id/resume` (resume a stopped session).
    async fn resume_session(
        &self,
        session_id: &str,
        request: ResumeSessionRequest,
        client: ClientInfo,
    ) -> Result<ResumeSessionResponse, ApiError>;

    /// `POST /api/v1/sessions/:id/archive` (archive/restore).
    ///
    /// Archive is a reversible visibility flag; the event log is retained.
    async fn archive_session(
        &self,
        session_id: &str,
        request: SessionArchiveRequest,
        client: ClientInfo,
    ) -> Result<SessionArchiveResponse, ApiError>;

    /// `POST /api/v1/sessions/:id/title` (rename; emits `SessionTitleChanged`).
    async fn rename_session(
        &self,
        session_id: &str,
        request: SessionRenameRequest,
        client: ClientInfo,
    ) -> Result<SessionRenameResponse, ApiError>;

    /// `DELETE /api/v1/sessions/:id` (delete).
    ///
    /// Running sessions are stopped first, then their body events are purged (irreversible).
    async fn delete_session(&self, session_id: &str, client: ClientInfo) -> Result<(), ApiError>;

    /// `GET /api/v1/projects/:id/worktrees`
    async fn list_worktrees(&self, project_id: &str) -> Result<WorktreesResponse, ApiError>;

    /// `POST /api/v1/projects/:id/worktrees`
    async fn create_worktree(
        &self,
        project_id: &str,
        request: CreateWorktreeRequest,
        client: ClientInfo,
    ) -> Result<WorktreeInfo, ApiError>;

    /// `DELETE /api/v1/projects/:id/worktrees`
    async fn remove_worktree(
        &self,
        project_id: &str,
        request: RemoveWorktreeRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError>;

    /// `POST /api/v1/projects/:id/worktrees/prune`
    /// (cleanup of management info for deleted worktrees).
    async fn prune_worktrees(
        &self,
        project_id: &str,
        request: PruneWorktreesRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError>;

    /// `POST /api/v1/nodes/:node_id/projects/scan`
    /// (bulk scan and registration of the Git repositories under a directory).
    async fn scan_projects(
        &self,
        node_id: &str,
        request: ProjectScanRequest,
        client: ClientInfo,
    ) -> Result<ProjectScanResponse, ApiError>;

    /// `POST /api/v1/nodes/:node_id/projects/link`
    /// (manually link an arbitrary directory to a logical project).
    async fn link_project(
        &self,
        node_id: &str,
        request: ProjectLinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectLinkResponse, ApiError>;

    /// `POST /api/v1/nodes/:node_id/projects/unlink`
    /// (remove this node's binding for a directory; orphan projects are pruned).
    async fn unlink_project(
        &self,
        node_id: &str,
        request: ProjectUnlinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectUnlinkResponse, ApiError>;

    /// `POST /api/v1/nodes/:node_id/projects/inspect` (read-only identity preview).
    async fn inspect_project(
        &self,
        node_id: &str,
        request: ProjectPreviewRequest,
        client: ClientInfo,
    ) -> Result<ProjectPreview, ApiError> {
        let response = self
            .execute_node_rpc(
                &NodeId::from(node_id),
                NodeRpcRequest::InspectProject {
                    local_path: request.local_path,
                },
                Some(client),
            )
            .await?;
        match response {
            NodeRpcResponse::ProjectPreview { preview } => Ok(preview),
            _ => Err(ApiError::internal("unexpected node RPC response")),
        }
    }

    /// `POST /api/v1/nodes/:node_id/projects/add` (auto-detect and register).
    async fn add_project(
        &self,
        node_id: &str,
        request: ProjectPreviewRequest,
        client: ClientInfo,
    ) -> Result<ProjectPreview, ApiError> {
        let response = self
            .execute_node_rpc(
                &NodeId::from(node_id),
                NodeRpcRequest::AddProject {
                    local_path: request.local_path,
                },
                Some(client),
            )
            .await?;
        match response {
            NodeRpcResponse::ProjectPreview { preview } => Ok(preview),
            _ => Err(ApiError::internal("unexpected node RPC response")),
        }
    }

    /// `GET /api/v1/agents` (ACP Registry catalog + per-node install state).
    async fn list_agents(&self) -> Result<AgentsResponse, ApiError>;

    /// `POST /api/v1/nodes/:node_id/agents/...` (install / update / remove).
    async fn manage_agent(
        &self,
        node_id: &str,
        action: AgentAction,
        client: ClientInfo,
    ) -> Result<AgentOpResponse, ApiError>;

    /// `POST /api/v1/nodes/:node_id/agents/:agent_id/probe`
    /// (temporary ACP session to discover models / modes).
    async fn probe_agent_config(
        &self,
        node_id: &str,
        agent_id: &str,
        request: ProbeAgentRequest,
        client: ClientInfo,
    ) -> Result<AgentCapabilitiesPreview, ApiError> {
        let response = self
            .execute_node_rpc(
                &NodeId::from(node_id),
                NodeRpcRequest::ProbeAgentConfig {
                    agent_id: agent_id.to_owned(),
                    local_path: request.local_path,
                    force: request.force,
                    initial_config: request.initial_config,
                },
                Some(client),
            )
            .await?;
        match response {
            NodeRpcResponse::AgentCapabilities { preview } => Ok(preview),
            _ => Err(ApiError::internal("unexpected node RPC response")),
        }
    }

    /// `POST /api/v1/auth/rotate-token` (regenerate the Web access token).
    ///
    /// The old token is invalidated immediately, so switch to the new token in the response.
    async fn rotate_auth_token(
        &self,
        client: ClientInfo,
    ) -> Result<RotateAuthTokenResponse, ApiError>;

    /// `GET /api/v1/nodes/tokens` (list issued node pairing tokens).
    ///
    /// Hub only. The default local-node implementation returns `INVALID_STATE`
    /// (node pairing tokens are a server (`hub.db`) concept).
    async fn node_tokens(&self) -> Result<NodeTokensResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node pairing tokens are only available on the hub",
        ))
    }

    /// `POST /api/v1/nodes/tokens` (issue a new node pairing token. Hub only).
    async fn issue_node_token(
        &self,
        _request: IssueNodeTokenRequest,
        _client: ClientInfo,
    ) -> Result<IssueNodeTokenResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node pairing tokens are only available on the hub",
        ))
    }

    /// `DELETE /api/v1/nodes/tokens/:node_id` (revoke a token. Hub only).
    async fn revoke_node_token(&self, _node_id: &str, _client: ClientInfo) -> Result<(), ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node pairing tokens are only available on the hub",
        ))
    }

    /// `GET /api/v1/sessions/:id/diff`
    async fn workspace_diff(
        &self,
        session_id: &str,
        scope: DiffScope,
        base_branch: Option<String>,
    ) -> Result<WorkspaceDiffResponse, ApiError>;

    /// `GET /api/v1/provisioners`
    async fn provisioners(&self) -> Result<ProvisionersResponse, ApiError>;

    /// `POST /api/v1/provisioners/:name/test` (connectivity check).
    ///
    /// Hub only. The default local-node implementation returns `INVALID_STATE`
    /// (provisioners are launched only on the hub host).
    async fn provisioner_test(
        &self,
        _name: &str,
    ) -> Result<fxg_protocol::client_api::ProvisionerTestResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "provisioners can only be tested on the hub",
        ))
    }

    /// `PUT /api/v1/provisioners/:name` (hub only).
    ///
    /// The default local-node implementation returns `INVALID_STATE`
    /// (provisioners are owned by the hub).
    async fn upsert_provisioner(
        &self,
        _name: &str,
        _definition: ProvisionerDefinition,
        _client: ClientInfo,
    ) -> Result<ProvisionersResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "provisioners can only be managed on the hub",
        ))
    }

    /// `DELETE /api/v1/provisioners/:name` (hub only).
    async fn delete_provisioner(&self, _name: &str, _client: ClientInfo) -> Result<(), ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "provisioners can only be managed on the hub",
        ))
    }

    /// `GET /api/v1/settings/hub` (hub only).
    async fn settings_server(&self) -> Result<HubSettingsSnapshot, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "hub settings are only available on the hub",
        ))
    }

    /// `PATCH /api/v1/settings/hub` (hub only).
    async fn update_hub_settings(
        &self,
        _request: UpdateHubSettingsRequest,
    ) -> Result<HubSettingsSnapshot, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "hub settings are only available on the hub",
        ))
    }

    /// `GET /api/v1/hub/secrets` (hub-owned secret metadata).
    async fn server_secrets(&self) -> Result<SecretsResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "server secrets are only available on the hub",
        ))
    }

    /// `PUT /api/v1/hub/secrets/:secret_id` (write-only).
    async fn upsert_server_secret(
        &self,
        _secret_id: &SecretId,
        _request: UpsertSecretRequest,
    ) -> Result<SecretsResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "server secrets are only available on the hub",
        ))
    }

    /// `DELETE /api/v1/hub/secrets/:secret_id`.
    async fn delete_server_secret(
        &self,
        _secret_id: &SecretId,
    ) -> Result<SecretsResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "server secrets are only available on the hub",
        ))
    }

    /// `GET /api/v1/nodes/:node_id/agents` (hub proxy).
    async fn list_agents_node(&self, _node_id: &str) -> Result<AgentsResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "agent listing for a specific node is only available on the hub",
        ))
    }

    /// `POST /api/v1/system/kill-switch`
    async fn kill_switch(
        &self,
        reason: &str,
        client: ClientInfo,
    ) -> Result<KillSwitchResponse, ApiError>;

    /// `POST /api/v1/push/subscribe` (register a Web Push subscription).
    ///
    /// Hub only (Phase 5). The default local-node implementation returns
    /// `INVALID_STATE`.
    async fn push_subscribe(
        &self,
        _request: PushSubscribeRequest,
        _client: ClientInfo,
    ) -> Result<PushSubscribeResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "web push is only available on the hub",
        ))
    }

    // ------------------------------------------------------------------
    // PTY WS (`/api/v1/pty/ws`)
    // ------------------------------------------------------------------

    /// Spawns a PTY (`params.pty_id` is assigned by the caller).
    async fn pty_spawn(&self, params: PtySpawnParams) -> Result<(), PtyChannelError>;

    /// Checks whether a PTY exists (whether it can be attached).
    async fn pty_attach(&self, pty_id: &str) -> Result<(), PtyChannelError>;

    /// Subscribes to PTY output.
    async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Result<broadcast::Receiver<PtyChannelEvent>, PtyChannelError>;

    /// Writes keystrokes to the PTY.
    async fn pty_write(&self, pty_id: &str, data: &[u8]) -> Result<(), PtyChannelError>;

    /// Resizes the PTY.
    async fn pty_resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyChannelError>;

    /// Kills the PTY.
    async fn pty_kill(&self, pty_id: &str) -> Result<(), PtyChannelError>;

    /// Kills PTYs spawned by this socket when the PTY WS disconnects (attached ones excluded).
    async fn pty_disconnect(&self, pty_id: &str);
}
