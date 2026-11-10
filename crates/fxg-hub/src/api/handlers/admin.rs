//! Hub administration: tokens, kill switch, shutdown and Web Push.

use axum::Extension;
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fxg_protocol::client_api::{IssueNodeTokenRequest, KillSwitchRequest, PushSubscribeRequest};

use crate::api::{ClientApiBackend, ClientApiState, ClientInfo};

pub(crate) async fn rotate_auth_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
) -> Response {
    match state.backend.rotate_auth_token(client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn node_tokens<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.node_tokens().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn issue_node_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<IssueNodeTokenRequest>,
) -> Response {
    match state.backend.issue_node_token(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn revoke_node_token<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(node_id): UrlPath<String>,
) -> Response {
    match state.backend.revoke_node_token(&node_id, client).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn kill_switch<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<KillSwitchRequest>,
) -> Response {
    let reason = if request.reason.trim().is_empty() {
        "kill switch".to_owned()
    } else {
        request.reason
    };
    match state.backend.kill_switch(&reason, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn shutdown<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    if state.backend.request_shutdown() {
        Json(fxg_protocol::client_api::ShutdownResponse { ok: true }).into_response()
    } else {
        (
            StatusCode::NOT_IMPLEMENTED,
            "shutdown is not available on this endpoint",
        )
            .into_response()
    }
}

pub(crate) async fn push_subscribe<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<PushSubscribeRequest>,
) -> Response {
    match state.backend.push_subscribe(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}
