//! Workspace terminals (`GET /api/v1/terminals`).
//!
//! The list is live (node memory only): a terminal that disappeared on a node
//! restart is simply absent. Both the node and the hub serve the same response;
//! the hub relays a [`NodeRpcRequest::ListTerminals`] to the owning node.

use axum::Extension;
use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use fxg_protocol::client_api::TerminalsQuery;
use fxg_protocol::node_hub::{NodeRpcRequest, NodeRpcResponse};

use crate::api::{ApiError, ClientApiBackend, ClientApiState, ClientInfo};

pub(crate) async fn list_terminals<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Query(query): Query<TerminalsQuery>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(&query.node_id, NodeRpcRequest::ListTerminals, Some(client))
        .await
    {
        Ok(NodeRpcResponse::Terminals { response }) => Json(response).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}
