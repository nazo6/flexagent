#![allow(
    clippy::disallowed_types,
    clippy::let_underscore_must_use,
    clippy::wildcard_enum_match_arm,
    reason = "the mock server speaks raw OpenCode JSON and intentionally ignores events the test does not assert"
)]

//! Contract test for the native OpenCode driver against a mock v2 server.
//!
//! This covers the HTTP + SSE paths that are independent of a real OpenCode
//! installation: session creation, streaming text, permissions, forms, revert,
//! fork and compaction.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::Json;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use futures_util::StreamExt;
use fxg_acp::agent::extension::AcpExtensionOptions;
use fxg_acp::driver::{AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind, StartSessionRequest};
use fxg_acp::opencode_native::OpenCodeNativeDriver;
use fxg_protocol::common::{
    ElicitationAction, ElicitationContent, ElicitationValue, PromptInput, StreamDeltaPayload,
};
use fxg_protocol::events::UnifiedEventPayload;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;

#[derive(Clone)]
struct Mock {
    tx: broadcast::Sender<String>,
    calls: Arc<Mutex<Vec<String>>>,
    next_session: Arc<AtomicUsize>,
}

impl Mock {
    fn record(&self, call: impl Into<String>) {
        self.calls.lock().expect("calls").push(call.into());
    }

    fn emit(&self, value: Value) {
        let _ = self.tx.send(value.to_string());
    }
}

#[tokio::test]
async fn native_driver_uses_the_http_and_sse_contract() {
    let (tx, _) = broadcast::channel::<String>(256);
    let state = Mock {
        tx,
        calls: Arc::new(Mutex::new(Vec::new())),
        next_session: Arc::new(AtomicUsize::new(1)),
    };
    let app = Router::new()
        .route("/api/info", get(info))
        .route("/api/agent", get(agents))
        .route("/api/model", get(models))
        .route("/api/command", get(commands))
        .route("/api/session", post(create_session))
        .route("/api/session/{session_id}", get(get_session))
        .route("/api/session/{session_id}/prompt", post(prompt))
        .route("/api/session/{session_id}/command", post(command))
        .route("/api/session/{session_id}/interrupt", post(ok))
        .route("/api/session/{session_id}/agent", post(ok))
        .route("/api/session/{session_id}/model", post(ok))
        .route("/api/session/{session_id}/compact", post(compact))
        .route(
            "/api/session/{session_id}/permission/{request_id}/reply",
            post(permission_reply),
        )
        .route(
            "/api/session/{session_id}/form/{form_id}/reply",
            post(form_reply),
        )
        .route("/api/session/{session_id}/fork", post(fork))
        .route("/api/session/{session_id}/revert/stage", post(revert_stage))
        .route(
            "/api/session/{session_id}/revert/commit",
            post(revert_commit),
        )
        .route("/api/event", get(events))
        .route("/api/experimental/session/{session_id}/log", get(log))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let driver = OpenCodeNativeDriver::with_server(format!("http://{addr}"), "test");
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let started = driver
        .start_session(
            StartSessionRequest {
                session_id: "fxg-mock".to_owned(),
                title: Some("mock".to_owned()),
                cwd: dir.path().to_path_buf(),
                launch: AgentLaunchSpec {
                    agent_id: "opencode".to_owned(),
                    display_name: "OpenCode".to_owned(),
                    driver_kind: DriverKind::Custom,
                    extension: None,
                    program: Some(PathBuf::from("opencode")),
                    args: vec!["serve".to_owned()],
                    env: Vec::new(),
                    acp: AcpExtensionOptions::default(),
                },
                extra_args: Vec::new(),
                initial_mode: None,
                resume: None,
                fork_from: None,
                fork_before_message_id: None,
                initial_config: Vec::new(),
                acp_parser: None,
            },
            tx,
        )
        .await
        .expect("start");

    assert_eq!(started.agent_session_id.as_deref(), Some("ses_mock1"));
    // Give the SSE pump a moment to attach before emitting the turn.
    tokio::time::sleep(Duration::from_millis(150)).await;

    started
        .handle
        .send_prompt(PromptInput {
            text: "hello".to_owned(),
            attachments: Vec::new(),
            agent_message_id: Some("msg_self".to_owned()),
        })
        .await
        .expect("prompt");

    let mut text = String::new();
    let mut permission = None;
    let mut elicitation = None;
    let mut child_announced = false;
    let mut child_text = false;
    let mut child_completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await
        else {
            continue;
        };
        match event {
            DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta { text_delta, .. }) => {
                text.push_str(&text_delta);
            }
            DriverEvent::Event(UnifiedEventPayload::PermissionRequest { request_id, .. }) => {
                permission = Some(request_id);
            }
            DriverEvent::Event(UnifiedEventPayload::ElicitationRequest {
                elicitation_id, ..
            }) => {
                elicitation = Some(elicitation_id);
            }
            DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                child_session_id,
                status,
                ..
            }) => {
                if child_session_id == "ses_child" {
                    child_announced = true;
                    child_completed = status == fxg_protocol::events::SubagentStatus::Completed;
                }
            }
            DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                text: message,
                subagent_id: Some(child),
                ..
            }) if child == "ses_child" && message == "child result" => {
                child_text = true;
            }
            DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                status: fxg_protocol::common::SessionStatus::Idle,
                ..
            }) => break,
            _ => {}
        }
    }
    assert_eq!(text, "hello from mock");
    assert_eq!(permission.as_deref(), Some("perm_1"));
    assert_eq!(elicitation.as_deref(), Some("frm_1"));
    assert!(child_announced, "the child session must be announced");
    assert!(child_text, "child text must carry the subagent id");
    assert!(child_completed, "the child completion must be reported");

    started
        .handle
        .respond_permission("perm_1".to_owned(), "once".to_owned())
        .await
        .expect("permission reply");
    let mut content = ElicitationContent::new();
    content.insert(
        "choice".to_owned(),
        ElicitationValue::String("a".to_owned()),
    );
    started
        .handle
        .respond_elicitation("frm_1".to_owned(), ElicitationAction::Accept, Some(content))
        .await
        .expect("form reply");
    started
        .handle
        .revert_to_message("msg_self", true)
        .await
        .expect("revert");
    started.handle.compact_context().await.expect("compact");

    // A native fork returns a fresh agent session.
    let (fork_tx, _fork_rx) = mpsc::unbounded_channel();
    let forked = driver
        .start_session(
            StartSessionRequest {
                session_id: "fxg-mock-fork".to_owned(),
                title: Some("fork".to_owned()),
                cwd: dir.path().to_path_buf(),
                launch: AgentLaunchSpec {
                    agent_id: "opencode".to_owned(),
                    display_name: "OpenCode".to_owned(),
                    driver_kind: DriverKind::Custom,
                    extension: None,
                    program: Some(PathBuf::from("opencode")),
                    args: vec!["serve".to_owned()],
                    env: Vec::new(),
                    acp: AcpExtensionOptions::default(),
                },
                extra_args: Vec::new(),
                initial_mode: None,
                resume: None,
                fork_from: Some("ses_mock1".to_owned()),
                fork_before_message_id: Some("msg_self".to_owned()),
                initial_config: Vec::new(),
                acp_parser: None,
            },
            fork_tx,
        )
        .await
        .expect("fork");
    assert!(forked.context_restored);
    assert_ne!(forked.agent_session_id.as_deref(), Some("ses_mock1"));

    let calls = state.calls.lock().expect("calls").clone();
    for expected in [
        "POST /api/session",
        "POST /api/session/ses_mock1/prompt",
        "POST /api/session/ses_mock1/permission/perm_1/reply",
        "POST /api/session/ses_mock1/form/frm_1/reply",
        "POST /api/session/ses_mock1/revert/stage",
        "POST /api/session/ses_mock1/revert/commit",
        "POST /api/session/ses_mock1/compact",
        "POST /api/session/ses_mock1/fork",
    ] {
        assert!(
            calls.iter().any(|call| call == expected),
            "missing call {expected}; got {calls:?}"
        );
    }

    forked.handle.shutdown().await.expect("fork shutdown");
    started.handle.shutdown().await.expect("shutdown");
}

async fn info() -> Json<Value> {
    Json(json!({"version": "2.2.7"}))
}

async fn agents() -> Json<Value> {
    Json(json!({"data": [
        {"id": "build", "name": "Build", "mode": "primary", "hidden": false},
        {"id": "plan", "name": "Plan", "mode": "primary", "hidden": false},
        {"id": "explore", "name": "Explore", "mode": "subagent", "hidden": true}
    ]}))
}

async fn models() -> Json<Value> {
    Json(json!({"data": [
        {"id": "model-a", "modelID": "model-a", "providerID": "mock", "name": "Model A", "enabled": true,
         "variants": [{"id": "low"}, {"id": "high"}]}
    ]}))
}

async fn commands() -> Json<Value> {
    Json(json!({"data": [
        {"name": "review", "description": "Review changes"},
        {"name": "compact", "description": "Compact"}
    ]}))
}

async fn create_session(State(state): State<Mock>) -> Json<Value> {
    state.record("POST /api/session");
    let number = state.next_session.fetch_add(1, Ordering::SeqCst);
    let id = if number == 1 {
        "ses_mock1".to_owned()
    } else {
        format!("ses_fork{number}")
    };
    Json(
        json!({"data": {"id": id, "agent": "build", "model": {"id": "model-a", "providerID": "mock"}}}),
    )
}

async fn get_session() -> Json<Value> {
    Json(json!({"data": {"id": "ses_mock1", "agent": "build",
        "model": {"id": "model-a", "providerID": "mock", "variant": "high"}}}))
}

async fn prompt(State(state): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    state.record("POST /api/session/ses_mock1/prompt");
    assert_eq!(body["id"], "msg_self");
    let tx = state.tx.clone();
    tokio::spawn(async move {
        let _ = tx.send(
            json!({"type": "session.execution.started", "id": "evt_start",
                   "data": {"sessionID": "ses_mock1"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "permission.asked", "id": "evt_perm",
                   "data": {"sessionID": "ses_mock1", "id": "perm_1", "action": "bash",
                            "resources": ["cargo test"], "message": "Run tests"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "form.created", "id": "evt_form",
                   "data": {"form": {"id": "frm_1", "sessionID": "ses_mock1", "title": "Pick",
                            "fields": [{"key": "choice", "type": "string", "title": "Choice", "required": true,
                                        "options": [{"value": "a", "label": "A"}]}]}}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.text.delta", "id": "evt_delta",
                   "data": {"sessionID": "ses_mock1", "assistantMessageID": "msg_1", "ordinal": 0,
                            "delta": "hello from mock"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.text.ended", "id": "evt_text",
                   "durable": {"aggregateID": "ses_mock1", "seq": 1, "version": 1},
                   "data": {"sessionID": "ses_mock1", "assistantMessageID": "msg_1",
                            "ordinal": 0, "text": "hello from mock"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.created", "id": "evt_child_created",
                   "data": {"sessionID": "ses_child", "parentID": "ses_mock1",
                            "title": "Explore"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.text.ended", "id": "evt_child_text",
                   "durable": {"aggregateID": "ses_child", "seq": 1, "version": 1},
                   "data": {"sessionID": "ses_child", "assistantMessageID": "msg_child",
                            "ordinal": 0, "text": "child result"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.execution.succeeded", "id": "evt_child_done",
                   "data": {"sessionID": "ses_child"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.execution.succeeded", "id": "evt_done",
                   "data": {"sessionID": "ses_mock1"}})
            .to_string(),
        );
    });
    Json(json!({"data": {}}))
}

async fn command(State(state): State<Mock>, Json(_body): Json<Value>) -> Json<Value> {
    state.record("POST /api/session/ses_mock1/command");
    Json(json!({"data": {}}))
}

async fn permission_reply(
    State(state): State<Mock>,
    axum::extract::Path((_session, request_id)): axum::extract::Path<(String, String)>,
) -> Json<Value> {
    state.record(format!(
        "POST /api/session/ses_mock1/permission/{request_id}/reply"
    ));
    state.emit(json!({"type": "permission.replied", "id": "evt_prep",
        "data": {"sessionID": "ses_mock1", "requestID": request_id, "reply": "once"}}));
    Json(json!({"data": {}}))
}

async fn form_reply(
    State(state): State<Mock>,
    axum::extract::Path((_session, form_id)): axum::extract::Path<(String, String)>,
    Json(body): Json<Value>,
) -> Json<Value> {
    state.record(format!("POST /api/session/ses_mock1/form/{form_id}/reply"));
    assert_eq!(body["answer"]["choice"], "a");
    state.emit(json!({"type": "form.replied", "id": "evt_frep",
        "data": {"sessionID": "ses_mock1", "id": form_id, "answer": {"choice": "a"}}}));
    Json(json!({"data": {}}))
}

async fn fork(State(state): State<Mock>) -> Json<Value> {
    state.record("POST /api/session/ses_mock1/fork");
    Json(json!({"data": {"id": "ses_fork2"}}))
}

async fn revert_stage(State(state): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    state.record("POST /api/session/ses_mock1/revert/stage");
    assert_eq!(body["messageID"], "msg_self");
    assert_eq!(body["files"], true);
    Json(json!({"data": {}}))
}

async fn revert_commit(State(state): State<Mock>) -> Json<Value> {
    state.record("POST /api/session/ses_mock1/revert/commit");
    Json(json!({"data": {}}))
}

async fn compact(State(state): State<Mock>, Json(_body): Json<Value>) -> Json<Value> {
    state.record("POST /api/session/ses_mock1/compact");
    Json(json!({"data": {}}))
}

async fn ok() -> Json<Value> {
    Json(json!({"data": {}}))
}

async fn events(
    State(state): State<Mock>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(state.tx.subscribe()).filter_map(|message| {
        futures_util::future::ready(match message {
            Ok(data) => Some(Ok(Event::default().data(data))),
            Err(_) => None,
        })
    });
    Sse::new(stream)
}

async fn log() -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let stream = futures_util::stream::iter(vec![Ok::<_, Infallible>(
        Event::default()
            .data(json!({"type": "log.synced", "aggregateID": "ses_mock1"}).to_string()),
    )]);
    Sse::new(stream)
}
