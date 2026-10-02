//! Phase 6: 一時VMプロビジョナーの E2E テスト。
//!
//! 実バイナリ `fxg daemon --stdio --ephemeral` をプロビジョナーとして子プロセス起動し、
//! 中央サーバーとの stdio JSON Lines 通信・`StartSession` 中継・ブートストラップログの
//! ストリーム配信・セッション終了時の `DrainAndShutdown` → `WorkspaceBundleUpload` →
//! `sessions.git_bundle_path` 保存までを検証する。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use fxg_db::{Db, DbRole};
use fxg_protocol::client_api::{CreateSessionRequest, CreateSessionResponse, NodesResponse};
use fxg_protocol::config::{AUTH_TOKEN_FILE_NAME, ProvisionerConfig};
use fxg_server::Server;
use fxg_server::api::{ClientApiBackend, ClientEvent};
use fxg_server::state::ServerOptions;

/// Git コマンドを実行する (失敗は panic)。
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

/// 条件が満たされるまでポーリングする。
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
async fn provisioner_runs_stdio_node_and_drains_bundle() {
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

    // 子プロセス (一時ノード) の config.toml: 存在しないモックエージェントを定義し、
    // StartSession が確実に失敗するようにする (stdio 経路とエラー伝播の検証)
    std::fs::create_dir_all(&node_home).expect("mkdir node home");
    std::fs::write(
        node_home.join("config.toml"),
        "[agents.custom.mock]\nname = \"Mock\"\ncommand = \"fxg-missing-mock-agent\"\n",
    )
    .expect("write node config");

    // サーバー側にプロジェクトを登録する (`canonical_git_url` が必須)
    let project_id = "github.com/nazo6/flexagent";
    let db = Db::open(&fxg_db::hub_db_path(&server_home), DbRole::Hub)
        .await
        .expect("open server db");
    db.upsert_project(&fxg_db::registration::ProjectRecord {
        project_id: project_id.to_owned(),
        name: "flexagent".to_owned(),
        canonical_git_url: Some(repo.to_string_lossy().into_owned()),
    })
    .await
    .expect("register project");
    drop(db);

    // プロビジョナー: 実バイナリを `daemon --stdio --ephemeral` で起動する
    // (`{BOOTSTRAP_SCRIPT}` を使わない自由形式の定義。ブートストラップなしの経路検証)
    let provisioner = ProvisionerConfig {
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
    let provisioner_name = "test-ephemeral";
    let server = Server::start(ServerOptions {
        fxg_home: server_home.clone(),
        listen_addr: "127.0.0.1:0".to_owned(),
        allowed_hosts: Vec::new(),
        allowed_origins: Vec::new(),
        provisioners: BTreeMap::from([(provisioner_name.to_owned(), provisioner)]),
        git_credentials: BTreeMap::new(),
    })
    .await
    .expect("start server");
    let addr = server.listen_addr();
    let token =
        fxg_server::api::auth::load_or_create_token(&server_home.join(AUTH_TOKEN_FILE_NAME))
            .expect("token");
    let http = reqwest::Client::new();

    // ブートストラップログ (エフェメラルイベント) を購読する
    let mut events = server.state().subscribe_client_events();

    let response = http
        .post(format!("http://{addr}/api/v1/sessions"))
        .bearer_auth(&token)
        .json(&CreateSessionRequest {
            command_id: "cmd-1".to_owned(),
            project_id: project_id.to_owned(),
            node_id: None,
            provisioner: Some(provisioner_name.to_owned()),
            local_path: None,
            worktree: None,
            agent_id: "mock".to_owned(),
            initial_prompt: None,
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
        response.text().await.unwrap_or_default()
    );
    let created: CreateSessionResponse = response.json().await.expect("decode");
    let session_id = created.session_id.clone();

    // 1. 一時ノードが登録され、ライフサイクルが進む (provisioning → ready)
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

    // 2. セッション状態が error になる (モックエージェントが見つからない)
    wait_until("session error", Duration::from_secs(60), || {
        let db = server_state.db().clone();
        let session_id = session_id.clone();
        async move {
            db.get_session(&session_id)
                .await
                .ok()
                .flatten()
                .map(|session| session.status == fxg_protocol::common::SessionStatus::Error)
                .unwrap_or(false)
        }
    })
    .await;

    // 3. ブートストラップログが Client WS へ配信されている
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

    // 4. アイドル/セッション終了監視による Drain → Git バンドル退避
    wait_until("workspace bundle stored", Duration::from_secs(90), || {
        let db = server_state.db().clone();
        let session_id = session_id.clone();
        async move {
            db.get_git_bundle_path(&session_id)
                .await
                .ok()
                .flatten()
                .is_some()
        }
    })
    .await;

    let bundle_path = server_state
        .db()
        .get_git_bundle_path(&session_id)
        .await
        .expect("bundle path query")
        .expect("bundle path");
    let bundle_bytes = std::fs::read(&bundle_path).expect("read bundle");
    assert!(!bundle_bytes.is_empty(), "bundle must not be empty");
    assert!(
        Path::new(&bundle_path).starts_with(server_home.join("bundles")),
        "bundle path must live under the server home: {bundle_path}"
    );

    // 5. 一時ノードは terminated として記録される
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

    // REST でも一時ノードが見える (`GET /api/v1/nodes`)
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
