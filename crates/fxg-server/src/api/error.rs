//! REST / WS API error type and its HTTP status mapping.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fxg_db::DbError;
use fxg_protocol::client_api::ApiErrorResponse;
use fxg_protocol::common::{
    CommandResult, DataSafety, ErrorCode, ErrorInfo, RawJson, RecoveryAction,
};

use super::request_context;

/// Role-independent API error (converted to the shared REST / WS error format).
#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP status
    pub status: StatusCode,
    /// Structured error code
    pub code: ErrorCode,
    /// English human-readable fallback message
    pub message: String,
    /// Technical details for the expandable "details" section
    pub details: Option<Box<RawJson>>,
    /// Whether retrying may succeed without a state change
    pub retryable: bool,
    /// Recovery actions the client can offer
    pub recovery: Vec<RecoveryAction>,
    /// Correlation ID shared with logs / incidents
    pub request_id: Option<Box<str>>,
    /// What happened to the user's data
    pub data_safety: Option<DataSafety>,
}

impl ApiError {
    /// Builds an error with an explicit status and code.
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
            retryable: code.is_retryable(),
            recovery: Vec::new(),
            request_id: None,
            data_safety: None,
        }
    }

    /// Builds an error, choosing the recommended status from the `ErrorCode`.
    pub fn from_code(code: ErrorCode, message: impl Into<String>) -> Self {
        let status = match code {
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden | ErrorCode::PtyDisabled | ErrorCode::PermissionDenied => {
                StatusCode::FORBIDDEN
            }
            ErrorCode::NodeOffline
            | ErrorCode::SyncBacklog
            | ErrorCode::VersionMismatch
            | ErrorCode::ProvisionerFailed => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::AlreadyResolved
            | ErrorCode::Busy
            | ErrorCode::CommandDuplicate
            | ErrorCode::ResumeRequired
            | ErrorCode::ForkUnsupported
            | ErrorCode::AuthRequired
            | ErrorCode::WorktreeConflict
            | ErrorCode::AgentNotInstalled
            | ErrorCode::AgentLaunchFailed
            | ErrorCode::AgentProtocolError
            | ErrorCode::GitUnavailable
            | ErrorCode::GitFailed
            | ErrorCode::Cancelled
            | ErrorCode::RevisionMismatch
            | ErrorCode::SecretInUse => StatusCode::CONFLICT,
            ErrorCode::InvalidState | ErrorCode::ConfigInvalid | ErrorCode::SecretInvalid => {
                StatusCode::BAD_REQUEST
            }
            ErrorCode::NotFound | ErrorCode::SecretMissing => StatusCode::NOT_FOUND,
            ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
            ErrorCode::DiskFull => StatusCode::INSUFFICIENT_STORAGE,
            ErrorCode::Internal
            | ErrorCode::IoError
            | ErrorCode::PtyFailed
            | ErrorCode::ServerError
            | ErrorCode::DbCorrupt => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, code, message)
    }

    /// Adds technical details.
    pub fn with_details(mut self, details: impl Into<RawJson>) -> Self {
        self.details = Some(Box::new(details.into()));
        self
    }

    /// Adds a recovery action.
    pub fn with_recovery(mut self, action: RecoveryAction) -> Self {
        self.recovery.push(action);
        self
    }

    /// Attaches the correlation ID.
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into().into_boxed_str());
        self
    }

    /// Declares what happened to the user's data.
    pub fn with_data_safety(mut self, data_safety: DataSafety) -> Self {
        self.data_safety = Some(data_safety);
        self
    }

    /// Builds from a structured error returned by a node.
    pub fn from_info(info: ErrorInfo) -> Self {
        let mut api = Self::from_code(info.code, info.message);
        api.details = info.details.map(Box::new);
        api.retryable = info.retryable;
        api.recovery = info.recovery;
        api.request_id = info.request_id.map(String::into_boxed_str);
        api.data_safety = info.data_safety;
        api
    }

    /// Builds from a failed `CommandResult`, falling back to `INTERNAL` when the
    /// node did not attach an error body.
    pub fn from_command(result: CommandResult) -> Self {
        match result.error {
            Some(info) => Self::from_info(info),
            None => Self::internal("command failed without an error body"),
        }
    }

    /// Converts into the structured error body.
    ///
    /// Fills in the request correlation ID from the current request when the
    /// error itself did not come with one (e.g. errors raised by handlers).
    pub fn into_info(self) -> ErrorInfo {
        ErrorInfo {
            code: self.code,
            message: self.message,
            params: std::collections::BTreeMap::new(),
            details: self.details.map(|details| *details),
            retryable: self.retryable,
            recovery: self.recovery,
            request_id: self
                .request_id
                .map(String::from)
                .or_else(request_context::current_request_id),
            data_safety: self.data_safety,
            docs: None,
        }
    }

    /// `503 NODE_OFFLINE` with reconnect / select-node recovery actions.
    pub fn node_offline(node_id: &str, message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::NodeOffline, message)
            .with_recovery(RecoveryAction::ReconnectNode {
                node_id: node_id.into(),
            })
            .with_recovery(RecoveryAction::SelectOnlineNode)
    }

    /// `400 INVALID_STATE`
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::InvalidState, message)
    }

    /// `404 NOT_FOUND`
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::NotFound, message)
    }

    /// `403 FORBIDDEN`
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::from_code(ErrorCode::Forbidden, message)
    }

    /// `500 INTERNAL` (also logged)
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::warn!("api error: {err}");
        Self::from_code(ErrorCode::Internal, err.to_string())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ApiError {}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match &err {
            DbError::SessionNotFound(_) | DbError::NodeNotFound(_) => {
                Self::not_found(err.to_string())
            }
            _ => Self::internal(err),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        (
            status,
            Json(ApiErrorResponse {
                error: self.into_info(),
            }),
        )
            .into_response()
    }
}
