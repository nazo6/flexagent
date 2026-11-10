//! Error types for `fxg-db`.

/// Error raised by `fxg-db` operations.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// SQLite operation error
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Migration application error
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// JSON conversion error
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Filesystem operation error
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Event payload in the DB cannot be decoded (data corruption)
    #[error("invalid event payload in database: {0}")]
    InvalidPayload(String),
    /// Attempted to append an event that must not be persisted
    /// (streaming chunks, keystrokes, etc.)
    #[error("event is not persistable: {0}")]
    NonPersistableEvent(fxg_protocol::common::EventType),
    /// `node_seq` is negative (data corruption)
    #[error("node_seq {0} is negative")]
    NegativeNodeSeq(i64),
    /// `node_seq` does not fit into an i64
    #[error("node_seq {node_seq} of session {session_id} does not fit into i64")]
    NodeSeqOutOfRange {
        /// Target session ID
        session_id: String,
        /// Sequence number that failed conversion
        node_seq: u64,
    },
    /// Session not found
    #[error("session not found: {0}")]
    SessionNotFound(String),
    /// Node not found
    #[error("node not found: {0}")]
    NodeNotFound(String),
    /// Enum value in the DB is unknown (data corruption)
    #[error("invalid enum value in database: {0}")]
    InvalidEnumValue(String),
    /// Cursor value does not fit into an i64
    #[error("cursor {0} does not fit into i64")]
    CursorOutOfRange(u64),
    /// Permission request not found
    #[error("permission request not found: {0}")]
    PermissionRequestNotFound(String),
    /// Incident not found
    #[error("incident not found: {0}")]
    IncidentNotFound(i64),
    /// Search query is empty
    #[error("search query must not be empty")]
    EmptySearchQuery,
    /// `expected_revision` did not match the stored revision (optimistic lock conflict)
    #[error("settings revision mismatch")]
    RevisionMismatch,
    /// Secret is in use and cannot be deleted
    #[error("secret {id} is in use: {used_by:?}")]
    SecretInUse {
        /// Referenced secret ID
        id: fxg_protocol::SecretId,
        /// Identifiers of agents / git hosts referencing the secret
        used_by: Vec<String>,
    },
    /// Secret parameter or state is invalid
    #[error("secret invalid: {0}")]
    SecretInvalid(String),
}
