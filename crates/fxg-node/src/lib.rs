//! FlexAgent node daemon crate (`fxg-node`)
//!
//! Core implementation of the resident node (`fxg node start`). It provides the local
//! HTTP/WS server (`127.0.0.1:7860`), logical project resolution, Git Worktree
//! management, and the Outbox sync worker.
//!
//! Design: `docs/01-architecture-and-sync.md` / `docs/04-agent-drivers-and-windows.md`
//! / `docs/05-cli-and-config.md`.
//!
//! # Modules
//!
//! - `paths`: role-scoped path resolution lives in `fxg_protocol::paths`
//! - `git`: `git` CLI wrapper
//! - `project`: logical project (`project_key`) resolution
//! - `worktree`: Git Worktree detection, creation, removal, and hook execution
//! - `session`: immediate delivery and persistence of session events (compaction)
//! - `daemon`: node daemon (local HTTP/WS server + authentication)

#![warn(missing_docs)]
// Migration baseline: remove this crate-level allow as Phase 1 of
// `docs/proposals/2026-11-08-type-safety-plan.md` lands.
#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "type-safety migration baseline; see docs/06-type-safety.md"
)]

pub mod daemon;
pub mod error;
pub mod git;
pub mod project;
pub mod session;
pub mod session_manager;
pub mod sync;
pub mod worktree;

pub mod fs_browse;
pub mod workspace;

#[cfg(test)]
mod testutil;

pub(crate) mod rpc;

pub use error::NodeError;
pub use project::{ResolvedProject, normalize_git_url, resolve_project};
pub use session::{SessionBroadcast, SessionEventBus};
pub use worktree::{
    WorktreeAddOutcome, WorktreeAddRequest, WorktreeEntry, ensure_worktree, list_worktrees,
    prune_worktrees, remove_worktree, resolve_worktree_dir,
};
