//! Local IPC messages between the CLI (`fxg`) and the local daemon (`fxg daemon`).
//!
//! - **Transport**: Named Pipe on Windows (`\\.\pipe\fxg-daemon-<username>`), and a
//!   Unix Domain Socket on Linux / macOS / WSL
//!   (`$XDG_RUNTIME_DIR/fxg/daemon.sock` or `/tmp/fxg-<uid>/daemon.sock`)
//! - **Framing**: length-prefixed JSON
//!   (a little-endian length header of [`IPC_LENGTH_PREFIX_BYTES`] bytes + JSON)
//!
//! This is the internal protocol of the server implementation and is not exposed
//! to the UI, so it is excluded from `specta` export
//! (design: `docs/03-protocol-and-api.md` §4).

use serde::{Deserialize, Serialize};

use crate::client_api::WorktreeInfo;
use crate::common::{
    ClientSource, ElicitationAction, ElicitationContent, ElicitationRequestEntry, ErrorInfo,
    HookLogEntry, PermissionRequestEntry, ProjectResolutionSource, ProjectSummary, PromptInput,
    ResolvedBy, SessionControlAction, SessionSummary, StreamDeltaPayload,
};
use crate::events::SessionEventEnvelope;
use crate::{CommandId, ProjectId, SessionId};

/// Length of the length header of length-prefixed JSON (little-endian u32).
pub const IPC_LENGTH_PREFIX_BYTES: usize = 4;

/// Runtime status of the local node (the `GetLocalStatus` response).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalStatus {
    /// Sessions running locally or recently active
    pub sessions: Vec<SessionSummary>,
    /// WebSocket connection state to the central server
    pub central_connected: bool,
    /// Number of outbox events not yet sent
    pub unsynced_event_count: u64,
    /// Last synchronization time (Unix epoch ms)
    pub last_synced_at: Option<i64>,
}

/// Details of the logical project resolution (the result of `fxg project info`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Logical project ID (`project_key`)
    pub project_id: ProjectId,
    /// Display name
    pub name: String,
    /// Git URL the canonical form was derived from
    pub canonical_git_url: Option<String>,
    /// Git root (`None` when not a git repository)
    pub git_root: Option<String>,
    /// Relative subpath from the git root (monorepo)
    pub relative_subpath: Option<String>,
    /// Directory that was resolved
    pub local_path: String,
    /// Whether it is a git repository
    pub is_git_repo: bool,
    /// How it was resolved
    pub source: ProjectResolutionSource,
}

/// Request message from the CLI to the daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum IpcClientMessage {
    /// Called by `fxg run <agent>`; the daemon starts the session.
    EnsureSession {
        /// Correlation ID
        command_id: CommandId,
        /// Working directory (current directory / worktree)
        cwd: String,
        /// Agent ID
        agent_id: String,
        /// Arguments passed through to the agent process
        extra_args: Vec<String>,
        /// Initial mode (when `--mode` is given; applied with ACP `session/set_mode`)
        initial_mode: Option<String>,
    },
    /// Imports an existing agent session (native resume with a recorded ID).
    SessionImport {
        /// Correlation ID
        command_id: CommandId,
        /// Agent ID
        agent_id: String,
        /// Agent-side session ID to take over
        agent_session_id: String,
        /// Working directory
        cwd: String,
        /// Optional title
        title: Option<String>,
    },
    /// Returns the running sessions, the central server connection state and the
    /// number of unsent outbox events.
    GetLocalStatus {
        /// Correlation ID
        command_id: CommandId,
    },
    /// Returns local diagnostics (agent stderr tails, daemon log path).
    Diagnostics {
        /// Correlation ID
        command_id: CommandId,
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
    /// Restores files to a given turn (`fxg session revert`).
    ///
    /// Conversation events are not deleted; the revert operation is appended as a
    /// `session_reverted` event (`docs/04-agent-drivers-and-windows.md` §4.1).
    SessionRevert {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// `node_seq` of the `UserMessage` used as the revert point
        /// (defaults to the latest turn)
        target_node_seq: Option<u64>,
        /// Restore only files and keep the current agent conversation
        #[serde(default)]
        files_only: bool,
    },
    /// Branches the conversation from a given turn (`fxg session fork`).
    ///
    /// Creates a new `session_id` and injects the history up to the given
    /// `node_seq` into the new agent session as the initial context.
    SessionFork {
        /// Correlation ID
        command_id: CommandId,
        /// Source session ID
        session_id: SessionId,
        /// `node_seq` of the fork source (defaults to the latest event)
        from_node_seq: Option<u64>,
        /// Agent ID to use in the fork (defaults to the same agent)
        agent_id: Option<String>,
        /// Working directory of the fork (defaults to the source; used by `-w/--worktree`)
        cwd: Option<String>,
    },
    /// Resumes a stopped session (`fxg session resume`).
    ///
    /// Keeps the same `session_id` and continues by restoring the agent-side
    /// context (falling back in the order `session/resume` -> `session/load` ->
    /// history replay).
    SessionResume {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// Skip native restoration and continue with history replay (`--replay`)
        force_replay: bool,
    },
    /// Archives / restores a session (`fxg session archive` / `unarchive`).
    ///
    /// Archiving only hides or restores the session in the listing; the event log
    /// is kept.
    SessionArchive {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// `true` = archive, `false` = restore
        archived: bool,
    },
    /// Deletes a session (`fxg session delete`).
    ///
    /// A running session is stopped first, then the event bodies are purged
    /// (irreversible; only the tombstone used for synchronization remains).
    SessionDelete {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
    },
    /// Fetches session details (`fxg session show`).
    SessionShow {
        /// Correlation ID
        command_id: CommandId,
        /// Target session ID
        session_id: SessionId,
        /// Number of events to show from the end
        recent_events: u32,
    },
    /// Lists requests waiting for approval (`fxg inbox list`).
    InboxList {
        /// Correlation ID
        command_id: CommandId,
    },
    /// Resolves the logical project of a given directory (`fxg project info`).
    ProjectInfo {
        /// Correlation ID
        command_id: CommandId,
        /// Directory to resolve
        cwd: String,
    },
    /// Lists registered logical projects (`fxg project list`).
    ProjectList {
        /// Correlation ID
        command_id: CommandId,
    },
    /// Manually links the current directory to a logical project ID
    /// (stored in `.fxg.toml`. `fxg project link`)
    ProjectLink {
        /// Correlation ID
        command_id: CommandId,
        /// Target directory
        cwd: String,
        /// Logical project ID to link to
        project_id: ProjectId,
    },
    /// Scans the git repositories under a directory in bulk (`fxg project scan`).
    ProjectScan {
        /// Correlation ID
        command_id: CommandId,
        /// Directory to scan (walks `project_scan_dirs` when omitted)
        dir: Option<String>,
    },
    /// Lists worktrees (`fxg worktree list`).
    WorktreeList {
        /// Correlation ID
        command_id: CommandId,
        /// Directory of the target repository (resolved from the logical project when omitted)
        cwd: Option<String>,
        /// Filter by logical project ID
        project_id: Option<ProjectId>,
    },
    /// Creates a worktree (`fxg worktree add`).
    WorktreeAdd {
        /// Correlation ID
        command_id: CommandId,
        /// Directory of the main repository
        cwd: String,
        /// Logical project ID (resolved from `cwd` when omitted)
        project_id: Option<ProjectId>,
        /// Name of the branch to create
        branch: String,
        /// Base branch (defaults to the current HEAD)
        base_branch: Option<String>,
        /// Explicit destination path
        path: Option<String>,
    },
    /// Removes a worktree (`fxg worktree remove`).
    WorktreeRemove {
        /// Correlation ID
        command_id: CommandId,
        /// Directory of the main repository
        cwd: String,
        /// Branch name or path to remove
        target: String,
        /// Whether to remove even if there are uncommitted changes
        force: bool,
    },
    /// Cleans up the administrative data of deleted worktrees (`fxg worktree prune`).
    WorktreePrune {
        /// Correlation ID
        command_id: CommandId,
        /// Directory of the main repository
        cwd: String,
    },
    /// Lists sessions (`fxg ps` / `fxg session list`).
    ListSessions {
        /// Correlation ID
        command_id: CommandId,
        /// Whether to include stopped sessions (`-a/--all`)
        include_stopped: bool,
        /// Whether to include archived sessions (`--archived`)
        include_archived: bool,
    },
    /// Forcefully terminates every session, process tree and PTY on the local node (`fxg kill-all`).
    KillAll {
        /// Correlation ID
        command_id: CommandId,
    },
    /// Fetches or regenerates the authentication token (`fxg auth token` / `fxg auth rotate-token`).
    AuthToken {
        /// Correlation ID
        command_id: CommandId,
        /// Whether to regenerate it
        rotate: bool,
    },
    /// Resolves git credentials (`fxg git-askpass`, used as a `GIT_ASKPASS` helper).
    ///
    /// Git operations on an ephemeral VM (`fxg daemon --stdio`) are relayed through
    /// this method from the daemon to the central server (`GitCredentialRequest`),
    /// which authenticates without leaving the token on the VM disk
    /// (design: docs/01 §6.4).
    GitCredential {
        /// Correlation ID
        command_id: CommandId,
        /// Prompt string passed by git (`GIT_ASKPASS`)
        prompt: String,
    },
    /// Keep-alive
    Ping,
}

/// Response body from the daemon to the CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum IpcResult {
    /// Result of `SessionImport`
    SessionImported {
        /// New fxg session ID
        session_id: SessionId,
    },
    /// Result of `EnsureSession`
    EnsureSession {
        /// ID of the started session
        session_id: SessionId,
    },
    /// Result of `Diagnostics`
    Diagnostics {
        /// Diagnostics payload
        diagnostics: crate::client_api::DiagnosticsResponse,
    },
    /// Result of `GetLocalStatus`
    LocalStatus {
        /// Runtime status
        status: LocalStatus,
    },
    /// Acceptance of an asynchronous command
    /// (`SendPrompt` / `RespondPermission` / `ControlSession`)
    CommandAccepted {
        /// Target session ID (optional)
        session_id: Option<SessionId>,
    },
    /// Result of `SessionRevert`
    SessionReverted {
        /// `node_seq` of the `UserMessage` used as the reference
        target_node_seq: u64,
        /// Tree hash to restore to
        restored_tree_hash: String,
        /// Backup tree hash holding the state right before the restore
        backup_tree_hash: Option<String>,
        /// Number of restored files
        restored_files: u64,
        /// Number of removed files
        removed_files: u64,
        /// Whether the agent conversation was rebuilt
        context_rebuilt: bool,
    },
    /// Result of `SessionFork`
    SessionForked {
        /// New session ID
        session_id: SessionId,
    },
    /// Result of `SessionResume`
    SessionResumed {
        /// Resumed session ID (the same as the requested one)
        session_id: SessionId,
        /// Whether native restoration succeeded (`false` = continued with history replay)
        context_restored: bool,
    },
    /// Result of `SessionArchive`
    SessionArchived {
        /// Target session ID
        session_id: SessionId,
        /// Archive time (Unix epoch ms). `None` when restored
        archived_at: Option<i64>,
    },
    /// Result of `SessionDelete`
    SessionDeleted {
        /// ID of the deleted session
        session_id: SessionId,
    },
    /// Result of `SessionShow`
    SessionDetail(Box<SessionDetail>),
    /// Result of `InboxList`
    Inbox {
        /// Requests waiting for approval (across all sessions)
        requests: Vec<PermissionRequestEntry>,
        /// Elicitation requests waiting for an answer (across all sessions)
        elicitations: Vec<ElicitationRequestEntry>,
    },
    /// Result of `ProjectInfo`
    ProjectInfo {
        /// The resolved logical project
        project: ProjectInfo,
    },
    /// Result of `ProjectList`
    Projects {
        /// Registered logical projects
        projects: Vec<ProjectSummary>,
    },
    /// Result of `ProjectScan`
    ProjectScan {
        /// Logical projects resolved by the scan
        projects: Vec<ProjectSummary>,
        /// Directories that were actually scanned
        scanned_dirs: Vec<String>,
    },
    /// Result of `WorktreeList`
    Worktrees {
        /// Worktrees
        worktrees: Vec<WorktreeInfo>,
    },
    /// Result of `WorktreeAdd`
    WorktreeAdded {
        /// Worktree path
        path: String,
        /// Branch name
        branch: String,
        /// Whether it was newly created (`false` means an existing worktree was reused)
        created: bool,
        /// Execution log of the `post_create` hook
        hook_logs: Vec<HookLogEntry>,
    },
    /// Result of `ListSessions`
    Sessions {
        /// Sessions (newest update first)
        sessions: Vec<SessionSummary>,
    },
    /// Result of `KillAll`
    KillAll {
        /// Sessions that were terminated
        killed_sessions: Vec<String>,
        /// PTY IDs that were terminated
        killed_ptys: Vec<String>,
    },
    /// Result of `AuthToken`
    AuthToken {
        /// Client authentication token
        token: String,
    },
    /// Result of `GitCredential` (in memory only; never written to disk)
    GitCredential {
        /// Basic authentication user name
        username: String,
        /// Token (`None` when resolution failed)
        token: Option<String>,
        /// Failure message
        error: Option<String>,
    },
    /// A plain success response
    Ack {
        /// Supplementary message
        message: Option<String>,
    },
}

/// Message from the daemon to the CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcServerMessage {
    /// Response to a request
    Result {
        /// Correlation ID
        command_id: CommandId,
        /// Response body
        result: IpcResult,
    },
    /// Error response
    Error {
        /// Correlation ID (`None` when unknown, e.g. before the connection is established)
        command_id: Option<CommandId>,
        /// Structured error
        error: ErrorInfo,
    },
    /// Delivery of persisted events (the replay right after connecting and the
    /// realtime delivery afterwards)
    EventBatch {
        /// Target session ID
        session_id: SessionId,
        /// Events (ascending `node_seq`)
        events: Vec<SessionEventEnvelope>,
        /// Cursor of the node DB
        cursor: u64,
    },
    /// An ephemeral chunk produced while streaming (never persisted)
    LiveStreamDelta {
        /// Target session ID
        session_id: SessionId,
        /// The delta body
        delta: StreamDeltaPayload,
    },
}

/// Response body of `fxg session show`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ErrorCode;

    #[test]
    fn ensure_session_roundtrip() {
        let msg = IpcClientMessage::EnsureSession {
            command_id: "c1".into(),
            cwd: "/Users/nazo/src/flexagent".into(),
            agent_id: "opencode".into(),
            extra_args: vec!["--model".into(), "sonnet".into()],
            initial_mode: Some("plan".into()),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["method"], "ensure_session");
        assert_eq!(json["extra_args"][1], "sonnet");
        assert_eq!(json["initial_mode"], "plan");
        let decoded: IpcClientMessage = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn server_message_tags() {
        let msg = IpcServerMessage::Error {
            command_id: None,
            error: ErrorInfo::new(ErrorCode::NotFound, "no such session"),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["error"]["code"], "NOT_FOUND");
    }

    #[test]
    fn project_and_worktree_methods_roundtrip() {
        let cases = vec![
            IpcClientMessage::ProjectInfo {
                command_id: "c1".into(),
                cwd: "/home/nazo/src/flexagent".into(),
            },
            IpcClientMessage::ProjectList {
                command_id: "c2".into(),
            },
            IpcClientMessage::ProjectLink {
                command_id: "c3".into(),
                cwd: "/home/nazo/src/flexagent".into(),
                project_id: "internal/tools".into(),
            },
            IpcClientMessage::ProjectScan {
                command_id: "c4".into(),
                dir: Some("/home/nazo/src".into()),
            },
            IpcClientMessage::WorktreeList {
                command_id: "c5".into(),
                cwd: Some("/home/nazo/src/flexagent".into()),
                project_id: None,
            },
            IpcClientMessage::WorktreeAdd {
                command_id: "c6".into(),
                cwd: "/home/nazo/src/flexagent".into(),
                project_id: None,
                branch: "feat/auth".into(),
                base_branch: Some("main".into()),
                path: None,
            },
            IpcClientMessage::WorktreeRemove {
                command_id: "c7".into(),
                cwd: "/home/nazo/src/flexagent".into(),
                target: "feat/auth".into(),
                force: true,
            },
            IpcClientMessage::WorktreePrune {
                command_id: "c8".into(),
                cwd: "/home/nazo/src/flexagent".into(),
            },
            IpcClientMessage::ListSessions {
                command_id: "c9".into(),
                include_stopped: true,
                include_archived: true,
            },
            IpcClientMessage::SessionArchive {
                command_id: "c12".into(),
                session_id: "3f6b6f3e".into(),
                archived: true,
            },
            IpcClientMessage::SessionDelete {
                command_id: "c13".into(),
                session_id: "3f6b6f3e".into(),
            },
            IpcClientMessage::KillAll {
                command_id: "c10".into(),
            },
            IpcClientMessage::AuthToken {
                command_id: "c11".into(),
                rotate: false,
            },
        ];
        for message in cases {
            let json = serde_json::to_value(&message).unwrap();
            assert!(json["method"].is_string(), "method tag missing: {json}");
            let decoded: IpcClientMessage = serde_json::from_value(json).unwrap();
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn session_revert_and_fork_roundtrip() {
        for message in [
            IpcClientMessage::SessionRevert {
                command_id: "c1".into(),
                session_id: "s1".into(),
                target_node_seq: Some(4),
                files_only: false,
            },
            IpcClientMessage::SessionFork {
                command_id: "c2".into(),
                session_id: "s1".into(),
                from_node_seq: None,
                agent_id: Some("antigravity-acp".into()),
                cwd: None,
            },
        ] {
            let json = serde_json::to_value(&message).unwrap();
            assert!(json["method"].is_string(), "method tag missing: {json}");
            let decoded: IpcClientMessage = serde_json::from_value(json).unwrap();
            assert_eq!(decoded, message);
        }

        let reverted = IpcResult::SessionReverted {
            target_node_seq: 4,
            restored_tree_hash: "a".repeat(40),
            backup_tree_hash: Some("b".repeat(40)),
            restored_files: 3,
            removed_files: 1,
            context_rebuilt: true,
        };
        let json = serde_json::to_value(&reverted).unwrap();
        assert_eq!(json["result"], "session_reverted");
        assert_eq!(serde_json::from_value::<IpcResult>(json).unwrap(), reverted);

        let forked = IpcResult::SessionForked {
            session_id: "child".into(),
        };
        let json = serde_json::to_value(&forked).unwrap();
        assert_eq!(json["result"], "session_forked");
        assert_eq!(serde_json::from_value::<IpcResult>(json).unwrap(), forked);
    }

    #[test]
    fn session_resume_roundtrip() {
        let resume = IpcClientMessage::SessionResume {
            command_id: "c1".into(),
            session_id: "s1".into(),
            force_replay: true,
        };
        let json = serde_json::to_value(&resume).unwrap();
        assert_eq!(json["method"], "session_resume");
        assert_eq!(json["force_replay"], true);
        assert_eq!(
            serde_json::from_value::<IpcClientMessage>(json).unwrap(),
            resume
        );

        let resumed = IpcResult::SessionResumed {
            session_id: "s1".into(),
            context_restored: false,
        };
        let json = serde_json::to_value(&resumed).unwrap();
        assert_eq!(json["result"], "session_resumed");
        assert_eq!(json["context_restored"], false);
        assert_eq!(serde_json::from_value::<IpcResult>(json).unwrap(), resumed);
    }

    #[test]
    fn extended_results_serialize() {
        let result = IpcResult::ProjectInfo {
            project: ProjectInfo {
                project_id: "github.com/nazo6/flexagent".into(),
                name: "flexagent".into(),
                canonical_git_url: Some("git@github.com:nazo6/flexagent.git".into()),
                git_root: Some("/home/nazo/src/flexagent".into()),
                relative_subpath: None,
                local_path: "/home/nazo/src/flexagent".into(),
                is_git_repo: true,
                source: ProjectResolutionSource::GitRemote,
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["result"], "project_info");
        assert_eq!(json["project"]["source"], "git_remote");
        let decoded: IpcResult = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, result);

        let added = IpcResult::WorktreeAdded {
            path: "/home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth".into(),
            branch: "feat/auth".into(),
            created: true,
            hook_logs: vec![HookLogEntry {
                command: "pnpm install".into(),
                success: true,
                output: "done".into(),
            }],
        };
        let json = serde_json::to_value(&added).unwrap();
        assert_eq!(json["result"], "worktree_added");
        assert_eq!(json["hook_logs"][0]["success"], true);
        let decoded: IpcResult = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, added);
    }
}
