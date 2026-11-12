//! Session state, usage, prompts and control actions.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use specta::Type;

use super::EnumParseError;
use super::acp::{ConfigOptionValue, PermissionPolicy, ToolCallStatus};
use crate::{NodeId, ProjectId, SessionId};

/// Session status (same values as `sessions.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum SessionStatus {
    /// Provisioning an ephemeral VM
    Provisioning,
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

/// Scope of native session forking supported by the agent.
///
/// Stored in `sessions.fork_scope`; an unknown database value fails to decode
/// instead of being treated as a default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum ForkScope {
    /// Native forking is not supported.
    #[default]
    None,
    /// Only the current end of the conversation can be forked.
    Last,
    /// Any completed message boundary can be forked.
    Message,
}

impl ForkScope {
    /// String representation used in the DB / JSON.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Last => "last",
            Self::Message => "message",
        }
    }

    /// Whether the agent supports forking at all.
    pub const fn is_supported(self) -> bool {
        !matches!(self, Self::None)
    }
}

impl fmt::Display for ForkScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ForkScope {
    type Err = EnumParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "last" => Ok(Self::Last),
            "message" => Ok(Self::Message),
            other => Err(EnumParseError::new("fork scope", other)),
        }
    }
}

/// Execution log of a worktree creation hook.
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
/// The standard ACP has no API to start a compaction, so only drivers or
/// extensions with a native API emit it.
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
/// events. Both the local node and the hub return this same type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionSummary {
    /// Session ID (UUID v7)
    pub session_id: SessionId,
    /// Logical project ID (e.g. `github.com/nazo6/flexagent`)
    pub project_id: ProjectId,
    /// ID of the node running the session
    pub node_id: NodeId,
    /// Working directory (including the worktree path)
    pub local_path: String,
    /// Snapshot taken at start-up (live values come from the worktree API)
    pub git_branch: Option<String>,
    /// Whether it runs inside a worktree
    pub is_worktree: bool,
    /// Agent ID (`opencode` / a custom ID, etc.)
    pub agent_id: String,
    /// Session ID inside the agent (fixed by the `SessionAgentBound` event)
    pub agent_session_id: Option<String>,
    /// Session ID this session was forked from
    pub parent_session_id: Option<SessionId>,
    /// Session title
    pub title: String,
    /// Session status
    pub status: SessionStatus,
    /// Current ACP SessionMode (e.g. `code`, `plan`)
    pub current_mode: Option<String>,
    /// Approval policy (`ask` / `auto_allow_all`)
    #[serde(default)]
    pub permission_policy: PermissionPolicy,
    /// Latest context usage and accumulated cost (projection of the `UsageUpdated` event).
    ///
    /// `None` means nothing has been received yet (including the case where the
    /// agent never sends `usage_update`).
    pub usage: Option<SessionUsage>,
    /// Latest persisted `node_seq` (node) / highest `node_seq` applied to the projection (hub)
    pub last_node_seq: u64,
    /// Scope of ACP-native session forking supported by the agent.
    #[serde(default)]
    pub fork_scope: ForkScope,
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
        /// Subagent that produced this delta (`None` = the parent session)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_id: Option<String>,
    },
    /// Incremental text of the reasoning process
    AgentThoughtDelta {
        /// Thought ID (corresponds to `AgentThought.thought_id`)
        thought_id: String,
        /// Appended text
        text_delta: String,
        /// Subagent that produced this delta (`None` = the parent session)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_id: Option<String>,
    },
    /// Incremental text of a message between the parent session and a subagent
    SubagentMessageDelta {
        /// Message ID (corresponds to `SubagentMessage.message_id`)
        message_id: String,
        /// Session that sent the message
        sender_session_id: String,
        /// Session that received the message
        recipient_session_id: String,
        /// Appended text
        text_delta: String,
    },
    /// Progress update of a tool call
    ToolCallProgress {
        /// Tool call ID
        tool_call_id: String,
        /// Updated status
        status: ToolCallStatus,
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

/// Initial session configuration applied right after `session/new`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct InitialConfig {
    /// Configuration key (`model`, `thought_level`, ...)
    pub key: String,
    /// Value in FXG's normalized representation
    pub value: ConfigOptionValue,
}

/// A prompt sent to an agent: text plus optional attachments.
///
/// Shared by Client WS, Node⇔Hub messages and REST so the same payload
/// is validated once and recorded in `UserMessage`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PromptInput {
    /// Prompt body (may be empty when only attachments are sent)
    pub text: String,
    /// Attached images / files
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentMeta>,
}

impl PromptInput {
    /// Builds a text-only prompt.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
        }
    }

    /// Whether the prompt has neither text nor attachments.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty()
    }
}

crate::open_string_enum! {
    /// Where a prompt / command came from.
    pub enum ClientSource {
        /// The CLI
        Cli => "cli",
        /// The Web UI
        Web => "web",
        /// An Android PWA client
        Android => "android",
        /// History injected when resuming a session
        Resume => "resume",
    }
}

crate::open_string_enum! {
    /// Role of a conversation history item injected on resume.
    pub enum HistoryRole {
        /// A user utterance
        User => "user",
        /// An agent reply
        Agent => "agent",
        /// A tool result
        Tool => "tool",
        /// An assistant message
        Assistant => "assistant",
    }
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
        value: ConfigOptionValue,
    },
    /// Change the approval policy (ask / auto-approve)
    SetPermissionPolicy {
        /// New policy
        policy: PermissionPolicy,
    },
    /// Interrupt the current turn
    Cancel,
    /// End the session and return the agent process to the reuse pool
    Kill,
    /// Force-terminate the session and its process tree
    Dispose,
}
