//! Generic agent-native management (local node only).

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::response::{IntoResponse, Response};
use fxg_protocol::client_api::{AgentMcpAction, AgentMcpDefinition};

use crate::api::{ClientApiBackend, ClientApiState};

pub(crate) async fn management<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state.backend.agent_management(&node_id, &agent_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn mcp_action<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath((node_id, agent_id, name, action)): UrlPath<(String, String, String, String)>,
) -> Response {
    let action = match action.as_str() {
        "connect" => AgentMcpAction::Connect,
        "disconnect" => AgentMcpAction::Disconnect,
        _ => {
            return crate::api::ApiError::from_code(
                fxg_protocol::common::ErrorCode::InvalidState,
                format!("unknown agent MCP action: {action}"),
            )
            .into_response();
        }
    };
    match state
        .backend
        .agent_mcp_action(&node_id, &agent_id, &name, action)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn mcp_upsert<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath((node_id, agent_id, name)): UrlPath<(String, String, String)>,
    Json(definition): Json<AgentMcpDefinition>,
) -> Response {
    match state
        .backend
        .agent_mcp_upsert(&node_id, &agent_id, &name, definition)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn mcp_remove<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath((node_id, agent_id, name)): UrlPath<(String, String, String)>,
) -> Response {
    match state
        .backend
        .agent_mcp_remove(&node_id, &agent_id, &name)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}
