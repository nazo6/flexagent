//! Connection metadata, health checks and authentication.

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use fxg_protocol::client_api::{AuthLoginRequest, MetaResponse, SystemInfoResponse};
use fxg_protocol::common::ErrorCode;

use crate::api::auth::token_matches;
use crate::api::{ApiError, ClientApiBackend, ClientApiState};

/// `GET /api/v1/meta`: connection metadata that needs no authentication.
///
/// Returns only the connection kind so an unauthenticated Web UI can show the token input
/// dialog correctly (the only API reachable without auth. Host / Origin validation applies).
pub(crate) async fn meta<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    Json(MetaResponse {
        role: state.backend.connection_role(),
    })
    .into_response()
}

pub(crate) async fn healthz() -> Response {
    (StatusCode::OK, "ok").into_response()
}

pub(crate) async fn readyz<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.db().list_projects().await {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("database not ready: {err}"),
        )
            .into_response(),
    }
}

pub(crate) async fn diagnostics<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    match state.backend.diagnostics().await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn system_info<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
) -> Response {
    let extras = match state.backend.system_extras().await {
        Ok(extras) => extras,
        Err(err) => return err.into_response(),
    };
    Json(SystemInfoResponse {
        role: state.backend.connection_role(),
        node_id: state.backend.local_node_id(),
        version: state.backend.version().to_owned(),
        vapid_public_key: extras.vapid_public_key,
        unsynced_event_count: extras.unsynced_event_count,
        central_connected: extras.central_connected,
    })
    .into_response()
}

/// Name of the auth cookie (`docs/03` §3.0).
const SESSION_COOKIE: &str = "fxg_session";

/// `POST /api/v1/auth/login`: validates the Web UI's token input and issues an `fxg_session`
/// cookie (HttpOnly / SameSite=Strict).
///
/// The endpoint itself must also pass the shared security middleware's authentication
/// (`Authorization: Bearer`), so the token in the body is only re-validated.
pub(crate) async fn auth_login<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Json(request): Json<AuthLoginRequest>,
) -> Response {
    let token = request.token.trim();
    if !token_matches(&state.backend.auth_token(), token) {
        return ApiError::from_code(ErrorCode::Unauthorized, "invalid auth token").into_response();
    }
    let cookie = format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

/// `POST /api/v1/auth/logout`: invalidates the `fxg_session` cookie.
pub(crate) async fn auth_logout() -> Response {
    let cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}
