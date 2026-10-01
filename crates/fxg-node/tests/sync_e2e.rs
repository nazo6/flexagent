//! 中央サーバー + 複数ノードの E2E テスト。
//!
//! 検証項目 (Phase 4 完了条件):
//! - サーバー停止中にノードで発生したイベントが、起動・再接続時に欠落・重複なく
//!   同期される (NodeHello → ResyncRequest → EventBatchPush → EventBatchAck)
//! - 中央サーバー API 経由のセッション操作・プロンプト送信が動作する
//! - 承認応答の二重送信が `ALREADY_RESOLVED` (200 `already_resolved`) で冪等になる
//! - キルスイッチが全ノードへ配信され、監査ログが server.db / node.db 双方に残る
//! - ノード切断中のコマンドは `NODE_OFFLINE` として即時返却される

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use fxg_acp::{
    ActiveSessionHandle, AgentDriver, AgentLaunchSpec, DriverEvent, StartSessionRequest,
    StartedSession,
};
use fxg_db::{Db, DbRole};
use fxg_node::daemon::{DaemonConfig, NodeDaemon};
use fxg_node::session_manager::{DriverFactory, StartSessionParams};
use fxg_protocol::common::{ErrorCode, PermissionOption, SessionStatus, StreamDeltaPayload};
use fxg_protocol::events::UnifiedEventPayload;
use fxg_server::Server;
use fxg_server::api::auth::{generate_token, hash_token};
use fxg_server::state::ServerOptions;
use serde_json::json;
use tokio::sync::mpsc;

// ----------------------------------------------------------------------
// モックエージェントドライバ
// ----------------------------------------------------------------------

/// テスト用モックドライバ (プロセスを起動せず、送信内容を記録して
/// テストから任意のドライバ/エージェントイベントを注入できる)。
#[derive(Clone, Default)]
struct MockAgent {
    inner: Arc<MockInner>,
}

#[derive(Default)]
struct MockInner {
    prompts: Mutex<Vec<String>>,
    permissions: Mutex<Vec<(String, String)>>,
    events: Mutex<Vec<mpsc::UnboundedSender<DriverEvent>>>,
    shutdowns: AtomicU32,
}

impl MockAgent {
    fn emit(&self, event: DriverEvent) {
        for tx in self.inner.events.lock().expect("events").iter() {
            let _ = tx.send(event.clone());
        }
    }

    /// 稼働中セッションのイベントチャネルを閉じる (ドライバ終了の再現)。
    fn close_events(&self) {
        self.inner.events.lock().expect("events").clear();
    }

    fn prompts(&self) -> Vec<String> {
        self.inner.prompts.lock().expect("prompts").clone()
    }

    fn permissions(&self) -> Vec<(String, String)> {
        self.inner.permissions.lock().expect("permissions").clone()
    }

    fn shutdowns(&self) -> u32 {
        self.inner.shutdowns.load(Ordering::Relaxed)
    }

    fn factory(&self) -> DriverFactory {
        let agent = self.clone();
        Arc::new(move |_spec: &AgentLaunchSpec| Ok(Arc::new(agent.clone()) as Arc<dyn AgentDriver>))
    }
}

#[async_trait]
impl AgentDriver for MockAgent {
    fn driver_kind(&self) -> &'static str {
        "mock"
    }

    async fn start_session(
        &self,
        req: StartSessionRequest,
        event_tx: mpsc::UnboundedSender<DriverEvent>,
    ) -> anyhow::Result<StartedSession> {
        self.inner.events.lock().expect("events").push(event_tx);
        // E2E ではモックエージェントは常にネイティブ復元に対応する
        let context_restored = req
            .resume
            .as_ref()
            .is_some_and(|resume| resume.agent_session_id.is_some());
        Ok(StartedSession {
            handle: Box::new(self.clone()),
            context_restored,
        })
    }
}

#[async_trait]
impl ActiveSessionHandle for MockAgent {
    async fn send_prompt(&self, text: String) -> anyhow::Result<()> {
        self.inner.prompts.lock().expect("prompts").push(text);
        Ok(())
    }

    async fn respond_permission(
        &self,
        request_id: String,
        selected_option_id: String,
    ) -> anyhow::Result<()> {
        self.inner
            .permissions
            .lock()
            .expect("permissions")
            .push((request_id, selected_option_id));
        Ok(())
    }

    async fn set_mode(&self, _mode_id: String) -> anyhow::Result<()> {
        Ok(())
    }

    async fn set_config(&self, _key: String, _value: serde_json::Value) -> anyhow::Result<()> {
        Ok(())
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.inner.shutdowns.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

// ----------------------------------------------------------------------
// テストヘルパー
// ----------------------------------------------------------------------

/// 未使用ポートを予約する (テスト内でサーバー再起動に使う固定ポート)。
fn reserve_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

/// テスト用ローカルIPCエンドポイント (テスト間で衝突しない名前)。
fn test_ipc_endpoint(dir: &Path) -> String {
    if cfg!(windows) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(r"\\.\pipe\fxg-e2e-{}-{n}", std::process::id())
    } else {
        dir.join("daemon.sock").to_string_lossy().into_owned()
    }
}

/// テスト用 Git リポジトリを作成する (1コミット)。
async fn init_repo(dir: &Path) {
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "fxg test"],
    ] {
        fxg_node::git::run_git(dir, &args).await.expect("git init");
    }
    std::fs::write(dir.join("README.md"), "# test\n").expect("write");
    fxg_node::git::run_git(dir, &["add", "-A"])
        .await
        .expect("add");
    fxg_node::git::run_git(dir, &["commit", "-q", "-m", "initial"])
        .await
        .expect("commit");
}

/// ノードの `config.toml` にモックエージェント定義を書き込む。
fn write_mock_agent_config(fxg_home: &Path) {
    std::fs::create_dir_all(fxg_home).expect("mkdir home");
    std::fs::write(
        fxg_home.join("config.toml"),
        "[agents.custom.mock]\nname = \"Mock Agent\"\ncommand = \"mock-agent\"\n",
    )
    .expect("write config");
}

/// テストノードを起動する。
async fn start_node(
    home: &Path,
    node_id: &str,
    server_url: Option<String>,
    node_token: Option<String>,
    agent: &MockAgent,
) -> NodeDaemon {
    write_mock_agent_config(home);
    let mut config = DaemonConfig::new(home.to_path_buf(), node_id, "E2E Node");
    config.listen_addr = "127.0.0.1:0".to_owned();
    config.ipc_endpoint = test_ipc_endpoint(home);
    config.central_server_url = server_url;
    config.node_token = node_token;
    NodeDaemon::start_with_driver_factory(config, agent.factory())
        .await
        .expect("start node")
}

/// 中央サーバーを起動する。
async fn start_server(home: &Path, port: u16) -> Server {
    Server::start(ServerOptions {
        fxg_home: home.to_path_buf(),
        listen_addr: format!("127.0.0.1:{port}"),
        allowed_hosts: Vec::new(),
        allowed_origins: Vec::new(),
        provisioners: Default::default(),
        git_credentials: Default::default(),
    })
    .await
    .expect("start server")
}

/// サーバーDB (`server.db`) を開き、ノード個別トークンを発行する。
async fn issue_node_token(server_home: &Path, node_id: &str) -> String {
    let db = Db::open(&fxg_db::hub_db_path(server_home), DbRole::Hub)
        .await
        .expect("open server db");
    db.upsert_node(&fxg_db::NodeRecord::new(
        node_id, node_id, "test", "test", "0.1.0",
    ))
    .await
    .expect("register node");
    let token = generate_token();
    db.set_node_token_hash(node_id, Some(&hash_token(&token)))
        .await
        .expect("issue token");
    token
}

/// 条件が満たされるまでポーリングする (タイムアウトで panic)。
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// ノード上でセッションを作成し、数件のイベントを記録する。
async fn create_session_with_events(
    daemon: &NodeDaemon,
    agent: &MockAgent,
    repo: &Path,
    label: &str,
) -> String {
    let session_id = fxg_protocol::util::uuid_v7();
    daemon
        .state()
        .session_manager()
        .start_session(StartSessionParams {
            command_id: &format!("start-{label}"),
            session_id: &session_id,
            local_path: repo,
            agent_id: "mock",
            initial_prompt: None,
            mode: None,
            opencode_mode: None,
            extra_args: None,
            fork_context: None,
            restore_git_bundle: None,
        })
        .await
        .expect("start session");
    daemon
        .state()
        .session_manager()
        .send_prompt(&format!("prompt-{label}"), &session_id, label, "cli")
        .await
        .expect("send prompt");
    // 完成イベント (AgentMessage) とストリーミング差分を注入する
    agent.emit(DriverEvent::Delta(StreamDeltaPayload::AgentMessageDelta {
        message_id: "m1".to_owned(),
        text_delta: "po".to_owned(),
    }));
    agent.emit(DriverEvent::Event(UnifiedEventPayload::AgentMessage {
        message_id: "m1".to_owned(),
        text: format!("pong {label}"),
        is_complete: true,
    }));
    session_id
}

// ----------------------------------------------------------------------
// E2E テスト 1: オフライン蓄積 → 再接続同期 (2 ノード)
// ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn offline_events_sync_without_loss_or_duplication() {
    let server_home = tempfile::tempdir().expect("server home");
    let node_a_home = tempfile::tempdir().expect("node a home");
    let node_b_home = tempfile::tempdir().expect("node b home");
    let repo_a = tempfile::tempdir().expect("repo a");
    let repo_b = tempfile::tempdir().expect("repo b");
    init_repo(repo_a.path()).await;
    init_repo(repo_b.path()).await;

    let port = reserve_port();
    let server_url = format!("ws://127.0.0.1:{port}/api/v1/node/ws");
    let token_a = issue_node_token(server_home.path(), "node-a").await;
    let token_b = issue_node_token(server_home.path(), "node-b").await;

    let agent_a = MockAgent::default();
    let agent_b = MockAgent::default();

    // 1. サーバー停止中にノード A を起動し、イベントを蓄積する
    let node_a = start_node(
        node_a_home.path(),
        "node-a",
        Some(server_url.clone()),
        Some(token_a),
        &agent_a,
    )
    .await;
    let session_a = create_session_with_events(&node_a, &agent_a, repo_a.path(), "offline").await;

    // 2. 中央サーバーを起動し、ノード B を接続する
    let server = start_server(server_home.path(), port).await;
    let node_b = start_node(
        node_b_home.path(),
        "node-b",
        Some(server_url.clone()),
        Some(token_b),
        &agent_b,
    )
    .await;
    let session_b = create_session_with_events(&node_b, &agent_b, repo_b.path(), "online").await;

    // 3. 両ノードの接続と同期完了を待つ
    wait_until("nodes connected", Duration::from_secs(30), || async {
        let hub = server.state().hub();
        hub.is_online("node-a").await && hub.is_online("node-b").await
    })
    .await;

    let server_db = Db::open(&fxg_db::hub_db_path(server_home.path()), DbRole::Hub)
        .await
        .expect("server db");
    let node_a_db = Db::open(&fxg_db::node_db_path(node_a_home.path()), DbRole::Node)
        .await
        .expect("node a db");
    let node_b_db = Db::open(&fxg_db::node_db_path(node_b_home.path()), DbRole::Node)
        .await
        .expect("node b db");

    let node_a_last = node_a_db
        .get_session(&session_a)
        .await
        .expect("get a")
        .expect("session a")
        .last_node_seq;
    let node_b_last = node_b_db
        .get_session(&session_b)
        .await
        .expect("get b")
        .expect("session b")
        .last_node_seq;
    assert!(node_a_last >= 3, "offline session must have events");

    wait_until("session a synced", Duration::from_secs(30), || async {
        server_db
            .get_session(&session_a)
            .await
            .ok()
            .flatten()
            .map(|session| session.last_node_seq >= node_a_last)
            .unwrap_or(false)
    })
    .await;
    wait_until("session b synced", Duration::from_secs(30), || async {
        server_db
            .get_session(&session_b)
            .await
            .ok()
            .flatten()
            .map(|session| session.last_node_seq >= node_b_last)
            .unwrap_or(false)
    })
    .await;

    // 4. 欠落・重複なしを検証する (イベント数 == 最大 node_seq)
    let count_events = |db: Db, session_id: String| async move {
        let mut cursor = 0u64;
        let mut count = 0usize;
        loop {
            let batch = db
                .session_events_after(&session_id, cursor, 500)
                .await
                .expect("events");
            if batch.events.is_empty() {
                break;
            }
            count += batch.events.len();
            cursor = batch.cursor;
        }
        count
    };
    let node_a_count = count_events(node_a_db.clone(), session_a.clone()).await;
    let server_a_count = count_events(server_db.clone(), session_a.clone()).await;
    assert_eq!(server_a_count, node_a_count, "no loss / no duplication (A)");
    assert_eq!(
        node_a_count, node_a_last as usize,
        "node_seq has no gaps (A)"
    );
    let node_b_count = count_events(node_b_db.clone(), session_b.clone()).await;
    let server_b_count = count_events(server_db.clone(), session_b.clone()).await;
    assert_eq!(server_b_count, node_b_count, "no loss / no duplication (B)");

    // 5. 水位 (ACK) がノード側で進んでいる
    wait_until("watermarks acked", Duration::from_secs(30), || async {
        let node_a_sync = node_a_db.unsynced_event_count().await.unwrap_or(1);
        let node_b_sync = node_b_db.unsynced_event_count().await.unwrap_or(1);
        node_a_sync == 0 && node_b_sync == 0
    })
    .await;

    // 6. サーバー側のノード状態 (オンライン・エージェント一覧) を確認する
    let nodes = server_db.list_nodes().await.expect("nodes");
    assert_eq!(nodes.len(), 2);
    assert!(nodes.iter().all(|node| node.is_online));

    // 7. セッションのストリーミング差分が LiveStreamDelta として配信済みである
    //    (永続化されないため、イベント数には含まれない)
    //    — ここではモックが差分を受け取り済みであることのみ確認する
    assert!(agent_a.prompts().contains(&"offline".to_owned()));

    node_a.shutdown();
    node_b.shutdown();
    tokio::time::timeout(Duration::from_secs(10), node_a.wait())
        .await
        .expect("node a stop");
    tokio::time::timeout(Duration::from_secs(10), node_b.wait())
        .await
        .expect("node b stop");
    server.shutdown();
    tokio::time::timeout(Duration::from_secs(10), server.wait())
        .await
        .expect("server stop");
}

// ----------------------------------------------------------------------
// E2E テスト 2: 中央サーバー経由のコマンド・承認・キルスイッチ
// ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn central_server_commands_permissions_and_kill_switch() {
    let server_home = tempfile::tempdir().expect("server home");
    let node_home = tempfile::tempdir().expect("node home");
    let repo = tempfile::tempdir().expect("repo");
    init_repo(repo.path()).await;

    let port = reserve_port();
    let server_url = format!("ws://127.0.0.1:{port}/api/v1/node/ws");
    let node_token = issue_node_token(server_home.path(), "node-1").await;
    let agent = MockAgent::default();

    let server = start_server(server_home.path(), port).await;
    let node = start_node(
        node_home.path(),
        "node-1",
        Some(server_url),
        Some(node_token),
        &agent,
    )
    .await;
    wait_until("node connected", Duration::from_secs(30), || async {
        server.state().hub().is_online("node-1").await
    })
    .await;

    let server_db = Db::open(&fxg_db::hub_db_path(server_home.path()), DbRole::Hub)
        .await
        .expect("server db");
    let node_db = Db::open(&fxg_db::node_db_path(node_home.path()), DbRole::Node)
        .await
        .expect("node db");
    let client_token = fxg_server::api::auth::load_or_create_token(
        &server_home
            .path()
            .join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
    )
    .expect("client token");
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // 1. ノード上でプロジェクトを登録し、中央サーバーへバインドを同期させる
    let resolved = node
        .state()
        .resolve_and_register_project(repo.path())
        .await
        .expect("resolve project");
    let project_id = resolved.project_id.clone();
    wait_until(
        "project binding synced",
        Duration::from_secs(30),
        || async {
            server_db
                .list_projects()
                .await
                .map(|projects| {
                    projects.iter().any(|project| {
                        project.project_id == project_id
                            && project
                                .bindings
                                .iter()
                                .any(|binding| binding.node_id == "node-1")
                    })
                })
                .unwrap_or(false)
        },
    )
    .await;

    // 2. 中央サーバーの REST API から新規セッションを起動する
    let create = http
        .post(format!("{base}/api/v1/sessions"))
        .bearer_auth(&client_token)
        .json(&json!({
            "command_id": "create-1",
            "project_id": project_id,
            "node_id": "node-1",
            "agent_id": "mock",
            "initial_prompt": "hello from central",
        }))
        .send()
        .await
        .expect("create session");
    assert_eq!(create.status(), reqwest::StatusCode::OK, "create session");
    let created: serde_json::Value = create.json().await.expect("json");
    let session_id = created["session_id"]
        .as_str()
        .expect("session id")
        .to_owned();

    wait_until(
        "session visible on server",
        Duration::from_secs(30),
        || async {
            server_db
                .get_session(&session_id)
                .await
                .ok()
                .flatten()
                .is_some()
        },
    )
    .await;
    // 初期プロンプトがエージェントへ届く
    wait_until(
        "initial prompt dispatched",
        Duration::from_secs(30),
        || async { agent.prompts().iter().any(|p| p == "hello from central") },
    )
    .await;
    // ターン完了 (idle) を注入して後続プロンプトを即時 dispatch できるようにする
    agent.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Idle,
        error_message: None,
    }));
    wait_until("session idle", Duration::from_secs(30), || async {
        node_db
            .get_session(&session_id)
            .await
            .ok()
            .flatten()
            .map(|session| session.status == SessionStatus::Idle)
            .unwrap_or(false)
    })
    .await;

    // 3. 承認リクエストを発行し、二重応答の冪等性を検証する
    agent.emit(DriverEvent::Event(UnifiedEventPayload::PermissionRequest {
        request_id: "req-1".to_owned(),
        tool_name: "terminal/create".to_owned(),
        summary: "Run: cargo test".to_owned(),
        options: vec![
            PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow".to_owned(),
                kind: "allow_once".to_owned(),
            },
            PermissionOption {
                option_id: "allow_always".to_owned(),
                name: "Allow always".to_owned(),
                kind: "allow_always".to_owned(),
            },
        ],
        details: json!({ "command": "cargo test" }),
    }));
    wait_until(
        "permission visible in inbox",
        Duration::from_secs(30),
        || async {
            server_db
                .pending_permissions()
                .await
                .map(|requests| {
                    requests.iter().any(|request| {
                        request.session_id == session_id && request.request_id == "req-1"
                    })
                })
                .unwrap_or(false)
        },
    )
    .await;

    let respond_url = format!("{base}/api/v1/sessions/{session_id}/permissions/req-1/respond");
    let first = http
        .post(&respond_url)
        .bearer_auth(&client_token)
        .json(&json!({
            "selected_option_id": "allow_once",
            "always": true, // allow_always へ昇格される
            "resolved_by": "web",
        }))
        .send()
        .await
        .expect("first respond");
    assert_eq!(first.status(), reqwest::StatusCode::OK);
    let first_body: serde_json::Value = first.json().await.expect("json");
    assert_eq!(first_body["already_resolved"], false);

    let second = http
        .post(&respond_url)
        .bearer_auth(&client_token)
        .json(&json!({
            "selected_option_id": "allow_once",
            "always": false,
            "resolved_by": "android_push",
        }))
        .send()
        .await
        .expect("second respond");
    assert_eq!(second.status(), reqwest::StatusCode::OK);
    let second_body: serde_json::Value = second.json().await.expect("json");
    assert_eq!(second_body["already_resolved"], true, "idempotent resolve");

    // エージェントへは 1 回だけ応答される (allow_always へ昇格)
    wait_until(
        "permission delivered once",
        Duration::from_secs(30),
        || async { agent.permissions() == vec![("req-1".to_owned(), "allow_always".to_owned())] },
    )
    .await;

    // 4. Client WS (中央サーバー) からプロンプトを送信する
    {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let mut request = format!("ws://127.0.0.1:{port}/api/v1/client/ws")
            .into_client_request()
            .expect("ws request");
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {client_token}").parse().expect("header"),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("client ws");
        let send = fxg_protocol::client_api::ClientWsMessage::SendPrompt {
            command_id: "prompt-1".to_owned(),
            session_id: session_id.clone(),
            text: "via ws".to_owned(),
            client_source: "web".to_owned(),
        };
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&send).expect("json").into(),
            ))
            .await
            .expect("send prompt");

        // CommandResult が返るまで受信する (リプレイの EventBatch 等を読み飛ばす)
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            use futures_util::StreamExt;
            while let Some(message) = socket.next().await {
                let text = message.expect("ws message");
                let text = match text {
                    WsMessage::Text(text) => text,
                    _ => continue,
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if value["op"] == "command_result" && value["command_id"] == "prompt-1" {
                    return value;
                }
            }
            panic!("client ws closed before CommandResult");
        })
        .await
        .expect("CommandResult timeout");
        assert_eq!(result["success"], true, "ws send prompt: {result}");
    }
    wait_until("ws prompt dispatched", Duration::from_secs(30), || async {
        agent.prompts().iter().any(|p| p == "via ws")
    })
    .await;

    // 5. キルスイッチ: 全ノードへ配信され、両DBのセッションが stopped になる
    let kill = http
        .post(format!("{base}/api/v1/system/kill-switch"))
        .bearer_auth(&client_token)
        .json(&json!({ "reason": "e2e panic button" }))
        .send()
        .await
        .expect("kill switch");
    assert_eq!(kill.status(), reqwest::StatusCode::OK);
    let kill_body: serde_json::Value = kill.json().await.expect("json");
    assert_eq!(kill_body["notified_nodes"], 1);

    wait_until(
        "session stopped on node",
        Duration::from_secs(30),
        || async {
            node_db
                .get_session(&session_id)
                .await
                .ok()
                .flatten()
                .map(|session| session.status == SessionStatus::Stopped)
                .unwrap_or(false)
        },
    )
    .await;
    assert!(
        agent.shutdowns() >= 1,
        "agent process tree must be shut down"
    );
    wait_until(
        "session stopped on server",
        Duration::from_secs(30),
        || async {
            server_db
                .get_session(&session_id)
                .await
                .ok()
                .flatten()
                .map(|session| session.status == SessionStatus::Stopped)
                .unwrap_or(false)
        },
    )
    .await;

    // 6. 監査ログが server.db / node.db 双方に記録されている
    let server_audits = server_db.audit_logs(100).await.expect("audit logs");
    let actions: Vec<&str> = server_audits
        .iter()
        .map(|entry| entry.action.as_str())
        .collect();
    assert!(
        actions.contains(&"session_start"),
        "server audit: {actions:?}"
    );
    assert!(
        actions.contains(&"permission_resolved"),
        "server audit: {actions:?}"
    );
    assert!(
        actions.contains(&"kill_switch"),
        "server audit: {actions:?}"
    );

    let node_audits = node_db.audit_logs(100).await.expect("audit logs");
    let node_actions: Vec<&str> = node_audits
        .iter()
        .map(|entry| entry.action.as_str())
        .collect();
    assert!(
        node_actions.contains(&"permission_resolved"),
        "node audit: {node_actions:?}"
    );
    assert!(
        node_actions.contains(&"kill_switch"),
        "node audit: {node_actions:?}"
    );

    // 7. ノード停止中のコマンドは NODE_OFFLINE で即時返却される
    node.shutdown();
    tokio::time::timeout(Duration::from_secs(10), node.wait())
        .await
        .expect("node stop");
    wait_until("node offline", Duration::from_secs(30), || async {
        !server.state().hub().is_online("node-1").await
    })
    .await;

    {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let mut request = format!("ws://127.0.0.1:{port}/api/v1/client/ws")
            .into_client_request()
            .expect("ws request");
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {client_token}").parse().expect("header"),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("client ws");
        let send = fxg_protocol::client_api::ClientWsMessage::SendPrompt {
            command_id: "prompt-offline".to_owned(),
            session_id: session_id.clone(),
            text: "nobody home".to_owned(),
            client_source: "web".to_owned(),
        };
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&send).expect("json").into(),
            ))
            .await
            .expect("send prompt");

        let result = tokio::time::timeout(Duration::from_secs(30), async {
            use futures_util::StreamExt;
            while let Some(message) = socket.next().await {
                let text = message.expect("ws message");
                let text = match text {
                    WsMessage::Text(text) => text,
                    _ => continue,
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if value["op"] == "command_result" && value["command_id"] == "prompt-offline" {
                    return value;
                }
            }
            panic!("client ws closed before CommandResult");
        })
        .await
        .expect("CommandResult timeout");
        assert_eq!(result["success"], false);
        assert_eq!(result["code"], ErrorCode::NodeOffline.as_str());
    }

    server.shutdown();
    tokio::time::timeout(Duration::from_secs(10), server.wait())
        .await
        .expect("server stop");
}

// ----------------------------------------------------------------------
// E2E テスト: 中央サーバー経由の停止済みセッション再開 (resume)
// ----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn central_server_resumes_stopped_session() {
    let server_home = tempfile::tempdir().expect("server home");
    let node_home = tempfile::tempdir().expect("node home");
    let repo = tempfile::tempdir().expect("repo");
    init_repo(repo.path()).await;

    let port = reserve_port();
    let server_url = format!("ws://127.0.0.1:{port}/api/v1/node/ws");
    let node_token = issue_node_token(server_home.path(), "node-1").await;
    let agent = MockAgent::default();

    let server = start_server(server_home.path(), port).await;
    let node = start_node(
        node_home.path(),
        "node-1",
        Some(server_url),
        Some(node_token),
        &agent,
    )
    .await;
    wait_until("node connected", Duration::from_secs(30), || async {
        server.state().hub().is_online("node-1").await
    })
    .await;

    let server_db = Db::open(&fxg_db::hub_db_path(server_home.path()), DbRole::Hub)
        .await
        .expect("server db");
    let node_db = Db::open(&fxg_db::node_db_path(node_home.path()), DbRole::Node)
        .await
        .expect("node db");
    let client_token = fxg_server::api::auth::load_or_create_token(
        &server_home
            .path()
            .join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME),
    )
    .expect("client token");
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // 1. ノード上でセッションを開始し、エージェント側セッションIDを確定させる
    node.state()
        .resolve_and_register_project(repo.path())
        .await
        .expect("resolve project");
    let session_id = fxg_protocol::util::uuid_v7();
    node.state()
        .session_manager()
        .start_session(StartSessionParams {
            command_id: "start-resume",
            session_id: &session_id,
            local_path: repo.path(),
            agent_id: "mock",
            initial_prompt: None,
            mode: None,
            opencode_mode: None,
            extra_args: None,
            fork_context: None,
            restore_git_bundle: None,
        })
        .await
        .expect("start session");
    node.state()
        .session_manager()
        .send_prompt("prompt-resume", &session_id, "hello resume e2e", "cli")
        .await
        .expect("prompt");
    agent.emit(DriverEvent::Event(UnifiedEventPayload::SessionAgentBound {
        agent_session_id: "agent-sess-e2e".to_owned(),
    }));
    agent.emit(DriverEvent::Event(UnifiedEventPayload::StatusChanged {
        status: SessionStatus::Idle,
        error_message: None,
    }));
    wait_until("session synced to hub", Duration::from_secs(30), || async {
        server_db
            .get_session(&session_id)
            .await
            .ok()
            .flatten()
            .is_some_and(|session| {
                session.agent_session_id.as_deref() == Some("agent-sess-e2e")
                    && session.status == SessionStatus::Idle
            })
    })
    .await;

    // 2. ドライバ終了でセッションを停止させ、ハブへ `stopped` が同期されるまで待つ
    agent.close_events();
    wait_until("stopped synced to hub", Duration::from_secs(30), || async {
        server_db
            .get_session(&session_id)
            .await
            .ok()
            .flatten()
            .is_some_and(|session| session.status == SessionStatus::Stopped)
    })
    .await;

    // 3. 中央サーバー REST から再開する (モックはネイティブ復元対応)
    let response = http
        .post(format!("{base}/api/v1/sessions/{session_id}/resume"))
        .bearer_auth(&client_token)
        .json(&json!({ "force_replay": false }))
        .send()
        .await
        .expect("resume request");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "resume session request must succeed"
    );
    let resumed: serde_json::Value = response.json().await.expect("json");
    assert_eq!(resumed["session_id"].as_str(), Some(session_id.as_str()));
    assert_eq!(
        resumed["context_restored"].as_bool(),
        Some(true),
        "mock agent restores natively via SessionAgentBound"
    );

    // 4. 再開後は idle へ復帰し、継続プロンプトを送信できる
    wait_until("resumed session idle", Duration::from_secs(30), || async {
        node_db
            .get_session(&session_id)
            .await
            .ok()
            .flatten()
            .is_some_and(|session| session.status == SessionStatus::Idle)
    })
    .await;
    node.state()
        .session_manager()
        .send_prompt(
            "prompt-resume-2",
            &session_id,
            "continue after resume",
            "cli",
        )
        .await
        .expect("continuation prompt");
    assert_eq!(
        agent.prompts(),
        vec![
            "hello resume e2e".to_owned(),
            "continue after resume".to_owned()
        ],
        "native resume must not inject replay context"
    );

    // 5. 稼働中セッションへの再実行は INVALID_STATE (400)
    let again = http
        .post(format!("{base}/api/v1/sessions/{session_id}/resume"))
        .bearer_auth(&client_token)
        .json(&json!({ "force_replay": false }))
        .send()
        .await
        .expect("second resume");
    assert_eq!(again.status(), reqwest::StatusCode::BAD_REQUEST);

    node.shutdown();
    tokio::time::timeout(Duration::from_secs(10), node.wait())
        .await
        .expect("node stop");
    server.shutdown();
    tokio::time::timeout(Duration::from_secs(10), server.wait())
        .await
        .expect("server stop");
}
