//! Projects, nodes, worktrees and provisioners.

use axum::Extension;
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fxg_protocol::client_api::{
    CreateWorktreeRequest, NodesResponse, ProjectLinkRequest, ProjectPreviewRequest,
    ProjectScanRequest, ProjectUnlinkRequest, ProjectsResponse, PruneWorktreesRequest,
    RemoveWorktreeRequest,
};

use crate::api::{ApiError, ClientApiBackend, ClientApiState, ClientInfo};

pub(crate) async fn list_projects<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.list_projects().await {
        Ok(projects) => Json(ProjectsResponse { projects }).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn list_nodes<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.db().list_nodes().await {
        Ok(nodes) => Json(NodesResponse { nodes }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

pub(crate) async fn list_provisioners<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.provisioners().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

/// `POST /api/v1/provisioners/:name/test`: validates provisioner startup and handshake.
pub(crate) async fn test_provisioner<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(provisioner): UrlPath<String>,
) -> Response {
    match state.backend.provisioner_test(&provisioner).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn list_worktrees<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(project_id): UrlPath<String>,
) -> Response {
    match state.backend.list_worktrees(&project_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn create_worktree<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<CreateWorktreeRequest>,
) -> Response {
    match state
        .backend
        .create_worktree(&project_id, request, client)
        .await
    {
        Ok(worktree) => Json(worktree).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn remove_worktree<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<RemoveWorktreeRequest>,
) -> Response {
    match state
        .backend
        .remove_worktree(&project_id, request, client)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn prune_worktrees<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(project_id): UrlPath<String>,
    Json(request): Json<PruneWorktreesRequest>,
) -> Response {
    match state
        .backend
        .prune_worktrees(&project_id, request, client)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn scan_projects<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectScanRequest>,
) -> Response {
    match state.backend.scan_projects(&node_id, request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn inspect_project<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectPreviewRequest>,
) -> Response {
    match state
        .backend
        .inspect_project(&node_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn add_project<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectPreviewRequest>,
) -> Response {
    match state.backend.add_project(&node_id, request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn link_project<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectLinkRequest>,
) -> Response {
    match state.backend.link_project(&node_id, request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn unlink_project<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ProjectUnlinkRequest>,
) -> Response {
    match state
        .backend
        .unlink_project(&node_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}
