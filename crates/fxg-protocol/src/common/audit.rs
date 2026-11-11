//! Persisted event kinds, search hits and audit logs.

use serde::{Deserialize, Serialize};
use specta::Type;

use super::RawJson;
use crate::{EventId, NodeId, SessionId};

crate::open_string_enum! {
    /// Type of a persisted session event (`session_events.event_type`).
    pub enum EventType {
        /// Session created
        SessionCreated => "session_created",
        /// Session title changed
        SessionTitleChanged => "session_title_changed",
        /// Agent session bound
        SessionAgentBound => "session_agent_bound",
        /// User message
        UserMessage => "user_message",
        /// Agent message
        AgentMessage => "agent_message",
        /// Agent thought
        AgentThought => "agent_thought",
        /// Tool call
        ToolCall => "tool_call",
        /// Plan update
        Plan => "plan",
        /// Permission request
        PermissionRequest => "permission_request",
        /// Permission resolved
        PermissionResolved => "permission_resolved",
        /// Elicitation request
        ElicitationRequest => "elicitation_request",
        /// Elicitation resolved
        ElicitationResolved => "elicitation_resolved",
        /// Session archived
        SessionArchived => "session_archived",
        /// Session permission policy changed
        SessionPermissionPolicyChanged => "session_permission_policy_changed",
        /// Session deleted
        SessionDeleted => "session_deleted",
        /// Terminal output
        TerminalOutput => "terminal_output",
        /// Terminal input
        TerminalInput => "terminal_input",
        /// Capabilities updated
        CapabilitiesUpdated => "capabilities_updated",
        /// Provisioning log
        ProvisioningLog => "provisioning_log",
        /// Usage updated
        UsageUpdated => "usage_updated",
        /// Turn ended
        TurnEnded => "turn_ended",
        /// Compaction updated
        CompactionUpdated => "compaction_updated",
        /// Session status changed
        StatusChanged => "status_change",
    }
}

/// A hit of the FTS5 full-text search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SearchHit {
    /// Target session ID
    pub session_id: SessionId,
    /// Target event ID
    pub event_id: EventId,
    /// Sequence number inside the session
    pub node_seq: u64,
    /// Event type
    pub event_type: EventType,
    /// Snippet (an excerpt containing the match)
    pub snippet: String,
    /// Event creation time (Unix epoch ms)
    pub created_at: i64,
}

crate::open_string_enum! {
    /// Kind of an audit log action.
    pub enum AuditAction {
        /// Session started
        SessionStart => "session_start",
        /// Session resumed
        SessionResume => "session_resume",
        /// Session archived / restored
        SessionArchive => "session_archive",
        /// Session deleted
        SessionDelete => "session_delete",
        /// Session renamed
        SessionRename => "session_rename",
        /// Permission resolved
        PermissionResolved => "permission_resolved",
        /// Elicitation resolved
        ElicitationResolved => "elicitation_resolved",
        /// Web PTY spawned
        PtySpawn => "pty_spawn",
        /// Emergency kill switch
        KillSwitch => "kill_switch",
        /// Worktree managed
        WorktreeManage => "worktree_manage",
        /// Worktree pruned
        WorktreePrune => "worktree_prune",
        /// Project binding changed
        ProjectLink => "project_link",
        /// Agent managed
        AgentManage => "agent_manage",
        /// Node pairing token issued / revoked
        NodeTokenManage => "node_token_manage",
        /// Web access token rotated
        AuthTokenRotate => "auth_token_rotate",
        /// Settings updated
        SettingsUpdate => "settings_update",
        /// Secret created / updated / deleted / rotated
        SecretManage => "secret_manage",
    }
}

/// An audit log entry (`GET /api/v1/audit/logs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct AuditLogEntry {
    /// Log ID
    pub id: i64,
    /// Action kind (`session_start` / `permission_resolved` / `pty_spawn` / `kill_switch` / `worktree_manage`)
    pub action: AuditAction,
    /// Related session ID (optional)
    pub session_id: Option<SessionId>,
    /// Target node ID (optional)
    pub node_id: Option<NodeId>,
    /// Source IP address
    pub client_ip: String,
    /// Client User-Agent
    pub client_user_agent: Option<String>,
    /// Token identifier or authenticated subject
    pub auth_subject: String,
    /// Details of what was executed
    pub details: RawJson,
    /// Recorded time (Unix epoch ms)
    pub created_at: i64,
}
