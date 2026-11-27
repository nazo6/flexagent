//! Composer drafts (`/api/v1/drafts`).
//!
//! Drafts are stored in the connected endpoint's DB (`hub.db` / `node.db`), so
//! the handlers are role-agnostic and use [`ClientApiBackend::db`] directly.
//! Design: `docs/proposals/2026-11-27-composer-drafts-and-sync.md`.

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fxg_db::DbError;
use fxg_protocol::client_api::{CreateDraftRequest, DraftListQuery, UpdateDraftRequest};
use fxg_protocol::common::ErrorCode;

use crate::{ApiError, ClientApiBackend, ClientApiState};

/// Maps draft DB errors to the structured API error (revision conflicts are 409).
fn draft_error(err: DbError) -> ApiError {
    match err {
        DbError::RevisionMismatch => ApiError::from_code(
            ErrorCode::RevisionMismatch,
            "the draft was changed by another device",
        ),
        other => ApiError::from(other),
    }
}

/// `GET /api/v1/drafts`
pub(crate) async fn list_drafts<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Query(query): Query<DraftListQuery>,
) -> Response {
    match state.backend.db().list_drafts(&query).await {
        Ok(drafts) => Json(fxg_protocol::client_api::DraftListResponse { drafts }).into_response(),
        Err(err) => draft_error(err).into_response(),
    }
}

/// `POST /api/v1/drafts`
pub(crate) async fn create_draft<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    Json(request): Json<CreateDraftRequest>,
) -> Response {
    match state.backend.db().create_draft(&request).await {
        Ok(draft) => (StatusCode::CREATED, Json(draft)).into_response(),
        Err(err) => draft_error(err).into_response(),
    }
}

/// `GET /api/v1/drafts/{draft_id}`
pub(crate) async fn get_draft<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(draft_id): UrlPath<String>,
) -> Response {
    match state.backend.db().get_draft(&draft_id).await {
        Ok(Some(draft)) => Json(draft).into_response(),
        Ok(None) => ApiError::not_found("draft not found").into_response(),
        Err(err) => draft_error(err).into_response(),
    }
}

/// `PATCH /api/v1/drafts/{draft_id}`
pub(crate) async fn update_draft<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(draft_id): UrlPath<String>,
    Json(request): Json<UpdateDraftRequest>,
) -> Response {
    match state.backend.db().update_draft(&draft_id, &request).await {
        Ok(Some(draft)) => Json(draft).into_response(),
        Ok(None) => ApiError::not_found("draft not found").into_response(),
        Err(err) => draft_error(err).into_response(),
    }
}

/// `DELETE /api/v1/drafts/{draft_id}` (idempotent)
pub(crate) async fn delete_draft<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath(draft_id): UrlPath<String>,
) -> Response {
    match state.backend.db().delete_draft(&draft_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => draft_error(err).into_response(),
    }
}
