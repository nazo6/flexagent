//! 共通セキュリティミドルウェア (Host / Origin / Token 検証)。
//!
//! 設計: `docs/03-protocol-and-api.md` §3.0 / `docs/01-architecture-and-sync.md`
//! §7.1〜§7.3。ローカルノード (`127.0.0.1:7860`) と中央サーバー (`:8080`) で
//! 同一の検証ロジックを共有し、許可ホスト / オリジンだけを
//! [`ClientApiOptions`](super::ClientApiOptions) で差し替える。
//!
//! 1. **Host ヘッダ検証** (DNS Rebinding 対策): `localhost` / `127.0.0.1` / `[::1]`
//!    と設定許可ホスト以外は `403 Forbidden`
//! 2. **Origin ヘッダ検証** (CSWSH 対策): WebSocket ハンドシェイクで自オリジン
//!    以外は `403 Forbidden` (`Origin` 無しの非ブラウザクライアントは許可)
//! 3. **認証**: `Authorization: Bearer <auth_token>` または
//!    `Cookie: fxg_session=<auth_token>`。未認証は `401 Unauthorized`。
//!    例外は接続先メタ情報 (`GET /api/v1/meta`) のみで、Host / Origin 検証
//!    だけを適用する (未認証の Web UI が接続先種別を判定するために必要)。

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::auth::token_matches;
use super::{ApiError, ClientApiBackend, ClientApiState, ClientInfo, is_websocket_upgrade};

/// Host / Origin / Token 検証を行うミドルウェア。
///
/// 認証を通ったリクエストには [`ClientInfo`] (送信元 IP / UA / 認証主体) を
/// 拡張として挿入する (ハンドラは `Extension<ClientInfo>` で受け取る)。
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

/// 認証を免除するルート (`GET /api/v1/meta`) かどうか。
///
/// Host / Origin 検証は通常どおり適用される。
fn is_auth_exempt(request: &Request) -> bool {
    request.method() == Method::GET && request.uri().path() == super::META_PATH
}

/// 接続元情報から [`ClientInfo`] を組み立てる。
///
/// `ConnectInfo` は `into_make_service_with_connect_info` 経由で拡張に挿入される
/// (テスト等で存在しない場合は `local` 扱い)。
fn client_info(request: &Request) -> ClientInfo {
    let addr = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ConnectInfo(addr)| *addr);
    ClientInfo::from_request(request.headers(), addr)
}

/// Host ヘッダ検証 (DNS Rebinding 対策)。拒否時はエラーレスポンスを返す。
fn verify_host<B: ClientApiBackend>(
    state: &ClientApiState<B>,
    headers: &HeaderMap,
) -> Option<Response> {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
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

/// `localhost` / `127.0.0.1` / `[::1]` (ポート付き含む) かどうか。
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

/// WebSocket ハンドシェイクの Origin 検証 (CSWSH 対策)。
///
/// 非ブラウザクライアント (CLI / native) は `Origin` を送らないため許可する。
fn verify_origin<B: ClientApiBackend>(
    state: &ClientApiState<B>,
    request: &Request,
) -> Option<Response> {
    if !is_websocket_upgrade(request) {
        return None;
    }
    // Origin ヘッダが無い場合 (非ブラウザクライアント) は許可する
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())?;
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

/// 認証 (Bearer ヘッダまたは `fxg_session` Cookie)。拒否時は 401 を返す。
fn verify_auth<B: ClientApiBackend>(
    state: &ClientApiState<B>,
    headers: &HeaderMap,
) -> Option<Response> {
    let expected = state.backend.auth_token();

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

    Some(
        ApiError::new(
            axum::http::StatusCode::UNAUTHORIZED,
            fxg_protocol::common::ErrorCode::Unauthorized,
            "missing or invalid auth token",
        )
        .into_response(),
    )
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
