//! Node <-> Central Server protocol (shared by WebSocket and stdio pipe).
//!
//! A resident node (outbound WebSocket) and an ephemeral VM node (JSON Lines of
//! `fxg daemon --stdio`) differ only in the transport layer and share **exactly the
//! same message types** (design: `docs/03-protocol-and-api.md` §2).
//!
//! This is the internal protocol of the server implementation and is not exposed
//! to the UI, so it is excluded from `specta` export.

use serde::{Deserialize, Serialize};

use crate::common::{
    ClientSource, DataSafety, ElicitationContent, ErrorCode, ErrorInfo, NodeProjectReport,
    PromptInput, ResolvedBy, SessionControlAction, WarningInfo, WorkspaceDiffResponse,
};
use crate::events::SessionEventEnvelope;
use crate::{CommandId, NodeId, ProjectId, SessionId};
/// Synchronization state of a session held by the node (reported in `NodeHello`).
///
/// The hub compares it with `last_node_seq` in its own DB and returns
/// [`ServerToNodeMsg::ResyncRequest`] when something is missing or lagging.
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

/// Message sent from the node to the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum NodeToServerMsg {
    /// Handshake right after connecting
    /// (reports node information, project bindings and session sync states).
    NodeHello {
        /// Node ID (the server verifies that it matches the token's target)
        node_id: NodeId,
        /// Display name
        name: String,
        /// OS (`windows` / `linux` / `macos`)
        os: String,
        /// Architecture (`x86_64` / `aarch64`)
        arch: String,
        /// fxg binary version
        version: String,
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
    },
    /// Sends a batch of persisted events (store-and-forward outbox).
    ///
    /// Session metadata (such as `SessionCreated`) is also sent as events on
    /// `session_events`. Normally the events after `synced_up_to_node_seq` are sent
    /// in ascending `node_seq` order.
    EventBatchPush {
        /// Events to send (batches are split per session)
        events: Vec<SessionEventEnvelope>,
    },
    /// Realtime ephemeral chunk (for fast rendering before the DB flush).
    LiveStreamDelta {
        /// Target session ID
        session_id: SessionId,
        /// The delta body
        delta: crate::common::StreamDeltaPayload,
    },
    /// Result of a command from the server (correlated back to the requesting client).
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
    /// Notification that the PTY process exited.
    PtyExit {
        /// PTY ID
        pty_id: String,
        /// Exit code
        exit_code: Option<i32>,
    },
    /// Response with the result of fetching the git diff on the node.
    GitDiffResult {
        /// Correlation ID of the request
        request_id: String,
        /// The diff on success
        diff: Option<WorkspaceDiffResponse>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of a worktree operation (`ManageWorktree`).
    WorktreeResult {
        /// Correlation ID of the request
        command_id: CommandId,
        /// Worktree information on success (`None` when removed)
        worktree: Option<crate::client_api::WorktreeInfo>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of a session resume (`ResumeSession`).
    ResumeResult {
        /// Correlation ID of the request
        command_id: CommandId,
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Whether native restoration succeeded (`false` = continued with history replay)
        context_restored: Option<bool>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of archiving / restoring a session (`ArchiveSession`).
    ArchiveResult {
        /// Correlation ID of the request
        command_id: CommandId,
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Archive time on success (Unix epoch ms). `None` when restored
        archived_at: Option<i64>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of deleting a session (`DeleteSession`).
    DeleteResult {
        /// Correlation ID of the request
        command_id: CommandId,
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of browsing the file system (`BrowseFs`).
    BrowseFsResult {
        /// Correlation ID of the request
        request_id: String,
        /// Browse result on success
        response: Option<crate::client_api::FsBrowseResponse>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of a bulk project scan (`ProjectScan`).
    ProjectScanResult {
        /// Correlation ID of the request
        request_id: String,
        /// Scan result on success
        response: Option<crate::client_api::ProjectScanResponse>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of a manual project link (`ProjectLink`).
    ProjectLinkResult {
        /// Correlation ID of the request
        request_id: String,
        /// Link result on success
        response: Option<crate::client_api::ProjectLinkResponse>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of the ACP Registry catalog (`ListAgents`).
    AgentsResult {
        /// Correlation ID of the request
        request_id: String,
        /// Catalog plus installation state on success
        response: Option<crate::client_api::AgentsResponse>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the node settings snapshot (`GetNodeSettings` /
    /// `UpdateNodeSettings` / `UpdateAgentSettings`).
    NodeSettingsResult {
        /// Correlation ID of the request
        request_id: String,
        /// Snapshot on success
        snapshot: Option<Box<crate::settings::NodeSettingsSnapshot>>,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of a secret operation (`GetSecrets` /
    /// `UpsertSecret` / `DeleteSecret`).
    SecretOpResult {
        /// Correlation ID of the request
        request_id: String,
        /// Secret metadata after the operation (values are never included)
        secrets: Option<Vec<crate::settings::SecretSummary>>,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Failure message
        error: Option<String>,
    },
    /// Response with the result of an agent management operation (`ManageAgent`).
    AgentOpResult {
        /// Correlation ID of the request
        request_id: String,
        /// Whether it succeeded
        success: bool,
        /// Structured error code on failure
        code: Option<ErrorCode>,
        /// Display message on success (e.g. `installed opencode2 0.1.0`)
        message: Option<String>,
        /// Failure message
        error: Option<String>,
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
    /// In-memory git authentication request from an ephemeral VM (the `GIT_ASKPASS`
    /// proxy).
    GitCredentialRequest {
        /// Correlation ID
        request_id: String,
        /// Target repository URL
        repo_url: String,
        /// Operation kind (`clone` / `fetch` / `push`)
        operation: String,
    },
    /// Notification that a graceful drain completed (after receiving it the central
    /// server discards the child process / VM).
    DrainComplete {
        /// Node ID
        node_id: NodeId,
    },
}

/// Message sent from the server to the node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerToNodeMsg {
    /// Persistence-complete ACK for [`NodeToServerMsg::EventBatchPush`].
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
        /// Session ID to start (assigned by the server)
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
    /// Resumes a stopped session (`POST /api/v1/sessions/:id/resume`).
    ///
    /// Keeps the same `session_id` and continues by restoring the agent-side
    /// context (design: `docs/04-agent-drivers-and-windows.md` §4.1).
    ResumeSession {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// Skip native restoration and continue with history replay
        force_replay: bool,
    },
    /// Archives / restores a session (`POST /api/v1/sessions/:id/archive`).
    ///
    /// Archiving only hides or restores the session in the listing; the event log
    /// is kept.
    ArchiveSession {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// `true` = archive, `false` = restore
        archived: bool,
    },
    /// Deletes a session (`DELETE /api/v1/sessions/:id`).
    ///
    /// The running node stops the session if it is running, then appends a
    /// `SessionDeleted` event (tombstone) and purges the bodies of earlier events.
    DeleteSession {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
    },
    /// Request to start a workspace web terminal (PTY).
    PtySpawn {
        /// PTY ID
        pty_id: String,
        /// Target session ID
        session_id: SessionId,
        /// Number of columns
        cols: u16,
        /// Number of rows
        rows: u16,
        /// Shell command (defaults to the default shell)
        shell_cmd: Option<String>,
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
    /// Request for the git working tree / branch diff of the node.
    GetGitDiff {
        /// Correlation ID
        request_id: String,
        /// Target session ID
        session_id: SessionId,
        /// Scope to fetch (uncommitted / cumulative against the base branch)
        scope: crate::common::DiffScope,
        /// Base branch used for `BranchBase`
        base_branch: Option<String>,
    },
    /// Request for a worktree operation (create / remove).
    ManageWorktree {
        /// Correlation ID
        command_id: CommandId,
        /// Target logical project ID
        project_id: ProjectId,
        /// Operation to execute
        action: crate::common::WorktreeAction,
    },
    /// Request to browse the file system.
    BrowseFs {
        /// Correlation ID
        request_id: String,
        /// Path to browse (defaults to the home directory)
        path: Option<String>,
    },
    /// Request to scan and register the git repositories under a directory
    /// (`fxg project scan`).
    ProjectScan {
        /// Correlation ID
        request_id: String,
        /// Directory to scan (defaults to `node.project_scan_dirs`)
        dir: Option<String>,
    },
    /// Request to link an arbitrary directory to a logical project manually
    /// (`fxg project link`, which updates `project_path_links` in `node.db`).
    ProjectLink {
        /// Correlation ID
        request_id: String,
        /// Logical project ID to link to
        project_id: ProjectId,
        /// Target directory
        local_path: String,
    },
    /// Request for the ACP Registry catalog plus installation state (agent
    /// management in the Web UI).
    ListAgents {
        /// Correlation ID
        request_id: String,
    },
    /// Request for the node settings snapshot (hub proxy).
    GetNodeSettings {
        /// Correlation ID
        request_id: String,
    },
    /// Request to update node-owned settings (hub proxy).
    UpdateNodeSettings {
        /// Correlation ID
        request_id: String,
        /// New values and the observed revision
        request: crate::settings::UpdateNodeSettingsRequest,
    },
    /// Request to update agent settings (hub proxy).
    UpdateAgentSettings {
        /// Correlation ID
        request_id: String,
        /// New values and the observed revision
        request: crate::settings::UpdateAgentSettingsRequest,
    },
    /// Request for the node's secret metadata (hub proxy).
    GetSecrets {
        /// Correlation ID
        request_id: String,
    },
    /// Request to store a secret value (hub proxy; write-only).
    UpsertSecret {
        /// Correlation ID
        request_id: String,
        /// Target secret ID
        id: crate::SecretId,
        /// Name, kind and value
        request: crate::settings::UpsertSecretRequest,
    },
    /// Request to delete a secret (hub proxy).
    DeleteSecret {
        /// Correlation ID
        request_id: String,
        /// Target secret ID
        id: crate::SecretId,
    },
    /// Agent management operation request (install / update / remove).
    ManageAgent {
        /// Correlation ID
        request_id: String,
        /// Operation to execute
        action: crate::common::AgentAction,
    },
    /// Short-lived token response to a `GitCredentialRequest`.
    GitCredentialResponse {
        /// Correlation ID
        request_id: String,
        /// Basic authentication user name
        username: String,
        /// Token (in memory only; never written to the VM disk)
        token: Option<String>,
        /// Failure message
        error: Option<String>,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{DiffScope, SessionControlAction};

    #[test]
    fn event_batch_push_shape() {
        let msg = NodeToServerMsg::EventBatchPush { events: vec![] };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "event_batch_push");
        assert!(json["events"].as_array().unwrap().is_empty());
        let decoded: NodeToServerMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn node_hello_shape() {
        let msg = NodeToServerMsg::NodeHello {
            node_id: "home-win".into(),
            name: "Home Windows PC".into(),
            os: "windows".into(),
            arch: "x86_64".into(),
            version: "0.1.0".into(),
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
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "node_hello");
        assert_eq!(json["sessions"][0]["last_node_seq"], 3);
        let decoded: NodeToServerMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn server_to_node_control_session_shape() {
        let msg = ServerToNodeMsg::ControlSession {
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
        let decoded: ServerToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn git_diff_request_shape() {
        let msg = ServerToNodeMsg::GetGitDiff {
            request_id: "r1".into(),
            session_id: "s1".into(),
            scope: DiffScope::BranchBase,
            base_branch: Some("main".into()),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "get_git_diff");
        assert_eq!(json["scope"], "branch_base");
        let decoded: ServerToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn server_to_node_resume_session_shape() {
        let msg = ServerToNodeMsg::ResumeSession {
            command_id: "c1".into(),
            session_id: "s1".into(),
            force_replay: true,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "resume_session");
        assert_eq!(json["force_replay"], true);
        let decoded: ServerToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn node_to_server_resume_result_shape() {
        let msg = NodeToServerMsg::ResumeResult {
            command_id: "c1".into(),
            success: true,
            code: None,
            context_restored: Some(false),
            error: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["op"], "resume_result");
        assert_eq!(json["context_restored"], false);
        let decoded: NodeToServerMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn project_scan_and_link_shape() {
        let scan = ServerToNodeMsg::ProjectScan {
            request_id: "r1".into(),
            dir: Some("D:/ghq".into()),
        };
        let json = serde_json::to_value(&scan).unwrap();
        assert_eq!(json["op"], "project_scan");
        assert_eq!(json["dir"], "D:/ghq");
        assert_eq!(
            serde_json::from_value::<ServerToNodeMsg>(json).unwrap(),
            scan
        );

        let link = ServerToNodeMsg::ProjectLink {
            request_id: "r2".into(),
            project_id: "github.com/nazo6/flexagent".into(),
            local_path: "D:/ghq/github.com/nazo6/flexagent".into(),
        };
        let json = serde_json::to_value(&link).unwrap();
        assert_eq!(json["op"], "project_link");
        assert_eq!(
            serde_json::from_value::<ServerToNodeMsg>(json).unwrap(),
            link
        );

        let link_result = NodeToServerMsg::ProjectLinkResult {
            request_id: "r2".into(),
            response: Some(crate::client_api::ProjectLinkResponse {
                project_id: "github.com/nazo6/flexagent".into(),
                local_path: "D:/ghq/github.com/nazo6/flexagent".into(),
            }),
            error: None,
        };
        let json = serde_json::to_value(&link_result).unwrap();
        assert_eq!(json["op"], "project_link_result");
        assert_eq!(
            serde_json::from_value::<NodeToServerMsg>(json).unwrap(),
            link_result
        );
    }

    #[test]
    fn worktree_prune_action_shape() {
        let msg = ServerToNodeMsg::ManageWorktree {
            command_id: "c1".into(),
            project_id: "p1".into(),
            action: crate::common::WorktreeAction::Prune,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["action"]["action"], "prune");
        let decoded: ServerToNodeMsg = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn manage_agent_shape() {
        let refresh = ServerToNodeMsg::ManageAgent {
            request_id: "r1".into(),
            action: crate::common::AgentAction::RefreshCatalog,
        };
        let json = serde_json::to_value(&refresh).unwrap();
        assert_eq!(json["op"], "manage_agent");
        assert_eq!(json["action"]["action"], "refresh_catalog");
        assert_eq!(
            serde_json::from_value::<ServerToNodeMsg>(json).unwrap(),
            refresh
        );

        let set_pkg = ServerToNodeMsg::ManageAgent {
            request_id: "r2".into(),
            action: crate::common::AgentAction::SetPackageEnabled {
                agent_id: "demo".into(),
                enabled: true,
            },
        };
        let json = serde_json::to_value(&set_pkg).unwrap();
        assert_eq!(json["action"]["action"], "set_package_enabled");
        assert_eq!(json["action"]["agent_id"], "demo");
        assert_eq!(json["action"]["enabled"], true);

        let result = NodeToServerMsg::AgentOpResult {
            request_id: "r1".into(),
            success: true,
            code: None,
            message: Some("refreshed catalog".into()),
            error: None,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["op"], "agent_op_result");
        assert_eq!(
            serde_json::from_value::<NodeToServerMsg>(json).unwrap(),
            result
        );
    }
}
