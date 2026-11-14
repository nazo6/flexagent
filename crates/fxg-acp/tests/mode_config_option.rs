//! Session mode selection through ACP Session Config Options.
//!
//! ACP superseded Session Modes with Session Config Options, and agents such as
//! OpenCode only report the mode selector as `configOptions[category = mode]`.
//! This reproduces that shape with the mock agent: `session/set_mode` is
//! rejected, so the driver must derive the mode list from the config option and
//! route mode changes through `session/set_config_option`.

use std::path::PathBuf;
use std::time::Duration;

use fxg_acp::{
    AcpDriver, AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind, StartSessionRequest,
};
use fxg_protocol::events::UnifiedEventPayload;
use tokio::sync::mpsc;

fn mock_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg-mock-acp"))
}

/// Reads driver events until the next `CapabilitiesUpdated` payload.
async fn next_capabilities(rx: &mut mpsc::UnboundedReceiver<DriverEvent>) -> UnifiedEventPayload {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("capabilities event timed out")
            .expect("event channel closed");
        if let DriverEvent::Event(payload @ UnifiedEventPayload::CapabilitiesUpdated { .. }) = event
        {
            return payload;
        }
    }
}

fn capabilities(
    payload: &UnifiedEventPayload,
) -> (
    &Option<String>,
    &[fxg_protocol::common::ModeInfo],
    &[fxg_protocol::common::ConfigOptionInfo],
) {
    let UnifiedEventPayload::CapabilitiesUpdated {
        current_mode,
        available_modes,
        config_options,
        ..
    } = payload
    else {
        panic!("unexpected payload: {payload:?}");
    };
    (current_mode, available_modes, config_options)
}

#[tokio::test]
async fn modes_come_from_config_options_and_set_mode_uses_set_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let driver = AcpDriver::without_pool();
    let (events, mut rx) = mpsc::unbounded_channel();
    let request = StartSessionRequest {
        session_id: "s1".to_owned(),
        title: None,
        cwd: dir.path().to_path_buf(),
        launch: AgentLaunchSpec {
            agent_id: "mock".to_owned(),
            display_name: "Mock".to_owned(),
            driver_kind: DriverKind::Acp,
            extension: None,
            program: Some(mock_agent_bin()),
            args: vec!["--mode".to_owned(), "modes-as-config".to_owned()],
            env: Vec::new(),
            acp: Default::default(),
        },
        extra_args: Vec::new(),
        initial_mode: Some("plan".to_owned()),
        resume: None,
        fork_from: None,
        fork_before_message_id: None,
        initial_config: Vec::new(),
        acp_parser: None,
    };

    // The initial mode must go through `set_config_option`: the mock rejects
    // `session/set_mode`, which would leave the mode at the agent default.
    let started = driver
        .start_session(request, events)
        .await
        .expect("session start");
    let payload = next_capabilities(&mut rx).await;
    let (current_mode, available_modes, config_options) = capabilities(&payload);
    assert_eq!(current_mode.as_deref(), Some("plan"));
    assert_eq!(
        available_modes
            .iter()
            .map(|mode| (mode.mode_id.as_str(), mode.name.as_str()))
            .collect::<Vec<_>>(),
        vec![("build", "Build"), ("plan", "Plan")]
    );
    assert_eq!(
        available_modes[0].description.as_deref(),
        Some("The default agent.")
    );
    assert_eq!(
        config_options
            .iter()
            .find(|option| option.key == "mode")
            .map(|option| &option.current_value),
        Some(&fxg_protocol::common::ConfigOptionValue::Choice(
            fxg_protocol::common::ConfigOptionChoice {
                value: "plan".to_owned(),
                name: "Plan".to_owned(),
                description: Some("Read-only planning.".to_owned()),
            }
        ))
    );
    assert_eq!(
        config_options
            .iter()
            .find(|option| option.key == "model")
            .map(|option| &option.current_value),
        Some(&fxg_protocol::common::ConfigOptionValue::Choice(
            fxg_protocol::common::ConfigOptionChoice {
                value: "m1".to_owned(),
                name: "Model 1".to_owned(),
                description: None,
            }
        ))
    );

    // An in-session mode change also uses the config option.
    started
        .handle
        .set_mode("build".to_owned())
        .await
        .expect("set_mode routed through set_config_option");
    let payload = next_capabilities(&mut rx).await;
    let (current_mode, available_modes, _) = capabilities(&payload);
    assert_eq!(current_mode.as_deref(), Some("build"));
    assert_eq!(available_modes.len(), 2);

    started.handle.dispose().await.expect("dispose");
}
