//! Normalized session event types (`UnifiedEventPayload`).
//!
//! Design: `docs/03-protocol-and-api.md` §1,
//! event sourcing / projection rules: `docs/02-database-schema.md` §0.3.
//!
//! # Persistence lifecycle (docs/01 §2.3)
//!
//! - Discrete events (`UserMessage` / `ToolCall` / `PermissionRequest` /
//!   `StatusChanged` etc.) are persisted immediately when they occur.
//! - Streaming events (`AgentMessage` / `AgentThought`) persist only the final
//!   event with `is_complete = true` when the turn ends (intermediate chunks are
//!   delivered as [`crate::common::StreamDeltaPayload`] in memory only and are
//!   never written to the DB).
//! - Session metadata (creation, title, bound agent, status) is also recorded as
//!   events in the event log, and the hub-side `sessions` table is a projection of
//!   them (no separate metadata synchronization path exists).

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{EventId, NodeId, ProjectId, SessionId};

use crate::common::{
    AttachmentMeta, ClientSource, CommandInfo, CompactionStatus, ConfigOptionInfo,
    ElicitationAction, ElicitationContent, ElicitationMode, ElicitationSchema, EventType, FileDiff,
    ModeInfo, PermissionOption, PlanEntry, RawJson, ResolvedBy, SessionStatus, ToolCallKind,
    ToolCallStatus, TurnStopReason, UsageCost,
};

/// Envelope of a session event (the unit of persistence and synchronization).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionEventEnvelope {
    /// Event ID (**UUID v7**). Used for idempotent application and deduplication.
    pub event_id: EventId,
    /// Session ID (**UUID v7**).
    pub session_id: SessionId,
    /// Sequence number inside the session (`1, 2, 3...`). **Only the running node
    /// assigns it** and there are no gaps. The logical order inside a session is
    /// always defined by this value.
    pub node_seq: u64,
    /// Creation time according to the node's local clock (Unix epoch ms).
    pub created_at: i64,
    /// The event body.
    pub payload: UnifiedEventPayload,
}

/// A batch of events with a cursor (shared by Client WS delivery and the history API).
///
/// `cursor` is the ingestion order of the **source store** (`node.db` /
/// `hub.db`). Clients persist the `cursor` of the last batch they received and
/// pass it as `Subscribe { since_cursor }` when reconnecting. A `cursor` is only
/// meaningful for the store it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct SessionEventBatch {
    /// Events (ascending `node_seq`)
    pub events: Vec<SessionEventEnvelope>,
    /// Cursor of the source store at the time of this batch
    pub cursor: u64,
}

impl SessionEventBatch {
    /// An empty batch (e.g. when a first subscription has no events yet).
    pub const fn empty(cursor: u64) -> Self {
        Self {
            events: Vec::new(),
            cursor,
        }
    }
}

/// The body of a normalized session event.
///
/// Based on the event model of ACP (`agent-client-protocol-schema`), with
/// terminal output and permission resolution events merged in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[specta(collect)]
pub enum UnifiedEventPayload {
    /// The first event right after a session is created (`node_seq = 1`).
    ///
    /// The source of the hub-side `sessions` projection. No separate metadata
    /// synchronization path exists.
    SessionCreated {
        /// ID of the running node. The running node fills in its own ID so that
        /// the hub-side projection can be rebuilt entirely from the event log.
        node_id: NodeId,
        /// Logical project ID
        project_id: ProjectId,
        /// Project display name (used by the hub to create unknown projects)
        project_name: String,
        /// Working directory (including the worktree path)
        local_path: String,
        /// Git branch at start-up (a snapshot)
        git_branch: Option<String>,
        /// Whether it runs inside a worktree
        is_worktree: bool,
        /// Agent ID
        agent_id: String,
        /// Session ID this session was forked from
        parent_session_id: Option<SessionId>,
        /// Initial title
        title: String,
    },
    /// Session title change (a rename from the UI/CLI; emitted once the command
    /// reaches the running node).
    SessionTitleChanged {
        /// New title
        title: String,
    },
    /// The `agent_session_id` becomes known, e.g. when the ACP session is established.
    SessionAgentBound {
        /// Session ID inside the agent
        agent_session_id: String,
    },
    /// A prompt submitted by the user (including slash commands).
    UserMessage {
        /// Prompt body
        text: String,
        /// Attached files
        attachments: Vec<AttachmentMeta>,
        /// Source (`cli` / `web` / `android` / `resume`)
        client_source: ClientSource,
    },
    /// A reply message from the agent.
    ///
    /// While streaming it is delivered as `LiveStreamDelta`, and only the final
    /// event with `is_complete = true` is persisted when the turn ends.
    AgentMessage {
        /// Message ID (corresponds to `StreamDeltaPayload::AgentMessageDelta`)
        message_id: String,
        /// Body text
        text: String,
        /// Whether it is complete (only `true` is persisted)
        is_complete: bool,
    },
    /// The reasoning process of the agent (thinking).
    ///
    /// Like `AgentMessage`, only the final event with `is_complete = true` is persisted.
    AgentThought {
        /// Thought ID (corresponds to `StreamDeltaPayload::AgentThoughtDelta`)
        thought_id: String,
        /// Body text
        text: String,
        /// Whether it is complete
        is_complete: bool,
    },
    /// A tool call and its state, including file diffs.
    ///
    /// State updates are expressed by appending events with the same
    /// `tool_call_id` (the `tool_update` event was removed).
    ToolCall {
        /// Tool call ID
        tool_call_id: String,
        /// Display title
        title: String,
        /// Kind (`read` / `edit` / `execute` / `search` / `other`)
        kind: ToolCallKind,
        /// Status (`pending` / `in_progress` / `completed` / `failed`)
        status: ToolCallStatus,
        /// Affected file paths
        locations: Vec<String>,
        /// Unified diff / before-after of a file change
        diff: Option<FileDiff>,
        /// Raw output (optional)
        raw_output: Option<String>,
    },
    /// The execution plan of the agent (ACP Plan).
    PlanUpdate {
        /// Plan entries
        entries: Vec<PlanEntry>,
    },
    /// A permission approval request from the agent
    /// (ACP `session/request_permission`).
    PermissionRequest {
        /// ACP request id
        request_id: String,
        /// Tool name (e.g. `terminal/create`)
        tool_name: String,
        /// Summary (e.g. `Run command: cargo test`)
        summary: String,
        /// Options (e.g. `allow_once` / `allow_always` / `reject`)
        options: Vec<PermissionOption>,
        /// Arguments and diff details
        details: RawJson,
    },
    /// The outcome of a permission request.
    ///
    /// `fxg node start` emits this event at the same time as it answers the agent, so
    /// that approval dialogs in other clients close automatically. Second and
    /// later answers are dropped per `request_id` and `ALREADY_RESOLVED` is returned.
    PermissionResolved {
        /// ACP request id
        request_id: String,
        /// The selected `option_id`
        selected_option_id: String,
        /// Who resolved it (`cli` / `web` / `android_push`)
        resolved_by: ResolvedBy,
    },
    /// A structured input request from the agent (ACP `elicitation/create`).
    ///
    /// The agent's "question" tool arrives as this event and the UI / CLI builds
    /// an answer form from `requested_schema`. Answers are appended as
    /// [`UnifiedEventPayload::ElicitationResolved`]
    /// (Phase 1 supports form mode only).
    ElicitationRequest {
        /// ACP elicitation id
        elicitation_id: String,
        /// Message shown to the user
        message: String,
        /// Request mode (`form` / `url`. Phase 1 supports `form` only)
        mode: ElicitationMode,
        /// JSON Schema of the request in form mode (`requestedSchema`)
        requested_schema: ElicitationSchema,
        /// Related tool call ID (optional)
        tool_call_id: Option<String>,
    },
    /// The outcome of a structured input request (accept / decline / cancel).
    ElicitationResolved {
        /// ACP elicitation id
        elicitation_id: String,
        /// The user's response action
        action: ElicitationAction,
        /// Answer content for `accept` (`None` on decline / cancel)
        content: Option<ElicitationContent>,
        /// Who resolved it (`cli` / `web` / `android_push`)
        resolved_by: ResolvedBy,
    },
    /// A change of the session's archive state (hidden from / restored to the list).
    ///
    /// Archiving is a reversible visibility flag; the event log and conversation
    /// content are kept (a permanent erase is
    /// [`UnifiedEventPayload::SessionDeleted`]).
    SessionArchived {
        /// `true` = archived, `false` = restored
        archived: bool,
    },
    /// Change of the session approval policy (`ask` / `auto_allow_all`).
    SessionPermissionPolicyChanged {
        /// New policy
        policy: crate::common::PermissionPolicy,
    },
    /// Deletion of a session (tombstone).
    ///
    /// Applying it purges the bodies of all events before it (irreversible). The
    /// event itself remains as a tombstone so that the deletion propagates through
    /// the outbox / resync and references from forked sessions stay consistent.
    SessionDeleted {},
    /// An output chunk of ACP `terminal/*` or a PTY (persisted).
    ///
    /// It is, however, excluded from the FTS5 `searchable_text` (binary data).
    TerminalOutput {
        /// Terminal ID
        terminal_id: String,
        /// Executed command
        command: String,
        /// Raw bytes including ANSI escapes (Base64)
        data_b64: String,
        /// Exit code (once execution finished)
        exit_code: Option<i32>,
    },
    /// A stdin write to an interactive command (Web UI -> agent PTY).
    ///
    /// Keystrokes are ephemeral and are not persisted to the event log (raw
    /// delivery through the PTY WS is also possible). See
    /// [`UnifiedEventPayload::is_persistable`].
    TerminalInput {
        /// Terminal ID
        terminal_id: String,
        /// User keystrokes (Base64)
        data_b64: String,
    },
    /// Notification of updated modes, slash commands and configuration (persisted).
    ///
    /// The update source of the hub-side `sessions.current_mode` /
    /// `available_modes_json` / `available_commands_json` / `config_options_json`
    /// projections.
    CapabilitiesUpdated {
        /// Current mode (`None` keeps the existing value)
        current_mode: Option<String>,
        /// Selectable modes
        available_modes: Vec<ModeInfo>,
        /// Available slash commands
        available_commands: Vec<CommandInfo>,
        /// Configuration entries (model selection etc.)
        config_options: Vec<ConfigOptionInfo>,
        /// Whether the agent supports ACP-native `session/fork` + `session/load`.
        /// `None` keeps the existing value (events recorded before this field
        /// existed).
        #[serde(default)]
        fork_supported: Option<bool>,
    },
    /// A progress log line of ephemeral VM provisioning (`stderr`).
    #[serde(alias = "bootstrap_log")]
    ProvisioningLog {
        /// A single log line
        line: String,
    },
    /// An update of the context usage and accumulated cost (the `usage_update` of
    /// ACP `session/update`). The update source of the hub-side `sessions.usage_json`
    /// projection.
    UsageUpdated {
        /// Number of tokens currently occupying the context
        used_tokens: u64,
        /// Total context window size (in tokens)
        context_size: u64,
        /// Accumulated cost (only when reported by the agent)
        cost: Option<UsageCost>,
    },
    /// A turn was cut off for a reason other than normal completion (ACP `StopReason`).
    ///
    /// `EndTurn` / `Cancelled` end the turn normally and are not turned into an
    /// event. The UI / CLI shows this as a system line in the timeline and
    /// derives its label from `reason` (no message text is carried).
    TurnEnded {
        /// Why the turn was cut off
        reason: TurnStopReason,
    },
    /// Progress of context compaction (persisted).
    ///
    /// Normalizes `session.compaction.started` / `.ended` of the `opencode2`
    /// bridge (started with `/compact` or `ControlSession(Compact)`. A compaction
    /// runs as one turn, so `StatusChanged` separately updates the running
    /// indicator). Streaming of the summary body (`session.compaction.delta`) is
    /// ephemeral and not turned into events (the summary stays in the agent's
    /// history).
    CompactionUpdated {
        /// Progress status
        status: CompactionStatus,
        /// Extra information such as the failure reason (optional)
        detail: Option<String>,
    },
    /// A change of the session status. The update source of the hub-side
    /// `sessions.status` projection.
    StatusChanged {
        /// New status
        status: SessionStatus,
        /// Message for the error case
        error_message: Option<String>,
    },
}

impl UnifiedEventPayload {
    /// Returns the kind string stored in `session_events.event_type`.
    ///
    /// The values match the `event_type` definitions in
    /// `docs/02-database-schema.md` §1.
    pub const fn event_type(&self) -> EventType {
        match self {
            Self::SessionCreated { .. } => EventType::SessionCreated,
            Self::SessionTitleChanged { .. } => EventType::SessionTitleChanged,
            Self::SessionAgentBound { .. } => EventType::SessionAgentBound,
            Self::UserMessage { .. } => EventType::UserMessage,
            Self::AgentMessage { .. } => EventType::AgentMessage,
            Self::AgentThought { .. } => EventType::AgentThought,
            Self::ToolCall { .. } => EventType::ToolCall,
            Self::PlanUpdate { .. } => EventType::Plan,
            Self::PermissionRequest { .. } => EventType::PermissionRequest,
            Self::PermissionResolved { .. } => EventType::PermissionResolved,
            Self::ElicitationRequest { .. } => EventType::ElicitationRequest,
            Self::ElicitationResolved { .. } => EventType::ElicitationResolved,
            Self::SessionArchived { .. } => EventType::SessionArchived,
            Self::SessionPermissionPolicyChanged { .. } => {
                EventType::SessionPermissionPolicyChanged
            }
            Self::SessionDeleted {} => EventType::SessionDeleted,
            Self::TerminalOutput { .. } => EventType::TerminalOutput,
            Self::TerminalInput { .. } => EventType::TerminalInput,
            Self::CapabilitiesUpdated { .. } => EventType::CapabilitiesUpdated,
            Self::ProvisioningLog { .. } => EventType::ProvisioningLog,
            Self::UsageUpdated { .. } => EventType::UsageUpdated,
            Self::TurnEnded { .. } => EventType::TurnEnded,
            Self::CompactionUpdated { .. } => EventType::CompactionUpdated,
            Self::StatusChanged { .. } => EventType::StatusChanged,
        }
    }

    /// Whether this payload may be persisted to the event log.
    ///
    /// `TerminalInput` (keystrokes) is ephemeral and returns `false`.
    /// Streaming `AgentMessage` / `AgentThought` (`is_complete = false`) are not
    /// persisted either.
    pub const fn is_persistable(&self) -> bool {
        match self {
            Self::TerminalInput { .. } => false,
            Self::AgentMessage { is_complete, .. } | Self::AgentThought { is_complete, .. } => {
                *is_complete
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{
        ClientSource, CommandInfo, ConfigOptionInfo, ConfigOptionValue, ElicitationMode, ModeInfo,
        ResolvedBy,
    };

    fn sample_envelope(payload: UnifiedEventPayload) -> SessionEventEnvelope {
        SessionEventEnvelope {
            event_id: crate::util::uuid_v7().into(),
            session_id: crate::util::uuid_v7().into(),
            node_seq: 1,
            created_at: crate::util::now_ms(),
            payload,
        }
    }

    #[test]
    fn payload_serialization_uses_type_and_data_tags() {
        let payload = UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Running,
            error_message: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["type"], "status_changed");
        assert_eq!(json["data"]["status"], "running");
        assert!(json["data"]["error_message"].is_null());

        let decoded: UnifiedEventPayload = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn legacy_bootstrap_log_payload_decodes_as_provisioning_log() {
        let legacy = serde_json::json!({
            "type": "bootstrap_log",
            "data": { "line": "installing toolchain" },
        });
        let decoded: UnifiedEventPayload = serde_json::from_value(legacy).unwrap();
        assert_eq!(
            decoded,
            UnifiedEventPayload::ProvisioningLog {
                line: "installing toolchain".to_owned(),
            }
        );
        let json = serde_json::to_value(&decoded).unwrap();
        assert_eq!(json["type"], "provisioning_log");
        assert_eq!(decoded.event_type(), EventType::ProvisioningLog);
    }

    #[test]
    fn envelope_roundtrips_through_json() {
        let envelope = sample_envelope(UnifiedEventPayload::UserMessage {
            text: "Fix the failing tests".to_owned(),
            attachments: vec![],
            client_source: ClientSource::Cli,
        });
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: SessionEventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn capabilities_updated_roundtrips() {
        let payload = UnifiedEventPayload::CapabilitiesUpdated {
            current_mode: Some("code".to_owned()),
            available_modes: vec![ModeInfo {
                mode_id: "plan".to_owned(),
                name: "Plan".to_owned(),
                description: None,
            }],
            available_commands: vec![CommandInfo {
                name: "/review".to_owned(),
                description: "Review the changes".to_owned(),
                input_hint: None,
            }],
            config_options: vec![ConfigOptionInfo {
                key: "model".to_owned(),
                name: "Model".to_owned(),
                description: Some("Model used for the session".to_owned()),
                category: Some(crate::common::ConfigOptionCategory::Model),
                current_value: ConfigOptionValue::Select("sonnet".to_owned()),
                options: vec![
                    ConfigOptionValue::Select("sonnet".to_owned()),
                    ConfigOptionValue::Select("haiku".to_owned()),
                ],
            }],
            fork_supported: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["type"], "capabilities_updated");
        assert_eq!(json["data"]["current_mode"], "code");

        let decoded: UnifiedEventPayload = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn elicitation_events_roundtrip() {
        let payload = UnifiedEventPayload::ElicitationRequest {
            elicitation_id: "elic-1".to_owned(),
            message: "Which strategy should we use?".to_owned(),
            mode: ElicitationMode::Form,
            requested_schema: serde_json::from_value(serde_json::json!({
                "type": "object",
                "properties": {
                    "strategy": { "type": "string", "enum": ["a", "b"] }
                },
                "required": ["strategy"]
            }))
            .expect("schema"),
            tool_call_id: Some("tool-1".to_owned()),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["type"], "elicitation_request");
        assert_eq!(json["data"]["mode"], "form");
        assert_eq!(
            serde_json::from_value::<UnifiedEventPayload>(json).unwrap(),
            payload
        );
        assert!(payload.is_persistable());

        let resolved = UnifiedEventPayload::ElicitationResolved {
            elicitation_id: "elic-1".to_owned(),
            action: ElicitationAction::Accept,
            content: Some(
                serde_json::from_value(serde_json::json!({ "strategy": "a" })).expect("content"),
            ),
            resolved_by: ResolvedBy::Cli,
        };
        let json = serde_json::to_value(&resolved).unwrap();
        assert_eq!(json["type"], "elicitation_resolved");
        assert_eq!(json["data"]["action"], "accept");
        assert_eq!(
            serde_json::from_value::<UnifiedEventPayload>(json).unwrap(),
            resolved
        );
    }

    #[test]
    fn event_type_matches_db_vocabulary() {
        assert_eq!(
            UnifiedEventPayload::SessionCreated {
                node_id: "n".into(),
                project_id: "p".into(),
                project_name: "P".into(),
                local_path: "/tmp".into(),
                git_branch: None,
                is_worktree: false,
                agent_id: "opencode".into(),
                parent_session_id: None,
                title: "t".into(),
            }
            .event_type(),
            EventType::SessionCreated
        );
        assert_eq!(
            UnifiedEventPayload::PlanUpdate { entries: vec![] }.event_type(),
            EventType::Plan
        );
        assert_eq!(
            UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Idle,
                error_message: None,
            }
            .event_type(),
            EventType::StatusChanged
        );
    }

    #[test]
    fn persistability_rules() {
        assert!(
            !UnifiedEventPayload::TerminalInput {
                terminal_id: "t".into(),
                data_b64: "aGk=".into(),
            }
            .is_persistable()
        );
        assert!(
            !UnifiedEventPayload::AgentMessage {
                message_id: "m".into(),
                text: "partial".into(),
                is_complete: false,
            }
            .is_persistable()
        );
        assert!(
            UnifiedEventPayload::AgentMessage {
                message_id: "m".into(),
                text: "done".into(),
                is_complete: true,
            }
            .is_persistable()
        );
        assert!(
            UnifiedEventPayload::TerminalOutput {
                terminal_id: "t".into(),
                command: "cargo test".into(),
                data_b64: "".into(),
                exit_code: Some(0),
            }
            .is_persistable()
        );
    }

    #[test]
    fn batch_cursor_helper() {
        let batch = SessionEventBatch::empty(42);
        assert!(batch.events.is_empty());
        assert_eq!(batch.cursor, 42);
    }
}
