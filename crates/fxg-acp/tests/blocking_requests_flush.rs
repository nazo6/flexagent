//! Streaming thoughts and messages are finalized before blocking requests.
//!
//! When an agent emits `agent_thought_chunk` (or `agent_message_chunk`) and
//! pauses the turn on `elicitation/create` or `session/request_permission`,
//! the preceding completed event must be emitted *before* the request so
//! reloading the UI while waiting for user input does not lose the thought.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fxg_acp::{
    AcpDriver, AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind, StartSessionRequest,
};
use fxg_protocol::common::{
    ElicitationAction, ElicitationContent, ElicitationValue, PromptInput, SessionStatus,
};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn mock_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg-mock-acp"))
}

fn launch() -> AgentLaunchSpec {
    AgentLaunchSpec {
        agent_id: "mock".to_owned(),
        display_name: "Mock".to_owned(),
        driver_kind: DriverKind::Acp,
        extension: None,
        program: Some(mock_agent_bin()),
        args: vec!["--mode".to_owned(), "thought-before-requests".to_owned()],
        env: Vec::new(),
        acp: Default::default(),
    }
}

#[tokio::test]
async fn thoughts_are_finalized_before_elicitation_and_permission_requests() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let started = Arc::new(AcpDriver::without_pool())
        .start_session(
            StartSessionRequest {
                session_id: "s1".to_owned(),
                title: None,
                cwd: dir.path().to_path_buf(),
                launch: launch(),
                extra_args: Vec::new(),
                initial_mode: None,
                resume: None,
                fork_from: None,
                initial_config: Vec::new(),
                acp_parser: None,
            },
            tx,
        )
        .await
        .expect("start");

    started
        .handle
        .send_prompt(PromptInput::text("go"))
        .await
        .expect("prompt");

    // 1. Wait until ElicitationRequest arrives (turn is still paused waiting for user input).
    let mut pre_elicitation_events = Vec::new();
    let elicitation_id = tokio::time::timeout(TEST_TIMEOUT, async {
        while let Some(event) = rx.recv().await {
            if let DriverEvent::Event(payload) = event {
                if let UnifiedEventPayload::ElicitationRequest {
                    ref elicitation_id, ..
                } = payload
                {
                    let id = elicitation_id.clone();
                    pre_elicitation_events.push(payload);
                    return id;
                }
                pre_elicitation_events.push(payload);
            }
        }
        panic!("driver closed before elicitation request");
    })
    .await
    .expect("timed out waiting for elicitation request");

    let thought_idx = pre_elicitation_events
        .iter()
        .position(|payload| {
            matches!(
                payload,
                UnifiedEventPayload::AgentThought {
                    text,
                    is_complete: true,
                    ..
                } if text == "thinking before question"
            )
        })
        .unwrap_or_else(|| {
            panic!("AgentThought was not committed before ElicitationRequest: {pre_elicitation_events:#?}")
        });
    let elicit_idx = pre_elicitation_events.len() - 1;
    assert!(
        thought_idx < elicit_idx,
        "AgentThought must precede ElicitationRequest: {pre_elicitation_events:#?}"
    );

    // Respond to the elicitation so the agent proceeds to the permission request.
    let mut content: ElicitationContent = BTreeMap::new();
    content.insert(
        "choice".to_owned(),
        ElicitationValue::String("a".to_owned()),
    );
    started
        .handle
        .respond_elicitation(elicitation_id, ElicitationAction::Accept, Some(content))
        .await
        .expect("respond elicitation");

    // 2. Wait until PermissionRequest arrives (turn is paused again waiting for approval).
    let mut pre_permission_events = Vec::new();
    let request_id = tokio::time::timeout(TEST_TIMEOUT, async {
        while let Some(event) = rx.recv().await {
            if let DriverEvent::Event(payload) = event {
                if let UnifiedEventPayload::PermissionRequest { ref request_id, .. } = payload {
                    let id = request_id.clone();
                    pre_permission_events.push(payload);
                    return id;
                }
                pre_permission_events.push(payload);
            }
        }
        panic!("driver closed before permission request");
    })
    .await
    .expect("timed out waiting for permission request");

    assert!(
        pre_permission_events.iter().any(|payload| {
            matches!(
                payload,
                UnifiedEventPayload::AgentThought {
                    text,
                    is_complete: true,
                    ..
                } if text == "thinking before permission"
            )
        }),
        "AgentThought was not committed before PermissionRequest: {pre_permission_events:#?}"
    );

    started
        .handle
        .respond_permission(request_id, "allow_once".to_owned())
        .await
        .expect("respond permission");

    // 3. Turn finishes cleanly.
    let mut finished_answer = false;
    let mut went_idle = false;
    tokio::time::timeout(TEST_TIMEOUT, async {
        while let Some(event) = rx.recv().await {
            if let DriverEvent::Event(payload) = event {
                if let UnifiedEventPayload::AgentMessage {
                    text,
                    is_complete: true,
                    ..
                } = &payload
                {
                    if text == "all done" {
                        finished_answer = true;
                    }
                } else if let UnifiedEventPayload::StatusChanged {
                    status: SessionStatus::Idle,
                    ..
                } = &payload
                    && finished_answer
                {
                    went_idle = true;
                    break;
                }
            }
        }
    })
    .await
    .expect("timed out waiting for turn completion");
    assert!(finished_answer && went_idle);

    started.handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn dispose_while_paused_on_elicitation_completes_without_hanging() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let started = Arc::new(AcpDriver::without_pool())
        .start_session(
            StartSessionRequest {
                session_id: "s-dispose".to_owned(),
                title: None,
                cwd: dir.path().to_path_buf(),
                launch: launch(),
                extra_args: Vec::new(),
                initial_mode: None,
                resume: None,
                fork_from: None,
                initial_config: Vec::new(),
                acp_parser: None,
            },
            tx,
        )
        .await
        .expect("start");

    started
        .handle
        .send_prompt(PromptInput::text("go"))
        .await
        .expect("prompt");

    tokio::time::timeout(TEST_TIMEOUT, async {
        while let Some(event) = rx.recv().await {
            if let DriverEvent::Event(UnifiedEventPayload::ElicitationRequest { .. }) = event {
                return;
            }
        }
        panic!("driver closed before elicitation request");
    })
    .await
    .expect("timed out waiting for elicitation request");

    tokio::time::timeout(TEST_TIMEOUT, started.handle.dispose())
        .await
        .expect("dispose timed out while turn was paused on elicitation")
        .expect("dispose");
}
