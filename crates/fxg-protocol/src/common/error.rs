//! Shared error, recovery, warning and command-outcome contract.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use specta::Type;

use super::{EnumParseError, RawJson};
use crate::{CommandId, NodeId, ProjectId, SessionId};

/// Shared error code (used by both `CommandResult` and REST error responses).
///
/// The code is the machine-readable contract: clients use it as an i18n key and
/// to decide which recovery action to offer. `ErrorCode::Internal` must only be
/// used for genuinely unexpected failures; known failures get a dedicated code.
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
    /// The agent does not support the ACP-native `session/fork` + `session/load`
    /// pair, so the session cannot be forked.
    ForkUnsupported,
    /// The agent requires authentication before a session can be created.
    /// The advertised methods are carried in [`ErrorInfo::details`].
    AuthRequired,
    /// Remote PTY is disabled by the security policy
    PtyDisabled,
    /// Duplicate `command_id` submission (double tap / WS resend)
    CommandDuplicate,
    /// The target was not found
    NotFound,
    /// Internal error (a bug or an unclassified failure)
    Internal,
    /// The configuration file is invalid or cannot be read
    ConfigInvalid,
    /// `expected_revision` did not match the stored revision (optimistic lock conflict)
    RevisionMismatch,
    /// A referenced secret value does not exist
    SecretMissing,
    /// The secret is still referenced by an agent or Git credential and cannot be deleted
    SecretInUse,
    /// The secret ID or value is invalid
    SecretInvalid,
    /// The requested agent is not installed / discoverable on the node
    AgentNotInstalled,
    /// The agent process could not be launched or initialized
    AgentLaunchFailed,
    /// The agent violated the ACP protocol or the turn failed inside the agent
    AgentProtocolError,
    /// The `git` executable is not available
    GitUnavailable,
    /// A `git` command failed
    GitFailed,
    /// The worktree is in a conflicting state (branch checked out elsewhere, etc.)
    WorktreeConflict,
    /// The outbox has pending events and synchronization is degraded
    SyncBacklog,
    /// The database is corrupt or a stored value cannot be decoded
    DbCorrupt,
    /// Provisioning an ephemeral VM / sandbox failed
    ProvisionerFailed,
    /// Incompatible node / server / schema versions
    VersionMismatch,
    /// An operation timed out
    Timeout,
    /// The device is out of disk space
    DiskFull,
    /// The operation was denied by the file system / OS permissions
    PermissionDenied,
    /// A generic I/O error
    IoError,
    /// A PTY operation failed
    PtyFailed,
    /// The local or central server failed
    ServerError,
    /// The operation was cancelled
    Cancelled,
}

impl ErrorCode {
    /// Every variant (contract tests and message catalogs iterate this list).
    pub const ALL: &'static [Self] = &[
        Self::Unauthorized,
        Self::Forbidden,
        Self::NodeOffline,
        Self::AlreadyResolved,
        Self::Busy,
        Self::InvalidState,
        Self::ResumeRequired,
        Self::ForkUnsupported,
        Self::AuthRequired,
        Self::PtyDisabled,
        Self::CommandDuplicate,
        Self::NotFound,
        Self::Internal,
        Self::ConfigInvalid,
        Self::RevisionMismatch,
        Self::SecretMissing,
        Self::SecretInUse,
        Self::SecretInvalid,
        Self::AgentNotInstalled,
        Self::AgentLaunchFailed,
        Self::AgentProtocolError,
        Self::GitUnavailable,
        Self::GitFailed,
        Self::WorktreeConflict,
        Self::SyncBacklog,
        Self::DbCorrupt,
        Self::ProvisionerFailed,
        Self::VersionMismatch,
        Self::Timeout,
        Self::DiskFull,
        Self::PermissionDenied,
        Self::IoError,
        Self::PtyFailed,
        Self::ServerError,
        Self::Cancelled,
    ];

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
            Self::ForkUnsupported => "FORK_UNSUPPORTED",
            Self::AuthRequired => "AUTH_REQUIRED",
            Self::PtyDisabled => "PTY_DISABLED",
            Self::CommandDuplicate => "COMMAND_DUPLICATE",
            Self::NotFound => "NOT_FOUND",
            Self::Internal => "INTERNAL",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::RevisionMismatch => "REVISION_MISMATCH",
            Self::SecretMissing => "SECRET_MISSING",
            Self::SecretInUse => "SECRET_IN_USE",
            Self::SecretInvalid => "SECRET_INVALID",
            Self::AgentNotInstalled => "AGENT_NOT_INSTALLED",
            Self::AgentLaunchFailed => "AGENT_LAUNCH_FAILED",
            Self::AgentProtocolError => "AGENT_PROTOCOL_ERROR",
            Self::GitUnavailable => "GIT_UNAVAILABLE",
            Self::GitFailed => "GIT_FAILED",
            Self::WorktreeConflict => "WORKTREE_CONFLICT",
            Self::SyncBacklog => "SYNC_BACKLOG",
            Self::DbCorrupt => "DB_CORRUPT",
            Self::ProvisionerFailed => "PROVISIONER_FAILED",
            Self::VersionMismatch => "VERSION_MISMATCH",
            Self::Timeout => "TIMEOUT",
            Self::DiskFull => "DISK_FULL",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::IoError => "IO_ERROR",
            Self::PtyFailed => "PTY_FAILED",
            Self::ServerError => "SERVER_ERROR",
            Self::Cancelled => "CANCELLED",
        }
    }

    /// Whether retrying the operation may succeed without any state change.
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::NodeOffline
                | Self::Timeout
                | Self::SyncBacklog
                | Self::ServerError
                | Self::AgentLaunchFailed
        )
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
            "FORK_UNSUPPORTED" => Ok(Self::ForkUnsupported),
            "AUTH_REQUIRED" => Ok(Self::AuthRequired),
            "PTY_DISABLED" => Ok(Self::PtyDisabled),
            "COMMAND_DUPLICATE" => Ok(Self::CommandDuplicate),
            "NOT_FOUND" => Ok(Self::NotFound),
            "INTERNAL" => Ok(Self::Internal),
            "CONFIG_INVALID" => Ok(Self::ConfigInvalid),
            "REVISION_MISMATCH" => Ok(Self::RevisionMismatch),
            "SECRET_MISSING" => Ok(Self::SecretMissing),
            "SECRET_IN_USE" => Ok(Self::SecretInUse),
            "SECRET_INVALID" => Ok(Self::SecretInvalid),
            "AGENT_NOT_INSTALLED" => Ok(Self::AgentNotInstalled),
            "AGENT_LAUNCH_FAILED" => Ok(Self::AgentLaunchFailed),
            "AGENT_PROTOCOL_ERROR" => Ok(Self::AgentProtocolError),
            "GIT_UNAVAILABLE" => Ok(Self::GitUnavailable),
            "GIT_FAILED" => Ok(Self::GitFailed),
            "WORKTREE_CONFLICT" => Ok(Self::WorktreeConflict),
            "SYNC_BACKLOG" => Ok(Self::SyncBacklog),
            "DB_CORRUPT" => Ok(Self::DbCorrupt),
            "PROVISIONER_FAILED" => Ok(Self::ProvisionerFailed),
            "VERSION_MISMATCH" => Ok(Self::VersionMismatch),
            "TIMEOUT" => Ok(Self::Timeout),
            "DISK_FULL" => Ok(Self::DiskFull),
            "PERMISSION_DENIED" => Ok(Self::PermissionDenied),
            "IO_ERROR" => Ok(Self::IoError),
            "PTY_FAILED" => Ok(Self::PtyFailed),
            "SERVER_ERROR" => Ok(Self::ServerError),
            "CANCELLED" => Ok(Self::Cancelled),
            other => Err(EnumParseError::new("error code", other)),
        }
    }
}

/// Structured error exchanged by REST, Client WS, the node protocol and IPC.
///
/// `message` is the English fallback for logs / CLI; clients translate
/// `code` (and interpolate `params`) for display. `details` carries
/// machine-readable technical context that the UI shows in an expandable
/// section, never as the primary message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct ErrorInfo {
    /// Structured error code (also the i18n key)
    pub code: ErrorCode,
    /// English human-readable fallback message
    pub message: String,
    /// Parameters for message interpolation (path, agent_id, exit_code, ...)
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Technical details for the expandable "details" section
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<RawJson>,
    /// Whether retrying may succeed without a state change
    #[serde(default)]
    pub retryable: bool,
    /// Recovery actions the client can offer as buttons
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovery: Vec<RecoveryAction>,
    /// Correlation ID shared with logs / incidents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// What happened to the user's data (unchanged / not sent / recoverable / lost)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_safety: Option<DataSafety>,
    /// Link to the troubleshooting documentation anchor
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

impl ErrorInfo {
    /// Builds an error from a code and the English fallback message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            params: BTreeMap::new(),
            details: None,
            retryable: code.is_retryable(),
            recovery: Vec::new(),
            request_id: None,
            data_safety: None,
            docs: None,
        }
    }

    /// Adds an interpolation parameter.
    pub fn with_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }

    /// Adds technical details.
    pub fn with_details(mut self, details: impl Into<RawJson>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Adds a recovery action.
    pub fn with_recovery(mut self, action: RecoveryAction) -> Self {
        self.recovery.push(action);
        self
    }

    /// Attaches the correlation ID.
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    /// Declares what happened to the user's data.
    pub fn with_data_safety(mut self, data_safety: DataSafety) -> Self {
        self.data_safety = Some(data_safety);
        self
    }
}

/// Recovery action offered to the user after a failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[specta(collect)]
pub enum RecoveryAction {
    /// Retry the same operation (`idempotent` = safe to resend as-is)
    Retry {
        /// Whether the operation is idempotent / safe to retry
        idempotent: bool,
    },
    /// Resume a stopped session with replayed history
    ResumeWithReplay {
        /// Target session ID
        session_id: SessionId,
    },
    /// Reconnect to the target node
    ReconnectNode {
        /// Target node ID
        node_id: NodeId,
    },
    /// Pick another online node
    SelectOnlineNode,
    /// Install the missing agent on the node
    ReinstallAgent {
        /// Target node ID
        node_id: NodeId,
        /// Agent ID
        agent_id: String,
    },
    /// Log in to the agent (run `fxg agents login <agent_id>` / open the login UI)
    LoginAgent {
        /// Target agent ID
        agent_id: String,
    },
    /// Recreate the worktree
    RecreateWorktree {
        /// Logical project ID
        project_id: ProjectId,
        /// Branch name (when known)
        branch: Option<String>,
    },
    /// Open the configuration file
    OpenConfig {
        /// Path of the configuration file
        path: String,
    },
    /// Show the logs / diagnostics for a session
    ShowLogs {
        /// Target session ID (all logs when omitted)
        session_id: Option<SessionId>,
    },
    /// Export a support bundle
    ExportDiagnostics,
}

/// Non-fatal warning attached to a successful / partially successful response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct WarningInfo {
    /// Structured warning code (same vocabulary as [`ErrorCode`])
    pub code: ErrorCode,
    /// English human-readable fallback message
    pub message: String,
    /// Technical details
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<RawJson>,
    /// Recovery actions the client can offer
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovery: Vec<RecoveryAction>,
}

impl WarningInfo {
    /// Builds a warning from a code and the English fallback message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
            recovery: Vec::new(),
        }
    }
}

/// Whether the user's data was affected by a failure.
///
/// Every user-visible error should state this so the user knows whether they
/// need to recover anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
#[specta(collect)]
pub enum DataSafety {
    /// Nothing was modified
    Unchanged,
    /// The input / workspace is preserved and will be retried
    Preserved,
    /// The message / command was not sent
    NotSent,
    /// The data can be recovered from the given location
    Recoverable {
        /// Recovery point (snapshot tree hash, backup path, ...)
        location: String,
    },
    /// The data was lost
    Lost,
    /// Unknown
    Unknown,
}

/// Result of a command (correlated back from the node through the server to the requesting client).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[specta(collect)]
pub struct CommandResult {
    /// Correlation ID attached to each `ServerToNodeMsg` command
    pub command_id: CommandId,
    /// Whether it succeeded
    pub success: bool,
    /// Structured error on failure
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    /// Target session ID (optional)
    pub session_id: Option<SessionId>,
    /// Non-fatal issues (the command succeeded but part of the work failed)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningInfo>,
    /// What happened to the user's data (failures and destructive operations)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_safety: Option<DataSafety>,
}

/// Three-state outcome of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Everything succeeded
    Success,
    /// The command succeeded but some work was skipped or degraded
    Partial,
    /// The command failed
    Failed,
}

impl CommandResult {
    /// Builds a successful result.
    pub fn success(command_id: impl Into<CommandId>, session_id: impl Into<SessionId>) -> Self {
        Self {
            command_id: command_id.into(),
            success: true,
            error: None,
            session_id: Some(session_id.into()),
            warnings: Vec::new(),
            data_safety: None,
        }
    }

    /// Builds a failed result.
    ///
    /// The command ID is used as the correlation ID when the error does not
    /// carry one, so the UI can quote it when reporting a problem.
    pub fn failure(command_id: impl Into<CommandId>, mut error: ErrorInfo) -> Self {
        let command_id = command_id.into();
        if error.request_id.is_none() {
            error.request_id = Some(command_id.as_str().to_owned());
        }
        Self {
            command_id,
            success: false,
            error: Some(error),
            session_id: None,
            warnings: Vec::new(),
            data_safety: None,
        }
    }

    /// Adds a non-fatal warning (turns the outcome into `Partial`).
    pub fn with_warning(mut self, warning: WarningInfo) -> Self {
        self.warnings.push(warning);
        self
    }

    /// Declares what happened to the user's data.
    pub fn with_data_safety(mut self, data_safety: DataSafety) -> Self {
        self.data_safety = Some(data_safety);
        self
    }

    /// Three-state outcome derived from `success` and `warnings`.
    pub fn status(&self) -> CommandStatus {
        if !self.success {
            CommandStatus::Failed
        } else if self.warnings.is_empty() {
            CommandStatus::Success
        } else {
            CommandStatus::Partial
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_roundtrip_covers_all_codes() {
        let codes = [
            ErrorCode::Unauthorized,
            ErrorCode::Forbidden,
            ErrorCode::NodeOffline,
            ErrorCode::AlreadyResolved,
            ErrorCode::Busy,
            ErrorCode::InvalidState,
            ErrorCode::ResumeRequired,
            ErrorCode::ForkUnsupported,
            ErrorCode::PtyDisabled,
            ErrorCode::CommandDuplicate,
            ErrorCode::NotFound,
            ErrorCode::Internal,
            ErrorCode::ConfigInvalid,
            ErrorCode::AgentNotInstalled,
            ErrorCode::AgentLaunchFailed,
            ErrorCode::AgentProtocolError,
            ErrorCode::GitUnavailable,
            ErrorCode::GitFailed,
            ErrorCode::WorktreeConflict,
            ErrorCode::SyncBacklog,
            ErrorCode::DbCorrupt,
            ErrorCode::ProvisionerFailed,
            ErrorCode::VersionMismatch,
            ErrorCode::Timeout,
            ErrorCode::DiskFull,
            ErrorCode::PermissionDenied,
            ErrorCode::IoError,
            ErrorCode::PtyFailed,
            ErrorCode::ServerError,
            ErrorCode::Cancelled,
        ];
        for code in codes {
            assert_eq!(ErrorCode::from_str(code.as_str()).unwrap(), code);
        }
    }

    #[test]
    fn error_info_omits_empty_optional_fields() {
        let json = serde_json::to_value(ErrorInfo::new(ErrorCode::Timeout, "timed out")).unwrap();
        assert_eq!(json["code"], "TIMEOUT");
        assert_eq!(json["message"], "timed out");
        assert_eq!(json["retryable"], true);
        assert!(json.get("details").is_none());
        assert!(json.get("recovery").is_none());
        assert!(json.get("request_id").is_none());
    }

    #[test]
    fn error_info_carries_recovery_and_details() {
        let info = ErrorInfo::new(ErrorCode::ResumeRequired, "resume required")
            .with_recovery(RecoveryAction::ResumeWithReplay {
                session_id: "s1".into(),
            })
            .with_details(serde_json::json!({ "session_id": "s1" }))
            .with_param("session_id", "s1");
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["recovery"][0]["kind"], "resume_with_replay");
        assert_eq!(json["recovery"][0]["session_id"], "s1");
        assert_eq!(json["details"]["session_id"], "s1");
        assert_eq!(json["params"]["session_id"], "s1");
    }

    #[test]
    fn command_failure_uses_command_id_as_request_id() {
        let result = CommandResult::failure("cmd-1", ErrorInfo::new(ErrorCode::Busy, "busy"));
        assert!(!result.success);
        let error = result.error.expect("error body");
        assert_eq!(error.code, ErrorCode::Busy);
        assert_eq!(error.request_id.as_deref(), Some("cmd-1"));
    }

    #[test]
    fn retryable_codes_are_limited_to_transient_failures() {
        assert!(ErrorCode::NodeOffline.is_retryable());
        assert!(ErrorCode::Timeout.is_retryable());
        assert!(!ErrorCode::NotFound.is_retryable());
        assert!(!ErrorCode::ConfigInvalid.is_retryable());
    }
    #[test]
    fn every_error_code_has_a_ui_message_and_troubleshooting_anchor() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        let messages = std::fs::read_to_string(workspace.join("ui/messages/en.json"))
            .expect("read ui/messages/en.json");
        let troubleshooting = std::fs::read_to_string(workspace.join("docs/troubleshooting.md"))
            .expect("read docs/troubleshooting.md");

        assert_eq!(ErrorCode::ALL.len(), 35, "update ALL when adding a code");
        for code in ErrorCode::ALL {
            let snake = code.as_str().to_ascii_lowercase();
            let message_key = format!("error_{snake}");
            assert!(
                messages.contains(&format!("\"{message_key}\"")),
                "missing ui/messages/en.json key: {message_key}"
            );
            assert!(
                troubleshooting.contains(&format!("## {}", code.as_str())),
                "missing docs/troubleshooting.md section: {}",
                code.as_str()
            );
        }
    }
}
