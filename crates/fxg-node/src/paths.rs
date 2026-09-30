//! `~/.flexagent/` 以下のデータディレクトリ・ファイルパス解決。
//!
//! 設計: `docs/05-cli-and-pwa-ui.md` §2.1。
//!
//! ```text
//! ~/.flexagent/
//! ├── config.toml            # グローバル設定
//! ├── auth_token             # Client ⇔ Server/Node 認証トークン (0600)
//! ├── node_token             # Node ⇔ Server ペアリングトークン (0600)
//! ├── node.db                # ローカルノード SQLite DB
//! ├── server.db              # 中央サーバー SQLite DB
//! ├── worktrees/             # fxg が自動作成する Git Worktree
//! └── snapshots/             # Shadow Git Tree のセッション別インデックス
//! ```

use std::path::{Path, PathBuf};

use fxg_protocol::config::{
    AUTH_TOKEN_FILE_NAME, CONFIG_FILE_NAME, EnvLookup, NODE_TOKEN_FILE_NAME, fxg_home,
};

/// `~/.flexagent/` 配下のパス解決ヘルパー。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePaths {
    fxg_home: PathBuf,
}

impl NodePaths {
    /// 指定ディレクトリをデータディレクトリとして扱う。
    pub fn new(fxg_home: impl Into<PathBuf>) -> Self {
        Self {
            fxg_home: fxg_home.into(),
        }
    }

    /// 環境変数 (`FXG_HOME`) または `~/.flexagent` から解決する。
    pub fn from_env(env: EnvLookup<'_>) -> Self {
        Self::new(fxg_home(env))
    }

    /// データディレクトリ本体。
    pub fn fxg_home(&self) -> &Path {
        &self.fxg_home
    }

    /// グローバル設定ファイル (`config.toml`)。
    pub fn config_path(&self) -> PathBuf {
        self.fxg_home.join(CONFIG_FILE_NAME)
    }

    /// クライアント認証トークン (`auth_token`)。
    pub fn auth_token_path(&self) -> PathBuf {
        self.fxg_home.join(AUTH_TOKEN_FILE_NAME)
    }

    /// ノード個別ペアリングトークン (`node_token`)。
    pub fn node_token_path(&self) -> PathBuf {
        self.fxg_home.join(NODE_TOKEN_FILE_NAME)
    }

    /// ローカルノードDB (`node.db`)。
    pub fn node_db_path(&self) -> PathBuf {
        fxg_db::node_db_path(&self.fxg_home)
    }

    /// 中央サーバーDB (`server.db`)。
    pub fn hub_db_path(&self) -> PathBuf {
        fxg_db::hub_db_path(&self.fxg_home)
    }

    /// Worktree 集約ディレクトリ (`worktrees/`)。
    pub fn worktrees_dir(&self) -> PathBuf {
        self.fxg_home.join("worktrees")
    }

    /// Shadow Git Tree のインデックス置き場 (`snapshots/`)。
    pub fn snapshots_dir(&self) -> PathBuf {
        self.fxg_home.join("snapshots")
    }

    /// セッション用 Shadow Git Index のパス (`snapshots/<session-id>.index`)。
    pub fn snapshot_index_path(&self, session_id: &str) -> PathBuf {
        self.snapshots_dir().join(format!("{session_id}.index"))
    }

    /// ACP Registry キャッシュディレクトリ (`cache/`)。
    pub fn cache_dir(&self) -> PathBuf {
        self.fxg_home.join("cache")
    }

    /// エージェントバイナリ配置ディレクトリ (`agents/`)。
    pub fn agents_dir(&self) -> PathBuf {
        self.fxg_home.join("agents")
    }

    /// 退避 Git bundle 置き場 (`bundles/`)。
    pub fn bundles_dir(&self) -> PathBuf {
        self.fxg_home.join("bundles")
    }

    /// データディレクトリ (および主要サブディレクトリ) を作成する。
    pub fn ensure_dirs(&self) -> Result<(), crate::NodeError> {
        for dir in [
            self.fxg_home.clone(),
            self.worktrees_dir(),
            self.snapshots_dir(),
        ] {
            std::fs::create_dir_all(&dir)
                .map_err(|source| crate::NodeError::Io { path: dir, source })?;
        }
        Ok(())
    }
}

/// CLI ⇔ Daemon ローカルIPC のエンドポイントを解決する。
///
/// - **Windows**: Named Pipe `\\.\pipe\fxg-daemon-<username>`
/// - **Linux / macOS / WSL**: `$XDG_RUNTIME_DIR/fxg/daemon.sock` または
///   `/tmp/fxg-<uid>/daemon.sock`
///
/// 設計: `docs/03-protocol-and-api.md` §4。
pub fn ipc_endpoint(env: EnvLookup<'_>) -> String {
    ipc_endpoint_inner(env)
}

#[cfg(windows)]
fn ipc_endpoint_inner(env: EnvLookup<'_>) -> String {
    let username = env("USERNAME")
        .or_else(|| env("USER"))
        .unwrap_or_else(|| "default".to_owned());
    let sanitized: String = username
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!(r"\\.\pipe\fxg-daemon-{sanitized}")
}

#[cfg(unix)]
fn ipc_endpoint_inner(env: EnvLookup<'_>) -> String {
    if let Some(runtime_dir) = env("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(runtime_dir)
            .join("fxg")
            .join("daemon.sock")
            .to_string_lossy()
            .into_owned();
    }
    PathBuf::from(format!("/tmp/fxg-{}", current_uid()))
        .join("daemon.sock")
        .to_string_lossy()
        .into_owned()
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` は引数を持たず常に成功する POSIX システムコール。
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn resolves_standard_paths() {
        let paths = NodePaths::new("/tmp/fxg-home");
        assert_eq!(
            paths.config_path(),
            PathBuf::from("/tmp/fxg-home/config.toml")
        );
        assert_eq!(
            paths.auth_token_path(),
            PathBuf::from("/tmp/fxg-home/auth_token")
        );
        assert_eq!(paths.node_db_path(), PathBuf::from("/tmp/fxg-home/node.db"));
        assert_eq!(
            paths.snapshot_index_path("0195f0"),
            PathBuf::from("/tmp/fxg-home/snapshots/0195f0.index")
        );
        assert_eq!(
            paths.worktrees_dir(),
            PathBuf::from("/tmp/fxg-home/worktrees")
        );
    }

    #[test]
    fn ensure_dirs_creates_subdirectories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = NodePaths::new(dir.path().join("fxg"));
        paths.ensure_dirs().expect("ensure");
        assert!(paths.worktrees_dir().is_dir());
        assert!(paths.snapshots_dir().is_dir());
    }

    #[test]
    fn from_env_prefers_fxg_home() {
        let env = env_map(&[("FXG_HOME", "/tmp/fxg-env-home")]);
        let paths = NodePaths::from_env(&|key| env.get(key).cloned());
        assert_eq!(paths.fxg_home(), Path::new("/tmp/fxg-env-home"));
    }

    #[cfg(windows)]
    #[test]
    fn ipc_endpoint_is_named_pipe() {
        let env = env_map(&[("USERNAME", "nazo6")]);
        assert_eq!(
            ipc_endpoint(&|key| env.get(key).cloned()),
            r"\\.\pipe\fxg-daemon-nazo6"
        );
    }

    #[cfg(unix)]
    #[test]
    fn ipc_endpoint_prefers_xdg_runtime_dir() {
        let env = env_map(&[("XDG_RUNTIME_DIR", "/run/user/1000")]);
        assert_eq!(
            ipc_endpoint(&|key| env.get(key).cloned()),
            "/run/user/1000/fxg/daemon.sock"
        );

        let empty = env_map(&[]);
        let fallback = ipc_endpoint(&|key| empty.get(key).cloned());
        assert!(fallback.starts_with("/tmp/fxg-"), "got: {fallback}");
        assert!(fallback.ends_with("daemon.sock"));
    }
}
