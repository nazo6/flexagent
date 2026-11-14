//! External session discovery (`session/list`) and history import (`session/load`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use fxg_acp::{
    AcpDriver, AgentDriver, AgentLaunchSpec, DriverEvent, DriverKind, ExternalSessionsUnsupported,
    HistoryCaptureUnavailable, ResumeRequest, StartSessionRequest,
};
use fxg_protocol::common::ClientSource;
use fxg_protocol::events::UnifiedEventPayload;

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

fn request(cwd: &Path, resume: ResumeRequest, mode: &str) -> StartSessionRequest {
    StartSessionRequest {
        session_id: "s1".to_owned(),
        title: Some("Imported".to_owned()),
        cwd: cwd.to_path_buf(),
        launch: launch(mode),
        extra_args: Vec::new(),
        initial_mode: None,
        resume: Some(resume),
        fork_from: None,
        fork_before_message_id: None,
        initial_config: Vec::new(),
        acp_parser: None,
    }
}

#[tokio::test]
async fn lists_external_sessions_with_cursor_pagination() {
    let dir = tempfile::tempdir().expect("tempdir");
    let list = AcpDriver::without_pool()
        .list_sessions(&launch("list"), dir.path())
        .await
        .expect("session/list");

    assert!(list.supports_history);
    assert_eq!(list.sessions.len(), 2);
    assert_eq!(list.sessions[0].session_id, "external-1");
    assert_eq!(
        list.sessions[0].title.as_deref(),
        Some("First external session")
    );
    assert_eq!(
        list.sessions[0].updated_at.as_deref(),
        Some("2026-11-12T10:00:00Z")
    );
    assert_eq!(list.sessions[1].session_id, "external-2");
}

#[tokio::test]
async fn reports_list_as_unsupported_when_not_advertised() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = AcpDriver::without_pool()
        .list_sessions(&launch("no-fork"), dir.path())
        .await
        .expect_err("session/list is unsupported");
    assert!(err.downcast_ref::<ExternalSessionsUnsupported>().is_some());
}

#[tokio::test]
async fn import_captures_the_replayed_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let started = AcpDriver::without_pool()
        .start_session(
            request(
                dir.path(),
                ResumeRequest {
                    agent_session_id: Some("external-1".to_owned()),
                    allow_fresh: false,
                    capture_history: true,
                },
                "list",
            ),
            tx,
        )
        .await
        .expect("import session");
    assert!(started.context_restored);

    let mut user = None;
    let mut agent = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while user.is_none() || agent.is_none() {
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("replay events")
            .expect("channel");
        match event {
            DriverEvent::Event(UnifiedEventPayload::UserMessage {
                text,
                client_source,
                ..
            }) => user = Some((text, client_source)),
            DriverEvent::Event(UnifiedEventPayload::AgentMessage { text, .. }) => {
                agent = Some(text);
            }
            DriverEvent::Event(_) | DriverEvent::Delta(_) | DriverEvent::Failed { .. } => {}
        }
    }

    let (user_text, user_source) = user.expect("user message");
    assert_eq!(user_text, "hello from history");
    assert_eq!(user_source, ClientSource::Import);
    assert_eq!(agent.expect("agent message"), "reply from history");
    started.handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn import_fails_when_the_agent_replays_no_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = AcpDriver::without_pool()
        .start_session(
            request(
                dir.path(),
                ResumeRequest {
                    agent_session_id: Some("external-1".to_owned()),
                    allow_fresh: false,
                    capture_history: true,
                },
                "empty-history",
            ),
            tx,
        )
        .await;
    let err = match result {
        Ok(_) => panic!("an import without history must fail"),
        Err(err) => err,
    };
    assert!(
        err.downcast_ref::<HistoryCaptureUnavailable>().is_some(),
        "unexpected error: {err:#}"
    );
}
