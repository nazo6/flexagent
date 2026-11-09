//! CLI subcommands.
//!
//! The argument structs are re-exported here so `main.rs` and the tray can use
//! `crate::commands::...` without knowing the file split.

mod admin;
mod daemon;
mod project;
mod run;
mod session;

pub use admin::*;
pub use daemon::*;
pub use project::*;
pub use run::*;
pub use session::*;

pub(crate) use daemon::local_web_url;

use anyhow::Result;

/// Opens `node.db` (CLI-side reads of the DB-owned settings).
async fn node_db(env: fxg_protocol::config::EnvLookup<'_>) -> Result<fxg_db::Db> {
    let paths = fxg_node::NodePaths::from_env(env);
    Ok(fxg_db::Db::open(&paths.node_db_path(), fxg_db::DbRole::Node).await?)
}

#[cfg(test)]
mod tests;
