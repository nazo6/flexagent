//! Agent usage limits (plan / account quota).

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::api::{ClientApiBackend, ClientApiState};

/// Query of `GET /api/v1/nodes/:node_id/agents/:agent_id/usage`.
#[derive(Debug, Deserialize)]
pub(crate) struct AgentUsageQuery {
    /// Working directory used for agent-specific lookups
    pub local_path: Option<String>,
}

pub(crate) async fn usage<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    UrlPath((node_id, agent_id)): UrlPath<(String, String)>,
    Query(query): Query<AgentUsageQuery>,
) -> Response {
    match state
        .backend
        .agent_usage(&node_id, &agent_id, query.local_path)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => err.into_response(),
    }
}
