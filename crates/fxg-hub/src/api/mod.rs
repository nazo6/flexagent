//! Shared HTTP/WS API layer for clients.
//!
//! The hub (`fxg hub start`, `:8080`) and the node's local web server
//! (`fxg node start`, `127.0.0.1:7860`) expose **exactly the same API paths and WebSocket format**
//! (design: `docs/03-protocol-and-api.md` §3).
//!
//! The differences between the two (who owns the DB, how commands execute, whether node relay
//! is involved) are confined to implementations of the [`ClientApiBackend`] trait.
//!
//! - REST handlers: [`handlers`]
//! - Backend trait and shared types: [`backend`]
//! - REST / WS error type: [`error`]
//! - Client WS (`/api/v1/client/ws`): [`ws`]
//! - PTY WS (`/api/v1/pty/ws`): [`pty`]
//! - Security (Host / Origin / Token validation): [`security`]
//! - Token generation and comparison: [`auth`]

pub mod auth;
mod backend;
mod counters;
pub(crate) mod diagnostics;
mod error;
mod handlers;
mod pty;
mod request_context;
mod security;
mod static_ui;
mod ws;

use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::extract::ws::{Message, WebSocket};
use axum::http::{HeaderMap, header};
use axum::middleware;
use axum::routing::{delete, get, patch, post, put};
use futures_util::SinkExt;
use fxg_protocol::client_api::HubWsMessage;
use fxg_protocol::events::SessionEventBatch;

pub use backend::{ClientApiBackend, ClientCommand, ClientEvent, ClientInfo, SystemExtras};
pub use counters::{CounterKind, DiagnosticsCounters};
pub use diagnostics::{incident_retention, log_tail};
pub use error::ApiError;
pub use pty::{PtyChannelError, PtyChannelEvent, PtySpawnParams};
pub use ws::REPLAY_BATCH_SIZE;

use handlers::admin::{
    issue_node_token, kill_switch, node_tokens, push_subscribe, revoke_node_token,
    rotate_auth_token, shutdown,
};
use handlers::agents::{
    disable_package_agent, enable_package_agent, list_agents, probe_agent_config, refresh_catalog,
    remove_custom_agent, upsert_custom_agent,
};
use handlers::projects::{
    add_project, create_worktree, inspect_project, link_project, list_nodes, list_projects,
    list_provisioners, list_worktrees, prune_worktrees, remove_worktree, scan_projects,
    test_provisioner,
};
use handlers::sessions::{
    archive_session, audit_logs, control_session, create_session, delete_session, import_session,
    inbox, list_sessions, respond_elicitation, respond_permission, resume_session, search,
    send_prompt, session_detail, session_diff, session_events,
};
use handlers::settings::{
    browse_fs, delete_node_secret, delete_server_secret, list_agents_node, node_secrets,
    node_settings, server_secrets, settings_server, update_agent_settings, update_hub_settings,
    update_node_settings, upsert_node_secret, upsert_server_secret,
};
use handlers::stream::{client_ws, pty_ws};
use handlers::system::{
    auth_login, auth_logout, diagnostics, diagnostics_bundle, diagnostics_incidents, healthz, meta,
    readyz, resolve_incident, system_info,
};

/// Returns a header value as `&str` (`None` for a missing or non-UTF-8 value).
// `Result::ok` is disallowed by the error-handling policy, so the explicit
// match below is intentional.
#[allow(clippy::manual_ok_err)]
pub(crate) fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    match headers.get(name) {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => None,
        },
        None => None,
    }
}

/// Router construction options.
#[derive(Debug, Clone)]
pub struct ClientApiOptions {
    /// The port actually bound (for Host / Origin validation of localhost / 127.0.0.1)
    pub port: u16,
    /// Hosts allowed in addition to the `localhost` family
    /// (the hub's `server.allowed_hosts`)
    pub extra_allowed_hosts: Vec<String>,
    /// Origins allowed in addition to the same origin
    /// (the hub's `server.allowed_origins`)
    pub extra_allowed_origins: Vec<String>,
}

impl ClientApiOptions {
    /// For a local node (loopback only).
    pub fn local(port: u16) -> Self {
        Self {
            port,
            extra_allowed_hosts: Vec::new(),
            extra_allowed_origins: Vec::new(),
        }
    }
}

/// Shared state for the router.
#[derive(Clone)]
pub struct ClientApiState<B: ClientApiBackend> {
    /// Backend implementation
    pub backend: B,
    /// Security configuration
    pub options: Arc<ClientApiOptions>,
}

/// Unauthenticated connection metadata endpoint (`GET /api/v1/meta`).
///
/// The security middleware ([`security::security`]) exempts only this path from authentication
/// (Host / Origin validation still applies).
pub(crate) const META_PATH: &str = "/api/v1/meta";

/// Builds the shared Client REST / WS / PTY WS router.
///
/// The caller must serve it with
/// `axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())`
/// (so the client IP is recorded in audit logs).
pub fn client_router<B: ClientApiBackend>(backend: B, options: ClientApiOptions) -> Router {
    let state = ClientApiState {
        backend,
        options: Arc::new(options),
    };
    let api = Router::new()
        .route(META_PATH, get(meta::<B>))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz::<B>))
        .route("/api/v1/system/info", get(system_info::<B>))
        .route("/api/v1/system/kill-switch", post(kill_switch::<B>))
        .route("/api/v1/system/shutdown", post(shutdown::<B>))
        .route("/api/v1/diagnostics", get(diagnostics::<B>))
        .route(
            "/api/v1/diagnostics/incidents",
            get(diagnostics_incidents::<B>),
        )
        .route(
            "/api/v1/diagnostics/incidents/{id}/resolve",
            post(resolve_incident::<B>),
        )
        .route("/api/v1/diagnostics/bundle", get(diagnostics_bundle::<B>))
        .route("/api/v1/auth/login", post(auth_login::<B>))
        .route("/api/v1/auth/logout", post(auth_logout))
        .route("/api/v1/projects", get(list_projects::<B>))
        .route(
            "/api/v1/projects/{project_id}/worktrees",
            get(list_worktrees::<B>)
                .post(create_worktree::<B>)
                .delete(remove_worktree::<B>),
        )
        .route(
            "/api/v1/projects/{project_id}/worktrees/prune",
            post(prune_worktrees::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/projects/scan",
            post(scan_projects::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/projects/link",
            post(link_project::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/projects/inspect",
            post(inspect_project::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/projects/add",
            post(add_project::<B>),
        )
        .route("/api/v1/agents", get(list_agents::<B>))
        .route(
            "/api/v1/nodes/{node_id}/agents/refresh",
            post(refresh_catalog::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/agents/custom/{agent_id}",
            put(upsert_custom_agent::<B>).delete(remove_custom_agent::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/agents/{agent_id}/package",
            post(enable_package_agent::<B>).delete(disable_package_agent::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/agents/{agent_id}/probe",
            post(probe_agent_config::<B>),
        )
        .route(
            "/api/v1/nodes/tokens",
            get(node_tokens::<B>).post(issue_node_token::<B>),
        )
        .route(
            "/api/v1/nodes/tokens/{node_id}",
            delete(revoke_node_token::<B>),
        )
        .route("/api/v1/auth/rotate-token", post(rotate_auth_token::<B>))
        .route("/api/v1/nodes", get(list_nodes::<B>))
        .route("/api/v1/nodes/{node_id}/fs/browse", get(browse_fs::<B>))
        .route(
            "/api/v1/settings/hub",
            get(settings_server::<B>).patch(update_hub_settings::<B>),
        )
        .route("/api/v1/nodes/{node_id}/settings", get(node_settings::<B>))
        .route(
            "/api/v1/nodes/{node_id}/settings/node",
            patch(update_node_settings::<B>),
        )
        .route(
            "/api/v1/nodes/{node_id}/settings/agents",
            patch(update_agent_settings::<B>),
        )
        .route("/api/v1/hub/secrets", get(server_secrets::<B>))
        .route(
            "/api/v1/hub/secrets/{secret_id}",
            put(upsert_server_secret::<B>).delete(delete_server_secret::<B>),
        )
        .route("/api/v1/nodes/{node_id}/secrets", get(node_secrets::<B>))
        .route(
            "/api/v1/nodes/{node_id}/secrets/{secret_id}",
            put(upsert_node_secret::<B>).delete(delete_node_secret::<B>),
        )
        .route("/api/v1/nodes/{node_id}/agents", get(list_agents_node::<B>))
        .route("/api/v1/provisioners", get(list_provisioners::<B>))
        .route(
            "/api/v1/provisioners/{provisioner}/test",
            post(test_provisioner::<B>),
        )
        .route(
            "/api/v1/sessions",
            get(list_sessions::<B>).post(create_session::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/events",
            get(session_events::<B>),
        )
        .route("/api/v1/sessions/import", post(import_session::<B>))
        .route(
            "/api/v1/sessions/{session_id}/prompt",
            post(send_prompt::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/control",
            post(control_session::<B>),
        )
        .route("/api/v1/sessions/{session_id}/diff", get(session_diff::<B>))
        .route(
            "/api/v1/sessions/{session_id}/permissions/{request_id}/respond",
            post(respond_permission::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/elicitations/{elicitation_id}/respond",
            post(respond_elicitation::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/resume",
            post(resume_session::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}/archive",
            post(archive_session::<B>),
        )
        .route(
            "/api/v1/sessions/{session_id}",
            get(session_detail::<B>).delete(delete_session::<B>),
        )
        .route("/api/v1/inbox", get(inbox::<B>))
        .route("/api/v1/search", get(search::<B>))
        .route("/api/v1/push/subscribe", post(push_subscribe::<B>))
        .route("/api/v1/audit/logs", get(audit_logs::<B>))
        .route("/api/v1/client/ws", get(client_ws::<B>))
        .route("/api/v1/pty/ws", get(pty_ws::<B>))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security::security::<B>,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            diagnostics::record_api_errors::<B>,
        ))
        .layer(middleware::from_fn(request_context::request_context))
        .with_state(state);
    // Serve the embedded Web UI (SPA) for every non-API path.
    // It sits outside the security middleware (the UI assets themselves need no token).
    api.fallback(get(static_ui::serve_ui))
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

/// Shared helper for sending over WS (`send_json`).
pub(crate) async fn send_ws_json<T: serde::Serialize>(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: &T,
) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|err| {
        tracing::warn!("failed to encode ws message: {err}");
    })?;
    sender
        .send(Message::Text(text.into()))
        .await
        .map_err(|err| {
            tracing::debug!("ws send failed: {err}");
        })
}

/// Sends all events from `cursor` onward as `SessionEventBatch` messages.
pub(crate) async fn replay_events<B: ClientApiBackend>(
    backend: &B,
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    cursor: &mut u64,
) -> Result<(), String> {
    loop {
        let batch: SessionEventBatch = backend
            .db()
            .events_after_cursor(*cursor, REPLAY_BATCH_SIZE)
            .await
            .map_err(|err| {
                tracing::warn!("failed to load events for replay: {err}");
                format!("failed to load events for replay: {err}")
            })?;
        if batch.events.is_empty() {
            return Ok(());
        }
        let full_batch = batch.events.len() as u32 >= REPLAY_BATCH_SIZE;
        *cursor = (*cursor).max(batch.cursor);
        send_ws_json(
            sender,
            &HubWsMessage::EventBatch {
                events: batch.events,
                cursor: batch.cursor,
            },
        )
        .await
        .map_err(|()| "failed to send replayed events".to_owned())?;
        if !full_batch {
            return Ok(());
        }
    }
}

/// Whether the `Request` is a WebSocket upgrade.
pub(crate) fn is_websocket_upgrade(request: &Request) -> bool {
    header_str(request.headers(), header::UPGRADE)
        .map(|value| value.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}
