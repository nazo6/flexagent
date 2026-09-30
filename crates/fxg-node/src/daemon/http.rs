//! ローカルHTTP/WSサーバー (`127.0.0.1:7860`)。
//!
//! 設計: `docs/03-protocol-and-api.md` §3.0〜§3.2、
//! `docs/01-architecture-and-sync.md` §7.1〜§7.3。
//!
//! # セキュリティ (Defense in Depth)
//!
//! 1. **ループバック厳格バインド**: `127.0.0.1` のみにバインドする (呼び出し側で保証)
//! 2. **Host ヘッダ検証** (DNS Rebinding 対策): `localhost` / `127.0.0.1` / `[::1]`
//!    以外は `403 Forbidden`
//! 3. **Origin ヘッダ検証** (CSWSH 対策): WebSocket ハンドシェイクで
//!    自オリジン以外は `403 Forbidden`
//! 4. **認証**: `Authorization: Bearer <auth_token>` または
//!    `Cookie: fxg_session=<auth_token>`。未認証は `401 Unauthorized`
//!
//! エラーは `{ "error": { "code": "<ErrorCode>", "message": "..." } }` に統一する。

use std::net::SocketAddr;
use std::str::FromStr;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use fxg_db::SessionFilter;
use fxg_protocol::client_api::{
    ApiErrorBody, ApiErrorResponse, AuditLogsResponse, ConnectionRole, InboxResponse,
    KillSwitchRequest, KillSwitchResponse, NodesResponse, ProjectsResponse, SearchResponse,
    ServerWsMessage, SessionListResponse, SystemInfoResponse,
};
use fxg_protocol::common::{ErrorCode, SessionStatus};
use fxg_protocol::events::SessionEventBatch;
use tokio::sync::{broadcast, watch};

use super::{DaemonState, token_matches};
use crate::session::SessionBroadcast;

/// WS リプレイの1バッチあたりの件数。
const REPLAY_BATCH_SIZE: u32 = 500;

/// アプリケーション状態 (デーモン状態 + 実際にバインドされたアドレス)。
#[derive(Clone)]
pub(crate) struct AppState {
    daemon: DaemonState,
    http_addr: SocketAddr,
}

/// HTTPサーバーを起動する (シャットダウン要求まで稼働)。
pub async fn serve(
    state: DaemonState,
    listener: tokio::net::TcpListener,
    mut shutdown: watch::Receiver<bool>,
) {
    let http_addr = listener
        .local_addr()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 0)));
    let app = router(state, http_addr);
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.changed().await;
    });
    tracing::info!(addr = %http_addr, "local http server listening");
    if let Err(err) = server.await {
        tracing::error!("local http server stopped: {err}");
    }
}

/// ルーターを構築する (テストから直接利用できるよう `pub(crate)`)。
pub(crate) fn router(daemon: DaemonState, http_addr: SocketAddr) -> Router {
    let state = AppState { daemon, http_addr };
    Router::new()
        .route("/api/v1/system/info", get(system_info))
        .route("/api/v1/system/kill-switch", post(kill_switch))
        .route("/api/v1/projects", get(list_projects))
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/sessions/{session_id}/events", get(session_events))
        .route("/api/v1/inbox", get(inbox))
        .route("/api/v1/search", post(search))
        .route("/api/v1/audit/logs", get(audit_logs))
        .route("/api/v1/client/ws", get(client_ws))
        .layer(middleware::from_fn_with_state(state.clone(), security))
        .with_state(state)
}

// ----------------------------------------------------------------------
// セキュリティミドルウェア
// ----------------------------------------------------------------------

async fn security(State(app): State<AppState>, request: Request, next: Next) -> Response {
    if let Some(response) = verify_host(&app, request.headers()) {
        return response;
    }
    if let Some(response) = verify_origin(&app, &request) {
        return response;
    }
    if let Some(response) = verify_auth(&app, request.headers()) {
        return response;
    }
    next.run(request).await
}

/// Host ヘッダ検証 (DNS Rebinding 対策)。拒否時はエラーレスポンスを返す。
fn verify_host(app: &AppState, headers: &HeaderMap) -> Option<Response> {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return Some(api_error(
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
            "missing Host header",
        ));
    };
    let port = app.http_addr.port();
    let allowed = [
        format!("localhost:{port}"),
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
    ];
    if allowed
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(host))
    {
        None
    } else {
        Some(api_error(
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
            format!("host not allowed: {host}"),
        ))
    }
}

/// WebSocket ハンドシェイクの Origin 検証 (CSWSH 対策)。
///
/// 非ブラウザクライアント (CLI / native) は `Origin` を送らないため許可する。
fn verify_origin(app: &AppState, request: &Request) -> Option<Response> {
    let is_upgrade = request
        .headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    if !is_upgrade {
        return None;
    }
    // Origin ヘッダが無い場合 (非ブラウザクライアント) は許可する
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())?;
    let port = app.http_addr.port();
    let allowed = [
        format!("http://localhost:{port}"),
        format!("http://127.0.0.1:{port}"),
        format!("http://[::1]:{port}"),
    ];
    if allowed.iter().any(|candidate| candidate == origin) {
        None
    } else {
        Some(api_error(
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
            format!("origin not allowed: {origin}"),
        ))
    }
}

/// 認証 (Bearer ヘッダまたは `fxg_session` Cookie)。拒否時は 401 を返す。
fn verify_auth(app: &AppState, headers: &HeaderMap) -> Option<Response> {
    let expected = app.daemon.auth_token();

    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        && let Some(provided) = value.strip_prefix("Bearer ")
        && token_matches(&expected, provided.trim())
    {
        return None;
    }

    if let Some(cookie_header) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        && let Some(provided) = extract_cookie(cookie_header, "fxg_session")
        && token_matches(&expected, &provided)
    {
        return None;
    }

    Some(api_error(
        StatusCode::UNAUTHORIZED,
        ErrorCode::Unauthorized,
        "missing or invalid auth token",
    ))
}

/// `Cookie` ヘッダから指定キーの値を取り出す。
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

fn api_error(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErrorResponse {
            error: ApiErrorBody {
                code,
                message: message.into(),
            },
        }),
    )
        .into_response()
}

fn internal_error(err: impl std::fmt::Display) -> Response {
    tracing::warn!("local api error: {err}");
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::Internal,
        err.to_string(),
    )
}

// ----------------------------------------------------------------------
// REST ハンドラ
// ----------------------------------------------------------------------

async fn system_info(State(app): State<AppState>) -> Response {
    let unsynced = match app.daemon.db().unsynced_event_count().await {
        Ok(count) => count,
        Err(err) => return internal_error(err),
    };
    Json(SystemInfoResponse {
        role: ConnectionRole::LocalNode,
        version: app.daemon.version().to_owned(),
        // Web Push は中央サーバー専用 (Phase 5)
        vapid_public_key: None,
        unsynced_event_count: Some(unsynced),
        // Outbox 同期ワーカーは Phase 4 で実装する
        central_connected: Some(false),
    })
    .into_response()
}

async fn list_projects(State(app): State<AppState>) -> Response {
    match app.daemon.db().list_projects().await {
        Ok(projects) => Json(ProjectsResponse { projects }).into_response(),
        Err(err) => internal_error(err),
    }
}

async fn list_nodes(State(app): State<AppState>) -> Response {
    match app.daemon.db().list_nodes().await {
        Ok(nodes) => Json(NodesResponse { nodes }).into_response(),
        Err(err) => internal_error(err),
    }
}

#[derive(Debug, serde::Deserialize)]
struct SessionListQuery {
    project_id: Option<String>,
    node_id: Option<String>,
    status: Option<String>,
    limit: Option<u32>,
}

async fn list_sessions(
    State(app): State<AppState>,
    Query(params): Query<SessionListQuery>,
) -> Response {
    let statuses = match params.status.as_deref() {
        Some(status) => match SessionStatus::from_str(status) {
            Ok(status) => vec![status],
            Err(err) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::InvalidState,
                    err.to_string(),
                );
            }
        },
        None => Vec::new(),
    };
    let filter = SessionFilter {
        project_id: params.project_id,
        node_id: params.node_id,
        statuses,
        limit: Some(params.limit.unwrap_or(100)),
    };
    match app.daemon.db().list_sessions(&filter).await {
        Ok(sessions) => Json(SessionListResponse { sessions }).into_response(),
        Err(err) => internal_error(err),
    }
}

#[derive(Debug, serde::Deserialize)]
struct EventsQuery {
    after_cursor: Option<u64>,
    limit: Option<u32>,
}

async fn session_events(
    State(app): State<AppState>,
    UrlPath(session_id): UrlPath<String>,
    Query(params): Query<EventsQuery>,
) -> Response {
    let after_cursor = params.after_cursor.unwrap_or(0);
    let limit = params.limit.unwrap_or(REPLAY_BATCH_SIZE);
    match app
        .daemon
        .db()
        .session_events_after(&session_id, after_cursor, limit)
        .await
    {
        Ok(batch) => Json(batch).into_response(),
        Err(err) => internal_error(err),
    }
}

async fn inbox(State(app): State<AppState>) -> Response {
    match app.daemon.db().pending_permissions().await {
        Ok(requests) => Json(InboxResponse { requests }).into_response(),
        Err(err) => internal_error(err),
    }
}

#[derive(Debug, serde::Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct SearchBody {
    query: Option<String>,
    limit: Option<u32>,
}

async fn search(
    State(app): State<AppState>,
    Query(params): Query<SearchQuery>,
    body: Option<Json<SearchBody>>,
) -> Response {
    let body = body.map(|Json(body)| body);
    let query = params
        .q
        .or_else(|| body.as_ref().and_then(|body| body.query.clone()))
        .unwrap_or_default();
    let limit = params
        .limit
        .or_else(|| body.as_ref().and_then(|body| body.limit))
        .unwrap_or(50);

    match app.daemon.db().search(&query, limit).await {
        Ok(hits) => Json(SearchResponse { hits }).into_response(),
        Err(fxg_db::DbError::EmptySearchQuery) => api_error(
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidState,
            "search query must not be empty",
        ),
        Err(err) => internal_error(err),
    }
}

#[derive(Debug, serde::Deserialize)]
struct AuditQuery {
    limit: Option<u32>,
}

async fn audit_logs(State(app): State<AppState>, Query(params): Query<AuditQuery>) -> Response {
    match app.daemon.db().audit_logs(params.limit.unwrap_or(50)).await {
        Ok(logs) => Json(AuditLogsResponse { logs }).into_response(),
        Err(err) => internal_error(err),
    }
}

async fn kill_switch(
    State(app): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(request): Json<KillSwitchRequest>,
) -> Response {
    let client_ip = addr.ip().to_string();
    let reason = if request.reason.trim().is_empty() {
        "kill switch".to_owned()
    } else {
        request.reason
    };
    match app.daemon.kill_all_local(&reason).await {
        Ok((killed_sessions, killed_ptys)) => {
            tracing::warn!(
                client = %client_ip,
                killed_sessions = killed_sessions.len(),
                killed_ptys = killed_ptys.len(),
                "kill switch executed via local api"
            );
            let mut audit = fxg_db::AuditLogRecord::new(
                fxg_db::audit::actions::KILL_SWITCH,
                client_ip.clone(),
                "local-api",
            );
            audit.node_id = Some(app.daemon.node_id().to_owned());
            audit.details = serde_json::json!({
                "reason": reason,
                "killed_sessions": killed_sessions,
                "killed_ptys": killed_ptys,
            });
            if let Err(err) = app.daemon.db().append_audit_log(&audit).await {
                tracing::warn!("failed to record kill-switch audit log: {err}");
            }
            // ローカルノード自身のみ停止する (中央サーバー配信は Phase 4)
            Json(KillSwitchResponse { notified_nodes: 1 }).into_response()
        }
        Err(err) => internal_error(err),
    }
}

// ----------------------------------------------------------------------
// Client WebSocket (`/api/v1/client/ws`)
// ----------------------------------------------------------------------

async fn client_ws(State(app): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let daemon = app.daemon.clone();
    ws.on_upgrade(move |socket| handle_client_ws(daemon, socket))
}

/// Client WS の接続処理 (Subscribe → リプレイ → ライブ配信)。
async fn handle_client_ws(daemon: DaemonState, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let mut subscription: Option<broadcast::Receiver<SessionBroadcast>> = None;
    let mut cursor: u64 = 0;

    /// select で待つ次のイベント。
    ///
    /// `SessionBroadcast` は大きいため `Box` で包んで enum のサイズ差を抑える。
    enum Next {
        Client(Option<Result<Message, axum::Error>>),
        Bus(Box<Result<SessionBroadcast, broadcast::error::RecvError>>),
    }

    loop {
        let next = match subscription.as_mut() {
            Some(rx) => tokio::select! {
                incoming = receiver.next() => Next::Client(incoming),
                event = rx.recv() => Next::Bus(Box::new(event)),
            },
            None => Next::Client(receiver.next().await),
        };

        match next {
            Next::Client(None) | Next::Client(Some(Err(_))) => break,
            Next::Client(Some(Ok(message))) => match message {
                Message::Text(text) => {
                    let parsed: Result<fxg_protocol::client_api::ClientWsMessage, _> =
                        serde_json::from_str(&text);
                    match parsed {
                        Ok(fxg_protocol::client_api::ClientWsMessage::Subscribe {
                            since_cursor,
                            ..
                        }) => {
                            cursor = since_cursor.unwrap_or(0);
                            if replay(&daemon, &mut sender, &mut cursor).await.is_err() {
                                break;
                            }
                            subscription = Some(daemon.bus().subscribe());
                        }
                        Ok(fxg_protocol::client_api::ClientWsMessage::Ping) => {
                            if send_json(&mut sender, &ServerWsMessage::Pong)
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Ok(other) => {
                            // セッション操作コマンドは Phase 3 で実装する
                            let response = ServerWsMessage::CommandResult {
                                command_id: client_command_id(&other),
                                success: false,
                                code: Some(ErrorCode::InvalidState),
                                error: Some(
                                    "agent session commands are implemented in phase 3 (fxg-acp)"
                                        .to_owned(),
                                ),
                                session_id: None,
                            };
                            if send_json(&mut sender, &response).await.is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            let response = ServerWsMessage::Error {
                                code: ErrorCode::InvalidState,
                                message: format!("invalid client message: {err}"),
                            };
                            if send_json(&mut sender, &response).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                Message::Ping(payload) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            },
            Next::Bus(event) => match *event {
                Ok(message) => {
                    if forward_event(&mut sender, &mut cursor, message)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // 購読が遅延した場合はカーソルから履歴を取り直す
                    tracing::warn!(skipped, "client ws lagged; replaying from cursor");
                    if replay(&daemon, &mut sender, &mut cursor).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

fn client_command_id(message: &fxg_protocol::client_api::ClientWsMessage) -> String {
    use fxg_protocol::client_api::ClientWsMessage;
    match message {
        ClientWsMessage::SendPrompt { command_id, .. }
        | ClientWsMessage::RespondPermission { command_id, .. }
        | ClientWsMessage::ControlSession { command_id, .. } => command_id.clone(),
        _ => String::new(),
    }
}

/// `cursor` 以降の未取得イベントを `SessionEventBatch` として送信する。
async fn replay(
    daemon: &DaemonState,
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    cursor: &mut u64,
) -> Result<(), ()> {
    loop {
        let batch: SessionEventBatch = daemon
            .db()
            .events_after_cursor(*cursor, REPLAY_BATCH_SIZE)
            .await
            .map_err(|err| {
                tracing::warn!("failed to load events for replay: {err}");
            })?;
        if batch.events.is_empty() {
            return Ok(());
        }
        let full_batch = batch.events.len() as u32 >= REPLAY_BATCH_SIZE;
        *cursor = batch.cursor;
        send_json(
            sender,
            &ServerWsMessage::EventBatch {
                events: batch.events,
                cursor: batch.cursor,
            },
        )
        .await?;
        if !full_batch {
            return Ok(());
        }
    }
}

/// バスからのイベントをクライアントへ転送する。
async fn forward_event(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    cursor: &mut u64,
    message: SessionBroadcast,
) -> Result<(), ()> {
    match message {
        SessionBroadcast::Persisted {
            event,
            cursor: event_cursor,
        } => {
            *cursor = (*cursor).max(event_cursor);
            send_json(
                sender,
                &ServerWsMessage::EventBatch {
                    events: vec![event],
                    cursor: event_cursor,
                },
            )
            .await
        }
        SessionBroadcast::StreamDelta { session_id, delta } => {
            send_json(
                sender,
                &ServerWsMessage::LiveStreamDelta { session_id, delta },
            )
            .await
        }
        // エフェメラルイベント (キーストローク等) は PTY WS 経由で配信するため
        // Client WS では送らない (永続化されないため混乱を避ける)。
        SessionBroadcast::Ephemeral(_) => Ok(()),
    }
}

async fn send_json(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: &ServerWsMessage,
) -> Result<(), ()> {
    let text = serde_json::to_string(message).map_err(|err| {
        tracing::warn!("failed to encode server ws message: {err}");
    })?;
    sender
        .send(Message::Text(text.into()))
        .await
        .map_err(|err| {
            tracing::debug!("client ws send failed: {err}");
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonConfig, NodeDaemon};
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_daemon() -> (NodeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "http-node", "HTTP Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        config.ipc_endpoint = crate::testutil::test_ipc_endpoint(dir.path());
        let daemon = NodeDaemon::start(config).await.expect("start");
        (daemon, dir)
    }

    /// テスト用の `Host` ヘッダ値 (実際にバインドされたポートを使う)。
    fn host_of(daemon: &NodeDaemon) -> String {
        format!("127.0.0.1:{}", daemon.http_addr().port())
    }

    fn get(host: &str, uri: &str, token: Option<&str>) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder()
            .method("GET")
            .uri(uri)
            .header(header::HOST, host);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::empty()).expect("request")
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn rejects_unauthenticated_and_wrong_host_requests() {
        let (daemon, _dir) = test_daemon().await;
        let addr = daemon.http_addr();
        let app = router(daemon.state().clone(), addr);
        let token = daemon.state().auth_token();
        let host = host_of(&daemon);

        // 未認証 → 401
        let response = app
            .clone()
            .oneshot(get(&host, "/api/v1/system/info", None))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "UNAUTHORIZED");

        // 不正な Host → 403 (DNS Rebinding 対策)
        let request = HttpRequest::builder()
            .method("GET")
            .uri("/api/v1/system/info")
            .header(header::HOST, "evil.example.com")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "FORBIDDEN");

        // 誤ったトークン → 401
        let response = app
            .clone()
            .oneshot(get(&host, "/api/v1/system/info", Some("deadbeef")))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // 正しいトークン (Bearer) → 200
        let response = app
            .clone()
            .oneshot(get(&host, "/api/v1/system/info", Some(&token)))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["role"], "local_node");
        assert_eq!(body["version"], crate::daemon::VERSION);
        assert_eq!(body["central_connected"], false);

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn accepts_cookie_authentication() {
        let (daemon, _dir) = test_daemon().await;
        let app = router(daemon.state().clone(), daemon.http_addr());
        let token = daemon.state().auth_token();
        let host = host_of(&daemon);

        let request = HttpRequest::builder()
            .method("GET")
            .uri("/api/v1/nodes")
            .header(header::HOST, &host)
            .header(
                header::COOKIE,
                format!("theme=dark; fxg_session={token}; other=1"),
            )
            .body(Body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["nodes"].as_array().expect("nodes").len(), 1);
        assert_eq!(body["nodes"][0]["node_id"], "http-node");

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn rejects_cross_origin_websocket_upgrade() {
        let (daemon, _dir) = test_daemon().await;
        let host = host_of(&daemon);
        let token = daemon.state().auth_token();

        // 外部オリジンからの WS ハンドシェイクは 403 で拒否される (CSWSH 対策)
        let request = ws_request(&host, &token, "http://evil.example.com");
        let err = tokio_tungstenite::connect_async(request)
            .await
            .expect_err("must be rejected");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(response) => {
                assert_eq!(response.status(), 403);
            }
            other => panic!("unexpected error: {other}"),
        }

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    /// WS ハンドシェイク用のリクエストを組み立てる。
    fn ws_request(
        host: &str,
        token: &str,
        origin: &str,
    ) -> tokio_tungstenite::tungstenite::http::Request<()> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let mut request = format!("ws://{host}/api/v1/client/ws")
            .into_client_request()
            .expect("ws request");
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}").parse().expect("header value"),
        );
        request
            .headers_mut()
            .insert("Origin", origin.parse().expect("header value"));
        request
    }

    #[tokio::test]
    async fn client_ws_replays_history_and_streams_live_events() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let (daemon, _dir) = test_daemon().await;
        let host = host_of(&daemon);
        let token = daemon.state().auth_token();

        // 購読前に1イベントを永続化しておく (リプレイの検証用)
        let session_id = fxg_protocol::util::uuid_v7();
        daemon
            .state()
            .bus()
            .create_session(
                &session_id,
                fxg_protocol::events::UnifiedEventPayload::SessionCreated {
                    node_id: daemon.state().node_id().to_owned(),
                    project_id: "github.com/nazo6/flexagent".to_owned(),
                    project_name: "flexagent".to_owned(),
                    local_path: "/tmp/repo".to_owned(),
                    git_branch: Some("main".to_owned()),
                    is_worktree: false,
                    agent_id: "opencode2".to_owned(),
                    parent_session_id: None,
                    fork_from_node_seq: None,
                    title: "New Session".to_owned(),
                },
            )
            .await
            .expect("create session");

        let request = ws_request(&host, &token, &format!("http://{host}"));
        let (mut socket, response) = tokio_tungstenite::connect_async(request)
            .await
            .expect("handshake");
        assert_eq!(response.status(), 101, "self origin must upgrade");

        // Subscribe → リプレイ
        let subscribe = fxg_protocol::client_api::ClientWsMessage::Subscribe {
            since_cursor: None,
            focused_session_id: Some(session_id.clone()),
        };
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&subscribe).unwrap().into(),
            ))
            .await
            .expect("send subscribe");

        let message = next_ws_message(&mut socket).await;
        match message {
            ServerWsMessage::EventBatch { events, cursor } => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].session_id, session_id);
                assert_eq!(events[0].node_seq, 1);
                assert!(cursor > 0, "cursor must be reported");
            }
            other => panic!("unexpected message: {other:?}"),
        }

        // ライブ配信: 新規イベントが push される
        daemon
            .state()
            .bus()
            .record(
                &session_id,
                fxg_protocol::events::UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Running,
                    error_message: None,
                },
            )
            .await
            .expect("record");
        match next_ws_message(&mut socket).await {
            ServerWsMessage::EventBatch { events, .. } => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].node_seq, 2);
            }
            other => panic!("unexpected message: {other:?}"),
        }

        // ストリーミング差分はライブ配信される (永続化されない)
        daemon.state().bus().publish_delta(
            session_id.clone(),
            fxg_protocol::common::StreamDeltaPayload::AgentMessageDelta {
                message_id: "m1".to_owned(),
                text_delta: "Hel".to_owned(),
            },
        );
        match next_ws_message(&mut socket).await {
            ServerWsMessage::LiveStreamDelta {
                session_id: id,
                delta,
            } => {
                assert_eq!(id, session_id);
                assert!(matches!(
                    delta,
                    fxg_protocol::common::StreamDeltaPayload::AgentMessageDelta { .. }
                ));
            }
            other => panic!("unexpected message: {other:?}"),
        }

        // Ping → Pong
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&fxg_protocol::client_api::ClientWsMessage::Ping)
                    .unwrap()
                    .into(),
            ))
            .await
            .expect("send ping");
        match next_ws_message(&mut socket).await {
            ServerWsMessage::Pong => {}
            other => panic!("unexpected message: {other:?}"),
        }

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    /// WS から次の `ServerWsMessage` を受信する (タイムアウト付き)。
    async fn next_ws_message<S>(socket: &mut S) -> ServerWsMessage
    where
        S: futures_util::Sink<tokio_tungstenite::tungstenite::Message>
            + futures_util::Stream<
                Item = Result<
                    tokio_tungstenite::tungstenite::Message,
                    tokio_tungstenite::tungstenite::Error,
                >,
            > + Unpin,
    {
        use futures_util::StreamExt;
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("ws message timeout")
            .expect("socket closed")
            .expect("ws error");
        match message {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                serde_json::from_str(text.as_str()).expect("decode server message")
            }
            other => panic!("unexpected ws frame: {other:?}"),
        }
    }

    #[tokio::test]
    async fn serves_sessions_projects_and_kill_switch() {
        let (daemon, _dir) = test_daemon().await;
        let app = router(daemon.state().clone(), daemon.http_addr());
        let token = daemon.state().auth_token();
        let host = host_of(&daemon);

        // 空のセッション一覧
        let response = app
            .clone()
            .oneshot(get(&host, "/api/v1/sessions", Some(&token)))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["sessions"].as_array().expect("sessions").is_empty());

        // プロジェクト一覧 (空)
        let response = app
            .clone()
            .oneshot(get(&host, "/api/v1/projects", Some(&token)))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        // 検索 (空クエリは 400)
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/api/v1/search?q=")
            .header(header::HOST, &host)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // キルスイッチ → 監査ログが残る
        // (`ConnectInfo` は `into_make_service_with_connect_info` が挿入する拡張。
        //  テストでは同等の拡張を手動で注入する)
        let mut request = HttpRequest::builder()
            .method("POST")
            .uri("/api/v1/system/kill-switch")
            .header(header::HOST, &host)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"reason":"test"}"#))
            .expect("request");
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 54321))));
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["notified_nodes"], 1);

        let logs = daemon.state().db().audit_logs(10).await.expect("audit");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "kill_switch");
        assert_eq!(logs[0].details["reason"], "test");

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[test]
    fn extracts_cookie_values() {
        assert_eq!(
            extract_cookie("a=1; fxg_session=abc; b=2", "fxg_session").as_deref(),
            Some("abc")
        );
        assert_eq!(extract_cookie("a=1", "fxg_session"), None);
        assert_eq!(
            extract_cookie("fxg_session=with=equals", "fxg_session").as_deref(),
            Some("with=equals")
        );
    }
}
