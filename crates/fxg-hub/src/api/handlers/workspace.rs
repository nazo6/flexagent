//! Workspace file browsing and Git inspection (`/api/v1/nodes/:id/workspace/*`).
//!
//! The node enforces that the path belongs to a registered workspace and that
//! the request came from a direct (non-hub) client connection.

use axum::Extension;
use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::response::{IntoResponse, Response};
use fxg_protocol::NodeId;
use fxg_protocol::client_api::{WorkspaceFsQuery, WorkspaceGitDiffQuery, WorkspaceGitStatusQuery};
use fxg_protocol::common::DiffScope;
use fxg_protocol::node_hub::{NodeRpcRequest, NodeRpcResponse};

use crate::api::{ApiError, ClientApiBackend, ClientApiState, ClientInfo};

pub(crate) async fn workspace_fs_list<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Query(query): Query<WorkspaceFsQuery>,
    Extension(client): Extension<ClientInfo>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::WorkspaceFsList { path: query.path },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::WorkspaceFsList { response }) => Json(response).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn workspace_fs_file<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Query(query): Query<WorkspaceFsQuery>,
    Extension(client): Extension<ClientInfo>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::WorkspaceFsFile { path: query.path },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::WorkspaceFile { file }) => Json(file).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn workspace_git_status<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Query(query): Query<WorkspaceGitStatusQuery>,
    Extension(client): Extension<ClientInfo>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::WorkspaceGitStatus { path: query.path },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::WorkspaceGitStatus { response }) => Json(response).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn workspace_git_diff<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Query(query): Query<WorkspaceGitDiffQuery>,
    Extension(client): Extension<ClientInfo>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::WorkspaceGitDiff {
                path: query.path,
                scope: query.scope.unwrap_or(DiffScope::Uncommitted),
                base_branch: query.base,
                file: query.file,
            },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::WorkspaceGitDiff { diff }) => Json(diff).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}
