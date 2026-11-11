//! Shared implementation of daemon operations (used by Client REST and hub relay).
//!
//! The Client REST API (`localhost:7860` /
//! hub relay) perform the same entity operations (worktree
//! management, project resolution, audit logging), so the implementation is
//! centralized here.

use std::path::{Path, PathBuf};

use fxg_acp::registry::LocalSource;
use fxg_protocol::CommandId;
use fxg_protocol::client_api::{AgentSource, AgentSummary, AgentsResponse, WorktreeInfo};
use fxg_protocol::common::{AgentAction, ProjectSummary};

use super::DaemonState;
use crate::error::NodeError;
use crate::project::{self, ResolvedProject};
use crate::worktree;

/// Maximum search depth for the bulk Git repository scan (Web UI project scan).
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
    /// Authenticated subject (`local` / `bearer:<ip>` etc.)
    pub auth_subject: String,
    /// User-Agent
    pub user_agent: Option<String>,
}

impl AuditSource {
    /// Operation originating from the local process (no request context).
    pub fn local() -> Self {
        Self {
            client_ip: "local".to_owned(),
            auth_subject: "local".to_owned(),
            user_agent: None,
        }
    }

    /// Operation from a remote client (via the hub).
    pub fn remote(node_id: &str) -> Self {
        Self {
            client_ip: "server".to_owned(),
            auth_subject: format!("node:{node_id}"),
            user_agent: None,
        }
    }
}

pub use crate::worktree::WorktreeAddOutcome;

impl DaemonState {
    /// Records an audit log entry in `node.db.audit_logs` (failures are only warned about).
    pub async fn record_audit(
        &self,
        action: fxg_protocol::common::AuditAction,
        source: &AuditSource,
        session_id: Option<&str>,
        details: impl Into<fxg_protocol::common::RawJson>,
    ) {
        let audit = fxg_db::AuditLogRecord::new(
            action,
            source.client_ip.clone(),
            source.auth_subject.clone(),
        )
        .with_session(session_id)
        .with_node_id(Some(self.node_id()))
        .with_user_agent(source.user_agent.as_deref())
        .with_details(details);
        self.db().record_audit(&audit).await;
    }

    /// Imports an existing agent session and returns the new FXG session ID.
    ///
    /// Shared by the Client REST endpoint and the hub relay so the
    /// project-binding refresh and audit entry stay identical.
    pub async fn import_session(
        &self,
        command_id: &CommandId,
        agent_id: &str,
        agent_session_id: &str,
        cwd: &Path,
        title: Option<&str>,
        source: &AuditSource,
    ) -> Result<String, NodeError> {
        self.resolve_and_register_project(cwd).await?;
        let session_id = self
            .session_manager()
            .import_session(command_id, agent_id, agent_session_id, cwd, title)
            .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_START,
            source,
            Some(&session_id),
            serde_json::json!({
                "agent_id": agent_id,
                "cwd": cwd,
                "agent_session_id": agent_session_id,
                "imported": true,
            }),
        )
        .await;
        Ok(session_id)
    }

    /// Archives/restores a session (shared implementation for Client REST / hub relay).
    ///
    /// Archiving is a reversible visibility flag; the event log is retained.
    /// Returns the archive timestamp (Unix epoch ms; `None` when restoring).
    pub async fn set_session_archived(
        &self,
        command_id: &CommandId,
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

    /// Deletes a session (shared implementation for Client REST / hub relay).
    ///
    /// A running session is stopped first, then `SessionDeleted` (tombstone) is
    /// appended and the event bodies and permission history are purged
    /// (irreversible).
    pub async fn purge_session(
        &self,
        command_id: &CommandId,
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
        let resolved = project::resolve_project(dir, self.node_id(), self.db()).await?;
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
        // (e.g. when the project_key is changed in the Web UI)
        self.db()
            .delete_other_project_bindings(
                self.node_id(),
                &resolved.local_path.to_string_lossy(),
                &resolved.project_id,
            )
            .await?;
        self.db().delete_orphan_projects().await?;
        // Report surrounding changes (worktree detection, project binding) to the hub
        self.bump_state_revision().await?;
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
                if binding.node_id.as_str() == self.node_id() {
                    binding.path_exists = Path::new(&binding.local_path).is_dir();
                }
            }
        }
    }

    /// Writes an explicit `project_key` link and applies it immediately
    /// (Client API implementation shared with the Web UI).
    ///
    /// Linking a Worktree records the link on the main Worktree root so that other
    /// Worktrees of the same repository inherit it. Existing sessions of every
    /// affected path follow the new `project_id`.
    pub async fn project_link(
        &self,
        dir: &Path,
        project_key: &str,
    ) -> Result<ResolvedProject, NodeError> {
        let project_key = project_key.strip_prefix("user:").unwrap_or(project_key);
        project::validate_project_key(project_key)?;
        let canonical = fxg_pty::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let link_root = match crate::git::toplevel(&canonical).await? {
            Some(root) => {
                if crate::git::is_worktree(&root).await.unwrap_or(false) {
                    crate::git::main_worktree_root(&root).await?.unwrap_or(root)
                } else {
                    root
                }
            }
            None => canonical.clone(),
        };
        let link_path = link_root.to_string_lossy().into_owned();
        self.db()
            .upsert_project_path_link(&link_path, project_key)
            .await?;

        // Re-resolve every path this node knows about so Worktrees and sessions
        // follow the new identity.
        let mut paths: Vec<PathBuf> = vec![link_root.clone(), canonical.clone()];
        let sessions = self
            .db()
            .list_sessions(&fxg_db::SessionFilter {
                include_archived: true,
                ..fxg_db::SessionFilter::default()
            })
            .await?;
        for session in sessions {
            if session.node_id.as_str() == self.node_id() {
                paths.push(PathBuf::from(&session.local_path));
            }
        }
        paths.sort();
        paths.dedup();

        let mut affected: Vec<(String, String)> = Vec::new();
        for path in paths {
            match self.resolve_and_register_project(&path).await {
                Ok(resolved) => affected.push((
                    resolved.local_path.to_string_lossy().into_owned(),
                    resolved.project_id,
                )),
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        "failed to re-resolve project after link: {err}"
                    );
                }
            }
        }
        affected.sort();
        affected.dedup();
        self.db()
            .reassign_sessions_project(self.node_id(), &affected)
            .await?;
        for (path, project_id) in &affected {
            self.db()
                .delete_other_project_bindings(self.node_id(), path, project_id)
                .await?;
        }
        self.db().delete_orphan_projects().await?;
        self.bump_state_revision().await?;

        self.resolve_and_register_project(&canonical).await
    }

    /// Scans and registers Git repositories under the given directory (or
    /// `node.project_scan_dirs` when omitted).
    ///
    /// Returns the list of directories actually scanned.
    pub async fn project_scan(&self, dir: Option<&Path>) -> Result<Vec<String>, NodeError> {
        let scan_dirs: Vec<PathBuf> = match dir {
            Some(dir) => vec![dir.to_path_buf()],
            None => self
                .node_settings()
                .project_scan_dirs
                .iter()
                .map(PathBuf::from)
                .collect(),
        };
        if scan_dirs.is_empty() {
            return Err(NodeError::InvalidState(
                "no scan directories configured (set project_scan_dirs in the node settings)"
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

    /// Returns the available agents and registry import candidates
    /// (`GET /api/v1/agents`).
    pub async fn agents_catalog(&self) -> Result<AgentsResponse, NodeError> {
        let catalog = self.session_manager().catalog();
        let node_id = self.node_id().to_owned();
        let agents = catalog
            .list()
            .into_iter()
            .map(|entry| {
                let available = entry.is_available();
                let mut sources = Vec::new();
                if entry.custom.is_some() {
                    sources.push(AgentSource::Custom);
                }
                let mut external_path = None;
                for source in &entry.sources {
                    if let LocalSource::External(path) = source {
                        external_path = Some(path.to_string_lossy().into_owned());
                        sources.push(AgentSource::External);
                    }
                }
                AgentSummary {
                    id: entry.id,
                    name: entry.name,
                    version: entry.version,
                    description: entry.description,
                    sources,
                    external_path,
                    custom_config: entry.custom,
                    backend: entry.backend,
                    icon: entry.icon,
                    installed_nodes: if available {
                        vec![node_id.clone()]
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect();

        // The registry is only used by the import dialog. A failed or slow
        // fetch must not hide the built-in and custom agents above.
        let custom_ids: std::collections::BTreeSet<String> =
            catalog.custom_agents().into_keys().collect();
        let registry =
            match tokio::time::timeout(std::time::Duration::from_secs(5), catalog.index(false))
                .await
            {
                Ok(Ok(index)) => index
                    .agents
                    .iter()
                    .map(|agent| {
                        catalog
                            .registry()
                            .import_summary(agent, custom_ids.contains(&agent.id))
                    })
                    .collect(),
                Ok(Err(err)) => {
                    tracing::warn!("failed to load ACP registry: {err:#}");
                    Vec::new()
                }
                Err(_) => {
                    tracing::warn!("timed out while loading the ACP registry import catalog");
                    Vec::new()
                }
            };
        Ok(AgentsResponse { agents, registry })
    }

    /// Runs an agent management action and returns a display message.
    ///
    /// On success, updates the node's own `nodes` row (the list of available agents).
    pub async fn manage_agent(&self, action: &AgentAction) -> Result<String, NodeError> {
        let message = self.manage_agent_action(action).await?;
        self.bump_state_revision().await?;
        Ok(message)
    }

    /// The body of [`Self::manage_agent`] (does not update the node row).
    async fn manage_agent_action(&self, action: &AgentAction) -> Result<String, NodeError> {
        let catalog = self.session_manager().catalog();
        match action {
            AgentAction::RefreshCatalog => {
                catalog.index(true).await.map_err(agent_error)?;
                Ok("refreshed catalog".to_owned())
            }
            AgentAction::UpsertCustom { agent_id, config } => {
                if agent_id.trim().is_empty()
                    || agent_id.contains(|ch: char| ch.is_whitespace() || ch == '/' || ch == '\\')
                {
                    return Err(NodeError::Agent(format!(
                        "invalid custom agent id: {agent_id:?}"
                    )));
                }
                if config.command.trim().is_empty() {
                    return Err(NodeError::Agent(format!(
                        "custom agent {agent_id} requires a non-empty command"
                    )));
                }
                if catalog.is_driver_backed(agent_id) {
                    return Err(NodeError::Agent(format!(
                        "custom agent id {agent_id} is reserved by a built-in driver"
                    )));
                }
                if let Some(extension) = config.extension.as_deref()
                    && catalog.extension(extension).is_none()
                {
                    return Err(NodeError::Agent(format!(
                        "custom agent {agent_id} references an unknown extension: {extension}"
                    )));
                }
                self.db().upsert_custom_agent(agent_id, config).await?;
                self.refresh_agent_catalog().await?;
                Ok(format!(
                    "saved custom agent {agent_id} ({})",
                    config.command
                ))
            }
            AgentAction::RemoveCustom { agent_id } => {
                let removed = self.db().delete_custom_agent(agent_id).await? > 0;
                self.refresh_agent_catalog().await?;
                Ok(if removed {
                    format!("removed custom agent {agent_id}")
                } else {
                    format!("custom agent {agent_id} is not defined")
                })
            }
            AgentAction::ImportRegistryAgent {
                agent_id,
                extension,
            } => {
                let id = catalog.resolve_alias(agent_id);
                if catalog.is_driver_backed(&id) {
                    return Err(NodeError::Agent(format!(
                        "agent id {id} is reserved by a built-in driver"
                    )));
                }
                if catalog.custom_agents().contains_key(&id) {
                    return Err(NodeError::Agent(format!(
                        "custom agent {id} already exists; edit or remove it instead"
                    )));
                }
                if let Some(extension) = extension.as_deref()
                    && catalog.extension(extension).is_none()
                {
                    return Err(NodeError::Agent(format!(
                        "registry agent {id} references an unknown extension: {extension}"
                    )));
                }
                let index = catalog.index(false).await.map_err(agent_error)?;
                let agent = index.find(&id).ok_or_else(|| {
                    NodeError::Agent(format!("agent not found in registry: {id}"))
                })?;
                let config = catalog
                    .registry()
                    .import_custom_config(agent, extension.clone())
                    .map_err(agent_error)?;
                self.db().upsert_custom_agent(&id, &config).await?;
                self.refresh_agent_catalog().await?;
                Ok(format!(
                    "imported {id} from the registry ({})",
                    config.command
                ))
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
            .find(|project| project.project_id.as_str() == project_id)
            .ok_or_else(|| NodeError::InvalidWorktree(format!("unknown project: {project_id}")))?;
        let mut bindings: Vec<_> = project
            .bindings
            .iter()
            .filter(|binding| binding.node_id.as_str() == self.node_id())
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
                                binding.node_id.as_str() == self.node_id() && !binding.is_worktree
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
                    node_id: self.node_id().into(),
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

        let outcome = worktree::ensure_worktree(&worktree::WorktreeAddRequest {
            repo: &repo_root,
            project_id: &resolved.project_id,
            branch,
            base_branch: base_branch.as_deref(),
            fxg_home: self.paths().fxg_home(),
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

        Ok(outcome)
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
                    .filter(|binding| {
                        binding.node_id.as_str() == self.node_id() && !binding.is_worktree
                    })
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

// ----------------------------------------------------------------------
// Typed settings snapshots and secrets
// ----------------------------------------------------------------------

use fxg_protocol::SecretId;
use fxg_protocol::config::{GlobalConfig, process_env};
use fxg_protocol::settings::{
    ConfigSource, NodeBootstrapView, NodeSettingsSnapshot, SecretSummary, SourcedValue,
    UpsertSecretRequest, sourced_option, sourced_string,
};

impl DaemonState {
    /// Builds the typed settings snapshot exposed by
    /// `GET /api/v1/nodes/{node_id}/settings`.
    pub fn node_settings_snapshot(&self) -> NodeSettingsSnapshot {
        let config = match GlobalConfig::load_from_path(&self.paths().config_path()) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!("failed to read config.toml for the settings view: {err}");
                GlobalConfig::default()
            }
        };
        let env = process_env;
        let fxg_home_source = if env(fxg_protocol::config::ENV_FXG_HOME).is_some() {
            ConfigSource::Env
        } else {
            ConfigSource::Default
        };
        let fxg_home = SourcedValue::new(
            self.paths().fxg_home().to_string_lossy().into_owned(),
            fxg_home_source.clone(),
            (fxg_home_source == ConfigSource::Env)
                .then(|| fxg_protocol::config::ENV_FXG_HOME.to_owned()),
        );

        let node_id = sourced_string(
            &env,
            self.config().node_id.clone(),
            fxg_protocol::config::env_keys::NODE_ID,
            config.node.node_id.clone(),
            "node.node_id",
            None,
            None,
        );
        let listen_addr = sourced_string(
            &env,
            self.config().listen_addr.clone(),
            fxg_protocol::config::env_keys::NODE_LISTEN_ADDR,
            config.node.listen_addr.clone(),
            "node.listen_addr",
            Some(fxg_protocol::config::NodeBootstrapConfig::DEFAULT_LISTEN_ADDR),
            Some("--listen"),
        );
        let mut hub_url = sourced_option(
            &env,
            self.config().hub_url.clone(),
            fxg_protocol::config::env_keys::HUB_URL,
            config.node.hub_url.clone(),
            "node.hub_url",
            Some("--hub-url"),
        );
        hub_url.value = hub_url
            .value
            .map(|value| fxg_protocol::config::hub_http_origin(&value));

        let log = &self.config().log;
        let log_level = sourced_string(
            &env,
            log.level.clone().unwrap_or_else(|| "info".to_owned()),
            fxg_protocol::config::env_keys::LOG_LEVEL,
            config.log.level.clone(),
            "log.level",
            Some("info"),
            Some("--log-level"),
        );
        let log_file = sourced_option(
            &env,
            log.file.clone().filter(|value| value != "-"),
            fxg_protocol::config::env_keys::LOG_FILE,
            config.log.file.clone(),
            "log.file",
            Some("--log-file"),
        );
        let log_format = sourced_string(
            &env,
            log.format.clone().unwrap_or_else(|| "text".to_owned()),
            fxg_protocol::config::env_keys::LOG_FORMAT,
            config.log.format.clone(),
            "log.format",
            Some("text"),
            Some("--log-format"),
        );
        let log_no_color = SourcedValue::new(
            log.no_color,
            if log.no_color == config.log.no_color {
                ConfigSource::ConfigFile
            } else {
                ConfigSource::Cli
            },
            Some("--no-color"),
        );

        NodeSettingsSnapshot {
            bootstrap: NodeBootstrapView {
                fxg_home,
                node_id,
                listen_addr,
                hub_url,
                log_level,
                log_file,
                log_format,
                log_no_color,
            },
            node: self.node_settings(),
            agents: self.agent_settings(),
        }
    }

    /// Lists secret metadata with `used_by` and `configured` filled in.
    pub async fn node_secrets(&self) -> Result<Vec<SecretSummary>, NodeError> {
        Ok(fxg_db::secrets::list_secrets(self.db(), self.paths().fxg_home()).await?)
    }

    /// Stores a node-owned secret (write-only).
    pub async fn upsert_node_secret(
        &self,
        id: &SecretId,
        request: &UpsertSecretRequest,
    ) -> Result<Vec<SecretSummary>, NodeError> {
        Ok(fxg_db::secrets::upsert_secret(self.db(), self.paths().fxg_home(), id, request).await?)
    }

    /// Deletes a node-owned secret after checking references.
    pub async fn delete_node_secret(&self, id: &SecretId) -> Result<Vec<SecretSummary>, NodeError> {
        Ok(fxg_db::secrets::delete_secret(self.db(), self.paths().fxg_home(), id).await?)
    }
}
