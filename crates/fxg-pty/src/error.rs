//! `fxg-pty` のエラー型。

use std::path::PathBuf;

/// PTY / プロセス管理のエラー。
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    /// ファイルシステム・I/O エラー
    #[error("io error at {path}: {source}")]
    Io {
        /// 対象パス
        path: PathBuf,
        /// 元のI/Oエラー
        #[source]
        source: std::io::Error,
    },
    /// コマンドが PATH から解決できない
    #[error("command not found: {command} (cwd: {cwd})")]
    CommandNotFound {
        /// 解決対象のコマンド名
        command: String,
        /// 探索基準ディレクトリ
        cwd: PathBuf,
    },
    /// PTY の生成・読み書きエラー (portable-pty)
    #[error("pty error: {0}")]
    Pty(String),
    /// 指定 PTY ID が存在しない
    #[error("pty session not found: {0}")]
    NotFound(String),
    /// 同一 PTY ID が既に存在する
    #[error("pty session already exists: {0}")]
    AlreadyExists(String),
    /// Windows Job Object の操作エラー
    #[cfg(windows)]
    #[error("windows job object error: {0}")]
    JobObject(#[from] win32job::JobError),
    /// Windows API (`windows` クレート) 呼び出しエラー
    #[cfg(windows)]
    #[error("windows api error: {0}")]
    WindowsApi(#[from] windows::core::Error),
}

impl PtyError {
    /// `Display` 実装を持つエラー (portable-pty / anyhow 等) から変換する。
    pub fn from_display(err: impl std::fmt::Display) -> Self {
        Self::Pty(err.to_string())
    }
}
