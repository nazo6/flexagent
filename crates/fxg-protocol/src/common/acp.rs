//! ACP interaction types: permissions, elicitations, modes, plans and config.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use specta::Type;

use super::{EnumParseError, RawJson};
use crate::{NodeId, SessionId};

crate::open_string_enum! {
    /// Type discriminator of an elicitation form schema.
    pub enum ElicitationSchemaType {
        /// The schema describes an object
        Object => "object",
    }
}

/// Object of a restricted elicitation form schema (ACP `requestedSchema`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ElicitationSchema {
    /// Type discriminator. ACP always uses `object`.
    #[serde(rename = "type", default = "default_elicitation_schema_type")]
    pub schema_type: ElicitationSchemaType,
    /// Optional title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Property definitions.
    #[serde(default)]
    pub properties: BTreeMap<String, ElicitationProperty>,
    /// Required property names (`None` and `null` mean none).
    #[serde(default)]
    pub required: Option<Vec<String>>,
    /// Unknown keys are preserved for forward compatibility.
    #[serde(flatten)]
    pub extra: BTreeMap<String, RawJson>,
}

const fn default_elicitation_schema_type() -> ElicitationSchemaType {
    ElicitationSchemaType::Object
}

crate::open_string_enum! {
    /// Restricted JSON Schema type of an elicitation form field.
    pub enum ElicitationPropertyKind {
        /// String (or single-select enum)
        String => "string",
        /// Integer
        Integer => "integer",
        /// Floating-point number
        Number => "number",
        /// Boolean
        Boolean => "boolean",
        /// Multi-select array of strings
        Array => "array",
    }
}

/// One property of an elicitation form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ElicitationProperty {
    /// Property type.
    #[serde(rename = "type")]
    pub kind: ElicitationPropertyKind,
    /// Optional title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional default value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<RawJson>,
    /// Allowed string values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub r#enum: Vec<String>,
    /// Allowed values with display titles.
    #[serde(default, rename = "oneOf", skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<ElicitationOption>,
    /// Array item schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<ElicitationArrayItems>,
    /// Minimum integer value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,
    /// Maximum integer value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<i64>,
    /// Unknown keys are preserved for forward compatibility.
    #[serde(flatten)]
    pub extra: BTreeMap<String, RawJson>,
}

/// One option of a single-select elicitation field (`oneOf`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ElicitationOption {
    /// Submitted value.
    #[serde(rename = "const")]
    pub const_value: String,
    /// Display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Unknown keys are preserved for forward compatibility.
    #[serde(flatten)]
    pub extra: BTreeMap<String, RawJson>,
}

/// Item schema of a multi-select elicitation field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ElicitationArrayItems {
    /// Item type (ACP uses `string`).
    #[serde(rename = "type", default = "default_elicitation_item_type")]
    pub kind: ElicitationPropertyKind,
    /// Allowed values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub r#enum: Vec<String>,
    /// Unknown keys are preserved for forward compatibility.
    #[serde(flatten)]
    pub extra: BTreeMap<String, RawJson>,
}

const fn default_elicitation_item_type() -> ElicitationPropertyKind {
    ElicitationPropertyKind::String
}

/// One value in an elicitation answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(untagged)]
#[specta(collect)]
pub enum ElicitationValue {
    /// String value
    String(String),
    /// Integer value
    Integer(i64),
    /// Floating-point number value
    Number(f64),
    /// Boolean value
    Boolean(bool),
    /// String array (multi-select)
    StringArray(Vec<String>),
    /// Future / custom value, preserved as-is
    Unknown(RawJson),
}

/// Answers to an elicitation form (`accept` only).
pub type ElicitationContent = BTreeMap<String, ElicitationValue>;

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
    /// String representation used in JSON.
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

crate::open_string_enum! {
    /// Kind of a tool call (ACP `ToolKind`).
    pub enum ToolCallKind {
        /// Read a file or resource
        Read => "read",
        /// Edit / delete / move a file
        Edit => "edit",
        /// Search files or content
        Search => "search",
        /// Execute a command
        Execute => "execute",
        /// Any other tool
        Other => "other",
    }
}

crate::open_string_enum! {
    /// Status of a tool call (ACP `ToolCallStatus`).
    pub enum ToolCallStatus {
        /// Waiting to start
        Pending => "pending",
        /// Running
        InProgress => "in_progress",
        /// Finished successfully
        Completed => "completed",
        /// Failed
        Failed => "failed",
    }
}

crate::open_string_enum! {
    /// Status of a plan entry (ACP `PlanEntryStatus`).
    pub enum PlanEntryStatus {
        /// Not started
        Pending => "pending",
        /// In progress
        InProgress => "in_progress",
        /// Completed
        Completed => "completed",
    }
}

crate::open_string_enum! {
    /// Mode of an elicitation request (ACP `ElicitationMode`).
    pub enum ElicitationMode {
        /// Structured form
        Form => "form",
        /// URL-based interaction
        Url => "url",
    }
}

/// An execution plan entry of the agent (ACP Plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PlanEntry {
    /// Entry ID
    pub id: String,
    /// Display title
    pub title: String,
    /// Status (`pending` / `in_progress` / `completed`)
    pub status: PlanEntryStatus,
}

crate::open_string_enum! {
    /// Kind of a permission approval option (ACP `PermissionOptionKind`).
    pub enum PermissionOptionKind {
        /// Allow this request once
        AllowOnce => "allow_once",
        /// Allow this kind of request for the rest of the session
        AllowAlways => "allow_always",
        /// Reject this request once
        RejectOnce => "reject_once",
        /// Reject this kind of request for the rest of the session
        RejectAlways => "reject_always",
    }
}

crate::open_string_enum! {
    /// Per-session approval policy.
    pub enum PermissionPolicy {
        /// Ask the user for every request (default)
        Ask => "ask",
        /// Automatically approve requests using the agent's allow options
        AutoAllowAll => "auto_allow_all",
    }
}

#[allow(
    clippy::derivable_impls,
    reason = "open_string_enum does not support a #[default] variant marker"
)]
impl Default for PermissionPolicy {
    fn default() -> Self {
        Self::Ask
    }
}

impl PermissionOptionKind {
    /// Whether this option allows the request.
    ///
    /// Unknown values fall back to the `allow*` naming convention.
    pub fn is_allow(&self) -> bool {
        match self {
            Self::AllowOnce | Self::AllowAlways => true,
            Self::RejectOnce | Self::RejectAlways => false,
            Self::Unknown(raw) => raw.starts_with("allow"),
        }
    }

    /// Whether this option rejects the request.
    ///
    /// Unknown values fall back to the `reject*` / `deny*` naming convention.
    pub fn is_reject(&self) -> bool {
        match self {
            Self::RejectOnce | Self::RejectAlways => true,
            Self::AllowOnce | Self::AllowAlways => false,
            Self::Unknown(raw) => raw.starts_with("reject") || raw.starts_with("deny"),
        }
    }

    /// Whether this option applies for the rest of the session.
    ///
    /// Unknown values fall back to the `*always` naming convention.
    pub fn is_always(&self) -> bool {
        match self {
            Self::AllowAlways | Self::RejectAlways => true,
            Self::AllowOnce | Self::RejectOnce => false,
            Self::Unknown(raw) => raw.contains("always"),
        }
    }

    /// Whether this option applies only to the current request.
    pub fn is_once(&self) -> bool {
        !self.is_always()
    }
}

/// An option of a permission approval (ACP `PermissionOption`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PermissionOption {
    /// Option ID (the value given to `PermissionResolved.selected_option_id`)
    pub option_id: String,
    /// Display name
    pub name: String,
    /// Kind (`allow_once` / `allow_always` / `reject_once` / `reject_always`)
    pub kind: PermissionOptionKind,
}

impl PermissionOption {
    /// Whether this option rejects the request.
    pub fn is_reject(&self) -> bool {
        self.kind.is_reject()
    }
}

crate::open_string_enum! {
    /// Who resolved a permission / elicitation request.
    pub enum ResolvedBy {
        /// The CLI
        Cli => "cli",
        /// The Web UI
        Web => "web",
        /// An Android push notification
        AndroidPush => "android_push",
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

/// Value of an agent configuration entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(untagged)]
#[specta(collect)]
pub enum ConfigOptionValue {
    /// Select / model ID
    Select(String),
    /// Boolean toggle
    Boolean(bool),
    /// Agent-specific value preserved as-is (e.g. `{ providerID, id, name }`)
    Unknown(RawJson),
}

crate::open_string_enum! {
    /// Semantic category of an agent configuration option (UX hint).
    pub enum ConfigOptionCategory {
        /// Session mode selector
        Mode => "mode",
        /// Model selector
        Model => "model",
        /// Model-related configuration parameter
        ModelConfig => "model_config",
        /// Thought / reasoning level selector
        ThoughtLevel => "thought_level",
    }
}

/// Information about an agent configuration entry (model selection etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ConfigOptionInfo {
    /// Configuration key
    pub key: String,
    /// Display name
    pub name: String,
    /// Optional description reported by the agent
    pub description: Option<String>,
    /// Semantic category reported by the agent (UX hint only)
    pub category: Option<ConfigOptionCategory>,
    /// Current value
    pub current_value: ConfigOptionValue,
    /// List of selectable values
    pub options: Vec<ConfigOptionValue>,
}

/// Permission request projection exposed to clients (the global approval inbox).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct PermissionRequestEntry {
    /// ACP request id
    pub request_id: String,
    /// Target session ID
    pub session_id: SessionId,
    /// Target node ID
    pub node_id: NodeId,
    /// Tool name (e.g. `terminal/create`)
    pub tool_name: String,
    /// Summary (e.g. `Run command: cargo test`)
    pub summary: String,
    /// Available options
    pub options: Vec<PermissionOption>,
    /// Arguments and diff details
    pub details: RawJson,
    /// Status
    pub status: PermissionRequestStatus,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
    /// Resolution time (Unix epoch ms)
    pub resolved_at: Option<i64>,
    /// Who resolved it (`cli` / `web` / `android_push`)
    pub resolved_by: Option<ResolvedBy>,
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
    pub session_id: SessionId,
    /// Target node ID
    pub node_id: NodeId,
    /// Message shown to the user
    pub message: String,
    /// Request mode (`form` / `url`. Phase 1 supports `form` only)
    pub mode: ElicitationMode,
    /// JSON Schema of the request in form mode (`requestedSchema`)
    pub requested_schema: ElicitationSchema,
    /// Related tool call ID (optional)
    pub tool_call_id: Option<String>,
    /// Answer content for `accept` (`None` while unanswered or on decline / cancel)
    pub content: Option<ElicitationContent>,
    /// Status
    pub status: ElicitationRequestStatus,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
    /// Resolution time (Unix epoch ms)
    pub resolved_at: Option<i64>,
    /// Who resolved it (`cli` / `web` / `android_push`)
    pub resolved_by: Option<ResolvedBy>,
}
