//! ACP agent catalog and per-node agent operations.

use axum::Extension;
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::response::{IntoResponse, Response};
use fxg_protocol::client_api::ProbeAgentRequest;
use fxg_protocol::common::AgentAction;
use fxg_protocol::settings::CustomAgentConfig;

use crate::api::{ClientApiBackend, ClientApiState, ClientInfo};

pub(crate) async fn list_agents<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.list_agents().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn probe_agent_config<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
    Json(request): Json<ProbeAgentRequest>,
) -> Response {
    match state
        .backend
        .probe_agent_config(&node_id, &agent_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn refresh_catalog<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state
        .backend
        .manage_agent(&node_id, AgentAction::RefreshCatalog, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn upsert_custom_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
    Json(config): Json<CustomAgentConfig>,
) -> Response {
    match state
        .backend
        .manage_agent(
            &node_id,
            AgentAction::UpsertCustom { agent_id, config },
            client,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn remove_custom_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .manage_agent(&node_id, AgentAction::RemoveCustom { agent_id }, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn enable_package_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .manage_agent(
            &node_id,
            AgentAction::SetPackageEnabled {
                agent_id,
                enabled: true,
            },
            client,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn disable_package_agent<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .manage_agent(
            &node_id,
            AgentAction::SetPackageEnabled {
                agent_id,
                enabled: false,
            },
            client,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}
