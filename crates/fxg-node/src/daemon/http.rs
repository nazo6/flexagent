//! ローカルHTTP/WSサーバー (`127.0.0.1:7860`)。
//!
//! 設計: `docs/03-protocol-and-api.md` §3.0〜§3.3、
//! `docs/01-architecture-and-sync.md` §7.1〜§7.3。
//!
//! ルーター・ハンドラ・検証ロジックは中央サーバー (`fxg server`) と共通の
//! [`fxg_server::api`] に集約されており、本モジュールは
//! [`DaemonState`] (Client API バックエンド実装) を渡してローカル
//! (`127.0.0.1` ループバック限定) で配信するだけである。

use std::net::SocketAddr;

use axum::Router;
use fxg_server::api::{ClientApiOptions, client_router};
use tokio::sync::watch;

use super::DaemonState;

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
///
/// ローカルノードはループバック (`localhost` / `127.0.0.1` / `[::1]`) のみを
/// 許可する (DNS Rebinding / CSWSH 対策。設計: `docs/01` §7.1)。
pub(crate) fn router(daemon: DaemonState, http_addr: SocketAddr) -> Router {
    client_router(daemon, ClientApiOptions::local(http_addr.port()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{DaemonConfig, NodeDaemon};
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::http::{StatusCode, header};
    use axum::response::Response;
    use fxg_protocol::client_api::ServerWsMessage;
    use fxg_protocol::common::ErrorCode;
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
                    status: fxg_protocol::common::SessionStatus::Running,
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

    #[tokio::test]
    async fn client_ws_session_commands_return_structured_errors() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let (daemon, _dir) = test_daemon().await;
        let host = host_of(&daemon);
        let token = daemon.state().auth_token();

        let request = ws_request(&host, &token, &format!("http://{host}"));
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("handshake");

        // 存在しないセッションへの送信は INVALID_STATE の CommandResult になる
        let command = fxg_protocol::client_api::ClientWsMessage::SendPrompt {
            command_id: "c1".to_owned(),
            session_id: "missing-session".to_owned(),
            text: "hello".to_owned(),
            client_source: "web".to_owned(),
        };
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&command).unwrap().into(),
            ))
            .await
            .expect("send prompt");
        match next_ws_message(&mut socket).await {
            ServerWsMessage::CommandResult {
                command_id,
                success,
                code,
                session_id,
                ..
            } => {
                assert_eq!(command_id, "c1");
                assert!(!success);
                assert_eq!(code, Some(ErrorCode::InvalidState));
                assert_eq!(session_id, None);
            }
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
        let text = message.into_text().expect("text message");
        serde_json::from_str(&text).expect("server ws message")
    }
}
