//! CLI subcommands.
//!
//! The argument structs are re-exported here so `main.rs` and the tray can use
//! `crate::commands::...` without knowing the file split.

mod admin;
mod daemon;
mod diagnostics;
mod run;
mod session;

pub use admin::*;
pub use daemon::*;
pub use diagnostics::*;
pub use run::*;
pub use session::*;

pub(crate) use daemon::local_web_url;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::client::DaemonClient;

/// Opens `node.db` (CLI-side reads of the DB-owned settings).
async fn node_db(env: fxg_protocol::config::EnvLookup<'_>) -> Result<fxg_db::Db> {
    let paths = fxg_node::NodePaths::from_env(env);
    Ok(fxg_db::Db::open(&paths.node_db_path(), fxg_db::DbRole::Node).await?)
}

/// Create or reuse a worktree and return its path (`fxg run -w` /
/// `fxg session fork -w`).
pub(super) async fn create_worktree(
    client: &mut DaemonClient,
    cwd: &Path,
    branch: &str,
    base_branch: Option<String>,
) -> Result<PathBuf> {
    let (path, branch, created) = client.add_worktree(cwd, None, branch, base_branch).await?;
    if created {
        eprintln!("created worktree {branch} at {path}");
    } else {
        eprintln!("reusing existing worktree {branch} at {path}");
    }
    Ok(PathBuf::from(path))
}
