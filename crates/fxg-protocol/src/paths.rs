//! Resolution of the config, data, state, cache and log directories.
//!
//! Design: `docs/05-cli-and-config.md` §2.1.
//!
//! Every path is derived from a [`Role`] and five roots:
//!
//! - OS mode uses the platform directories (`dirs`) for the config, data,
//!   state, cache and log roots.
//! - Portable mode (`FXG_HOME`) uses
//!   `$FXG_HOME/{config,data,state,logs,cache}`.
//!
//! Inside a root, files are scoped by role (`node` / `hub`), so a node process
//! can never read the hub's database or access token:
//!
//! ```text
//! <config>/node.toml
//! <data>/node/{node.db,auth_token,node_token,secrets/,worktrees/}
//! <state>/node/{node.addr,start.lock}
//! <logs>/node/node.log
//! ```

use std::path::{Path, PathBuf};

use crate::config::EnvLookup;

/// Environment variable that switches to portable mode.
pub const ENV_FXG_HOME: &str = "FXG_HOME";
/// Application directory appended to every OS root.
pub const APP_DIR_NAME: &str = "flexagent";
/// Bootstrap configuration file of the local node.
pub const NODE_CONFIG_FILE_NAME: &str = "node.toml";
/// Bootstrap configuration file of the hub.
pub const HUB_CONFIG_FILE_NAME: &str = "hub.toml";
/// Web access token file name (one per role).
pub const AUTH_TOKEN_FILE_NAME: &str = "auth_token";
/// Node ⇔ hub pairing token file name.
pub const NODE_TOKEN_FILE_NAME: &str = "node_token";
/// Node-local database file name.
pub const NODE_DB_FILE_NAME: &str = "node.db";
/// Hub database file name.
pub const HUB_DB_FILE_NAME: &str = "hub.db";
/// Web Push VAPID key pair file name.
pub const VAPID_KEYS_FILE_NAME: &str = "vapid.json";
/// ACP Registry cache file name.
pub const REGISTRY_CACHE_FILE_NAME: &str = "registry.json";
/// Directory holding secret values.
pub const SECRETS_DIR_NAME: &str = "secrets";
/// Directory holding Git worktrees.
pub const WORKTREES_DIR_NAME: &str = "worktrees";

/// Execution role of a process (and owner of its paths).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Local node (`fxg node start`).
    Node,
    /// Hub (`fxg hub start`).
    Hub,
}

impl Role {
    /// Role name (`node` / `hub`), also used as the directory name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Hub => "hub",
        }
    }

    /// Bootstrap configuration file name.
    pub const fn config_file_name(self) -> &'static str {
        match self {
            Self::Node => NODE_CONFIG_FILE_NAME,
            Self::Hub => HUB_CONFIG_FILE_NAME,
        }
    }

    /// Database file name.
    pub const fn db_file_name(self) -> &'static str {
        match self {
            Self::Node => NODE_DB_FILE_NAME,
            Self::Hub => HUB_DB_FILE_NAME,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The five root directories a process may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
    cache: PathBuf,
    logs: PathBuf,
}

impl Roots {
    /// Portable mode: all classes live below `$FXG_HOME`.
    pub fn portable(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        Self {
            config: base.join("config"),
            data: base.join("data"),
            state: base.join("state"),
            cache: base.join("cache"),
            logs: base.join("logs"),
        }
    }

    /// OS mode: platform directories with the application directory appended.
    pub fn from_os() -> Self {
        let config = os_dir(dirs::config_dir());
        let data = os_dir(dirs::data_local_dir());
        let state = dirs::state_dir()
            .map(|dir| dir.join(APP_DIR_NAME))
            .unwrap_or_else(|| data.clone());
        let cache = cache_root();
        let logs = log_dir(&state);
        Self {
            config,
            data,
            state,
            cache,
            logs,
        }
    }

    /// Configuration root.
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// Persistent data root.
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// Runtime state root.
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// Cache root.
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    /// Log root.
    pub fn logs(&self) -> &Path {
        &self.logs
    }
}

fn os_dir(dir: Option<PathBuf>) -> PathBuf {
    dir.unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR_NAME)
}

#[cfg(target_os = "macos")]
fn log_dir(state: &Path) -> PathBuf {
    dirs::home_dir()
        .map(|home| home.join("Library/Logs").join(APP_DIR_NAME))
        .unwrap_or_else(|| state.to_path_buf())
}

#[cfg(target_os = "windows")]
fn log_dir(state: &Path) -> PathBuf {
    state.join("logs")
}

#[cfg(target_os = "windows")]
fn cache_root() -> PathBuf {
    os_dir(dirs::data_local_dir()).join("cache")
}

#[cfg(not(target_os = "windows"))]
fn cache_root() -> PathBuf {
    os_dir(dirs::cache_dir())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn log_dir(state: &Path) -> PathBuf {
    state.to_path_buf()
}

/// Role-scoped path resolution.
///
/// A node process holds [`Paths`] with [`Role::Node`] and a hub process holds
/// [`Role::Hub`]; the type exposes only that role's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    role: Role,
    roots: Roots,
}

impl Paths {
    /// Builds paths from explicit roots.
    pub fn new(roots: Roots, role: Role) -> Self {
        Self { role, roots }
    }

    /// Resolves the roots from `FXG_HOME` (portable) or the OS directories.
    pub fn resolve(env: EnvLookup<'_>, role: Role) -> Self {
        match env(ENV_FXG_HOME).filter(|value| !value.trim().is_empty()) {
            Some(base) => Self::portable(PathBuf::from(base), role),
            None => Self::new(Roots::from_os(), role),
        }
    }

    /// Portable mode rooted at `base`.
    pub fn portable(base: impl Into<PathBuf>, role: Role) -> Self {
        Self::new(Roots::portable(base), role)
    }

    /// The process role.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The root directories.
    pub fn roots(&self) -> &Roots {
        &self.roots
    }

    /// Role data directory (`<data>/<role>`).
    pub fn data_dir(&self) -> PathBuf {
        self.roots.data.join(self.role.as_str())
    }

    /// Bootstrap configuration file (`<config>/<role>.toml`).
    pub fn config_path(&self) -> PathBuf {
        self.roots.config.join(self.role.config_file_name())
    }

    /// Database file (`<data>/<role>/<role>.db`).
    pub fn db_path(&self) -> PathBuf {
        self.data_dir().join(self.role.db_file_name())
    }

    /// Web access token (`<data>/<role>/auth_token`).
    pub fn auth_token_path(&self) -> PathBuf {
        self.data_dir().join(AUTH_TOKEN_FILE_NAME)
    }

    /// Node pairing token (`<data>/node/node_token`).
    pub fn node_token_path(&self) -> PathBuf {
        self.data_dir().join(NODE_TOKEN_FILE_NAME)
    }

    /// Secret value directory (`<data>/<role>/secrets`).
    pub fn secrets_dir(&self) -> PathBuf {
        self.data_dir().join(SECRETS_DIR_NAME)
    }

    /// Git worktree directory (`<data>/node/worktrees`).
    pub fn worktrees_dir(&self) -> PathBuf {
        self.data_dir().join(WORKTREES_DIR_NAME)
    }

    /// Web Push VAPID key pair (`<data>/<role>/vapid.json`).
    pub fn vapid_path(&self) -> PathBuf {
        self.data_dir().join(VAPID_KEYS_FILE_NAME)
    }

    /// Runtime state directory (`<state>/<role>`).
    pub fn state_dir(&self) -> PathBuf {
        self.roots.state.join(self.role.as_str())
    }

    /// Bound address file (`<state>/<role>/<role>.addr`).
    pub fn addr_path(&self) -> PathBuf {
        self.state_dir()
            .join(format!("{}.addr", self.role.as_str()))
    }

    /// Auto-start lock file (`<state>/<role>/start.lock`).
    pub fn start_lock_path(&self) -> PathBuf {
        self.state_dir().join("start.lock")
    }

    /// Cache directory (`<cache>/<role>`).
    pub fn cache_dir(&self) -> PathBuf {
        self.roots.cache.join(self.role.as_str())
    }

    /// Log directory (`<logs>/<role>`).
    pub fn log_dir(&self) -> PathBuf {
        self.roots.logs.join(self.role.as_str())
    }

    /// Log file (`<logs>/<role>/<role>.log`).
    pub fn log_path(&self) -> PathBuf {
        self.log_dir().join(format!("{}.log", self.role.as_str()))
    }

    /// Creates the role directories (data, state, cache, logs; worktrees for a node).
    ///
    /// On Unix the directories are created with mode `0700`.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        let mut dirs = vec![
            self.data_dir(),
            self.state_dir(),
            self.cache_dir(),
            self.log_dir(),
        ];
        if self.role == Role::Node {
            dirs.push(self.worktrees_dir());
        }
        for dir in dirs {
            create_private_dir(&dir)?;
        }
        Ok(())
    }
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn portable_layout_is_role_scoped() {
        let node = Paths::portable("/tmp/fxg-home", Role::Node);
        assert_eq!(
            node.config_path(),
            PathBuf::from("/tmp/fxg-home/config/node.toml")
        );
        assert_eq!(
            node.db_path(),
            PathBuf::from("/tmp/fxg-home/data/node/node.db")
        );
        assert_eq!(
            node.auth_token_path(),
            PathBuf::from("/tmp/fxg-home/data/node/auth_token")
        );
        assert_eq!(
            node.node_token_path(),
            PathBuf::from("/tmp/fxg-home/data/node/node_token")
        );
        assert_eq!(
            node.secrets_dir(),
            PathBuf::from("/tmp/fxg-home/data/node/secrets")
        );
        assert_eq!(
            node.worktrees_dir(),
            PathBuf::from("/tmp/fxg-home/data/node/worktrees")
        );
        assert_eq!(
            node.addr_path(),
            PathBuf::from("/tmp/fxg-home/state/node/node.addr")
        );
        assert_eq!(
            node.start_lock_path(),
            PathBuf::from("/tmp/fxg-home/state/node/start.lock")
        );
        assert_eq!(
            node.log_path(),
            PathBuf::from("/tmp/fxg-home/logs/node/node.log")
        );
        assert_eq!(node.cache_dir(), PathBuf::from("/tmp/fxg-home/cache/node"));

        let hub = Paths::portable("/tmp/fxg-home", Role::Hub);
        assert_eq!(
            hub.config_path(),
            PathBuf::from("/tmp/fxg-home/config/hub.toml")
        );
        assert_eq!(
            hub.db_path(),
            PathBuf::from("/tmp/fxg-home/data/hub/hub.db")
        );
        assert_eq!(
            hub.vapid_path(),
            PathBuf::from("/tmp/fxg-home/data/hub/vapid.json")
        );
        assert_ne!(node.auth_token_path(), hub.auth_token_path());
    }

    #[test]
    fn resolve_prefers_fxg_home() {
        let env = env_map(&[("FXG_HOME", "/tmp/fxg-env-home")]);
        let paths = Paths::resolve(&|key| env.get(key).cloned(), Role::Node);
        assert_eq!(
            paths.db_path(),
            PathBuf::from("/tmp/fxg-env-home/data/node/node.db")
        );
    }

    #[test]
    fn os_layout_uses_the_application_directory() {
        let roots = Roots::from_os();
        for root in [roots.config(), roots.data(), roots.cache()] {
            assert!(
                root.components()
                    .any(|component| component.as_os_str() == APP_DIR_NAME),
                "{} must contain the application directory",
                root.display()
            );
        }
    }
}
