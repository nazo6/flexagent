//! Session lifecycle operations and their node / project resolution helpers.

use fxg_protocol::CommandId;
use fxg_protocol::client_api::{
    CreateSessionRequest, CreateSessionResponse, RespondElicitationRequest,
    RespondElicitationResponse, RespondPermissionRequest, RespondPermissionResponse,
    ResumeSessionRequest, ResumeSessionResponse, SessionArchiveRequest, SessionArchiveResponse,
};
use fxg_protocol::common::{ErrorCode, WorktreeAction};
use fxg_protocol::node_server::{NodeRpcRequest, NodeRpcResponse, ServerToNodeMsg};
use fxg_protocol::util::uuid_v7;

use crate::api::{ApiError, ClientInfo};
use crate::provisioner::{self, SpawnSessionRequest};

use super::ServerState;

impl ServerState {
    /// Resolves the executing node for a session (`NODE_OFFLINE` is returned early here).
    pub(super) async fn session_node(
        &self,
        session_id: &fxg_protocol::SessionId,
    ) -> Result<fxg_protocol::NodeId, ApiError> {
        let session = self
            .inner
            .db
            .get_session(session_id.as_str())
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {session_id}")))?;
        if !self.inner.hub.is_online(&session.node_id).await {
            return Err(ApiError::node_offline(
                session.node_id.as_str(),
                format!("node is offline: {}", session.node_id),
            ));
        }
        Ok(session.node_id)
    }

    /// Picks one online node that can serve the ACP Registry catalog.
    ///
    /// Resolves the default working directory for a project on this node.
    ///
    /// Prefers the most recently used binding (including worktrees) and falls back to the
    /// latest session's working directory when there is no binding
    /// ([`fxg_db::queries::resolve_default_local_path`]).
    async fn project_local_path(
        &self,
        project_id: &fxg_protocol::ProjectId,
        node_id: &fxg_protocol::NodeId,
    ) -> Result<String, ApiError> {
        self.inner
            .db
            .resolve_default_local_path(project_id, node_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "no default directory for project {project_id} on node {node_id}; \
                     specify local_path or register the project (scan / link / worktree)"
                ))
            })
    }

    /// When `always = true`, returns the `option_id` promoted to an `allow_always` option.
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
            .inner
            .db
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

impl ServerState {
    /// Fetches the projection row for a logical project.
    async fn project_record(
        &self,
        project_id: &fxg_protocol::ProjectId,
    ) -> Result<fxg_protocol::common::ProjectSummary, ApiError> {
        let projects = self
            .inner
            .db
            .list_projects()
            .await
            .map_err(ApiError::from)?;
        projects
            .into_iter()
            .find(|project| &project.project_id == project_id)
            .ok_or_else(|| ApiError::not_found(format!("project not found: {project_id}")))
    }
}

impl ServerState {
    pub(super) async fn create_session_impl(
        &self,
        request: CreateSessionRequest,
        client: ClientInfo,
    ) -> Result<CreateSessionResponse, ApiError> {
        // ACP-native fork: it must run on the node that owns the source session.
        // Cross-node / cross-agent / ephemeral-VM forks are not supported.
        let fork_source = match &request.fork {
            Some(fork) => Some(
                self.inner
                    .db
                    .get_session(fork.from_session_id.as_str())
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| {
                        ApiError::not_found(format!("session not found: {}", fork.from_session_id))
                    })?,
            ),
            None => None,
        };

        // Temporary VM provisioner: launch a child process on the server host
        if let Some(provisioner_name) = request.provisioner.clone() {
            if fork_source.is_some() {
                return Err(ApiError::bad_request(
                    "native forks cannot be combined with a provisioner",
                ));
            }
            if request.node_id.is_some() {
                return Err(ApiError::bad_request(
                    "node_id and provisioner are mutually exclusive",
                ));
            }
            let project = self.project_record(&request.project_id).await?;
            let git_url = project.canonical_git_url.clone().ok_or_else(|| {
                ApiError::bad_request(format!(
                    "project {} has no canonical git url; cannot clone into an ephemeral node",
                    request.project_id
                ))
            })?;

            let session_id = uuid_v7();
            let node_id = format!("eph-{}", &session_id[..18.min(session_id.len())]);
            let git_branch = request
                .worktree
                .as_ref()
                .map(|spec| spec.branch.clone())
                .filter(|branch| !branch.trim().is_empty());
            let agent_id = request.agent_id.clone();

            // Pre-register the ephemeral node (FK for sessions.node_id / UI display)
            let idle_timeout_secs = self
                .provisioners()
                .config(&provisioner_name)?
                .idle_timeout_secs
                .unwrap_or(900);
            provisioner::register_ephemeral_node(
                self,
                &node_id,
                &provisioner_name,
                idle_timeout_secs,
            )
            .await?;

            // Provisional projection row so the UI can show the session during bootstrap
            if let Err(err) = self
                .db()
                .upsert_provisional_session(&fxg_db::registration::ProvisionalSessionRecord {
                    session_id: session_id.clone(),
                    project_id: request.project_id.to_string(),
                    project_name: project.name.clone(),
                    node_id: node_id.clone(),
                    local_path: provisioner::EPHEMERAL_WORKSPACE.to_owned(),
                    git_branch: git_branch.clone(),
                    agent_id: agent_id.clone(),
                    title: format!("{agent_id} @ {}", project.name),
                })
                .await
            {
                return Err(ApiError::from(err));
            }

            provisioner::spawn_session(
                self,
                SpawnSessionRequest {
                    session_id: session_id.clone(),
                    node_id: node_id.clone(),
                    provisioner: provisioner_name.clone(),
                    project_id: request.project_id.to_string(),
                    git_url,
                    git_branch,
                    agent_id: agent_id.clone(),
                    initial_prompt: request.initial_prompt.clone(),
                    initial_config: request.initial_config.clone(),
                    mode: request.mode.clone(),
                    extra_args: request.extra_args.clone(),
                },
            )
            .await?;

            self.record_audit(
                fxg_db::audit::actions::SESSION_START,
                &client,
                Some(&session_id),
                serde_json::json!({
                    "node_id": node_id,
                    "agent_id": agent_id,
                    "provisioner": provisioner_name,
                    "project_id": request.project_id,
                }),
            )
            .await;
            return Ok(CreateSessionResponse {
                session_id: session_id.into(),
                command_id: request.command_id,
            });
        }

        let node_id = match fork_source.as_ref() {
            Some(source) => {
                if let Some(requested) = &request.node_id
                    && requested != &source.node_id
                {
                    return Err(ApiError::bad_request(format!(
                        "a native fork must run on the source node ({})",
                        source.node_id
                    )));
                }
                let is_ephemeral = self
                    .inner
                    .db
                    .list_nodes()
                    .await
                    .map_err(ApiError::from)?
                    .iter()
                    .any(|node| node.node_id == source.node_id && node.is_ephemeral);
                if is_ephemeral {
                    return Err(ApiError::bad_request(
                        "native forks from ephemeral VM sessions are not supported",
                    ));
                }
                source.node_id.clone()
            }
            None => request
                .node_id
                .clone()
                .ok_or_else(|| ApiError::bad_request("node_id or provisioner is required"))?,
        };
        if !self.inner.hub.is_online(&node_id).await {
            return Err(ApiError::node_offline(
                node_id.as_str(),
                format!("node is offline: {node_id}"),
            ));
        }

        // When creating a worktree at the same time, create it on the node and use the real path
        let local_path = if let Some(spec) = &request.worktree {
            let info = match self
                .node_rpc(
                    &node_id,
                    NodeRpcRequest::ManageWorktree {
                        project_id: request.project_id.clone(),
                        action: WorktreeAction::Add {
                            branch: spec.branch.clone(),
                            base_branch: spec.base_branch.clone(),
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
            info.path
        } else if let Some(path) = request.local_path.clone() {
            path
        } else if let Some(source) = fork_source.as_ref() {
            source.local_path.clone()
        } else {
            self.project_local_path(&request.project_id, &node_id)
                .await?
        };

        let session_id = uuid_v7();
        let agent_id = fork_source.as_ref().map_or_else(
            || request.agent_id.clone(),
            |source| source.agent_id.clone(),
        );
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &request.command_id,
                ServerToNodeMsg::StartSession {
                    command_id: request.command_id.clone(),
                    session_id: session_id.clone().into(),
                    project_id: request.project_id.clone(),
                    local_path: local_path.clone(),
                    agent_id: agent_id.clone(),
                    initial_prompt: request.initial_prompt.clone(),
                    initial_config: request.initial_config.clone(),
                    mode: request.mode.clone(),
                    extra_args: request.extra_args.clone(),
                    fork_from_session_id: fork_source
                        .as_ref()
                        .map(|source| source.session_id.clone()),
                },
            )
            .await?;
        if !result.success {
            return Err(ApiError::from_command(result));
        }

        self.record_audit(
            fxg_db::audit::actions::SESSION_START,
            &client,
            Some(&session_id),
            serde_json::json!({
                "node_id": node_id,
                "agent_id": agent_id,
                "local_path": local_path,
                "project_id": request.project_id,
            }),
        )
        .await;

        Ok(CreateSessionResponse {
            session_id: session_id.into(),
            command_id: result.command_id,
        })
    }

    pub(super) async fn respond_permission_impl(
        &self,
        session_id: &str,
        request_id: &str,
        request: RespondPermissionRequest,
        client: ClientInfo,
    ) -> Result<RespondPermissionResponse, ApiError> {
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        let selected = self
            .resolve_permission_option(session_id, request_id, &request)
            .await?;
        let command_id = CommandId::from(uuid_v7());
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &command_id,
                ServerToNodeMsg::RespondPermission {
                    command_id: command_id.clone(),
                    session_id: session_id.into(),
                    request_id: request_id.to_owned(),
                    selected_option_id: selected.clone(),
                    resolved_by: request.resolved_by.clone(),
                },
            )
            .await?;

        if result.success {
            self.record_audit(
                fxg_db::audit::actions::PERMISSION_RESOLVED,
                &client,
                Some(session_id),
                serde_json::json!({
                    "request_id": request_id,
                    "selected_option_id": selected,
                    "resolved_by": request.resolved_by,
                    "node_id": node_id,
                }),
            )
            .await;
            Ok(RespondPermissionResponse {
                already_resolved: false,
            })
        } else if result
            .error
            .as_ref()
            .is_some_and(|error| error.code == ErrorCode::AlreadyResolved)
        {
            // Idempotent resolution across all clients (later calls are normal transitions)
            Ok(RespondPermissionResponse {
                already_resolved: true,
            })
        } else {
            Err(ApiError::from_command(result))
        }
    }

    pub(super) async fn respond_elicitation_impl(
        &self,
        session_id: &str,
        elicitation_id: &str,
        request: RespondElicitationRequest,
        client: ClientInfo,
    ) -> Result<RespondElicitationResponse, ApiError> {
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        let command_id = CommandId::from(uuid_v7());
        let result = self
            .inner
            .hub
            .command(
                &node_id,
                &command_id,
                ServerToNodeMsg::RespondElicitation {
                    command_id: command_id.clone(),
                    session_id: session_id.into(),
                    elicitation_id: elicitation_id.to_owned(),
                    action: request.action,
                    content: request.content.clone(),
                    resolved_by: request.resolved_by.clone(),
                },
            )
            .await?;

        if result.success {
            self.record_audit(
                fxg_db::audit::actions::ELICITATION_RESOLVED,
                &client,
                Some(session_id),
                serde_json::json!({
                    "elicitation_id": elicitation_id,
                    "action": request.action.as_str(),
                    "resolved_by": request.resolved_by,
                    "node_id": node_id,
                }),
            )
            .await;
            Ok(RespondElicitationResponse {
                already_resolved: false,
            })
        } else if result
            .error
            .as_ref()
            .is_some_and(|error| error.code == ErrorCode::AlreadyResolved)
        {
            // Idempotent resolution across all clients (later calls are normal transitions)
            Ok(RespondElicitationResponse {
                already_resolved: true,
            })
        } else {
            Err(ApiError::from_command(result))
        }
    }

    pub(super) async fn resume_session_impl(
        &self,
        session_id: &str,
        request: ResumeSessionRequest,
        client: ClientInfo,
    ) -> Result<ResumeSessionResponse, ApiError> {
        let session = self
            .inner
            .db
            .get_session(session_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {session_id}")))?;

        // Resuming a temporary VM (provisioner) session (VM restart + workspace
        // handover) is v2,
        // so it is rejected in v1 (resident nodes only).
        let is_ephemeral = self
            .inner
            .db
            .list_nodes()
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .any(|node| node.node_id == session.node_id && node.is_ephemeral);
        if is_ephemeral {
            return Err(ApiError::from_code(
                ErrorCode::InvalidState,
                "resume for ephemeral sessions is not supported yet",
            ));
        }

        // Relay to the executing node (offline yields `NODE_OFFLINE`)
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        let context_restored = match self
            .node_rpc(
                &node_id,
                NodeRpcRequest::ResumeSession {
                    session_id: session_id.into(),
                    force_replay: request.force_replay,
                },
            )
            .await?
        {
            NodeRpcResponse::Resume { context_restored } => context_restored,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::SESSION_RESUME,
            &client,
            Some(session_id),
            serde_json::json!({
                "node_id": node_id,
                "force_replay": request.force_replay,
                "context_restored": context_restored,
            }),
        )
        .await;
        Ok(ResumeSessionResponse {
            session_id: session_id.into(),
            context_restored,
        })
    }

    pub(super) async fn archive_session_impl(
        &self,
        session_id: &str,
        request: SessionArchiveRequest,
        client: ClientInfo,
    ) -> Result<SessionArchiveResponse, ApiError> {
        // Relay to the executing node (existence check / offline is returned by `session_node`)
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        let archived_at = match self
            .node_rpc(
                &node_id,
                NodeRpcRequest::ArchiveSession {
                    session_id: session_id.into(),
                    archived: request.archived,
                },
            )
            .await?
        {
            NodeRpcResponse::Archive { archived_at } => archived_at,
            _ => return Err(ApiError::internal("unexpected node RPC response")),
        };
        self.record_audit(
            fxg_db::audit::actions::SESSION_ARCHIVE,
            &client,
            Some(session_id),
            serde_json::json!({
                "node_id": node_id,
                "archived": request.archived,
                "archived_at": archived_at,
            }),
        )
        .await;
        Ok(SessionArchiveResponse {
            session_id: session_id.into(),
            archived_at,
        })
    }

    pub(super) async fn delete_session_impl(
        &self,
        session_id: &str,
        client: ClientInfo,
    ) -> Result<(), ApiError> {
        // Relay to the executing node (existence check / offline is returned by `session_node`)
        let node_id = self
            .session_node(&fxg_protocol::SessionId::from(session_id))
            .await?;
        self.node_rpc(
            &node_id,
            NodeRpcRequest::DeleteSession {
                session_id: session_id.into(),
            },
        )
        .await?;
        self.record_audit(
            fxg_db::audit::actions::SESSION_DELETE,
            &client,
            Some(session_id),
            serde_json::json!({ "node_id": node_id }),
        )
        .await;
        Ok(())
    }
}
