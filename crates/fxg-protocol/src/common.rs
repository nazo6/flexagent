//! Shared helper types and error codes.
//!
//! Defines the types referenced by `UnifiedEventPayload`, by each protocol
//! message and by REST responses. All of them are handled by clients (UI / CLI),
//! so they carry `#[derive(specta::Type)]` + `#[specta(collect)]`
//! (design: `docs/03-protocol-and-api.md` §1.1).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Error returned when parsing the string representation of an enum fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} value: {value:?}")]
pub struct EnumParseError {
    /// Enum kind name (used in the error message)
    pub kind: &'static str,
    /// The string that failed to parse
    pub value: String,
}

impl EnumParseError {
    /// Creates a new parse error.
    pub fn new(kind: &'static str, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }
}

/// Session status (same values as `sessions.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum SessionStatus {
    /// Provisioning an ephemeral VM
    Provisioning,
    /// Bootstrapping (automatically installing tools)
    Bootstrapping,
    /// Idle (waiting for a prompt)
    Idle,
    /// Running a turn
    Running,
    /// Waiting for permission approval
    WaitingPermission,
    /// Waiting for an elicitation (question / structured input) answer
    WaitingInput,
    /// Stopped
    Stopped,
    /// Error
    Error,
}

impl SessionStatus {
    /// String representation used in the DB / JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Bootstrapping => "bootstrapping",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::WaitingPermission => "waiting_permission",
            Self::WaitingInput => "waiting_input",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SessionStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "provisioning" => Ok(Self::Provisioning),
            "bootstrapping" => Ok(Self::Bootstrapping),
            "idle" => Ok(Self::Idle),
            "running" => Ok(Self::Running),
            "waiting_permission" => Ok(Self::WaitingPermission),
            "waiting_input" => Ok(Self::WaitingInput),
            "stopped" => Ok(Self::Stopped),
            "error" => Ok(Self::Error),
            other => Err(EnumParseError::new("session status", other)),
        }
    }
}

/// Lifecycle status of a node (same values as `nodes.lifecycle_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum NodeLifecycleStatus {
    /// Provisioning (ephemeral VM)
    Provisioning,
    /// Bootstrapping (ephemeral VM)
    Bootstrapping,
    /// Ready to serve
    Ready,
    /// Shutting down (draining)
    Draining,
    /// Terminated
    Terminated,
    /// Error
    Error,
}

impl NodeLifecycleStatus {
    /// String representation used in the DB / JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Bootstrapping => "bootstrapping",
            Self::Ready => "ready",
            Self::Draining => "draining",
            Self::Terminated => "terminated",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for NodeLifecycleStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NodeLifecycleStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "provisioning" => Ok(Self::Provisioning),
            "bootstrapping" => Ok(Self::Bootstrapping),
            "ready" => Ok(Self::Ready),
            "draining" => Ok(Self::Draining),
            "terminated" => Ok(Self::Terminated),
            "error" => Ok(Self::Error),
            other => Err(EnumParseError::new("node lifecycle status", other)),
        }
    }
}

/// Status of a permission request (same values as `permission_requests.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum PermissionRequestStatus {
    /// Unresolved (waiting for approval)
    Pending,
    /// Approved
    Approved,
    /// Rejected
    Rejected,
    /// Cancelled (e.g. the session was stopped)
    Cancelled,
}

impl PermissionRequestStatus {
    /// String representation used in the DB / JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for PermissionRequestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PermissionRequestStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(EnumParseError::new("permission request status", other)),
        }
    }
}

/// User response action for an elicitation (same values as the ACP `ElicitationAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum ElicitationAction {
    /// Submit an answer (the form contents go into `content`)
    Accept,
    /// Explicitly decline
    Decline,
    /// Cancel the interaction (e.g. close the dialog)
    Cancel,
}

impl ElicitationAction {
    /// String representation used in JSON / IPC.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Decline => "decline",
            Self::Cancel => "cancel",
        }
    }
}

impl fmt::Display for ElicitationAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ElicitationAction {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "accept" => Ok(Self::Accept),
            "decline" => Ok(Self::Decline),
            "cancel" => Ok(Self::Cancel),
            other => Err(EnumParseError::new("elicitation action", other)),
        }
    }
}

/// Status of an elicitation request (same values as `elicitation_requests.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum ElicitationRequestStatus {
    /// Unresolved (waiting for an answer)
    Pending,
    /// Answered (`accept`)
    Accepted,
    /// Declined (`decline`)
    Declined,
    /// Cancelled (e.g. the session was stopped)
    Cancelled,
}

impl ElicitationRequestStatus {
    /// String representation used in the DB / JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for ElicitationRequestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ElicitationRequestStatus {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(EnumParseError::new("elicitation request status", other)),
        }
    }
}

/// How the logical project (`project_key`) was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum ProjectResolutionSource {
    /// Explicitly set in `.fxg.toml` (highest priority)
    FxgToml,
    /// Normalized `git remote.origin.url`
    GitRemote,
    /// Fallback (`local:<node_id>:<path-hash>`)
    Fallback,
}

impl ProjectResolutionSource {
    /// Identifier used for display.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FxgToml => "fxg_toml",
            Self::GitRemote => "git_remote",
            Self::Fallback => "fallback",
        }
    }
}

impl fmt::Display for ProjectResolutionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProjectResolutionSource {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fxg_toml" => Ok(Self::FxgToml),
            "git_remote" => Ok(Self::GitRemote),
            "fallback" => Ok(Self::Fallback),
            other => Err(EnumParseError::new("project resolution source", other)),
        }
    }
}

/// Execution log of the `copy_files` / `post_create` hooks run when creating a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct HookLogEntry {
    /// Command string that was executed
    pub command: String,
    /// Whether it succeeded
    pub success: bool,
    /// Output (stdout + stderr)
    pub output: String,
}

/// Accumulated session cost (ACP `Cost`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct UsageCost {
    /// Accumulated cost amount
    pub amount: f64,
    /// ISO 4217 currency code (e.g. `USD`)
    pub currency: String,
}

/// Context usage and accumulated cost of a session
/// (the `usage_update` of ACP `session/update`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionUsage {
    /// Number of tokens currently occupying the context
    pub used_tokens: u64,
    /// Total context window size (in tokens)
    pub context_size: u64,
    /// Accumulated cost (only when reported by the agent)
    pub cost: Option<UsageCost>,
}

/// Reasons an agent stopped a turn that should be surfaced to the user.
///
/// Of the ACP `StopReason` values, `EndTurn` / `Cancelled` end the turn normally
/// and are not turned into an event (see `UnifiedEventPayload::TurnEnded`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum TurnStopReason {
    /// Cut off after reaching the maximum number of tokens
    MaxTokens,
    /// Cut off after reaching the per-turn limit of agent requests
    MaxTurnRequests,
    /// The agent refused to continue (Refusal)
    Refusal,
}

impl TurnStopReason {
    /// String representation used in the DB / JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxTokens => "max_tokens",
            Self::MaxTurnRequests => "max_turn_requests",
            Self::Refusal => "refusal",
        }
    }
}

impl fmt::Display for TurnStopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Progress of context compaction.
///
/// This is the state representation of `UnifiedEventPayload::CompactionUpdated`.
/// The standard ACP has no API to start a compaction, so only drivers with a
/// native API (such as `opencode2`'s `POST /api/session/{id}/compact`) emit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum CompactionStatus {
    /// Compaction started
    Started,
    /// Compaction completed
    Completed,
    /// Compaction failed
    Failed,
}

impl CompactionStatus {
    /// String representation used in the DB / JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for CompactionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Aggregated session type used in API responses (built from a `sessions` row).
///
/// On the hub side it is produced from the projection obtained by applying
/// events. Both the local node and the central server return this same type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionSummary {
    /// Session ID (UUID v7)
    pub session_id: String,
    /// Logical project ID (e.g. `github.com/nazo6/flexagent`)
    pub project_id: String,
    /// ID of the node running the session
    pub node_id: String,
    /// Working directory (including the worktree path)
    pub local_path: String,
    /// Snapshot taken at start-up (live values come from the worktree API)
    pub git_branch: Option<String>,
    /// Whether it runs inside a worktree
    pub is_worktree: bool,
    /// Agent ID (`opencode2` / `antigravity-acp` etc.)
    pub agent_id: String,
    /// Session ID inside the agent (fixed by the `SessionAgentBound` event)
    pub agent_session_id: Option<String>,
    /// Session ID this session was forked from
    pub parent_session_id: Option<String>,
    /// Event (`node_seq`) of the parent session the fork / revert started from
    pub fork_from_node_seq: Option<u64>,
    /// Session title
    pub title: String,
    /// Session status
    pub status: SessionStatus,
    /// Current ACP SessionMode (e.g. `code`, `plan`)
    pub current_mode: Option<String>,
    /// Latest context usage and accumulated cost (projection of the `UsageUpdated` event).
    ///
    /// `None` means nothing has been received yet (including the case where the
    /// agent never sends `usage_update`).
    pub usage: Option<SessionUsage>,
    /// Latest persisted `node_seq` (node) / highest `node_seq` applied to the projection (hub)
    pub last_node_seq: u64,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
    /// Archive time (Unix epoch ms). `None` means not archived.
    ///
    /// Archived sessions are excluded from the default listing (they are only
    /// returned when `include_archived` is set).
    pub archived_at: Option<i64>,
}

/// Project binding information reported in `NodeHello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeProjectReport {
    /// Logical project ID
    pub project_id: String,
    /// Display name
    pub name: String,
    /// Git URL the canonical form was derived from
    pub canonical_git_url: Option<String>,
    /// Local path bindings on this node (including worktrees)
    pub bindings: Vec<ProjectNodeBinding>,
}

/// Local path binding between a project and a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectNodeBinding {
    /// Local path on the node
    pub local_path: String,
    /// Whether this is a git worktree (false means the main repository)
    pub is_worktree: bool,
    /// Whether the path exists on the node at report time (directory check)
    pub path_exists: bool,
    /// Git branch as of the last check
    pub git_branch: Option<String>,
    /// Last used time (Unix epoch ms)
    pub last_used_at: i64,
}

/// Ephemeral chunk produced while streaming.
///
/// It is never persisted to the DB and never resent (exclusive to
/// `LiveStreamDelta`). Once the final event (`AgentMessage` etc.) arrives, the
/// client replaces the chunk with the committed representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "delta_type", rename_all = "snake_case")]
#[specta(collect)]
pub enum StreamDeltaPayload {
    /// Incremental text of an agent reply message
    AgentMessageDelta {
        /// Message ID (corresponds to `AgentMessage.message_id`)
        message_id: String,
        /// Appended text
        text_delta: String,
    },
    /// Incremental text of the reasoning process
    AgentThoughtDelta {
        /// Thought ID (corresponds to `AgentThought.thought_id`)
        thought_id: String,
        /// Appended text
        text_delta: String,
    },
    /// Progress update of a tool call
    ToolCallProgress {
        /// Tool call ID
        tool_call_id: String,
        /// Updated status
        status: String,
        /// Output delta (optional)
        raw_output_delta: Option<String>,
    },
    /// Output delta of an ACP terminal/*
    TerminalOutputDelta {
        /// Terminal ID
        terminal_id: String,
        /// Output bytes (Base64)
        data_b64: String,
    },
}

/// Metadata of a file attached to a user message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AttachmentMeta {
    /// File name
    pub file_name: String,
    /// MIME type
    pub mime_type: String,
    /// Size in bytes
    pub size_bytes: u64,
    /// Reference to the real file on the node
    pub local_path: Option<String>,
    /// Base64 payload for transporting small attachments inline
    pub data_b64: Option<String>,
}

/// Unified diff information of a file change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct FileDiff {
    /// Path of the affected file
    pub path: String,
    /// Text before the change
    pub old_text: Option<String>,
    /// Text after the change
    pub new_text: Option<String>,
    /// Unified diff body
    pub unified_diff: String,
    /// Number of added lines
    pub additions: u32,
    /// Number of deleted lines
    pub deletions: u32,
}

/// An execution plan entry of the agent (ACP Plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PlanEntry {
    /// Entry ID
    pub id: String,
    /// Display title
    pub title: String,
    /// Status (`pending` / `in_progress` / `completed` etc.)
    pub status: String,
}

/// An option of a permission approval (ACP `PermissionOption`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PermissionOption {
    /// Option ID (the value given to `PermissionResolved.selected_option_id`)
    pub option_id: String,
    /// Display name
    pub name: String,
    /// Kind (`allow_once` / `allow_always` / `reject_once` / `reject_always` etc.)
    pub kind: String,
}

impl PermissionOption {
    /// Whether this is a reject-kind option (the kind starts with `reject`).
    pub fn is_reject_kind(kind: &str) -> bool {
        kind.starts_with("reject")
    }
}

/// Information about an ACP SessionMode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ModeInfo {
    /// Mode ID (e.g. `code`, `plan`)
    pub mode_id: String,
    /// Display name
    pub name: String,
    /// Description
    pub description: Option<String>,
}

/// Information about an ACP slash command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct CommandInfo {
    /// Command name
    pub name: String,
    /// Description
    pub description: String,
    /// Input hint
    pub input_hint: Option<String>,
}

/// Information about an agent configuration entry (model selection etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ConfigOptionInfo {
    /// Configuration key
    pub key: String,
    /// Display name
    pub name: String,
    /// Current value
    pub current_value: serde_json::Value,
    /// List of selectable values
    pub options: Vec<serde_json::Value>,
}

/// Session control action (`ControlSession` command).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "action", rename_all = "snake_case")]
#[specta(collect)]
pub enum SessionControlAction {
    /// Switch mode (`plan` / `code` etc.)
    SetMode {
        /// ID of the mode to switch to
        mode_id: String,
    },
    /// Change a configuration value (model selection etc.)
    SetConfig {
        /// Configuration key
        key: String,
        /// New value
        value: serde_json::Value,
    },
    /// Interrupt the current turn
    Cancel,
    /// Terminate the session entirely
    Kill,
}

/// Worktree operation action (`ManageWorktree` command).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "action", rename_all = "snake_case")]
#[specta(collect)]
pub enum WorktreeAction {
    /// Create a new worktree (`git worktree add -b <branch> <path> <base>`)
    Add {
        /// Name of the branch to create
        branch: String,
        /// Base branch (defaults to the current HEAD)
        base_branch: Option<String>,
        /// Explicit destination path (resolved from `worktree_dir_template` when omitted)
        new_path: Option<String>,
    },
    /// Remove a worktree (`git worktree remove`)
    Remove {
        /// Path to remove
        path: String,
        /// Whether to remove even if there are uncommitted changes
        force: bool,
    },
    /// Clean up the administrative data of deleted worktrees (`git worktree prune`)
    Prune,
}

/// Agent management action (shared by the `ManageAgent` command and the Client API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "action", rename_all = "snake_case")]
#[specta(collect)]
pub enum AgentAction {
    /// Force a refresh of the ACP Registry catalog index (`fxg agents update`)
    RefreshCatalog,
    /// Create or update a UI-managed custom agent definition
    UpsertCustom {
        /// Agent ID
        agent_id: String,
        /// Command, arguments and environment variables
        config: crate::config::CustomAgentConfig,
    },
    /// Delete a UI-managed custom agent definition
    RemoveCustom {
        /// Agent ID
        agent_id: String,
    },
    /// Enable or disable usage through the `npx` / `uvx` distribution
    SetPackageEnabled {
        /// Agent ID (aliases allowed)
        agent_id: String,
        /// `true` = show it in the list and use it through `npx` / `uvx`
        enabled: bool,
    },
}

/// Scope of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum DiffScope {
    /// Uncommitted diff of the working tree (`git diff HEAD`)
    Uncommitted,
    /// Cumulative diff against the base branch (`git diff <base>...HEAD`)
    BranchBase,
}

/// Result of fetching the diff of a git working tree (`GET /api/v1/sessions/:id/diff`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WorkspaceDiffResponse {
    /// Scope of the fetched diff
    pub scope: DiffScope,
    /// Base branch used for the comparison (set for `BranchBase`)
    pub base_branch: Option<String>,
    /// Current HEAD commit
    pub head_commit: String,
    /// Diff per changed file
    pub files: Vec<FileDiff>,
}

/// A conversation history item injected when forking a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ForkHistoryItem {
    /// Speaker role (`user` / `agent` etc.)
    pub role: String,
    /// Body text
    pub text: String,
    /// Summary of tool calls etc. (optional)
    pub tool_summary: Option<String>,
}

/// Shared error code (used by both `CommandResult` and REST error responses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[specta(collect)]
pub enum ErrorCode {
    /// Missing or mismatching authentication token (HTTP `401 Unauthorized`)
    Unauthorized,
    /// `Host` / `Origin` validation failure or policy violation (HTTP `403 Forbidden`.
    /// Distinct from `PTY_DISABLED`, which means remote PTY is disabled)
    Forbidden,
    /// The target node is disconnected from the central server (returned immediately, not queued)
    NodeOffline,
    /// The permission request was already resolved (a second or later answer; treated idempotently)
    AlreadyResolved,
    /// The session is busy
    Busy,
    /// The operation cannot run in the current state
    InvalidState,
    /// The operation targets a stopped session, but the agent does not support
    /// native restoration (`session/resume` etc.). An explicit resume through
    /// history replay (`fxg session resume` / Resume in the Web UI) is required.
    ResumeRequired,
    /// Remote PTY is disabled by the security policy
    PtyDisabled,
    /// Duplicate `command_id` submission (double tap / WS resend)
    CommandDuplicate,
    /// The target was not found
    NotFound,
    /// Internal error
    Internal,
}

impl ErrorCode {
    /// String representation used in JSON / WS / REST.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Forbidden => "FORBIDDEN",
            Self::NodeOffline => "NODE_OFFLINE",
            Self::AlreadyResolved => "ALREADY_RESOLVED",
            Self::Busy => "BUSY",
            Self::InvalidState => "INVALID_STATE",
            Self::ResumeRequired => "RESUME_REQUIRED",
            Self::PtyDisabled => "PTY_DISABLED",
            Self::CommandDuplicate => "COMMAND_DUPLICATE",
            Self::NotFound => "NOT_FOUND",
            Self::Internal => "INTERNAL",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ErrorCode {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "UNAUTHORIZED" => Ok(Self::Unauthorized),
            "FORBIDDEN" => Ok(Self::Forbidden),
            "NODE_OFFLINE" => Ok(Self::NodeOffline),
            "ALREADY_RESOLVED" => Ok(Self::AlreadyResolved),
            "BUSY" => Ok(Self::Busy),
            "INVALID_STATE" => Ok(Self::InvalidState),
            "RESUME_REQUIRED" => Ok(Self::ResumeRequired),
            "PTY_DISABLED" => Ok(Self::PtyDisabled),
            "COMMAND_DUPLICATE" => Ok(Self::CommandDuplicate),
            "NOT_FOUND" => Ok(Self::NotFound),
            "INTERNAL" => Ok(Self::Internal),
            other => Err(EnumParseError::new("error code", other)),
        }
    }
}

/// Result of a command (correlated back from the node through the server to the requesting client).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct CommandResult {
    /// Correlation ID attached to each `ServerToNodeMsg` command
    pub command_id: String,
    /// Whether it succeeded
    pub success: bool,
    /// Structured error code on failure
    pub code: Option<ErrorCode>,
    /// Human-readable error message
    pub error: Option<String>,
    /// Target session ID (optional)
    pub session_id: Option<String>,
}

/// Project projection exposed to clients (`GET /api/v1/projects`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectSummary {
    /// Logical project ID
    pub project_id: String,
    /// Display name
    pub name: String,
    /// Git URL the canonical form was derived from
    pub canonical_git_url: Option<String>,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
    /// Last update time (Unix epoch ms)
    pub updated_at: i64,
    /// Local path bindings for each participating node
    pub bindings: Vec<ProjectBindingSummary>,
}

/// Project-to-node binding exposed to clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ProjectBindingSummary {
    /// Node ID
    pub node_id: String,
    /// Local path on the node
    pub local_path: String,
    /// Whether this is a git worktree
    pub is_worktree: bool,
    /// Whether the local path actually exists on the node
    ///
    /// On the central server this is the value from the last `NodeHello` report;
    /// on the local node it is checked live when fetching.
    pub path_exists: bool,
    /// Git branch as of the last check
    pub git_branch: Option<String>,
    /// Last used time (Unix epoch ms)
    pub last_used_at: i64,
}

/// Node information exposed to clients (`GET /api/v1/nodes` / `GET /api/v1/system/info`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct NodeSummary {
    /// Node ID
    pub node_id: String,
    /// Display name
    pub name: String,
    /// OS (`windows` / `linux` / `macos`)
    pub os: String,
    /// Architecture (`x86_64` / `aarch64`)
    pub arch: String,
    /// fxg binary version
    pub version: String,
    /// Whether this is an ephemeral VM / container node
    pub is_ephemeral: bool,
    /// Provisioner identifier of an ephemeral node
    pub provisioner: Option<String>,
    /// Lifecycle status
    pub lifecycle_status: NodeLifecycleStatus,
    /// Whether it is currently connected
    pub is_online: bool,
    /// Last successful contact time (Unix epoch ms)
    pub last_seen_at: i64,
    /// Agent IDs available on this node
    pub installed_agents: Vec<String>,
}

/// Permission request projection exposed to clients (the global approval inbox).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PermissionRequestEntry {
    /// ACP request id
    pub request_id: String,
    /// Target session ID
    pub session_id: String,
    /// Target node ID
    pub node_id: String,
    /// Tool name (e.g. `terminal/create`)
    pub tool_name: String,
    /// Summary (e.g. `Run command: cargo test`)
    pub summary: String,
    /// Available options
    pub options: Vec<PermissionOption>,
    /// Arguments and diff details
    pub details: serde_json::Value,
    /// Status
    pub status: PermissionRequestStatus,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
    /// Resolution time (Unix epoch ms)
    pub resolved_at: Option<i64>,
    /// Who resolved it (`cli` / `web` / `android_push`)
    pub resolved_by: Option<String>,
}

/// Elicitation request projection exposed to clients (the question inbox).
///
/// Represents a request where the agent asked the user for structured input via
/// `elicitation/create` (Phase 1 supports form mode only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ElicitationRequestEntry {
    /// ACP elicitation id
    pub elicitation_id: String,
    /// Target session ID
    pub session_id: String,
    /// Target node ID
    pub node_id: String,
    /// Message shown to the user
    pub message: String,
    /// Request mode (`form` / `url`. Phase 1 supports `form` only)
    pub mode: String,
    /// JSON Schema of the request in form mode (`requestedSchema`)
    pub requested_schema: serde_json::Value,
    /// Related tool call ID (optional)
    pub tool_call_id: Option<String>,
    /// Answer content for `accept` (`Value::Null` while unanswered or on decline / cancel)
    pub content: serde_json::Value,
    /// Status
    pub status: ElicitationRequestStatus,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
    /// Resolution time (Unix epoch ms)
    pub resolved_at: Option<i64>,
    /// Who resolved it (`cli` / `web` / `android_push`)
    pub resolved_by: Option<String>,
}

/// A hit of the FTS5 full-text search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SearchHit {
    /// Target session ID
    pub session_id: String,
    /// Target event ID
    pub event_id: String,
    /// Sequence number inside the session
    pub node_seq: u64,
    /// Event type
    pub event_type: String,
    /// Snippet (an excerpt containing the match)
    pub snippet: String,
    /// Event creation time (Unix epoch ms)
    pub created_at: i64,
}

/// An audit log entry (`GET /api/v1/audit/logs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AuditLogEntry {
    /// Log ID
    pub id: i64,
    /// Action kind (`session_start` / `permission_resolved` / `pty_spawn` / `kill_switch` / `worktree_manage`)
    pub action: String,
    /// Related session ID (optional)
    pub session_id: Option<String>,
    /// Target node ID (optional)
    pub node_id: Option<String>,
    /// Source IP address
    pub client_ip: String,
    /// Client User-Agent
    pub client_user_agent: Option<String>,
    /// Token identifier or authenticated subject
    pub auth_subject: String,
    /// Details of what was executed
    pub details: serde_json::Value,
    /// Recorded time (Unix epoch ms)
    pub created_at: i64,
}
