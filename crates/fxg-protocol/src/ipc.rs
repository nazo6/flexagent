//! Local IPC messages between the CLI (`fxg`) and the local daemon (`fxg node start`).
//!
//! - **Transport**: Named Pipe on Windows (`\\.\pipe\fxg-node-<username>`), and a
//!   Unix Domain Socket on Linux / macOS / WSL
//!   (`$XDG_RUNTIME_DIR/fxg/node.sock` or `/tmp/fxg-<uid>/node.sock`)
//! - **Framing**: length-prefixed JSON
//!   (a little-endian length header of [`IPC_LENGTH_PREFIX_BYTES`] bytes + JSON)
//!
//! This is the internal protocol of the hub implementation and is not exposed
//! to the UI, so it is excluded from `specta` export
//! (design: `docs/03-protocol-and-api.md` §4).

use serde::{Deserialize, Serialize};

use crate::client_api::SessionDetail;
use crate::common::{
    ClientSource, ErrorInfo, PromptInput, SessionControlAction, SessionSummary, StreamDeltaPayload,
};
use crate::events::SessionEventEnvelope;
use crate::node_hub::{NodeRpcRequest, NodeRpcResponse};
use crate::{CommandId, ProjectId, SessionId};

/// Length of the length header of length-prefixed JSON (little-endian u32).
pub const IPC_LENGTH_PREFIX_BYTES: usize = 4;

/// Protocol version of the local IPC message set.
///
/// The CLI and the daemon are shipped from the same binary; a mismatch means a
/// stale daemon is still running, so it is rejected with a clear error.
pub const IPC_PROTOCOL_VERSION: u32 = 1;

/// Runtime status of the local node (the `GetLocalStatus` response).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalStatus {
    /// Sessions running locally or recently active
    pub sessions: Vec<SessionSummary>,
    /// WebSocket connection state to the hub
    pub hub_connected: bool,
    /// Number of outbox events not yet sent
    pub unsynced_event_count: u64,
    /// Last synchronization time (Unix epoch ms)
    pub last_synced_at: Option<i64>,
}

/// Envelope for every CLI -> daemon request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcRequest {
    /// Protocol version of the sender
    pub protocol_version: u32,
    /// Correlation ID used by the response
    pub request_id: CommandId,
    /// Operation to execute
    pub request: IpcRequestKind,
}

/// Operation carried by an [`IpcRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequestKind {
    /// Shared node operation (same type as Hub -> Node).
    Node {
        /// Operation to execute
        request: NodeRpcRequest,
    },
    /// CLI-only query or session command.
    Local {
        /// Operation to execute
        request: LocalRequest,
    },
}

/// Outcome of an [`IpcRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IpcOutcome {
    /// The operation succeeded.
    Ok {
        /// Typed result
        result: IpcResultKind,
    },
    /// The operation failed with a structured error.
    Err {
        /// Error code, message and recovery actions
        error: ErrorInfo,
    },
}

/// Successful payload of an [`IpcRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResultKind {
    /// Result of a shared [`NodeRpcRequest`].
    Node {
        /// Typed node response
        response: NodeRpcResponse,
    },
    /// Result of a CLI-only [`LocalRequest`].
    Local {
        /// Typed local result
        result: LocalResult,
    },
}

/// Request message from the CLI to the daemon (CLI-only operations).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum LocalRequest {
    /// Called by `fxg session new <agent>`; the node starts the session.
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
    /// Returns the running sessions, the hub connection state and the
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
    /// Returns a redacted support bundle (`fxg support bundle`).
    SupportBundle {
        /// Correlation ID
        command_id: CommandId,
    },
    /// Returns recent persisted incidents (`fxg support incidents`).
    Incidents {
        /// Correlation ID
        command_id: CommandId,
        /// Maximum number of entries
        limit: u32,
        /// Include resolved incidents
        include_resolved: bool,
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
    /// Branches the conversation (`fxg session fork`).
    ///
    /// Creates a new `session_id` using the ACP-native `session/fork` +
    /// `session/load` pair. The source session must use the same agent.
    SessionFork {
        /// Correlation ID
        command_id: CommandId,
        /// Source session ID
        session_id: SessionId,
        /// Working directory of the fork (defaults to the source; used by `-w/--worktree`)
        cwd: Option<String>,
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
    /// Creates a worktree (`fxg session new -w` / `fxg session fork -w`).
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
    },
    /// Lists sessions (`fxg session list`).
    ListSessions {
        /// Correlation ID
        command_id: CommandId,
        /// Whether to include stopped sessions (`-a/--all`)
        include_stopped: bool,
        /// Whether to include archived sessions (`--archived`)
        include_archived: bool,
    },
    /// Gracefully shuts down the local node (`fxg node stop`).
    Shutdown {
        /// Correlation ID
        command_id: CommandId,
    },
    /// Fetches or regenerates the authentication token (`fxg node token show` / `fxg node token rotate`).
    AuthToken {
        /// Correlation ID
        command_id: CommandId,
        /// Whether to regenerate it
        rotate: bool,
    },
    /// Resolves git credentials (`fxg internal git-askpass`, used as a `GIT_ASKPASS` helper).
    ///
    /// Git operations on an ephemeral VM (`fxg internal stdio`) are relayed through
    /// this method from the node to the hub (`GitCredentialRequest`),
    /// which authenticates without leaving the token on the VM disk
    /// (design: docs/01 §6.4).
    GitCredential {
        /// Correlation ID
        command_id: CommandId,
        /// Prompt string passed by git (`GIT_ASKPASS`)
        prompt: String,
    },
    /// Keep-alive
    Ping {
        /// Correlation ID
        command_id: CommandId,
    },
}

impl LocalRequest {
    /// Correlation ID carried by the request.
    pub fn command_id(&self) -> &CommandId {
        match self {
            Self::EnsureSession { command_id, .. }
            | Self::SessionImport { command_id, .. }
            | Self::GetLocalStatus { command_id }
            | Self::Diagnostics { command_id }
            | Self::SupportBundle { command_id }
            | Self::Incidents { command_id, .. }
            | Self::SendPrompt { command_id, .. }
            | Self::ControlSession { command_id, .. }
            | Self::SessionFork { command_id, .. }
            | Self::SessionShow { command_id, .. }
            | Self::WorktreeAdd { command_id, .. }
            | Self::ListSessions { command_id, .. }
            | Self::Shutdown { command_id }
            | Self::AuthToken { command_id, .. }
            | Self::GitCredential { command_id, .. }
            | Self::Ping { command_id } => command_id,
        }
    }
}

/// Response body from the daemon to the CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum LocalResult {
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
    /// Result of `SupportBundle`
    SupportBundle {
        /// Redacted support bundle
        bundle: Box<crate::client_api::SupportBundle>,
    },
    /// Result of `Incidents`
    Incidents {
        /// Incident list payload
        incidents: crate::common::IncidentListResponse,
    },
    /// Result of `GetLocalStatus`
    LocalStatus {
        /// Runtime status
        status: LocalStatus,
    },
    /// Acceptance of an asynchronous command
    /// (`SendPrompt` / `ControlSession`)
    CommandAccepted {
        /// Target session ID (optional)
        session_id: Option<SessionId>,
    },
    /// Result of `SessionFork`
    SessionForked {
        /// New session ID
        session_id: SessionId,
    },
    /// Result of `SessionShow`
    SessionDetail(Box<SessionDetail>),
    /// Result of `WorktreeAdd`
    WorktreeAdded {
        /// Worktree path
        path: String,
        /// Branch name
        branch: String,
        /// Whether it was newly created (`false` means an existing worktree was reused)
        created: bool,
    },
    /// Result of `ListSessions`
    Sessions {
        /// Sessions (newest update first)
        sessions: Vec<SessionSummary>,
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
    Response {
        /// Correlation ID
        request_id: CommandId,
        /// Typed result or structured error
        outcome: IpcOutcome,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ErrorCode;

    #[test]
    fn node_request_envelope_roundtrip() {
        let request = IpcRequest {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id: "c1".into(),
            request: IpcRequestKind::Node {
                request: NodeRpcRequest::ProjectLink {
                    project_id: "github.com/nazo6/flexagent".into(),
                    local_path: "/home/nazo/src/flexagent".into(),
                },
            },
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["protocol_version"], IPC_PROTOCOL_VERSION);
        assert_eq!(json["request"]["type"], "node");
        assert_eq!(json["request"]["request"]["type"], "project_link");
        let decoded: IpcRequest = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn local_request_envelope_roundtrip() {
        let request = IpcRequest {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id: "c1".into(),
            request: IpcRequestKind::Local {
                request: LocalRequest::EnsureSession {
                    command_id: "c1".into(),
                    cwd: "/Users/nazo/src/flexagent".into(),
                    agent_id: "opencode".into(),
                    extra_args: vec!["--model".into(), "sonnet".into()],
                    initial_mode: Some("plan".into()),
                },
            },
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["request"]["type"], "local");
        assert_eq!(json["request"]["request"]["method"], "ensure_session");
        assert_eq!(json["request"]["request"]["extra_args"][1], "sonnet");
        let decoded: IpcRequest = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn outcome_error_shape() {
        let message = IpcServerMessage::Response {
            request_id: "c1".into(),
            outcome: IpcOutcome::Err {
                error: ErrorInfo::new(ErrorCode::NotFound, "no such session"),
            },
        };
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["type"], "response");
        assert_eq!(json["outcome"]["status"], "err");
        assert_eq!(json["outcome"]["error"]["code"], "NOT_FOUND");
    }

    #[test]
    fn local_worktree_request_roundtrips() {
        let request = LocalRequest::WorktreeAdd {
            command_id: "c2".into(),
            cwd: "/home/nazo/src/flexagent".into(),
            project_id: None,
            branch: "feat/auth".into(),
            base_branch: Some("main".into()),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json["method"].is_string(), "method tag missing: {json}");
        let decoded: LocalRequest = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, request);
    }
}
