//! Error types for `fxg-node`.

use std::path::PathBuf;

/// Windows `ERROR_PIPE_BUSY` (231).
///
/// Every instance of the named pipe is connected, so `WaitNamedPipe` should be used.
/// Defined here because both the server and the client refer to it.
#[cfg(windows)]
pub(crate) const PIPE_BUSY: i32 = 231;

/// Node daemon error.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// Filesystem / I/O error
    #[error("io error at {path}: {source}")]
    Io {
        /// Target path
        path: PathBuf,
        /// Original I/O error
        #[source]
        source: std::io::Error,
    },
    /// I/O error without an associated path
    #[error("io error: {0}")]
    Plain(#[from] std::io::Error),
    /// `git` command is not available
    #[error("git is not available: {0}")]
    GitUnavailable(String),
    /// `git` command exited with an error
    #[error("git {args} failed in {cwd}: {message}")]
    Git {
        /// Working directory
        cwd: PathBuf,
        /// Arguments (for display)
        args: String,
        /// Message derived from stderr
        message: String,
    },
    /// Not a Git repository
    #[error("not a git repository: {0}")]
    NotARepository(PathBuf),
    /// Invalid worktree / branch state
    #[error("invalid worktree state: {0}")]
    InvalidWorktree(String),
    /// Invalid state or unmet precondition for the operation (e.g. missing configuration)
    #[error("invalid state: {0}")]
    InvalidState(String),
    /// Invalid session state or operation
    #[error("invalid session: {0}")]
    InvalidSession(String),
    /// The session is stopped and the agent does not support native resume
    /// (`session/resume` / `session/load` / binding an existing opencode2 session).
    /// An explicit resume with replayed history is required.
    #[error("native resume is not available; resume with replayed history: {0}")]
    ResumeRequired(String),
    /// The session is running (busy) and cannot be operated on
    #[error("session is busy: {0}")]
    Busy(String),
    /// A command with the same `command_id` was already processed (idempotency)
    #[error("duplicate command: {0}")]
    CommandDuplicate(String),
    /// The approval request was already resolved (second or later response)
    #[error("already resolved: {0}")]
    AlreadyResolved(String),
    /// Starting the target agent or operating its driver failed
    #[error("agent error: {0}")]
    Agent(String),
    /// Attempted to record an event that must not be persisted
    /// (mid-stream chunks, keystrokes, and so on)
    #[error("event is not persistable: {0}")]
    NonPersistableEvent(&'static str),
    /// Database error (fxg-db)
    #[error("database error: {0}")]
    Db(#[from] fxg_db::DbError),
    /// Configuration file error (fxg-protocol)
    #[error("config error: {0}")]
    Config(#[from] fxg_protocol::config::ConfigError),
    /// PTY / process error (fxg-pty)
    #[error("pty error: {0}")]
    Pty(#[from] fxg_pty::PtyError),
    /// JSON conversion error
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Startup or I/O error of the server (local HTTP/WS / IPC)
    #[error("server error: {0}")]
    Server(String),
}

impl NodeError {
    /// Creates an I/O error with an associated path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Converts to the structured error code carried in IPC / WebSocket / REST responses.
    ///
    /// Clients (CLI / PWA) use this code to decide on idempotent retries
    /// (`COMMAND_DUPLICATE` / `ALREADY_RESOLVED`).
    pub fn error_code(&self) -> fxg_protocol::common::ErrorCode {
        use fxg_protocol::common::ErrorCode;
        match self {
            Self::NotARepository(_)
            | Self::InvalidWorktree(_)
            | Self::InvalidSession(_)
            | Self::InvalidState(_)
            | Self::NonPersistableEvent(_)
            | Self::Config(_) => ErrorCode::InvalidState,
            Self::CommandDuplicate(_) => ErrorCode::CommandDuplicate,
            Self::AlreadyResolved(_) => ErrorCode::AlreadyResolved,
            Self::Busy(_) => ErrorCode::Busy,
            Self::ResumeRequired(_) => ErrorCode::ResumeRequired,
            Self::Db(fxg_db::DbError::SessionNotFound(_))
            | Self::Db(fxg_db::DbError::NodeNotFound(_)) => ErrorCode::NotFound,
            _ => ErrorCode::Internal,
        }
    }
}
