//! ACP terminals: the command is visible from its start event, output produced
//! after the turn is persisted by a snapshot, and one command can be killed
//! without ending the session.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use fxg_agent::{
    AcpDriver, AcpLaunchOptions, AgentDriver, AgentLaunchSpec, DriverEvent, LaunchBackend,
    StartSessionRequest,
};
use fxg_protocol::common::{PromptInput, StreamDeltaPayload};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

fn mock_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg-mock-acp"))
}

fn launch() -> AgentLaunchSpec {
    AgentLaunchSpec {
        agent_id: "mock".to_owned(),
        display_name: "Mock".to_owned(),
        backend: LaunchBackend::Acp(AcpLaunchOptions::default()),
        program: Some(mock_agent_bin()),
        args: vec!["--mode".to_owned(), "terminal".to_owned()],
        env: Vec::new(),
    }
}

fn decode(data_b64: &str) -> String {
    let bytes = STANDARD
        .decode(data_b64)
        .expect("terminal output is base64");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Waits for the start event and returns the terminal ID.
async fn wait_for_start_event(
    rx: &mut mpsc::UnboundedReceiver<DriverEvent>,
    program: &str,
) -> String {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            match rx.recv().await {
                Some(DriverEvent::Event(UnifiedEventPayload::TerminalOutput {
                    terminal_id,
                    command,
                    data_b64,
                    exit_code,
                    running,
                    ..
                })) => {
                    assert_eq!(data_b64, "", "the start event carries no output");
                    assert_eq!(exit_code, None);
                    assert!(running, "the start event reports a running command");
                    assert!(
                        command.starts_with(program),
                        "unexpected command: {command}"
                    );
                    return terminal_id;
                }
                Some(_) => continue,
                None => panic!("event stream closed before the terminal start event"),
            }
        }
    })
    .await
    .expect("terminal start event")
}

/// Accumulates live deltas until `needle` appears in the output.
async fn wait_for_output(
    rx: &mut mpsc::UnboundedReceiver<DriverEvent>,
    terminal_id: &str,
    needle: &str,
) -> String {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let mut text = String::new();
        while !text.contains(needle) {
            match rx.recv().await {
                Some(DriverEvent::Delta(StreamDeltaPayload::TerminalOutputDelta {
                    terminal_id: id,
                    data_b64,
                })) if id == terminal_id => text.push_str(&decode(&data_b64)),
                Some(_) => continue,
                None => panic!("event stream closed before the output arrived"),
            }
        }
        text
    })
    .await
    .expect("terminal output")
}

/// Waits for the next persisted terminal event and returns
/// `(output, running, exit_code)`.
async fn wait_for_terminal_event(
    rx: &mut mpsc::UnboundedReceiver<DriverEvent>,
    terminal_id: &str,
) -> (String, bool, Option<i32>) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            match rx.recv().await {
                Some(DriverEvent::Event(UnifiedEventPayload::TerminalOutput {
                    terminal_id: id,
                    data_b64,
                    running,
                    exit_code,
                    ..
                })) if id == terminal_id => return (decode(&data_b64), running, exit_code),
                Some(_) => continue,
                None => panic!("event stream closed before the terminal event"),
            }
        }
    })
    .await
    .expect("terminal event")
}

#[tokio::test]
async fn terminal_start_snapshot_and_kill() {
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
                fork_before_message_id: None,
                initial_config: Vec::new(),
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

    // The mock agent starts `printf ready; sleep 60` and ends the turn, so the
    // command outlives the turn.
    let program = if cfg!(windows) { "cmd " } else { "sh " };
    let terminal_id = wait_for_start_event(&mut rx, program).await;
    let streamed = wait_for_output(&mut rx, &terminal_id, "ready").await;
    assert!(streamed.contains("ready"), "streamed output: {streamed:?}");

    // Turn end: the output produced so far is published as one snapshot event.
    started.handle.snapshot_terminals().await;
    let (snapshot, running, exit_code) = wait_for_terminal_event(&mut rx, &terminal_id).await;
    assert!(running, "the snapshot still reports a running command");
    assert_eq!(exit_code, None);
    assert!(
        snapshot.contains("ready"),
        "snapshot output: {snapshot:?} (streamed: {streamed:?})"
    );

    // Killing the command closes the card with the bytes produced after the
    // snapshot only (the UI appends them to the snapshot).
    started
        .handle
        .kill_terminal(&terminal_id)
        .await
        .expect("kill terminal");
    let (final_output, running, _) = wait_for_terminal_event(&mut rx, &terminal_id).await;
    assert!(!running, "the final event reports the command as finished");
    assert!(
        !final_output.contains("ready"),
        "the final event must not repeat the snapshot output: {final_output:?}"
    );

    // An unknown terminal is rejected instead of silently succeeding.
    assert!(
        started
            .handle
            .kill_terminal("missing-terminal")
            .await
            .is_err(),
        "unknown terminals must not be reported as killed"
    );

    started.handle.shutdown().await.expect("shutdown");
}
