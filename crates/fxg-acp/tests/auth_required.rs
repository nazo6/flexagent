//! `session/new` auth_required handling (Unix only; uses a POSIX shell mock).

#![cfg(unix)]

use std::path::PathBuf;

use fxg_acp::{
    AcpDriver, AgentAuthRequired, AgentDriver, AgentLaunchSpec, DriverKind, StartSessionRequest,
};

/// A minimal ACP agent that advertises a terminal auth method and rejects
/// `session/new` with `-32000` (auth required).
const MOCK_AGENT: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\("[^"]*"\|[0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[{"id":"terminal-login","name":"Terminal login","type":"terminal","args":["--login"]}]}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"authentication required"}}\n' "$id"
      ;;
  esac
done
"#;

#[tokio::test]
async fn session_new_auth_required_returns_advertised_methods() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("mock-acp-agent.sh");
    std::fs::write(&script, MOCK_AGENT).expect("write mock agent");

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
            program: PathBuf::from("/bin/sh"),
            args: vec![script.to_string_lossy().into_owned()],
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
