//! FlexAgent node daemon crate (`fxg-node`)
//!
//! Core implementation of the resident node (`fxg daemon`). It provides the local IPC
//! server (Named Pipe / UDS), the local HTTP/WS server (`127.0.0.1:7860`), logical
//! project resolution, Git Worktree management, and the Outbox sync worker.
//!
//! Design: `docs/01-architecture-and-sync.md` / `docs/04-agent-drivers-and-windows.md`
//! / `docs/05-cli-and-pwa-ui.md`.
//!
//! # Modules
//!
//! - `paths`: `~/.flexagent/` path resolution and local IPC endpoints
//! - `git`: `git` CLI wrapper
//! - `project`: logical project (`project_key`) resolution
//! - `worktree`: Git Worktree detection, creation, removal, and hook execution
//! - `session`: immediate delivery and persistence of session events (compaction)
//! - `daemon`: node daemon (local HTTP/WS server + local IPC + authentication)

#![warn(missing_docs)]
// Migration baseline: remove this crate-level allow as Phase 1 of
// `docs/changelog/2026-11-08-type-safety-plan.md` lands.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

pub mod daemon;
pub mod error;
pub mod git;
pub mod paths;
pub mod project;
pub mod session;
pub mod session_manager;
pub mod sync;
pub mod worktree;

pub mod bootstrap;
pub mod credentials;
pub mod fs_browse;

#[cfg(test)]
mod testutil;

pub mod ipc_client;
pub(crate) mod ipc_framing;

pub use error::NodeError;
pub use ipc_client::IpcClient;
pub use paths::{NodePaths, ipc_endpoint};
pub use project::{ResolvedProject, normalize_git_url, resolve_project};
pub use session::{SessionBroadcast, SessionEventBus};
pub use worktree::{
    WorktreeAddOutcome, WorktreeAddRequest, WorktreeEntry, ensure_worktree, list_worktrees,
    prune_worktrees, remove_worktree, resolve_worktree_dir,
};
