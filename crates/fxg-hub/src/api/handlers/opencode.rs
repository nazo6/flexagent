//! Native OpenCode management (local node only).

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::response::{IntoResponse, Response};
use fxg_protocol::client_api::{OpenCodeMcpAction, OpenCodeMcpDefinition};

use crate::api::{ClientApiBackend, ClientApiState};

pub(crate) async fn management<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state.backend.opencode_management(&node_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn mcp_action<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath((node_id, name, action)): UrlPath<(String, String, String)>,
) -> Response {
    let action = match action.as_str() {
        "connect" => OpenCodeMcpAction::Connect,
        "disconnect" => OpenCodeMcpAction::Disconnect,
        _ => {
            return crate::api::ApiError::from_code(
                fxg_protocol::common::ErrorCode::InvalidState,
                format!("unknown OpenCode MCP action: {action}"),
            )
            .into_response();
        }
    };
    match state
        .backend
        .opencode_mcp_action(&node_id, &name, action)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn mcp_upsert<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath((node_id, name)): UrlPath<(String, String)>,
    Json(definition): Json<OpenCodeMcpDefinition>,
) -> Response {
    match state
        .backend
        .opencode_mcp_upsert(&node_id, &name, definition)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn mcp_remove<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath((node_id, name)): UrlPath<(String, String)>,
) -> Response {
    match state.backend.opencode_mcp_remove(&node_id, &name).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}
