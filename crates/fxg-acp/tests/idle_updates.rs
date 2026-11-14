//! Background `session/update` notifications that arrive outside a turn.
//!
//! ACP agents may report back after the prompt response (agy's teamwork, a
//! background task). The driver must display and persist those updates without
//! waiting for the next prompt, and must not merge a report that only arrives
//! with the next turn into that turn's answer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fxg_acp::{
    AcpDriver, AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind, StartSessionRequest,
    StartedSession,
};
use fxg_protocol::common::{PromptInput, SessionStatus};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

/// How long the tests wait for late updates before failing.
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn mock_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg-mock-acp"))
}

fn launch(mode: &str) -> AgentLaunchSpec {
    AgentLaunchSpec {
        agent_id: "mock".to_owned(),
        display_name: "Mock".to_owned(),
        driver_kind: DriverKind::Acp,
        extension: None,
        program: Some(mock_agent_bin()),
        args: vec!["--mode".to_owned(), mode.to_owned()],
        env: Vec::new(),
        acp: Default::default(),
    }
}

fn request(launch: AgentLaunchSpec, cwd: &Path) -> StartSessionRequest {
    StartSessionRequest {
        session_id: "s1".to_owned(),
        title: None,
        cwd: cwd.to_path_buf(),
        launch,
        extra_args: Vec::new(),
        initial_mode: None,
        resume: None,
        fork_from: None,
        fork_before_message_id: None,
        initial_config: Vec::new(),
        acp_parser: None,
    }
}

async fn start(
    mode: &str,
) -> (
    StartedSession,
    mpsc::UnboundedReceiver<DriverEvent>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, rx) = mpsc::unbounded_channel();
    let started = Arc::new(AcpDriver::without_pool())
        .start_session(request(launch(mode), dir.path()), tx)
        .await
        .expect("start");
    (started, rx, dir)
}

/// Collects completed agent messages until the predicate matches.
async fn wait_for_message(
    rx: &mut mpsc::UnboundedReceiver<DriverEvent>,
    expected: &str,
) -> Vec<String> {
    let mut seen = Vec::new();
    let timeout = tokio::time::sleep(TEST_TIMEOUT);
    tokio::pin!(timeout);
    loop {
        tokio::select! {
            () = &mut timeout => panic!("timed out waiting for {expected:?}; messages: {seen:#?}"),
            event = rx.recv() => {
                let Some(event) = event else { panic!("driver closed; messages: {seen:#?}") };
                if let DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                    text,
                    is_complete: true,
                    ..
                }) = &event
                {
                    seen.push(text.clone());
                    if text == expected {
                        return seen;
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn background_updates_are_finalized_while_idle() {
    let (started, mut rx, _dir) = start("late-updates").await;
    started
        .handle
        .send_prompt(PromptInput::text("go"))
        .await
        .expect("prompt");

    // No second prompt: the report that arrives after `end_turn` must be
    // persisted on its own after the idle quiet period.
    let messages = wait_for_message(&mut rx, "late result done").await;
    assert!(
        messages.iter().any(|text| text == "first answer"),
        "missing the turn's answer: {messages:#?}"
    );

    started.handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn queued_report_before_the_user_echo_stays_separate() {
    let (started, mut rx, _dir) = start("late-updates").await;
    started
        .handle
        .send_prompt(PromptInput::text("first"))
        .await
        .expect("prompt");
    // Let the background report settle so the next turn is deterministic.
    wait_for_message(&mut rx, "late result done").await;

    started
        .handle
        .send_prompt(PromptInput::text("second"))
        .await
        .expect("prompt");

    let mut messages = Vec::new();
    let mut running = false;
    let timeout = tokio::time::sleep(TEST_TIMEOUT);
    tokio::pin!(timeout);
    loop {
        tokio::select! {
            () = &mut timeout => panic!("timed out; messages: {messages:#?}"),
            event = rx.recv() => {
                let Some(event) = event else { panic!("driver closed; messages: {messages:#?}") };
                match &event {
                    DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                        status: SessionStatus::Running,
                        ..
                    }) => running = true,
                    DriverEvent::Event(UnifiedEventPayload::StatusChanged {
                        status: SessionStatus::Idle,
                        ..
                    }) if running => break,
                    DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                        text,
                        is_complete: true,
                        ..
                    }) if running => messages.push(text.clone()),
                    DriverEvent::Event(_) | DriverEvent::Delta(_) | DriverEvent::Failed { .. } => {}
                }
            }
        }
    }

    assert!(
        messages.iter().any(|text| text == "queued prelude"),
        "the queued report was not persisted on its own: {messages:#?}"
    );
    assert!(
        messages.iter().any(|text| text == "next answer"),
        "missing the new turn's answer: {messages:#?}"
    );
    assert!(
        !messages
            .iter()
            .any(|text| text.contains("queued prelude") && text.contains("next answer")),
        "the report was merged into the new answer: {messages:#?}"
    );

    started.handle.shutdown().await.expect("shutdown");
}
