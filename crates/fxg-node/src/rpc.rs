//! Generic node RPC dispatch.
//!
//! Runs a [`NodeRpcRequest`] requested by the central server and returns a typed
//! [`NodeRpcOutcome`]. Transport concerns (WS / stdio framing, correlation and
//! resend) stay in [`crate::sync`].

use std::path::{Path, PathBuf};

use fxg_protocol::CommandId;
use fxg_protocol::client_api::{ProjectLinkResponse, ProjectScanResponse, WorktreeInfo};
use fxg_protocol::common::WorktreeAction;
use fxg_protocol::node_server::{NodeRpcOutcome, NodeRpcRequest, NodeRpcResponse};

use crate::daemon::{AuditSource, DaemonState};
use crate::error::NodeError;
use crate::session_manager::ResumeParams;

/// Executes a node RPC and converts failures into structured [`ErrorInfo`].
pub(crate) async fn execute(
    state: &DaemonState,
    request_id: &CommandId,
    request: NodeRpcRequest,
    source: &AuditSource,
) -> NodeRpcOutcome {
    match execute_inner(state, request_id, request, source).await {
        Ok(response) => NodeRpcOutcome::Ok { response },
        Err(error) => {
            if error.is_incident() {
                let info = error.info();
                let record = fxg_db::IncidentRecord::new(
                    error.incident_level(),
                    info.code,
                    error.incident_source(),
                    info.message.clone(),
                )
                .with_node_id(Some(state.node_id()))
                .with_request_id(Some(request_id.as_str()))
                .with_details(serde_json::json!({
                    "retryable": info.retryable,
                    "params": info.params,
                }));
                state.db().record_incident_best_effort(&record).await;
            }
            NodeRpcOutcome::Err {
                error: error.info(),
            }
        }
    }
}

async fn execute_inner(
    state: &DaemonState,
    request_id: &CommandId,
    request: NodeRpcRequest,
    source: &AuditSource,
) -> Result<NodeRpcResponse, NodeError> {
    match request {
        NodeRpcRequest::GetGitDiff {
            session_id,
            scope,
            base_branch,
        } => {
            let session = state
                .db()
                .get_session(session_id.as_str())
                .await?
                .ok_or_else(|| NodeError::InvalidSession(session_id.to_string()))?;
            let diff = crate::git::workspace_diff(
                Path::new(&session.local_path),
                scope,
                base_branch.as_deref(),
            )
            .await?;
            Ok(NodeRpcResponse::GitDiff { diff })
        }
        NodeRpcRequest::ManageWorktree { project_id, action } => {
            let worktree = match action {
                WorktreeAction::Add {
                    branch,
                    base_branch,
                } => {
                    let repo = state.project_main_repo(project_id.as_str()).await?;
                    let outcome = state
                        .worktree_add(
                            &repo,
                            Some(project_id.as_str()),
                            &branch,
                            base_branch,
                            source,
                        )
                        .await?;
                    let head_commit = crate::git::head_commit(&outcome.path).await.unwrap_or(None);
                    Some(WorktreeInfo {
                        node_id: state.node_id().into(),
                        path: outcome.path.to_string_lossy().into_owned(),
                        branch: Some(outcome.branch),
                        head_commit,
                        is_main: false,
                    })
                }
                WorktreeAction::Remove { path, force } => {
                    state
                        .worktree_remove(None, Path::new(&path), force, source)
                        .await?;
                    None
                }
                WorktreeAction::Prune => {
                    let repo = state.project_main_repo(project_id.as_str()).await?;
                    state.worktree_prune(&repo, source).await?;
                    None
                }
            };
            // Report the worktree change to the hub (project_node_bindings update).
            state.trigger_node_hello();
            Ok(NodeRpcResponse::Worktree { worktree })
        }
        NodeRpcRequest::BrowseFs { path } => {
            let response = crate::fs_browse::browse_fs(path.as_deref())?;
            Ok(NodeRpcResponse::BrowseFs { response })
        }
        NodeRpcRequest::ProjectScan { dir } => {
            let scanned_dirs = state.project_scan(dir.as_deref().map(Path::new)).await?;
            let projects = state.list_projects_with_path_state().await?;
            let response = ProjectScanResponse {
                scanned_dirs,
                projects,
            };
            state
                .record_audit(
                    fxg_db::audit::actions::PROJECT_LINK,
                    source,
                    None,
                    serde_json::json!({
                        "action": "scan",
                        "scanned_dirs": &response.scanned_dirs,
                        "projects": response.projects.len(),
                    }),
                )
                .await;
            Ok(NodeRpcResponse::ProjectScan { response })
        }
        NodeRpcRequest::ProjectLink {
            project_id,
            local_path,
        } => {
            let dir = PathBuf::from(&local_path);
            if !dir.is_dir() {
                return Err(NodeError::InvalidState(format!(
                    "directory not found: {local_path}"
                )));
            }
            let resolved = state.project_link(&dir, project_id.as_str()).await?;
            state
                .record_audit(
                    fxg_db::audit::actions::PROJECT_LINK,
                    source,
                    None,
                    serde_json::json!({
                        "action": "link",
                        "project_id": &resolved.project_id,
                        "local_path": resolved.local_path.to_string_lossy(),
                    }),
                )
                .await;
            let response = ProjectLinkResponse {
                project_id: resolved.project_id.into(),
                local_path: resolved.local_path.to_string_lossy().into_owned(),
            };
            Ok(NodeRpcResponse::ProjectLink { response })
        }
        NodeRpcRequest::InspectProject { local_path } => {
            let dir = PathBuf::from(&local_path);
            if !dir.is_dir() {
                return Err(NodeError::InvalidState(format!(
                    "directory not found: {local_path}"
                )));
            }
            let resolved =
                crate::project::resolve_project(&dir, state.node_id(), state.db()).await?;
            Ok(NodeRpcResponse::ProjectPreview {
                preview: project_preview(&resolved).await,
            })
        }
        NodeRpcRequest::AddProject { local_path } => {
            let dir = PathBuf::from(&local_path);
            if !dir.is_dir() {
                return Err(NodeError::InvalidState(format!(
                    "directory not found: {local_path}"
                )));
            }
            let resolved = state.resolve_and_register_project(&dir).await?;
            state
                .record_audit(
                    fxg_db::audit::actions::PROJECT_LINK,
                    source,
                    None,
                    serde_json::json!({
                        "action": "add",
                        "project_id": &resolved.project_id,
                        "local_path": resolved.local_path.to_string_lossy(),
                    }),
                )
                .await;
            Ok(NodeRpcResponse::ProjectPreview {
                preview: project_preview(&resolved).await,
            })
        }
        NodeRpcRequest::ListAgents => {
            let response = state.agents_catalog().await?;
            Ok(NodeRpcResponse::Agents { response })
        }
        NodeRpcRequest::ProbeAgentConfig {
            agent_id,
            local_path,
            force,
            initial_config,
        } => {
            let preview = state
                .session_manager()
                .probe_agent_capabilities(&agent_id, Path::new(&local_path), &initial_config, force)
                .await?;
            Ok(NodeRpcResponse::AgentCapabilities { preview })
        }
        NodeRpcRequest::GetNodeSettings => Ok(NodeRpcResponse::NodeSettings {
            snapshot: Box::new(state.node_settings_snapshot()),
        }),
        NodeRpcRequest::UpdateNodeSettings { request } => {
            state.update_node_settings(&request).await?;
            state
                .record_audit(
                    fxg_db::audit::actions::SETTINGS_UPDATE,
                    source,
                    None,
                    serde_json::json!({ "section": "node" }),
                )
                .await;
            Ok(NodeRpcResponse::NodeSettings {
                snapshot: Box::new(state.node_settings_snapshot()),
            })
        }
        NodeRpcRequest::UpdateAgentSettings { request } => {
            state.update_agent_settings(&request).await?;
            state
                .record_audit(
                    fxg_db::audit::actions::SETTINGS_UPDATE,
                    source,
                    None,
                    serde_json::json!({ "section": "agents" }),
                )
                .await;
            Ok(NodeRpcResponse::NodeSettings {
                snapshot: Box::new(state.node_settings_snapshot()),
            })
        }
        NodeRpcRequest::GetSecrets => Ok(NodeRpcResponse::Secrets {
            secrets: state.node_secrets().await?,
        }),
        NodeRpcRequest::UpsertSecret { id, request } => {
            let secrets = state.upsert_node_secret(&id, &request).await?;
            state
                .record_audit(
                    fxg_db::audit::actions::SECRET_MANAGE,
                    source,
                    None,
                    serde_json::json!({
                        "action": "upsert",
                        "secret_id": id.as_str(),
                        "has_description": request.description.is_some(),
                    }),
                )
                .await;
            Ok(NodeRpcResponse::Secrets { secrets })
        }
        NodeRpcRequest::DeleteSecret { id } => {
            let secrets = state.delete_node_secret(&id).await?;
            state
                .record_audit(
                    fxg_db::audit::actions::SECRET_MANAGE,
                    source,
                    None,
                    serde_json::json!({ "action": "delete", "secret_id": id.as_str() }),
                )
                .await;
            Ok(NodeRpcResponse::Secrets { secrets })
        }
        NodeRpcRequest::ManageAgent { action } => {
            let message = state.manage_agent(&action).await?;
            state
                .record_audit(
                    fxg_db::audit::actions::AGENT_MANAGE,
                    source,
                    None,
                    serde_json::json!({ "action": action, "message": &message }),
                )
                .await;
            state.trigger_node_hello();
            Ok(NodeRpcResponse::AgentOp {
                message: Some(message),
            })
        }
        NodeRpcRequest::ResumeSession {
            session_id,
            force_replay,
        } => {
            let outcome = state
                .session_manager()
                .resume(ResumeParams {
                    command_id: request_id,
                    session_id: session_id.as_str(),
                    force_replay,
                })
                .await?;
            state
                .record_audit(
                    fxg_db::audit::actions::SESSION_RESUME,
                    source,
                    Some(session_id.as_str()),
                    serde_json::json!({ "force_replay": force_replay }),
                )
                .await;
            Ok(NodeRpcResponse::Resume {
                context_restored: outcome.context_restored,
            })
        }
        NodeRpcRequest::ArchiveSession {
            session_id,
            archived,
        } => {
            let archived_at = state
                .set_session_archived(request_id, session_id.as_str(), archived, source)
                .await?;
            Ok(NodeRpcResponse::Archive { archived_at })
        }
        NodeRpcRequest::DeleteSession { session_id } => {
            state
                .purge_session(request_id, session_id.as_str(), source)
                .await?;
            Ok(NodeRpcResponse::Delete)
        }
    }
}

/// Builds the client-facing preview for a resolved project.
async fn project_preview(
    resolved: &crate::project::ResolvedProject,
) -> fxg_protocol::client_api::ProjectPreview {
    let is_worktree = match &resolved.git_root {
        Some(root) => crate::git::is_worktree(root).await.unwrap_or(false),
        None => false,
    };
    resolved.to_preview(is_worktree)
}
