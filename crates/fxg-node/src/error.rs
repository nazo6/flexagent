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
}
