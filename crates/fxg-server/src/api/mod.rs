//! Shared HTTP/WS API layer for clients.
//!
//! The central server (`fxg server`, `:8080`) and the node's local web server
//! (`fxg daemon`, `127.0.0.1:7860`) expose **exactly the same API paths and WebSocket format**
//! (design: `docs/03-protocol-and-api.md` §3).
//!
//! This module is the shared router/handler implementation; the differences between the two
//! (who owns the DB, how commands execute, whether node relay is involved) are confined to
//! implementations of the [`ClientApiBackend`] trait.
//!
//! - REST: the Axum router built by [`client_router`]
//! - Client WS (`/api/v1/client/ws`): [`ws`]
//! - PTY WS (`/api/v1/pty/ws`): [`pty`]
//! - Security (Host / Origin / Token validation): [`security`]
//! - Token generation and comparison: [`auth`]

pub mod auth;
mod pty;
mod request_context;
mod security;
mod static_ui;
mod ws;

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use futures_util::SinkExt;
use fxg_db::{Db, DbError, SessionFilter};
use fxg_protocol::client_api::{
    AgentOpResponse, AgentsResponse, ApiErrorResponse, AuditLogsResponse, AuthLoginRequest,
    ConnectionRole, CreateSessionRequest, CreateSessionResponse, CreateWorktreeRequest,
    InboxResponse, IssueNodeTokenRequest, IssueNodeTokenResponse, KillSwitchRequest,
    KillSwitchResponse, MetaResponse, NodeTokensResponse, NodesResponse, ProjectLinkRequest,
    ProjectLinkResponse, ProjectScanRequest, ProjectScanResponse, ProjectsResponse,
    ProvisionersResponse, PruneWorktreesRequest, PushSubscribeRequest, PushSubscribeResponse,
    RemoveWorktreeRequest, RespondElicitationRequest, RespondElicitationResponse,
    RespondPermissionRequest, RespondPermissionResponse, ResumeSessionRequest,
    ResumeSessionResponse, RotateAuthTokenResponse, SearchResponse, ServerWsMessage,
    SessionArchiveRequest, SessionArchiveResponse, SessionListResponse, SessionRevertRequest,
    SessionRevertResponse, SystemInfoResponse, WorktreeInfo, WorktreesResponse,
};
use fxg_protocol::common::{
    AgentAction, ClientSource, CommandResult, DataSafety, DiffScope, ElicitationContent, ErrorCode,
    ErrorInfo, ProjectSummary, RawJson, RecoveryAction, ResolvedBy, SessionControlAction,
    SessionStatus, WorkspaceDiffResponse,
};
use fxg_protocol::config::CustomAgentConfig;
use fxg_protocol::events::{SessionEventBatch, SessionEventEnvelope};
use fxg_protocol::{CommandId, SessionId};
use tokio::sync::broadcast;

pub use pty::{PtyChannelError, PtyChannelEvent, PtySpawnParams};
pub use ws::REPLAY_BATCH_SIZE;

use self::auth::token_matches;

// ----------------------------------------------------------------------
// Shared types
// ----------------------------------------------------------------------

/// Role-independent API error (converted to the shared REST / WS error format).
#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP status
    pub status: StatusCode,
    /// Structured error code
    pub code: ErrorCode,
    /// English human-readable fallback message
    pub message: String,
    /// Technical details for the expandable "details" section
    pub details: Option<Box<RawJson>>,
    /// Whether retrying may succeed without a state change
    pub retryable: bool,
    /// Recovery actions the client can offer
    pub recovery: Vec<RecoveryAction>,
    /// Correlation ID shared with logs / incidents
    pub request_id: Option<Box<str>>,
    /// What happened to the user's data
    pub data_safety: Option<DataSafety>,
}

impl ApiError {
    /// Builds an error with an explicit status and code.
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
            retryable: code.is_retryable(),
            recovery: Vec::new(),
            request_id: None,
            data_safety: None,
        }
    }

    /// Builds an error, choosing the recommended status from the `ErrorCode`.
    pub fn from_code(code: ErrorCode, message: impl Into<String>) -> Self {
        let status = match code {
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden | ErrorCode::PtyDisabled | ErrorCode::PermissionDenied => {
                StatusCode::FORBIDDEN
            }
            ErrorCode::NodeOffline
            | ErrorCode::SyncBacklog
            | ErrorCode::VersionMismatch
            | ErrorCode::ProvisionerFailed => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::AlreadyResolved
            | ErrorCode::Busy
            | ErrorCode::CommandDuplicate
            | ErrorCode::ResumeRequired
            | ErrorCode::WorktreeConflict
            | ErrorCode::SnapshotUnavailable
            | ErrorCode::AgentNotInstalled
            | ErrorCode::AgentLaunchFailed
            | ErrorCode::AgentProtocolError
            | ErrorCode::GitUnavailable
            | ErrorCode::GitFailed
            | ErrorCode::Cancelled => StatusCode::CONFLICT,
            ErrorCode::InvalidState | ErrorCode::ConfigInvalid => StatusCode::BAD_REQUEST,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
            ErrorCode::DiskFull => StatusCode::INSUFFICIENT_STORAGE,
            ErrorCode::Internal
            | ErrorCode::IoError
            | ErrorCode::PtyFailed
            | ErrorCode::ServerError
            | ErrorCode::DbCorrupt => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, code, message)
    }

    /// Adds technical details.
    pub fn with_details(mut self, details: impl Into<RawJson>) -> Self {
        self.details = Some(Box::new(details.into()));
        self
    }

    /// Adds a recovery action.
    pub fn with_recovery(mut self, action: RecoveryAction) -> Self {
        self.recovery.push(action);
        self
    }

    /// Attaches the correlation ID.
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into().into_boxed_str());
        self
    }

    /// Declares what happened to the user's data.
    pub fn with_data_safety(mut self, data_safety: DataSafety) -> Self {
        self.data_safety = Some(data_safety);
        self
    }

    /// Builds from a structured error returned by a node.
    pub fn from_info(info: ErrorInfo) -> Self {
        let mut api = Self::from_code(info.code, info.message);
        api.details = info.details.map(Box::new);
        api.retryable = info.retryable;
        api.recovery = info.recovery;
        api.request_id = info.request_id.map(String::into_boxed_str);
        api.data_safety = info.data_safety;
        api
    }

    /// Builds from a failed `CommandResult`, falling back to `INTERNAL` when the
    /// node did not attach an error body.
    pub fn from_command(result: CommandResult) -> Self {
        match result.error {
            Some(info) => Self::from_info(info),
            None => Self::internal("command failed without an error body"),
        }
    }

    /// Converts into the structured error body.
    ///
    /// Fills in the request correlation ID from the current request when the
    /// error itself did not come with one (e.g. errors raised by handlers).
    pub fn into_info(self) -> ErrorInfo {
        ErrorInfo {
            code: self.code,
            message: self.message,
            params: std::collections::BTreeMap::new(),
            details: self.details.map(|details| *details),
            retryable: self.retryable,
            recovery: self.recovery,
            request_id: self
                .request_id
                .map(String::from)
                .or_else(request_context::current_request_id),
            data_safety: self.data_safety,
            docs: None,
        }
    }

    /// `503 NODE_OFFLINE` with reconnect / select-node recovery actions.
    pub fn node_offline(node_id: &str, message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::NodeOffline, message)
            .with_recovery(RecoveryAction::ReconnectNode {
                node_id: node_id.into(),
            })
            .with_recovery(RecoveryAction::SelectOnlineNode)
    }

    /// `400 INVALID_STATE`
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::InvalidState, message)
    }

    /// `404 NOT_FOUND`
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::NotFound, message)
    }

    /// `403 FORBIDDEN`
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::Forbidden, message)
    }

    /// `500 INTERNAL` (also logged)
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::warn!("api error: {err}");
        Self::from_code(ErrorCode::Internal, err.to_string())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ApiError {}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match &err {
            DbError::SessionNotFound(_) | DbError::NodeNotFound(_) => {
                Self::not_found(err.to_string())
            }
            _ => Self::internal(err),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        (
            status,
            Json(ApiErrorResponse {
                error: self.into_info(),
            }),
        )
            .into_response()
    }
}

/// Returns a header value as `&str` (`None` for a missing or non-UTF-8 value).
// `Result::ok` is disallowed by the error-handling policy, so the explicit
// match below is intentional.
#[allow(clippy::manual_ok_err)]
pub(crate) fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    match headers.get(name) {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => None,
        },
        None => None,
    }
}

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
    /// VAPID public key for Web Push (central server only)
    pub vapid_public_key: Option<String>,
    /// Number of unsynced Outbox events (only when connected to a local node)
    pub unsynced_event_count: Option<u64>,
    /// WebSocket connection state to the central server (only when connected to a local node)
    pub central_connected: Option<bool>,
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
    /// Progress log from temporary VM bootstrap (`stderr`) (not persisted)
    BootstrapLog {
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
        /// Prompt body
        text: String,
        /// Source (`cli` / `web` / `android`)
        client_source: ClientSource,
    },
    /// Response to an approval request
    RespondPermission {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// ACP request id
        request_id: String,
        /// Selected `option_id`
        selected_option_id: String,
        /// Who resolved it (`cli` / `web` / `android_push`)
        resolved_by: ResolvedBy,
    },
    /// Response to an elicitation (structured input request)
    RespondElicitation {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// ACP elicitation id
        elicitation_id: String,
        /// User response action (`accept` / `decline` / `cancel`)
        action: fxg_protocol::common::ElicitationAction,
        /// Answer content for `accept` (per the form's `requested_schema`)
        content: Option<ElicitationContent>,
        /// Who resolved it (`cli` / `web` / `android_push`)
        resolved_by: ResolvedBy,
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
                text,
                client_source,
            } => Some(Self::SendPrompt {
                command_id: command_id.clone(),
                session_id: session_id.clone(),
                text: text.clone(),
                client_source: client_source.clone(),
            }),
            ClientWsMessage::RespondPermission {
                command_id,
                session_id,
                request_id,
                selected_option_id,
                resolved_by,
            } => Some(Self::RespondPermission {
                command_id: command_id.clone(),
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                selected_option_id: selected_option_id.clone(),
                resolved_by: resolved_by.clone(),
            }),
            ClientWsMessage::RespondElicitation {
                command_id,
                session_id,
                elicitation_id,
                action,
                content,
                resolved_by,
            } => Some(Self::RespondElicitation {
                command_id: command_id.clone(),
                session_id: session_id.clone(),
                elicitation_id: elicitation_id.clone(),
                action: *action,
                content: content.clone(),
                resolved_by: resolved_by.clone(),
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
            Self::SendPrompt { command_id, .. }
            | Self::RespondPermission { command_id, .. }
            | Self::RespondElicitation { command_id, .. }
            | Self::ControlSession { command_id, .. } => command_id,
        }
    }
}

// ----------------------------------------------------------------------
// Backend trait
// ----------------------------------------------------------------------

/// Backend abstraction used by the shared router (node / central server).
///
/// DB read endpoints are self-contained in [`Self::db`], so implementations only supply
/// role-specific behavior (command execution, relaying).
#[async_trait]
pub trait ClientApiBackend: Clone + Send + Sync + 'static {
    /// The target DB (`node.db` for a local node, `server.db` for the central server).
    fn db(&self) -> &Db;

    /// `GET /api/v1/projects` (list of logical projects).
    ///
    /// The default implementation returns the DB projection as-is. A local node overrides it to
    /// return a list with live existence checks of binding paths.
    async fn list_projects(&self) -> Result<Vec<ProjectSummary>, ApiError> {
        self.db().list_projects().await.map_err(ApiError::from)
    }

    /// Client auth token.
    fn auth_token(&self) -> String;

    /// Kind of connection target (returned by `GET /api/v1/system/info`).
    fn connection_role(&self) -> ConnectionRole;

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

    /// `POST /api/v1/sessions/:id/revert` (rewind via Shadow Git Tree).
    async fn revert_session(
        &self,
        session_id: &str,
        request: SessionRevertRequest,
        client: ClientInfo,
    ) -> Result<SessionRevertResponse, ApiError>;

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

    /// `GET /api/v1/agents` (ACP Registry catalog + per-node install state).
    async fn list_agents(&self) -> Result<AgentsResponse, ApiError>;

    /// `POST /api/v1/nodes/:node_id/agents/...` (install / update / remove).
    async fn manage_agent(
        &self,
        node_id: &str,
        action: AgentAction,
        client: ClientInfo,
    ) -> Result<AgentOpResponse, ApiError>;

    /// `POST /api/v1/auth/rotate-token` (regenerate the client auth token).
    ///
    /// The old token is invalidated immediately, so switch to the new token in the response.
    async fn rotate_auth_token(
        &self,
        client: ClientInfo,
    ) -> Result<RotateAuthTokenResponse, ApiError>;

    /// `GET /api/v1/nodes/tokens` (list issued per-node tokens).
    ///
    /// Central server only. The default local-node implementation returns `INVALID_STATE`
    /// (node tokens are a server (`server.db`) concept).
    async fn node_tokens(&self) -> Result<NodeTokensResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node tokens are only available on the central server",
        ))
    }

    /// `POST /api/v1/nodes/tokens` (issue a new node token. Central server only).
    async fn issue_node_token(
        &self,
        _request: IssueNodeTokenRequest,
        _client: ClientInfo,
    ) -> Result<IssueNodeTokenResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node tokens are only available on the central server",
        ))
    }

    /// `DELETE /api/v1/nodes/tokens/:node_id` (revoke a token. Central server only).
    async fn revoke_node_token(&self, _node_id: &str, _client: ClientInfo) -> Result<(), ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "node tokens are only available on the central server",
        ))
    }

    /// `GET /api/v1/sessions/:id/diff`
    async fn workspace_diff(
        &self,
        session_id: &str,
        scope: DiffScope,
        base_branch: Option<String>,
    ) -> Result<WorkspaceDiffResponse, ApiError>;

    /// `GET /api/v1/nodes/:node_id/fs/browse`
    async fn browse_fs(
        &self,
        node_id: &str,
        path: Option<String>,
    ) -> Result<fxg_protocol::client_api::FsBrowseResponse, ApiError>;

    /// `GET /api/v1/provisioners`
    async fn provisioners(&self) -> Result<ProvisionersResponse, ApiError>;

    /// `POST /api/v1/provisioners/:name/test` (connectivity check).
    ///
    /// Central server only. The default local-node implementation returns `INVALID_STATE`
    /// (provisioners are launched only on the server host).
    async fn provisioner_test(
        &self,
        _name: &str,
    ) -> Result<fxg_protocol::client_api::ProvisionerTestResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "provisioners can only be tested on the central server",
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
    /// Central server only (Phase 5). The default local-node implementation returns
    /// `INVALID_STATE`.
    async fn push_subscribe(
        &self,
        _request: PushSubscribeRequest,
        _client: ClientInfo,
    ) -> Result<PushSubscribeResponse, ApiError> {
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "web push is only available on the central server",
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

// ----------------------------------------------------------------------
// Router
// ----------------------------------------------------------------------

/// Router construction options.
#[derive(Debug, Clone)]
pub struct ClientApiOptions {
    /// The port actually bound (for Host / Origin validation of localhost / 127.0.0.1)
    pub port: u16,
    /// Hosts allowed in addition to the `localhost` family
    /// (the central server's `server.allowed_hosts`)
    pub extra_allowed_hosts: Vec<String>,
    /// Origins allowed in addition to the same origin
    /// (the central server's `server.allowed_origins`)
    pub extra_allowed_origins: Vec<String>,
}

impl ClientApiOptions {
    /// For a local node (loopback only).
    pub fn local(port: u16) -> Self {
        Self {
            port,
            extra_allowed_hosts: Vec::new(),
            extra_allowed_origins: Vec::new(),
        }
    }
}

/// Shared state for the router.
#[derive(Clone)]
pub struct ClientApiState<B: ClientApiBackend> {
    /// Backend implementation
    pub backend: B,
    /// Security configuration
    pub options: Arc<ClientApiOptions>,
}

/// Unauthenticated connection metadata endpoint (`GET /api/v1/meta`).
///
/// The security middleware ([`security::security`]) exempts only this path from authentication
/// (Host / Origin validation still applies).
pub(crate) const META_PATH: &str = "/api/v1/meta";

/// Builds the shared Client REST / WS / PTY WS router.
///
/// The caller must serve it with
/// `axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())`
/// (so the client IP is recorded in audit logs).
pub fn client_router<B: ClientApiBackend>(backend: B, options: ClientApiOptions) -> Router {
    let state = ClientApiState {
        backend,
        options: Arc::new(options),
    };
    let api = Router::new()
        .route(META_PATH, get(meta::<B>))
        .route("/api/v1/system/info", get(system_info::<B>))
        .route("/api/v1/system/kill-switch", post(kill_switch::<B>))
        .route("/api/v1/auth/login", post(auth_login::<B>))
        .route("/api/v1/auth/logout", post(auth_logout))
        .route("/api/v1/projects", get(list_projects::<B>))
        .route(
            "/api/v1/projects/{project_id}/worktrees",
            get(list_worktrees::<B>)
                .post(create_worktree::<B>)
                .delete(remove_worktree::<B>),
        )
        .route(
            "/api/v1/projects/{project_id}/worktrees/prune",
            post(prune_worktrees::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/projects/scan",
            post(scan_projects::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/projects/link",
            post(link_project::<B>),
        )
        .route("/api/v1/agents", get(list_agents::<B>))
        .route(
            "/api/v1/nodes/{node_id}/agents/refresh",
            post(refresh_catalog::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/agents/custom/{agent_id}",
            put(upsert_custom_agent::<B>).delete(remove_custom_agent::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/agents/{agent_id}/package",
            post(enable_package_agent::<B>).delete(disable_package_agent::<B>),
        )
        .route(
            "/api/v1/nodes/tokens",
            get(node_tokens::<B>).post(issue_node_token::<B>),
        )
        .route(
            "/api/v1/nodes/tokens/{node_id}",
            delete(revoke_node_token::<B>),
        )
        .route("/api/v1/auth/rotate-token", post(rotate_auth_token::<B>))
        .route("/api/v1/nodes", get(list_nodes::<B>))
        .route("/api/v1/nodes/{node_id}/fs/browse", get(browse_fs::<B>))
        .route("/api/v1/provisioners", get(list_provisioners::<B>))
        .route(
            "/api/v1/provisioners/{provisioner}/test",
            post(test_provisioner::<B>),
        )
        .route(
            "/api/v1/sessions",
            get(list_sessions::<B>).post(create_session::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/events",
            get(session_events::<B>),
        )
        .route("/api/v1/sessions/{session_id}/diff", get(session_diff::<B>))
        .route(
            "/api/v1/sessions/{session_id}/permissions/{request_id}/respond",
            post(respond_permission::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/elicitations/{elicitation_id}/respond",
            post(respond_elicitation::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/revert",
            post(revert_session::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/resume",
            post(resume_session::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/archive",
            post(archive_session::<B>),
        )
        .route("/api/v1/sessions/{session_id}", delete(delete_session::<B>))
        .route("/api/v1/inbox", get(inbox::<B>))
        .route("/api/v1/search", post(search::<B>))
        .route("/api/v1/push/subscribe", post(push_subscribe::<B>))
        .route("/api/v1/audit/logs", get(audit_logs::<B>))
        .route("/api/v1/client/ws", get(client_ws::<B>))
        .route("/api/v1/pty/ws", get(pty_ws::<B>))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::security::<B>,
        ))
        .layer(middleware::from_fn(request_context::request_context))
        .with_state(state);
    // Serve the embedded Web UI (SPA) for every non-API path.
    // It sits outside the security middleware (the UI assets themselves need no token).
    api.fallback(get(static_ui::serve_ui))
}

// ----------------------------------------------------------------------
// REST handlers
// ----------------------------------------------------------------------

/// `GET /api/v1/meta`: connection metadata that needs no authentication.
///
/// Returns only the connection kind so an unauthenticated Web UI can show the token input
/// dialog correctly (the only API reachable without auth. Host / Origin validation applies).
async fn meta<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    Json(MetaResponse {
        role: state.backend.connection_role(),
    })
    .into_response()
}

async fn system_info<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    let extras = match state.backend.system_extras().await {
        Ok(extras) => extras,
        Err(err) => return err.into_response(),
    };
    Json(SystemInfoResponse {
        role: state.backend.connection_role(),
        version: state.backend.version().to_owned(),
        vapid_public_key: extras.vapid_public_key,
        unsynced_event_count: extras.unsynced_event_count,
        central_connected: extras.central_connected,
    })
    .into_response()
}

/// Name of the auth cookie (`docs/03` §3.0).
const SESSION_COOKIE: &str = "fxg_session";

/// `POST /api/v1/auth/login`: validates the Web UI's token input and issues an `fxg_session`
/// cookie (HttpOnly / SameSite=Strict).
///
/// The endpoint itself must also pass the shared security middleware's authentication
/// (`Authorization: Bearer`), so the token in the body is only re-validated.
async fn auth_login<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Json(request): Json<AuthLoginRequest>,
) -> Response {
    let token = request.token.trim();
    if !token_matches(&state.backend.auth_token(), token) {
        return ApiError::from_code(ErrorCode::Unauthorized, "invalid auth token").into_response();
    }
    let cookie = format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

/// `POST /api/v1/auth/logout`: invalidates the `fxg_session` cookie.
async fn auth_logout() -> Response {
    let cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

async fn list_projects<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.list_projects().await {
        Ok(projects) => Json(ProjectsResponse { projects }).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn list_nodes<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.db().list_nodes().await {
        Ok(nodes) => Json(NodesResponse { nodes }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn browse_fs<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Query(query): Query<fxg_protocol::client_api::FsBrowseQuery>,
) -> Response {
    match state.backend.browse_fs(&node_id, query.path).await {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn list_provisioners<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.provisioners().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

/// `POST /api/v1/provisioners/:name/test`: validates provisioner startup and handshake.
async fn test_provisioner<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(provisioner): UrlPath<String>,
) -> Response {
    match state.backend.provisioner_test(&provisioner).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct SessionListQuery {
    project_id: Option<String>,
    node_id: Option<String>,
    status: Option<String>,
    /// Whether to include archived sessions (default `false`)
    include_archived: Option<bool>,
    limit: Option<u32>,
}

async fn list_sessions<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<SessionListQuery>,
) -> Response {
    let statuses = match params.status.as_deref() {
        Some(status) => match SessionStatus::from_str(status) {
            Ok(status) => vec![status],
            Err(err) => return ApiError::bad_request(err.to_string()).into_response(),
        },
        None => Vec::new(),
    };
    let filter = SessionFilter {
        project_id: params.project_id.map(fxg_protocol::ProjectId::from),
        node_id: params.node_id.map(fxg_protocol::NodeId::from),
        statuses,
        include_archived: params.include_archived.unwrap_or(false),
        limit: Some(params.limit.unwrap_or(100)),
    };
    match state.backend.db().list_sessions(&filter).await {
        Ok(sessions) => Json(SessionListResponse { sessions }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct EventsQuery {
    after_cursor: Option<u64>,
    limit: Option<u32>,
}

async fn session_events<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(session_id): UrlPath<String>,
    Query(params): Query<EventsQuery>,
) -> Response {
    match state
        .backend
        .db()
        .session_events_after(
            &session_id,
            params.after_cursor.unwrap_or(0),
            params.limit.unwrap_or(REPLAY_BATCH_SIZE),
        )
        .await
    {
        Ok(batch) => Json(batch).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct DiffQuery {
    scope: Option<String>,
    base: Option<String>,
}

async fn session_diff<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(session_id): UrlPath<String>,
    Query(params): Query<DiffQuery>,
) -> Response {
    let scope = match params.scope.as_deref() {
        // Accept both the design notation (`scope=branch`) and the enum name
        None | Some("") | Some("uncommitted") => DiffScope::Uncommitted,
        Some("branch") | Some("branch_base") | Some("branchbase") => DiffScope::BranchBase,
        Some(other) => {
            return ApiError::bad_request(format!("unknown diff scope: {other}")).into_response();
        }
    };
    match state
        .backend
        .workspace_diff(&session_id, scope, params.base)
        .await
    {
        Ok(diff) => Json(diff).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn inbox<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    let db = state.backend.db();
    match (
        db.pending_permissions().await,
        db.pending_elicitations().await,
    ) {
        (Ok(requests), Ok(elicitations)) => Json(InboxResponse {
            requests,
            elicitations,
        })
        .into_response(),
        (Err(err), _) | (_, Err(err)) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchBody {
    query: Option<String>,
    limit: Option<u32>,
}

async fn search<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<SearchQuery>,
    body: Option<Json<SearchBody>>,
) -> Response {
    let body = body.map(|Json(body)| body);
    let query = params
        .q
        .or_else(|| body.as_ref().and_then(|body| body.query.clone()))
        .unwrap_or_default();
    let limit = params
        .limit
        .or_else(|| body.as_ref().and_then(|body| body.limit))
        .unwrap_or(50);

    match state.backend.db().search(&query, limit).await {
        Ok(hits) => Json(SearchResponse { hits }).into_response(),
        Err(fxg_db::DbError::EmptySearchQuery) => {
            ApiError::bad_request("search query must not be empty").into_response()
        }
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct AuditQuery {
    limit: Option<u32>,
}

async fn audit_logs<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<AuditQuery>,
) -> Response {
    match state
        .backend
        .db()
        .audit_logs(params.limit.unwrap_or(50))
        .await
    {
        Ok(logs) => Json(AuditLogsResponse { logs }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

async fn create_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<CreateSessionRequest>,
) -> Response {
    match state.backend.create_session(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn respond_permission<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((session_id, request_id)): UrlPath<(String, String)>,
    Json(request): Json<RespondPermissionRequest>,
) -> Response {
    match state
        .backend
        .respond_permission(&session_id, &request_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn respond_elicitation<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((session_id, elicitation_id)): UrlPath<(String, String)>,
    Json(request): Json<RespondElicitationRequest>,
) -> Response {
    match state
        .backend
        .respond_elicitation(&session_id, &elicitation_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn revert_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
    Json(request): Json<SessionRevertRequest>,
) -> Response {
    match state
        .backend
        .revert_session(&session_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn resume_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
    Json(request): Json<ResumeSessionRequest>,
) -> Response {
    match state
        .backend
        .resume_session(&session_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn archive_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
    Json(request): Json<SessionArchiveRequest>,
) -> Response {
    match state
        .backend
        .archive_session(&session_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn delete_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
) -> Response {
    match state.backend.delete_session(&session_id, client).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn list_worktrees<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(project_id): UrlPath<String>,
) -> Response {
    match state.backend.list_worktrees(&project_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn create_worktree<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<CreateWorktreeRequest>,
) -> Response {
    match state
        .backend
        .create_worktree(&project_id, request, client)
        .await
    {
        Ok(worktree) => Json(worktree).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn remove_worktree<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<RemoveWorktreeRequest>,
) -> Response {
    match state
        .backend
        .remove_worktree(&project_id, request, client)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn prune_worktrees<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<PruneWorktreesRequest>,
) -> Response {
    match state
        .backend
        .prune_worktrees(&project_id, request, client)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn scan_projects<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectScanRequest>,
) -> Response {
    match state.backend.scan_projects(&node_id, request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn link_project<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectLinkRequest>,
) -> Response {
    match state.backend.link_project(&node_id, request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn list_agents<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.list_agents().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn refresh_catalog<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state
        .backend
        .manage_agent(&node_id, AgentAction::RefreshCatalog, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn upsert_custom_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
    Json(config): Json<CustomAgentConfig>,
) -> Response {
    match state
        .backend
        .manage_agent(
            &node_id,
            AgentAction::UpsertCustom { agent_id, config },
            client,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn remove_custom_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .manage_agent(&node_id, AgentAction::RemoveCustom { agent_id }, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn enable_package_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .manage_agent(
            &node_id,
            AgentAction::SetPackageEnabled {
                agent_id,
                enabled: true,
            },
            client,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn disable_package_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .manage_agent(
            &node_id,
            AgentAction::SetPackageEnabled {
                agent_id,
                enabled: false,
            },
            client,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn rotate_auth_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
) -> Response {
    match state.backend.rotate_auth_token(client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn node_tokens<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    match state.backend.node_tokens().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn issue_node_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<IssueNodeTokenRequest>,
) -> Response {
    match state.backend.issue_node_token(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn revoke_node_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state.backend.revoke_node_token(&node_id, client).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn kill_switch<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<KillSwitchRequest>,
) -> Response {
    let reason = if request.reason.trim().is_empty() {
        "kill switch".to_owned()
    } else {
        request.reason
    };
    match state.backend.kill_switch(&reason, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn push_subscribe<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<PushSubscribeRequest>,
) -> Response {
    match state.backend.push_subscribe(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

// ----------------------------------------------------------------------
// WebSocket upgrades
// ----------------------------------------------------------------------

async fn client_ws<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    ws: WebSocketUpgrade,
) -> Response {
    let backend = state.backend.clone();
    ws.on_upgrade(move |socket| ws::handle_client_ws(backend, socket))
}

async fn pty_ws<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    ws: WebSocketUpgrade,
) -> Response {
    let backend = state.backend.clone();
    ws.on_upgrade(move |socket| pty::handle_pty_ws(backend, socket))
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

/// Shared helper for sending over WS (`send_json`).
pub(crate) async fn send_ws_json<T: serde::Serialize>(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: &T,
) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|err| {
        tracing::warn!("failed to encode ws message: {err}");
    })?;
    sender
        .send(Message::Text(text.into()))
        .await
        .map_err(|err| {
            tracing::debug!("ws send failed: {err}");
        })
}

/// Sends all events from `cursor` onward as `SessionEventBatch` messages.
pub(crate) async fn replay_events<B: ClientApiBackend>(
    backend: &B,
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    cursor: &mut u64,
) -> Result<(), String> {
    loop {
        let batch: SessionEventBatch = backend
            .db()
            .events_after_cursor(*cursor, REPLAY_BATCH_SIZE)
            .await
            .map_err(|err| {
                tracing::warn!("failed to load events for replay: {err}");
                format!("failed to load events for replay: {err}")
            })?;
        if batch.events.is_empty() {
            return Ok(());
        }
        let full_batch = batch.events.len() as u32 >= REPLAY_BATCH_SIZE;
        *cursor = (*cursor).max(batch.cursor);
        send_ws_json(
            sender,
            &ServerWsMessage::EventBatch {
                events: batch.events,
                cursor: batch.cursor,
            },
        )
        .await
        .map_err(|()| "failed to send replayed events".to_owned())?;
        if !full_batch {
            return Ok(());
        }
    }
}

/// Whether the `Request` is a WebSocket upgrade.
pub(crate) fn is_websocket_upgrade(request: &Request) -> bool {
    header_str(request.headers(), header::UPGRADE)
        .map(|value| value.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}
