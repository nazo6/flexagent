#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "the real-driver test only asserts the events it needs"
)]

//! Real OpenCode v2 integration test (ignored by default).
//!
//! Run with:
//! `cargo test -p fxg-acp --test opencode_native_real -- --ignored`
//!
//! It uses the running `opencode serve` service (or the
//! `FXG_OPENCODE_URL` / `FXG_OPENCODE_PASSWORD` override) and creates a
//! session in a temporary directory. It does not touch fxg's DB.

use std::path::PathBuf;
use std::time::Duration;

use fxg_acp::agent::extension::AcpExtensionOptions;
use fxg_acp::driver::{
    AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind, ResumeRequest, StartSessionRequest,
};
use fxg_acp::opencode_native::OpenCodeNativeDriver;
use fxg_protocol::common::{PromptInput, StreamDeltaPayload};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

#[tokio::test]
#[ignore = "requires a running OpenCode v2 service"]
async fn native_driver_streams_a_real_turn() {
    let driver = OpenCodeNativeDriver::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let started = driver
        .start_session(
            StartSessionRequest {
                session_id: "fxg-native-test".to_owned(),
                title: Some("fxg native driver test".to_owned()),
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
        .expect("start session");

    let agent_session_id = started
        .agent_session_id
        .clone()
        .expect("the native driver returns the agent session id");
    assert!(agent_session_id.starts_with("ses_"));
    assert!(started.handle.assigns_message_ids());

    started
        .handle
        .send_prompt(PromptInput {
            text: "Reply with exactly: OK".to_owned(),
            attachments: Vec::new(),
            agent_message_id: Some("msg_fxg_native_1".to_owned()),
        })
        .await
        .expect("send prompt");

    let mut text = String::new();
    let mut saw_running = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await else {
            continue;
        };
        match event {
            DriverEvent::Event(UnifiedEventPayload::StatusChanged { .. }) => {
                saw_running = true;
            }
            DriverEvent::Event(UnifiedEventPayload::AgentMessage {
                text: message,
                is_complete,
                ..
            }) if is_complete => {
                text = message;
                break;
            }
            DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta { text_delta, .. }) => {
                text.push_str(&text_delta)
            }
            DriverEvent::Failed { message } => panic!("driver failed: {message}"),
            _ => {}
        }
    }
    assert!(saw_running, "the execution status must be observed");
    assert!(
        text.to_uppercase().contains("OK"),
        "unexpected agent reply: {text:?}"
    );

    // A native fork produces a new agent session while keeping the history.
    let (fork_tx, _fork_rx) = mpsc::unbounded_channel();
    let forked = driver
        .start_session(
            StartSessionRequest {
                session_id: "fxg-native-fork".to_owned(),
                title: Some("fxg native fork".to_owned()),
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
                fork_from: Some(agent_session_id.clone()),
                fork_before_message_id: None,
                initial_config: Vec::new(),
                acp_parser: None,
            },
            fork_tx,
        )
        .await
        .expect("fork session");
    assert!(forked.context_restored);
    assert_ne!(
        forked.agent_session_id.as_deref(),
        Some(agent_session_id.as_str())
    );
    forked.handle.shutdown().await.expect("fork shutdown");

    // Reverting a real turn must succeed. The message ID is checked by the
    // driver through the native API; a stale ID returns an error.
    started
        .handle
        .revert_to_message("msg_fxg_native_1", false)
        .await
        .expect("revert the turn");

    started.handle.shutdown().await.expect("shutdown");
    let _ = ResumeRequest {
        agent_session_id: None,
        allow_fresh: true,
        capture_history: false,
    };
}
