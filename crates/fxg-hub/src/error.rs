//! Error types for `fxg-server`.

/// Hub startup and runtime errors.
#[derive(Debug, thiserror::Error)]
pub enum HubError {
    /// Database error (fxg-db)
    #[error("database error: {0}")]
    Db(#[from] fxg_db::DbError),
    /// Filesystem / I/O error
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Configuration file error (fxg-protocol)
    #[error("config error: {0}")]
    Config(#[from] fxg_protocol::config::ConfigError),
    /// Socket bind failure, etc.
    #[error("hub error: {0}")]
    Hub(String),
    /// The secret ID or value is invalid
    #[error("invalid secret: {0}")]
    SecretInvalid(String),
}
