//! Resolution of data directories and file paths under `~/.flexagent/`.
//!
//! Design: `docs/05-cli-and-pwa-ui.md` §2.1.
//!
//! ```text
//! ~/.flexagent/
//! ├── config.toml            # Global configuration
//! ├── auth_token             # Client <-> Server/Node auth token (0600)
//! ├── node_token             # Node <-> Server pairing token (0600)
//! ├── node.db                # Local node SQLite DB
//! ├── hub.db              # Hub SQLite DB
//! ├── worktrees/             # Git worktrees created automatically by fxg
//! ```

use std::path::{Path, PathBuf};

use fxg_protocol::config::{
    AUTH_TOKEN_FILE_NAME, CONFIG_FILE_NAME, EnvLookup, NODE_TOKEN_FILE_NAME, fxg_home,
};

/// Path resolution helper for entries under `~/.flexagent/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePaths {
    fxg_home: PathBuf,
}

impl NodePaths {
    /// Uses the given directory as the data directory.
    pub fn new(fxg_home: impl Into<PathBuf>) -> Self {
        Self {
            fxg_home: fxg_home.into(),
        }
    }

    /// Resolves from the environment variable (`FXG_HOME`) or `~/.flexagent`.
    pub fn from_env(env: EnvLookup<'_>) -> Self {
        Self::new(fxg_home(env))
    }

    /// The data directory itself.
    pub fn fxg_home(&self) -> &Path {
        &self.fxg_home
    }

    /// Global configuration file (`config.toml`).
    pub fn config_path(&self) -> PathBuf {
        self.fxg_home.join(CONFIG_FILE_NAME)
    }

    /// Client auth token (`auth_token`).
    pub fn auth_token_path(&self) -> PathBuf {
        self.fxg_home.join(AUTH_TOKEN_FILE_NAME)
    }

    /// Node-specific pairing token (`node_token`).
    pub fn node_token_path(&self) -> PathBuf {
        self.fxg_home.join(NODE_TOKEN_FILE_NAME)
    }

    /// Local node DB (`node.db`).
    pub fn node_db_path(&self) -> PathBuf {
        fxg_db::node_db_path(&self.fxg_home)
    }

    /// Hub DB (`hub.db`).
    pub fn hub_db_path(&self) -> PathBuf {
        fxg_db::hub_db_path(&self.fxg_home)
    }

    /// Directory aggregating worktrees (`worktrees/`).
    pub fn worktrees_dir(&self) -> PathBuf {
        self.fxg_home.join("worktrees")
    }

    /// ACP Registry cache directory (`cache/`).
    pub fn cache_dir(&self) -> PathBuf {
        self.fxg_home.join("cache")
    }

    /// Directory holding agent binaries (`agents/`).
    pub fn agents_dir(&self) -> PathBuf {
        self.fxg_home.join("agents")
    }

    /// Creates the data directory (and the main subdirectories).
    pub fn ensure_dirs(&self) -> Result<(), crate::NodeError> {
        for dir in [self.fxg_home.clone(), self.worktrees_dir()] {
            std::fs::create_dir_all(&dir)
                .map_err(|source| crate::NodeError::Io { path: dir, source })?;
        }
        Ok(())
    }
}

/// Resolves the endpoint of the CLI <-> daemon local IPC.
///
/// - **Windows**: Named Pipe `\\.\pipe\fxg-node-<username>`
/// - **Linux / macOS / WSL**: `$XDG_RUNTIME_DIR/fxg/node.sock` or
///   `/tmp/fxg-<uid>/node.sock`
///
/// If the environment variable `FXG_IPC_ENDPOINT` is set, it takes precedence
/// (for isolated execution in ephemeral VMs and for multiple instances in tests).
///
/// Design: `docs/03-protocol-and-api.md` §4.
pub fn ipc_endpoint(env: EnvLookup<'_>) -> String {
    if let Some(value) =
        env(fxg_protocol::config::env_keys::IPC_ENDPOINT).filter(|value| !value.trim().is_empty())
    {
        return value;
    }
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
    format!(r"\\.\pipe\fxg-node-{sanitized}")
}

#[cfg(unix)]
fn ipc_endpoint_inner(env: EnvLookup<'_>) -> String {
    if let Some(runtime_dir) = env("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(runtime_dir)
            .join("fxg")
            .join("node.sock")
            .to_string_lossy()
            .into_owned();
    }
    PathBuf::from(format!("/tmp/fxg-{}", current_uid()))
        .join("node.sock")
        .to_string_lossy()
        .into_owned()
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` is a POSIX system call that takes no arguments and always succeeds.
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
            r"\\.\pipe\fxg-node-nazo6"
        );
    }

    #[cfg(unix)]
    #[test]
    fn ipc_endpoint_prefers_xdg_runtime_dir() {
        let env = env_map(&[("XDG_RUNTIME_DIR", "/run/user/1000")]);
        assert_eq!(
            ipc_endpoint(&|key| env.get(key).cloned()),
            "/run/user/1000/fxg/node.sock"
        );

        let empty = env_map(&[]);
        let fallback = ipc_endpoint(&|key| empty.get(key).cloned());
        assert!(fallback.starts_with("/tmp/fxg-"), "got: {fallback}");
        assert!(fallback.ends_with("node.sock"));
    }
}
