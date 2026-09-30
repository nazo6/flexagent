//! デーモン操作の共有実装 (IPC / Client REST 双方から利用)。
//!
//! ローカルIPC (`fxg` CLI) と Client REST API (`localhost:7860` /
//! 中央サーバー中継) は同じ実体操作 (Worktree 管理・プロジェクト解決・監査ログ)
//! を行うため、実装をここへ集約する。

use std::path::{Path, PathBuf};

use fxg_protocol::client_api::WorktreeInfo;
use fxg_protocol::common::HookLogEntry;

use super::DaemonState;
use crate::error::NodeError;
use crate::project::{self, ResolvedProject};
use crate::worktree;

/// 監査ログの記録元 (クライアント情報)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditSource {
    /// 送信元 IP アドレス
    pub client_ip: String,
    /// 認証主体 (`local-ipc` / `bearer:<ip>` 等)
    pub auth_subject: String,
    /// User-Agent
    pub user_agent: Option<String>,
}

impl AuditSource {
    /// ローカル CLI (IPC) 由来の操作。
    pub fn local() -> Self {
        Self {
            client_ip: "local".to_owned(),
            auth_subject: "local-ipc".to_owned(),
            user_agent: None,
        }
    }

    /// リモート (中央サーバー経由) 操作。
    pub fn remote(node_id: &str) -> Self {
        Self {
            client_ip: "server".to_owned(),
            auth_subject: format!("node:{node_id}"),
            user_agent: None,
        }
    }
}

/// Worktree 作成結果。
#[derive(Debug, Clone)]
pub struct WorktreeAddOutcome {
    /// 作成/再利用されたパス
    pub path: PathBuf,
    /// チェックアウトされたブランチ
    pub branch: String,
    /// 新規作成だったか (`false` は既存 Worktree の再利用)
    pub created: bool,
    /// `post_create` フックの実行ログ
    pub hook_logs: Vec<HookLogEntry>,
}

impl DaemonState {
    /// 監査ログを `node.db.audit_logs` に記録する (失敗は警告のみ)。
    pub async fn record_audit(
        &self,
        action: &str,
        source: &AuditSource,
        session_id: Option<&str>,
        details: serde_json::Value,
    ) {
        let mut audit = fxg_db::AuditLogRecord::new(
            action,
            source.client_ip.clone(),
            source.auth_subject.clone(),
        );
        audit.session_id = session_id.map(str::to_owned);
        audit.node_id = Some(self.node_id().to_owned());
        audit.client_user_agent = source.user_agent.clone();
        audit.details = details;
        if let Err(err) = self.db().append_audit_log(&audit).await {
            tracing::warn!("failed to record audit log ({action}): {err}");
        }
    }

    /// プロジェクトを解決し、`projects` / `project_node_bindings` を更新する。
    pub async fn resolve_and_register_project(
        &self,
        dir: &Path,
    ) -> Result<ResolvedProject, NodeError> {
        let resolved = project::resolve_project(dir, self.node_id()).await?;
        self.db()
            .upsert_project(&resolved.to_project_record())
            .await?;

        // Worktree 内での実行かどうかと現在ブランチを記録する
        // (取得に失敗した場合はメインリポジトリ扱いで登録する)
        let is_worktree = match &resolved.git_root {
            Some(root) => crate::git::is_worktree(root).await.unwrap_or(false),
            None => false,
        };
        let branch = match &resolved.git_root {
            Some(root) => crate::git::current_branch(root).await.unwrap_or(None),
            None => None,
        };
        self.db()
            .upsert_project_binding(&resolved.to_binding_record(
                self.node_id(),
                is_worktree,
                branch.as_deref(),
            ))
            .await?;

        // 再リンク時に古いプロジェクトの紐付けを残さない
        // (`fxg project link` で project_key を変更した場合など)
        self.db()
            .delete_other_project_bindings(
                self.node_id(),
                &resolved.local_path.to_string_lossy(),
                &resolved.project_id,
            )
            .await?;
        self.db().delete_orphan_projects().await?;
        Ok(resolved)
    }

    /// プロジェクトのメインリポジトリ (Worktree ではない) のローカルパスを解決する。
    ///
    /// メイン紐付けが無い場合は最初の紐付けのパスを返す。
    pub async fn project_main_repo(&self, project_id: &str) -> Result<PathBuf, NodeError> {
        let projects = self.db().list_projects().await?;
        let project = projects
            .iter()
            .find(|project| project.project_id == project_id)
            .ok_or_else(|| NodeError::InvalidWorktree(format!("unknown project: {project_id}")))?;
        let mut bindings: Vec<_> = project
            .bindings
            .iter()
            .filter(|binding| binding.node_id == self.node_id())
            .collect();
        if bindings.is_empty() {
            return Err(NodeError::InvalidWorktree(format!(
                "project is not bound to this node: {project_id}"
            )));
        }
        bindings.sort_by_key(|binding| (binding.is_worktree, -binding.last_used_at));
        Ok(PathBuf::from(&bindings[0].local_path))
    }

    /// Worktree 一覧を返す (`cwd` または登録済みプロジェクトのメインリポジトリから解決)。
    pub async fn worktree_list(
        &self,
        cwd: Option<&Path>,
        project_id: Option<&str>,
    ) -> Result<Vec<WorktreeInfo>, NodeError> {
        let repos: Vec<PathBuf> = match cwd {
            Some(cwd) => vec![cwd.to_path_buf()],
            None => {
                let projects = self.db().list_projects().await?;
                projects
                    .into_iter()
                    .filter(|project| project_id.is_none_or(|id| id == project.project_id.as_str()))
                    .flat_map(|project| {
                        project
                            .bindings
                            .into_iter()
                            .filter(|binding| {
                                binding.node_id == self.node_id() && !binding.is_worktree
                            })
                            .map(|binding| PathBuf::from(binding.local_path))
                    })
                    .collect()
            }
        };

        let mut worktrees = Vec::new();
        for repo in repos {
            let entries = worktree::list_worktrees(&repo).await?;
            for entry in entries {
                worktrees.push(WorktreeInfo {
                    node_id: self.node_id().to_owned(),
                    path: entry.path.to_string_lossy().into_owned(),
                    branch: entry.branch,
                    head_commit: entry.head,
                    is_main: entry.is_main,
                });
            }
        }
        worktrees.sort_by(|a, b| a.path.cmp(&b.path));
        worktrees.dedup_by(|a, b| a.path == b.path);
        Ok(worktrees)
    }

    /// 新規 Worktree を作成し、紐付けと監査ログを更新する。
    ///
    /// `expected_project_id` が指定された場合、`repo` の解決結果と一致しなければ
    /// 拒否する (CLI からの指定ミス検出)。
    pub async fn worktree_add(
        &self,
        repo: &Path,
        expected_project_id: Option<&str>,
        branch: &str,
        base_branch: Option<String>,
        new_path: Option<PathBuf>,
        source: &AuditSource,
    ) -> Result<WorktreeAddOutcome, NodeError> {
        let resolved = self.resolve_and_register_project(repo).await?;
        let repo_root = resolved
            .git_root
            .clone()
            .ok_or_else(|| NodeError::NotARepository(repo.to_path_buf()))?;
        if let Some(expected) = expected_project_id
            && expected != resolved.project_id
        {
            return Err(NodeError::InvalidWorktree(format!(
                "project_id mismatch: {expected} (resolved: {})",
                resolved.project_id
            )));
        }

        let project_config = project::load_project_config(&resolved.local_path)
            .await?
            .unwrap_or_default();
        let global_config =
            fxg_protocol::config::GlobalConfig::load_from_path(&self.paths().config_path())?;
        let dir_template = project_config
            .worktree
            .dir_template
            .clone()
            .unwrap_or_else(|| {
                global_config
                    .node
                    .resolved_worktree_dir_template()
                    .to_owned()
            });
        let base_branch = base_branch.or_else(|| project_config.worktree.base_branch.clone());

        let outcome = worktree::ensure_worktree(&worktree::WorktreeAddRequest {
            repo: &repo_root,
            project_id: &resolved.project_id,
            branch,
            base_branch: base_branch.as_deref(),
            dir_template: &dir_template,
            new_path: new_path.as_deref(),
            fxg_home: self.paths().fxg_home(),
            copy_files: &project_config.worktree.copy_files,
            post_create: &project_config.worktree.post_create,
        })
        .await?;

        // Worktree をノードの紐付けとして登録する
        self.db()
            .upsert_project(&resolved.to_project_record())
            .await?;
        let mut binding = resolved.to_binding_record(self.node_id(), true, Some(branch));
        binding.local_path = outcome.path.to_string_lossy().into_owned();
        self.db().upsert_project_binding(&binding).await?;

        self.record_audit(
            fxg_db::audit::actions::WORKTREE_MANAGE,
            source,
            None,
            serde_json::json!({
                "action": "add",
                "branch": branch,
                "path": outcome.path.to_string_lossy(),
                "created": outcome.created,
            }),
        )
        .await;

        Ok(WorktreeAddOutcome {
            path: outcome.path,
            branch: outcome.branch,
            created: outcome.created,
            hook_logs: outcome.hook_logs,
        })
    }

    /// Worktree を削除し、紐付けと監査ログを更新する。
    ///
    /// `repo_hint` が無い場合は、登録済みのメインリポジトリの Worktree 一覧から
    /// `target` を含むリポジトリを探索する。
    pub async fn worktree_remove(
        &self,
        repo_hint: Option<&Path>,
        target: &Path,
        force: bool,
        source: &AuditSource,
    ) -> Result<(), NodeError> {
        let repo = match repo_hint {
            Some(repo) => repo.to_path_buf(),
            None => self.find_repo_for_worktree(target).await?,
        };
        worktree::remove_worktree(&repo, target, force).await?;
        let _ = self
            .db()
            .delete_project_binding(self.node_id(), &target.to_string_lossy())
            .await;

        self.record_audit(
            fxg_db::audit::actions::WORKTREE_MANAGE,
            source,
            None,
            serde_json::json!({
                "action": "remove",
                "path": target.to_string_lossy(),
                "force": force,
            }),
        )
        .await;
        Ok(())
    }

    /// `target` の Worktree を含むメインリポジトリを探索する。
    async fn find_repo_for_worktree(&self, target: &Path) -> Result<PathBuf, NodeError> {
        let projects = self.db().list_projects().await?;
        let main_roots: Vec<PathBuf> = projects
            .iter()
            .flat_map(|project| {
                project
                    .bindings
                    .iter()
                    .filter(|binding| binding.node_id == self.node_id() && !binding.is_worktree)
                    .map(|binding| PathBuf::from(&binding.local_path))
            })
            .collect();
        let canonical_target = fxg_pty::canonicalize(target).ok();
        for root in main_roots {
            let Ok(entries) = worktree::list_worktrees(&root).await else {
                continue;
            };
            let found = entries.iter().any(|entry| {
                entry.path == target
                    || (canonical_target.is_some()
                        && fxg_pty::canonicalize(&entry.path).ok() == canonical_target)
            });
            if found {
                return Ok(root);
            }
        }
        Err(NodeError::InvalidWorktree(format!(
            "worktree not found: {}",
            target.display()
        )))
    }
}
