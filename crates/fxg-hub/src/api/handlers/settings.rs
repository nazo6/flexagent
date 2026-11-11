//! Hub / node settings, secrets and filesystem browsing.

use axum::Extension;
use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::response::{IntoResponse, Response};
use fxg_protocol::node_hub::{NodeRpcRequest, NodeRpcResponse};
use fxg_protocol::settings::{
    SecretsResponse, TestPairingRequest, UpdateAgentSettingsRequest, UpdateHubSettingsRequest,
    UpdateNodeSettingsRequest, UpdatePairingRequest, UpsertSecretRequest,
};
use fxg_protocol::{NodeId, SecretId};

use crate::api::{ApiError, ClientApiBackend, ClientApiState, ClientInfo};

// ----------------------------------------------------------------------
// Settings, secrets and project profiles
// ----------------------------------------------------------------------

pub(crate) async fn settings_server<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.settings_server().await {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn update_hub_settings<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Json(request): Json<UpdateHubSettingsRequest>,
) -> Response {
    match state.backend.update_hub_settings(request).await {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn browse_fs<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Query(query): Query<fxg_protocol::client_api::FsBrowseQuery>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::BrowseFs { path: query.path },
            None,
        )
        .await
    {
        Ok(NodeRpcResponse::BrowseFs { response }) => Json(response).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn node_settings<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::GetNodeSettings,
            None,
        )
        .await
    {
        Ok(NodeRpcResponse::NodeSettings { snapshot }) => Json(*snapshot).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn update_node_settings<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<UpdateNodeSettingsRequest>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::UpdateNodeSettings { request },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::NodeSettings { snapshot }) => Json(*snapshot).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn update_agent_settings<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<UpdateAgentSettingsRequest>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::UpdateAgentSettings { request },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::NodeSettings { snapshot }) => Json(*snapshot).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn update_pairing<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<UpdatePairingRequest>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::UpdatePairing { request },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::NodeSettings { snapshot }) => Json(*snapshot).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn clear_pairing<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<ClearPairingBody>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::ClearPairing {
                expected_revision: request.expected_revision,
            },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::NodeSettings { snapshot }) => Json(*snapshot).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn test_pairing<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
    Json(request): Json<TestPairingRequest>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::TestPairing { request },
            None,
        )
        .await
    {
        Ok(NodeRpcResponse::HubLink { status }) => Json(status).into_response(),
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

/// `DELETE /api/v1/nodes/{node_id}/settings/pairing` body.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ClearPairingBody {
    /// Revision the client observed (`REVISION_MISMATCH` on conflict)
    expected_revision: u64,
}

pub(crate) async fn node_secrets<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(&NodeId::from(node_id), NodeRpcRequest::GetSecrets, None)
        .await
    {
        Ok(NodeRpcResponse::Secrets { secrets }) => {
            Json(SecretsResponse { secrets }).into_response()
        }
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn upsert_node_secret<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, secret_id)): UrlPath<(String, String)>,
    Json(request): Json<UpsertSecretRequest>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::UpsertSecret {
                id: SecretId::new(secret_id),
                request,
            },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::Secrets { secrets }) => {
            Json(SecretsResponse { secrets }).into_response()
        }
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn delete_node_secret<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((node_id, secret_id)): UrlPath<(String, String)>,
) -> Response {
    match state
        .backend
        .execute_node_rpc(
            &NodeId::from(node_id),
            NodeRpcRequest::DeleteSecret {
                id: SecretId::new(secret_id),
            },
            Some(client),
        )
        .await
    {
        Ok(NodeRpcResponse::Secrets { secrets }) => {
            Json(SecretsResponse { secrets }).into_response()
        }
        Ok(_) => ApiError::internal("unexpected node RPC response").into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn server_secrets<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.server_secrets().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn upsert_server_secret<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(secret_id): UrlPath<String>,
    Json(request): Json<UpsertSecretRequest>,
) -> Response {
    match state
        .backend
        .upsert_server_secret(&SecretId::new(secret_id), request)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn delete_server_secret<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(secret_id): UrlPath<String>,
) -> Response {
    match state
        .backend
        .delete_server_secret(&SecretId::new(secret_id))
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn list_agents_node<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state.backend.list_agents_node(&node_id).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}
