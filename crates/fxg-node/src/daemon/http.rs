//! Local HTTP/WS server (`127.0.0.1:7860`).
//!
//! Design: `docs/03-protocol-and-api.md` §3.0-§3.3,
//! `docs/01-architecture-and-sync.md` §7.1-§7.3.
//!
//! The router, handlers and validation logic are shared with the hub
//! (`fxg hub start`) in [`fxg_hub::api`]; this module only supplies the
//! [`DaemonState`] (the Client API backend implementation) and serves it locally
//! (loopback `127.0.0.1` only).

#![allow(
    clippy::disallowed_types,
    reason = "tests parse raw HTTP JSON; see docs/06-type-safety.md"
)]

use std::net::SocketAddr;

use axum::Router;
use fxg_hub::api::{ClientApiOptions, client_router};
use tokio::sync::watch;

use super::DaemonState;

/// Starts the HTTP server (runs until a shutdown request arrives).
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
        if let Err(err) = shutdown.changed().await {
            tracing::debug!("shutdown sender dropped; stopping local http server: {err}");
        }
    });
    tracing::info!(addr = %http_addr, "local http server listening");
    if let Err(err) = server.await {
        tracing::error!("local http server stopped: {err}");
    }
}

/// Builds the router (`pub(crate)` so tests can use it directly).
///
/// Local nodes only allow loopback (`localhost` / `127.0.0.1` / `[::1]`)
/// (DNS rebinding / CSWSH mitigation; design: `docs/01` §7.1).
pub(crate) fn router(daemon: DaemonState, http_addr: SocketAddr) -> Router {
    client_router(daemon, ClientApiOptions::local(http_addr.port()))
}

#[cfg(test)]
mod tests {

    fn set_cookie(response: &axum::response::Response) -> &str {
        match response
            .headers()
            .get(header::SET_COOKIE)
            .expect("Set-Cookie")
            .to_str()
        {
            Ok(value) => value,
            Err(_) => panic!("Set-Cookie is not ASCII"),
        }
    }

    use super::*;
    use crate::daemon::{DaemonConfig, NodeDaemon};
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::http::{StatusCode, header};
    use axum::response::Response;
    use fxg_acp::AgentSessionInfo;
    use fxg_protocol::client_api::HubWsMessage;
    use fxg_protocol::common::ErrorCode;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_daemon() -> (NodeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = DaemonConfig::new(dir.path().to_path_buf(), "http-node", "HTTP Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        let daemon = NodeDaemon::start(config).await.expect("start");
        (daemon, dir)
    }

    /// `Host` header value for tests (uses the actually bound port).
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

        // Unauthenticated → 401
        let response = app
            .clone()
            .oneshot(get(&host, "/api/v1/system/info", None))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "UNAUTHORIZED");

        // Invalid Host → 403 (DNS rebinding mitigation)
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

        // Wrong token → 401
        let response = app
            .clone()
            .oneshot(get(&host, "/api/v1/system/info", Some("deadbeef")))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // Correct token (Bearer) → 200
        let response = app
            .clone()
            .oneshot(get(&host, "/api/v1/system/info", Some(&token)))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["role"], "local_node");
        assert_eq!(body["version"], crate::daemon::VERSION);
        assert_eq!(body["hub_link"]["state"], "standalone");

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

    fn post_json(host: &str, uri: &str, token: Option<&str>, body: &str) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder()
            .method("POST")
            .uri(uri)
            .header(header::HOST, host)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::from(body.to_owned())).expect("request")
    }

    #[tokio::test]
    async fn login_issues_session_cookie_and_logout_clears_it() {
        let (daemon, _dir) = test_daemon().await;
        let app = router(daemon.state().clone(), daemon.http_addr());
        let token = daemon.state().auth_token();
        let host = host_of(&daemon);

        // Login with the correct token → 204 + HttpOnly / SameSite=Strict cookie
        let login_body = format!("{{\"token\":\"{token}\"}}");
        let response = app
            .clone()
            .oneshot(post_json(
                &host,
                "/api/v1/auth/login",
                Some(&token),
                &login_body,
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cookie = set_cookie(&response);
        assert!(cookie.contains(&format!("fxg_session={token}")), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Strict"), "{cookie}");

        // Mismatched body token → 401 (rejected even with a valid Bearer header)
        let response = app
            .clone()
            .oneshot(post_json(
                &host,
                "/api/v1/auth/login",
                Some(&token),
                "{\"token\":\"deadbeef\"}",
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // Logout → 204 + expiring cookie (Max-Age=0)
        let response = app
            .clone()
            .oneshot(post_json(&host, "/api/v1/auth/logout", Some(&token), "{}"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cookie = set_cookie(&response);
        assert!(cookie.contains("fxg_session=;"), "{cookie}");
        assert!(cookie.contains("Max-Age=0"), "{cookie}");

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

        // WS handshakes from external origins are rejected with 403 (CSWSH mitigation)
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

    /// Builds a request for a WS handshake.
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

        // Persist one event before subscribing (to verify replay)
        let session_id = fxg_protocol::util::uuid_v7();
        daemon
            .state()
            .bus()
            .create_session(
                &session_id,
                fxg_protocol::events::UnifiedEventPayload::SessionCreated {
                    node_id: daemon.state().node_id().into(),
                    project_id: "github.com/nazo6/flexagent".into(),
                    project_name: "flexagent".to_owned(),
                    local_path: "/tmp/repo".to_owned(),
                    git_branch: Some("main".to_owned()),
                    is_worktree: false,
                    agent_id: "opencode".to_owned(),
                    parent_session_id: None,
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

        // Subscribe → replay
        let subscribe = fxg_protocol::client_api::ClientWsMessage::Subscribe {
            since_cursor: None,
            focused_session_id: Some(session_id.clone().into()),
        };
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&subscribe).unwrap().into(),
            ))
            .await
            .expect("send subscribe");

        let message = next_ws_message(&mut socket).await;
        match message {
            HubWsMessage::EventBatch { events, cursor } => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].session_id.as_str(), session_id);
                assert_eq!(events[0].node_seq, 1);
                assert!(cursor > 0, "cursor must be reported");
            }
            other => panic!("unexpected message: {other:?}"),
        }

        // Live streaming: new events are pushed
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
            HubWsMessage::EventBatch { events, .. } => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].node_seq, 2);
            }
            other => panic!("unexpected message: {other:?}"),
        }

        // Streaming deltas are delivered live (not persisted)
        daemon.state().bus().publish_delta(
            session_id.clone(),
            fxg_protocol::common::StreamDeltaPayload::AgentMessageDelta {
                message_id: "m1".to_owned(),
                text_delta: "Hel".to_owned(),
                subagent_id: None,
            },
        );
        match next_ws_message(&mut socket).await {
            HubWsMessage::LiveStreamDelta {
                session_id: id,
                delta,
            } => {
                assert_eq!(id.as_str(), session_id);
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
            HubWsMessage::Pong => {}
            other => panic!("unexpected message: {other:?}"),
        }

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn client_ws_replays_from_zero_when_cursor_is_ahead_of_store() {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let (daemon, _dir) = test_daemon().await;
        let host = host_of(&daemon);
        let token = daemon.state().auth_token();

        // Persist one event before subscribing (the cursor becomes 1 or later)
        let session_id = fxg_protocol::util::uuid_v7();
        daemon
            .state()
            .bus()
            .create_session(
                &session_id,
                fxg_protocol::events::UnifiedEventPayload::SessionCreated {
                    node_id: daemon.state().node_id().into(),
                    project_id: "github.com/nazo6/flexagent".into(),
                    project_name: "flexagent".to_owned(),
                    local_path: "/tmp/repo".to_owned(),
                    git_branch: Some("main".to_owned()),
                    is_worktree: false,
                    agent_id: "opencode".to_owned(),
                    parent_session_id: None,
                    title: "New Session".to_owned(),
                },
            )
            .await
            .expect("create session");

        let request = ws_request(&host, &token, &format!("http://{host}"));
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("handshake");

        // Passing a stale cursor left over from a DB rebuild/reset (beyond the end of
        // the store) replays everything from 0 instead of skipping
        let subscribe = fxg_protocol::client_api::ClientWsMessage::Subscribe {
            since_cursor: Some(9_999),
            focused_session_id: None,
        };
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&subscribe).unwrap().into(),
            ))
            .await
            .expect("send subscribe");

        match next_ws_message(&mut socket).await {
            HubWsMessage::EventBatch { events, cursor } => {
                assert_eq!(events.len(), 1, "stale cursor must not skip events");
                assert_eq!(events[0].session_id.as_str(), session_id);
                assert_eq!(events[0].node_seq, 1);
                assert!(cursor <= 9_999, "cursor must be rewound to the store");
            }
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

        // Sending to a nonexistent session returns an INVALID_STATE CommandResult
        let command = fxg_protocol::client_api::ClientWsMessage::SendPrompt {
            command_id: "c1".into(),
            session_id: "missing-session".into(),
            input: fxg_protocol::common::PromptInput::text("hello"),
            client_source: fxg_protocol::common::ClientSource::Web,
        };
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&command).unwrap().into(),
            ))
            .await
            .expect("send prompt");
        match next_ws_message(&mut socket).await {
            HubWsMessage::CommandResult {
                command_id,
                success,
                error,
                session_id,
            } => {
                assert_eq!(command_id.as_str(), "c1");
                assert!(!success);
                assert_eq!(error.map(|err| err.code), Some(ErrorCode::InvalidState));
                assert_eq!(session_id, None);
            }
            other => panic!("unexpected message: {other:?}"),
        }

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    /// Starts a daemon with the mock agent driver for session REST tests.
    async fn test_session_daemon() -> (NodeDaemon, crate::testutil::MockAgent, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::testutil::write_mock_agent_config(dir.path()).await;
        let mut config =
            DaemonConfig::new(dir.path().to_path_buf(), "session-node", "Session Node");
        config.listen_addr = "127.0.0.1:0".to_owned();
        let mock = crate::testutil::MockAgent::default();
        let daemon = NodeDaemon::start_with_driver_factory(config, mock.factory())
            .await
            .expect("start");
        (daemon, mock, dir)
    }

    async fn create_session_via_rest(
        app: &Router,
        host: &str,
        token: &str,
        workspace: &std::path::Path,
    ) -> String {
        let body = serde_json::json!({
            "command_id": "create-1",
            "project_id": "local:http-test",
            "node_id": "session-node",
            "local_path": workspace.to_string_lossy(),
            "agent_id": "mock",
        });
        let response = app
            .clone()
            .oneshot(post_json(
                host,
                "/api/v1/sessions",
                Some(token),
                &body.to_string(),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        body["session_id"].as_str().expect("session id").to_owned()
    }

    #[tokio::test]
    async fn rest_session_detail_and_event_tail() {
        let (daemon, _dir) = test_daemon().await;
        let app = router(daemon.state().clone(), daemon.http_addr());
        let host = host_of(&daemon);
        let token = daemon.state().auth_token();
        let session_id = fxg_protocol::util::uuid_v7();
        daemon
            .state()
            .bus()
            .create_session(
                &session_id,
                fxg_protocol::events::UnifiedEventPayload::SessionCreated {
                    node_id: daemon.state().node_id().into(),
                    project_id: "github.com/nazo6/flexagent".into(),
                    project_name: "flexagent".to_owned(),
                    local_path: "/tmp/repo".to_owned(),
                    git_branch: Some("main".to_owned()),
                    is_worktree: false,
                    agent_id: "mock".to_owned(),
                    parent_session_id: None,
                    title: "New Session".to_owned(),
                },
            )
            .await
            .expect("create session");
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
            .expect("record status");

        let response = app
            .clone()
            .oneshot(get(
                &host,
                &format!("/api/v1/sessions/{session_id}?recent_events=1"),
                Some(&token),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["event_count"], 2);
        assert_eq!(body["recent_events"].as_array().expect("events").len(), 1);
        assert!(
            body["pending_permissions"]
                .as_array()
                .expect("pending")
                .is_empty()
        );
        assert!(
            body["pending_elicitations"]
                .as_array()
                .expect("pending")
                .is_empty()
        );

        let response = app
            .clone()
            .oneshot(get(
                &host,
                &format!("/api/v1/sessions/{session_id}/events?after_cursor=0"),
                Some(&token),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["events"].as_array().expect("events").len(), 2);
        let cursor = body["cursor"].as_u64().expect("cursor");

        let response = app
            .clone()
            .oneshot(get(
                &host,
                &format!("/api/v1/sessions/{session_id}/events?before_cursor={cursor}&limit=1"),
                Some(&token),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["events"].as_array().expect("events").len(), 1);

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn rest_prompt_control_and_import_return_command_results() {
        let (daemon, mock, dir) = test_session_daemon().await;
        let app = router(daemon.state().clone(), daemon.http_addr());
        let host = host_of(&daemon);
        let token = daemon.state().auth_token();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");

        let session_id = create_session_via_rest(&app, &host, &token, &workspace).await;

        let prompt = serde_json::json!({
            "command_id": "prompt-1",
            "input": {"text": "hello"},
            "client_source": "cli",
        });
        let response = app
            .clone()
            .oneshot(post_json(
                &host,
                &format!("/api/v1/sessions/{session_id}/prompt"),
                Some(&token),
                &prompt.to_string(),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["success"], true);
        assert_eq!(mock.prompts(), vec!["hello".to_owned()]);

        let control = serde_json::json!({
            "command_id": "control-1",
            "action": {"action": "cancel"},
        });
        let response = app
            .clone()
            .oneshot(post_json(
                &host,
                &format!("/api/v1/sessions/{session_id}/control"),
                Some(&token),
                &control.to_string(),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["success"], true);

        let set_mode = serde_json::json!({
            "command_id": "control-mode-1",
            "action": {"action": "set_mode", "mode_id": "plan"},
        });
        let response = app
            .clone()
            .oneshot(post_json(
                &host,
                &format!("/api/v1/sessions/{session_id}/control"),
                Some(&token),
                &set_mode.to_string(),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["success"], true);
        assert_eq!(mock.modes(), vec!["plan".to_owned()]);

        mock.set_resume_supported(true);
        let import = serde_json::json!({
            "command_id": "import-1",
            "agent_id": "mock",
            "agent_session_id": "agent-9",
            "cwd": workspace.to_string_lossy(),
            "title": "Imported",
        });
        let response = app
            .clone()
            .oneshot(post_json(
                &host,
                "/api/v1/sessions/import",
                Some(&token),
                &import.to_string(),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["session_id"].as_str().is_some());
        assert!(mock.starts().iter().any(|start| {
            start
                .resume
                .as_ref()
                .is_some_and(|resume| resume.agent_session_id.as_deref() == Some("agent-9"))
        }));

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn rest_external_sessions_lists_agent_sessions() {
        let (daemon, mock, dir) = test_session_daemon().await;
        mock.set_external_sessions(
            vec![AgentSessionInfo {
                session_id: "external-1".to_owned(),
                cwd: dir.path().to_path_buf(),
                title: Some("External".to_owned()),
                updated_at: Some("2026-11-12T10:00:00Z".to_owned()),
            }],
            true,
        );
        let app = router(daemon.state().clone(), daemon.http_addr());
        let host = host_of(&daemon);
        let token = daemon.state().auth_token();

        let response = app
            .oneshot(get(
                &host,
                "/api/v1/sessions/external?agent=mock&force=true",
                Some(&token),
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let sessions = body["sessions"].as_array().expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["agent_id"], "mock");
        assert_eq!(sessions[0]["agent_session_id"], "external-1");
        assert_eq!(sessions[0]["title"], "External");
        let agents = body["agents"].as_array().expect("agents");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["supports_list"], true);
        assert_eq!(agents[0]["supports_history"], true);

        daemon.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    #[tokio::test]
    async fn rest_shutdown_stops_the_local_node() {
        let (daemon, _dir) = test_daemon().await;
        let app = router(daemon.state().clone(), daemon.http_addr());
        let host = host_of(&daemon);
        let token = daemon.state().auth_token();

        let response = app
            .oneshot(post_json(
                &host,
                "/api/v1/system/shutdown",
                Some(&token),
                "{}",
            ))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["ok"], true);

        tokio::time::timeout(std::time::Duration::from_secs(5), daemon.wait())
            .await
            .expect("stop");
    }

    /// Receives the next `HubWsMessage` from the WS (with a timeout).
    async fn next_ws_message<S>(socket: &mut S) -> HubWsMessage
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
