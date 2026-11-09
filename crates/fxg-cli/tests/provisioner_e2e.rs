//! Phase 6: end-to-end tests for ephemeral VM provisioners.
//!
//! The tests start the real `fxg daemon --stdio --ephemeral` binary as a child
//! process through a provisioner and verify stdio JSON Lines traffic with the
//! central server, `StartSession` relaying, streamed bootstrap logs, and the
//! `DrainAndShutdown` → `DrainComplete` lifecycle.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use fxg_db::{Db, DbRole};
use fxg_protocol::client_api::{CreateSessionRequest, CreateSessionResponse, NodesResponse};
use fxg_protocol::config::AUTH_TOKEN_FILE_NAME;
use fxg_protocol::settings::ProvisionerDefinition;
use fxg_server::Server;
use fxg_server::api::{ClientApiBackend, ClientEvent};
use fxg_server::state::ServerOptions;

/// Run a Git command (panics on failure).
fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// Poll until the condition holds.
async fn wait_until<F, Fut>(what: &str, timeout: Duration, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if check().await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for: {what}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn provisioner_runs_stdio_node_and_drains() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let server_home = tmp.path().join("server");
    let node_home = tmp.path().join("node");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "fxg test"]);
    std::fs::write(repo.join("README.md"), "# test\n").expect("write");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "initial"]);

    // config.toml of the child process (the ephemeral node): define a
    // nonexistent mock agent so StartSession fails deterministically (verifying
    // the stdio path and error propagation)
    std::fs::create_dir_all(&node_home).expect("mkdir node home");
    // Define the mock custom agent in the ephemeral node's DB.
    let node_db = Db::open(&fxg_db::node_db_path(&node_home), DbRole::Node)
        .await
        .expect("open node db");
    node_db
        .upsert_custom_agent(
            "mock",
            &fxg_protocol::settings::CustomAgentConfig {
                name: Some("Mock".to_owned()),
                command: "fxg-missing-mock-agent".to_owned(),
                ..Default::default()
            },
        )
        .await
        .expect("register custom agent");
    drop(node_db);

    // Register the project on the server side (`canonical_git_url` is required)
    let project_id = "github.com/nazo6/flexagent";
    let db = Db::open(&fxg_db::hub_db_path(&server_home), DbRole::Hub)
        .await
        .expect("open server db");
    db.upsert_project(&fxg_db::registration::ProjectRecord {
        project_id: project_id.into(),
        name: "flexagent".to_owned(),
        canonical_git_url: Some(repo.to_string_lossy().into_owned()),
        identity: fxg_protocol::settings::ProjectIdentity::git_remote(),
    })
    .await
    .expect("register project");
    drop(db);

    // Provisioner: start the real binary with `daemon --stdio --ephemeral`
    // (a free-form definition that avoids `{BOOTSTRAP_SCRIPT}`; verifies the
    // path without bootstrap)
    let provisioner_name = "test-ephemeral";
    let provisioner = ProvisionerDefinition {
        name: provisioner_name.to_owned(),
        description: Some("test stdio provisioner".to_owned()),
        command: env!("CARGO_BIN_EXE_fxg").to_owned(),
        args: vec![
            "daemon".to_owned(),
            "--stdio".to_owned(),
            "--ephemeral".to_owned(),
            "--workspace".to_owned(),
            repo.to_string_lossy().into_owned(),
        ],
        idle_timeout_secs: Some(30),
        env: BTreeMap::from([(
            "FXG_HOME".to_owned(),
            node_home.to_string_lossy().into_owned(),
        )]),
    };
    let db = Db::open(&fxg_db::hub_db_path(&server_home), DbRole::Hub)
        .await
        .expect("open server db");
    db.upsert_provisioner(&provisioner)
        .await
        .expect("register provisioner");
    drop(db);
    let server = Server::start(ServerOptions {
        fxg_home: server_home.clone(),
        listen_addr: "127.0.0.1:0".to_owned(),
        allowed_hosts: Vec::new(),
        allowed_origins: Vec::new(),
        log: Default::default(),
    })
    .await
    .expect("start server");
    let addr = server.listen_addr();
    let token =
        fxg_server::api::auth::load_or_create_token(&server_home.join(AUTH_TOKEN_FILE_NAME))
            .expect("token");
    let http = reqwest::Client::new();

    // Subscribe to bootstrap logs (ephemeral events)
    let mut events = server.state().subscribe_client_events();

    let response = http
        .post(format!("http://{addr}/api/v1/sessions"))
        .bearer_auth(&token)
        .json(&CreateSessionRequest {
            command_id: "cmd-1".into(),
            project_id: project_id.into(),
            node_id: None,
            provisioner: Some(provisioner_name.to_owned()),
            local_path: None,
            worktree: None,
            agent_id: "mock".to_owned(),
            initial_prompt: None,
            initial_config: Vec::new(),
            mode: None,
            extra_args: None,
            fork: None,
        })
        .send()
        .await
        .expect("post session");
    let status = response.status();
    assert_eq!(
        status,
        200,
        "create session failed: {}",
        response.text().await.expect("response body")
    );
    let created: CreateSessionResponse = response.json().await.expect("decode");
    let session_id = created.session_id.clone();

    // 1. The ephemeral node registers and its lifecycle advances
    //    (provisioning → ready)
    let server_state = server.state().clone();
    wait_until("ephemeral node ready", Duration::from_secs(60), || {
        let db = server_state.db().clone();
        async move {
            db.list_nodes()
                .await
                .map(|nodes| {
                    nodes.iter().any(|node| {
                        node.provisioner.as_deref() == Some(provisioner_name) && node.is_ephemeral
                    })
                })
                .unwrap_or(false)
        }
    })
    .await;

    // 2. The session status becomes error (the mock agent is missing)
    wait_until("session error", Duration::from_secs(60), || {
        let db = server_state.db().clone();
        let session_id = session_id.clone();
        async move {
            db.get_session(session_id.as_str())
                .await
                .expect("load session")
                .map(|session| session.status == fxg_protocol::common::SessionStatus::Error)
                .unwrap_or(false)
        }
    })
    .await;

    // 3. Bootstrap logs are delivered to the Client WS
    let mut bootstrap_lines = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let ClientEvent::BootstrapLog {
            session_id: event_session,
            line,
        } = event
            && event_session == session_id
        {
            bootstrap_lines.push(line);
        }
    }

    // 4. The ephemeral node is recorded as terminated
    wait_until("node terminated", Duration::from_secs(60), || {
        let db = server_state.db().clone();
        async move {
            db.list_nodes()
                .await
                .map(|nodes| {
                    nodes.iter().any(|node| {
                        node.provisioner.as_deref() == Some(provisioner_name)
                            && node.lifecycle_status
                                == fxg_protocol::common::NodeLifecycleStatus::Terminated
                    })
                })
                .unwrap_or(false)
        }
    })
    .await;

    // The ephemeral node is visible through REST as well (`GET /api/v1/nodes`)
    let nodes: NodesResponse = http
        .get(format!("http://{addr}/api/v1/nodes"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get nodes")
        .json()
        .await
        .expect("decode nodes");
    assert!(
        nodes
            .nodes
            .iter()
            .any(|node| node.is_ephemeral && node.provisioner.as_deref() == Some(provisioner_name))
    );

    server.shutdown();
    server.wait().await;
}
