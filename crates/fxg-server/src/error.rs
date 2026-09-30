//! `fxg-server` のエラー型。

/// 中央サーバーの起動・実行エラー。
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// データベースエラー (fxg-db)
    #[error("database error: {0}")]
    Db(#[from] fxg_db::DbError),
    /// ファイルシステム・I/O エラー
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 設定ファイルエラー (fxg-protocol)
    #[error("config error: {0}")]
    Config(#[from] fxg_protocol::config::ConfigError),
    /// ソケットのバインド失敗など
    #[error("server error: {0}")]
    Server(String),
}
