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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::Json;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use futures_util::StreamExt;
use fxg_agent::driver::{
    AgentDriver, AgentLaunchSpec, DriverEvent, LaunchBackend, ResumeRequest, StartSessionRequest,
};
use fxg_agent::opencode::OpenCodeNativeDriver;
use fxg_protocol::common::{
    ElicitationAction, ElicitationContent, ElicitationValue, PlanEntryStatus, PromptInput,
    StreamDeltaPayload, TurnStopReason,
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
    extra_command: Arc<AtomicBool>,
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
        extra_command: Arc::new(AtomicBool::new(false)),
    };
    let app = Router::new()
        .route("/api/info", get(info))
        .route("/api/agent", get(agents))
        .route("/api/model", get(models))
        .route("/api/model/default", get(default_model))
        .route("/api/command", get(commands))
        .route("/api/session", post(create_session))
        .route("/api/session/active", get(active_sessions))
        .route("/api/session/{session_id}", get(get_session))
        .route("/api/session/{session_id}/permission", get(empty_list))
        .route("/api/session/{session_id}/form", get(empty_list))
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

    // Lightweight capability probe does not create an OpenCode session.
    let launch = AgentLaunchSpec {
        agent_id: "opencode".to_owned(),
        display_name: "OpenCode".to_owned(),
        backend: LaunchBackend::Custom,
        program: Some(PathBuf::from("opencode")),
        args: vec!["serve".to_owned()],
        env: Vec::new(),
    };
    let probed = driver
        .probe_capabilities(&launch, dir.path(), &[])
        .await
        .expect("probe")
        .expect("native probe must return preview");
    assert_eq!(probed.current_mode.as_deref(), Some("build"));
    assert_eq!(probed.available_modes.len(), 2);
    assert!(
        !state
            .calls
            .lock()
            .expect("calls")
            .iter()
            .any(|call| call == "POST /api/session"),
        "probe must not create an OpenCode session"
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let started = driver
        .start_session(
            StartSessionRequest {
                session_id: "fxg-mock".to_owned(),
                title: Some("mock".to_owned()),
                cwd: dir.path().to_path_buf(),
                launch: launch.clone(),
                extra_args: Vec::new(),
                initial_mode: None,
                resume: None,
                fork_from: None,
                fork_before_message_id: None,
                initial_config: Vec::new(),
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
    let mut child_completed = 0;
    let mut child_title = None;
    let mut child_permission = false;
    let mut child_tool = false;
    let mut plan = None;
    let mut turn_ended = None;
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
            DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
                request_id,
                subagent_id,
                ..
            }) => {
                if request_id == "perm_1" {
                    permission = Some(request_id.clone());
                }
                if request_id == "perm_child" {
                    child_permission = subagent_id.as_deref() == Some("ses_child");
                }
            }
            DriverEvent::Event(UnifiedEventPayload::ElicitationRequest {
                elicitation_id, ..
            }) => {
                elicitation = Some(elicitation_id);
            }
            DriverEvent::Event(UnifiedEventPayload::SubagentUpdated {
                child_session_id,
                title,
                status,
                ..
            }) => {
                if child_session_id == "ses_child" {
                    child_announced = true;
                    if status == fxg_protocol::events::SubagentStatus::Completed {
                        child_completed += 1;
                        child_title = title;
                    }
                }
            }
            DriverEvent::Event(UnifiedEventPayload::ToolCall {
                tool_call_id,
                status: fxg_protocol::common::ToolCallStatus::Completed,
                subagent_id,
                ..
            }) if tool_call_id == "tool_child" => {
                child_tool = subagent_id.as_deref() == Some("ses_child");
            }
            DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                text: message,
                subagent_id: Some(child),
                ..
            }) if child == "ses_child" && message == "child result" => {
                child_text = true;
            }
            DriverEvent::Event(UnifiedEventPayload::PlanUpdate {
                entries,
                subagent_id: None,
            }) => {
                plan = Some(entries);
            }
            DriverEvent::Event(UnifiedEventPayload::TurnEnded { reason }) => {
                turn_ended = Some(reason);
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
    assert_eq!(
        child_completed, 1,
        "a completed child must be announced exactly once"
    );
    assert_eq!(child_title.as_deref(), Some("Explore subtask"));
    assert!(
        child_permission,
        "a child permission request must carry the subagent id"
    );
    assert!(child_tool, "a child tool call must carry the subagent id");
    let plan = plan.expect("todowrite must produce a plan update");
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].title, "Write code");
    assert_eq!(plan[0].status, PlanEntryStatus::InProgress);
    assert_eq!(plan[1].status, PlanEntryStatus::Pending);
    assert_eq!(
        turn_ended,
        Some(TurnStopReason::MaxTokens),
        "finish=length must surface as a max-token turn end"
    );

    // A completed turn refreshes the command catalog. An unchanged refresh is
    // suppressed; a new command is announced once.
    state.emit(
        json!({"type": "session.execution.succeeded", "id": "evt_refresh_1",
        "data": {"sessionID": "ses_mock1"}}),
    );
    assert!(
        next_capability_commands(&mut rx, 400).await.is_none(),
        "an unchanged catalog must not emit duplicate capabilities"
    );
    state.extra_command.store(true, Ordering::SeqCst);
    state.emit(
        json!({"type": "session.execution.succeeded", "id": "evt_refresh_2",
        "data": {"sessionID": "ses_mock1"}}),
    );
    let commands = next_capability_commands(&mut rx, 2_000)
        .await
        .expect("a changed command catalog must be announced");
    assert!(commands.iter().any(|command| command == "deploy"));

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

    // A native fork returns a fresh agent session and binds it.
    let (fork_tx, mut fork_rx) = mpsc::unbounded_channel();
    let forked = driver
        .start_session(
            StartSessionRequest {
                session_id: "fxg-mock-fork".to_owned(),
                title: Some("fork".to_owned()),
                cwd: dir.path().to_path_buf(),
                launch: launch.clone(),
                extra_args: Vec::new(),
                initial_mode: None,
                resume: None,
                fork_from: Some("ses_mock1".to_owned()),
                fork_before_message_id: Some("msg_self".to_owned()),
                initial_config: Vec::new(),
            },
            fork_tx,
        )
        .await
        .expect("fork");
    assert!(forked.context_restored);
    assert_ne!(forked.agent_session_id.as_deref(), Some("ses_mock1"));
    let mut fork_bound = None;
    while let Ok(event) = fork_rx.try_recv() {
        if let DriverEvent::Event(UnifiedEventPayload::SessionAgentBound { agent_session_id }) =
            event
        {
            fork_bound = Some(agent_session_id);
        }
    }
    assert_eq!(
        fork_bound.as_deref(),
        forked.agent_session_id.as_deref(),
        "forked session must emit SessionAgentBound"
    );

    // Importing an existing OpenCode session is a new fxg session, so the
    // binding must be emitted even though the agent-side context is restored.
    let (resume_tx, mut resume_rx) = mpsc::unbounded_channel();
    let resumed = driver
        .start_session(
            StartSessionRequest {
                session_id: "fxg-mock-import".to_owned(),
                title: Some("import".to_owned()),
                cwd: dir.path().to_path_buf(),
                launch,
                extra_args: Vec::new(),
                initial_mode: None,
                resume: Some(ResumeRequest {
                    agent_session_id: Some("ses_mock1".to_owned()),
                    allow_fresh: false,
                    capture_history: false,
                }),
                fork_from: None,
                fork_before_message_id: None,
                initial_config: Vec::new(),
            },
            resume_tx,
        )
        .await
        .expect("import");
    assert!(resumed.context_restored);
    let mut import_bound = None;
    while let Ok(event) = resume_rx.try_recv() {
        if let DriverEvent::Event(UnifiedEventPayload::SessionAgentBound { agent_session_id }) =
            event
        {
            import_bound = Some(agent_session_id);
        }
    }
    assert_eq!(
        import_bound.as_deref(),
        Some("ses_mock1"),
        "imported sessions must emit SessionAgentBound"
    );

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

    resumed.handle.shutdown().await.expect("import shutdown");
    forked.handle.shutdown().await.expect("fork shutdown");
    started.handle.shutdown().await.expect("shutdown");
}

async fn next_capability_commands(
    rx: &mut mpsc::UnboundedReceiver<DriverEvent>,
    timeout_ms: u64,
) -> Option<Vec<String>> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(DriverEvent::Event(UnifiedEventPayload::CapabilitiesUpdated {
                available_commands,
                ..
            }))) => {
                return Some(
                    available_commands
                        .into_iter()
                        .map(|command| command.name)
                        .collect(),
                );
            }
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => return None,
        }
    }
}

async fn info() -> Json<Value> {
    Json(json!({"version": "2.2.7"}))
}

async fn agents(uri: axum::http::Uri) -> Json<Value> {
    assert!(
        uri.query()
            .is_some_and(|query| query.contains("location%5Bdirectory%5D=")),
        "agent catalog must be scoped to the session directory: {uri}"
    );
    Json(json!({"data": [
        {"id": "build", "name": "Build", "mode": "primary", "hidden": false},
        {"id": "plan", "name": "Plan", "mode": "primary", "hidden": false},
        {"id": "explore", "name": "Explore", "mode": "subagent", "hidden": true}
    ]}))
}

async fn models(uri: axum::http::Uri) -> Json<Value> {
    assert!(
        uri.query()
            .is_some_and(|query| query.contains("location%5Bdirectory%5D=")),
        "model catalog must be scoped to the session directory: {uri}"
    );
    Json(json!({"data": [
        {"id": "model-a", "modelID": "model-a", "providerID": "mock", "name": "Model A", "enabled": true,
         "variants": [{"id": "low"}, {"id": "high"}]}
    ]}))
}

async fn default_model(uri: axum::http::Uri) -> Json<Value> {
    assert!(
        uri.query()
            .is_some_and(|query| query.contains("location%5Bdirectory%5D=")),
        "default model must be scoped to the session directory: {uri}"
    );
    Json(json!({"data": {
        "id": "model-a", "modelID": "model-a", "providerID": "mock", "name": "Model A",
        "enabled": true, "variants": [{"id": "low"}, {"id": "high"}]
    }}))
}

async fn commands(State(state): State<Mock>, uri: axum::http::Uri) -> Json<Value> {
    assert!(
        uri.query()
            .is_some_and(|query| query.contains("location%5Bdirectory%5D=")),
        "command catalog must be scoped to the session directory: {uri}"
    );
    let mut data = vec![
        json!({"name": "review", "description": "Review changes"}),
        json!({"name": "compact", "description": "Compact"}),
    ];
    if state.extra_command.load(Ordering::SeqCst) {
        data.push(json!({"name": "deploy", "description": "Deploy changes"}));
    }
    Json(json!({ "data": data }))
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

async fn active_sessions() -> Json<Value> {
    Json(json!({"data": {}}))
}

async fn empty_list() -> Json<Value> {
    Json(json!({"data": []}))
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
            json!({"type": "session.tool.input.started", "id": "evt_todo_start",
                   "data": {"sessionID": "ses_mock1", "id": "tool_todo", "name": "todowrite"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.tool.called", "id": "evt_todo_called",
            "data": {"sessionID": "ses_mock1", "id": "tool_todo",
                     "input": {"todos": [
                         {"content": "Write code", "status": "in_progress"},
                         {"content": "Run tests", "status": "pending"}
                     ]}}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.tool.success", "id": "evt_todo_done",
                   "data": {"sessionID": "ses_mock1", "id": "tool_todo", "content": []}})
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
            json!({"type": "session.renamed", "id": "evt_child_renamed",
                   "data": {"sessionID": "ses_child", "title": "Explore subtask"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "permission.asked", "id": "evt_child_perm",
                   "data": {"sessionID": "ses_child", "id": "perm_child", "action": "bash",
                            "resources": ["cargo test"], "message": "Run tests"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.tool.input.started", "id": "evt_child_tool_start",
                   "data": {"sessionID": "ses_child", "id": "tool_child", "name": "bash"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.tool.called", "id": "evt_child_tool_called",
                   "data": {"sessionID": "ses_child", "id": "tool_child",
                            "input": {"command": "cargo test"}}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.tool.success", "id": "evt_child_tool_done",
                   "data": {"sessionID": "ses_child", "id": "tool_child",
                            "content": "ok"}})
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
            json!({"type": "session.idle", "id": "evt_child_idle",
                   "data": {"sessionID": "ses_child"}})
            .to_string(),
        );
        let _ = tx.send(
            json!({"type": "session.step.ended", "id": "evt_step",
                   "data": {"sessionID": "ses_mock1", "finish": "length",
                            "tokens": {"input": 10, "output": 5}}})
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
    Json(body): Json<Value>,
) -> Json<Value> {
    assert_eq!(body["decision"], "once");
    assert!(
        body.get("reply").is_none(),
        "the v2 API expects `decision`, not `reply`"
    );
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
