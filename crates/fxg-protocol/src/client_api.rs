//! API type definitions for Client (PWA / Browser / CLI) <-> Server (or Local Node).
//!
//! The central server (`fxg server`, `:8080`) and the node's local web server
//! (`fxg daemon`, `127.0.0.1:7860`) implement exactly the same API paths and
//! WebSocket format (design: `docs/03-protocol-and-api.md` §3). The UI connects
//! to either of them with the same types and the same code, without caring about
//! the endpoint URL.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::common::{
    AuditLogEntry, ClientSource, CommandResult, ElicitationAction, ElicitationContent,
    ElicitationRequestEntry, ErrorInfo, NodeSummary, PermissionRequestEntry, ProjectSummary,
    ResolvedBy, SearchHit, SessionControlAction, SessionSummary, StreamDeltaPayload,
};
use crate::config::CustomAgentConfig;
use crate::events::SessionEventBatch;
use crate::{CommandId, NodeId, SessionId};

/// Kind of endpoint (`GET /api/v1/system/info`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum ConnectionRole {
    /// Connected to the central server (`fxg server`)
    CentralServer,
    /// Connected to the node's local web server (`fxg daemon`)
    LocalNode,
}

/// `GET /api/v1/system/info` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SystemInfoResponse {
    /// Kind of endpoint
    pub role: ConnectionRole,
    /// fxg binary version
    pub version: String,
    /// VAPID public key for Web Push (central server only)
    pub vapid_public_key: Option<String>,
    /// Number of unsynced outbox events (local node only)
    pub unsynced_event_count: Option<u64>,
    /// WebSocket connection state to the central server (local node only)
    pub central_connected: Option<bool>,
}

/// `GET /api/v1/meta` response (endpoint metadata that needs no authentication).
///
/// `GET /api/v1/system/info` requires authentication, so an unauthenticated Web UI
/// determines the endpoint kind from this endpoint and switches the token dialog
/// correctly (central server / local node) (design: `docs/03` §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct MetaResponse {
    /// Kind of endpoint
    pub role: ConnectionRole,
}

/// Error response body shared by REST and WS.
///
/// Format: `{ "error": { "code": "<ErrorCode>", "message": "...", ... } }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ApiErrorResponse {
    /// The structured error
    pub error: ErrorInfo,
}

/// `GET /api/v1/projects` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectsResponse {
    /// Logical projects (including node / worktree bindings)
    pub projects: Vec<ProjectSummary>,
}

/// `GET /api/v1/nodes` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodesResponse {
    /// Nodes (including online state and ephemeral-node attributes)
    pub nodes: Vec<NodeSummary>,
}

/// An available ephemeral VM / sandbox provisioner (`GET /api/v1/provisioners`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProvisionerSummary {
    /// Provisioner identifier (`local-docker` / `colab-pro` etc.)
    pub name: String,
    /// Description
    pub description: Option<String>,
    /// Seconds until the idle instance is discarded automatically
    pub idle_timeout_secs: Option<u64>,
}

/// `GET /api/v1/provisioners` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProvisionersResponse {
    /// Provisioners
    pub provisioners: Vec<ProvisionerSummary>,
}

/// `POST /api/v1/provisioners/:name/test` response (connectivity check).
///
/// Starts the provisioner and verifies up to the `NodeHello` handshake of
/// `fxg daemon --stdio` (or reports a failure during bootstrapping).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProvisionerTestResponse {
    /// Name of the tested provisioner
    pub name: String,
    /// Whether the handshake succeeded
    pub ok: bool,
    /// Ephemeral node ID (when the handshake succeeded)
    pub node_id: Option<NodeId>,
    /// Bootstrap log written to `stderr` (the last N lines)
    pub log_lines: Vec<String>,
    /// Failure message
    pub error: Option<String>,
}

/// Information about a git worktree (`GET /api/v1/projects/:id/worktrees`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorktreeInfo {
    /// Node ID
    pub node_id: NodeId,
    /// Local path of the worktree
    pub path: String,
    /// Branch currently checked out
    pub branch: Option<String>,
    /// HEAD commit
    pub head_commit: Option<String>,
    /// Whether this is the main repository (not a worktree)
    pub is_main: bool,
}

/// `GET /api/v1/projects/:id/worktrees` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorktreesResponse {
    /// Worktrees
    pub worktrees: Vec<WorktreeInfo>,
}

/// `POST /api/v1/projects/:id/worktrees` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct CreateWorktreeRequest {
    /// Target node ID
    pub node_id: NodeId,
    /// Name of the branch to create
    pub branch: String,
    /// Base branch (defaults to the current HEAD)
    pub base_branch: Option<String>,
    /// Explicit destination path (resolved from `worktree_dir_template` when omitted)
    pub path: Option<String>,
}

/// `DELETE /api/v1/projects/:id/worktrees` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct RemoveWorktreeRequest {
    /// Target node ID
    pub node_id: NodeId,
    /// Path to remove
    pub path: String,
    /// Whether to remove even if there are uncommitted changes
    pub force: bool,
}

/// `POST /api/v1/projects/:id/worktrees/prune` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PruneWorktreesRequest {
    /// Target node ID
    pub node_id: NodeId,
}

/// `POST /api/v1/nodes/:node_id/projects/scan` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectScanRequest {
    /// Directory to scan
    /// (walks `node.project_scan_dirs` of `config.toml` when omitted)
    pub dir: Option<String>,
}

/// `POST /api/v1/nodes/:node_id/projects/scan` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectScanResponse {
    /// Directories that were actually scanned
    pub scanned_dirs: Vec<String>,
    /// Projects registered on the node after the scan
    pub projects: Vec<ProjectSummary>,
}

/// `POST /api/v1/nodes/:node_id/projects/link` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectLinkRequest {
    /// Logical project ID to link to
    pub project_id: String,
    /// Target directory
    pub local_path: String,
}

/// `POST /api/v1/nodes/:node_id/projects/link` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectLinkResponse {
    /// The resolved logical project ID
    pub project_id: String,
    /// Target directory (the local path after resolution)
    pub local_path: String,
}

/// `GET /api/v1/sessions` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionListResponse {
    /// Sessions (newest update first)
    pub sessions: Vec<SessionSummary>,
}

/// `GET /api/v1/agents` response (ACP Registry catalog plus installation state).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentsResponse {
    /// Every agent of the registry (including built-in and custom ones)
    pub agents: Vec<AgentSummary>,
}

/// How an agent is launched (the means actually usable on this node).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum AgentSource {
    /// An installation already on PATH (detected with `which`)
    External,
    /// Execution of an npm / uvx package (fetched at run time)
    Package,
    /// A custom ACP agent defined in `config.toml` or in the UI
    Custom,
}

/// A single ACP agent (catalog information plus installation state).
///
/// An agent with an empty `sources` is "listed in the catalog but not yet
/// startable on this node".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentSummary {
    /// Agent ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Version provided by the registry (`-` for custom agents)
    pub version: String,
    /// Description
    pub description: Option<String>,
    /// Launch forms usable on this node (empty = not installed)
    pub sources: Vec<AgentSource>,
    /// Whether it exists in the ACP Registry catalog
    pub registry: bool,
    /// Whether it can run through `npx` / `uvx` (present on PATH).
    /// When `true`, the UI's "enable" action can add it to the list.
    pub package_available: bool,
    /// Executable detected on PATH (only when external)
    pub external_path: Option<String>,
    /// Contents of the custom definition (for editing in the UI)
    pub custom_config: Option<CustomAgentConfig>,
    /// Nodes where it is available (based on the NodeHello reports)
    pub installed_nodes: Vec<String>,
}

/// Common response of agent management operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentOpResponse {
    /// Message describing the outcome (e.g. `installed opencode2 0.1.0`)
    pub message: Option<String>,
}

/// One entry of `GET /api/v1/nodes/tokens` (an issued per-node token).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeTokenSummary {
    /// Target node ID
    pub node_id: NodeId,
    /// Leading prefix of the token hash (for matching and display; the plaintext is never stored)
    pub token_prefix: String,
}

/// `GET /api/v1/nodes/tokens` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeTokensResponse {
    /// Issued node tokens
    pub tokens: Vec<NodeTokenSummary>,
}

/// `POST /api/v1/nodes/tokens` request (issue a new node token).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct IssueNodeTokenRequest {
    /// Node ID to pair (the node_id of the fxg daemon)
    pub node_id: NodeId,
}

/// `POST /api/v1/nodes/tokens` response.
///
/// The plaintext token is returned only once, at issue time (the server stores
/// only its SHA-256 hash).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct IssueNodeTokenResponse {
    /// Target node ID
    pub node_id: NodeId,
    /// The issued plaintext token (stored in `~/.flexagent/node_token`)
    pub token: String,
}

/// `POST /api/v1/auth/rotate-token` response (regenerate the client token).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct RotateAuthTokenResponse {
    /// New client authentication token (the old one is invalidated immediately)
    pub token: String,
}

/// Specification of a worktree created together with the session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorktreeSpec {
    /// Branch name
    pub branch: String,
    /// Base branch
    pub base_branch: Option<String>,
    /// Explicit destination path
    pub new_path: Option<String>,
}

/// Specification of a context fork to another node or agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionForkSpec {
    /// Source session ID
    pub from_session_id: SessionId,
    /// Event sequence number of the fork source (latest when omitted)
    pub from_node_seq: Option<u64>,
    /// Bundle data (Base64) when restoring a git bundle saved from an ephemeral VM
    pub restore_git_bundle_b64: Option<String>,
}

/// `POST /api/v1/sessions` request (start a new session).
///
/// Besides specifying an existing resident `node_id`, it supports on-demand
/// start-up and automatic setup of an ephemeral VM via `provisioner`, creating a
/// worktree at the same time, and a context fork.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct CreateSessionRequest {
    /// Correlation ID assigned by the client (used for idempotent duplicate rejection)
    pub command_id: CommandId,
    /// Logical project ID
    pub project_id: String,
    /// ID of the resident node to run on (mutually exclusive with `provisioner`)
    pub node_id: Option<NodeId>,
    /// Ephemeral VM provisioner name (mutually exclusive with `node_id`; requires the central server)
    pub provisioner: Option<String>,
    /// Working directory (an existing worktree or the repository root)
    pub local_path: Option<String>,
    /// Specification used when creating and starting in a new worktree
    pub worktree: Option<WorktreeSpec>,
    /// Agent ID (`opencode2` / `antigravity-acp` etc.)
    pub agent_id: String,
    /// Initial prompt
    pub initial_prompt: Option<String>,
    /// Initial mode (`code` / `plan` etc.)
    pub mode: Option<String>,
    /// Extra arguments passed through to the agent
    pub extra_args: Option<Vec<String>>,
    /// Fork specification from an existing session
    pub fork: Option<SessionForkSpec>,
}

/// `POST /api/v1/sessions` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct CreateSessionResponse {
    /// ID of the started session
    pub session_id: SessionId,
    /// Correlation ID from the request
    pub command_id: CommandId,
}

/// `GET /api/v1/inbox` response (unresolved permission requests across all sessions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct InboxResponse {
    /// Unresolved (`pending`) permission requests
    pub requests: Vec<PermissionRequestEntry>,
    /// Unresolved (`pending`) elicitation (structured input) requests
    pub elicitations: Vec<ElicitationRequestEntry>,
}

/// `POST /api/v1/sessions/:id/permissions/:req_id/respond` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct RespondPermissionRequest {
    /// The selected `option_id` (`allow_once` / `allow_always` / `reject` etc.)
    pub selected_option_id: String,
    /// Whether to allow the same tool operation for the rest of the session (promote to `allow_always`)
    pub always: bool,
    /// Who resolved it (`cli` / `web` / `android_push`)
    pub resolved_by: ResolvedBy,
}

/// `POST /api/v1/sessions/:id/permissions/:req_id/respond` response.
///
/// It also returns 200 with `already_resolved = true` when the request was
/// already resolved (the UI treats it as a normal transition; idempotent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct RespondPermissionResponse {
    /// Whether it had already been resolved before this answer
    pub already_resolved: bool,
}

/// `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct RespondElicitationRequest {
    /// The user's response action (`accept` / `decline` / `cancel`)
    pub action: ElicitationAction,
    /// Answer content for `accept` (following the form's `requested_schema`; `None` otherwise)
    pub content: Option<ElicitationContent>,
    /// Who resolved it (`cli` / `web` / `android_push`)
    pub resolved_by: ResolvedBy,
}

/// `POST /api/v1/sessions/:id/elicitations/:elicitation_id/respond` response.
///
/// It also returns 200 with `already_resolved = true` when the request was
/// already resolved (idempotent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct RespondElicitationResponse {
    /// Whether it had already been resolved before this answer
    pub already_resolved: bool,
}

/// `POST /api/v1/sessions/:id/revert` request.
///
/// Restores the workspace to the shadow git tree of the given turn
/// (`UserMessage.node_seq`) (the Web UI version of `fxg session revert`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionRevertRequest {
    /// `node_seq` of the `UserMessage` used as the revert point
    /// (defaults to the latest turn)
    pub target_node_seq: Option<u64>,
}

/// `POST /api/v1/sessions/:id/revert` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionRevertResponse {
    /// `node_seq` of the `UserMessage` used as the reference
    pub target_node_seq: u64,
    /// Tree hash to restore to
    pub restored_tree_hash: String,
    /// Backup tree hash holding the state right before the restore
    /// (another revert brings it back)
    pub backup_tree_hash: Option<String>,
    /// Number of restored files
    pub restored_files: u64,
    /// Number of removed files
    pub removed_files: u64,
}

/// `POST /api/v1/sessions/:id/resume` request.
///
/// Resumes a stopped session with the same `session_id`
/// (design: `docs/04-agent-drivers-and-windows.md` §4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ResumeSessionRequest {
    /// Skip native restoration and continue with history replay (default `false`)
    #[serde(default)]
    pub force_replay: bool,
}

/// `POST /api/v1/sessions/:id/resume` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ResumeSessionResponse {
    /// Resumed session ID (the same as the requested one)
    pub session_id: SessionId,
    /// Whether the agent-side context could be restored natively
    /// (`false` = continued as a new agent session through history replay)
    pub context_restored: bool,
}

/// `POST /api/v1/sessions/:id/archive` request.
///
/// Archiving only hides or restores the session in the listing; the event log is
/// kept (erasing conversation bodies is done by `DELETE /api/v1/sessions/:id`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionArchiveRequest {
    /// `true` = archive, `false` = restore
    pub archived: bool,
}

/// `POST /api/v1/sessions/:id/archive` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionArchiveResponse {
    /// Target session ID
    pub session_id: SessionId,
    /// Archive time (Unix epoch ms). `None` when restored
    pub archived_at: Option<i64>,
}

/// `POST /api/v1/search` response (FTS5 full-text search).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SearchResponse {
    /// Search hits (newest first)
    pub hits: Vec<SearchHit>,
}

/// `POST /api/v1/push/subscribe` request (register a PWA Web Push subscription).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PushSubscribeRequest {
    /// Push service endpoint URL
    pub endpoint: String,
    /// `p256dh` public key of the subscription (Base64)
    pub p256dh: String,
    /// `auth` secret of the subscription (Base64)
    pub auth: String,
    /// Device name (e.g. `Pixel 9 Chrome PWA`)
    pub device_name: Option<String>,
}

/// `POST /api/v1/push/subscribe` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PushSubscribeResponse {
    /// Whether registration succeeded
    pub ok: bool,
}

/// `POST /api/v1/auth/login` request (token entry in the Web UI).
///
/// It is called as an authenticated request (`Authorization: Bearer <auth_token>`)
/// and on success returns
/// `Set-Cookie: fxg_session=<token>; HttpOnly; SameSite=Strict`.
/// Afterwards the cookie can be used for authentication (docs/03 §3.0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AuthLoginRequest {
    /// Authentication token (the value held by the Web UI, also sent in the body for revalidation)
    pub token: String,
}

/// `POST /api/v1/system/kill-switch` request (emergency stop).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct KillSwitchRequest {
    /// Reason for the action (recorded in the audit log)
    pub reason: String,
}

/// `POST /api/v1/system/kill-switch` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct KillSwitchResponse {
    /// Number of nodes the `KillAllSessions` message was delivered to
    pub notified_nodes: u64,
}

/// `GET /api/v1/audit/logs` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AuditLogsResponse {
    /// Audit logs (newest first)
    pub logs: Vec<AuditLogEntry>,
}

/// Client -> Server (`/api/v1/client/ws`) message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "op", rename_all = "snake_case")]
#[specta(collect)]
pub enum ClientWsMessage {
    /// Starts the subscription on connect (specifying the cursor to resume from).
    ///
    /// `since_cursor` refers to `session_events.cursor` of the **connected store**.
    /// When the endpoint is switched, start from `None` (full initial load).
    Subscribe {
        /// Cursor of the last received batch
        since_cursor: Option<u64>,
        /// Currently open session (a hint for prioritized delivery)
        focused_session_id: Option<SessionId>,
    },
    /// Sends a prompt (including slash commands).
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
    /// Answers a permission request.
    RespondPermission {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// ACP request id
        request_id: String,
        /// The selected `option_id`
        selected_option_id: String,
        /// Who resolved it (`cli` / `web` / `android_push`)
        resolved_by: ResolvedBy,
    },
    /// Answers an elicitation (structured input request).
    RespondElicitation {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// ACP elicitation id
        elicitation_id: String,
        /// The user's response action (`accept` / `decline` / `cancel`)
        action: ElicitationAction,
        /// Answer content for `accept` (following the form's `requested_schema`)
        content: Option<ElicitationContent>,
        /// Who resolved it (`cli` / `web` / `android_push`)
        resolved_by: ResolvedBy,
    },
    /// Mode switch / configuration change / cancel / kill.
    ControlSession {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// Operation to execute
        action: SessionControlAction,
    },
    /// Keep-alive
    Ping,
}

/// Server -> Client (`/api/v1/client/ws`) message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "op", rename_all = "snake_case")]
#[specta(collect)]
pub enum ServerWsMessage {
    /// A batch of incremental events (the replay right after connecting and the
    /// realtime delivery afterwards).
    EventBatch {
        /// Events (ascending `node_seq`)
        events: Vec<crate::events::SessionEventEnvelope>,
        /// Cursor of the source store (passed as `since_cursor` when reconnecting)
        cursor: u64,
    },
    /// An ephemeral chunk produced while streaming (never persisted).
    LiveStreamDelta {
        /// Target session ID
        session_id: SessionId,
        /// The delta body
        delta: StreamDeltaPayload,
    },
    /// A progress log line of ephemeral VM bootstrapping (`stderr`) (ephemeral).
    ///
    /// Bootstrapping happens before the ephemeral node's `node.db` exists, so it is
    /// streamed directly from the central server instead of being persisted in the
    /// event log.
    BootstrapLog {
        /// Target session ID
        session_id: SessionId,
        /// A single log line
        line: String,
    },
    /// Result of a command (correlated back to the requesting client).
    CommandResult {
        /// Correlation ID
        command_id: CommandId,
        /// Whether it succeeded
        success: bool,
        /// Structured error on failure
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ErrorInfo>,
        /// Target session ID (optional)
        session_id: Option<SessionId>,
    },
    /// Synchronization state notification (drives the sync badge in the UI).
    SystemStatus {
        /// Whether the central server is connected (when connected to a local node)
        central_connected: bool,
        /// Number of unsynced outbox events
        unsynced_event_count: u64,
        /// Last synchronization time (Unix epoch ms)
        last_synced_at: Option<i64>,
    },
    /// Error notification.
    Error {
        /// Structured error
        error: ErrorInfo,
    },
    /// Response to `Ping`
    Pong,
}

/// Client -> Server (`/api/v1/pty/ws`) message.
///
/// A bidirectional stream channel that connects the terminal of the Web UI / mobile
/// PWA directly to the ConPTY / Unix PTY on the node. When the node setting
/// `allow_remote_pty = false` is in effect, `spawn` from remote (through the
/// central server) is rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "op", rename_all = "snake_case")]
#[specta(collect)]
pub enum PtyClientMessage {
    /// Attach to an existing PTY
    Attach {
        /// PTY ID
        pty_id: String,
    },
    /// Start a new PTY
    Spawn {
        /// Target session ID
        session_id: SessionId,
        /// Number of columns
        cols: u16,
        /// Number of rows
        rows: u16,
        /// Shell command (defaults to the default shell)
        shell_cmd: Option<String>,
    },
    /// Send keystrokes
    Input {
        /// Input bytes (Base64)
        data_b64: String,
    },
    /// Resize the screen
    Resize {
        /// Number of columns
        cols: u16,
        /// Number of rows
        rows: u16,
    },
    /// Terminate the PTY
    Kill,
}

/// Server -> Client (`/api/v1/pty/ws`) message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "op", rename_all = "snake_case")]
#[specta(collect)]
pub enum PtyServerMessage {
    /// The PTY has started (subsequent output belongs to the attached PTY ID)
    Spawned {
        /// ID of the started PTY
        pty_id: String,
    },
    /// PTY output
    Output {
        /// Output bytes (Base64)
        data_b64: String,
    },
    /// The PTY process exited
    Exit {
        /// Exit code
        exit_code: Option<i32>,
    },
    /// Error notification.
    ///
    /// `code` is not an `ErrorCode` but a string specific to the PTY channel
    /// (e.g. `"FORBIDDEN"` when remote PTY is disabled).
    Error {
        /// Error code string
        code: String,
        /// Human-readable message
        message: String,
    },
}

/// Helper converting a `CommandResult` into `ServerWsMessage::CommandResult`.
pub fn command_result_ws(result: CommandResult) -> ServerWsMessage {
    ServerWsMessage::CommandResult {
        command_id: result.command_id,
        success: result.success,
        error: result.error,
        session_id: result.session_id,
    }
}

/// Helper converting a `SessionEventBatch` into `ServerWsMessage::EventBatch`.
pub fn event_batch_ws(batch: SessionEventBatch) -> ServerWsMessage {
    ServerWsMessage::EventBatch {
        events: batch.events,
        cursor: batch.cursor,
    }
}

/// Query parameters of `GET /api/v1/nodes/:id/fs/browse`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct FsBrowseQuery {
    /// Directory path to browse (defaults to the home directory)
    pub path: Option<String>,
}

/// An entry inside the file system (a folder or a file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct FsEntry {
    /// Entry name (file name / folder name)
    pub name: String,
    /// Absolute path
    pub path: String,
    /// Whether it is a directory
    pub is_dir: bool,
    /// Hidden attribute (the name starts with '.', or the Windows Hidden attribute)
    pub is_hidden: bool,
}

/// `GET /api/v1/nodes/:id/fs/browse` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct FsBrowseResponse {
    /// Absolute path currently being browsed ("C:\\Users\\..." on Windows, "/home/..." on Unix)
    pub current_path: String,
    /// Path of the parent directory (None when there is no parent or this is the drive list)
    pub parent_path: Option<String>,
    /// Child entries
    pub entries: Vec<FsEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ErrorCode;

    #[test]
    fn client_ws_message_tags() {
        let msg = ClientWsMessage::Subscribe {
            since_cursor: Some(10),
            focused_session_id: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "subscribe");
        assert_eq!(json["since_cursor"], 10);
        let decoded: ClientWsMessage = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);

        let ping = ClientWsMessage::Ping;
        assert_eq!(serde_json::to_value(&ping).unwrap()["op"], "ping");
    }

    #[test]
    fn server_ws_error_shape() {
        let msg = ServerWsMessage::Error {
            error: ErrorInfo::new(ErrorCode::NodeOffline, "node is offline"),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "error");
        assert_eq!(json["error"]["code"], "NODE_OFFLINE");
        assert_eq!(json["error"]["message"], "node is offline");
    }

    #[test]
    fn api_error_response_shape() {
        let resp = ApiErrorResponse {
            error: ErrorInfo::new(ErrorCode::AlreadyResolved, "already resolved"),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], "ALREADY_RESOLVED");
        assert_eq!(json["error"]["message"], "already resolved");
    }

    #[test]
    fn pty_messages_roundtrip() {
        let client = PtyClientMessage::Spawn {
            session_id: "s1".into(),
            cols: 80,
            rows: 24,
            shell_cmd: None,
        };
        let json = serde_json::to_value(&client).unwrap();
        assert_eq!(json["op"], "spawn");
        assert_eq!(
            serde_json::from_value::<PtyClientMessage>(json).unwrap(),
            client
        );

        let server = PtyServerMessage::Error {
            code: "FORBIDDEN".into(),
            message: "Remote PTY is disabled on this node by security policy".into(),
        };
        let json = serde_json::to_value(&server).unwrap();
        assert_eq!(json["op"], "error");
        assert_eq!(json["code"], "FORBIDDEN");
    }

    #[test]
    fn create_session_request_serialization() {
        let req = CreateSessionRequest {
            command_id: "c1".into(),
            project_id: "github.com/nazo6/flexagent".into(),
            node_id: Some("home-win".into()),
            provisioner: None,
            local_path: None,
            worktree: Some(WorktreeSpec {
                branch: "feat/auth".into(),
                base_branch: Some("main".into()),
                new_path: None,
            }),
            agent_id: "opencode".into(),
            initial_prompt: Some("Fix the failing tests".into()),
            mode: Some("code".into()),
            extra_args: Some(vec!["--verbose".into()]),
            fork: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["worktree"]["branch"], "feat/auth");
        assert!(json["provisioner"].is_null());
        let decoded: CreateSessionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, req);
    }
}
