//! FlexAgent ノードデーモンクレート (`fxg-node`)
//!
//! 常駐ノード (`fxg daemon`) の中核実装。ローカルIPCサーバー (Named Pipe / UDS)、
//! ローカル HTTP/WS サーバー (`127.0.0.1:7860`)、論理プロジェクト解決、
//! Git Worktree 管理、Shadow Git Tree スナップショット、Outbox 同期ワーカーを含む。
//!
//! 設計: `docs/01-architecture-and-sync.md` / `docs/04-agent-drivers-and-windows.md`
//! / `docs/05-cli-and-pwa-ui.md`。
//!
//! # モジュール構成
//!
//! - `paths`: `~/.flexagent/` のパス解決とローカルIPC エンドポイント
//! - `git`: `git` CLI ラッパー
//! - `project`: 論理プロジェクト (`project_key`) の解決
//! - `worktree`: Git Worktree の検出・作成・削除とフック実行
//! - `snapshot`: Shadow Git Tree によるターン単位スナップショット / 復元
//! - `session`: セッションイベントの即時配信と永続化 (Compaction)
//! - `daemon`: ノードデーモン (ローカルHTTP/WS サーバー + ローカルIPC + 認証)

#![warn(missing_docs)]

pub mod daemon;
pub mod error;
pub mod git;
pub mod paths;
pub mod project;
pub mod session;
pub mod snapshot;
pub mod worktree;

#[cfg(test)]
mod testutil;

pub mod ipc_client;
pub(crate) mod ipc_framing;

pub use error::NodeError;
pub use ipc_client::IpcClient;
pub use paths::{NodePaths, ipc_endpoint};
pub use project::{ProjectResolutionSource, ResolvedProject, normalize_git_url, resolve_project};
pub use session::{SessionBroadcast, SessionEventBus};
pub use snapshot::{
    DEFAULT_SNAPSHOT_SIZE_LIMIT_BYTES, RestoreOutcome, ShadowGitTree, SnapshotOutcome,
};
pub use worktree::{
    HookLogEntry, WorktreeAddOutcome, WorktreeAddRequest, WorktreeEntry, ensure_worktree,
    list_worktrees, prune_worktrees, remove_worktree, resolve_worktree_dir,
};
