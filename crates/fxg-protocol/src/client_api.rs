//! API type definitions for Client (PWA / Browser / CLI) <-> Server (or Local Node).
//!
//! The hub (`fxg hub start`, `:8080`) and the node's local web server
//! (`fxg node start`, `127.0.0.1:7860`) implement exactly the same API paths and
//! WebSocket format (design: `docs/03-protocol-and-api.md` §3). The UI connects
//! to either of them with the same types and the same code, without caring about
//! the endpoint URL.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::common::{
    AuditLogEntry, ClientSource, CommandResult, ConfigOptionInfo, DiffScope, ElicitationAction,
    ElicitationContent, ElicitationRequestEntry, ErrorInfo, IncidentEntry, ModeInfo, NodeSummary,
    PermissionRequestEntry, ProjectSummary, PromptInput, ResolvedBy, SearchHit,
    SessionControlAction, SessionSummary, StreamDeltaPayload, SystemStatus,
};
use crate::events::{SessionEventBatch, SessionEventEnvelope};
use crate::settings::{CustomAgentConfig, ProjectIdentity};
use crate::{CommandId, NodeId, ProjectId, SecretId, SessionId};

/// Kind of endpoint (`GET /api/v1/system/info`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum ConnectionRole {
    /// Connected to the hub (`fxg hub start`)
    Hub,
    /// Connected to the node's local web server (`fxg node start`)
    LocalNode,
}

/// State of the node ⇔ hub link (`GET /api/v1/system/info`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum HubLinkState {
    /// No pairing (the node runs standalone / local only)
    Standalone,
    /// A pairing exists and a connection attempt is in progress
    Connecting,
    /// Connected and syncing with the hub
    Connected,
    /// The hub rejected the pairing token (HTTP 401 / 403)
    Unauthorized,
    /// The hub could not be reached or the connection failed
    Failed,
}

/// Snapshot of the node ⇔ hub link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct HubLinkStatus {
    /// Current state
    pub state: HubLinkState,
    /// Hub address (HTTP origin) when a pairing exists
    pub hub_url: Option<String>,
    /// When the current state was entered (Unix epoch ms)
    pub since: i64,
    /// English detail when `state` is `unauthorized` / `failed`
    pub message: Option<String>,
}

impl HubLinkStatus {
    /// Standalone state (no pairing configured).
    pub fn standalone() -> Self {
        Self::new(HubLinkState::Standalone, None, None)
    }

    /// Builds a status and stamps the current time.
    pub fn new(state: HubLinkState, hub_url: Option<String>, message: Option<String>) -> Self {
        Self {
            state,
            hub_url,
            since: crate::util::now_ms(),
            message,
        }
    }

    /// Whether the node is connected to the hub.
    pub fn is_connected(&self) -> bool {
        self.state == HubLinkState::Connected
    }
}

/// `GET /api/v1/system/info` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SystemInfoResponse {
    /// Kind of endpoint
    pub role: ConnectionRole,
    /// Node ID when connected to a local node (`None` on the hub)
    pub node_id: Option<NodeId>,
    /// fxg binary version
    pub version: String,
    /// VAPID public key for Web Push
    pub vapid_public_key: Option<String>,
    /// Number of unsynced outbox events (paired local node only; `None` while standalone)
    pub unsynced_event_count: Option<u64>,
    /// Node ⇔ hub link state (local node only)
    pub hub_link: Option<HubLinkStatus>,
    /// Last successful synchronization time (Unix epoch ms; local node only)
    pub last_synced_at: Option<i64>,
}

/// Recent stderr lines for one agent (diagnostics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentDiagnostics {
    /// Agent ID
    pub agent_id: String,
    /// Recent stderr lines (oldest first, bounded)
    pub stderr_tail: Vec<String>,
}

/// `GET /api/v1/diagnostics` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct DiagnosticsResponse {
    /// Endpoint identifier (`local-node` / `central-server`)
    pub node_id: String,
    /// Generation time (Unix epoch ms)
    pub generated_at: i64,
    /// Per-agent stderr tails
    pub agents: Vec<AgentDiagnostics>,
    /// Daemon log path (local node only)
    pub node_log: Option<String>,
    /// Component health and delivery counters
    pub health: SystemStatus,
    /// Recent persisted incidents (newest first)
    pub incidents: Vec<IncidentEntry>,
}

/// Query of `GET /api/v1/diagnostics/incidents`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct IncidentQuery {
    /// Maximum number of entries to return (default 50)
    pub limit: Option<u32>,
    /// Include incidents that were marked resolved
    pub include_resolved: Option<bool>,
}

/// Redacted bootstrap configuration included in a support bundle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SupportBundleConfig {
    /// Data directory after replacing the user's home directory with `~`
    pub data_dir: String,
    /// Log level
    pub log_level: Option<String>,
    /// Log file destination
    pub log_file: Option<String>,
    /// Log format
    pub log_format: Option<String>,
    /// Local node bind address
    pub node_listen_addr: Option<String>,
    /// Hub URL
    pub hub_url: Option<String>,
    /// Hub bind address
    pub hub_listen_addr: Option<String>,
    /// Extra allowed hosts (the owner's `allowed_hosts`)
    pub allowed_hosts: Vec<String>,
    /// Extra allowed origins (the owner's `allowed_origins`)
    pub allowed_origins: Vec<String>,
}

/// Database statistics included in a support bundle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct DatabaseStats {
    /// Database file path after home-directory redaction
    pub path: String,
    /// Database file size in bytes
    pub size_bytes: u64,
    /// Number of sessions
    pub sessions: u64,
    /// Number of persisted events
    pub events: u64,
    /// Number of incidents
    pub incidents: u64,
    /// Events not yet acknowledged by the hub (nodes only)
    pub pending_outbox_events: u64,
}

/// `GET /api/v1/diagnostics/bundle` response (`fxg support bundle`).
///
/// Everything text-based is redacted: tokens, cookies, environment values,
/// prompts and the user's home directory never leave the process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SupportBundle {
    /// Generation time (Unix epoch ms)
    pub generated_at: i64,
    /// fxg binary version
    pub version: String,
    /// Kind of endpoint
    pub role: ConnectionRole,
    /// Redacted bootstrap configuration
    pub config: SupportBundleConfig,
    /// Diagnostics (health, incidents, agent stderr tails)
    pub diagnostics: DiagnosticsResponse,
    /// Database statistics
    pub database: DatabaseStats,
    /// Recent sessions with their event counts
    pub sessions: Vec<crate::common::SupportBundleSession>,
    /// Tail of the process log (redacted, oldest first)
    pub logs: Vec<String>,
}

/// `GET /api/v1/meta` response (endpoint metadata that needs no authentication).
///
/// `GET /api/v1/system/info` requires authentication, so an unauthenticated Web UI
/// determines the endpoint kind from this endpoint and switches the token dialog
/// correctly (hub / local node) (design: `docs/03` §3.1).
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
    /// Command to execute
    pub command: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Seconds until the idle instance is discarded automatically
    pub idle_timeout_secs: Option<u64>,
    /// Environment variables injected into the process
    pub env: std::collections::BTreeMap<String, String>,
    /// Environment variables resolved from the secret store at launch time
    pub env_secrets: std::collections::BTreeMap<String, SecretId>,
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
/// `fxg internal stdio` (or reports a failure during provisioning).
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
    /// Directory to scan (walks `node_settings.project_scan_dirs` when omitted)
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
    pub project_id: ProjectId,
    /// Target directory
    pub local_path: String,
}

/// `POST /api/v1/nodes/:node_id/projects/inspect` / `.../add` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectPreviewRequest {
    /// Target directory
    pub local_path: String,
}

/// Automatically resolved project identity for a folder.
///
/// Used before adding a project (preview) and after registering it (result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectPreview {
    /// Logical project ID after resolution
    pub project_id: ProjectId,
    /// Display name derived from the identity
    pub name: String,
    /// Git URL the canonical form was derived from
    pub canonical_git_url: Option<String>,
    /// How the identity was derived
    pub identity: ProjectIdentity,
    /// Canonical directory path
    pub local_path: String,
    /// Whether the directory is a Git repository
    pub is_git_repo: bool,
    /// Whether the directory is a Git worktree
    pub is_worktree: bool,
    /// Monorepo subpath relative to the Git root (if any)
    pub relative_subpath: Option<String>,
}

/// `POST /api/v1/nodes/:node_id/projects/link` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectLinkResponse {
    /// The resolved logical project ID
    pub project_id: ProjectId,
    /// Target directory (the local path after resolution)
    pub local_path: String,
}

/// `POST /api/v1/nodes/:node_id/projects/unlink` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectUnlinkRequest {
    /// Directory whose binding on the node should be removed
    pub local_path: String,
}

/// `POST /api/v1/nodes/:node_id/projects/unlink` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectUnlinkResponse {
    /// Directory whose binding was removed
    pub local_path: String,
}

/// `GET /api/v1/sessions` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionListResponse {
    /// Sessions (newest update first)
    pub sessions: Vec<SessionSummary>,
}

/// `GET /api/v1/agents` response (available agents plus import candidates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentsResponse {
    /// Built-in and custom agents available on this node
    pub agents: Vec<AgentSummary>,
    /// ACP Registry entries that can be imported as custom agents
    pub registry: Vec<RegistryAgentSummary>,
}

/// `POST /api/v1/nodes/{node_id}/agents/{agent_id}/probe` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProbeAgentRequest {
    /// Working directory used to create the temporary ACP session
    pub local_path: String,
    /// Bypass the node's probe cache (used by the UI's "Refresh")
    #[serde(default)]
    pub force: bool,
    /// Configuration applied to the temporary session before capabilities are read.
    ///
    /// Some agents only report dependent options (for example the thought level)
    /// after a model is selected.
    #[serde(default)]
    pub initial_config: Vec<crate::common::InitialConfig>,
}

/// `POST /api/v1/nodes/{node_id}/agents/registry/{agent_id}/import` request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
#[serde(default)]
pub struct ImportRegistryAgentRequest {
    /// Built-in extension ID to apply to the imported custom agent.
    pub extension: Option<String>,
}

/// Capabilities returned by starting a temporary ACP session.
///
/// ACP only exposes the selectable models (`configOptions`) in the
/// `session/new` response, so the node probes once and caches the result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentCapabilitiesPreview {
    /// Available agent modes
    #[serde(default)]
    pub available_modes: Vec<ModeInfo>,
    /// Current mode (agent default)
    #[serde(default)]
    pub current_mode: Option<String>,
    /// Configuration entries (model selection, thinking level, ...)
    #[serde(default)]
    pub config_options: Vec<ConfigOptionInfo>,
}

/// How an agent is launched (the means actually usable on this node).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum AgentSource {
    /// An installation already on PATH (detected with `which`)
    External,
    /// A custom ACP agent defined in `node.db` (including registry imports)
    Custom,
}

/// How the agent runtime is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum AgentBackendKind {
    /// ACP only (`AcpDriver`)
    Acp,
    /// ACP plus fxg-side extensions
    AcpExtended,
    /// A fully custom `AgentDriver`
    Custom,
}

/// A single ACP agent available on this node (built-in or custom).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentSummary {
    /// Agent ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Version (`-` for built-in agents, registry version for imports)
    pub version: String,
    /// Description
    pub description: Option<String>,
    /// Launch forms usable on this node (empty = not installed)
    pub sources: Vec<AgentSource>,
    /// Executable detected on PATH (only when external)
    pub external_path: Option<String>,
    /// Contents of the custom definition (for editing in the UI)
    pub custom_config: Option<CustomAgentConfig>,
    /// Built-in extension ID applied to this agent (`None` = ACP only)
    pub extension: Option<String>,
    /// Effective ACP client capabilities advertised on connections
    pub capabilities: crate::settings::AgentAcpCapabilities,
    /// How the agent runtime is implemented
    pub backend: AgentBackendKind,
    /// Icon URL or built-in asset key
    pub icon: Option<String>,
    /// Nodes where it is available (based on the NodeHello reports)
    pub installed_nodes: Vec<String>,
}

/// How a registry agent can be distributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum RegistryDistributionKind {
    /// A platform-specific binary archive
    Binary,
    /// An npm package executed with `npx`
    Npx,
    /// A Python package executed with `uvx`
    Uvx,
}

/// One ACP Registry entry shown in the "import as custom agent" dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct RegistryAgentSummary {
    /// Registry agent ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Registry version
    pub version: String,
    /// Description
    pub description: Option<String>,
    /// Icon URL
    pub icon: Option<String>,
    /// Available distribution forms on this node
    pub distribution: Vec<RegistryDistributionKind>,
    /// Whether this node can build a launch command from the registry entry
    pub available: bool,
    /// Why `available` is false (missing CLI / package runner)
    pub unavailable_reason: Option<String>,
    /// Whether a custom agent with the same ID already exists
    pub imported: bool,
}

/// Common response of agent management operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AgentOpResponse {
    /// Message describing the outcome (e.g. `imported qwen-code from the registry`)
    pub message: Option<String>,
}

/// One entry of `GET /api/v1/nodes/tokens` (an issued node pairing token).
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
    /// Issued node pairing tokens
    pub tokens: Vec<NodeTokenSummary>,
}

/// `POST /api/v1/nodes/tokens` request (issue a new node pairing token).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct IssueNodeTokenRequest {
    /// Node ID to pair (the node_id of the fxg node start)
    pub node_id: NodeId,
}

/// `POST /api/v1/nodes/tokens` response.
///
/// The plaintext token is returned only once, at issue time (the hub stores
/// only its SHA-256 hash).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct IssueNodeTokenResponse {
    /// Target node ID
    pub node_id: NodeId,
    /// The issued plaintext token (stored in `<data>/node/node_token`)
    pub token: String,
}

/// `POST /api/v1/system/shutdown` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ShutdownResponse {
    /// Whether the graceful shutdown was requested
    pub ok: bool,
}

/// `POST /api/v1/auth/rotate-token` response (regenerate the Web access token).
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
}

/// Specification of a context fork.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionForkSpec {
    /// Source session ID
    pub from_session_id: SessionId,
    /// Source `node_seq` to fork at. `None` means the current end of the source
    /// session (the only boundary supported by ACP-native forks today).
    pub up_to_node_seq: Option<u64>,
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
    pub project_id: ProjectId,
    /// ID of the resident node to run on (mutually exclusive with `provisioner`)
    pub node_id: Option<NodeId>,
    /// Ephemeral VM provisioner name (mutually exclusive with `node_id`; requires the hub)
    pub provisioner: Option<String>,
    /// Working directory (an existing worktree or the repository root)
    pub local_path: Option<String>,
    /// Specification used when creating and starting in a new worktree
    pub worktree: Option<WorktreeSpec>,
    /// Agent ID (`opencode` / a custom ID, etc.)
    pub agent_id: String,
    /// Initial prompt (text and optional attachments)
    pub initial_prompt: Option<PromptInput>,
    /// Initial session configuration (model / thinking level, etc.)
    #[serde(default)]
    pub initial_config: Vec<crate::common::InitialConfig>,
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

/// `GET /api/v1/sessions/:id` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionDetail {
    /// The session aggregate
    pub session: SessionSummary,
    /// Total number of persisted events
    pub event_count: u64,
    /// Recent events (oldest first)
    pub recent_events: Vec<SessionEventEnvelope>,
    /// Permission requests still pending for this session
    pub pending_permissions: Vec<PermissionRequestEntry>,
    /// Elicitation requests still waiting for an answer for this session
    pub pending_elicitations: Vec<ElicitationRequestEntry>,
}

/// `POST /api/v1/sessions/:id/prompt` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SendPromptRequest {
    /// Correlation ID assigned by the client
    pub command_id: CommandId,
    /// Prompt body and attachments
    pub input: PromptInput,
    /// Source (`cli` / `web` / `android`)
    pub client_source: ClientSource,
}

/// `POST /api/v1/sessions/:id/control` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ControlSessionRequest {
    /// Correlation ID assigned by the client
    pub command_id: CommandId,
    /// Operation to execute
    pub action: SessionControlAction,
}

/// One session that exists on the agent side and can be imported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ExternalSessionSummary {
    /// Agent that owns the session (`opencode` / a custom ID, etc.)
    pub agent_id: String,
    /// Agent-side session ID (`session/list` `sessionId`)
    pub agent_session_id: String,
    /// Working directory recorded by the agent
    pub cwd: String,
    /// Human-readable title reported by the agent
    pub title: Option<String>,
    /// ISO 8601 timestamp of the last activity reported by the agent
    pub updated_at: Option<String>,
    /// fxg session that already owns this agent session (`None` = not imported)
    pub imported_session_id: Option<SessionId>,
}

/// Discovery state for one agent (capabilities and listing failure).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ExternalSessionAgentInfo {
    /// Agent ID
    pub agent_id: String,
    /// Display name
    pub name: String,
    /// Whether the agent advertises ACP `session/list`
    pub supports_list: bool,
    /// Whether the agent can replay history (`loadSession`) on import
    pub supports_history: bool,
    /// Failure while inspecting or listing this agent (`None` = success)
    pub error: Option<ErrorInfo>,
}

/// `GET /api/v1/sessions/external` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ExternalSessionListResponse {
    /// Agent sessions that are not managed by fxg yet
    pub sessions: Vec<ExternalSessionSummary>,
    /// Per-agent discovery results (including unsupported / failed agents)
    pub agents: Vec<ExternalSessionAgentInfo>,
}

/// `POST /api/v1/sessions/import` request.
/// A local node derives the target from `cwd` and ignores `node_id` (or
/// validates it when present). The hub requires `node_id` and relays the import
/// to that node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionImportRequest {
    /// Correlation ID assigned by the client
    pub command_id: CommandId,
    /// Node that owns the agent session (hub only; omit for a local node)
    pub node_id: Option<NodeId>,
    /// Agent ID (`opencode` / a custom ID, etc.)
    pub agent_id: String,
    /// Agent-side session ID to take over
    pub agent_session_id: String,
    /// Working directory
    pub cwd: String,
    /// Optional title
    pub title: Option<String>,
    /// Capture the history replayed by ACP `session/load` into the fxg event log.
    ///
    /// Defaults to `false` so existing CLI / API callers keep the metadata-only
    /// import behavior.
    #[serde(default)]
    pub mirror_history: bool,
}

/// `POST /api/v1/sessions/import` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionImportResponse {
    /// ID of the imported session
    pub session_id: SessionId,
    /// Correlation ID from the request
    pub command_id: CommandId,
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

/// `POST /api/v1/sessions/:id/resume` request.
///
/// Resumes a stopped session with the same `session_id`
/// (design: `docs/04-agent-drivers-and-windows.md` §4.1).
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

/// `POST /api/v1/sessions/:id/title` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionRenameRequest {
    /// New display title
    pub title: String,
}

/// `POST /api/v1/sessions/:id/title` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionRenameResponse {
    /// Target session ID
    pub session_id: SessionId,
    /// Title after the rename
    pub title: String,
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

/// Client -> Endpoint (`/api/v1/client/ws`) message.
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
    /// Sends a prompt (including slash commands and attachments).
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

/// Endpoint -> Client (`/api/v1/client/ws`) message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "op", rename_all = "snake_case")]
#[specta(collect)]
pub enum HubWsMessage {
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
    /// A progress log line of ephemeral VM provisioning (`stderr`) (ephemeral).
    ///
    /// Provisioning happens before the ephemeral node's `node.db` exists, so it is
    /// streamed directly from the hub instead of being persisted in the
    /// event log.
    #[serde(alias = "bootstrap_log")]
    ProvisioningLog {
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
        /// Node ⇔ hub link state (when connected to a local node)
        hub_link: HubLinkStatus,
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
    /// Project list or node bindings changed (triggers a `GET /api/v1/projects` refresh).
    ProjectsUpdated,
}

/// Client -> Endpoint (`/api/v1/pty/ws`) message.
///
/// A bidirectional stream channel that connects the terminal of the Web UI / mobile
/// PWA directly to the ConPTY / Unix PTY on the node. Terminals are scoped to a
/// workspace (`node_id` + `path`): opening the same workspace again attaches to
/// the running shell instead of starting another one. When the node setting
/// `allow_remote_pty = false` is in effect, `open` from remote (through the
/// hub) is rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "op", rename_all = "snake_case")]
#[specta(collect)]
pub enum PtyClientMessage {
    /// Open the terminal of a workspace (idempotent: attaches when it is running).
    Open {
        /// Node that owns the workspace
        node_id: NodeId,
        /// Workspace directory (canonical path of a registered project binding)
        path: String,
        /// Session that opened the terminal (origin, not ownership)
        session_id: Option<SessionId>,
        /// Number of columns
        cols: u16,
        /// Number of rows
        rows: u16,
        /// Replace a running terminal with a new shell
        new: bool,
    },
    /// Attach to an existing PTY by ID
    Attach {
        /// PTY ID
        pty_id: String,
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

/// Endpoint -> Client (`/api/v1/pty/ws`) message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "op", rename_all = "snake_case")]
#[specta(collect)]
pub enum PtyServerMessage {
    /// The PTY was opened (subsequent output belongs to the attached PTY ID)
    Opened {
        /// PTY ID
        pty_id: String,
        /// Canonical workspace path
        path: String,
        /// Whether an existing terminal was reused instead of starting a new one
        attached: bool,
        /// Number of columns
        cols: u16,
        /// Number of rows
        rows: u16,
    },
    /// PTY output (a sequence of retained output may precede the live stream)
    Output {
        /// Output bytes (Base64)
        data_b64: String,
        /// Sequence number of this chunk (0-based, monotonic per PTY)
        seq: u64,
    },
    /// The PTY process exited
    Exit {
        /// Exit code
        exit_code: Option<i32>,
    },
    /// Error notification.
    ///
    /// `code` is not an `ErrorCode` but a string specific to the PTY channel
    /// (e.g. `"PTY_DISABLED"` when remote PTY is disabled).
    Error {
        /// Error code string
        code: String,
        /// Human-readable message
        message: String,
    },
}

/// One running terminal (`GET /api/v1/terminals`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct TerminalSummary {
    /// PTY ID
    pub pty_id: String,
    /// Node that runs the terminal
    pub node_id: NodeId,
    /// Canonical workspace path
    pub path: String,
    /// Session that opened the terminal (origin; may be gone)
    pub session_id: Option<SessionId>,
    /// OS process ID of the shell
    pub pid: Option<u32>,
    /// Current columns
    pub cols: u16,
    /// Current rows
    pub rows: u16,
    /// Number of attached clients
    pub attached_clients: u32,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
}

/// Running terminals (`GET /api/v1/terminals`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct TerminalsResponse {
    /// Running terminals on the node
    pub terminals: Vec<TerminalSummary>,
}

/// Query parameters of `GET /api/v1/terminals`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct TerminalsQuery {
    /// Node whose running terminals are listed
    pub node_id: NodeId,
}

/// Helper converting a `CommandResult` into `HubWsMessage::CommandResult`.
pub fn command_result_ws(result: CommandResult) -> HubWsMessage {
    HubWsMessage::CommandResult {
        command_id: result.command_id,
        success: result.success,
        error: result.error,
        session_id: result.session_id,
    }
}

/// Helper converting a `SessionEventBatch` into `HubWsMessage::EventBatch`.
pub fn event_batch_ws(batch: SessionEventBatch) -> HubWsMessage {
    HubWsMessage::EventBatch {
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

/// Query of `GET /api/v1/nodes/:id/workspace/fs/list` and
/// `GET /api/v1/nodes/:id/workspace/fs/file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceFsQuery {
    /// Directory or file path inside a registered workspace
    pub path: String,
}

/// Kind of a file system entry inside a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum WorkspaceEntryKind {
    /// Regular file
    File,
    /// Directory
    Directory,
    /// Symbolic link (the link itself, not the target)
    Symlink,
    /// Other entry (socket, FIFO, device)
    Other,
}

/// One entry of a workspace directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceFsEntry {
    /// Entry name (basename)
    pub name: String,
    /// Absolute path of the entry
    pub path: String,
    /// Entry kind
    pub kind: WorkspaceEntryKind,
    /// Whether the entry is hidden (dot-prefixed, or the Windows Hidden attribute)
    pub is_hidden: bool,
    /// Size in bytes (0 for directories when unavailable)
    pub size: u64,
    /// Last modification time (Unix epoch ms)
    pub modified_at: Option<i64>,
}

/// `GET /api/v1/nodes/:id/workspace/fs/list` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceFsListResponse {
    /// Access root of the workspace (the Git toplevel when the workspace is in a repository)
    pub root_path: String,
    /// Absolute path of the listed directory
    pub path: String,
    /// Parent directory inside the access root (`None` at the root)
    pub parent_path: Option<String>,
    /// Child entries, directories first
    pub entries: Vec<WorkspaceFsEntry>,
    /// Whether the listing stopped at the entry limit
    pub truncated: bool,
}

/// Content of a workspace file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[specta(collect)]
pub enum WorkspaceFileContent {
    /// UTF-8 text
    Text {
        /// File text
        text: String,
        /// Whether the text was truncated at the read limit
        truncated: bool,
        /// SHA-256 hex digest of the file bytes (used by optimistic writes)
        content_hash: String,
    },
    /// Binary file (NUL bytes or invalid UTF-8)
    Binary,
    /// File larger than the read limit
    TooLarge {
        /// Size in bytes
        size: u64,
    },
}

/// `GET /api/v1/nodes/:id/workspace/fs/file` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceFileView {
    /// Absolute file path
    pub path: String,
    /// File size in bytes
    pub size: u64,
    /// Last modification time (Unix epoch ms)
    pub modified_at: Option<i64>,
    /// Text, binary or too large
    pub content: WorkspaceFileContent,
}

/// Status of a changed file in a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[specta(collect)]
pub enum WorkspaceGitFileStatus {
    /// Modified
    Modified,
    /// Added to the index / worktree
    Added,
    /// Deleted
    Deleted,
    /// Renamed
    Renamed,
    /// Copied
    Copied,
    /// Type changed (file / symlink)
    TypeChanged,
    /// Untracked
    Untracked,
    /// Unmerged / conflicted
    Conflicted,
    /// Unrecognized porcelain status (raw `XY` code kept as-is)
    Unknown {
        /// Raw porcelain `XY` code
        raw: String,
    },
}

/// One changed file of a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceGitEntry {
    /// Path relative to the access root
    pub path: String,
    /// Previous path for renames / copies (relative)
    pub old_path: Option<String>,
    /// Absolute path of the current file
    pub absolute_path: String,
    /// Display status
    pub status: WorkspaceGitFileStatus,
    /// Whether the index has changes for this path
    pub staged: bool,
    /// Whether the worktree has changes for this path
    pub unstaged: bool,
}

/// Query of `GET /api/v1/nodes/:id/workspace/git/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceGitStatusQuery {
    /// Directory inside a registered workspace
    pub path: String,
}

/// `GET /api/v1/nodes/:id/workspace/git/status` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceGitStatusResponse {
    /// Access root of the workspace
    pub root_path: String,
    /// Whether the workspace is inside a Git work tree
    pub is_repository: bool,
    /// Current branch (`None` when detached or not a repository)
    pub branch: Option<String>,
    /// Commits ahead of the upstream
    pub ahead: u32,
    /// Commits behind the upstream
    pub behind: u32,
    /// Changed entries, capped at the status limit
    pub entries: Vec<WorkspaceGitEntry>,
    /// Whether the entry list stopped at the limit
    pub truncated: bool,
}

/// Query of `GET /api/v1/nodes/:id/workspace/git/diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceGitDiffQuery {
    /// Directory inside a registered workspace
    pub path: String,
    /// Diff scope (defaults to uncommitted)
    pub scope: Option<DiffScope>,
    /// Base branch for `BranchBase`
    pub base: Option<String>,
    /// Absolute file path inside the workspace (omit for the whole workspace)
    pub file: Option<String>,
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
        let msg = HubWsMessage::Error {
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
        let client = PtyClientMessage::Open {
            node_id: "home-win".into(),
            path: "/home/u/ghq/github.com/nazo6/flexagent".into(),
            session_id: Some("s1".into()),
            cols: 80,
            rows: 24,
            new: false,
        };
        let json = serde_json::to_value(&client).unwrap();
        assert_eq!(json["op"], "open");
        assert_eq!(
            serde_json::from_value::<PtyClientMessage>(json).unwrap(),
            client
        );

        let server = PtyServerMessage::Opened {
            pty_id: "p1".into(),
            path: "/home/u/ghq/github.com/nazo6/flexagent".into(),
            attached: true,
            cols: 80,
            rows: 24,
        };
        let json = serde_json::to_value(&server).unwrap();
        assert_eq!(json["op"], "opened");
        assert_eq!(json["attached"], true);

        let output = PtyServerMessage::Output {
            data_b64: "aGk=".into(),
            seq: 7,
        };
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["seq"], 7);
        assert_eq!(
            serde_json::from_value::<PtyServerMessage>(json).unwrap(),
            output
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
            }),
            agent_id: "opencode".into(),
            initial_prompt: Some(PromptInput::text("Fix the failing tests")),
            initial_config: Vec::new(),
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
