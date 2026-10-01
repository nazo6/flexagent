//! デーモン操作の共有実装 (IPC / Client REST 双方から利用)。
//!
//! ローカルIPC (`fxg` CLI) と Client REST API (`localhost:7860` /
//! 中央サーバー中継) は同じ実体操作 (Worktree 管理・プロジェクト解決・監査ログ)
//! を行うため、実装をここへ集約する。

use std::path::{Path, PathBuf};

use fxg_protocol::client_api::{AgentSummary, AgentsResponse, WorktreeInfo};
use fxg_protocol::common::{AgentAction, HookLogEntry, ProjectSummary};

use super::DaemonState;
use crate::error::NodeError;
use crate::project::{self, ResolvedProject};
use crate::worktree;

/// Git リポジトリ一括スキャン (`fxg project scan`) の最大探索深さ。
const SCAN_MAX_DEPTH: usize = 3;

/// ACP Registry 操作のエラーを [`NodeError::Agent`] へ変換する。
fn agent_error(err: anyhow::Error) -> NodeError {
    NodeError::Agent(format!("{err:#}"))
}

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
        // 周辺の変更 (Worktree 検出・プロジェクト紐付け) をハブへ報告する
        self.trigger_node_hello();
        Ok(resolved)
    }

    /// プロジェクト一覧に自ノード紐付けのパス実在確認 (ライブ) を付与して返す。
    ///
    /// パス実在は外部要因 (Worktree の手動削除・外部ドライブの取り外し等) で
    /// 変わるため、クライアントへの返却時に確認する。
    pub async fn list_projects_with_path_state(&self) -> Result<Vec<ProjectSummary>, NodeError> {
        let mut projects = self.db().list_projects().await?;
        self.refresh_binding_path_state(&mut projects);
        Ok(projects)
    }

    /// 自ノードの紐付けに `path_exists` (ライブ値) を書き込む。
    ///
    /// ノード側 DB の紐付けは自ノード分のみのため、他ノードの確認は行わない。
    pub fn refresh_binding_path_state(&self, projects: &mut [ProjectSummary]) {
        for project in projects.iter_mut() {
            for binding in project.bindings.iter_mut() {
                if binding.node_id == self.node_id() {
                    binding.path_exists = Path::new(&binding.local_path).is_dir();
                }
            }
        }
    }

    /// `.fxg.toml` に `project_key` を書き込み、紐付けを即時反映する
    /// (`fxg project link` / Client API 共有実装)。
    pub async fn project_link(
        &self,
        dir: &Path,
        project_id: &str,
    ) -> Result<ResolvedProject, NodeError> {
        project::patch_project_key(dir, project_id)?;
        self.resolve_and_register_project(dir).await
    }

    /// 指定ディレクトリ (省略時は `node.project_scan_dirs`) 配下の Git
    /// リポジトリを一括走査して登録する (`fxg project scan`)。
    ///
    /// 実際に走査したディレクトリ一覧を返す。
    pub async fn project_scan(&self, dir: Option<&Path>) -> Result<Vec<String>, NodeError> {
        let config =
            fxg_protocol::config::GlobalConfig::load_from_path(&self.paths().config_path())?;
        let scan_dirs: Vec<PathBuf> = match dir {
            Some(dir) => vec![dir.to_path_buf()],
            None => config
                .node
                .project_scan_dirs
                .iter()
                .map(PathBuf::from)
                .collect(),
        };
        if scan_dirs.is_empty() {
            return Err(NodeError::InvalidState(
                "no scan directories configured (set node.project_scan_dirs in config.toml)"
                    .to_owned(),
            ));
        }
        for scan_dir in &scan_dirs {
            let repositories = crate::git::find_repositories(scan_dir, SCAN_MAX_DEPTH)?;
            for repo in repositories {
                self.resolve_and_register_project(&repo).await?;
            }
        }
        Ok(scan_dirs
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect())
    }

    /// Worktree 管理情報をクリーンアップする (`git worktree prune`)。
    pub async fn worktree_prune(
        &self,
        repo: &Path,
        source: &AuditSource,
    ) -> Result<String, NodeError> {
        let output = worktree::prune_worktrees(repo).await?;
        self.record_audit(
            fxg_db::audit::actions::WORKTREE_PRUNE,
            source,
            None,
            serde_json::json!({
                "repo": repo.to_string_lossy(),
                "output": output.trim(),
            }),
        )
        .await;
        Ok(output)
    }

    /// ACP Registry カタログ + 導入状態を返す (`GET /api/v1/agents`)。
    pub async fn agents_catalog(&self) -> Result<AgentsResponse, NodeError> {
        let registry = self.session_manager().registry();
        let index = registry.index(false).await.map_err(agent_error)?;
        let node_id = self.node_id().to_owned();
        let agents = registry
            .list(&index, true)
            .into_iter()
            .map(|entry| AgentSummary {
                id: entry.id.clone(),
                name: entry.name,
                version: entry.version,
                description: entry.description,
                distributions: entry
                    .distributions
                    .iter()
                    .map(|kind| (*kind).to_owned())
                    .collect(),
                installed: entry.installed,
                custom: entry.custom,
                builtin: entry.builtin,
                installed_versions: if entry.custom || entry.builtin {
                    Vec::new()
                } else {
                    registry.installed_versions(&entry.id)
                },
                installed_nodes: if entry.installed {
                    vec![node_id.clone()]
                } else {
                    Vec::new()
                },
            })
            .collect();
        Ok(AgentsResponse { agents })
    }

    /// エージェント管理操作 (install / update / remove) を実行し、
    /// 表示メッセージを返す (`fxg agents` の Web UI 版)。
    pub async fn manage_agent(&self, action: &AgentAction) -> Result<String, NodeError> {
        let registry = self.session_manager().registry();
        match action {
            AgentAction::Install { agent_id } => {
                let id = registry.resolve_alias(agent_id);
                let index = registry.index(false).await.map_err(agent_error)?;
                let agent = index.find(&id).ok_or_else(|| {
                    NodeError::Agent(format!("agent not found in registry: {id}"))
                })?;
                if agent.distribution.binary.is_empty() {
                    return Ok(format!(
                        "{id} is distributed via npx/uvx; no explicit install required"
                    ));
                }
                let path = registry.install(&id, &index).await.map_err(agent_error)?;
                Ok(format!(
                    "installed {id} {} → {}",
                    agent.version,
                    path.display()
                ))
            }
            AgentAction::Update { agent_id } => {
                let index = registry.index(true).await.map_err(agent_error)?;
                let ids: Vec<String> = match agent_id {
                    Some(id) => vec![registry.resolve_alias(id)],
                    None => index
                        .agents
                        .iter()
                        .filter(|agent| registry.is_installed(agent))
                        .map(|agent| agent.id.clone())
                        .collect(),
                };
                if ids.is_empty() {
                    return Ok("(no installed agents)".to_owned());
                }
                let mut lines = Vec::new();
                for id in ids {
                    let Some(agent) = index.find(&id) else {
                        lines.push(format!("{id}: no longer in registry (skipped)"));
                        continue;
                    };
                    if agent.distribution.binary.is_empty() {
                        lines.push(format!("{id}: npx/uvx distribution (no update required)"));
                        continue;
                    }
                    if registry
                        .installed_versions(&id)
                        .iter()
                        .any(|version| version == &agent.version)
                    {
                        lines.push(format!("{id}: up to date ({})", agent.version));
                        continue;
                    }
                    let path = registry.install(&id, &index).await.map_err(|err| {
                        NodeError::Agent(format!("failed to update agent {id}: {err:#}"))
                    })?;
                    lines.push(format!(
                        "updated {id} → {} ({})",
                        agent.version,
                        path.display()
                    ));
                }
                Ok(lines.join("\n"))
            }
            AgentAction::Remove { agent_id } => {
                let id = registry.resolve_alias(agent_id);
                let removed = registry.remove(&id).map_err(agent_error)?;
                Ok(if removed {
                    format!("removed {id}")
                } else {
                    format!("{id} is not installed")
                })
            }
        }
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
        self.trigger_node_hello();
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
