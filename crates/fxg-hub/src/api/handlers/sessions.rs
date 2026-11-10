//! Session listing, events, diffs, inbox, search and lifecycle actions.

use std::str::FromStr;

use axum::Extension;
use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fxg_db::SessionFilter;
use fxg_protocol::client_api::{
    AuditLogsResponse, CreateSessionRequest, InboxResponse, RespondElicitationRequest,
    RespondPermissionRequest, ResumeSessionRequest, SearchResponse, SessionArchiveRequest,
    SessionListResponse,
};
use fxg_protocol::common::{DiffScope, SessionStatus};

use crate::api::{ApiError, ClientApiBackend, ClientApiState, ClientInfo, REPLAY_BATCH_SIZE};

#[derive(Debug, serde::Deserialize)]
pub(crate) struct SessionListQuery {
    project_id: Option<String>,
    node_id: Option<String>,
    status: Option<String>,
    /// Whether to include archived sessions (default `false`)
    include_archived: Option<bool>,
    limit: Option<u32>,
}

pub(crate) async fn list_sessions<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<SessionListQuery>,
) -> Response {
    let statuses = match params.status.as_deref() {
        Some(status) => match SessionStatus::from_str(status) {
            Ok(status) => vec![status],
            Err(err) => return ApiError::bad_request(err.to_string()).into_response(),
        },
        None => Vec::new(),
    };
    let filter = SessionFilter {
        project_id: params.project_id.map(fxg_protocol::ProjectId::from),
        node_id: params.node_id.map(fxg_protocol::NodeId::from),
        statuses,
        include_archived: params.include_archived.unwrap_or(false),
        limit: Some(params.limit.unwrap_or(100)),
    };
    match state.backend.db().list_sessions(&filter).await {
        Ok(sessions) => Json(SessionListResponse { sessions }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct EventsQuery {
    after_cursor: Option<u64>,
    limit: Option<u32>,
}

pub(crate) async fn session_events<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(session_id): UrlPath<String>,
    Query(params): Query<EventsQuery>,
) -> Response {
    match state
        .backend
        .db()
        .session_events_after(
            &session_id,
            params.after_cursor.unwrap_or(0),
            params.limit.unwrap_or(REPLAY_BATCH_SIZE),
        )
        .await
    {
        Ok(batch) => Json(batch).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct DiffQuery {
    scope: Option<String>,
    base: Option<String>,
}

pub(crate) async fn session_diff<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(session_id): UrlPath<String>,
    Query(params): Query<DiffQuery>,
) -> Response {
    let scope = match params.scope.as_deref() {
        // Accept both the design notation (`scope=branch`) and the enum name
        None | Some("") | Some("uncommitted") => DiffScope::Uncommitted,
        Some("branch") | Some("branch_base") | Some("branchbase") => DiffScope::BranchBase,
        Some(other) => {
            return ApiError::bad_request(format!("unknown diff scope: {other}")).into_response();
        }
    };
    match state
        .backend
        .workspace_diff(&session_id, scope, params.base)
        .await
    {
        Ok(diff) => Json(diff).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn inbox<B: ClientApiBackend>(State(state): State<ClientApiState<B>>) -> Response {
    let db = state.backend.db();
    match (
        db.pending_permissions().await,
        db.pending_elicitations().await,
    ) {
        (Ok(requests), Ok(elicitations)) => Json(InboxResponse {
            requests,
            elicitations,
        })
        .into_response(),
        (Err(err), _) | (_, Err(err)) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct SearchQuery {
    q: Option<String>,
    limit: Option<u32>,
}

pub(crate) async fn search<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let query = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(50);

    match state.backend.db().search(&query, limit).await {
        Ok(hits) => Json(SearchResponse { hits }).into_response(),
        Err(fxg_db::DbError::EmptySearchQuery) => {
            ApiError::bad_request("search query must not be empty").into_response()
        }
        Err(err) => ApiError::from(err).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct AuditQuery {
    limit: Option<u32>,
}

pub(crate) async fn audit_logs<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(params): Query<AuditQuery>,
) -> Response {
    match state
        .backend
        .db()
        .audit_logs(params.limit.unwrap_or(50))
        .await
    {
        Ok(logs) => Json(AuditLogsResponse { logs }).into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

pub(crate) async fn create_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    Json(request): Json<CreateSessionRequest>,
) -> Response {
    match state.backend.create_session(request, client).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn respond_permission<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((session_id, request_id)): UrlPath<(String, String)>,
    Json(request): Json<RespondPermissionRequest>,
) -> Response {
    match state
        .backend
        .respond_permission(&session_id, &request_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn respond_elicitation<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath((session_id, elicitation_id)): UrlPath<(String, String)>,
    Json(request): Json<RespondElicitationRequest>,
) -> Response {
    match state
        .backend
        .respond_elicitation(&session_id, &elicitation_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn resume_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
    Json(request): Json<ResumeSessionRequest>,
) -> Response {
    match state
        .backend
        .resume_session(&session_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn archive_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
    Json(request): Json<SessionArchiveRequest>,
) -> Response {
    match state
        .backend
        .archive_session(&session_id, request, client)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}

pub(crate) async fn delete_session<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Extension(client): Extension<ClientInfo>,
    UrlPath(session_id): UrlPath<String>,
) -> Response {
    match state.backend.delete_session(&session_id, client).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}
