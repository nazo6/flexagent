//! Error types for `fxg-pty`.

use std::path::PathBuf;

/// Errors from PTY / process management.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    /// Filesystem or I/O error
    #[error("io error at {path}: {source}")]
    Io {
        /// Target path
        path: PathBuf,
        /// Underlying I/O error
        #[source]
        source: std::io::Error,
    },
    /// The command cannot be resolved from `PATH`
    #[error("command not found: {command} (cwd: {cwd}; {reason})")]
    CommandNotFound {
        /// Command name to resolve
        command: String,
        /// Directory used as the search base
        cwd: PathBuf,
        /// Resolver error (search paths and the underlying reason)
        reason: String,
    },
    /// PTY creation or read/write error (portable-pty)
    #[error("pty error: {0}")]
    Pty(String),
    /// The PTY input writer has stopped, so the session can no longer receive input
    #[error("pty input is closed: {0}")]
    InputClosed(String),
    /// No PTY with the given ID exists
    #[error("pty session not found: {0}")]
    NotFound(String),
    /// A PTY with the same ID already exists
    #[error("pty session already exists: {0}")]
    AlreadyExists(String),
    /// Windows Job Object operation error
    #[cfg(windows)]
    #[error("windows job object error: {0}")]
    JobObject(#[from] win32job::JobError),
    /// Windows API (`windows` crate) call error
    #[cfg(windows)]
    #[error("windows api error: {0}")]
    WindowsApi(#[from] windows::core::Error),
}

impl PtyError {
    /// Converts from an error implementing `Display` (portable-pty / anyhow, etc.).
    pub fn from_display(err: impl std::fmt::Display) -> Self {
        Self::Pty(err.to_string())
    }
}
