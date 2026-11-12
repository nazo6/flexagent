//! Subagent event routing (ACP RFD + opencode vendor extension).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fxg_acp::agent::extended::ExtendedAcpDriver;
use fxg_acp::agent::extension::AgentExtension;
use fxg_acp::agent::opencode::OpenCodeExtension;
use fxg_acp::{
    AcpDriver, AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind, StartSessionRequest,
};
use fxg_protocol::common::PromptInput;
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

fn mock_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg-mock-acp"))
}

fn request(mode: &str, mut launch: AgentLaunchSpec, cwd: &Path) -> StartSessionRequest {
    launch.args.extend(["--mode".to_owned(), mode.to_owned()]);
    StartSessionRequest {
        session_id: "s1".to_owned(),
        title: None,
        cwd: cwd.to_path_buf(),
        launch,
        extra_args: Vec::new(),
        initial_mode: None,
        resume: None,
        fork_from: None,
        initial_config: Vec::new(),
        acp_parser: None,
    }
}

fn launch(
    extension: Option<&str>,
    acp: fxg_acp::agent::extension::AcpExtensionOptions,
) -> AgentLaunchSpec {
    AgentLaunchSpec {
        agent_id: extension.unwrap_or("mock").to_owned(),
        display_name: "Mock".to_owned(),
        driver_kind: if extension.is_some() {
            DriverKind::AcpExtended
        } else {
            DriverKind::Acp
        },
        extension: extension.map(str::to_owned),
        program: Some(mock_agent_bin()),
        args: Vec::new(),
        env: Vec::new(),
        acp,
    }
}

async fn run(
    mode: &str,
    driver: Arc<dyn AgentDriver>,
    launch: AgentLaunchSpec,
) -> Vec<DriverEvent> {
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let started = driver
        .start_session(request(mode, launch, dir.path()), tx)
        .await
        .expect("start");
    started
        .handle
        .send_prompt(PromptInput::text("go"))
        .await
        .expect("prompt");

    let mut events = Vec::new();
    let mut running = false;
    let timeout = tokio::time::sleep(Duration::from_secs(15));
    tokio::pin!(timeout);
    loop {
        tokio::select! {
            () = &mut timeout => panic!("timed out; events: {events:#?}"),
            event = rx.recv() => {
                let Some(event) = event else { break };
                if let DriverEvent::Event(UnifiedEventPayload::StatusChanged { status, .. }) =
                    &event
                {
                    if *status == fxg_protocol::common::SessionStatus::Running {
                        running = true;
                    } else if *status == fxg_protocol::common::SessionStatus::Idle && running {
                        events.push(event);
                        break;
                    }
                }
                events.push(event);
            }
        }
    }
    started.handle.shutdown().await.expect("shutdown");
    events
}

fn subagent_updates(events: &[DriverEvent]) -> Vec<&UnifiedEventPayload> {
    events
        .iter()
        .filter_map(|event| match event {
            DriverEvent::Event(payload @ UnifiedEventPayload::SubagentUpdated { .. }) => {
                Some(payload)
            }
            DriverEvent::Event(_) | DriverEvent::Delta(_) | DriverEvent::Failed { .. } => None,
        })
        .collect()
}

#[test]
fn mock_subagent_update_payload_parses() {
    let value = serde_json::json!({
        "sessionId": "root-session",
        "update": {
            "sessionUpdate": "subagent_update",
            "sessionId": "child-rfd",
            "title": "Explore",
            "description": "RFD child",
            "state": { "state": "running" }
        }
    });
    let parsed =
        serde_json::from_value::<agent_client_protocol::v1::schema::SessionNotification>(value);
    assert!(parsed.is_ok(), "parse failed: {parsed:#?}");
}

#[tokio::test]
async fn acp_child_updates_are_tagged_and_announced() {
    let events = run(
        "subagents",
        Arc::new(AcpDriver::without_pool()),
        launch(None, Default::default()),
    )
    .await;

    let updates = subagent_updates(&events);
    assert!(
        updates.iter().any(|payload| matches!(
            payload,
            UnifiedEventPayload::SubagentUpdated {
                child_session_id,
                depth: 1,
                title: Some(title),
                ..
            } if child_session_id == "child-rfd" && title == "Explore"
        )),
        "missing child announcement: {updates:#?}"
    );

    assert!(
        events.iter().any(|event| matches!(
            event,
            DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                subagent_id: Some(child),
                text,
                ..
            }) if child == "child-rfd" && text == "hello from the child"
        )),
        "child message was not tagged: {events:#?}"
    );

    assert!(
        events.iter().any(|event| matches!(
            event,
            DriverEvent::Event(UnifiedEventPayload::ToolCall {
                subagent_id: Some(child),
                tool_call_id,
                ..
            }) if child == "child-rfd" && tool_call_id == "child-rfd-t1"
        )),
        "child tool call was not tagged: {events:#?}"
    );
}

#[tokio::test]
async fn opencode_child_notifications_are_parsed() {
    let extension = Arc::new(OpenCodeExtension);
    let options = extension.acp_options();
    let driver = Arc::new(ExtendedAcpDriver::new(
        Arc::new(AcpDriver::without_pool()),
        extension,
    ));
    let events = run(
        "opencode-subagents",
        driver,
        launch(Some("opencode"), options),
    )
    .await;

    let updates = subagent_updates(&events);
    assert!(
        updates.iter().any(|payload| matches!(
            payload,
            UnifiedEventPayload::SubagentUpdated {
                child_session_id,
                depth: 1,
                title: Some(title),
                ..
            } if child_session_id == "child-open" && title == "Explore"
        )),
        "missing opencode child announcement: {updates:#?}"
    );

    assert!(
        events.iter().any(|event| matches!(
            event,
            DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                subagent_id: Some(child),
                text,
                ..
            }) if child == "child-open" && text == "child says hi"
        )),
        "opencode child message was not parsed: {events:#?}"
    );

    // The vendor resends the same status with each update; unchanged state must
    // not produce another announcement.
    let announcements = updates
        .iter()
        .filter(|payload| {
            matches!(
                payload,
                UnifiedEventPayload::SubagentUpdated { child_session_id, .. }
                    if child_session_id == "child-open"
            )
        })
        .count();
    assert_eq!(announcements, 1, "duplicate announcements: {updates:#?}");
}
