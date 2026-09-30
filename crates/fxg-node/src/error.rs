//! `fxg-node` のエラー型。

use std::path::PathBuf;

/// ノードデーモンのエラー。
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// ファイルシステム・I/O エラー
    #[error("io error at {path}: {source}")]
    Io {
        /// 対象パス
        path: PathBuf,
        /// 元のI/Oエラー
        #[source]
        source: std::io::Error,
    },
    /// パスを伴わないI/Oエラー
    #[error("io error: {0}")]
    Plain(#[from] std::io::Error),
    /// `git` コマンドが存在しない
    #[error("git is not available: {0}")]
    GitUnavailable(String),
    /// `git` コマンドがエラー終了した
    #[error("git {args} failed in {cwd}: {message}")]
    Git {
        /// 実行ディレクトリ
        cwd: PathBuf,
        /// 引数 (表示用)
        args: String,
        /// stderr 由来のメッセージ
        message: String,
    },
    /// Git リポジトリではない
    #[error("not a git repository: {0}")]
    NotARepository(PathBuf),
    /// Worktree / ブランチの状態が不正
    #[error("invalid worktree state: {0}")]
    InvalidWorktree(String),
    /// セッションの状態・操作が不正
    #[error("invalid session state: {0}")]
    InvalidSession(String),
    /// 同一 `command_id` のコマンドが既に処理済み (冪等性)
    #[error("duplicate command: {0}")]
    CommandDuplicate(String),
    /// 承認リクエストが既に解決済み (2回目以降の応答)
    #[error("already resolved: {0}")]
    AlreadyResolved(String),
    /// 対象のエージェント起動・ドライバ操作が失敗した
    #[error("agent error: {0}")]
    Agent(String),
    /// 永続化してはいけないイベントを記録しようとした
    /// (ストリーミング途中のチャンク・キーストローク等)
    #[error("event is not persistable: {0}")]
    NonPersistableEvent(&'static str),
    /// データベースエラー (fxg-db)
    #[error("database error: {0}")]
    Db(#[from] fxg_db::DbError),
    /// 設定ファイルエラー (fxg-protocol)
    #[error("config error: {0}")]
    Config(#[from] fxg_protocol::config::ConfigError),
    /// PTY / プロセスエラー (fxg-pty)
    #[error("pty error: {0}")]
    Pty(#[from] fxg_pty::PtyError),
    /// JSON 変換エラー
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// サーバー (ローカルHTTP/WS・IPC) の起動・入出力エラー
    #[error("server error: {0}")]
    Server(String),
}

impl NodeError {
    /// パス付きI/Oエラーを生成する。
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// IPC / WebSocket / REST 応答に載せる構造化エラーコードへ変換する。
    ///
    /// クライアント (CLI / PWA) はこのコードで冪等な再試行
    /// (`COMMAND_DUPLICATE` / `ALREADY_RESOLVED`) を判定する。
    pub fn error_code(&self) -> fxg_protocol::common::ErrorCode {
        use fxg_protocol::common::ErrorCode;
        match self {
            Self::NotARepository(_)
            | Self::InvalidWorktree(_)
            | Self::InvalidSession(_)
            | Self::NonPersistableEvent(_)
            | Self::Config(_) => ErrorCode::InvalidState,
            Self::CommandDuplicate(_) => ErrorCode::CommandDuplicate,
            Self::AlreadyResolved(_) => ErrorCode::AlreadyResolved,
            Self::Db(fxg_db::DbError::SessionNotFound(_))
            | Self::Db(fxg_db::DbError::NodeNotFound(_)) => ErrorCode::NotFound,
            _ => ErrorCode::Internal,
        }
    }
}
