//! Node-scoped operations that the hub relays (`/api/v1/nodes/:node_id/...`).
//!
//! The local node executes them in-process, so a direct client and a hub client
//! see the same payloads.

use axum::Extension;
use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::response::{IntoResponse, Response};
use fxg_protocol::NodeId;
use fxg_protocol::client_api::{AuditLogsResponse, ShutdownResponse};
use fxg_protocol::node_hub::{NodeRpcRequest, NodeRpcResponse};
use serde::Deserialize;

use crate::{ApiError, ClientApiBackend, ClientApiState, ClientInfo};

/// Query of `GET /api/v1/nodes/:node_id/audit/logs`.
#[derive(Debug, Deserialize)]
pub(crate) struct NodeAuditQuery {
    /// Maximum number of entries (default 50)
    limit: Option<u32>,
}

/// `GET /api/v1/nodes/:node_id/diagnostics`.
pub(crate) async fn node_diagnostics<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(&NodeId::from(node_id), NodeRpcRequest::Diagnostics, None)
        .await
    {
        Ok(NodeRpcResponse::Diagnostics { diagnostics }) => Json(*diagnostics).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

/// `GET /api/v1/nodes/:node_id/audit/logs`.
pub(crate) async fn node_audit_logs<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Query(query): Query<NodeAuditQuery>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::ListAuditLogs {
                limit: query.limit.unwrap_or(50),
            },
            None,
        )
        .await
    {
        Ok(NodeRpcResponse::AuditLogs { logs }) => Json(AuditLogsResponse { logs }).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

/// `POST /api/v1/nodes/:node_id/shutdown`.
pub(crate) async fn node_shutdown<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Extension(client): Extension<ClientInfo>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::Shutdown,
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::Shutdown) => Json(ShutdownResponse { ok: true }).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}
