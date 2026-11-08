//! Shared implementation of daemon operations (used by both IPC and Client REST).
//!
//! The local IPC (`fxg` CLI) and the Client REST API (`localhost:7860` /
//! central server relay) perform the same entity operations (worktree
//! management, project resolution, audit logging), so the implementation is
//! centralized here.

#![allow(
    clippy::disallowed_types,
    reason = "audit details are display-only opaque JSON; see docs/06-type-safety.md"
)]

use std::path::{Path, PathBuf};

use fxg_acp::registry::{AcpRegistry, LocalSource};
use fxg_protocol::client_api::{AgentSource, AgentSummary, AgentsResponse, WorktreeInfo};
use fxg_protocol::common::{AgentAction, HookLogEntry, ProjectSummary};

use super::DaemonState;
use crate::error::NodeError;
use crate::project::{self, ResolvedProject};
use crate::worktree;

/// Maximum search depth for the bulk Git repository scan (`fxg project scan`).
const SCAN_MAX_DEPTH: usize = 3;

/// Converts an ACP Registry operation error into [`NodeError::Agent`].
fn agent_error(err: anyhow::Error) -> NodeError {
    NodeError::Agent(format!("{err:#}"))
}

/// Audit log source (client information).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditSource {
    /// Source IP address
    pub client_ip: String,
    /// Authenticated subject (`local-ipc` / `bearer:<ip>` etc.)
    pub auth_subject: String,
    /// User-Agent
    pub user_agent: Option<String>,
}

impl AuditSource {
    /// Operation originating from the local CLI (IPC).
    pub fn local() -> Self {
        Self {
            client_ip: "local".to_owned(),
            auth_subject: "local-ipc".to_owned(),
            user_agent: None,
        }
    }

    /// Operation from a remote client (via the central server).
    pub fn remote(node_id: &str) -> Self {
        Self {
            client_ip: "server".to_owned(),
            auth_subject: format!("node:{node_id}"),
            user_agent: None,
        }
    }
}

/// Worktree creation result.
#[derive(Debug, Clone)]
pub struct WorktreeAddOutcome {
    /// Created/reused path
    pub path: PathBuf,
    /// Checked-out branch
    pub branch: String,
    /// Whether it was newly created (`false` means an existing worktree was reused)
    pub created: bool,
    /// Execution log of the `post_create` hook
    pub hook_logs: Vec<HookLogEntry>,
}

impl DaemonState {
    /// Records an audit log entry in `node.db.audit_logs` (failures are only warned about).
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

    /// Archives/restores a session (shared implementation for IPC / Client REST / central server relay).
    ///
    /// Archiving is a reversible visibility flag; the event log is retained.
    /// Returns the archive timestamp (Unix epoch ms; `None` when restoring).
    pub async fn set_session_archived(
        &self,
        command_id: &str,
        session_id: &str,
        archived: bool,
        source: &AuditSource,
    ) -> Result<Option<i64>, NodeError> {
        let archived_at = self
            .session_manager()
            .set_archived(command_id, session_id, archived)
            .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_ARCHIVE,
            source,
            Some(session_id),
            serde_json::json!({ "archived": archived, "archived_at": archived_at }),
        )
        .await;
        Ok(archived_at)
    }

    /// Deletes a session (shared implementation for IPC / Client REST / central server relay).
    ///
    /// A running session is stopped first, then `SessionDeleted` (tombstone) is
    /// appended and the event bodies and permission history are purged
    /// (irreversible).
    pub async fn purge_session(
        &self,
        command_id: &str,
        session_id: &str,
        source: &AuditSource,
    ) -> Result<(), NodeError> {
        self.session_manager()
            .delete(command_id, session_id)
            .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_DELETE,
            source,
            Some(session_id),
            serde_json::json!({}),
        )
        .await;
        Ok(())
    }

    /// Resolves a project and updates `projects` / `project_node_bindings`.
    pub async fn resolve_and_register_project(
        &self,
        dir: &Path,
    ) -> Result<ResolvedProject, NodeError> {
        let resolved = project::resolve_project(dir, self.node_id()).await?;
        self.db()
            .upsert_project(&resolved.to_project_record())
            .await?;

        // Record whether this runs inside a worktree and the current branch
        // (if retrieval fails, register it as the main repository)
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

        // Do not leave the old project binding behind when re-linking
        // (e.g. when the project_key is changed via `fxg project link`)
        self.db()
            .delete_other_project_bindings(
                self.node_id(),
                &resolved.local_path.to_string_lossy(),
                &resolved.project_id,
            )
            .await?;
        self.db().delete_orphan_projects().await?;
        // Report surrounding changes (worktree detection, project binding) to the hub
        self.trigger_node_hello();
        Ok(resolved)
    }

    /// Returns the project list with a live check of path existence for the bindings of this node.
    ///
    /// Path existence depends on external factors (manual worktree deletion,
    /// external drive removal, etc.), so it is checked when returning to clients.
    pub async fn list_projects_with_path_state(&self) -> Result<Vec<ProjectSummary>, NodeError> {
        let mut projects = self.db().list_projects().await?;
        self.refresh_binding_path_state(&mut projects);
        Ok(projects)
    }

    /// Writes `path_exists` (live value) into this node's bindings.
    ///
    /// The node-side DB only holds this node's bindings, so other nodes are not checked.
    pub fn refresh_binding_path_state(&self, projects: &mut [ProjectSummary]) {
        for project in projects.iter_mut() {
            for binding in project.bindings.iter_mut() {
                if binding.node_id == self.node_id() {
                    binding.path_exists = Path::new(&binding.local_path).is_dir();
                }
            }
        }
    }

    /// Writes `project_key` into `.fxg.toml` and applies the binding immediately
    /// (`fxg project link` / Client API shared implementation).
    pub async fn project_link(
        &self,
        dir: &Path,
        project_id: &str,
    ) -> Result<ResolvedProject, NodeError> {
        project::patch_project_key(dir, project_id)?;
        self.resolve_and_register_project(dir).await
    }

    /// Scans and registers Git repositories under the given directory (or
    /// `node.project_scan_dirs` when omitted) (`fxg project scan`).
    ///
    /// Returns the list of directories actually scanned.
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

    /// Cleans up worktree administrative data (`git worktree prune`).
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

    /// Returns the ACP Registry catalog plus install state (`GET /api/v1/agents`).
    pub async fn agents_catalog(&self) -> Result<AgentsResponse, NodeError> {
        let registry = self.session_manager().registry();
        // Custom definitions should still be returnable even if the registry cannot be fetched
        let index = registry.index(false).await.unwrap_or_else(|err| {
            tracing::warn!("failed to load ACP registry: {err:#}");
            Default::default()
        });
        let node_id = self.node_id().to_owned();
        let agents = registry
            .list(&index, true)
            .into_iter()
            .map(|entry| {
                let available = entry.is_available();
                let mut sources = Vec::new();
                if entry.custom.is_some() {
                    sources.push(AgentSource::Custom);
                }
                let mut external_path = None;
                for source in &entry.sources {
                    match source {
                        LocalSource::External(path) => {
                            external_path = Some(path.to_string_lossy().into_owned());
                            sources.push(AgentSource::External);
                        }
                        LocalSource::Npx | LocalSource::Uvx => {
                            if !sources.contains(&AgentSource::Package) {
                                sources.push(AgentSource::Package);
                            }
                        }
                    }
                }
                AgentSummary {
                    id: entry.id,
                    name: entry.name,
                    version: entry.version,
                    description: entry.description,
                    sources,
                    registry: entry.registry,
                    package_available: entry.package_available,
                    external_path,
                    custom_config: entry.custom,
                    installed_nodes: if available {
                        vec![node_id.clone()]
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect();
        Ok(AgentsResponse { agents })
    }

    /// Runs an agent management action and returns a display message (Web UI version of `fxg agents`).
    ///
    /// On success, updates the node's own `nodes` row (the list of available agents).
    pub async fn manage_agent(&self, action: &AgentAction) -> Result<String, NodeError> {
        let message = self.manage_agent_action(action).await?;
        self.register_node().await?;
        Ok(message)
    }

    /// The body of [`Self::manage_agent`] (does not update the node row).
    async fn manage_agent_action(&self, action: &AgentAction) -> Result<String, NodeError> {
        let registry = self.session_manager().registry();
        match action {
            AgentAction::RefreshCatalog => {
                registry.index(true).await.map_err(agent_error)?;
                Ok("refreshed catalog".to_owned())
            }
            AgentAction::UpsertCustom { agent_id, config } => {
                registry
                    .upsert_custom(agent_id, config.clone())
                    .map_err(agent_error)?;
                Ok(format!(
                    "saved custom agent {agent_id} ({})",
                    config.command
                ))
            }
            AgentAction::RemoveCustom { agent_id } => {
                let removed = registry.remove_custom(agent_id).map_err(agent_error)?;
                Ok(if removed {
                    format!("removed custom agent {agent_id}")
                } else {
                    format!("custom agent {agent_id} is not defined in the UI store")
                })
            }
            AgentAction::SetPackageEnabled { agent_id, enabled } => {
                let id = registry.resolve_alias(agent_id);
                if !enabled {
                    registry
                        .set_package_enabled(&id, false)
                        .map_err(agent_error)?;
                    return Ok(format!("disabled npx/uvx for {id}"));
                }
                let index = registry.index(false).await.map_err(agent_error)?;
                let agent = index.find(&id).ok_or_else(|| {
                    NodeError::Agent(format!("agent not found in registry: {id}"))
                })?;
                let runner = AcpRegistry::package_runner(agent).ok_or_else(|| {
                    NodeError::Agent(format!(
                        "{id} cannot run via npx/uvx on this node (npx/uvx not found in PATH, or no package distribution)"
                    ))
                })?;
                registry
                    .set_package_enabled(&id, true)
                    .map_err(agent_error)?;
                Ok(format!("enabled {id} via {runner}"))
            }
        }
    }

    /// Resolves the local path of the project's main repository (not a worktree).
    ///
    /// If there is no main binding, returns the path of the first binding.
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

    /// Returns the worktree list (resolved from `cwd` or the main repositories of registered projects).
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

    /// Creates a new worktree and updates the binding and audit log.
    ///
    /// If `expected_project_id` is given and does not match the resolution of
    /// `repo`, the request is rejected (detects mistakes from the CLI).
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

        // Register the worktree as a node binding
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

    /// Removes a worktree and updates the binding and audit log.
    ///
    /// Without `repo_hint`, searches the worktree lists of registered main
    /// repositories for the repository containing `target`.
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
        if let Err(err) = self
            .db()
            .delete_project_binding(self.node_id(), &target.to_string_lossy())
            .await
        {
            tracing::warn!(
                path = %target.display(),
                "worktree removed but its project binding could not be deleted: {err}"
            );
        }

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

    /// Finds the main repository containing the worktree at `target`.
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
        let canonical_target = match fxg_pty::canonicalize(target) {
            Ok(path) => Some(path),
            Err(err) => {
                tracing::debug!(
                    path = %target.display(),
                    "failed to canonicalize the target worktree: {err}"
                );
                None
            }
        };
        let mut failures = Vec::new();
        for root in main_roots {
            let entries = match worktree::list_worktrees(&root).await {
                Ok(entries) => entries,
                Err(err) => {
                    tracing::warn!(
                        root = %root.display(),
                        "failed to list worktrees while resolving {}: {err}",
                        target.display()
                    );
                    failures.push(format!("{}: {err}", root.display()));
                    continue;
                }
            };
            let found = entries.iter().any(|entry| {
                let canonical_entry = match fxg_pty::canonicalize(&entry.path) {
                    Ok(path) => Some(path),
                    Err(err) => {
                        tracing::debug!(
                            path = %entry.path.display(),
                            "failed to canonicalize a worktree entry: {err}"
                        );
                        None
                    }
                };
                entry.path == target
                    || (canonical_target.is_some() && canonical_entry == canonical_target)
            });
            if found {
                return Ok(root);
            }
        }
        let detail = if failures.is_empty() {
            String::new()
        } else {
            format!(
                " (some repositories could not be inspected: {})",
                failures.join("; ")
            )
        };
        Err(NodeError::InvalidWorktree(format!(
            "worktree not found: {}{detail}",
            target.display()
        )))
    }
}
