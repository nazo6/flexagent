//! `session/new` auth_required handling (cross-platform Rust mock ACP agent).

use std::path::PathBuf;

use fxg_acp::{
    AcpDriver, AgentAuthRequired, AgentDriver, AgentLaunchSpec, DriverKind, StartSessionRequest,
};

fn mock_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg-mock-acp"))
}

#[tokio::test]
async fn session_new_auth_required_returns_advertised_methods() {
    let dir = tempfile::tempdir().expect("tempdir");

    let driver = AcpDriver::without_pool();
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let request = StartSessionRequest {
        session_id: "s1".to_owned(),
        title: None,
        cwd: dir.path().to_path_buf(),
        launch: AgentLaunchSpec {
            agent_id: "mock".to_owned(),
            display_name: "Mock".to_owned(),
            driver_kind: DriverKind::Acp,
            program: mock_agent_bin(),
            args: vec!["--mode".to_owned(), "auth-required".to_owned()],
            env: Vec::new(),
        },
        extra_args: Vec::new(),
        initial_mode: None,
        resume: None,
        fork_from: None,
        initial_config: Vec::new(),
    };

    let err = match driver.start_session(request, events).await {
        Ok(_) => panic!("auth required must fail"),
        Err(err) => err,
    };
    let required = err
        .downcast_ref::<AgentAuthRequired>()
        .expect("AgentAuthRequired error");
    assert_eq!(required.0.len(), 1);
    assert_eq!(required.0[0].id, "terminal-login");
    assert_eq!(required.0[0].args, vec!["--login"]);
}
