//! `ClientApiBackend` implementation for the central server.
//!
//! Session lifecycle methods live in `sessions.rs`; server settings and secrets live in
//! `settings.rs`.

use async_trait::async_trait;
use fxg_db::{Db, PushSubscriptionRecord};
use fxg_protocol::CommandId;
use fxg_protocol::client_api::{
    AgentOpResponse, AgentsResponse, ConnectionRole, CreateSessionRequest, CreateSessionResponse,
    CreateWorktreeRequest, DiagnosticsResponse, IssueNodeTokenRequest, IssueNodeTokenResponse,
    KillSwitchResponse, NodeTokenSummary, NodeTokensResponse, ProjectLinkRequest,
    ProjectLinkResponse, ProjectScanRequest, ProjectScanResponse, ProvisionerSummary,
    ProvisionersResponse, PruneWorktreesRequest, PushSubscribeRequest, PushSubscribeResponse,
    RemoveWorktreeRequest, RespondElicitationRequest, RespondElicitationResponse,
    RespondPermissionRequest, RespondPermissionResponse, ResumeSessionRequest,
    ResumeSessionResponse, RotateAuthTokenResponse, SessionArchiveRequest, SessionArchiveResponse,
    WorktreeInfo, WorktreesResponse,
};
use fxg_protocol::common::{
    AgentAction, CommandResult, DiffScope, ErrorCode, WorkspaceDiffResponse, WorktreeAction,
};
use fxg_protocol::node_server::{NodeRpcRequest, NodeRpcResponse, ServerToNodeMsg};
use fxg_protocol::util::uuid_v7;
use tokio::sync::broadcast;

use crate::api::{
    ApiError, ClientApiBackend, ClientCommand, ClientEvent, ClientInfo, PtyChannelError,
    PtyChannelEvent, PtySpawnParams, SystemExtras, auth,
};
use fxg_protocol::settings::{
    ProjectProfile, SecretsResponse, ServerSettingsSnapshot, UpdateProjectProfileRequest,
    UpdateServerSettingsRequest, UpsertSecretRequest,
};
use fxg_protocol::{NodeId, SecretId};

use super::ServerState;
use super::settings::secret_error;

#[async_trait]
impl ClientApiBackend for ServerState {
    fn db(&self) -> &Db {
        &self.inner.db
    }

    fn auth_token(&self) -> String {
        ServerState::auth_token(self)
    }

    fn connection_role(&self) -> ConnectionRole {
        ConnectionRole::CentralServer
    }

    async fn diagnostics(&self) -> Result<DiagnosticsResponse, ApiError> {
        Ok(DiagnosticsResponse {
            node_id: "central-server".to_owned(),
            generated_at: fxg_protocol::util::now_ms(),
            agents: Vec::new(),
            daemon_log: None,
        })
    }

    async fn system_extras(&self) -> Result<SystemExtras, ApiError> {
        Ok(SystemExtras {
            vapid_public_key: Some(self.inner.push.public_key().to_owned()),
            ..SystemExtras::default()
        })
    }

    fn subscribe_client_events(&self) -> broadcast::Receiver<ClientEvent> {
        self.inner.client_events.subscribe()
    }

    async fn dispatch_command(&self, command: ClientCommand) -> CommandResult {
        let command_id = command.command_id().to_owned();
        let (node_id, message) = match &command {
            ClientCommand::SendPrompt {
                session_id,
                input,
                client_source,
                ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::SendPrompt {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    input: input.clone(),
                    client_source: client_source.clone(),
                },
            ),
            ClientCommand::ControlSession {
                session_id, action, ..
            } => (
                self.session_node(session_id).await,
                ServerToNodeMsg::ControlSession {
                    command_id: command_id.clone(),
                    session_id: session_id.clone(),
                    action: action.clone(),
                },
            ),
        };

        match node_id {
            Ok(node_id) => self.run_ws_command(&node_id, &command_id, message).await,
            // Return immediately without queueing when the node is offline or the session unknown
            Err(err) => command_failure(command_id, err),
        }
    }

    async fn prune_worktrees(
        &self,
        project_id: &str,
        request: PruneWorktreesRequest,
        client: ClientInfo,
    ) -> Result<(), ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::node_offline(
                request.node_id.as_str(),
                format!("node is offline: {}", request.node_id),
            ));
        }
        self.node_rpc(
            &request.node_id,
            NodeRpcRequest::ManageWorktree {
                project_id: project_id.into(),
                action: WorktreeAction::Prune,
            },
        )
        .await?;
        self.record_audit(
            fxg_db::audit::actions::WORKTREE_PRUNE,
            &client,
            None,
            serde_json::json!({
                "node_id": request.node_id,
                "project_id": project_id,
            }),
        )
        .await;
        Ok(())
    }

    async fn scan_projects(
        &self,
        node_id: &str,
        request: ProjectScanRequest,
        client: ClientInfo,
    ) -> Result<ProjectScanResponse, ApiError> {
        if !self
            .inner
            .hub
            .is_online(&fxg_protocol::NodeId::from(node_id))
            .await
        {
            return Err(ApiError::node_offline(
                node_id,
                format!("node is offline: {node_id}"),
            ));
        }
        let response = match self
            .node_rpc(
                &fxg_protocol::NodeId::from(node_id),
                NodeRpcRequest::ProjectScan {
                    dir: request.dir.clone(),
                },
            )
            .await?
        {
            NodeRpcResponse::ProjectScan { response } => response,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &client,
            None,
            serde_json::json!({
                "action": "scan",
                "node_id": node_id,
                "dir": request.dir,
                "scanned_dirs": response.scanned_dirs,
            }),
        )
        .await;
        Ok(response)
    }

    async fn link_project(
        &self,
        node_id: &str,
        request: ProjectLinkRequest,
        client: ClientInfo,
    ) -> Result<ProjectLinkResponse, ApiError> {
        if !self
            .inner
            .hub
            .is_online(&fxg_protocol::NodeId::from(node_id))
            .await
        {
            return Err(ApiError::node_offline(
                node_id,
                format!("node is offline: {node_id}"),
            ));
        }
        let response = match self
            .node_rpc(
                &fxg_protocol::NodeId::from(node_id),
                NodeRpcRequest::ProjectLink {
                    project_id: request.project_id.clone(),
                    local_path: request.local_path.clone(),
                },
            )
            .await?
        {
            NodeRpcResponse::ProjectLink { response } => response,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::PROJECT_LINK,
            &client,
            None,
            serde_json::json!({
                "action": "link",
                "node_id": node_id,
                "project_id": response.project_id,
                "local_path": response.local_path,
            }),
        )
        .await;
        Ok(response)
    }

    async fn list_agents(&self) -> Result<AgentsResponse, ApiError> {
        // The central server does not pick a representative node anymore.
        Err(ApiError::from_code(
            ErrorCode::InvalidState,
            "specify a node: GET /api/v1/nodes/{node_id}/agents",
        ))
    }

    async fn list_agents_node(&self, node_id: &str) -> Result<AgentsResponse, ApiError> {
        let mut response = match self
            .node_rpc(
                &fxg_protocol::NodeId::from(node_id),
                NodeRpcRequest::ListAgents,
            )
            .await?
        {
            NodeRpcResponse::Agents { response } => response,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };

        // Merge availability across all nodes (installed_agents from NodeHello)
        let nodes = self.inner.db.list_nodes().await.map_err(ApiError::from)?;
        for agent in &mut response.agents {
            agent.installed_nodes = nodes
                .iter()
                .filter(|node| node.installed_agents.iter().any(|id| id == &agent.id))
                .map(|node| node.node_id.as_str().to_owned())
                .collect();
        }
        Ok(response)
    }

    async fn manage_agent(
        &self,
        node_id: &str,
        action: AgentAction,
        client: ClientInfo,
    ) -> Result<AgentOpResponse, ApiError> {
        if !self
            .inner
            .hub
            .is_online(&fxg_protocol::NodeId::from(node_id))
            .await
        {
            return Err(ApiError::node_offline(
                node_id,
                format!("node is offline: {node_id}"),
            ));
        }
        let message = match self
            .node_rpc(
                &fxg_protocol::NodeId::from(node_id),
                NodeRpcRequest::ManageAgent {
                    action: action.clone(),
                },
            )
            .await?
        {
            NodeRpcResponse::AgentOp { message } => message,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::AGENT_MANAGE,
            &client,
            None,
            serde_json::json!({
                "node_id": node_id,
                "action": action,
                "message": &message,
            }),
        )
        .await;
        Ok(AgentOpResponse { message })
    }

    async fn rotate_auth_token(
        &self,
        client: ClientInfo,
    ) -> Result<RotateAuthTokenResponse, ApiError> {
        let token = ServerState::rotate_auth_token(self).map_err(ApiError::internal)?;
        self.record_audit(
            fxg_db::audit::actions::AUTH_TOKEN_ROTATE,
            &client,
            None,
            serde_json::json!({ "scope": "server" }),
        )
        .await;
        Ok(RotateAuthTokenResponse { token })
    }

    async fn node_tokens(&self) -> Result<NodeTokensResponse, ApiError> {
        let tokens = self
            .inner
            .db
            .list_node_tokens()
            .await
            .map_err(ApiError::from)?;
        Ok(NodeTokensResponse {
            tokens: tokens
                .into_iter()
                .map(|(node_id, hash)| NodeTokenSummary {
                    node_id: node_id.into(),
                    token_prefix: hash.chars().take(12).collect(),
                })
                .collect(),
        })
    }

    async fn issue_node_token(
        &self,
        request: IssueNodeTokenRequest,
        client: ClientInfo,
    ) -> Result<IssueNodeTokenResponse, ApiError> {
        let node_id = request.node_id.as_str().trim().to_owned();
        if node_id.is_empty() {
            return Err(ApiError::bad_request("node_id must not be empty"));
        }
        // Register an unregistered node as a placeholder
        // (overwritten with real info by `NodeHello` when the node connects)
        let nodes = self.inner.db.list_nodes().await.map_err(ApiError::from)?;
        if !nodes.iter().any(|node| node.node_id.as_str() == node_id) {
            self.inner
                .db
                .upsert_node(&fxg_db::NodeRecord::new(
                    node_id.clone(),
                    node_id.clone(),
                    "unknown",
                    "unknown",
                    "unknown",
                ))
                .await
                .map_err(ApiError::from)?;
        }
        let token = auth::generate_token();
        self.inner
            .db
            .set_node_token_hash(&node_id, Some(&auth::hash_token(&token)))
            .await
            .map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::NODE_TOKEN_MANAGE,
            &client,
            None,
            serde_json::json!({ "action": "issue", "node_id": &node_id }),
        )
        .await;
        Ok(IssueNodeTokenResponse {
            node_id: node_id.into(),
            token,
        })
    }

    async fn revoke_node_token(&self, node_id: &str, client: ClientInfo) -> Result<(), ApiError> {
        self.inner
            .db
            .set_node_token_hash(node_id, None)
            .await
            .map_err(ApiError::from)?;
        self.record_audit(
            fxg_db::audit::actions::NODE_TOKEN_MANAGE,
            &client,
            None,
            serde_json::json!({ "action": "revoke", "node_id": node_id }),
        )
        .await;
        Ok(())
    }

    async fn create_session(
        &self,
        request: CreateSessionRequest,
        client: ClientInfo,
    ) -> Result<CreateSessionResponse, ApiError> {
        self.create_session_impl(request, client).await
    }

    async fn respond_permission(
        &self,
        session_id: &str,
        request_id: &str,
        request: RespondPermissionRequest,
        client: ClientInfo,
    ) -> Result<RespondPermissionResponse, ApiError> {
        self.respond_permission_impl(session_id, request_id, request, client)
            .await
    }

    async fn respond_elicitation(
        &self,
        session_id: &str,
        elicitation_id: &str,
        request: RespondElicitationRequest,
        client: ClientInfo,
    ) -> Result<RespondElicitationResponse, ApiError> {
        self.respond_elicitation_impl(session_id, elicitation_id, request, client)
            .await
    }

    async fn resume_session(
        &self,
        session_id: &str,
        request: ResumeSessionRequest,
        client: ClientInfo,
    ) -> Result<ResumeSessionResponse, ApiError> {
        self.resume_session_impl(session_id, request, client).await
    }

    async fn archive_session(
        &self,
        session_id: &str,
        request: SessionArchiveRequest,
        client: ClientInfo,
    ) -> Result<SessionArchiveResponse, ApiError> {
        self.archive_session_impl(session_id, request, client).await
    }

    async fn delete_session(&self, session_id: &str, client: ClientInfo) -> Result<(), ApiError> {
        self.delete_session_impl(session_id, client).await
    }

    async fn list_worktrees(&self, project_id: &str) -> Result<WorktreesResponse, ApiError> {
        let projects = self
            .inner
            .db
            .list_projects()
            .await
            .map_err(ApiError::from)?;
        let project = projects
            .iter()
            .find(|project| project.project_id.as_str() == project_id)
            .ok_or_else(|| ApiError::not_found(format!("project not found: {project_id}")))?;
        let worktrees = project
            .bindings
            .iter()
            .map(|binding| WorktreeInfo {
                node_id: binding.node_id.clone(),
                path: binding.local_path.clone(),
                branch: binding.git_branch.clone(),
                // HEAD commits are not included in the node's real-time reports (NodeHello)
                head_commit: None,
                is_main: !binding.is_worktree,
            })
            .collect();
        Ok(WorktreesResponse { worktrees })
    }

    async fn create_worktree(
        &self,
        project_id: &str,
        request: CreateWorktreeRequest,
        _client: ClientInfo,
    ) -> Result<WorktreeInfo, ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::node_offline(
                request.node_id.as_str(),
                format!("node is offline: {}", request.node_id),
            ));
        }
        let info = match self
            .node_rpc(
                &request.node_id,
                NodeRpcRequest::ManageWorktree {
                    project_id: project_id.into(),
                    action: WorktreeAction::Add {
                        branch: request.branch,
                        base_branch: request.base_branch,
                    },
                },
            )
            .await?
        {
            NodeRpcResponse::Worktree {
                worktree: Some(info),
            } => info,
            _ => return Err(ApiError::internal("node returned no worktree info")),
        };
        Ok(info)
    }

    async fn remove_worktree(
        &self,
        project_id: &str,
        request: RemoveWorktreeRequest,
        _client: ClientInfo,
    ) -> Result<(), ApiError> {
        if !self.inner.hub.is_online(&request.node_id).await {
            return Err(ApiError::node_offline(
                request.node_id.as_str(),
                format!("node is offline: {}", request.node_id),
            ));
        }
        self.node_rpc(
            &request.node_id,
            NodeRpcRequest::ManageWorktree {
                project_id: project_id.into(),
                action: WorktreeAction::Remove {
                    path: request.path,
                    force: request.force,
                },
            },
        )
        .await?;
        Ok(())
    }

    async fn workspace_diff(
        &self,
        session_id: &str,
        scope: DiffScope,
        base_branch: Option<String>,
    ) -> Result<WorkspaceDiffResponse, ApiError> {
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        match self
            .node_rpc(
                &node_id,
                NodeRpcRequest::GetGitDiff {
                    session_id: session_id.into(),
                    scope,
                    base_branch,
                },
            )
            .await?
        {
            NodeRpcResponse::GitDiff { diff } => Ok(diff),
            _ => Err(ApiError::internal("unexpected node RPC response")),
        }
    }

    async fn provisioners(&self) -> Result<ProvisionersResponse, ApiError> {
        let provisioners = self
            .provisioners()
            .configs()
            .into_iter()
            .map(|(name, config)| ProvisionerSummary {
                name,
                description: config.description,
                command: config.command,
                args: config.args,
                idle_timeout_secs: config.idle_timeout_secs,
                env: config.env,
            })
            .collect();
        Ok(ProvisionersResponse { provisioners })
    }

    async fn provisioner_test(
        &self,
        name: &str,
    ) -> Result<fxg_protocol::client_api::ProvisionerTestResponse, ApiError> {
        crate::provisioner::provisioner_test(self, name).await
    }

    async fn settings_server(&self) -> Result<ServerSettingsSnapshot, ApiError> {
        self.server_settings_snapshot()
            .await
            .map_err(ApiError::internal)
    }

    async fn update_server_settings(
        &self,
        request: UpdateServerSettingsRequest,
    ) -> Result<ServerSettingsSnapshot, ApiError> {
        self.update_server_settings(&request)
            .await
            .map_err(ApiError::internal)
    }

    async fn execute_node_rpc(
        &self,
        node_id: &NodeId,
        request: NodeRpcRequest,
        _client: Option<ClientInfo>,
    ) -> Result<NodeRpcResponse, ApiError> {
        let request_id = CommandId::from(uuid_v7());
        self.inner.hub.rpc(node_id, &request_id, request).await
    }

    async fn server_secrets(&self) -> Result<SecretsResponse, ApiError> {
        ServerState::server_secrets(self)
            .await
            .map_err(ApiError::internal)
    }

    async fn upsert_server_secret(
        &self,
        secret_id: &SecretId,
        request: UpsertSecretRequest,
    ) -> Result<SecretsResponse, ApiError> {
        ServerState::upsert_server_secret(self, secret_id, &request)
            .await
            .map_err(secret_error)
    }

    async fn delete_server_secret(
        &self,
        secret_id: &SecretId,
    ) -> Result<SecretsResponse, ApiError> {
        ServerState::delete_server_secret(self, secret_id)
            .await
            .map_err(secret_error)
    }

    async fn project_profile(&self, project_id: &str) -> Result<ProjectProfile, ApiError> {
        ServerState::project_profile(self, project_id).await
    }

    async fn update_project_profile(
        &self,
        project_id: &str,
        request: UpdateProjectProfileRequest,
    ) -> Result<ProjectProfile, ApiError> {
        ServerState::update_project_profile(self, project_id, &request).await
    }

    async fn kill_switch(
        &self,
        reason: &str,
        client: ClientInfo,
    ) -> Result<KillSwitchResponse, ApiError> {
        let notified = self
            .inner
            .hub
            .broadcast(ServerToNodeMsg::KillAllSessions {
                reason: reason.to_owned(),
            })
            .await;
        self.record_audit(
            fxg_db::audit::actions::KILL_SWITCH,
            &client,
            None,
            serde_json::json!({
                "reason": reason,
                "notified_nodes": notified,
            }),
        )
        .await;
        Ok(KillSwitchResponse {
            notified_nodes: notified.len() as u64,
        })
    }

    async fn push_subscribe(
        &self,
        request: PushSubscribeRequest,
        client: ClientInfo,
    ) -> Result<PushSubscribeResponse, ApiError> {
        if request.endpoint.trim().is_empty() {
            return Err(ApiError::bad_request("endpoint is required"));
        }
        let record = PushSubscriptionRecord {
            endpoint: request.endpoint.clone(),
            p256dh: request.p256dh.clone(),
            auth: request.auth.clone(),
            device_name: request.device_name.clone(),
        };
        fxg_db::push::upsert_push_subscription(self.inner.db.pool(), &record)
            .await
            .map_err(ApiError::from)?;
        tracing::info!(
            endpoint = %record.endpoint,
            device = record.device_name.as_deref().unwrap_or("unknown"),
            client_ip = %client.ip,
            "registered web push subscription"
        );
        Ok(PushSubscribeResponse { ok: true })
    }

    async fn pty_spawn(&self, params: PtySpawnParams) -> Result<(), PtyChannelError> {
        let session = self
            .inner
            .db
            .get_session(&params.session_id)
            .await
            .map_err(PtyChannelError::internal)?
            .ok_or_else(|| {
                PtyChannelError::not_found(format!("session not found: {}", params.session_id))
            })?;
        if !self.inner.hub.is_online(&session.node_id).await {
            return Err(PtyChannelError::offline(format!(
                "node is offline: {}",
                session.node_id
            )));
        }
        self.inner
            .hub
            .insert_pty(&params.pty_id, &session.node_id)
            .await;
        let result = self
            .inner
            .hub
            .send(
                &session.node_id,
                ServerToNodeMsg::PtySpawn {
                    pty_id: params.pty_id.clone(),
                    session_id: params.session_id.clone().into(),
                    cols: params.cols,
                    rows: params.rows,
                    shell_cmd: params.shell_cmd.clone(),
                },
            )
            .await;
        if let Err(err) = result {
            self.inner.hub.remove_pty(&params.pty_id).await;
            return Err(PtyChannelError::new(err.code.as_str(), err.message));
        }
        Ok(())
    }

    async fn pty_attach(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        if self.inner.hub.pty_subscribe(pty_id).await.is_some() {
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
        self.inner
            .hub
            .pty_subscribe(pty_id)
            .await
            .ok_or_else(|| PtyChannelError::not_found(format!("pty session not found: {pty_id}")))
    }

    async fn pty_write(&self, pty_id: &str, data: &[u8]) -> Result<(), PtyChannelError> {
        use base64::Engine as _;
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyInput {
                    pty_id: pty_id.to_owned(),
                    data_b64,
                },
            )
            .await
    }

    async fn pty_resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyChannelError> {
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyResize {
                    pty_id: pty_id.to_owned(),
                    cols,
                    rows,
                },
            )
            .await
    }

    async fn pty_kill(&self, pty_id: &str) -> Result<(), PtyChannelError> {
        self.inner
            .hub
            .pty_send(
                pty_id,
                ServerToNodeMsg::PtyKill {
                    pty_id: pty_id.to_owned(),
                },
            )
            .await
    }

    async fn pty_disconnect(&self, pty_id: &str) {
        fxg_protocol::discard_with!(
            self.inner
                .hub
                .pty_send(
                    pty_id,
                    ServerToNodeMsg::PtyKill {
                        pty_id: pty_id.to_owned(),
                    },
                )
                .await,
            "notify the node that a pty websocket disconnected"
        );
        self.inner.hub.remove_pty(pty_id).await;
    }
}

/// Helper that builds a `CommandResult`, with `session_id` on success.
fn command_failure(command_id: CommandId, err: ApiError) -> CommandResult {
    CommandResult::failure(command_id, err.into_info())
}

impl ServerState {
    /// Relays a WS command to a node and returns the `CommandResult`.
    async fn run_ws_command(
        &self,
        node_id: &fxg_protocol::NodeId,
        command_id: &CommandId,
        message: ServerToNodeMsg,
    ) -> CommandResult {
        match self.inner.hub.command(node_id, command_id, message).await {
            Ok(result) => result,
            Err(err) => command_failure(command_id.clone(), err),
        }
    }
}
