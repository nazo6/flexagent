//! Native `session/fork` handling (cross-platform Rust mock ACP agent).

use std::path::{Path, PathBuf};

use fxg_acp::{
    AcpDriver, AgentDriver, AgentLaunchSpec, DriverKind, NativeForkUnavailable, StartSessionRequest,
};

fn mock_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg-mock-acp"))
}

fn request(mode: &str, fork_from: Option<&str>, cwd: &Path) -> StartSessionRequest {
    StartSessionRequest {
        session_id: "s1".to_owned(),
        title: None,
        cwd: cwd.to_path_buf(),
        launch: AgentLaunchSpec {
            agent_id: "mock".to_owned(),
            display_name: "Mock".to_owned(),
            driver_kind: DriverKind::Acp,
            extension: None,
            program: Some(mock_agent_bin()),
            args: vec!["--mode".to_owned(), mode.to_owned()],
            env: Vec::new(),
            acp: Default::default(),
        },
        extra_args: Vec::new(),
        initial_mode: None,
        resume: None,
        fork_from: fork_from.map(str::to_owned),
        fork_before_message_id: None,
        initial_config: Vec::new(),
        acp_parser: None,
    }
}

#[tokio::test]
async fn native_fork_attaches_the_forked_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let started = AcpDriver::without_pool()
        .start_session(request("fork", Some("source-session"), dir.path()), events)
        .await
        .expect("native fork");
    assert!(started.context_restored);
}

#[tokio::test]
async fn native_fork_is_reported_when_unsupported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = match AcpDriver::without_pool()
        .start_session(
            request("no-fork", Some("source-session"), dir.path()),
            events,
        )
        .await
    {
        Ok(_) => panic!("fork must be unavailable"),
        Err(err) => err,
    };
    assert!(err.downcast_ref::<NativeForkUnavailable>().is_some());
}
