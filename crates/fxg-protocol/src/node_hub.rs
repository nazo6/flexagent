//! Node <-> Hub protocol (shared by WebSocket and stdio pipe).
//!
//! A resident node (outbound WebSocket) and an ephemeral VM node (JSON Lines of
//! `fxg internal stdio`) differ only in the transport layer and share **exactly the
//! same message types** (design: `docs/03-protocol-and-api.md` §2).
//!
//! This is the internal protocol of the hub implementation and is not exposed
//! to the UI, so it is excluded from `specta` export.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::client_api::HubLinkStatus;
use crate::client_api::{
    AgentCapabilitiesPreview, AgentsResponse, FsBrowseResponse, ProjectLinkResponse,
    ProjectPreview, ProjectScanResponse, ProjectUnlinkResponse, TerminalsResponse,
    WorkspaceFileView, WorkspaceFsListResponse, WorkspaceGitStatusResponse, WorktreeInfo,
};
use crate::common::{
    AgentAction, ClientSource, DataSafety, DiffScope, ElicitationContent, ErrorInfo,
    NodeProjectReport, PromptInput, ResolvedBy, SessionControlAction, WarningInfo,
    WorkspaceDiffResponse, WorktreeAction,
};
use crate::events::SessionEventEnvelope;
use crate::settings::{
    NodeSettingsSnapshot, SecretSummary, TestPairingRequest, UpdateAgentSettingsRequest,
    UpdateNodeSettingsRequest, UpdatePairingRequest, UpsertSecretRequest,
};
use crate::{CommandId, NodeId, ProjectId, SecretId, SessionId};

/// Version of the node <-> server message set.
///
/// The node and the hub must come from the same `fxg` release. A node
/// with an older protocol is rejected at `NodeHello`; backward compatibility
/// shims are intentionally not provided.
pub const PROTOCOL_VERSION: u32 = 4;
/// Synchronization state of a session held by the node (reported in `NodeHello`).
///
/// The hub compares it with `last_node_seq` in its own DB and returns
/// [`HubToNodeMsg::ResyncRequest`] when something is missing or lagging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSyncState {
    /// Target session ID
    pub session_id: SessionId,
    /// Current logical project ID (follows `project_key` changes)
    pub project_id: ProjectId,
    /// Latest `node_seq` persisted on the node
    pub last_node_seq: u64,
}

/// Resend specification for a session the hub is missing or lagging on (`ResyncRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncTarget {
    /// Target session ID
    pub session_id: SessionId,
    /// Resend events at or after (`>=`) this value regardless of the watermark
    /// (`1` means a full resend including `SessionCreated`)
    pub from_node_seq: u64,
}

/// Generic node operation requested by the hub.
///
/// The request is correlated by the `request_id` carried in
/// [`HubToNodeMsg::NodeRpc`]. Both success and failure are returned through
/// [`NodeRpcOutcome`], so errors keep their `ErrorCode` / `RecoveryAction`
/// instead of being flattened to a string at the hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeRpcRequest {
    /// Fetch the git diff of a session workspace.
    GetGitDiff {
        /// Target session ID
        session_id: SessionId,
        /// Diff scope (uncommitted / cumulative against the base branch)
        scope: DiffScope,
        /// Base branch used for `BranchBase`
        base_branch: Option<String>,
    },
    /// Create, remove or prune a worktree.
    ManageWorktree {
        /// Target logical project ID
        project_id: ProjectId,
        /// Operation to execute
        action: WorktreeAction,
    },
    /// Browse the file system of the node.
    BrowseFs {
        /// Path to browse (defaults to the home directory)
        path: Option<String>,
    },
    /// Scan and register the git repositories under a directory.
    ProjectScan {
        /// Directory to scan (defaults to `node.project_scan_dirs`)
        dir: Option<String>,
    },
    /// Link an arbitrary directory to a logical project.
    ProjectLink {
        /// Logical project ID to link to
        project_id: ProjectId,
        /// Target directory
        local_path: String,
    },
    /// Resolve a directory to a logical project without writing to the DB.
    InspectProject {
        /// Target directory
        local_path: String,
    },
    /// Resolve a directory and register the project binding.
    AddProject {
        /// Target directory
        local_path: String,
    },
    /// Remove this node's binding for a directory (orphan projects are pruned).
    UnlinkProject {
        /// Target directory
        local_path: String,
    },
    /// Fetch the ACP Registry catalog plus installation state.
    ListAgents,
    /// List the running workspace terminals of the node.
    ListTerminals,
    /// Probe an agent by starting a temporary ACP session.
    ///
    /// ACP only reports model choices in the `session/new` response, so this
    /// starts and immediately closes a session without creating an FXG session.
    ProbeAgentConfig {
        /// Agent ID
        agent_id: String,
        /// Working directory used for the temporary ACP session
        local_path: String,
        /// Bypass the node's probe cache
        #[serde(default)]
        force: bool,
        /// Configuration applied before capabilities are read
        #[serde(default)]
        initial_config: Vec<crate::common::InitialConfig>,
    },
    /// Fetch the node settings snapshot.
    GetNodeSettings,
    /// Update node-owned settings.
    UpdateNodeSettings {
        /// New values and the observed revision
        request: UpdateNodeSettingsRequest,
    },
    /// Update agent settings.
    UpdateAgentSettings {
        /// New values and the observed revision
        request: UpdateAgentSettingsRequest,
    },
    /// Pair the node with a hub (or change the address / token).
    UpdatePairing {
        /// Hub address, optional token and the observed revision
        request: UpdatePairingRequest,
    },
    /// Remove the hub pairing (the outbox is kept).
    ClearPairing {
        /// Revision the client observed (`REVISION_MISMATCH` on conflict)
        expected_revision: u64,
    },
    /// Try a hub address / token without saving it.
    TestPairing {
        /// Hub address and optional token to try
        request: TestPairingRequest,
    },
    /// Fetch the node's secret metadata (values are never returned).
    GetSecrets,
    /// Store a secret value (write-only).
    UpsertSecret {
        /// Target secret ID
        id: SecretId,
        /// Name, description and value
        request: UpsertSecretRequest,
    },
    /// Delete a secret.
    DeleteSecret {
        /// Target secret ID
        id: SecretId,
    },
    /// Manage the agent catalog (refresh / custom definitions / registry import).
    ManageAgent {
        /// Operation to execute
        action: AgentAction,
    },
    /// Import an existing agent session (native resume with a recorded ID).
    ImportSession {
        /// Agent ID
        agent_id: String,
        /// Agent-side session ID to take over
        agent_session_id: String,
        /// Working directory
        cwd: String,
        /// Optional title
        title: Option<String>,
        /// Mirror the history replayed by `session/load`
        #[serde(default)]
        mirror_history: bool,
    },
    /// List sessions that exist on the agent side (ACP `session/list`).
    ListExternalSessions {
        /// Agent IDs to inspect (empty = every available agent)
        #[serde(default)]
        agents: Vec<String>,
        /// Bypass the node's short-lived discovery cache
        #[serde(default)]
        force: bool,
    },
    /// Resume a stopped session.
    ResumeSession {
        /// Target session ID
        session_id: SessionId,
        /// Skip native restoration and continue with history replay
        force_replay: bool,
    },
    /// Rename a session.
    RenameSession {
        /// Target session ID
        session_id: SessionId,
        /// New title
        title: String,
    },
    /// Archive or restore a session.
    ArchiveSession {
        /// Target session ID
        session_id: SessionId,
        /// `true` = archive, `false` = restore
        archived: bool,
    },
    /// Delete a session and purge its event bodies.
    DeleteSession {
        /// Target session ID
        session_id: SessionId,
    },
    /// List a directory inside a registered workspace.
    WorkspaceFsList {
        /// Directory path
        path: String,
    },
    /// Read a file inside a registered workspace.
    WorkspaceFsFile {
        /// File path
        path: String,
    },
    /// Read the Git status of a registered workspace.
    WorkspaceGitStatus {
        /// Directory inside the workspace
        path: String,
    },
    /// Fetch the diff of a registered workspace, optionally for one file.
    WorkspaceGitDiff {
        /// Directory inside the workspace
        path: String,
        /// Diff scope
        scope: DiffScope,
        /// Base branch for `BranchBase`
        base_branch: Option<String>,
        /// Absolute file path (omit for the whole workspace)
        file: Option<String>,
    },
}

impl NodeRpcRequest {
    /// Whether the operation exposes workspace files and is therefore limited
    /// to a direct (non-hub) client connection.
    pub fn requires_local_workspace_access(&self) -> bool {
        matches!(
            self,
            Self::WorkspaceFsList { .. }
                | Self::WorkspaceFsFile { .. }
                | Self::WorkspaceGitStatus { .. }
                | Self::WorkspaceGitDiff { .. }
        )
    }
}

impl NodeRpcRequest {
    /// Time the hub waits for the node before failing the operation.
    ///
    /// Project scans, catalog refreshes, resumes and deletes can exceed the
    /// normal timeout because they traverse directories or cold-start an agent.
    pub fn timeout(&self) -> Duration {
        match self {
            Self::ProjectScan { .. }
            | Self::ListAgents
            | Self::ProbeAgentConfig { .. }
            | Self::ImportSession { .. }
            | Self::ListExternalSessions { .. }
            | Self::ResumeSession { .. }
            | Self::DeleteSession { .. } => Duration::from_secs(120),
            Self::ManageAgent { .. } => Duration::from_secs(600),
            // A pairing test performs a WebSocket handshake with the hub.
            Self::TestPairing { .. } => Duration::from_secs(15),
            _ => Duration::from_secs(30),
        }
    }
}

/// Successful payload of a [`NodeRpcRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeRpcResponse {
    /// Git diff payload (`GetGitDiff`).
    GitDiff {
        /// Diff of the workspace
        diff: WorkspaceDiffResponse,
    },
    /// Worktree payload (`ManageWorktree`; `None` when removed or pruned).
    Worktree {
        /// Worktree information on success
        worktree: Option<WorktreeInfo>,
    },
    /// Filesystem browse payload (`BrowseFs`).
    BrowseFs {
        /// Browse result
        response: FsBrowseResponse,
    },
    /// Workspace directory listing (`WorkspaceFsList`).
    WorkspaceFsList {
        /// Listing result
        response: WorkspaceFsListResponse,
    },
    /// Workspace file content (`WorkspaceFsFile`).
    WorkspaceFile {
        /// File content
        file: WorkspaceFileView,
    },
    /// Workspace Git status (`WorkspaceGitStatus`).
    WorkspaceGitStatus {
        /// Status result
        response: WorkspaceGitStatusResponse,
    },
    /// Workspace Git diff (`WorkspaceGitDiff`).
    WorkspaceGitDiff {
        /// Diff result
        diff: WorkspaceDiffResponse,
    },
    /// Project scan payload (`ProjectScan`).
    ProjectScan {
        /// Scan result
        response: ProjectScanResponse,
    },
    /// Project link payload (`ProjectLink`).
    ProjectLink {
        /// Link result
        response: ProjectLinkResponse,
    },
    /// Auto-resolved project preview (`InspectProject` / `AddProject`).
    ProjectPreview {
        /// Resolved identity and path
        preview: ProjectPreview,
    },
    /// Removed project binding (`UnlinkProject`).
    ProjectUnlinked {
        /// Removed binding
        response: ProjectUnlinkResponse,
    },
    /// ACP Registry catalog (`ListAgents`).
    Agents {
        /// Catalog plus installation state
        response: AgentsResponse,
    },
    /// Running terminals (`ListTerminals`).
    Terminals {
        /// Running terminals of the node
        response: TerminalsResponse,
    },
    /// Agent capabilities from a temporary ACP session (`ProbeAgentConfig`).
    AgentCapabilities {
        /// Modes and configuration entries reported by the agent
        preview: AgentCapabilitiesPreview,
    },
    /// Settings snapshot (`GetNodeSettings` / `UpdateNodeSettings` /
    /// `UpdateAgentSettings`).
    NodeSettings {
        /// Snapshot after the operation
        snapshot: Box<NodeSettingsSnapshot>,
    },
    /// Node ⇔ hub link state (`TestPairing`).
    HubLink {
        /// State after the test
        status: HubLinkStatus,
    },
    /// Secret metadata after the operation (`GetSecrets` / `UpsertSecret` /
    /// `DeleteSecret`; values are never included).
    Secrets {
        /// Secret metadata
        secrets: Vec<SecretSummary>,
    },
    /// Agent management result (`ManageAgent`).
    AgentOp {
        /// Display message on success (e.g. `imported qwen-code from the registry`)
        message: Option<String>,
    },
    /// Session import result (`ImportSession`).
    SessionImported {
        /// ID of the imported session
        session_id: SessionId,
    },
    /// External session discovery result (`ListExternalSessions`).
    ExternalSessions {
        /// Sessions and per-agent discovery state
        response: crate::client_api::ExternalSessionListResponse,
    },
    /// Session resume result (`ResumeSession`).
    Resume {
        /// Whether native restoration succeeded (`false` = history replay)
        context_restored: bool,
    },
    /// Session rename result (`RenameSession`).
    Renamed {
        /// Title after the rename
        title: String,
    },
    /// Session archive result (`ArchiveSession`).
    Archive {
        /// Archive time on success (Unix epoch ms). `None` when restored
        archived_at: Option<i64>,
    },
    /// Session delete result (`DeleteSession`).
    Delete,
}

/// Outcome of a [`NodeRpcRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NodeRpcOutcome {
    /// The operation succeeded.
    Ok {
        /// Typed response payload
        response: NodeRpcResponse,
    },
    /// The operation failed with a structured error.
    Err {
        /// Error code, message, recovery actions and data-safety hint
        error: ErrorInfo,
    },
}

/// Message sent from the node to the hub.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NodeToHubMsg {
    /// Handshake right after connecting
    /// (reports node information, project bindings and session sync states).
    NodeHello {
        /// Node ID (the hub verifies that it matches the token's target)
        node_id: NodeId,
        /// Display name
        name: String,
        /// OS (`windows` / `linux` / `macos`)
        os: String,
        /// Architecture (`x86_64` / `aarch64`)
        arch: String,
        /// fxg binary version
        version: String,
        /// Protocol version of the message set (missing on pre-version nodes = 0)
        #[serde(default)]
        protocol_version: u32,
        /// Whether this is an ephemeral VM / container node
        is_ephemeral: bool,
        /// Agent IDs available on this node
        installed_agents: Vec<String>,
        /// Project binding report
        projects: Vec<NodeProjectReport>,
        /// Whether `projects` is a complete snapshot (when `false` the hub must not prune)
        projects_complete: bool,
        /// Monotonically increasing revision of the node's reported state
        state_revision: u64,
        /// Session sync states
        sessions: Vec<SessionSyncState>,
        /// Whether `sessions` hit the node's reporting limit (more sessions exist)
        #[serde(default)]
        sessions_truncated: bool,
    },
    /// Sends a batch of persisted events (store-and-forward outbox).
    ///
    /// Session metadata (such as `SessionCreated`) is also sent as events on
    /// `session_events`. Normally the events after `synced_up_to_node_seq` are sent
    /// in ascending `node_seq` order.
    EventBatchPush {
        /// Events to send (batches are split per session)
        events: Vec<SessionEventEnvelope>,
        /// Whether this batch is a full replay requested by `ResyncRequest`
        /// (the hub suppresses Web Push for historical events)
        #[serde(default)]
        resync: bool,
    },
    /// Realtime ephemeral chunk (for fast rendering before the DB flush).
    LiveStreamDelta {
        /// Target session ID
        session_id: SessionId,
        /// The delta body
        delta: crate::common::StreamDeltaPayload,
    },
    /// Result of a command from the hub (correlated back to the requesting client).
    CommandResult {
        /// Correlation ID
        command_id: CommandId,
        /// Whether it succeeded
        success: bool,
        /// Structured error on failure
        error: Option<ErrorInfo>,
        /// Target session ID (optional)
        session_id: Option<SessionId>,
        /// Non-fatal issues (the command succeeded but part of the work failed)
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<WarningInfo>,
        /// What happened to the user's data (failures and destructive operations)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data_safety: Option<DataSafety>,
    },
    /// A PTY output data chunk (web terminal -> client).
    PtyOutput {
        /// PTY ID
        pty_id: String,
        /// Output bytes (Base64)
        data_b64: String,
    },
    /// Result of [`HubToNodeMsg::PtySpawn`].
    ///
    /// The node assigns or reuses the PTY ID and returns the retained output so
    /// the hub can seed its replay log before live chunks arrive.
    PtyOpened {
        /// Correlation ID of the spawn request
        request_id: CommandId,
        /// Actual PTY ID (an existing terminal is reused for the same workspace)
        pty_id: String,
        /// Canonical workspace path
        path: String,
        /// Number of columns
        cols: u16,
        /// Number of rows
        rows: u16,
        /// Whether an existing terminal was reused
        attached: bool,
        /// Retained output (Base64)
        snapshot_b64: String,
    },
    /// Failure of [`HubToNodeMsg::PtySpawn`] (before the PTY exists).
    PtyOpenFailed {
        /// Correlation ID of the spawn request
        request_id: CommandId,
        /// Proposed PTY ID
        pty_id: String,
        /// Error code string
        code: String,
        /// Human-readable message
        message: String,
    },
    /// Notification that the PTY process exited.
    PtyExit {
        /// PTY ID
        pty_id: String,
        /// Exit code
        exit_code: Option<i32>,
    },
    /// Failure notification for starting or operating a PTY (the error code is a
    /// string, e.g. `PTY_DISABLED`).
    ///
    /// When the node setting `allow_remote_pty = false` is in effect, a remote
    /// `PtySpawn` gets `PTY_DISABLED` (design: docs/01 §7.4).
    PtyError {
        /// PTY ID
        pty_id: String,
        /// Error code string
        code: String,
        /// Human-readable message
        message: String,
    },
    /// Notification that a graceful drain completed (after receiving it the central
    /// server discards the child process / VM).
    DrainComplete {
        /// Node ID
        node_id: NodeId,
    },
    /// Response to [`HubToNodeMsg::NodeRpc`].
    NodeRpcResult {
        /// Correlation ID
        request_id: CommandId,
        /// Typed success payload or structured error
        outcome: NodeRpcOutcome,
    },
}

/// Message sent from the hub to the node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HubToNodeMsg {
    /// Persistence-complete ACK for [`NodeToHubMsg::EventBatchPush`].
    ///
    /// Batches are sent split per session and the ACKs are returned per session
    /// too. After receiving one, the node advances `sessions.synced_up_to_node_seq`
    /// as its watermark.
    EventBatchAck {
        /// Target session ID
        session_id: SessionId,
        /// Every `node_seq` at or below this value has been persisted in the hub
        acked_up_to_node_seq: u64,
    },
    /// Response to `NodeHello`: asks for a resend of sessions the hub is missing or
    /// lagging on.
    ///
    /// The node resends events at or after the given `from_node_seq` regardless of
    /// the watermark.
    ResyncRequest {
        /// Sessions to resend
        sessions: Vec<ResyncTarget>,
    },
    /// Request from a client (web / Android) to start a new session.
    StartSession {
        /// Correlation ID
        command_id: CommandId,
        /// Session ID to start (assigned by the hub)
        session_id: SessionId,
        /// Logical project ID
        project_id: ProjectId,
        /// Working directory (including the worktree path)
        local_path: String,
        /// Agent ID
        agent_id: String,
        /// Initial prompt (text and optional attachments)
        initial_prompt: Option<PromptInput>,
        /// Initial session configuration (model / thinking level, etc.)
        #[serde(default)]
        initial_config: Vec<crate::common::InitialConfig>,
        /// Initial mode
        mode: Option<String>,
        /// Extra arguments passed to the agent
        extra_args: Option<Vec<String>>,
        /// Source session to fork natively (ACP `session/fork` + `session/load`).
        /// The fork must run on the node that owns the source session.
        fork_from_session_id: Option<SessionId>,
        /// Source `node_seq` to fork at. `None` means the current end.
        fork_up_to_node_seq: Option<u64>,
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
        action: crate::common::ElicitationAction,
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
    /// Request to open the workspace web terminal (PTY).
    ///
    /// Opening the same workspace again attaches to the running terminal. The
    /// node answers with [`NodeToHubMsg::PtyOpened`].
    PtySpawn {
        /// Correlation ID for the reply
        request_id: CommandId,
        /// Proposed PTY ID (reused when the workspace already has a terminal)
        pty_id: String,
        /// Workspace directory
        path: String,
        /// Session that opened the terminal (origin)
        session_id: Option<SessionId>,
        /// Number of columns
        cols: u16,
        /// Number of rows
        rows: u16,
        /// Replace a running terminal with a new shell
        new: bool,
    },
    /// Marks an additional client as attached to the PTY.
    PtyAttach {
        /// PTY ID
        pty_id: String,
    },
    /// Marks a client as detached (the PTY stays alive until its idle timeout).
    PtyDetach {
        /// PTY ID
        pty_id: String,
    },
    /// Sends user input (keystrokes) to the PTY.
    PtyInput {
        /// PTY ID
        pty_id: String,
        /// Input bytes (Base64)
        data_b64: String,
    },
    /// Resizes the PTY window.
    PtyResize {
        /// PTY ID
        pty_id: String,
        /// Number of columns
        cols: u16,
        /// Number of rows
        rows: u16,
    },
    /// Terminates the PTY process.
    PtyKill {
        /// PTY ID
        pty_id: String,
    },
    /// Request to flush the ephemeral VM before shutdown.
    DrainAndShutdown {
        /// Reason for shutting down (for logs)
        reason: String,
    },
    /// Emergency kill switch: immediately and forcibly stop every session, process
    /// tree and PTY running on the node.
    KillAllSessions {
        /// Reason for the action (for the audit log)
        reason: String,
    },
    /// Generic node operation (git, worktree, projects, agents, settings, secrets,
    /// and session lifecycle).
    NodeRpc {
        /// Correlation ID
        request_id: CommandId,
        /// Operation to execute
        request: NodeRpcRequest,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{DiffScope, SessionControlAction};

    #[test]
    fn event_batch_push_shape() {
        let msg = NodeToHubMsg::EventBatchPush {
            events: vec![],
            resync: false,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "event_batch_push");
        assert!(json["events"].as_array().unwrap().is_empty());
        let decoded: NodeToHubMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn node_hello_shape() {
        let msg = NodeToHubMsg::NodeHello {
            node_id: "home-win".into(),
            name: "Home Windows PC".into(),
            os: "windows".into(),
            arch: "x86_64".into(),
            version: "0.1.0".into(),
            protocol_version: PROTOCOL_VERSION,
            is_ephemeral: false,
            installed_agents: vec!["opencode2".into()],
            projects: vec![],
            projects_complete: true,
            state_revision: 0,
            sessions: vec![SessionSyncState {
                session_id: "s1".into(),
                project_id: "p1".into(),
                last_node_seq: 3,
            }],
            sessions_truncated: false,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "node_hello");
        assert_eq!(json["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(json["sessions"][0]["last_node_seq"], 3);
        let decoded: NodeToHubMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn node_hello_without_protocol_version_decodes_as_zero() {
        let msg = NodeToHubMsg::NodeHello {
            node_id: "home-win".into(),
            name: "Home Windows PC".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            version: "0.1.0".into(),
            protocol_version: PROTOCOL_VERSION,
            is_ephemeral: false,
            installed_agents: vec![],
            projects: vec![],
            projects_complete: true,
            state_revision: 0,
            sessions: vec![],
            sessions_truncated: false,
        };
        let mut json = serde_json::to_value(&msg).unwrap();
        json.as_object_mut().unwrap().remove("protocol_version");
        let decoded: NodeToHubMsg = serde_json::from_value(json).unwrap();
        let NodeToHubMsg::NodeHello {
            protocol_version, ..
        } = decoded
        else {
            panic!("expected node hello");
        };
        assert_eq!(protocol_version, 0);
    }

    #[test]
    fn server_to_node_control_session_shape() {
        let msg = HubToNodeMsg::ControlSession {
            command_id: "c1".into(),
            session_id: "s1".into(),
            action: SessionControlAction::SetMode {
                mode_id: "plan".into(),
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "control_session");
        assert_eq!(json["action"]["action"], "set_mode");
        assert_eq!(json["action"]["mode_id"], "plan");
        let decoded: HubToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn node_rpc_envelope_shape() {
        let msg = HubToNodeMsg::NodeRpc {
            request_id: "c1".into(),
            request: NodeRpcRequest::GetGitDiff {
                session_id: "s1".into(),
                scope: DiffScope::BranchBase,
                base_branch: Some("main".into()),
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "node_rpc");
        assert_eq!(json["request_id"], "c1");
        assert_eq!(json["request"]["type"], "get_git_diff");
        assert_eq!(json["request"]["scope"], "branch_base");
        let decoded: HubToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn node_rpc_result_shape() {
        let msg = NodeToHubMsg::NodeRpcResult {
            request_id: "c1".into(),
            outcome: NodeRpcOutcome::Ok {
                response: NodeRpcResponse::Resume {
                    context_restored: false,
                },
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "node_rpc_result");
        assert_eq!(json["outcome"]["status"], "ok");
        assert_eq!(json["outcome"]["response"]["type"], "resume");
        assert_eq!(json["outcome"]["response"]["context_restored"], false);
        let decoded: NodeToHubMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn pty_open_envelope_shape() {
        let spawn = HubToNodeMsg::PtySpawn {
            request_id: "c1".into(),
            pty_id: "p1".into(),
            path: "/home/u/repo".into(),
            session_id: Some("s1".into()),
            cols: 80,
            rows: 24,
            new: false,
        };
        let json = serde_json::to_value(&spawn).unwrap();
        assert_eq!(json["op"], "pty_spawn");
        assert_eq!(json["request_id"], "c1");
        assert_eq!(json["path"], "/home/u/repo");
        assert_eq!(serde_json::from_value::<HubToNodeMsg>(json).unwrap(), spawn);

        let opened = NodeToHubMsg::PtyOpened {
            request_id: "c1".into(),
            pty_id: "p1".into(),
            path: "/home/u/repo".into(),
            cols: 80,
            rows: 24,
            attached: true,
            snapshot_b64: "aGk=".into(),
        };
        let json = serde_json::to_value(&opened).unwrap();
        assert_eq!(json["op"], "pty_opened");
        assert_eq!(json["attached"], true);
        assert_eq!(json["snapshot_b64"], "aGk=");
        assert_eq!(
            serde_json::from_value::<NodeToHubMsg>(json).unwrap(),
            opened
        );
    }

    #[test]
    fn import_session_rpc_shape() {
        let msg = HubToNodeMsg::NodeRpc {
            request_id: "c1".into(),
            request: NodeRpcRequest::ImportSession {
                agent_id: "opencode2".into(),
                agent_session_id: "agent-9".into(),
                cwd: "/tmp/repo".into(),
                title: Some("Imported".into()),
                mirror_history: true,
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["request"]["type"], "import_session");
        assert_eq!(json["request"]["agent_session_id"], "agent-9");
        let decoded: HubToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);

        let result = NodeRpcResponse::SessionImported {
            session_id: "s1".into(),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["type"], "session_imported");
        assert_eq!(json["session_id"], "s1");
        assert_eq!(
            serde_json::from_value::<NodeRpcResponse>(json).unwrap(),
            result
        );
    }

    #[test]
    fn node_rpc_error_keeps_structured_info() {
        let msg = NodeToHubMsg::NodeRpcResult {
            request_id: "c1".into(),
            outcome: NodeRpcOutcome::Err {
                error: ErrorInfo::new(crate::common::ErrorCode::NotFound, "session not found")
                    .with_param("session_id", "s1"),
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["outcome"]["status"], "err");
        assert_eq!(json["outcome"]["error"]["code"], "NOT_FOUND");
        assert_eq!(json["outcome"]["error"]["params"]["session_id"], "s1");
        let decoded: NodeToHubMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn node_rpc_timeout_matches_operation() {
        assert_eq!(
            NodeRpcRequest::GetNodeSettings.timeout(),
            Duration::from_secs(30)
        );
        assert_eq!(
            NodeRpcRequest::ProjectScan { dir: None }.timeout(),
            Duration::from_secs(120)
        );
        assert_eq!(
            NodeRpcRequest::ListAgents.timeout(),
            Duration::from_secs(120)
        );
        assert_eq!(
            NodeRpcRequest::ImportSession {
                agent_id: "opencode2".into(),
                agent_session_id: "agent-9".into(),
                cwd: "/tmp/repo".into(),
                title: None,
                mirror_history: false,
            }
            .timeout(),
            Duration::from_secs(120)
        );
        assert_eq!(
            NodeRpcRequest::ProbeAgentConfig {
                agent_id: "opencode".into(),
                local_path: "/tmp".into(),
                force: false,
                initial_config: Vec::new(),
            }
            .timeout(),
            Duration::from_secs(120)
        );
        assert_eq!(
            NodeRpcRequest::ManageAgent {
                action: AgentAction::RefreshCatalog,
            }
            .timeout(),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn project_scan_and_link_shape() {
        let scan = HubToNodeMsg::NodeRpc {
            request_id: "r1".into(),
            request: NodeRpcRequest::ProjectScan {
                dir: Some("D:/ghq".into()),
            },
        };
        let json = serde_json::to_value(&scan).unwrap();
        assert_eq!(json["request"]["type"], "project_scan");
        assert_eq!(json["request"]["dir"], "D:/ghq");
        assert_eq!(serde_json::from_value::<HubToNodeMsg>(json).unwrap(), scan);

        let link = HubToNodeMsg::NodeRpc {
            request_id: "r2".into(),
            request: NodeRpcRequest::ProjectLink {
                project_id: "github.com/nazo6/flexagent".into(),
                local_path: "D:/ghq/github.com/nazo6/flexagent".into(),
            },
        };
        let json = serde_json::to_value(&link).unwrap();
        assert_eq!(json["request"]["type"], "project_link");
        assert_eq!(serde_json::from_value::<HubToNodeMsg>(json).unwrap(), link);

        let link_result = NodeToHubMsg::NodeRpcResult {
            request_id: "r2".into(),
            outcome: NodeRpcOutcome::Ok {
                response: NodeRpcResponse::ProjectLink {
                    response: crate::client_api::ProjectLinkResponse {
                        project_id: "github.com/nazo6/flexagent".into(),
                        local_path: "D:/ghq/github.com/nazo6/flexagent".into(),
                    },
                },
            },
        };
        let json = serde_json::to_value(&link_result).unwrap();
        assert_eq!(json["op"], "node_rpc_result");
        assert_eq!(
            serde_json::from_value::<NodeToHubMsg>(json).unwrap(),
            link_result
        );
    }

    #[test]
    fn worktree_prune_action_shape() {
        let msg = HubToNodeMsg::NodeRpc {
            request_id: "c1".into(),
            request: NodeRpcRequest::ManageWorktree {
                project_id: "p1".into(),
                action: WorktreeAction::Prune,
            },
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["request"]["action"]["action"], "prune");
        let decoded: HubToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn manage_agent_shape() {
        let refresh = HubToNodeMsg::NodeRpc {
            request_id: "r1".into(),
            request: NodeRpcRequest::ManageAgent {
                action: AgentAction::RefreshCatalog,
            },
        };
        let json = serde_json::to_value(&refresh).unwrap();
        assert_eq!(json["request"]["type"], "manage_agent");
        assert_eq!(json["request"]["action"]["action"], "refresh_catalog");
        assert_eq!(
            serde_json::from_value::<HubToNodeMsg>(json).unwrap(),
            refresh
        );

        let import = NodeRpcRequest::ManageAgent {
            action: AgentAction::ImportRegistryAgent {
                agent_id: "qwen-code".into(),
                extension: Some("opencode".into()),
            },
        };
        let json = serde_json::to_value(&import).unwrap();
        assert_eq!(json["action"]["action"], "import_registry_agent");
        assert_eq!(json["action"]["agent_id"], "qwen-code");
        assert_eq!(json["action"]["extension"], "opencode");

        let result = NodeToHubMsg::NodeRpcResult {
            request_id: "r1".into(),
            outcome: NodeRpcOutcome::Ok {
                response: NodeRpcResponse::AgentOp {
                    message: Some("refreshed catalog".into()),
                },
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["op"], "node_rpc_result");
        assert_eq!(
            serde_json::from_value::<NodeToHubMsg>(json).unwrap(),
            result
        );
    }
}
