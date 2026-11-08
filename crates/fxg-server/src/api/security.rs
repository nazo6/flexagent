//! Shared security middleware (Host / Origin / Token validation).
//!
//! Design: `docs/03-protocol-and-api.md` §3.0 / `docs/01-architecture-and-sync.md` §7.1–§7.3.
//! The local node (`127.0.0.1:7860`) and the central server (`:8080`) share the same validation
//! logic, swapping only the allowed hosts / origins via
//! [`ClientApiOptions`](super::ClientApiOptions).
//!
//! 1. **Host header validation** (DNS rebinding mitigation): anything other than `localhost` /
//!    `127.0.0.1` / `[::1]` and the configured allowed hosts gets `403 Forbidden`
//! 2. **Origin header validation** (CSWSH mitigation): on a WebSocket handshake, anything other
//!    than the same origin gets `403 Forbidden` (non-browser clients without `Origin` are allowed)
//! 3. **Authentication**: `Authorization: Bearer <auth_token>` or
//!    `Cookie: fxg_session=<auth_token>`. Unauthenticated requests get `401 Unauthorized`.
//!    The only exception is the connection metadata (`GET /api/v1/meta`), where only Host /
//!    Origin validation applies (an unauthenticated Web UI needs it to determine the connection
//!    kind).

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::auth::token_matches;
use super::{
    ApiError, ClientApiBackend, ClientApiState, ClientInfo, header_str, is_websocket_upgrade,
};

/// Middleware that validates Host / Origin / Token.
///
/// For authenticated requests it inserts [`ClientInfo`] (source IP / UA / auth subject) into the
/// extensions (handlers receive it as `Extension<ClientInfo>`).
pub(crate) async fn security<B: ClientApiBackend>(
    State(state): State<ClientApiState<B>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(response) = verify_host(&state, request.headers()) {
        return response;
    }
    if let Some(response) = verify_origin(&state, &request) {
        return response;
    }
    if !is_auth_exempt(&request)
        && let Some(response) = verify_auth(&state, request.headers())
    {
        return response;
    }
    let info = client_info(&request);
    request.extensions_mut().insert(info);
    next.run(request).await
}

/// Whether the route is exempt from authentication (`GET /api/v1/meta` and health probes).
///
/// Host / Origin validation still applies as usual.
fn is_auth_exempt(request: &Request) -> bool {
    request.method() == Method::GET
        && matches!(
            request.uri().path(),
            super::META_PATH | "/healthz" | "/readyz"
        )
}

/// Builds [`ClientInfo`] from the connection details.
///
/// `ConnectInfo` is inserted into the extensions via `into_make_service_with_connect_info`
/// (treated as `local` when absent, e.g. in tests).
fn client_info(request: &Request) -> ClientInfo {
    let addr = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ConnectInfo(addr)| *addr);
    ClientInfo::from_request(request.headers(), addr)
}

/// Host header validation (DNS rebinding mitigation). Returns an error response on rejection.
fn verify_host<B: ClientApiBackend>(
    state: &ClientApiState<B>,
    headers: &HeaderMap,
) -> Option<Response> {
    let Some(host) = header_str(headers, header::HOST) else {
        return Some(ApiError::forbidden("missing Host header").into_response());
    };
    let port = state.options.port;
    if is_loopback_host(host, port)
        || state
            .options
            .extra_allowed_hosts
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(host))
    {
        None
    } else {
        Some(ApiError::forbidden(format!("host not allowed: {host}")).into_response())
    }
}

/// Whether it is `localhost` / `127.0.0.1` / `[::1]` (including with a port).
fn is_loopback_host(host: &str, port: u16) -> bool {
    let allowed = [
        format!("localhost:{port}"),
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
    ];
    allowed
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(host))
}

/// Origin validation for WebSocket handshakes (CSWSH mitigation).
///
/// Non-browser clients (CLI / native) do not send `Origin`, so they are allowed.
fn verify_origin<B: ClientApiBackend>(
    state: &ClientApiState<B>,
    request: &Request,
) -> Option<Response> {
    if !is_websocket_upgrade(request) {
        return None;
    }
    // Allow requests without an Origin header (non-browser clients)
    let origin = header_str(request.headers(), header::ORIGIN)?;
    let port = state.options.port;
    let allowed = [
        format!("http://localhost:{port}"),
        format!("http://127.0.0.1:{port}"),
        format!("http://[::1]:{port}"),
    ];
    if allowed.iter().any(|candidate| candidate == origin)
        || state
            .options
            .extra_allowed_origins
            .iter()
            .any(|candidate| candidate == origin)
    {
        None
    } else {
        Some(ApiError::forbidden(format!("origin not allowed: {origin}")).into_response())
    }
}

/// Authentication (Bearer header or `fxg_session` cookie). Returns 401 on rejection.
fn verify_auth<B: ClientApiBackend>(
    state: &ClientApiState<B>,
    headers: &HeaderMap,
) -> Option<Response> {
    let expected = state.backend.auth_token();

    if let Some(value) = header_str(headers, header::AUTHORIZATION)
        && let Some(provided) = value.strip_prefix("Bearer ")
        && token_matches(&expected, provided.trim())
    {
        return None;
    }

    if let Some(cookie_header) = header_str(headers, header::COOKIE)
        && let Some(provided) = extract_cookie(cookie_header, "fxg_session")
        && token_matches(&expected, &provided)
    {
        return None;
    }

    Some(
        ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            fxg_protocol::common::ErrorCode::Unauthorized,
            "missing or invalid auth token",
        )
        .into_response(),
    )
}

/// Extracts the value of the given key from a `Cookie` header.
fn extract_cookie(header: &str, key: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if name.trim() == key {
            Some(value.trim().to_owned())
        } else {
            None
        }
    })
}
