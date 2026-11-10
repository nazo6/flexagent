//! Client API backend implementation for the local node (`fxg node start`).
//!
//! Supplies the daemon's `SessionManager` / `PtySessionManager` / Git operations
//! to the shared router of `fxg-server` ([`fxg_hub::api::client_router`])
//! (design: `docs/03-protocol-and-api.md` §3).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use fxg_hub::api::{
    ApiError, ClientApiBackend, ClientCommand, ClientEvent, ClientInfo, PtyChannelError,
    PtyChannelEvent, PtySpawnParams, SystemExtras,
};
use fxg_protocol::CommandId;
use fxg_protocol::client_api::{
    AgentDiagnostics, AgentOpResponse, AgentsResponse, ConnectionRole, CreateSessionRequest,
    CreateSessionResponse, CreateWorktreeRequest, DiagnosticsResponse, KillSwitchResponse,
    ProjectLinkRequest, ProjectLinkResponse, ProjectScanRequest, ProjectScanResponse,
    ProjectUnlinkRequest, ProjectUnlinkResponse, ProvisionersResponse, PruneWorktreesRequest,
    RemoveWorktreeRequest, RespondElicitationRequest, RespondElicitationResponse,
    RespondPermissionRequest, RespondPermissionResponse, ResumeSessionRequest,
    ResumeSessionResponse, RotateAuthTokenResponse, SessionArchiveRequest, SessionArchiveResponse,
    SessionImportRequest, SessionImportResponse, SessionRenameRequest, SessionRenameResponse,
    SupportBundle, SupportBundleConfig, WorktreeInfo, WorktreesResponse,
};
use fxg_protocol::common::{
    AgentAction, CommandResult, ComponentHealth, ComponentName, ComponentState, DiffScope,
    ProjectSummary, WorkspaceDiffResponse,
};
use tokio::sync::broadcast;

use super::DaemonState;
use super::ops::AuditSource;
use crate::error::NodeError;
use crate::session_manager::{ResumeParams, StartSessionParams};

/// Buffer for the local subscription forwarded to the PTY WS.
const PTY_CHANNEL_CAPACITY: usize = 1024;

/// Converts a node error into an API error.
fn api_error(err: NodeError) -> ApiError {
    ApiError::from_code(err.error_code(), err.to_string())
}

/// Converts a `ClientInfo` into the audit log source.
fn audit_source(client: &ClientInfo) -> AuditSource {
    AuditSource {
        client_ip: client.ip.clone(),
        auth_subject: client.auth_subject.clone(),
        user_agent: client.user_agent.clone(),
    }
}

impl DaemonState {
    /// Shared diagnostics payload (REST).
    pub async fn diagnostics_response(&self) -> Result<DiagnosticsResponse, ApiError> {
        <Self as ClientApiBackend>::diagnostics(self).await
    }

    /// Redacted support bundle (REST).
    pub async fn support_bundle_response(&self) -> Result<SupportBundle, ApiError> {
        <Self as ClientApiBackend>::support_bundle(self).await
    }
}

#[async_trait]
impl ClientApiBackend for DaemonState {
    fn db(&self) -> &fxg_db::Db {
        DaemonState::db(self)
    }

    fn auth_token(&self) -> String {
        DaemonState::auth_token(self)
    }

    fn connection_role(&self) -> ConnectionRole {
        ConnectionRole::LocalNode
    }

    fn local_node_id(&self) -> Option<fxg_protocol::NodeId> {
        Some(fxg_protocol::NodeId::from(self.node_id()))
    }

    fn request_shutdown(&self) -> bool {
        DaemonState::request_shutdown(self);
        true
    }

    async fn execute_node_rpc(
        &self,
        node_id: &fxg_protocol::NodeId,
        request: fxg_protocol::node_hub::NodeRpcRequest,
        client: Option<ClientInfo>,
    ) -> Result<fxg_protocol::node_hub::NodeRpcResponse, ApiError> {
        if node_id.as_str() != self.node_id() {
            return Err(ApiError::not_found(format!(
                "requested node {} but this node is {}",
                node_id.as_str(),
                self.node_id()
            )));
        }
        let source = client
            .as_ref()
            .map(audit_source)
            .unwrap_or_else(AuditSource::local);
        let request_id = CommandId::from(fxg_protocol::util::uuid_v7());
        match crate::rpc::execute(self, &request_id, request, &source).await {
            fxg_protocol::node_hub::NodeRpcOutcome::Ok { response } => Ok(response),
            fxg_protocol::node_hub::NodeRpcOutcome::Err { error } => {
                Err(ApiError::from_info(error))
            }
        }
    }

    /// The local node returns a list verified live for the existence of linked paths.
    async fn list_projects(&self) -> Result<Vec<ProjectSummary>, ApiError> {
        self.list_projects_with_path_state()
            .await
            .map_err(api_error)
    }

    fn diagnostics_counters(&self) -> Option<&fxg_hub::api::DiagnosticsCounters> {
        Some(DaemonState::diagnostics_counters(self))
    }

    fn fxg_home(&self) -> Option<&std::path::Path> {
        Some(DaemonState::paths(self).fxg_home())
    }

    async fn role_health(&self, now: i64) -> Vec<ComponentHealth> {
        let mut sync = if self.config().hub_url.is_none() {
            ComponentHealth {
                component: ComponentName::Sync,
                state: ComponentState::NotMeasured,
                message: Some("hub is not configured (local-only mode)".to_owned()),
                last_ok_at: None,
                last_error_at: None,
            }
        } else if self.hub_connected() {
            ComponentHealth::ok(ComponentName::Sync, now)
        } else {
            ComponentHealth::failed(
                ComponentName::Sync,
                ComponentState::Degraded,
                "not connected to the hub",
                now,
            )
        };
        match self.db().unsynced_event_count().await {
            Ok(backlog) if backlog > 1000 => {
                sync = ComponentHealth::failed(
                    ComponentName::Sync,
                    ComponentState::Degraded,
                    format!("{backlog} events are waiting in the outbox"),
                    now,
                );
            }
            Ok(_) => {}
            Err(err) => {
                sync = ComponentHealth::failed(
                    ComponentName::Sync,
                    ComponentState::Error,
                    format!("failed to read the outbox backlog: {err}"),
                    now,
                );
            }
        }
        vec![sync]
    }

    fn agent_diagnostics(&self) -> Vec<AgentDiagnostics> {
        self.session_manager()
            .agent_diagnostics()
            .into_iter()
            .map(|(agent_id, stderr_tail)| AgentDiagnostics {
                agent_id,
                stderr_tail,
            })
            .collect()
    }

    fn support_bundle_config(&self) -> SupportBundleConfig {
        let config = self.config();
        SupportBundleConfig {
            fxg_home: DaemonState::paths(self)
                .fxg_home()
                .to_string_lossy()
                .into_owned(),
            log_level: config.log.level.clone(),
            log_file: config.log.file.clone(),
            log_format: config.log.format.clone(),
            node_listen_addr: Some(config.listen_addr.clone()),
            hub_url: config.hub_url.clone(),
            hub_listen_addr: None,
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
        }
    }

    fn support_bundle_logs(&self, lines: usize) -> Vec<String> {
        let path = DaemonState::paths(self)
            .fxg_home()
            .join("logs")
            .join("node.log");
        fxg_hub::api::log_tail(&path, lines)
    }

    async fn system_extras(&self) -> Result<SystemExtras, ApiError> {
        let unsynced = self
            .db()
            .unsynced_event_count()
            .await
            .map_err(ApiError::from)?;
        Ok(SystemExtras {
            vapid_public_key: None,
            unsynced_event_count: Some(unsynced),
            hub_connected: Some(self.hub_connected()),
            last_synced_at: self.db().last_synced_at().await.map_err(ApiError::from)?,
        })
    }

    fn subscribe_client_events(&self) -> broadcast::Receiver<ClientEvent> {
        self.inner.client_events.subscribe()
    }

    async fn dispatch_command(&self, command: ClientCommand) -> CommandResult {
        let manager = self.session_manager();
        let (command_id, result) = match command {
            ClientCommand::SendPrompt {
                command_id,
                session_id,
                input,
                client_source,
            } => {
                let result = manager
                    .send_prompt(&command_id, session_id.as_str(), &input, client_source)
                    .await
                    .map(|()| session_id);
                (command_id, result)
            }
            ClientCommand::ControlSession {
                command_id,
                session_id,
                action,
            } => {
                let result = manager
                    .control(&command_id, session_id.as_str(), &action)
                    .await
                    .map(|()| session_id);
                (command_id, result)
            }
        };

        match result {
            Ok(session_id) => CommandResult::success(command_id, session_id),
            Err(err) => CommandResult::failure(command_id, err.info()),
        }
    }

    async fn create_session(
        &self,
        request: CreateSessionRequest,
        client: ClientInfo,
    ) -> Result<CreateSessionResponse, ApiError> {
        if request.provisioner.is_some() {
            return Err(ApiError::bad_request(
                "provisioners are only available on the hub",
            ));
        }
        if let Some(node_id) = &request.node_id
            && node_id.as_str() != self.node_id()
        {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let source = audit_source(&client);

        // ACP-native fork from an existing session on this node. The source and
        // the new session share the same agent (the hub already validates it).
        let fork_source = match &request.fork {
            Some(fork) => {
                let source = self
                    .db()
                    .get_session(fork.from_session_id.as_str())
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| {
                        ApiError::not_found(format!("session not found: {}", fork.from_session_id))
                    })?;
                // Refresh the binding of the fork source's working directory.
                self.resolve_and_register_project(Path::new(&source.local_path))
                    .await
                    .map_err(api_error)?;
                Some(source)
            }
            None => None,
        };

        // Working directory: create a worktree at the same time, use an existing
        // path, or fall back to the node default
        // (when creating a worktree simultaneously, `worktree_add` has already
        //  registered the binding, so only existing-path and default resolution
        //  are registered again here)
        let (local_path, register_binding) = match (&request.worktree, &request.local_path) {
            (Some(spec), _) => {
                let repo = self
                    .project_main_repo(request.project_id.as_str())
                    .await
                    .map_err(api_error)?;
                let outcome = self
                    .worktree_add(
                        &repo,
                        Some(request.project_id.as_str()),
                        &spec.branch,
                        spec.base_branch.clone(),
                        &source,
                    )
                    .await
                    .map_err(api_error)?;
                (outcome.path, false)
            }
            (None, Some(path)) => (PathBuf::from(path), true),
            (None, None) => {
                if let Some(source) = fork_source.as_ref() {
                    (PathBuf::from(&source.local_path), false)
                } else {
                    let path = self
                        .db()
                        .resolve_default_local_path(
                            &request.project_id,
                            &fxg_protocol::NodeId::from(self.node_id()),
                        )
                        .await
                        .map_err(ApiError::from)?
                        .ok_or_else(|| {
                            ApiError::not_found(format!(
                                "no default directory for project {} on this node; \
                                 specify local_path or create a worktree",
                                request.project_id
                            ))
                        })?;
                    (PathBuf::from(path), true)
                }
            }
        };

        if register_binding {
            // Refresh the working directory binding (last_used_at / branch)
            self.resolve_and_register_project(&local_path)
                .await
                .map_err(api_error)?;
        }

        let session_id = fxg_protocol::util::uuid_v7();
        let agent_id = fork_source.as_ref().map_or_else(
            || request.agent_id.clone(),
            |source| source.agent_id.clone(),
        );
        let outcome = self
            .session_manager()
            .start_session(StartSessionParams {
                command_id: &request.command_id,
                session_id: session_id.as_str(),
                local_path: &local_path,
                agent_id: &agent_id,
                initial_prompt: request.initial_prompt.as_ref(),
                initial_config: &request.initial_config,
                mode: request.mode.as_deref(),
                extra_args: request.extra_args.as_deref(),
                fork_from_session_id: fork_source
                    .as_ref()
                    .map(|source| source.session_id.as_str()),
            })
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_START,
            &source,
            Some(&outcome.session_id),
            serde_json::json!({
                "agent_id": agent_id,
                "local_path": local_path.to_string_lossy(),
                "fork_from": fork_source.as_ref().map(|source| source.session_id.as_str()),
            }),
        )
        .await;

        Ok(CreateSessionResponse {
            session_id: outcome.session_id.into(),
            command_id: request.command_id,
        })
    }

    async fn import_session(
        &self,
        request: SessionImportRequest,
        client: ClientInfo,
    ) -> Result<SessionImportResponse, ApiError> {
        if let Some(node_id) = &request.node_id
            && node_id.as_str() != self.node_id()
        {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let source = audit_source(&client);
        let session_id = DaemonState::import_session(
            self,
            &request.command_id,
            &request.agent_id,
            &request.agent_session_id,
            Path::new(&request.cwd),
            request.title.as_deref(),
            &source,
        )
        .await
        .map_err(api_error)?;
        Ok(SessionImportResponse {
            session_id: session_id.into(),
            command_id: request.command_id,
        })
    }

    async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        request: RespondPermissionRequest,
        client: ClientInfo,
    ) -> Result<RespondPermissionResponse, ApiError> {
        let selected = self
            .resolve_permission_option(session_id, request_id, &request)
            .await?;
        let command_id = CommandId::from(fxg_protocol::util::uuid_v7());
        match self
            .session_manager()
            .respond_permission(
                &command_id,
                session_id,
                request_id,
                &selected,
                request.resolved_by.clone(),
            )
            .await
        {
            Ok(()) => {
                self.record_audit(
                    fxg_db::audit::actions::PERMISSION_RESOLVED,
                    &audit_source(&client),
                    Some(session_id),
                    serde_json::json!({
                        "request_id": request_id,
                        "selected_option_id": selected,
                        "resolved_by": request.resolved_by,
                    }),
                )
                .await;
                Ok(RespondPermissionResponse {
                    already_resolved: false,
                })
            }
            // Idempotent resolution across all clients: the second and later calls are
            // treated as normal transitions
            Err(NodeError::AlreadyResolved(_)) => Ok(RespondPermissionResponse {
                already_resolved: true,
            }),
            Err(err) => Err(api_error(err)),
        }
    }

    async fn respond_elicitation(
        &self,
        session_id: &str,
        elicitation_id: &str,
        request: RespondElicitationRequest,
        client: ClientInfo,
    ) -> Result<RespondElicitationResponse, ApiError> {
        let command_id = CommandId::from(fxg_protocol::util::uuid_v7());
        match self
            .session_manager()
            .respond_elicitation(
                &command_id,
                session_id,
                elicitation_id,
                request.action,
                request.content.clone(),
                request.resolved_by.clone(),
            )
            .await
        {
            Ok(()) => {
                self.record_audit(
                    fxg_db::audit::actions::ELICITATION_RESOLVED,
                    &audit_source(&client),
                    Some(session_id),
                    serde_json::json!({
                        "elicitation_id": elicitation_id,
                        "action": request.action.as_str(),
                        "resolved_by": request.resolved_by,
                    }),
                )
                .await;
                Ok(RespondElicitationResponse {
                    already_resolved: false,
                })
            }
            // Idempotent resolution across all clients: the second and later calls are
            // treated as normal transitions
            Err(NodeError::AlreadyResolved(_)) => Ok(RespondElicitationResponse {
                already_resolved: true,
            }),
            Err(err) => Err(api_error(err)),
        }
    }

    async fn resume_session(
        &self,
        session_id: &str,
        request: ResumeSessionRequest,
        client: ClientInfo,
    ) -> Result<ResumeSessionResponse, ApiError> {
        let command_id = CommandId::from(fxg_protocol::util::uuid_v7());
        let outcome = self
            .session_manager()
            .resume(ResumeParams {
                command_id: &command_id,
                session_id,
                force_replay: request.force_replay,
            })
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_RESUME,
            &audit_source(&client),
            Some(session_id),
            serde_json::json!({
                "force_replay": request.force_replay,
                "context_restored": outcome.context_restored,
            }),
        )
        .await;
        Ok(ResumeSessionResponse {
            session_id: outcome.session_id.into(),
            context_restored: outcome.context_restored,
        })
    }

    async fn archive_session(
        &self,
        session_id: &str,
        request: SessionArchiveRequest,
        client: ClientInfo,
    ) -> Result<SessionArchiveResponse, ApiError> {
        let command_id = CommandId::from(fxg_protocol::util::uuid_v7());
        let archived_at = self
            .set_session_archived(
                &command_id,
                session_id,
                request.archived,
                &audit_source(&client),
            )
            .await
            .map_err(api_error)?;
        Ok(SessionArchiveResponse {
            session_id: session_id.into(),
            archived_at,
        })
    }

    async fn rename_session(
        &self,
        session_id: &str,
        request: SessionRenameRequest,
        client: ClientInfo,
    ) -> Result<SessionRenameResponse, ApiError> {
        let command_id = CommandId::from(fxg_protocol::util::uuid_v7());
        let title = self
            .session_manager()
            .rename(&command_id, session_id, &request.title)
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_RENAME,
            &audit_source(&client),
            Some(session_id),
            serde_json::json!({ "title": title }),
        )
        .await;
        Ok(SessionRenameResponse {
            session_id: session_id.into(),
            title,
        })
    }

    async fn delete_session(&self, session_id: &str, client: ClientInfo) -> Result<(), ApiError> {
        let command_id = CommandId::from(fxg_protocol::util::uuid_v7());
        self.purge_session(&command_id, session_id, &audit_source(&client))
            .await
            .map_err(api_error)?;
        Ok(())
    }

    async fn list_worktrees(&self, project_id: &str) -> Result<WorktreesResponse, ApiError> {
        let worktrees = self
            .worktree_list(None, Some(project_id))
            .await
            .map_err(api_error)?;
        Ok(WorktreesResponse { worktrees })
    }

    async fn create_worktree(
        &self,
        project_id: &str,
        request: CreateWorktreeRequest,
        client: ClientInfo,
    ) -> Result<WorktreeInfo, ApiError> {
        if request.node_id.as_str() != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {} but this node is {}",
                request.node_id,
                self.node_id()
            )));
        }
        let repo = self
            .project_main_repo(project_id)
            .await
            .map_err(api_error)?;
        let outcome = self
            .worktree_add(
                &repo,
                Some(project_id),
                &request.branch,
                request.base_branch,
                &audit_source(&client),
            )
            .await
            .map_err(api_error)?;
        let head_commit = crate::git::head_commit(&outcome.path).await.unwrap_or(None);
        Ok(WorktreeInfo {
            node_id: self.node_id().into(),
            path: outcome.path.to_string_lossy().into_owned(),
            branch: Some(outcome.branch),
            head_commit,
            is_main: false,
        })
    }

    async fn remove_worktree(
        &self,
        _project_id: &str,
        request: RemoveWorktreeRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError> {
        if request.node_id.as_str() != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {} but this node is {}",
                request.node_id,
                self.node_id()
            )));
        }
        self.worktree_remove(
            None,
            &PathBuf::from(&request.path),
            request.force,
            &audit_source(&client),
        )
        .await
        .map_err(api_error)
    }

    async fn prune_worktrees(
        &self,
        project_id: &str,
        request: PruneWorktreesRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError> {
        if request.node_id.as_str() != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {} but this node is {}",
                request.node_id,
                self.node_id()
            )));
        }
        let repo = self
            .project_main_repo(project_id)
            .await
            .map_err(api_error)?;
        self.worktree_prune(&repo, &audit_source(&client))
            .await
            .map_err(api_error)?;
        Ok(())
    }

    async fn scan_projects(
        &self,
        node_id: &str,
        request: ProjectScanRequest,
        client: ClientInfo,
    ) -> Result<ProjectScanResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let scanned_dirs = self
            .project_scan(request.dir.as_deref().map(Path::new))
            .await
            .map_err(api_error)?;
        let projects = self.db().list_projects().await.map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &audit_source(&client),
            None,
            serde_json::json!({
                "action": "scan",
                "scanned_dirs": scanned_dirs,
            }),
        )
        .await;
        Ok(ProjectScanResponse {
            scanned_dirs,
            projects,
        })
    }

    async fn link_project(
        &self,
        node_id: &str,
        request: ProjectLinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectLinkResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let dir = PathBuf::from(&request.local_path);
        if !dir.is_dir() {
            return Err(ApiError::not_found(format!(
                "directory not found: {}",
                request.local_path
            )));
        }
        let resolved = self
            .project_link(&dir, request.project_id.as_str())
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &audit_source(&client),
            None,
            serde_json::json!({
                "action": "link",
                "project_id": resolved.project_id,
                "local_path": resolved.local_path.to_string_lossy(),
            }),
        )
        .await;
        Ok(ProjectLinkResponse {
            project_id: resolved.project_id.into(),
            local_path: resolved.local_path.to_string_lossy().into_owned(),
        })
    }

    async fn unlink_project(
        &self,
        node_id: &str,
        request: ProjectUnlinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectUnlinkResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let local_path = request.local_path;
        let deleted = self
            .db()
            .delete_project_binding(self.node_id(), &local_path)
            .await
            .map_err(ApiError::from)?;
        if deleted == 0 {
            return Err(ApiError::not_found(format!(
                "no project binding for {local_path}"
            )));
        }
        // Drop the logical project as well when this was its last binding (sessions keep it).
        self.db()
            .delete_orphan_projects()
            .await
            .map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &audit_source(&client),
            None,
            serde_json::json!({
                "action": "unlink",
                "local_path": local_path,
            }),
        )
        .await;
        self.trigger_node_hello();
        Ok(ProjectUnlinkResponse { local_path })
    }

    async fn list_agents(&self) -> Result<AgentsResponse, ApiError> {
        self.agents_catalog().await.map_err(api_error)
    }

    async fn manage_agent(
        &self,
        node_id: &str,
        action: AgentAction,
        client: ClientInfo,
    ) -> Result<AgentOpResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        let message = DaemonState::manage_agent(self, &action)
            .await
            .map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::AGENT_MANAGE,
            &audit_source(&client),
            None,
            serde_json::json!({ "action": action, "message": &message }),
        )
        .await;
        // Report the change in install state to the hub (updates nodes.installed_agents)
        self.trigger_node_hello();
        Ok(AgentOpResponse {
            message: Some(message),
        })
    }

    async fn rotate_auth_token(
        &self,
        client: ClientInfo,
    ) -> Result<RotateAuthTokenResponse, ApiError> {
        let token = DaemonState::rotate_auth_token(self).map_err(api_error)?;
        // Never record the token itself in the audit log
        self.record_audit(
            fxg_db::audit::actions::AUTH_TOKEN_ROTATE,
            &audit_source(&client),
            None,
            serde_json::json!({ "scope": "node" }),
        )
        .await;
        Ok(RotateAuthTokenResponse { token })
    }

    async fn workspace_diff(
        &self,
        session_id: &str,
        scope: DiffScope,
        base_branch: Option<String>,
    ) -> Result<WorkspaceDiffResponse, ApiError> {
        let session = self
            .db()
            .get_session(session_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {session_id}")))?;
        crate::git::workspace_diff(
            Path::new(&session.local_path),
            scope,
            base_branch.as_deref(),
        )
        .await
        .map_err(api_error)
    }

    async fn provisioners(&self) -> Result<ProvisionersResponse, ApiError> {
        // Ephemeral VM provisioners only run on the hub (design: docs/01 §6.1)
        Ok(ProvisionersResponse {
            provisioners: Vec::new(),
        })
    }

    async fn list_agents_node(&self, node_id: &str) -> Result<AgentsResponse, ApiError> {
        if node_id != self.node_id() {
            return Err(ApiError::bad_request(format!(
                "requested node {node_id} but this node is {}",
                self.node_id()
            )));
        }
        self.agents_catalog().await.map_err(api_error)
    }

    async fn kill_switch(
        &self,
        reason: &str,
        client: ClientInfo,
    ) -> Result<KillSwitchResponse, ApiError> {
        let (killed_sessions, killed_ptys) =
            self.kill_all_local(reason).await.map_err(api_error)?;
        self.record_audit(
            fxg_db::audit::actions::KILL_SWITCH,
            &audit_source(&client),
            None,
            serde_json::json!({
                "reason": reason,
                "killed_sessions": killed_sessions,
                "killed_ptys": killed_ptys,
            }),
        )
        .await;
        Ok(KillSwitchResponse { notified_nodes: 1 })
    }

    async fn pty_spawn(&self, params: PtySpawnParams) -> Result<(), PtyChannelError> {
        let session = self
            .db()
            .get_session(&params.session_id)
            .await
            .map_err(PtyChannelError::internal)?
            .ok_or_else(|| {
                PtyChannelError::not_found(format!("session not found: {}", params.session_id))
            })?;
        let mut request = fxg_pty::PtySpawnRequest::new(params.pty_id.clone());
        request.session_id = Some(params.session_id.clone());
        request.cwd = Some(PathBuf::from(&session.local_path));
        request.shell_cmd = params.shell_cmd.clone();
        request.cols = params.cols;
        request.rows = params.rows;
        self.pty()
            .spawn(request)
            .map_err(PtyChannelError::internal)?;
        self.record_audit(
            fxg_db::audit::actions::PTY_SPAWN,
            &AuditSource::local(),
            Some(&params.session_id),
            serde_json::json!({ "pty_id": params.pty_id }),
        )
        .await;
        Ok(())
    }

    async fn pty_attach(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        if self.pty().contains(pty_id) {
            Ok(())
        } else {
            Err(PtyChannelError::not_found(format!(
                "pty session not found: {pty_id}"
            )))
        }
    }

    async fn pty_subscribe(
        &self,
        pty_id: &str,
    ) -> Result<broadcast::Receiver<PtyChannelEvent>, PtyChannelError> {
        let mut events = self.pty().subscribe(pty_id).map_err(map_pty_error)?;
        let (tx, rx) = broadcast::channel(PTY_CHANNEL_CAPACITY);
        fxg_protocol::spawn_guarded!("pty subscriber forwarder", async move {
            loop {
                match events.recv().await {
                    Ok(fxg_pty::PtyEvent::Output { data, .. }) => {
                        if tx.send(PtyChannelEvent::Output { data }).is_err() {
                            break;
                        }
                    }
                    Ok(fxg_pty::PtyEvent::Exit { exit_code, .. }) => {
                        fxg_protocol::discard_with!(
                            tx.send(PtyChannelEvent::Exit { exit_code }),
                            "broadcast a pty event"
                        );
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "pty subscriber lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(rx)
    }

    async fn pty_write(&self, pty_id: &str, data: &[u8]) -> Result<(), PtyChannelError> {
        self.pty()
            .write(pty_id, data)
            .map(|_| ())
            .map_err(map_pty_error)
    }

    async fn pty_resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyChannelError> {
        self.pty().resize(pty_id, cols, rows).map_err(map_pty_error)
    }

    async fn pty_kill(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        self.pty().kill(pty_id).map_err(map_pty_error)
    }

    async fn pty_disconnect(&self, pty_id: &str) {
        fxg_protocol::discard_with!(self.pty().kill(pty_id), "kill the pty terminal");
    }
}

impl DaemonState {
    /// Returns the `option_id` promoted to an `allow_always` option when `always = true`.
    ///
    /// Permission request options are taken from the node projection (`permission_requests`).
    async fn resolve_permission_option(
        &self,
        session_id: &str,
        request_id: &str,
        request: &RespondPermissionRequest,
    ) -> Result<String, ApiError> {
        if !request.always {
            return Ok(request.selected_option_id.clone());
        }
        let pending = self
            .db()
            .pending_permissions()
            .await
            .map_err(ApiError::from)?;
        if let Some(entry) = pending
            .iter()
            .find(|entry| entry.session_id.as_str() == session_id && entry.request_id == request_id)
            && let Some(option) = entry.options.iter().find(|option| option.kind.is_always())
        {
            return Ok(option.option_id.clone());
        }
        Ok(request.selected_option_id.clone())
    }
}

/// Converts a `fxg-pty` error into a PTY channel error.
fn map_pty_error(err: fxg_pty::PtyError) -> PtyChannelError {
    match err {
        fxg_pty::PtyError::NotFound(id) => {
            PtyChannelError::not_found(format!("pty session not found: {id}"))
        }
        other => PtyChannelError::internal(other),
    }
}
