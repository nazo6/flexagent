//! CLI subcommands.
//!
//! The argument structs are re-exported here so `main.rs` and the tray can use
//! `crate::commands::...` without knowing the file split.

mod agents;
mod doctor;
mod hub;
mod internal;
mod logs;
mod node;
mod service;
mod session;
mod session_new;
mod support;
mod web;

pub use agents::*;
pub use doctor::*;
pub use hub::*;
pub use internal::*;
pub use logs::*;
pub use node::*;
pub use service::*;
pub use session::*;
pub use session_new::*;
pub use support::*;
pub use web::*;

pub(crate) use node::{local_web_url, open_session_url};

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::client::NodeClient;

/// Opens `node.db` (CLI-side reads of the DB-owned settings).
async fn node_db(env: fxg_protocol::config::EnvLookup<'_>) -> Result<fxg_db::Db> {
    let paths = fxg_protocol::paths::Paths::resolve(env, fxg_protocol::paths::Role::Node);
    Ok(fxg_db::Db::open(&paths.db_path(), fxg_protocol::paths::Role::Node).await?)
}

/// Create or reuse a worktree and return its path (`fxg session new -w` /
/// `fxg session fork -w`).
pub(super) async fn create_worktree(
    client: &mut NodeClient,
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

/// Replaces the port component of a `host:port` bind address.
pub(super) fn replace_port(listen_addr: &str, port: u16, default_host: &str) -> String {
    let host = listen_addr
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(default_host);
    format!("{host}:{port}")
}

/// Appends logging flags to detached child-process arguments.
pub(super) fn push_log_args(
    args: &mut Vec<String>,
    log_level: Option<&str>,
    log_file: Option<&str>,
    no_log_file: bool,
    log_format: Option<&str>,
    no_color: bool,
) {
    if let Some(level) = log_level {
        args.push("--log-level".to_owned());
        args.push(level.to_owned());
    }
    if let Some(file) = log_file {
        args.push("--log-file".to_owned());
        args.push(file.to_owned());
    }
    if no_log_file {
        args.push("--no-log-file".to_owned());
    }
    if let Some(format) = log_format {
        args.push("--log-format".to_owned());
        args.push(format.to_owned());
    }
    if no_color {
        args.push("--no-color".to_owned());
    }
}
