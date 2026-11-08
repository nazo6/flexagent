//! Native `session/fork` handling (Unix only; uses a POSIX shell mock).

#![cfg(unix)]

use std::path::PathBuf;

use fxg_acp::{
    AcpDriver, AgentDriver, AgentLaunchSpec, DriverKind, NativeForkUnavailable, StartSessionRequest,
};

/// A minimal ACP agent with `session/fork` + `loadSession`.
const FORK_AGENT: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\("[^"]*"\|[0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"fork":{}}},"authMethods":[]}}\n' "$id"
      ;;
    *'"method":"session/fork"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"forked-session"}}\n' "$id"
      ;;
    *'"method":"session/load"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"forked-session"}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"source-session"}}\n' "$id"
      ;;
  esac
done
"#;

/// A minimal ACP agent without `session/fork`.
const NO_FORK_AGENT: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\("[^"]*"\|[0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"source-session"}}\n' "$id"
      ;;
  esac
done
"#;

fn request(script: &std::path::Path, fork_from: Option<&str>) -> StartSessionRequest {
    StartSessionRequest {
        session_id: "s1".to_owned(),
        title: None,
        cwd: script.parent().expect("script dir").to_path_buf(),
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
        fork_from: fork_from.map(str::to_owned),
    }
}

#[tokio::test]
async fn native_fork_attaches_the_forked_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("fork-agent.sh");
    std::fs::write(&script, FORK_AGENT).expect("write mock");
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let started = AcpDriver::without_pool()
        .start_session(request(&script, Some("source-session")), events)
        .await
        .expect("native fork");
    assert!(started.context_restored);
    // The handle is dropped here; the mock process exits on stdin EOF.
}

#[tokio::test]
async fn native_fork_is_reported_when_unsupported() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("no-fork-agent.sh");
    std::fs::write(&script, NO_FORK_AGENT).expect("write mock");
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = match AcpDriver::without_pool()
        .start_session(request(&script, Some("source-session")), events)
        .await
    {
        Ok(_) => panic!("fork must be unavailable"),
        Err(err) => err,
    };
    assert!(err.downcast_ref::<NativeForkUnavailable>().is_some());
}
